# Revue de fiabilité, stabilité et intégrité des fichiers — extraction DSM 7.4.1-90080 (DS925+)

| Champ | Valeur |
|---|---|
| Objet | Analyse statique du corpus `DSM_DS925+_90080` et confrontation à HomeVault |
| Corpus | `/media/nicolas/500GB/Projets/DSM7/` — 1,5 Go d'extraction DSM + 937 Mo de source noyau |
| Version DSM | 7.4.1-90080, build 2026/07/22, modèle DS925+ (`unique=synology_v1000nk_ds925+`) |
| Référence HomeVault | 1.0.0, `HEAD=3afe03e` |
| Date | 2026-11-05 |
| Statut | Analyse / proposition. Aucun code n'a été modifié. |
| Méthode | Lecture des configurations, unités systemd, scripts, aide embarquée, chaînes de caractères des binaires ; confrontation ligne à ligne au code `src/` de HomeVault |

---

## 0. Résumé pour décideur

DSM 7.4.1 n'est pas « fiable » parce qu'il ferait une chose magique : il empile
des mécanismes simples, redondants et **vérifiables**. Les quatre piliers utiles
à copier sont :

1. **Le scrub n'est pas un bouton isolé.** Il est planifié, reprend après
   redémarrage, se déclenche après un arrêt sale, se met en pause, et ne
   s'exécute que sur un volume à la fois. DSM sait aussi qu'un scrub sans
   checksums ne peut pas réparer : il refuse ou prévient explicitement.
2. **Le montage a une échelle de secours.** Superbloc Btrfs restauré, journal
   abandonné, montage en lecture seule, superblocs ext4 de secours : un
   superbloc abîmé ne condamne pas le volume.
3. **La sauvegarde est vérifiée, pas seulement écrite.** Hyper Backup tient un
   index de checksums par fichier, propose un contrôle d'intégrité planifié
   (structure + données, avec limite de temps), reprend automatiquement après
   une coupure réseau, et signale une cible corrompue.
4. **Les transferts lents ou instables ont des filets.** SMB3 Multichannel,
   durable handles, leasing, signature, chiffrement de transport ; iSCSI
   FUA/Sync Cache ; NFS `sync` ; reprise HTTP.

HomeVault est déjà **meilleur que DSM** sur le transfert HTTP (upload
reprenable, SHA-256, `fsync`, `rename` atomique, vérification de quota sous
verrou) et sur la sauvegarde de configuration (archive chiffrée, manifeste
SHA-256 par entrée). Il est **en retard** sur sept points qui touchent
directement la perte de données :

| # | Écart | Risque | Priorité |
|---|---|---|---|
| 1 | `verify_checksums` de USB Copy ne vérifie que la taille et l'existence | Un fichier corrompu est déclaré « vérifié » | **Critique** |
| 2 | Aucun scrub planifié ni déclenché après arrêt sale | Corruption silencieuse jusqu'au prochain scrub manuel | **Critique** |
| 3 | Pas de fsck/récupération de montage au démarrage | Un superbloc abîmé laisse le pool démonté, sans remède | **Critique** |
| 4 | Pas de suivi des attributs SMART ni des erreurs I/O | Disque mourant détecté trop tard | **Élevé** |
| 5 | Écritures de configuration non atomiques (`smb.conf`, exports) | Fichier tronqué après coupure = partages absents au boot | **Élevé** |
| 6 | `restic check` non planifié et sans `--read-data-subset` | Une sauvegarde « OK » peut avoir des packs corrompus | **Élevé** |
| 7 | SMB sans durable handles / chiffrement / `strict sync` first-class | Session perdue à la moindre coupure, pas de durabilité explicite | **Élevé** |

Le reste de ce document détaille les preuves, les compromis de DSM qu'il ne
faut **pas** copier, et un plan d'implémentation par lots.

---

## 1. Périmètre, corpus et méthode

### 1.1 Ce que contient réellement le dossier

```
DSM7/
├── DSM_DS925+_90080/          1,5 Go
│   ├── hda1/                  rootfs DSM extrait (bin, etc.defaults, usr/syno, systemd)
│   ├── packages_extracted/    19 paquets SPK (SMBService, HyperBackup, HA, ScsiTarget, …)
│   ├── texts/                 chaînes d'interface DSM (24 langues)
│   ├── indexdb/               index d'aide
│   ├── disk-compatibility/    bases de compatibilité disques
│   ├── bios.ROM, zImage, rd.gz, model.dtb, updater
│   ├── checksum.syno          CRC32 + taille + offset des membres du .pat
│   └── Synology.sig           signature CMS/PKCS#7 des membres
└── sources/7.4-90080/
    └── apollolake/linux-4.4.x.txz   937 Mo de sources noyau (59 722 fichiers)
```

**Limites importantes :**

- Le `.pat` d'origine et les membres `hda1.tgz`, `*.spk`,
  `synohdpack_img.txz`, `DiskCompatibilityDB.tar` **ne sont plus présents**.
  `checksum.syno` ne peut donc plus être rejoué sur les archives d'origine.
- 136 liens symboliques de `hda1/` sont cassés (chemins absolus `/usr/...`),
  ce qui est normal pour un rootfs extrait hors de son contexte.
- Les binaires ne sont pas exécutables ici (bibliothèques manquantes, notamment
  `libboost_regex.so.1.80.0` pour `synofstool`).
- Le source noyau ne contient pas de `.config` ; on ne peut pas conclure sur
  les options de compilation (Btrfs, ext4, md, DRBD…).
- L'analyse est donc **statique** : configurations, scripts, unités systemd,
  aide embarquée et chaînes de caractères. Chaque affirmation est reliée à un
  fichier du corpus en annexe A.

### 1.2 Convention de preuve

Les chemins sont relatifs à `DSM_DS925+_90080/` sauf mention contraire.
Les citations de chaînes de binaires sont identifiées par le binaire et non
par un numéro de ligne (les binaires sont strippés).

---

## 2. Architecture de fiabilité de DSM — ce qui mérite d'être copié

### 2.1 Intégrité au repos

#### Btrfs par défaut, avec checksums et CoW

- `hda1/etc.defaults/synoinfo.conf` : `defaultfs="btrfs"`,
  `btrfs_default_compression="zstd"`, `max_btrfs_snapshots="65536"`,
  `support_btrfs_dedupe="yes"`, `support_iscsi_btrfs_lun="yes"`.
- L'aide `AdminCenter/file_share_create.html` décrit l'option
  **« Enable data checksum for advanced data integrity »** : « use CRC32
  checksum and copy-on-write to protect shared folders », option **non
  modifiable après création**. C'est le point clé : le checksum est lié au
  maintien du CoW, pas un ornement.
- `StorageManager/storage_pool_data_scrubbing.html` précise la conséquence :
  si l'option n'est pas activée sur un dossier partagé, **le scrub ne peut pas
  vérifier ni réparer** les données de ce dossier, faute de checksums.
- `hda1/usr/bin/btrfs-block-csum`, `hda1/usr/syno/sbin/synobtrfssnap`,
  `hda1/usr/syno/sbin/synobtrfsdedupe` montrent que DSM outille le checksum,
  les snapshots et la déduplication au niveau Btrfs.

**À copier :** un dossier partagé doit être créé en subvolume Btrfs **sans
`nodatacow`** ; le scrub doit refuser de prétendre réparer un dossier sans
checksums ; l'interface doit rendre l'option explicite et documenter qu'elle
est immuable.

#### Le piège ext4 de DSM

`hda1/etc.defaults/mke2fs.conf` contient :

```
ext4 = {
    features = has_journal,extent,huge_file,64bit,flex_bg,uninit_bg,^metadata_csum,dir_nlink,extra_isize
    inode_size = 256
}
...
enable_periodic_fsck = 0
```

Deux choix **à ne pas reproduire** :

- `^metadata_csum` **désactive les checksums de métadonnées ext4**. Un bloc
  d'inode ou de bitmap corrompu n'est pas détecté par le filesystem lui-même.
- `enable_periodic_fsck = 0` supprime le fsck périodique. DSM compense par un
  fsck au boot après arrêt sale (voir 2.2), mais ce n'est pas équivalent.

**À copier / à améliorer :** HomeVault doit au contraire **forcer**
`metadata_csum` (et `64bit`, `dir_index`, `extent`, `flex_bg`) dans ses
`mkfs.ext4`, et vérifier le résultat avec `dumpe2fs -h`.

#### Balayage et réparation

`hda1/usr/syno/sbin/synoraidtool` expose :

- `--method=sche-scrubbing --start -p /dev/md2 -a manual` : scrub manuel ;
- `--method=sche-scrubbing --run` : exécution planifiée ;
- `--method=sche-scrubbing --sync` : synchronisation de la configuration ;
- `--method=set --set-resync-speed-by-config` et `--minimize-resync-speed` :
  limitation de la vitesse de resync ;
- `repair specified space with hotspare disks` : réparation par spare.

Les unités systemd associées :

| Unité | Rôle |
|---|---|
| `data-scrubbing@.service` | scrub d'un pool, `synoraidtool -S -p %i`, sans timeout |
| `scrubbing-auto-scrubbing.service` | scrub automatique après arrêt sale / fsck |
| `scrubbing-sche-run.service` | lance la planification après le boot |
| `scrubbing-conf-sync.service` | resynchronise la configuration de scrub |
| `syno-storage-task-resume.service` | reprend les tâches stockage interrompues (`synostgtask --resume --async`) |
| `syno-md-resync-speed-adjust@.service` | applique la vitesse de resync après boot |
| `syno_btrfs_quick_balance.service` | balance Btrfs si le ratio metadata/data dépasse 4 % et que l'espace non alloué tombe sous 5 Gio |

L'aide `storage_pool_data_scrubbing.html` ajoute les règles opérationnelles :

- scrub supporté sur volumes Btrfs, ou pools SHR (≥3 disques), RAID 5, RAID 6,
  RAID F1 ;
- **un seul pool à la fois** ;
- pause/reprise manuelles ;
- un pool dégradé est temporairement retiré de la planification, puis réinséré
  après réparation ; un pool « crashed » en est retiré définitivement ;
- un scrub planifié interrompu par une coupure reprend au redémarrage.

**À copier :** le triptyque « planification + reprise après reboot + scrub
post-arrêt sale », avec **une seule opération lourde à la fois**.

#### Récupération de montage : l'échelle de secours

`hda1/usr/syno/lib/systemd/scripts/volume.sh` est le document le plus utile du
corpus pour la robustesse au boot. La fonction `VolumeRemount` essaie, dans
l'ordre :

1. `btrfs rescue super-recover -y <dev>` puis montage normal ;
2. montage avec `drop_log_tree` (journal Btrfs abandonné) ;
3. montage `ro` ;
4. montage `ro,no_block_group,nologreplay` ;
5. montage `ro,recovery`.

Pour ext4 :

1. `mount -o ro,noload` (ne pas rejouer le journal) ;
2. essai des superblocs de secours
   (`1 131072 393216 655360 917504 1179648 3276800 3538944`).

Le script `VolumeScan` exécute ensuite `e2fsck -C-1 -pvf`, journalise dans
`/var/log/fsck/<volume>.log`, appelle `debugfs` pour produire la liste des
fichiers perdus, et signale le résultat à `synostorage`.

Il existe aussi une **protection du cache d'écriture** : si
`/sys/fs/syno-cache-protection/pool_enable` vaut `1`, DSM ajuste
`vm.dirty_ratio=40`, `vm.dirty_background_ratio=30`,
`vm.dirty_expire_centisecs=4000`, `vm.dirty_writeback_centisecs=500`, puis
rejoue le journal de protection (`spacetool --bootup-replay-start/end`).

**À copier :** une échelle de montage en lecture seule, avec notification,
plutôt qu'un `nofail` silencieux. HomeVault écrit aujourd'hui
`defaults,nofail` (`src/disk.rs:2252`, `src/pools.rs:436`) : le boot ne bloque
pas, mais rien ne dit à l'administrateur que le pool n'est **pas** monté.

#### SMART, usure et prédiction

- `hda1/usr/syno/bin/syno_predict_disk_health` s'appuie sur
  `hda1/usr/lib/libsynodiskhealthml.so` (modèle de prédiction).
- Les features extraites du binaire incluent les attributs SMART 7, 187, 188,
  197, 198, et exigent **au moins 7 jours d'historique** par numéro de série.
- `syno_disk_health_record` maintient `/var/lib/drive/health_report.json` et
  un rapport mensuel.
- L'aide `StorageManager/disk.html` documente : test SMART rapide/étendu,
  planification, **bad sector warning**, **SSD estimated endurance**, IronWolf
  Health, test de performance, cache d'écriture, firmware.

**À copier :** un historique d'attributs (pas seulement `smartctl -H`), des
seuils sur les deltas, et une notification avant la panne. Le modèle ML n'est
pas nécessaire : une régression simple sur 5/187/188/197/198 suffit.

#### Autres mécanismes de stabilité

- **Hot spare** : `StorageManager/hotspare.html` décrit l'auto-remplacement
  quand un disque passe en état « Critical », avec un ordre de priorité
  (spare dédié au pool, plus petite capacité suffisante, même unité, puis
  autres).
- **Online Assemble** : `StorageManager/storage_pool_online_assemble.html`
  permet de réassembler un pool importé sans redémarrer.
- **Réparation** : `storage_pool_repair.html` impose un disque de taille
  suffisante et documente SHR-1/SHR-2.
- **Vitesse de resync** : `storage_pool_adjust_resync_speed.html` propose
  « Lower the impact on overall system performance », « Run RAID resync
  faster » ou un profil personnalisé Min/Max.
- **TRIM** : `volume_ssd_trim.html` planifie le TRIM sur les pools 100 % SSD.
- **Défragmentation** : `volume_filesystem_defragmentation.html` (Btrfs
  uniquement, volume sain).
- **Déduplication/compression** : `volume_btrfs_dedup.html` (Btrfs, SSD/HDD,
  avec analyseur et planification).

---

### 2.2 Transferts

#### SMB — le catalogue complet

L'aide `AdminCenter/file_winmacnfs_win.html` et les chaînes de
`packages_extracted/SMBService-*/ui/texts/enu/strings` donnent la liste des
mécanismes :

| Mécanisme | Détail DSM | Intérêt fiabilité |
|---|---|---|
| SMB3 Multichannel | `option_enable_multichannel`, désactivé si Link Aggregation | Agrégation et bascule de liens |
| Persistent / durable handles | `enable_persistent_handles`, « restore lost SMB2 connection… opened files remain available during network outage » | Reprise après coupure réseau |
| SMB2 leasing / SMB3 directory leasing | `option_enable_smb2_leasing`, `option_enable_smb3_directory_leasing` | Cache client, moins d'allers-retours |
| Server signing | `Disable` / `Client defined` / `Force` | Intégrité des paquets, anti-MITM |
| Transport encryption | `Disable` / `Client defined` / `Force` (SMB3) | Confidentialité, et désactive les oplocks |
| Opportunistic locking | Oui | Performance, à désactiver pour les bases |
| AIO read | « Enable asynchronous read » | Débit 4K/10GbE, coût CPU |
| Strict sync | « Synchronize data to drive immediately upon SMB client request » | Durabilité explicite, coût performance |
| Transfer log | Création, déplacement, renommage, lecture, écriture, permissions | Traçabilité |
| MSDFS | Liens distribués | Disponibilité de l'espace de noms |

SMBService implémente aussi une **auto-réparation** dans
`conf/systemd/samba-util.sh` :

- si `/etc/samba/smb.conf` est vide ou absent, il est remplacé par
  `/etc.defaults/samba/smb.conf` après mise de côté du fichier fautif ;
- les répertoires `/var/cache/samba`, `/var/log/samba`, `/etc/samba/private`
  sont recréés ;
- les permissions de `/run/samba/*` sont corrigées.

**À copier :** un `smb.conf` vide ne doit jamais empêcher le service de
démarrer ; il doit être restauré depuis une copie saine et l'événement
journalisé.

#### NFS

`AdminCenter/file_winmacnfs_nfs.html` documente NFSv3/4/4.1, le multipathing
NFSv4.1, Kerberos et les tailles de paquets UDP. `hda1/etc.defaults/nfs/syno_nfs_conf`
montre cependant `nfsv4_enable=no` par défaut : DSM reste prudent, mais cela
laisse NFSv3 comme mode courant.

HomeVault fait déjà mieux sur un point : ses exports générés contiennent
`sync` par défaut (`src/shares.rs:1303-1333`), ce qui évite le classique
« écriture acquittée, perdue au crash » du NFS asynchrone.

#### FileStation

Les chaînes de `packages_extracted/FileStation-*/ui/file_browser/texts/enu/strings`
montrent :

- calcul **MD5** à la demande (`calculate_MD5_hint`, `calculatingmd5`) ;
- reprise (`resume`) ;
- politiques de conflit : écraser / ignorer / conserver les deux ;
- quotas de dossier partagé, limites de nombre de fichiers/dossiers.

MD5 est **insuffisant** comme checksum d'intégrité moderne (collisions
connues). HomeVault est déjà au-dessus avec SHA-256/SHA-512/BLAKE3
(`src/checksum.rs`).

#### USB Copy

`packages_extracted/USBCopy-*/app/texts/enu/strings` documente :

- modes **Incremental**, **Mirroring**, **Multi-versioned**, **Photo/Video
  Import** ;
- politique de conflit ;
- éjection automatique ;
- suppression de la source après copie.

**Aucune chaîne ne mentionne un checksum ou une vérification de contenu.**
C'est un angle mort de DSM, et HomeVault a hélas reproduit le nom sans le
comportement (voir §4.1).

#### iSCSI

`packages_extracted/ScsiTarget-*/ui/texts/enu/strings` et
`hda1/usr/syno/synoman/webman/help/enu/iSCSIManager/snapshot.html` donnent :

- **Buffered I/O** vs **Direct I/O** (« Directly read and write data on the
  drive for data reliability. Recommended for SSD storage ») ;
- **FUA and Sync Cache SCSI commands** : « can lower the risk of storage
  damage caused by unexpected shutdowns… recommended for SSDs » ;
- **Thin Provisioning** vs **Thick Provisioning** ;
- **Space reclamation (UNMAP)** ;
- snapshots **application-consistent** (via Synology Snapshot Manager) vs
  **crash-consistent** ;
- **Low-Capacity Write** : accepte d'écrire sous 1 Go restant, au prix d'un
  risque de crash du filesystem — un compromis explicite, pas une bonne
  pratique ;
- `block_lun_repair/iSCSIBlockLunConfUpdate.sh` : hook de réparation après
  expansion d'un LUN bloc.

#### Haute disponibilité

`packages_extracted/HighAvailability-*/ui/texts/enu/strings` et les binaires
`synoha*`, `corosync`, `libcrmcommon`, `libpengine`, `drbd-env.sh` montrent une
architecture active/passive :

- réplication DRBD, `synodrbd --drbd-init` ;
- heartbeats corosync/Pacemaker ;
- **quorum server** externe ;
- détection de **split-brain** et suspension des services pour éviter
  d'aggraver l'incohérence ;
- miroir du cache mémoire (`desc_cache_protection_sync`) ;
- conditions de bascule : pool dégradé, disque absent, version DSM/paquet
  différente, partition système pleine, interface heartbeat instable.

C'est puissant mais coûteux : deux machines identiques, un quorum, une
procédure de recovery. Pour HomeVault, ce n'est pas un P0.

---

### 2.3 Sauvegardes et vérification

Hyper Backup est le composant le plus mature du corpus sur l'intégrité :

- la bibliothèque `libsynobackup.so.1` contient la table
  `file_info_tb(shareName, basePath, createTime, modifyTime, fileSize,
  archiveVersion, checksum)` et des routines SHA-256/MD5 ;
- elle expose `IntegrityCheckFinishedTargetPass`,
  `IntegrityCheckFinishedTargetFailed`, `status_need_integrity_check` ;
- `texts/enu/strings` documente :
  - **Backup Integrity Check** avec deux niveaux : **index structure** et
    **data** ;
  - planification (fréquence, heure, limite de temps) ;
  - **auto-resume** avec un nombre maximal de tentatives après coupure réseau
    (`auto_resume_retry_limit_desc`, `max_auto_resume_retry`) ;
  - détection de cible corrompue (`target_broken`, `desc_target_cksum_error`) ;
  - avertissement après export/import hors bande : « Some data in the backup
    destination were copied or moved without data checksum. Please run an
    integrity check » ;
  - chiffrement côté client, chiffrement du transport, compression,
    déduplication, rétention, verrouillage de versions.

**À copier :** un index de checksums par fichier n'est pas nécessaire avec
restic (les packs sont déjà adressés par contenu), mais **le contrôle
périodique des données** et **la reprise après coupure** le sont.

---

### 2.4 Intégrité de la chaîne de mise à jour

Le corpus contient trois niveaux :

1. `checksum.syno` : pour chaque membre du `.pat`, une ligne
   `CRC32 taille nom taille_compressée offset`. Exemple :
   `1979056239 243565596 hda1.tgz 342895807 305`.
2. `Synology.sig` : signature **CMS/PKCS#7** (`1.2.840.113549.1.7.2`) des
   mêmes membres. C'est la vraie barrière cryptographique ; le CRC32 n'est
   qu'une détection d'erreur.
3. `packages_extracted/SMBService-*/selfcheck/SMBService.full.default.list`
   + `.asc` : manifeste de fichiers (taille, SHA-1, permissions) signé PGP.

**À copier :** signature + manifeste, pas seulement un hash. **À améliorer :**
le manifeste DSM utilise SHA-1 ; HomeVault doit utiliser SHA-256/BLAKE3 et un
mécanisme de confiance explicite (clé publique épinglée).

---

## 3. Les compromis de DSM qu'il ne faut pas copier

| Compromis DSM | Preuve | Pourquoi ne pas copier |
|---|---|---|
| `^metadata_csum` sur ext4 | `hda1/etc.defaults/mke2fs.conf` | Perte de la détection d'intégrité des métadonnées |
| `enable_periodic_fsck = 0` | même fichier | Un fsck seulement après arrêt sale laisse passer les corruptions lentes |
| MD5 comme checksum utilisateur | FileStation `calculate_MD5_hint` | Collisions ; SHA-256/BLAKE3 sont disponibles |
| USB Copy sans vérification | `USBCopy` strings | Un « copy » non vérifié n'est pas une sauvegarde |
| rsync sans vérification de contenu | `hda1/etc.defaults/rsyncd.conf`, aide `file_rsync.html` | Le transfert est checksumé, mais rien ne prouve la copie finale |
| NFSv4 désactivé par défaut | `syno_nfs_conf` | Moins de fonctionnalités, pas plus de fiabilité |
| Signature/Chiffrement SMB « Client defined » | aide `file_winmacnfs_win.html` | Un client old-school peut négocier l'absence de protection |
| Scrub limité à certains RAID | aide `storage_pool_data_scrubbing.html` | RAID1/10 et ext4 ne bénéficient pas du même contrôle |
| Checksum par dossier immuable | aide `file_share_create.html` | Impossible de corriger un choix initial sans recréer le partage |
| SHA-1 dans le manifeste de paquet | `*.full.default.list` | Affaiblit la chaîne de mise à jour |
| Low-Capacity Write iSCSI | aide `ScsiTarget` | Accepte explicitement un risque de crash du filesystem |
| HA split-brain complexe | `HighAvailability` strings | Coût opérationnel élevé ; une réplication bien vérifiée couvre souvent le besoin |

---

## 4. Confrontation à HomeVault — matrice d'écarts

Légende : **P0** = perte de données ou fausse assurance ; **P1** =
consolidation ; **P2** = différenciation.

### 4.1 Intégrité des fichiers et des supports

| Domaine | DSM 7.4.1 | HomeVault `HEAD=3afe03e` | Écart | Prio |
|---|---|---|---|---|
| Checksums Btrfs / CoW par partage | Option explicite CRC32 + CoW, immuable | Btrfs/ZFS ont des checksums natifs ; pas de notion « data checksum » par partage, pas de test qui vérifie l'absence de `nodatacow` | Moyen | P1 |
| ext4 metadata_csum | **Désactivé** | `mkfs.ext4` par défaut (moderne) ; pas de test explicite | HomeVault probablement meilleur, à verrouiller | P1 |
| Scrub planifié | Oui, un pool à la fois, pause/reprise | **Manuel uniquement** (`src/pools.rs:566`, `src/handlers/storage.rs:579`) | **Élevé** | **P0** |
| Scrub après arrêt sale | Oui (`scrubbing-auto-scrubbing.service`) | Aucun marqueur d'arrêt propre, aucun déclenchement | **Élevé** | **P0** |
| Reprise d'un scrub/repair après reboot | Oui (`syno-storage-task-resume.service`) | Aucune reprise ; le job est perdu | Moyen | P1 |
| Récupération de montage | Échelle Btrfs/ext4 complète (`volume.sh`) | `nofail` seulement ; pas de `fsck`, pas de superbloc de secours | **Critique** | **P0** |
| fsck au boot | Oui après arrêt sale | Dépend de la distribution ; aucun code HomeVault | **Élevé** | **P0** |
| SMART détaillé | Historique, bad sector warning, endurance, prédiction | `smartctl -H` pour la santé (`src/health.rs:518`), `-A` pour la liste (`src/disk.rs:1259`), pas d'historique ni de delta | **Élevé** | **P0** |
| Tests SMART planifiés | Oui | Oui via tâches planifiées (`src/disk.rs:3625`) | Parité | — |
| Erreurs I/O noyau | Journalisées et remontées | Pas de veille `dmesg`/`journalctl` dédiée | **Élevé** | P0/P1 |
| TRIM SSD | Planifié | Aucun `fstrim`/timer | Moyen | P1 |
| Balance Btrfs | Oui si metadata/data > 4 % et non-alloué < 5 Gio | Aucun | Moyen | P1 |
| Défragmentation Btrfs | Oui | Aucun | Faible | P2 |
| Compression Btrfs | `zstd` par défaut | Montage Btrfs sans `compress=` (`src/pools.rs:1698`) ; ZFS `lz4` | Faible | P2 |
| Déduplication | Oui (Btrfs) | Non | Faible | P2 |
| Hot spare auto | Oui, y compris remplacement sur « Critical » | Spares mdadm/ZFS créés, mais pas d'auto-remplacement sur seuil SMART | Moyen | P1 |
| Online assemble mdadm | Oui | Import ZFS/Btrfs seulement (`src/pools.rs:148`) ; pas d'assemblage mdadm étranger | Moyen | P1 |
| Vitesse de resync | Réglable Min/Max | Aucun réglage | Moyen | P1 |
| Bitmap write-intent | — | Déjà présent (`src/disk.rs:1751`) | Bien | — |

### 4.2 Transferts

| Domaine | DSM 7.4.1 | HomeVault | Écart | Prio |
|---|---|---|---|---|
| Upload HTTP reprenable | Non (FileStation) | Oui, sessions, offset autoritatif, 32 Mio/chunk, `fsync`, SHA-256, `rename` atomique (`src/handlers/explorer.rs:2370-2515`) | **HomeVault devant** | — |
| Téléchargement HTTP | MD5 à la demande | `Range`, reprise, `X-Content-Sha256` ≤ 64 Mio (`src/handlers/explorer.rs:242`) | HomeVault devant ; digest absent au-delà de 64 Mio | P1 |
| Conflits de copie | Écraser/Ignorer/Conserver | `overwrite`/`keep_both`/`skip` (`src/browser.rs`) | Parité | — |
| SMB Multichannel | Oui | Oui (`SmbConfig.multichannel`) | Parité | — |
| SMB durable handles | Oui | **Non** (absent de la liste blanche `SMB_EXTRA_KEYS`) | **Élevé** | **P0** |
| SMB chiffrement transport | 3 modes | Seulement via paramètre libre `server smb encrypt` ; pas de bascule | Moyen | P0/P1 |
| SMB signature | 3 modes | `server signing` accepté en paramètre libre ; Security Advisor le contrôle | Moyen | P1 |
| SMB `strict sync` / AIO / leases | Oui | AIO via paramètres libres ; `strict sync`, leases, oplocks non exposés | Moyen | P1 |
| SMB auto-réparation config | Oui | Non : écriture directe, pas de `.bak`, pas de validation | **Élevé** | **P0** |
| NFS `sync` | Non explicite | Oui par défaut (`src/shares.rs:1303`) | **HomeVault devant** | — |
| NFSv4.2 / Kerberos | 4.1, Kerberos | 4.2 accepté, pas de `sec=krb5p` | Faible | P2 |
| Rsync | Service, pas de checksum final | Service, pas de `--checksum` final | Moyen | P1 |
| Rclone | — | `sync` sans `--checksum`, sans retry explicite (`src/handlers/rclone.rs:1866`) | Moyen | P1 |
| USB Copy checksum | **Absent** | Champ `verify_checksums` mais **taille/existence seulement** (`src/usb_copy.rs:509`) | **Critique** | **P0** |
| iSCSI FUA / Sync Cache | Oui | **Non** (`src/iscsi.rs`) | **Élevé** | P0/P1 |
| iSCSI thin/thick, UNMAP, snapshots | Oui | Non | Moyen | P2 |
| iSCSI protection volume plein | Oui (LUN protégé, Low-Capacity Write) | Non | Moyen | P1 |
| HA active/passive | Oui (DRBD/corosync/quorum) | Non (réplication de snapshots) | Stratégique | P2 |

### 4.3 Sauvegardes et reprise

| Domaine | DSM 7.4.1 | HomeVault | Écart | Prio |
|---|---|---|---|---|
| Vérification d'intégrité planifiée | Oui (index + data, limite de temps) | `restic check` **à la demande** (`src/restic.rs:1661`) ; `backup_target_scheduler` couvre les dépôts **reçus**, pas les dépôts locaux | **Élevé** | **P0** |
| Lecture complète des données | Oui | `restic check` sans `--read-data` ni `--read-data-subset` | **Élevé** | **P0** |
| Reprise après coupure réseau | Oui (`max_auto_resume_retry`) | Le planificateur replanifie, mais pas de retry immédiat avec backoff (`src/restic_scheduler.rs:250-330`) | Moyen | P1 |
| Détection de cible corrompue | Oui | Partielle : `parse_check_output` détecte les erreurs de structure | Moyen | P1 |
| Chiffrement côté client | Oui | Restic chiffre nativement | Parité | — |
| Rétention / versions | Oui | Oui (`restic forget`, GFS snapshots) | Parité | — |
| Journal de changements par version | Oui | Non | Faible | P2 |
| Relink / import d'un dépôt existant | Oui | `restic init` seulement ; import à vérifier | Moyen | P1 |
| Sauvegarde de configuration | `.dss` chiffré, restauration sélective | Archive `.hvbak` AES-256-GCM, manifeste SHA-256, restauration étagée (`src/config_backup.rs`) | **HomeVault devant** | — |
| Sauvegarde SQLite | — | `VACUUM INTO`, `quick_check` au boot, `integrity_check` à la demande (`src/storage/database.rs`) | Bien | — |
| Durabilité SQLite | — | `PRAGMA synchronous = NORMAL` (`src/storage/database.rs:134`) | Dernière transaction potentiellement perdue à la coupure | P2 |

### 4.4 Stabilité de service et chaîne de mise à jour

| Domaine | DSM 7.4.1 | HomeVault | Écart | Prio |
|---|---|---|---|---|
| Redémarrage service | systemd + supervision | `Restart=on-failure`, `RestartSec=5` (`installer/install.sh:1698`) ; pas de `LimitNOFILE`, `TimeoutStopSec`, `StartLimit*` | Moyen | P1 |
| Écritures de configuration atomiques | Auto-réparation SMB | `AtomicWriter` **supprime la cible avant `rename`** (`src/storage/atomic.rs:46-60`) ; `smb.conf`/exports écrits en direct (`src/shares.rs:334-376`) | **Élevé** | **P0** |
| Validation de configuration | `testparm` implicite, SMB auto-réparé | Pas de `testparm` avant reload ; pas de `.bak` | **Élevé** | **P0** |
| Surveillance I/O | Oui (SMART, latence, logs) | Non | **Élevé** | P0 |
| Panic/watchdog noyau | `kernel.panic=3` (`sysctl.conf`) | Non configuré | Moyen | P1 |
| UPS + write cache | Oui, avertissement | UPS oui (`src/ups.rs`), pas de garde « write cache sans UPS » | Moyen | P1 |
| Signature des paquets/apps | PKCS#7 + PGP (SHA-1) | Catalogue Docker : pas de vérification de signature, tags non épinglés | Moyen | P1 |
| Mises à jour | `.pat` signé | Installation APT + binaire ; pas de mécanisme de mise à jour signé visible | Moyen | P1 |

---

## 5. Recommandations priorisées

### P0 — avant toute promesse de fiabilité

#### P0-1. Rendre la vérification USB Copy réelle

**Problème.** `UsbCopyProfile.verify_checksums` existe, mais
`verify_copy` ne compare que la taille et l'existence
(`src/usb_copy.rs:509`). Un fichier corrompu de même taille passe.

**Action.**

1. Remplacer `verify_copy` par un parcours qui, pour chaque fichier :
   - ouvre source et destination en streaming ;
   - calcule SHA-256 (ou BLAKE3) des deux ;
   - compare taille + hash ;
   - collecte `{path, expected, actual}` sans s'arrêter au premier échec ;
   - retourne `Err` avec la liste complète.
2. Ne jamais supprimer la source (`remove_source_file`) si la vérification a
   échoué.
3. Ajouter un mode `verify = "size" | "sha256" | "none"` pour que le nom du
   champ ne mente plus.
4. Ajouter les stratégies DSM manquantes si elles ne sont pas déjà là :
   `incremental`, `mirror`, `versioned`, `photo-import`, avec politique de
   conflit (`overwrite`/`skip`/`rename`).
5. Test d'intégration : copier un fichier, corrompre un octet côté
   destination, vérifier que la copie est refusée et que la source reste.

**Fichiers.** `src/usb_copy.rs`, `tests/usb_copy_integration.rs`,
`frontend/src/routes/UsbCopy.svelte`, `docs/guides/usb-copy.md` (à créer).

**Effort.** 2–3 jours.

---

#### P0-2. Scrub planifié, repris après reboot et déclenché après arrêt sale

**Problème.** Le scrub n'existe que comme endpoint manuel
(`POST /storage/virtual-disks/{name}/scrub`). Rien ne garantit qu'il tourne
un jour, ni qu'il reprenne après une coupure.

**Action.**

1. Table `scrub_schedules` :
   `(id, pool, kind, cron, priority, paused, last_run, last_status, next_run)`.
2. Boucle planificatrice (même modèle que `snapshot_scheduler`) :
   - une seule opération lourde à la fois (scrub, resync, balance, `restic
     check`) ;
   - `next_run` recalculé même en cas d'échec ;
   - reprise au démarrage si `last_status = 'running'` ;
   - notification à la fin, avec le nombre d'erreurs corrigées/non corrigées.
3. Marqueur d'arrêt propre :
   - écrire `data/state/clean-shutdown` sur `SIGTERM` après `fsync` ;
   - le supprimer au démarrage ;
   - si absent : lancer un scrub (Btrfs/ZFS) ou `mdadm --action check`
     (RAID redondant) des pools montés, et journaliser « arrêt non propre ».
4. Pour mdadm, `mdadm --action check` fonctionne sur RAID1/5/6/10, pas
   seulement RAID5/6 : ne pas recopier la limitation DSM.
5. Interface : « Planifier », « Pause », « Reprendre », « Dernier résultat »,
   « Erreurs corrigées / non corrigées ».
6. Test : simuler un `SIGKILL`, redémarrer, vérifier que le scrub est
   programmé et que l'événement est visible.

**Fichiers.** `src/pools.rs`, `src/handlers/storage.rs`, nouveau
`src/scrub_scheduler.rs`, `sqlx/migrations/`, `frontend/src/routes/Storage.svelte`,
`docs/guides/storage.md`.

**Effort.** 4–6 jours.

---

#### P0-3. Récupération de montage et fsck au démarrage

**Problème.** `defaults,nofail` évite le blocage du boot mais laisse un pool
non monté, sans diagnostic ni tentative de récupération. Aucun `fsck`,
aucune échelle de superblocs.

**Action.**

1. Nouveau module `src/storage/recovery.rs`, appelé au démarrage avant
   l'ouverture des partages :
   - **mdadm** : `mdadm --assemble --scan`, puis `mdadm --detail` ;
     si dégradé, notifier mais monter si possible.
   - **ZFS** : `zpool import -N` puis `zpool status -x` ; ne jamais forcer
     `-f` sans confirmation admin.
   - **Btrfs** : `btrfs device scan`, essayer le montage normal ; en cas
     d'échec, `btrfs rescue super-recover -y`, puis `mount -o ro`, puis
     `mount -o ro,nologreplay` ; notifier le mode dégradé.
   - **ext4** : si `dumpe2fs -h` indique « not cleanly unmounted »,
     `e2fsck -p` (réparation automatique sûre) ; si échec, proposer
     `e2fsck -fy` en action admin explicite ; essayer les superblocs de
     secours (`-b 32768`, `-b 98304`, …).
2. Ne **jamais** monter en écriture un volume récupéré en lecture seule sans
   confirmation.
3. Ajouter `x-systemd.device-timeout=30s`, `x-systemd.mount-timeout=30s`,
   `_netdev` le cas échéant aux entrées fstab générées.
4. Écrire un fichier d'état lisible par l'interface :
   `data/state/storage-recovery.json` (`pool`, `mode`, `errors`, `action`).
5. Test avec `dm-flakey` / `dm-error` : simuler un superbloc Btrfs corrompu,
   vérifier le montage en lecture seule et la notification.

**Fichiers.** `src/storage/recovery.rs` (nouveau), `src/disk.rs`,
`src/pools.rs`, `src/health.rs`, `installer/install.sh` (unités/timers).

**Effort.** 5–8 jours.

---

#### P0-4. Surveillance SMART, erreurs I/O et rapport de santé

**Problème.** `check_disk_smart` ne lit que `smartctl -H` (`src/health.rs:518`).
Les attributs 5/187/188/197/198 sont affichés dans la liste des disques
(`src/disk.rs:1259`) mais ne déclenchent **aucune alerte** et ne sont pas
historisés.

**Action.**

1. Table `disk_health_samples(serial, model, attr_id, raw, normalized, sampled_at)`.
2. Échantillonnage quotidien (03:00) + au démarrage :
   - attributs 5 (Reallocated_Sector_Ct), 187 (Reported_Uncorrect),
     188 (Command_Timeout), 197 (Current_Pending_Sector),
     198 (Offline_Uncorrectable), 199 (UDMA_CRC_Error_Count),
     177 (Wear_Leveling_Count), 231 (SSD_Life_Left) ;
   - température, heures de fonctionnement, état `-H`.
3. Seuils :
   - tout passage de `-H` à `FAILED` → alerte critique immédiate ;
   - augmentation de 197/198/187 ≥ 1 → alerte ;
   - augmentation de 5 ≥ 5 → alerte ;
   - température > 55 °C pendant 3 échantillons → alerte ;
   - usure SSD < 10 % → alerte.
4. Veille I/O :
   - `journalctl -k --since "1 hour ago"` filtré sur
     `I/O error`, `Buffer I/O error`, `medium error`,
     `critical medium error`, `md/raid.*read error`,
     `BTRFS error`, `blk_update_request: I/O error` ;
   - `btrfs device stats <mount>` ;
   - `zpool status -x` ;
   - `mdadm --detail` (`Mismatch Count`, `State`).
5. Rapport hebdomadaire (e-mail/notification) : dernier scrub, dernier test
   SMART, secteurs réalloués, erreurs I/O, état RAID, dernier `restic check`.

**Fichiers.** `src/disk.rs`, `src/health.rs`, `src/notification_engine.rs`,
`sqlx/migrations/`, `frontend/src/routes/Health.svelte`.

**Effort.** 4–6 jours.

---

#### P0-5. Écritures de configuration atomiques, validées et auto-réparées

**Problème.** `AtomicWriter` fait `remove_file` puis `rename`
(`src/storage/atomic.rs:46-60`) : entre les deux, le fichier n'existe plus.
`write_config_file` (`src/shares.rs:359`) écrit directement `smb.conf` ;
`append_config_file` fait un `append` non atomique. Une coupure peut laisser
un fichier tronqué, et le service ne démarre plus.

**Action.**

1. Corriger `AtomicWriter` :
   - ne **pas** supprimer la cible : `rename(tmp, target)` remplace
     atomiquement sur POSIX ;
   - `fsync` du fichier **et** du répertoire parent ;
   - nom temporaire unique (`.tmp.<pid>.<random>`) ;
   - préserver le mode et le propriétaire ;
   - en cas d'échec, laisser l'ancien fichier intact.
2. Pour `/etc` (root) : écrire un temporaire dans le même répertoire via
   `sudo tee`, `fsync`, puis `sudo mv` atomique ; conserver
   `smb.conf.bak`/`exports.bak`.
3. Valider avant de recharger :
   - SMB : `testparm -s` ;
   - exports : `exportfs -ra` en mode vérification ou parseur interne ;
   - rsync : `rsync --daemon --config=... --dry-run` si disponible.
4. Au démarrage : si `smb.conf` est vide ou invalide, restaurer le `.bak`
   (ou le fichier par défaut), journaliser, et notifier.
5. Test : `kill -9` entre l'écriture du temporaire et le `rename` ; vérifier
   que l'ancien fichier est toujours là.

**Fichiers.** `src/storage/atomic.rs`, `src/shares.rs`,
`src/rsync_server.rs`, `src/sftp.rs`, tests.

**Effort.** 2–3 jours.

---

#### P0-6. `restic check` planifié, avec lecture de données et reprise

**Problème.** `restic check` est disponible à la demande
(`src/restic.rs:1661`) mais le planificateur ne fait que backup + retention
(`src/restic_scheduler.rs`). Un dépôt peut avoir des packs corrompus sans que
personne ne le sache. Le contrôle `backup_target_scheduler` couvre les dépôts
**reçus**, pas les dépôts **locaux**.

**Action.**

1. Ajouter `check_schedule`, `last_check_at`, `last_check_ok`,
   `last_check_mode` à la configuration du dépôt.
2. Planification :
   - hebdomadaire : `restic check --read-data-subset=10%` ;
   - mensuel : `restic check --read-data-subset=100%` (ou `--read-data`) ;
   - jamais deux contrôles simultanés ; fenêtre de maintenance.
3. Reprise : 3 tentatives avec backoff exponentiel (1 min, 5 min, 15 min)
   sur erreurs réseau ; ne pas avancer `last_check_ok` si le contrôle n'a pas
   abouti.
4. Notification : échec, erreurs de structure, packs manquants, durée.
5. Interface : « Dernier contrôle », « Mode », « Résultat », « Lancer
   maintenant ».
6. Test : dépôt sain → OK ; dépôt avec un pack tronqué → échec détecté.

**Fichiers.** `src/restic.rs`, `src/restic_scheduler.rs`,
`src/handlers/restic.rs`, `sqlx/migrations/`,
`frontend/src/routes/Restic.svelte`.

**Effort.** 3–4 jours.

---

#### P0-7. SMB : durable handles, chiffrement, `strict sync` et AIO first-class

**Problème.** `SmbConfig` n'expose que `multichannel`
(`src/shares.rs:159-200`). `durable handles` n'est même pas dans la liste
blanche `SMB_EXTRA_KEYS` (`src/shares.rs:653-666`). Une coupure réseau coupe
les fichiers ouverts ; aucune durabilité explicite n'est proposée.

**Action.**

1. Ajouter à `SmbConfig` :
   - `durable_handles: bool` → `durable handles = yes` ;
   - `smb_encrypt: "off" | "desired" | "required"` →
     `server smb encrypt = ...` ;
   - `server_signing: "off" | "desired" | "mandatory"` →
     `server signing = ...` ;
   - `strict_sync: bool` → `strict sync = yes` ;
   - `aio_read_size` / `aio_write_size` (0 = auto) ;
   - `use_sendfile: bool` ;
   - `oplocks`, `kernel_oplocks`, `level2_oplocks`.
2. Rendre ces champs exhaustifs dans `smb_global_params` (le destructuring
   exhaustif est déjà la garantie de non-régression).
3. Documenter les compromis :
   - `durable handles = yes` désactive le verrouillage inter-protocoles ;
   - le chiffrement SMB3 coûte du CPU et peut exclure de vieux clients ;
   - `strict sync` protège les bases mais réduit le débit.
4. Valider avec `testparm -s` avant `systemctl reload smbd`.
5. Test : `testparm` accepte la configuration ; un client SMB qui perd la
   connexion retrouve le handle (test manuel documenté).

**Fichiers.** `src/shares.rs`, `frontend/src/lib/components/shares/`,
`docs/guides/shares.md`, tests.

**Effort.** 3–4 jours.

---

### P1 — consolidation

#### P1-1. RAID : vitesse de resync, moniteur et remplacement automatique

- Écrire `/proc/sys/dev/raid/speed_limit_min|max` (ou
  `mdadm --grow --bitmap=internal` déjà présent) selon un profil
  « impact faible / rapide / personnalisé ».
- Activer `mdadm --monitor --scan --program` pour transformer les événements
  (`Fail`, `DegradedArray`, `SpareActive`) en notifications HomeVault.
- Pour RAID5, évaluer `--consistency-policy=ppl` (partial parity log) afin de
  réduire le « write hole » ; documenter le coût.
- Sur un disque `Critical` (SMART), proposer/allouer automatiquement un spare
  au pool, comme DSM.

**Fichiers.** `src/disk.rs`, `src/health.rs`, `src/notification_engine.rs`,
`installer/install.sh`.

#### P1-2. Maintenance Btrfs et TRIM

- Monter les pools Btrfs avec `compress=zstd:3,space_cache=v2,noatime`
  (aujourd'hui `defaults,nofail,subvol=@`).
- Ajouter un `fstrim.timer` hebdomadaire pour les pools SSD (ou un job
  HomeVault qui appelle `fstrim`).
- Balance conditionnelle : si `btrfs filesystem usage` montre
  `Metadata,DUP / Data,single > 4 %` et `Device unallocated < 5 Gio`, lancer
  `btrfs balance start -dusage=50`.
- Exposer `btrfs device stats` dans Health.

**Fichiers.** `src/pools.rs`, `src/health.rs`, `installer/install.sh`.

#### P1-3. Assemblage mdadm étranger et reprise de pool

- Ajouter `mdadm --assemble --scan` et la détection des arrays « foreign »
  dans `list_importable_pools` (aujourd'hui ZFS/Btrfs seulement).
- Après import, vérifier `mdadm --detail` et lancer un `check` si l'array est
  dégradé.

#### P1-4. Digest des gros téléchargements et checksums de chunks

- Au-delà de 64 Mio, calculer le SHA-256 **en streaming** et l'exposer via
  `Repr-Digest`/`Content-Digest` (RFC 9530) ou un fichier `.sha256` à côté.
- Pour l'upload par chunks, accepter un `X-Chunk-Sha256` optionnel et rejeter
  un chunk corrompu **avant** de l'écrire, avec l'offset de reprise.
- Ne pas hacher deux fois un fichier de plusieurs Go sans nécessité : le
  digest doit être optionnel.

#### P1-5. Rsync / rclone : vérification finale

- Rsync : après un job, exécuter `rsync --checksum --dry-run` ou
  `rsync --itemize-changes` et journaliser les différences.
- Rclone : ajouter `--checksum`, `--retries 3`, `--low-level-retries 10`,
  `--timeout 5m`, `--contimeout 1m`, et un `rclone check` post-sync.
- Enregistrer le résultat dans `transfer_history`.

#### P1-6. Réplication : Btrfs over SSH et vérification destination

- `src/replication.rs:274` refuse explicitement Btrfs over SSH. Ajouter le
  pipeline `btrfs send | ssh … btrfs receive`.
- Après réception : `zfs get -H -o value checksum <dataset>` /
  `btrfs subvolume show` ; lancer un scrub destination ou au moins
  `btrfs device stats`.
- Reprise : `zfs send -s`/`receive -s` déjà présent ; ajouter un retry avec
  backoff sur coupure SSH.

#### P1-7. iSCSI : sémantique d'écriture explicite

- Exposer `emulate_write_cache`, `emulate_fua_write`, `write_cache`
  (`write through` vs `write back`).
- Ajouter `unmap` (space reclamation) et la protection volume plein :
  passer le LUN en lecture seule sous un seuil (ex. 1 Gio), comme DSM.
- Documenter que `write back` sans UPS augmente le risque de perte.

#### P1-8. Chaîne d'approvisionnement

- Épingler les images Docker par digest (`@sha256:...`) et vérifier le digest
  avant `docker pull`.
- Signer les manifestes du catalogue (minisign/ed25519) et vérifier la
  signature avant chargement.
- Vérifier les archives de mise à jour par SHA-256 + signature.

#### P1-9. Stabilité système

- Unité HomeVault : `LimitNOFILE=65536`, `TimeoutStopSec=30`,
  `StartLimitIntervalSec=60`, `StartLimitBurst=5`, `OOMPolicy=stop`.
- `sysctl` : `kernel.panic=10`, `kernel.panic_on_oops=1`,
  `vm.dirty_background_ratio=5`, `vm.dirty_ratio=10` (à ajuster).
- Activer `smartd` et `mdmonitor` **ou** les remplacer par les jobs HomeVault
  (ne pas faire les deux en double).
- Ajouter un avertissement si le write cache d'un disque est actif sans UPS.

---

### P2 — différenciation

- **HA** : ne pas copier DSM. Si le besoin est réel, viser une réplication
  vérifiée + témoin (witness) plutôt qu'un cluster DRBD/corosync complet.
- **Prédiction de panne** : régression simple sur les attributs SMART, pas un
  modèle ML.
- **Compression/déduplication Btrfs** : `zstd` + `btrfs dedupe` (si le kernel
  le supporte).
- **Rapport de santé hebdomadaire** par e-mail/notification.
- **Journal de changement par version** pour les sauvegardes.
- **Snapshots applicatifs** : hooks de quiescence avant snapshot (bases,
  machines virtuelles) pour passer de « crash-consistent » à
  « application-consistent ».

---

## 6. Plan de mise en œuvre proposé

| Lot | Contenu | Durée | Dépendances |
|---|---|---|---|
| L1 | P0-1 USB Copy checksum + P0-5 atomicité config | 4–5 j | — |
| L2 | P0-3 récupération montage/fsck + P0-2 scrub planifié | 8–12 j | L1 (config stable) |
| L3 | P0-4 SMART/I/O + P1-1 RAID | 6–8 j | L2 (health) |
| L4 | P0-6 restic check + P1-5 rsync/rclone | 5–6 j | L1 |
| L5 | P0-7 SMB + P1-7 iSCSI | 6–8 j | L1 |
| L6 | P1-2 Btrfs/TRIM, P1-3 assemble, P1-6 réplication | 8–10 j | L2 |
| L7 | P1-8 supply chain, P1-9 système, P2 | 5–8 j | — |

**Règle de sortie :** chaque lot doit fournir code, API, UI, documentation,
tests unitaires, tests d'intégration et test E2E, conformément au
« Definition of done » du projet.

---

## 7. Tests et critères d'acceptation

### 7.1 Tests unitaires

- Parsers SMART : attributs 5/187/188/197/198, valeurs `-`, `WHEN_FAILED`,
  firmwares qui changent le format.
- `AtomicWriter` : échec entre écriture et rename → ancien fichier intact ;
  fsync du répertoire appelé ; mode préservé.
- `verify_copy` : fichier identique → OK ; un octet modifié → échec ;
  fichier manquant → échec ; fichier de même taille mais contenu différent →
  échec.
- Planification de scrub : cron, priorité, pause, reprise, un seul job.
- `restic check` : parsing des erreurs, avancement de `last_check_ok`
  uniquement sur succès.

### 7.2 Tests d'intégration

- Boucle `mdadm` sur loop devices : créer, dégrader, vérifier, réparer,
  scrub.
- Btrfs : créer un pool, corrompre un superbloc (via `dd`), vérifier la
  récupération en lecture seule.
- ext4 : simuler un arrêt sale, vérifier `e2fsck -p` et le journal.
- USB : copier, corrompre, vérifier le refus et la conservation de la source.
- SMB : `testparm` sur chaque combinaison de toggles ; reload sans casser.
- Restic : dépôt avec pack tronqué → `check` échoue.

### 7.3 Tests E2E

- Playwright : planification de scrub, affichage du dernier résultat,
  notification d'arrêt sale, bascule SMB durable handles/chiffrement,
  affichage SMART.
- Test de résilience : couper le réseau pendant un upload, reprendre,
  vérifier le SHA-256 final.
- Test de coupure : `kill -9` pendant l'écriture de `smb.conf`, redémarrer,
  vérifier que le service démarre et que la configuration est saine.

### 7.4 Métriques à exposer

- `scrub_last_success_timestamp{pool}`
- `scrub_errors_total{pool}`
- `smart_bad_sectors{serial}`
- `smart_temperature_celsius{serial}`
- `io_errors_total{device}`
- `raid_mismatch_count{array}`
- `backup_check_last_success{repo}`
- `config_write_failures_total`
- `clean_shutdown_last_timestamp`

---

## 8. Annexes

### A. Index des preuves

| Preuve | Chemin | Ce qu'elle établit |
|---|---|---|
| Version | `DSM_DS925+_90080/VERSION` | 7.4.1-90080, build 2026/07/22 |
| Modèle | `hda1/etc.defaults/synoinfo.conf` | `unique=synology_v1000nk_ds925+` |
| FS par défaut | `hda1/etc.defaults/synoinfo.conf` | `defaultfs=btrfs`, `btrfs_default_compression=zstd` |
| ext4 | `hda1/etc.defaults/mke2fs.conf` | `^metadata_csum`, `enable_periodic_fsck=0` |
| Scrub | `hda1/usr/lib/systemd/system/data-scrubbing@.service` | scrub par pool, sans timeout |
| Scrub auto | `hda1/usr/lib/systemd/system/scrubbing-auto-scrubbing.service` | après arrêt sale |
| Reprise | `hda1/usr/lib/systemd/system/syno-storage-task-resume.service` | `synostgtask --resume` |
| Resync | `hda1/usr/lib/systemd/system/syno-md-resync-speed-adjust@.service` | vitesse de resync |
| Balance | `hda1/usr/syno/lib/systemd/scripts/syno_btrfs_quick_balance.sh` | seuils 4 % / 5 Gio |
| Montage | `hda1/usr/syno/lib/systemd/scripts/volume.sh` | échelle Btrfs/ext4, e2fsck |
| SMART | `hda1/usr/syno/bin/syno_predict_disk_health` | prédiction, 7 jours |
| SMART ML | `hda1/usr/lib/libsynodiskhealthml.so` | attributs 7/187/188/197/198 |
| Disque | `hda1/usr/syno/synoman/webman/help/enu/StorageManager/disk.html` | tests, bad sector, endurance |
| Scrub aide | `.../StorageManager/storage_pool_data_scrubbing.html` | prérequis, pause, reprise |
| Checksum partage | `.../AdminCenter/file_share_create.html` | CRC32 + CoW, immuable |
| Resync aide | `.../StorageManager/storage_pool_adjust_resync_speed.html` | Min/Max |
| Hot spare | `.../StorageManager/hotspare.html` | auto-remplacement |
| Online assemble | `.../StorageManager/storage_pool_online_assemble.html` | assemblage sans reboot |
| TRIM | `.../StorageManager/volume_ssd_trim.html` | planification |
| Dédup | `.../StorageManager/volume_btrfs_dedup.html` | analyseur, planification |
| SMB aide | `.../AdminCenter/file_winmacnfs_win.html` | signing, encryption, durable handles, AIO, strict sync |
| SMB strings | `packages_extracted/SMBService-*/ui/texts/enu/strings` | multichannel, leasing, persistent handles |
| SMB auto-réparation | `packages_extracted/SMBService-*/conf/systemd/samba-util.sh` | restauration `smb.conf` vide |
| NFS | `hda1/etc.defaults/nfs/syno_nfs_conf` | `nfsv4_enable=no` |
| NFS aide | `.../AdminCenter/file_winmacnfs_nfs.html` | v3/v4/4.1, multipathing, Kerberos |
| Rsync | `hda1/etc.defaults/rsyncd.conf`, `.../AdminCenter/file_rsync.html` | service, pas de checksum final |
| FileStation | `packages_extracted/FileStation-*/ui/file_browser/texts/enu/strings` | MD5, reprise, conflits |
| USB Copy | `packages_extracted/USBCopy-*/app/texts/enu/strings` | modes, pas de checksum |
| iSCSI | `packages_extracted/ScsiTarget-*/ui/texts/enu/strings` | FUA, Direct I/O, thin, UNMAP |
| iSCSI snapshots | `.../iSCSIManager/snapshot.html` | application/crash consistent |
| HA | `packages_extracted/HighAvailability-*/ui/texts/enu/strings` | DRBD, quorum, split-brain |
| HyperBackup | `packages_extracted/HyperBackup-*/texts/enu/strings` | integrity check, auto-resume |
| HyperBackup binaire | `packages_extracted/HyperBackup-*/lib/libsynobackup.so.1` | `file_info_tb`, SHA-256 |
| Config backup | `.../AdminCenter/system_configbackup.html` | `.dss`, chiffrement |
| Mise à jour | `checksum.syno` | CRC32 + taille + offset |
| Signature | `Synology.sig` | CMS/PKCS#7 |
| Selfcheck | `packages_extracted/SMBService-*/selfcheck/*.list{,.asc}` | manifeste PGP, SHA-1 |
| Noyau | `sources/7.4-90080/apollolake/linux-4.4.x.txz` | source noyau, pas de `.config` |

### B. Commandes de reproduction

```bash
# Vue d'ensemble
ls -la /media/nicolas/500GB/Projets/DSM7/DSM_DS925+_90080
cat  /media/nicolas/500GB/Projets/DSM7/DSM_DS925+_90080/VERSION

# Configuration stockage
grep -E 'defaultfs|btrfs|trim|smart|write_cache|raid' \
  /media/nicolas/500GB/Projets/DSM7/DSM_DS925+_90080/hda1/etc.defaults/synoinfo.conf
cat /media/nicolas/500GB/Projets/DSM7/DSM_DS925+_90080/hda1/etc.defaults/mke2fs.conf

# Unités de scrub / reprise
for f in data-scrubbing@.service scrubbing-auto-scrubbing.service \
         syno-storage-task-resume.service syno-md-resync-speed-adjust@.service; do
  cat "/media/nicolas/500GB/Projets/DSM7/DSM_DS925+_90080/hda1/usr/lib/systemd/system/$f"
done

# Échelle de montage
sed -n '40,160p' \
  /media/nicolas/500GB/Projets/DSM7/DSM_DS925+_90080/hda1/usr/syno/lib/systemd/scripts/volume.sh

# Chaînes utiles
strings -n 5 /media/nicolas/500GB/Projets/DSM7/DSM_DS925+_90080/hda1/usr/syno/sbin/synoraidtool \
  | grep -i -E 'scrub|resync|hotspare|repair'
strings -n 5 /media/nicolas/500GB/Projets/DSM7/DSM_DS925+_90080/packages_extracted/HyperBackup-*/lib/libsynobackup.so.1 \
  | grep -i -E 'checksum|integrity|resume|retry'
```

### C. Limites et angles morts

- Aucune exécution réelle de DSM : les conclusions sur le comportement
  runtime (timings, erreurs) sont inférées des scripts et des chaînes.
- Les binaires strippés ne révèlent pas tous les algorithmes ; les noms de
  fonctions et les chaînes sont des indices, pas une preuve d'implémentation.
- Le source noyau fourni est `linux-4.4.x` pour `apollolake`, alors que le
  DS925+ est `v1000nk` ; il ne reflète pas nécessairement le noyau exécuté.
- Les paquets `SynoOnlinePack` et certains binaires propriétaires peuvent
  contenir des mécanismes non documentés ici.
- Les chemins HomeVault cités sont ceux de `HEAD=3afe03e` ; un décalage
  ultérieur peut invalider les numéros de ligne, pas les constats.

---

## 9. Conclusion

DSM 7.4.1 est une référence utile non pas pour ses options visibles, mais pour
sa **discipline de reprise** : planifier, reprendre, vérifier, et dire quand
on ne peut pas réparer. HomeVault possède déjà les briques de transfert les
plus solides (HTTP reprenable, SHA-256, `fsync`, `rename` atomique) et une
sauvegarde de configuration mieux conçue que celle de DSM.

Les six chantiers qui feraient passer HomeVault d'un « NAS qui marche » à un
« NAS dont on peut prouver l'intégrité » sont, dans l'ordre :

1. `verify_checksums` USB réellement vérifiant ;
2. scrub planifié + reprise + post-arrêt sale ;
3. récupération de montage et fsck au boot ;
4. historique SMART + erreurs I/O + notifications ;
5. écritures de configuration atomiques et validées ;
6. `restic check` planifié avec lecture de données.

Les six sont indépendants, testables, et aucun ne demande de réécrire
l'architecture.
