# Analyse approfondie de DSM 7.4.1‑90080 (DS925+) et comparatif HomeVault

> **Document d'analyse** — racine du dépôt, à la demande explicite du mainteneur.
> Il n'appartient pas à `docs/` (English‑only) et ne constitue pas une source de
> vérité : `CLAUDE.md` et `docs/` priment.
>
> | Champ | Valeur |
> |---|---|
> | Firmware analysé | `/media/nicolas/500GB/Projets/DSM7/DSM_DS925+_90080` |
> | Sources GPL | `/media/nicolas/500GB/Projets/DSM7/sources/7.4-90080/apollolake/linux-4.4.x` |
> | Version DSM | 7.4.1‑90080, GM, build `2026/07/22 01:35`, `productversion="7.4.1"` |
> | Modèle | DS925+ (`synology_v1000nk_ds925+`), extension DX525 |
> | Baseline HomeVault | commit `c37c803`, 04/11/2026 |
> | Méthode | analyse statique : binaires (symboles/chaînes), descripteurs `.lib`, bundles JS, units systemd, manifestes `.spk`, initramfs, source noyau GPL, configuration `/etc.defaults`. Aucune exécution du firmware. |

---

## Sommaire

1. [Synthèse](#1-synthèse)
2. [Périmètre réel de l'image](#2-périmètre-réel-de-limage)
3. [Chaîne de démarrage et installation](#3-chaîne-de-démarrage-et-installation)
4. [Noyau et intégration matérielle](#4-noyau-et-intégration-matérielle)
5. [Services et ordonnancement](#5-services-et-ordonnancement)
6. [La couche Web/API](#6-la-couche-webapi)
7. [Le frontend](#7-le-frontend)
8. [Le stockage en profondeur](#8-le-stockage-en-profondeur)
9. [Réseau](#9-réseau)
10. [Sécurité et identité](#10-sécurité-et-identité)
11. [Sauvegarde, continuité, cloud](#11-sauvegarde-continuité-cloud)
12. [L'écosystème de paquets](#12-lécosystème-de-paquets)
13. [Cartographie fonctionnelle des API](#13-cartographie-fonctionnelle-des-api)
14. [Comparatif HomeVault ↔ DSM](#14-comparatif-homevault--dsm)
15. [Lecture stratégique](#15-lecture-stratégique)
16. [Limites de l'analyse](#16-limites-de-lanalyse)
17. [Annexe — métriques, chemins, preuves](#17-annexe--métriques-chemins-preuves)

---

## 1. Synthèse

L'image est un **DSM 7.4.1‑90080** (build du 22/07/2026) pour **DS925+**
(plateforme AMD `v1000nk`), 1,5 Go extraits, accompagnée de **937 Mo de source
noyau GPL** (`linux-4.4.x`). Ce n'est **pas tout DSM** : elle ne contient que
**19 paquets** installés d'usine (File Station, SMB, Storage Manager, HA,
Active Backup Agent, Hyper Backup Entire DSM Extension, QuickConnect, Active
Insight…). Les autres applications (Drive, Photos, VMM, Container Manager,
Surveillance, MailPlus, DNS Server, VPN Center, Replication Service, AntiVirus,
Threat Prevention, RADIUS…) sont des **paquets du Package Center**, attestés par
les 63 profils AppArmor en cache et par `synopackageslimit.conf`, mais non
installés ici.

Côté code, quatre couches portent la valeur :

1. **Un noyau Linux 4.4.302 massivement patché** : **934 options
   `CONFIG_SYNO_*`**, 263 modules, dont des modules maison (`synobios`,
   `synoacl_vfs`, `synorbd`, `syno_extent_pool`, `flashcache_syno`,
   `target_core_*`, `xt_geoip`, `drbd`). C'est là que se joue l'intégration
   matérielle (baies, LED, ventilateurs, TPM, iSCSI, cache SSD).
2. **Un monolithe C++ exposé en SCGI** : `nginx → synoscgi → plugins .so`,
   **456 API / 1 740 méthodes**, décrites par des fichiers JSON `.lib` qui
   portent l'autorisation (`authLevel`, `allowUser`), les capacités (upload,
   download, demo) et le slice systemd. Un **profil AppArmor par handler**.
3. **Un front hybride** : shell **Vue 2.7.14** + design system
   `syno-vue-components`, applications **ExtJS 3.4** (2009) hébergées dans des
   fenêtres Vue, Sencha Touch pour le mobile historique, 21 locales, 30 Mo
   d'aide.
4. **Un modèle de paquet très intégré** : chaque `.spk` déclare ports pare‑feu,
   PAM, vhost nginx, syslog, moniteur HA, profil AppArmor, units systemd,
   index d'aide, textes et API.

S'y ajoute une **dette de versions** visible : noyau 4.4.302, PostgreSQL 11.11,
OpenSSL 1.1.1w, Samba 4.15.13, PHP 8.1.32, Redis 4.0, btrfs-progs 4.0, LVM
2.02.132 — des composants upstream anciens, maintenus par des backports
Synology non auditables de l'extérieur. Enfin, la plateforme conserve des
**canaux cloud sortants** (Active Insight, SupportService en gRPC capable
d'exécuter des actions distantes, QuickConnect) qui n'ont pas d'équivalent chez
HomeVault.

Côté HomeVault, l'écart n'est plus « fonctionnel » mais **de profondeur
d'intégration et d'écosystème**. HomeVault couvre l'essentiel des usages d'un
NAS domestique et dépasse DSM sur la sobriété, la sécurité produit (passkeys,
anti‑ransomware, snapshots immuables), l'IA/RAG/MCP, la testabilité et la
maintenabilité. DSM reste devant sur **SHR/SHR2, les volumes à extents
propriétaires, la déduplication, le thin provisioning, le cache SSD noyau,
KMIP/WORM, le SMB AD/Kerberos et NFSv4, l'écosystème de paquets, la HA
DRBD/Pacemaker, l'identité d'entreprise et l'isolation AppArmor par API**.

> **Correction importante par rapport à la v1 du document** : le firmware ne
> contient **pas** le client Hyper Backup ni Snapshot Replication. Il contient
> l'**agent Active Backup for Business (DSM)** et l'extension **Hyper Backup
> Entire DSM** (restauration bare‑metal), plus les API noyau de sauvegarde et de
> réplication. Les applications complètes sont des paquets.

### 1.1 Vue d'ensemble

```
┌──────────────────────────── Navigateur ─────────────────────────────┐
│  shell Vue 2.7 (login, fenêtres, widgets)                          │
│    └── applications ExtJS 3.4 chargées à la demande (config JSON)  │
└───────────────────────────────▲────────────────────────────────────┘
                                │ HTTP/HTTPS (nginx)
┌───────────────────────────────┴────────── DSM ─────────────────────┐
│ nginx ──SCGI──► synoscgi ──► SYNO.*.so   (456 API, 1 740 méthodes) │
│   │                │            └── profil AppArmor par handler     │
│   │                ├── descripteurs .lib (authLevel, allowUser…)   │
│   │                ├── /etc/synoinfo.conf (345 clés, 91 flags)     │
│   │                ├── PostgreSQL (paquets) / SQLite (index)       │
│   │                └── synostg*, spacetool, synosharesnaptool…     │
│   └── statique : webman, modules, help (30 Mo), texts (21 locales) │
│                                                                    │
│ systemd (435 units, slices pkg-*) ── daemons (scemd, synostoraged) │
│ kernel 4.4.302 + 263 modules (synobios, synorbd, synoacl_vfs, …)   │
│ mdadm / btrfs / dm-crypt / DRBD / LIO-TCMU / OVS / KVM             │
└────────────────────────────────────────────────────────────────────┘
```

---

## 2. Périmètre réel de l'image

### 2.1 Contenu

| Élément | Détail |
|---|---|
| Version | `7.4.1‑90080`, GM, build `2026/07/22 01:35` |
| Modèle | `synology_v1000nk_ds925+`, DTB `dx525` (unité d'extension) |
| Boot | `zImage` 6,1 Mo, `rd.gz` 9,0 Mo (LZMA, cpio de 38 Mo), GRUB, `bios.ROM` 9,3 Mo |
| Signature | `Synology.sig` = **PKCS#7 SignedData** (7 261 o) + `checksum.syno` (3 093 o) |
| Racine | `hda1/` 925 Mo : systemd, `/usr/syno`, nginx, PostgreSQL, SQLite, Redis, configuration Samba |
| Paquets | 19 paquets, 517 Mo |
| Index | `indexdb/` : aide + applications au format « flint » (`iamflint`) ; recherche de fichiers par **Lucene embarqué** (`synoelasticd`, paquet SynoFinder) |
| Aide | 30 Mo de HTML (toutes locales), 21 locales |
| Sources | noyau 4.4.302 + `SynoBuildConf` + `synoconfigs` + `back-porting-records` |

### 2.2 Volumétrie mesurée

| Indicateur | Valeur |
|---|---|
| APIs déclarées | **456** |
| Méthodes exposées | **1 740** |
| Descripteurs `.lib` exploitables | ~125 |
| Bibliothèques `.so` sous `webapi/` | 151 |
| Units systemd | **435** |
| Binaires `usr/syno/bin` / `usr/syno/sbin` | 305 / 175 |
| Modules noyau dans la racine | 263 |
| Modules noyau dans l'initramfs | 51 |
| Options `CONFIG_SYNO_*` | **934** (dont 177 liées à BTRFS) |
| Profils AppArmor | **193** fichiers, dont **63 paquets** en cache |
| Clés `synoinfo.conf` | 345, dont **91 `support_*=yes`** |
| Modules UI intégrés (`webman/modules`) | ~58 |
| ExtJS 3.4 (`ext-all.js`) | 816 Ko |
| Design system Vue (`syno-vue-components`) | 1,26 Mo |

### 2.3 Logiciels embarqués (versions)

| Logiciel | Version | Commentaire |
|---|---|---|
| Noyau Linux | 4.4.302 | branche EOL, patchset Synology |
| nginx | 1.23.1 | 2022 |
| Samba | 4.15.13 | 2022, EOL upstream |
| PostgreSQL | 11.11 | EOL upstream (2023) |
| OpenSSL | 1.1.1w | EOL (2023) |
| OpenSSH | 8.2p1 | 2020 |
| PHP | 8.1.32 | 2021, EOL 2025 |
| Python 3 / 2 | 3.8.15 / 2.7.18 | 3.8 EOL 2024 |
| SQLite | 3.40.0 | 2022 |
| Redis (cache API) | 4.0 | 2017 |
| rsync | 3.1.2 | 2015 |
| btrfs-progs | v4.0 (fork Synology) | 2015 |
| mdadm | v3.4 | 2023 |
| LVM2 | 2.02.132 | 2015 |
| syslog-ng | 3.36.1 | 2021 |
| CUPS | 2.0.3 | 2014 |
| Avahi | 0.7 | 2017 |
| Netatalk (AFP) | 3.1.12 | 2015 |
| FFmpeg | 4.1.9 | 2020 |
| curl | 7.86.0 | 2022 |

**Lecture** : ces versions upstream sont anciennes ; Synology applique des
backports de sécurité non vérifiables depuis l'extérieur. C'est un écart de
maintenance structurel avec une distribution Linux à jour, et un argument pour
la séparation « appliance » / « composants système » de HomeVault.

---

## 3. Chaîne de démarrage et installation

Le firmware n'est pas seulement un système d'exploitation : c'est un
**installateur/répareteur autonome**.

```
BIOS/UEFI → GRUB → zImage (4.4.302) → rd.gz (initramfs LZMA, 38 Mo)
   → busybox /linuxrc (ELF)
   → /linuxrc.syno (shell)
   → /linuxrc.syno.impl  ← moteur d'installation, de réparation et de mise à jour
   → montage de la racine (md0) → /etc/rc → systemd
```

### 3.1 L'initramfs

- Busybox + outils réels : `mdadm`, `btrfs`, `e2fsck`, `mke2fs`, `blkid`,
  `iptables`/`ip6tables`, `curl`, `fdisk`, `chroot`, `dmidecode`… (51 modules
  noyau embarqués).
- `linuxrc` est un lien vers `busybox` ; `linuxrc.syno` enveloppe
  `linuxrc.syno.impl` avec journalisation et gestion de redémarrage.
- `synogrinst.sh` implémente l'**installation de groupe** (plusieurs NAS
  installés simultanément depuis un contrôleur), avec verrou, tirage aléatoire
  et reprise.

### 3.2 Le moteur `linuxrc.syno.impl`

Fonctions observées dans le script :

- **Assemblage de la racine** : `md0` (racine, RAID1 système) et `md1` (swap) ;
  réparation automatique (`mdadm --manage /dev/md1 --add …`), reconstruction,
  `--installable_disk_list`.
- **Détection d'état** : `.noroot`, `.system_crashed`, `.restore_to_default`,
  `.autoinstall_noroot`, `/.synoinstall-borrowed-swap-raid-device`.
- **Réinstallation automatique** si la racine est absente/corrompue, ou si
  l'utilisateur a demandé une réinitialisation.
- **Mise à jour** : `.pat`, `upgrade.sh`, `bootup-smallupdate.sh`,
  `checker.pat`, `/autonano.pat` (petites mises à jour), `dsm-upgrade-prepare`.
- **Mode junior** : DSM minimal sans paquets, utilisé pour l'installation et
  certaines opérations de réparation ; bascule vers le mode complet.
- **Firmware matériel** : mise à jour SAS (`sas_fw_upgrade.sh`) et firmware OOB
  (`syno_oob_fw_upgrade`, partitions A/B avec effacement de magie en cas
  d'échec).
- **Verrous noyau** : `/proc/sys/kernel/syno_install_flag`.

### 3.3 `updater` (binaire statique de 17 Mo)

- Statiquement lié, embarque OpenSSL, curl, tar, xz, `jq`, `pg_dump`.
- Orchestre la mise à jour DSM : préparation, sauvegarde **PostgreSQL**
  (`pg_dump`), extraction, rollback (`--dsm-upgrade-prepare-rollback`), appel
  aux API `SYNO.Core.Upgrade.Server`.
- Vérifie la **signature PKCS#7** de l'image (`Synology.sig`) — messages
  `bad signature`, `Unable to load Public Key`.

**Conséquence architecturale** : DSM est conçu pour se réinstaller et se
réparer lui‑même sur du matériel varié, sans support externe. HomeVault, livré
comme binaire + service, n'a pas (encore) d'équivalent de ce moteur
d'auto‑réparation et de mise à jour signée.

---

## 4. Noyau et intégration matérielle

### 4.1 Un noyau 4.4.302 très dérivé

- `VERSION=4`, `PATCHLEVEL=4`, `SUBLEVEL=302` (branche **EOL** depuis 2022).
- **934 options `CONFIG_SYNO_*`** réparties par plateforme (`v1000`,
  `apollolake`, `broadwellnk`, `geminilake`, `epyc7002`, `rtd1619`…).
  Familles les plus représentées : BTRFS (177), MD (68), USB (51), SATA (45),
  FS (32), EXT4 (29), SAS (27), ECRYPTFS (21), NFSD (16).
- Répertoires maison : `fs/syno_cache_protection`, `fs/notify/synotify`,
  `lib/synolib` (SATA/PCIe/remap/UART), `drivers/hwmon/syno_hddmon`,
  `drivers/hwmon/syno_smbus_hddmon`, `drivers/hwmon/syno_dpm`.
- `SynoBuildConf/_modules` liste les modules par plateforme ; `v1000` ajoute
  KVM AMD, VFIO, OVS, VFIO‑PCI, TPM, FC, NVMe, `amd-xgbe`, `marvell10g`.

### 4.2 Modules propriétaires décisifs (présents dans la racine)

| Module | Rôle déduit |
|---|---|
| `synobios.ko` | Abstraction carte/modèle : GPIO, LED, boutons, ventilateurs, microcontrôleur. Contient les tables `DS925pFanSpeedMapping`… |
| `synoacl_vfs.ko` | ACL Synology (NFSv4‑like) appliquées au VFS — compense `CONFIG_BTRFS_FS_POSIX_ACL` désactivé |
| `synorbd.ko` | « Synology RAID Block Device » : couche de volume au‑dessus des espaces |
| `synofsbd.ko` | Block device de filesystem (montage, journal) |
| `syno_extent_pool.ko` | **Pool d'extents** maison — remplace `dm-thin` (désactivé dans la config) |
| `flashcache_syno.ko`, `syno_flashcache_control.ko` | Cache SSD/NVMe en mode noyau |
| `target_core_mod/user/file/iblock/pscsi`, `iscsi_target_mod`, `tcm_loop` | Cible iSCSI LIO/TCMU fournie en modules externes |
| `drbd.ko` | Réplication bloc pour la HA |
| `openvswitch.ko` | OVS (réseau VMM/containers) |
| `xt_geoip.ko` | Filtrage pare‑feu par pays |
| `kvm*.ko`, `vfio*.ko`, `vhost*.ko` | Virtualisation (VMM) |
| `aufs.ko` | Couches Docker |
| `exfat.ko`, `hfsplus.ko`, `isofs`, `udf` | Systèmes de fichiers amovibles |
| `r8125`, `r8168`, `ixgbe`, `bnxt`, `qede`, `mlx4/5`, `atlantic` | Pilotes NIC |

**Lecture** : DSM ne se contente pas d'assembler des briques Linux. Il
**remplace** les sous‑systèmes génériques (thin provisioning, ACL, cache,
RAID de volume) par des modules maison, et **code en dur** les spécificités de
chaque châssis (ventilateurs, LED, baies, TPM). C'est ce qui rend DSM
difficilement portable… et difficilement remplaçable sur son propre matériel.

### 4.3 Conséquences

- **Avantages** : intégration matérielle imbattable (LED, LCM, ventilateurs,
  baies, expansion, UPS, TPM), fonctions bloc impossibles en user space
  (cache, extents, snapshots COW efficaces).
- **Coûts** : noyau 4.4 EOL, patchset de 934 options à maintenir, dépendance
  totale au matériel certifié, impossibilité de suivre les correctifs upstream.

---

## 5. Services et ordonnancement

- **435 units systemd**, avec sections `[X-Synology]` (nom i18n) et un wrapper
  `synosystemctl`.
- Découpage par **slices et targets** : `pkg-volume.target`,
  `pkg-root.target`, `syno_dsm_storage_manager.slice`, `pkg-*.service`.
- Daemons structurants : `synostoraged` (espace/disque/cache), `synoindexd`,
  `synoindexscand`, `synoindexworkerd`, `synoindexnotifyd`, `scemd` (matériel),
  `synocrond`, `ddnsd`, `heartbeatd`, `synoconfd`, `synodbud*`.
- **HA** : **56 units de service** (`pkg-synoha-*`, plus de 80 fichiers systemd
  avec targets/paths/sockets) couvrant DRBD, Corosync, Pacemaker, moniteurs
  actif/passif, *safemode*, bascule, migration de version, synchronisation NTP,
  reprise de session.
- **iSCSI** : `pkg-iscsi`, `pkg-scsi-plugin-server`, `pkg-scsit-monitor`,
  `pkg-tcmu-runner`.
- **SMB** : `pkg-synosamba-smbd`, `nmbd`, `winbindd`, `synowstransfer`
  (WS‑Discovery 5357 via nginx du paquet).

### 5.1 Deux étages de services

| Étage | Nombre | Exemples | Ordonnancement |
|---|---|---|---|
| Cœur DSM | 435 units | `syno-space`, `syno-volume`, `synostoraged`, `synoindexd`, `nginx`, `synoscgi`, `pgsql`, `scemd`, `synocrond` | targets `syno-*`, `syno-bootup-done.target` |
| Paquets (SPK) | ~120 units | `pkg-FileStation-*`, `pkg-synosamba-*`, `pkg-iscsi`, `pkg-synoha-*`, `pkg-c2fs-*` | `pkg-<nom>.service`, `syno-packages.target` |

Quelques services structurants :

| Unit | Rôle |
|---|---|
| `syno-space.service` / `syno-volume.service` | assemblage des espaces et scan des volumes (`space.sh`, `volume.sh`) |
| `synostoraged.service` | supervision espace/disques/cache |
| `synobtrfsdedupe.service` | déduplication btrfs (Type=notify) |
| `synosharesnapshot.service` | métadonnées de snapshots de partages |
| `synonetd` / `synoneteventd` | configuration et événements réseau |
| `synoovs-db` / `synoovs-vswitch` | OVS |
| `nginx` / `synoscgi` | frontal Web + dispatcher API |
| `synocached` | **Redis 4.0** pour le cache des sessions/API |
| `pgsql` / `pgsql-adapter` | PostgreSQL 11.11 (index) |
| `synoindexd`, `synoindexscand`, `synoindexworkerd`, `synomkthumbd`, `synocontentextractd` | indexation, vignettes, extraction de texte |
| `scemd` | ventilateurs, LED, boutons, températures, alimentation |
| `synocrond` / `synoscheduled` | cron DSM et tâches planifiées |
| `synobootseq` (`syno-bootup-done.target`) | machine à états du démarrage |

*Détail* : le binaire `synoscgi` est aussi exposé par un lien symbolique au nom
volontairement très long (`synoscgi______…`) pour éviter la troncature du nom de
processus dans `ps`/systemd — un détail de terrain révélateur de la maturité
opérationnelle de la plateforme.

---

## 6. La couche Web/API

### 6.1 Flux

```
nginx :5000/:80/:443 ──SCGI──► unix:/run/synoscgi.sock ──► synoscgi
                                                             └─ .so par descripteur .lib
```

- `nginx.conf.default` : `worker_processes auto`, 1 024 connexions,
  `client_max_body_size 0` (uploads non bornés au niveau HTTP), écoute 5000 et
  80 ; le TLS est généré par profil (`SYNO.Core.Web.Security.TLSProfile`).
- `entry.cgi` (12 Ko) : dispatcher `api=…&method=…&version=…`.
- Descripteur `.lib` : `authLevel`, `allowUser` (listes `admin.local`,
  `admin.ldap`, `admin.domain`, `normal.*`), `lib`, `appPriv`, et par méthode :
  `allowDemo`, `allowUpload`, `allowDownload`, `cgiProcReusable`,
  `grantByDefault`, `grantByUser`, `systemdSlice`, `actionPrivileges`.
- Réponse : `{ "success": bool, "data": …, "error": { "code": n } }`.
- Implémentation : **C++ (ABI `__cxx11`), JsonCpp, Boost 1.80**, classes
  `SYNO::APIRequest`/`APIResponse`, `FileStation::UploadBaseHandler`,
  `SYNOVFS::Utils`.

### 6.2 Modèle d'autorisation mesuré

| `authLevel` | APIs | Signification |
|---|---|---|
| 0 | **18** | pré‑authentification : login, OTP, reset, SSO, unlock, strings UI |
| 1 | **433** | utilisateur authentifié (les listes `allowUser` restreignent l'admin) |
| 2 | **6** | opérations sensibles (initdata, session, OTP étendu, rapport HTML) |

- **365 API** déclarent la liste admin complète (`admin.local`,
  `admin.ldap`, `admin.domain`) ; 67 déclarent 6 rôles (admin + normal) ;
  6 n'ont aucune liste (API système).
- Les 18 API non authentifiées sont cohérentes avec un portail de connexion
  (pas de fuite manifeste), mais elles constituent la surface d'attaque
  pré‑login à auditer.
- Chaque handler reçoit un **profil AppArmor** (ex.
  `/usr/syno/sbin/synoscgi//SYNO.FileStation.Upload`), avec `caps64`,
  `net_allowed_af` et politique compilée : l'isolation est **par méthode**, pas
  seulement par paquet.

### 6.3 Une seconde API, moderne

`webapi/openapi/core/web/v1/openapi.json` : **OpenAPI 3.0.0**, 4 chemins
(`/web-config`, `/external`, `/http-compression`, `/tls-profile`), sécurité par
cookie `id`. DSM expose donc un **pont OpenAPI** au‑dessus du modèle `.lib`,
pour l'outillage et les intégrations.

### 6.4 Poids fonctionnel des API

| Domaine | Méthodes | APIs |
|---|---:|---:|
| Stockage (`Storage.CGI`) | 229 | 23 |
| Réseau | 92 | 28 |
| Notifications | 88 | 24 |
| Vérification d'annuaire | 68 | 8 |
| Stockage (`Core.Storage`) | 68 | 5 |
| Mise à jour | 65 | 18 |
| Partages | 65 | 14 |
| Sécurité | 54 | 14 |
| Services de fichiers | 52 | 14 |
| Annuaire/SSO | 51 | 21 |
| Paquets | 50 | 23 |

### 6.5 Où vivent les données

DSM n'a **pas** de base unique : il combine fichiers plats, cache loop,
PostgreSQL et SQLite.

| Donnée | Stockage | Détail |
|---|---|---|
| Comptes/groupes | `/etc/{passwd,group,shadow}` + `synouser.conf` + `group_desc` | écrits par `synouser`/`synogroup` via `libsynosdk` ; runtime reconstruit par `syno-local-auth-rebuild.service` |
| Cache AD/LDAP | image loop chiffrée `.account.cache.img` montée par `accountdb-cache.service` | extension `expand_accountdb_cache.sh` |
| Annuaire externe | SSSD (`sssd`, `sssd-{nss,pam,ifp,autofs}`) + `synoldapclientd` | AD/LDAP |
| Privilèges d'action | `actionPriv.db` (JSON) + `actionPriv.info` | 148 méthodes d'API portent `actionPrivileges` |
| Partages | `/etc/samba/smb.share.conf` + `synoshare.attr` + `share_right.map` | `synoshare`, `synocheckshare` |
| Quotas | Btrfs **qgroups** / `repquota` ext4 | `synoquota`, `synosharequota` ; défaut 5 % |
| Index de fichiers | **PostgreSQL 11.11** : bases `synoindex` et `mediaserver` | `synoindex/sql/*.pgsql`, migrations intégrées |
| Cache Web | **Redis 4.0** (`synocached`) | sessions/jetons |
| Compatibilité disques | SQLite (`var.defaults/lib/disk-compatibility/*.db`) | seule usage SQLite |
| Capacités produit | `/etc/synoinfo.conf` (345 clés) | modèle `etc.defaults`, fusionné par `configupdate-{rootfs,share,volume}` |

**Contraste** : HomeVault a **une** base SQLite avec 78 migrations versionnées,
et des fichiers de configuration générés (`smb.conf`, exports NFS, `dnsmasq`).
DSM répartit l'état entre six mécanismes, ce qui complique la sauvegarde
cohérente, l'audit et la migration — et explique le rôle central de
`configupdate` et des « config backup » dans ses mises à jour.

---

## 7. Le frontend

### 7.1 Deux générations, un seul shell

| Couche | Techno | Preuve |
|---|---|---|
| Shell, login, fenêtres, popups | **Vue 2.7.14** + `syno-vue-components` (window manager, popup manager, icon manager) | `webman/entry/dist/dsm.entry.bundle.js`, `scripts/vue/`, `scripts/syno-vue-components/` |
| Applications historiques | **ExtJS 3.4** + patch maison (276 Ko) | `scripts/ext-3.4/ext-all.js` (816 Ko), `synoSDSjslib/dist/extjs-patch.bundle.js` |
| Hybride | fenêtres Vue hébergeant des panneaux ExtJS | `SYNO.SDS.StorageManager.Vue.StoragePanel`, `Vue.CreateWizardWindow` |
| Mobile historique | Sencha Touch 2.4.1 | `scripts/sencha-touch-2.4.1` |
| Legacy | ExtJS 2/3, Prototype 1.7.3 | `scripts/ext-2`, `ext-3`, `prototype-1.7.3` |

L'entry bundle montre le plugin Vue enregistré avec
`windowManager.desktopEl = #sds-desktop`, puis le *handshake* de session, le
déverrouillage de volume et le login.

### 7.2 Registre d'applications

- `SYNO.SDS.AppUtil.systemApps` liste les apps système : PkgMan, AdminCenter,
  FileStation3, StorageManager, Backup, ResourceMonitor, SecurityScan,
  LogCenter, HelpBrowser, SupportForm (et une variante « SOFS » pour les
  clusters).
- Chaque paquet déclare son UI dans un `config` JSON : `"type":"app"`,
  `depend:[…]`, `allowMultiInstance`, `supportKey` (feature‑gate matériel),
  `canLaunch` (demo/admin), `defaultHttp`/`defaultHttps`, alias.
- `theme.json` mappe les CSS « default » vers les CSS actifs (thème).

### 7.3 Internationalisation et aide

- Fichiers `texts/<lang>/strings` de type INI (`[section]`, `clé = "valeur"`),
  plus un jeu `texts/` par paquet ; **21 locales**.
- Aide : 30 Mo de HTML au total (toutes locales) + index de recherche
  « flint ».
- Pas de gettext `.mo` : le pipeline est propriétaire.

### 7.4 Dette technique

ExtJS 3.4 (2009) reste le socle de la majorité des écrans en 2026. Le shell Vue
ne remplace pas les applications : il les **encapsule**. C'est un choix de
compatibilité assumé, avec un coût de sécurité et d'accessibilité.

---

## 8. Le stockage en profondeur

### 8.1 Briques

- **btrfs par défaut** (`defaultfs="btrfs"`, compression `zstd`), ext4 possible.
- **mdadm** pour les RAID, **LVM** présent mais doublé par les modules maison.
- **`spacetool`** (binaire C++) : assemblage/désassemblage des espaces,
  `--install-space-create [custom | SHR | SHR2]`, `space_mapping.xml`,
  `--bootup-assemble`, `--recovery-assemble-all`, `--fast-repair-cleanup`,
  rejeu du journal de protection de cache (`--bootup-replay-*`), notification
  de disque lent, montage RO→RW.
- **`synostgpool` / `synostgvolume` / `synostgsysraid` / `synostoragecore`** :
  logique métier `SYNO::SDS::STORAGE_MANAGER`.
- **`synorbd` + `syno_extent_pool`** : volume logique à extents, remplaçant
  `dm-thin` (désactivé).
- **`flashcache_syno`** : cache SSD en mode noyau.

### 8.2 Opérations réellement exposées

| API | Opérations |
|---|---|
| `Storage.CGI.Pool` (25) | `create`, `expand_by_add_disk`, `expand_unallocated`, `expand_unfinished_shr`, `migrate`, `repair`, `replace`, `reassemble`, `remove_missing_pool`, `data_scrubbing`, `check_fast_repair` |
| `Storage.CGI.Volume` (45) | `create`, `create_on_existing_pool`, `convert_shr_to_pool`, `convert_shr_without_drive`, `expand_pool_child`, `failover_keep_rw`, `transfer_to_rw`, `export_recovery_key`, `unlock_by_recovery_key`, `unlock_by_vault`, `set_dek`, `change_recovery_key`, `ssd_trim_*`, `defrag`, `fs_scrubbing` |
| `Storage.CGI.Storage` (26) | `repair_system_partition`, `repair_rootbackup`, `repair_sys_partition`, `get_resync_speed`, `set_resync_speed`, `set_space_reclaim_schedule`, `load_eunit_topology` |
| `Storage.CGI.Spare` (6) | `add`, `delete`, `edit`, `scope_prediction` |
| `Storage.CGI.EncryptionKeyVault` (10) | `enable`, `disable`, `set_autounlock_key`, `verify_passwd`, `sync_to_passive` |

### 8.3 Chiffrement

- `dm-crypt`/LUKS, **TPM** (`support_crypto="tpm"`), **KMIP**
  (`support_kmip="yes"`), **coffre de clés** (`EncryptionKeyVault`), clés de
  récupération exportables, `sync_to_passive` pour la HA.
- Chiffrement au niveau **volume et partage** (`support_volume_encryption`,
  `support_share_encryption`).

### 8.4 Snapshots, dédup, cache

- Snapshots btrfs par sous‑volume, planification, rétention, **WORM**
  (`support_worm="yes"`), snapshots de partage (`Core.Share.Snapshot`).
- **Déduplication btrfs** (`support_btrfs_dedupe`, `Storage.CGI.BtrfsDedupe`).
- **Cache SSD/NVMe** (`support_ssd_cache`, `Flashcache`, `Cache.Protection`),
  avec protection par rejeu de journal.
- **Scrubbing** data/FS, **fast repair**, **resync speed** réglable.

### 8.5 iSCSI

- Cible LIO/TCMU en modules externes, paquets `ScsiTarget`
  (`pkg-iscsi`, `pkg-tcmu-runner`, `pkg-scsi-plugin-server`,
  `pkg-scsit-monitor`).
- API `Core.Storage.iSCSILUN`, LUN file/block, snapshots de LUN, sauvegarde de
  LUN (`support_iscsi_lunbkp`), plugin (`support_iscsi_plugin`),
  `support_iscsi_btrfs_lun`.

### 8.6 Quotas et ACL

- `SYNO.Core.Quota` (get/set/inspect), quotas utilisateur, groupe, partage
  (`support_share_quota`, `support_share_user_quota`).
- **ACL Synology** via `synoacl_vfs` + `synoacltool`, indépendantes des ACL
  POSIX btrfs (désactivées) : modèle NFSv4‑like, rapports de permissions
  (`Core.Share.PermissionReport`).

---

## 9. Réseau

| Domaine | Implémentation |
|---|---|
| Interfaces | `Core.Network.Interface/Ethernet`, MAC clone, MTU, WOL |
| Agrégation | `Core.Network.Bond` (create/set_mode/…), `bonding.ko` |
| VLAN | `8021q.ko`, API Interface |
| OVS | `openvswitch.ko`, outils `ovs-vsctl`/`ovs-vswitchd`/`ovs-docker`, `Core.Network.OVS` |
| IPv6 | `IPv6`, `IPv6.Router`, `IPv6.Router.Prefix` |
| Routage | `Router.Static.Route`, `Router.Gateway.List`, `PPPoE`, `PPPoE.Relay` |
| Proxy | `Core.Network.Proxy` |
| QoS | `sch_htb`, `sch_sfq`, `cls_fw`, `cls_u32`, `TrafficControl.Rules` |
| Pare‑feu | iptables/ipset + `xt_geoip`, `Security.Firewall.{Rules,Profile,Geoip,Adapter}` |
| UPnP | `Network.UPnPServer`, `PortForwarding.*` |
| VPN serveur | L2TP, PPTP, OpenVPN (`xl2tpd`, `pppd`, `openvpn`), VPN Center en paquet |
| VPN client | `Network.VPN.{check,connect,disconnect}`, OpenVPN avec conf |
| DDNS | 25 fournisseurs dans `ddns_provider.conf` (OVH, Cloudflare absent du cœur, No‑IP, DynDNS, TWNIC, Synology…) |
| Découverte | Avahi/Bonjour, WS‑Discovery via nginx de paquet (5357) |

---

## 10. Sécurité et identité

### 10.1 Authentification

- Cookie `sid` + en‑tête **`SynoToken`** ; `device_id` / « known device » ;
  `SYNO.API.Encryption` (échange RSA) ; `SYNO.API.Auth.Key` (clé d'application).
- Politique de mot de passe : `PasswordPolicy`, `PasswordExpiry`,
  `PasswordMeter`, `UsernamePolicy`, confirmation.
- **OTP** TOTP, `OTP.Admin`, `OTP.EnforcePolicy`, `OTP.Mail`, `OTP.Ex`.
- **Secure SignIn** : FIDO2/WebAuthn via `pam_syno_fido.so` et
  `pam_syno_authenticator.so`.
- PAM : `sso`, `codegrant`, `setup_otp`, `auth_key`, `confirm`, `webui` —
  l'OTP et le SSO sont intégrés au login système, pas seulement au portail.

### 10.2 Autorisation et isolation

- **193 fichiers AppArmor**, dont 63 profils de paquets en cache ; profils
  **par handler d'API**.
- Utilisateurs système dédiés (`postgres`, `http`, `SynoRsyncd`, `myds`,
  `tokenmgr`, `taskmgr`, `HybridShareSystem`…), `root` seul, `administrators`
  avec sudo complet, `wheel` en NOPASSWD limité.
- ACL Synology + rapports de permissions.
- `support_app_privilege`, `actionPrivileges` (148 méthodes) : privilèges
  d'action fins, au‑delà du rôle.

### 10.3 Protection réseau et anti‑abus

- `AutoBlock` (+ règles), `DoS`, `SmartBlock` (appareils/utilisateurs de
  confiance), `SecurityScan`, `SecurityAdvisor` (checklists, rapport HTML,
  historique de connexions), `GeoIP`.
- `TrustDevice`, `RescueEmail`, `DisableAdmin`, `ResetAdmin`.
- Firmware et mises à jour **signés** (PKCS#7), checksum par fichier.

### 10.4 Ce que DSM fait mieux que HomeVault ici

- Isolation d'exécution **par API** et privilèges d'action granulaires.
- Intégration PAM (SSO/OTP/FIDO2) au niveau système.
- Chiffrement matériel (TPM), KMIP, coffre de clés HA.
- Signature et chaîne de mise à jour vérifiée de bout en bout.

---

## 11. Sauvegarde, continuité, cloud

### 11.1 Ce qui est réellement dans l'image

| Composant | Rôle |
|---|---|
| `BackupRestoreManager` | **Active Backup for Business Agent (DSM)** : sauvegarde du NAS complet vers un serveur ABB distant |
| `HyperBackup-ED` | **Hyper Backup Entire DSM Extension** : restauration bare‑metal d'un NAS complet |
| `SYNO.Backup.App` / `.Config` | API noyau de sauvegarde/restauration de configuration |
| `SYNO.DisasterRecovery.*`, `SYNO.DR.Node.*` | API noyau de réplication de snapshots et de nœuds DR |
| `SYNO.S2S.*` | Site‑to‑Site |
| `SYNO.Core.CMS.*` | Gestion centralisée multi‑NAS |

Le **client Hyper Backup** complet, **Snapshot Replication** (paquet
`ReplicationService`) et **Active Backup for Business Server** sont des paquets
du Package Center, non installés ici.

### 11.2 Haute disponibilité

- Paquet `HighAvailability` + **DRBD**, **Corosync**, **Pacemaker**, 80+ units.
- Bascule active/passive, *safemode*, synchronisation des clés, reprise de
  session, migration de version coordonnée, moniteurs de services.
- Licence `SYNO.License.HA` (UUID, coffre, login distant).

### 11.3 Cloud et télémétrie (endpoints repérés)

| Canal | Endpoint / protocole | Rôle |
|---|---|---|
| Active Insight | `https://api.insight.synology.com`, `/v2/logs` (HTTPS 443) | métriques, logs, alertes |
| Mise à jour des collecteurs | `https://collectorupdate7.synology.com/getUpdate/v2/collector/2.0.0` | code Python exécuté sur le NAS |
| SupportService | `subscribe.insight.synology.com:443` (**gRPC**, TLS) | canal persistant de **remote actions** |
| Mises à jour d'actions | `https://collectorupdate7.synology.com/getUpdate/v2/action/1.0.0` | scripts d'action téléchargés puis exécutés |
| Diagnostic | `https://supapi.synology.com` | envoi de bundles de support |
| SynoAnalytics | pipeline `SYNO::DataCollect` (OpenTelemetry) | collecte conditionnée au consentement |
| QuickConnect | `global.quickconnect.to`, `relayinfo.quickconnect.to`, `myds.synology.com`, `checkipv6.synology.com` | relais, découverte, IP WAN |
| Auth support | `defch.lp.cs.quickconnect.to:50051` (gRPC) | authentification du canal support |
| Hybrid Share / C2 | `c2fs-*`, `libnats`, API C2 | montage de partages cloud |

**Point d'attention** : `SupportService` maintient un canal **gRPC sortant** et
peut télécharger puis exécuter des scripts Python d'action (`actions/`,
`action_handler/cli.py`). C'est une capacité de support à distance puissante,
activée par l'écosystème, qui n'a **aucun équivalent** dans HomeVault — et qui
justifie le choix « zéro télémétrie » de ce dernier.

Aucun de ces canaux n'est neutre : ils sont proposés/incités au premier
démarrage et supposent un compte Synology pour plusieurs fonctions.

---

## 12. L'écosystème de paquets

### 12.1 Les 19 paquets installés d'usine

| Paquet | Version | Rôle | Units | Particularité |
|---|---|---|---|---|
| ActiveInsight | 3.0.5‑24122 | supervision cloud | 8 | otel-collector, 27 collecteurs Python |
| BackupRestoreManager | 2.5.2‑12503 | agent ABB (DSM) | 4 + target | `synology-backupd`, API `SYNO.BRM` |
| DhcpServer | 1.0.2‑0046 | DHCP LAN | 1 | dnsmasq embarqué |
| FileStation | 1.4.4‑2221 | fichiers | 2 | 30 modules API, `synovfsd`, `thumbd` |
| HighAvailability | 2.1.6‑1745 | cluster actif/passif | 56 | DRBD/Corosync/Pacemaker |
| HybridShare | 1.5.0‑1820 | partages C2 | 9 | `c2fs.ko`, NATS |
| HyperBackup‑ED | 4.2.1‑4228 | image DSM complète | 5 | restauration bare‑metal |
| OAuthService | 1.3.0‑0220 | OAuth 2.0/OIDC | 0 | CGI (`oauth_{login,token,…}.cgi`) |
| Python2 | 2.7.18‑1004 | runtime legacy | 0 | compatibilité |
| QuickConnect | 1.0.10‑0187 | accès distant | 8 | `synorelayd`, WebRTC, gRPC |
| ScsiTarget | 1.0.12‑0802 | SAN Manager | 4 | LIO/TCMU, LUN backup |
| SecureSignIn | 1.1.7‑0455 | FIDO2/MFA | 1 | modules PAM |
| SMBService | 4.15.13‑4209 | SMB/CIFS | 9 | **aucune API propre** (cœur DSM) |
| StorageManager | 1.1.0‑2018 | UI stockage | 0 | UI seule (ExtJS/Vue) |
| SupportService | 2.0.5‑21076 | support distant | 2 | gRPC + actions Python |
| SynoAnalytics | 1.0.4‑20014 | collecte UDC | 1 | OpenTelemetry, opt‑in |
| SynologyUniversalSearch | 1.9.0‑0900 | recherche | 2 | **Lucene** embarqué |
| SynoOnlinePack_v2 | 1071 | base compatibilité disques | 0 | données |
| USBCopy | 2.2.1‑1306 | copie USB | 0 | déclenché par `scemd`/udev |

### 12.2 Catalogue supporté par la plateforme

Les 63 profils AppArmor en cache donnent la liste des paquets que le système
sait faire tourner :

- **Fichiers/productivité** : FileStation, SynologyDrive, SynoFinder, Chat,
  ChatPlus, Calendar, Contacts, NoteStation, Spreadsheet, TextEditor,
  DocumentViewer, PDFViewer, MailClient, MailPlus‑Server.
- **Média** : AudioStation, VideoStation, PhotoStation, SynologyPhotos,
  MediaServer, CodecPack.
- **Sauvegarde/sync** : HyperBackupVault, ActiveBackup‑GSuite,
  ActiveBackup‑Office365, CloudSync, ReplicationService, SynoDataTransferServer.
- **Sécurité** : AntiVirus, AntiVirus‑McAfee, ThreatPrevention, SafeAccess,
  RadiusServer, SecureSignIn.
- **Réseau** : DNSServer, VPNCenter, VPNPlusServer, DirectoryServer,
  DirectoryServerForWindowsDomain, QuickConnect.
- **Plateforme** : Docker, Virtualization, SynologyApplicationGateway,
  SynologyApplicationService, SynologyMeet, PrestoServer, MangoDrive, Git,
  LogCenter, OAuthService, HighAvailability, HybridShare, PetaSpace, Chamber,
  DSMAgent, EOS‑Bridge, iCloudPd‑pkg, USBCopy, BeeCam.

### 12.3 Points d'intégration d'un paquet

Un `.spk` ne se contente pas de copier des binaires : il déclare

- `conf/privilege` (utilisateur/groupe dédié),
- `conf/systemd/*.service` + overrides,
- `resource/port-config` (ports pare‑feu et redirections),
- `resource/pam-config` (intégration PAM),
- `resource/nginx-config` (vhost dédié, ex. WS‑Discovery 5357),
- `resource/syslog-config`, `resource/ha-monitor-config`,
  `resource/user-data-collector`, `resource/feasibility`, `resource/sdk-plugin`,
- `apparmor/`, `indexdb/`, `webapi/*.so`, `ui/`, `texts/`,
- `scripts/` (preinst, postinst, preuninst, postuninst, preupgrade,
  postupgrade, start‑stop‑status, backup/export/import).

C'est le modèle le plus abouti de l'écosystème : **un paquet est un locataire
de première classe**, avec ses ports, sa sécurité, ses services et son aide.

### 12.4 Versions minimales connues

`synopackageslimit.conf` fixe les versions minimales par paquet
(HyperBackup 4.1.3, SynologyDrive 3.5.1, CloudSync 2.7.1, VideoStation 3.2.0,
AudioStation 7.2.0, VPNCenter 1.4.8…). Cela confirme que ces applications
existent pour cette plateforme, même absentes de l'image.

---

## 13. Cartographie fonctionnelle des API

| Domaine | APIs représentatives | Ce qu'elles couvrent |
|---|---|---|
| Comptes | `Core.User`, `Core.Group`, `Core.ACL`, `Core.AppPriv`, `Core.NormalUser`, `Core.UserSettings` | Utilisateurs, groupes, rôles, privilèges d'app, politiques |
| Annuaire/SSO | `Core.Directory.LDAP`, `.SSO.{SAML,OIDC,CAS,IWA}`, `.Azure`, `.Entra`, `.WebSphere`, `DirectoryServiceCheck` | AD/LDAP, SAML, OIDC, CAS, Azure/Entra, WebSphere, diagnostics |
| Stockage | `Core.Storage.{Disk,Pool,Volume,iSCSILUN}`, `Storage.CGI.*` | Disques, pools, volumes, SHR, cache, dédup, KMIP, spares |
| Partages | `Core.Share`, `.Crypto`, `.Snapshot`, `.Permission`, `.Migration` | Création, chiffrement, snapshots, ACL, migration |
| Fichiers | `FileStation.*` (28 API) | Navigation, upload, partage, VFS, WORM, MD5, favoris |
| Services | `Core.FileServ.{SMB,NFS,FTP,Rsync,AFP}` | Protocoles, Kerberos, chroot, découverte |
| Sauvegarde | `Backup.App`, `Backup.Config`, `Backup.Service.NetworkBackup`, `DisasterRecovery`, `DR.Node`, `S2S` | Sauvegarde, réplication, DR, site‑à‑site |
| Réseau | `Core.Network.*` (28 API), `PortForwarding`, `DDNS` | Bond, VLAN, OVS, IPv6, VPN, proxy, QoS, UPnP, DDNS |
| Sécurité | `Security.{Firewall,AutoBlock,DoS,SmartBlock}`, `SecurityScan`, `SecurityAdvisor`, `OTP`, `TrustDevice` | Pare‑feu, anti‑abus, audit, MFA |
| Notifications | `Notification.{Mail,SMS,Push,Line,Webhook}` | Canaux et filtres |
| Système | `Core.System.*`, `ResourceMonitor`, `TaskScheduler`, `Upgrade`, `Package`, `SupportForm` | Supervision, tâches, mises à jour, paquets, support |
| Matériel | `Core.Hardware.*`, `ExternalDevice.*` | Ventilateurs, LED, LCM, hibernation, UPS, imprimantes, Bluetooth |
| Virtualisation | `Core.Virtualization.Host.Capability` | Capacités hôte (VMM en paquet) |
| Média | `MediaIndexing`, `File.Thumbnail`, `PhotoViewer`, `AudioPlayer`, `VideoPlayer2` | Indexation, vignettes, lecteurs |

---

## 14. Comparatif HomeVault ↔ DSM

Légende : ✅ parité · 🟡 partiel ou approche différente · ❌ absent côté
HomeVault · ⭐ HomeVault au‑delà.

### 14.1 Socle, matériel, exploitation

| Capacité | DSM 7.4.1 | HomeVault | Verdict |
|---|---|---|---|
| Installation | Initramfs + installateur + auto‑réparation md0/md1 | Installeur unique, premier démarrage guidé | 🟡 |
| Mise à jour | Signature PKCS#7, ASU, rollback, junior mode | Vérification + notes, installation manuelle | 🟡 |
| Intégration matérielle | 263 modules, `synobios`, LED/LCM/ventilateurs/baies/TPM, 91 feature‑flags | hwmon, SMART, ventilateurs, spindown | 🟡 |
| Noyau | Linux 4.4.302 patché (EOL) | Noyau de la distribution (suivi) | ⭐ |
| Composants upstream | nginx 1.23.1, Samba 4.15, PostgreSQL 11, OpenSSL 1.1.1, PHP 8.1, Redis 4.0 (backports Synology) | paquets de la distribution, maintenus | ⭐ |
| Empreinte | Image 1,5 Go, RAM élevée | Binaire + SQLite, 40–120 Mo au repos | ⭐ |
| Services | 435 units, slices par paquet | systemd + binaire unique | 🟡 |
| Isolation | AppArmor par handler, utilisateurs dédiés | Processus unique, garde `require_auth` | ❌ (DSM devant) |
| Supervision | Resource Monitor, Log Center, SNMP, syslog | Métriques 1 h/24 h/7 j, journal, SNMP, syslog | ✅ |
| Tâches planifiées | Moteur + types extensibles par paquets | Cron générique (commandes, snapshots, SMART, power) | ✅ |

### 14.2 Identité et sécurité

| Capacité | DSM | HomeVault | Verdict |
|---|---|---|---|
| Comptes/groupes/rôles | Local + AD/LDAP, privilèges d'action | Local, rôles admin/user, groupes | ✅ |
| Politique de mot de passe | Longueur, classes, expiration, jauge | 6 règles, pas d'expiration | 🟡 |
| 2FA TOTP | Oui + politique obligatoire | Oui + codes de secours | ✅ |
| Passkeys/FIDO2 | Oui (Secure SignIn, PAM) | Oui (WebAuthn natif) | ✅ |
| Appareils de confiance | Oui | Oui (30 j) | ✅ |
| Récupération | E‑mail de secours | E‑mail + code hors ligne | ⭐ |
| SSO | SAML/OIDC/CAS/Azure/Entra/WebSphere/IWA | SAML/OIDC/LDAP/AD | 🟡 |
| Pare‑feu | Profils, GeoIP, double pile, ipset | ufw, règles | 🟡 |
| Anti‑brute force | AutoBlock + DoS + SmartBlock | Auto‑block + throttle IP | ✅ |
| Anti‑virus | Paquet (absent) | ClamAV à la demande/planifié, **pas de `clamd` temps réel** | ✅ / 🟡 |
| Anti‑ransomware | Non natif (snapshots/ABB) | Honeypots + détection de débit (heuristique assumée) | ⭐ |
| IDS/IPS | Paquet Threat Prevention (non installé) | Absent | ❌ |
| Security Advisor | Oui + rapport HTML | Oui, *fail‑closed* | ✅ |
| Chiffrement matériel | TPM, KMIP, coffre HA | LUKS2 | 🟡 |
| Signature des artefacts | PKCS#7, checksums | Pas de signature d'artefact | ❌ |
| Audit | Log Center + connexions | Journal + audit | ✅ |

### 14.3 Stockage

| Capacité | DSM | HomeVault | Verdict |
|---|---|---|---|
| Disques/SMART/tests | Complet + firmware + remap baies | SMART + tests | ✅ |
| RAID | SHR/SHR2, 0/1/5/6/10/F1, migration, extension | mdadm/ZFS/Btrfs, redondance guidée | 🟡 |
| Volumes | Extents maison (`synorbd`/`syno_extent_pool`), thin | ZFS/Btrfs/mdadm | 🟡 |
| Thin provisioning | Oui (volumes à extents) | Non (pools/datasets uniquement) | ❌ |
| Cache SSD | Cache noyau + protection par journal | Cache tier + mover (pas de L2ARC/SLOG) | 🟡 |
| Déduplication | Btrfs + planification | Non | ❌ |
| Snapshots | Par partage, planifiés, WORM | Par pool uniquement, GFS, immuables | 🟡 |
| Réplication | Core + paquet ReplicationService | Locale/SSH, rétention | ✅ |
| Chiffrement | Volume/partage, TPM, KMIP, clé de récupération | LUKS2 | 🟡 |
| iSCSI | LIO/TCMU, LUN, snapshots, backup | Cible, CHAP, fichier/bloc | ✅ |
| Quotas | Utilisateur/groupe/partage | Dossier, partage, transfert | ✅ |
| Expansion | DX525 + topologie eunit | Non (matériel) | ❌ |
| WORM | Oui | Non (snapshots immuables) | ❌ |
| Réparation système | md0/md1, rootbackup, fast repair | Rescue (ddrescue/TestDisk) | 🟡 |

### 14.4 Fichiers, partage, recherche

| Capacité | DSM | HomeVault | Verdict |
|---|---|---|---|
| SMB | Samba 4.15.x, ACL AD, WS‑Discovery, multicanal | Samba hôte, Time Machine ; **pas d'AD/Kerberos ni de multicanal** | 🟡 |
| NFS | v3/v4 (v4 désactivé par défaut), Kerberos | Export v3/v4 ; **pas de Kerberos**, version globale non appliquée | 🟡 |
| FTP/SFTP/WebDAV | Oui | Oui | ✅ |
| Rsync/AFP | Rsync + AFP legacy | Rsync + SSH | ✅ |
| Explorateur | File Station : partages, VFS (CIFS/NFS/Google Drive), ISO, WORM, MD5 | Uploads repris, tags, corbeille, checksums, ISO, liens | ✅ |
| Recherche | Index contenu + média (flint) | FTS5 opt‑in + nom | ✅ |
| Vignettes | ffmpeg, transcodage, lecteurs | Vignettes image, aperçu | 🟡 |
| Photos/Vidéo | Paquets PhotoStation/Photos/VideoStation | Non | ❌ |
| Étiquettes | Non | Oui, privées | ⭐ |

### 14.5 Sauvegarde, continuité, cloud

| Capacité | DSM | HomeVault | Verdict |
|---|---|---|---|
| Sauvegarde | ABB Agent + HB ED + API config ; Hyper Backup en paquet | Restic + dépôts + planification + restauration | 🟡 |
| Cible distante | Hyper Backup Vault (paquet), rsync | Backup destination append‑only + quotas | ✅ |
| Réplication | Snapshot Replication (paquet) + core | Locale/SSH + rétention | ✅ |
| Cloud sync | CloudSync (paquet), Hybrid Share/C2 | rclone (nombreux backends) | ✅ |
| Sync fichiers | Synology Drive (paquet) | Syncthing | 🟡 |
| HA | DRBD + Corosync + Pacemaker, 80+ units | Non | ❌ |
| DR | DR Node, S2S, CMS | Runbook + config/restic | 🟡 |
| Télémétrie/support | Active Insight, SynoAnalytics, support distant | Aucun | ⭐ |

### 14.6 Réseau

| Capacité | DSM | HomeVault | Verdict |
|---|---|---|---|
| Bond/VLAN/OVS | Complet | 802.3ad + active‑backup, VLAN ; **pas d'OVS** | 🟡 |
| IPv6 | Complet (routeur, préfixes) | auto/DHCP/statique ; **pas de DHCPv6‑PD** | 🟡 |
| Routes statiques / policy routing | Oui | **Absent** (seule la passerelle par défaut) | ❌ |
| PPPoE / proxy client | Oui | **Absent** | ❌ |
| DHCP/TFTP/PXE | Paquet DhcpServer (installé ici) | Oui (dnsmasq) | ✅ |
| DNS local | Paquet DNSServer (non installé) | Oui | ⭐ |
| DDNS | 25 fournisseurs | 4 (DuckDNS, Dynv6, Cloudflare, custom) | 🟡 |
| VPN serveur | L2TP/PPTP/OpenVPN (+ VPN Center) | WireGuard/OpenVPN | 🟡 |
| VPN client | Oui | OpenVPN seulement (pas de client WG/L2TP) | 🟡 |
| Reverse proxy | App Portal | Oui | ✅ |
| UPnP | Oui | Client IGD | ✅ |
| QoS | HTB/SFQ, règles | Par service + planification | ✅ |
| WOL/mDNS | Oui | Oui | ✅ |
| Relais QuickConnect | Oui | Non (choix) | ❌ / ⭐ |

### 14.7 Écosystème et applications

| Capacité | DSM | HomeVault | Verdict |
|---|---|---|---|
| Package Center | 63+ paquets supportés, signature, sandbox | Catalogue Docker | 🟡 |
| Conteneurs | Container Manager (paquet) | Docker + Compose | ⭐ (image) |
| VM | VMM (paquet) + KVM/VFIO | Incus (VMs + conteneurs) | ✅ / ⭐ |
| Média | Photos, Video, Audio, MediaServer | Non | ❌ |
| Productivité | Drive, Office, Calendar, Contacts, Notes, Mail, Chat | Non | ❌ |
| Surveillance | Surveillance Station | Non | ❌ |
| Web hosting | Web Station (non attesté ici) | Non | 🟡 |
| IA/RAG/MCP | Non | Oui | ⭐ |
| Mobile | Apps natives | PWA + Web Push | 🟡 |
| Notifications | Mail/SMS (4 modèles)/Push/Webhook/Line | Mail/Webhook/SMS (générique)/Web Push | ✅ |
| Internationalisation | 21 locales, aide 30 Mo | FR/EN/DE/ES (produit FR) | 🟡 |
| Accessibilité | Non documentée | WCAG 2.2 AA explicite | ⭐ |

### 14.8 Écarts HomeVault vérifiés dans le code (pas dans les docs)

L'audit source confirme les limites suivantes, qui nuancent plusieurs « ✅ »
apparents :

| Écart | Réalité mesurée |
|---|---|
| **SMB AD/Kerberos** | `security=ads` est accepté comme chaîne, mais aucun `realm`, `winbind`, `idmap` ni Kerberos n'est configuré : les partages SMB utilisent les comptes locaux + ACL POSIX (`src/shares.rs:589`). |
| **SMB multicanal** | Aucune directive `server multi channel support` (`src/shares.rs:1408`). |
| **NFSv4/Kerberos** | Pas d'export `sec=krb5`, pas de configuration nfsd ; la version globale est persistée mais jamais rendue dans `/etc/exports` (`src/shares.rs:184`). |
| **Snapshots** | ZFS/Btrfs uniquement, **par pool** ; pas de snapshot par partage, pas de rollback Btrfs en place (`src/snapshots.rs:446`). |
| **RAID** | Extension en ligne limitée à raid1/5/6 ; **pas de migration de niveau** ; spare refusé sur ZFS/Btrfs natif ; SHR = conseil (`src/disk.rs:3121`, `:3502`). |
| **Dédup / thin / cache device** | Absents ; le cache est un « mover » de pool, pas un cache bloc (`src/cache.rs:20`). |
| **Chiffrement** | LUKS2 par volume, **pas de KMIP/coffre**, pas de chiffrement par partage (`src/encryption.rs:189`). |
| **Restic** | Pas de `restic mount` : restauration vers un répertoire seulement (`src/restic.rs:1094`). |
| **Réseau** | Pas de PPPoE, routes statiques/policy routing, proxy client, client WireGuard, serveur L2TP/PPTP ; 2 modes de bond (`src/network.rs:1567`). |
| **Sécurité** | Pas d'IDS/IPS, pas de GeoIP, antivirus à la demande (`clamscan`), anti‑ransomware heuristique (`src/ransomware.rs:1`). |
| **Audit** | Événements via `tracing` + syslog ; pas de table d'audit générale interrogeable (`src/events.rs:1`). |
| **Métriques** | Rétention fixe de 8 jours, non configurable (`src/metrics.rs:10`). |
| **VM** | Incus : catalogue, presets, cloud‑init, snapshots, console/VNC ; pas de clone/migration/export (`src/incus.rs:1`). |
| **Applications** | Pas d'imprimante, DLNA, photothèque, mail, calendrier, NVR, PHP : uniquement des entrées de catalogue Docker. |

À l'inverse, l'audit confirme des points **au‑delà de DSM** : cible de sauvegarde
`rest-server --append-only` par tenant, snapshots immuables tenus par le
système de fichiers (`zfs hold` / sous‑volume Btrfs en lecture seule),
reflink instantané, Security Advisor *fail‑closed*, anti‑ransomware avec
honeypots, conseiller de reachability ordonné, shaping par service excluant
structurellement le management, redondance guidée pour tailles mixtes, et
sauvegarde de configuration auto‑descriptive (`HVCONFIG1`).

---

## 15. Lecture stratégique

### 15.1 Où DSM reste réellement devant

1. **Intégration matérielle** : noyau patché, `synobios`, LED/LCM/ventilateurs,
   baies, expansion, TPM. Impossible à répliquer sans le matériel.
2. **Profondeur stockage** : SHR/SHR2, volumes à extents, migration à chaud,
   dédup, cache noyau, KMIP, WORM, réparation système.
3. **Isolation** : AppArmor par handler + utilisateurs par paquet + slices.
4. **Écosystème** : 63 paquets attestés, intégration ports/PAM/nginx/syslog/HA.
5. **Identité d'entreprise** : AD/LDAP, SAML/OIDC/CAS/Azure/Entra/WebSphere,
   politiques OTP.
6. **Continuité** : HA DRBD/Pacemaker, CMS, DR, S2S.
7. **Mise à jour** : image signée, rollback, junior mode, auto‑réparation.

### 15.2 Où HomeVault est au‑delà

- **Sobriété** : un binaire, SQLite, 1 cœur/1 Go/8 Go, 40–120 Mo au repos.
- **Sécurité produit** : passkeys, code hors ligne, snapshots immuables,
  anti‑ransomware, pas de télémétrie.
- **Données** : restic + réplication + cible append‑only + FTS5 + tags + ⌘K.
- **IA** : assistant local, RAG, MCP.
- **Ingénierie** : 617 routes, 311 fichiers Rust, 96 tests d'intégration,
  35 suites E2E, 78 guides.
- **Accessibilité et simplicité** : WCAG 2.2 AA, 7 règles produit.

### 15.3 Ce qu'il ne faut pas copier

QuickConnect, Active Insight/SynoAnalytics, le canal gRPC de *remote actions*
(`subscribe.insight.synology.com`), ExtJS 3.4, 21 locales + 30 Mo d'aide, la
prolifération de paquets, le verrouillage C2/Hybrid Share, le noyau 4.4 patché
et l'empilement de composants upstream anciens (PostgreSQL 11, OpenSSL 1.1.1,
Samba 4.15, PHP 8.1, Redis 4.0, btrfs-progs 4.0).

### 15.4 Ce qui mérite d'être repris (déjà partiellement planifié)

- **Isolation d'exécution** : `systemd` durci par service, capacités minimales,
  utilisateur dédié par sous‑système, et AppArmor si disponible. C'est le seul
  déficit structurel réel face à DSM.
- **Mise à jour** : signature des artefacts + installation atomique + rollback.
- **Réparation système** : équivalent `md0`/rootbackup, ou au minimum une
  procédure de restauration bare‑metal testée.
- **RAID** : migration de niveau et extension à chaud (aujourd'hui : création
  et redondance guidée).
- **SMB AD/Kerberos** et **NFSv4/Kerberos** : le chaînage winbind/realm/idmap
  et les exports `sec=krb5` manquent pour un usage entreprise.
- **Thin provisioning** : les volumes à extents de DSM n'ont pas d'équivalent.
- **Chiffrement** : coffre de clés/KMIP pour les sauvegardes d'entreprise.
- **WORM** : verrou par fichier en plus des snapshots immuables.

> **Note de cohérence** : `docs/analysis/dsm-parity-implementation-plan.md`
> (2026‑11‑02) avait déjà relevé que 13 des 20 « manques » supposés étaient
> livrés. Les 22 routes non authentifiées de la Phase 4 ont depuis été
> corrigées (`require_auth`/`require_admin` présents dans `dhcp_server`,
> `usb_copy`, `fan_control`, `ups_server`, `guided_redundancy`). Ce document ne
> répète donc pas ces constats périmés.

---

## 16. Limites de l'analyse

- **Analyse statique** : pas d'exécution du firmware. Les conclusions reposent
  sur les binaires, descripteurs, bundles, units, manifestes, scripts
  d'initramfs et configuration par défaut.
- **Image partielle** : 19 paquets installés. L'absence d'une application ici
  ne signifie pas son absence dans DSM ; le catalogue est attesté par les
  profils AppArmor et `synopackageslimit.conf`.
- **Source partielle** : seul le noyau GPL est fourni, pas les binaires
  propriétaires (`synostg*`, `synoscgi`, `synobios`).
- **Volumétrie non comparable** : DSM est compilé ; les LOC HomeVault ne se
  comparent pas aux binaires C++.
- **Versions** : DSM 7.4.1‑90080 (juillet 2026) ; HomeVault `c37c803`
  (04/11/2026). Les statuts peuvent évoluer vite.

---

## 17. Annexe — métriques, chemins, preuves

### 17.1 DSM — chemins clés

| Constat | Chemin / commande |
|---|---|
| Version, paquets | `VERSION` |
| Modèle, flags | `hda1/etc.defaults/synoinfo.conf` (345 clés, 91 `support_*=yes`) |
| Boot | `rd.gz` (LZMA → cpio), `linuxrc.syno`, `linuxrc.syno.impl`, `etc/rc` |
| Installateur | `etc.defaults/synogrinst.sh`, `upgrade.sh`, `bootup-smallupdate.sh` |
| Noyau | `sources/7.4-90080/apollolake/linux-4.4.x` (`synoconfigs/`, `SynoBuildConf/_modules`) |
| Modules | `hda1/lib/modules/*.ko` (263) |
| Routage SCGI | `hda1/etc.defaults/nginx/nginx.conf.default` |
| Dispatcher | `hda1/usr/syno/synoman/webapi/entry.cgi` |
| Descripteurs | `hda1/usr/syno/synoman/webapi/*.lib` |
| OpenAPI | `hda1/usr/syno/synoman/webapi/openapi/core/web/v1/openapi.json` |
| Front | `hda1/usr/syno/synoman/{scripts,webman,synoSDSjslib}` |
| Thème | `hda1/usr/syno/theme/theme.json` |
| Stockage | `hda1/usr/syno/{bin,sbin}/synostg*`, `spacetool`, `synostoragecore` |
| HA | `packages_extracted/HighAvailability-*/conf/systemd/pkg-synoha-*` |
| iSCSI | `packages_extracted/ScsiTarget-*/conf/systemd/pkg-*` |
| AppArmor | `hda1/usr/share/apparmor.d/` (193), `pkg_cache/` (63) |
| Signature | `Synology.sig` (PKCS#7), `checksum.syno` |
| Catalogue | `hda1/etc.defaults/synopackageslimit.conf` |

### 17.2 HomeVault — baseline `c37c803`

| Indicateur | Valeur mesurée |
|---|---|
| Routes HTTP | 617 |
| Fichiers Rust / LOC | 311 / ~203 000 |
| Composants Svelte / LOC | 164 / ~66 500 |
| Migrations SQL | 78 |
| Tests d'intégration | 96 fichiers |
| Suites E2E | 35 répertoires |
| LOC de tests | ~71 000 |
| Guides `docs/guides/` | 78 |

### 17.3 Correspondances DSM → HomeVault

| Concept DSM | Équivalent HomeVault |
|---|---|
| `.lib` + `.so` + `synoscgi` | `src/web/mod.rs` (Axum) + `src/handlers/*` + `admin_guard` |
| `synobios`/`syno_hddmon` | `src/fan_control.rs`, `src/disk.rs`, `src/smart.rs`, `src/power.rs` |
| `synorbd`/`syno_extent_pool`/SHR | `src/storage.rs`, `src/pools.rs`, `src/guided_redundancy.rs` |
| btrfs snapshots | `src/snapshots.rs`, `src/snapshot_schedules.rs`, `src/snapshot_locks.rs` |
| `ReplicationService` | `src/replication.rs` |
| Active Backup Agent | `src/restic.rs`, `src/backup_target.rs` |
| CloudSync | `src/rclone.rs` |
| Synology Drive | `src/syncthing.rs` |
| Secure SignIn | `src/passkey.rs`, `src/identity.rs` |
| SSO DSM | `src/oidc.rs`, `src/saml.rs`, `src/directory.rs` |
| Security Advisor | `src/security_advisor.rs` |
| Log Center | `src/journal.rs`, `src/logging.rs` |
| Resource Monitor | `src/metrics.rs`, `src/health.rs` |
| Task Scheduler | `src/scheduled_tasks_scheduler.rs` |
| AppArmor par API | *(à construire : systemd durci + profils)* |
| QuickConnect | *(rejeté : ACME + DDNS + reachability + VPN)* |

### 17.4 Reproduire les mesures

```bash
# 1. Initramfs (LZMA + cpio)
xz -dc --format=lzma FW/rd.gz > /tmp/rd.cpio
mkdir -p /tmp/rd && cd /tmp/rd && cpio -idm --quiet < /tmp/rd.cpio

# 2. API : nombre d'API et de méthodes
cd FW/hda1/usr/syno/synoman/webapi
python3 - <<'PY'
import json,glob
apis=methods=0
for f in glob.glob('*.lib'):
    d=json.load(open(f)); apis+=len(d)
    for v in d.values():
        for ms in v.get('methods',{}).values(): methods+=len(ms)
print(apis, methods)
PY

# 3. Noyau : options Synology
cd SOURCES/7.4-90080/apollolake/linux-4.4.x
grep -rhoE 'config SYNO_[A-Z0-9_]+' synoconfigs/ | sort -u | wc -l

# 4. Isolation
find FW/hda1/usr/share/apparmor.d -type f | wc -l
ls FW/hda1/usr/share/apparmor.d/pkg_cache | wc -l

# 5. Versions
strings FW/hda1/usr/bin/nginx | grep -m1 'nginx/'
strings FW/hda1/usr/bin/postgres | grep -m1 'PostgreSQL'
strings FW/packages_extracted/SMBService-*/usr/sbin/smbd | grep -m1 '4\.15\.'
```

### 17.5 Glossaire

| Terme | Signification dans ce document |
|---|---|
| **SCGI** | Protocole de passerelle utilisé par nginx vers `synoscgi` (équivalent FastCGI, plus simple). |
| **`.lib`** | Descripteur JSON d'une API : autorisations, méthodes, capacités. |
| **`.spk`** | Archive de paquet Synology (manifeste `INFO` + `package.tgz` + scripts). |
| **junior mode** | DSM minimal sans paquets, utilisé pour installer/réparer. |
| **md0 / md1** | RAID1 système (racine) et swap, répliqués sur chaque disque. |
| **SHR / SHR2** | Synology Hybrid RAID (1 ou 2 tolérances), au‑dessus de mdadm/LVM. |
| **RBD / extent pool** | Couche de volume maison Synology (`synorbd`, `syno_extent_pool`). |
| **KMIP** | Protocole de gestion de clés externes (KMS). |
| **WORM** | Write Once Read Many : verrouillage immuable de fichiers. |
| **AppArmor** | LSM Linux utilisé pour confiner chaque handler d'API. |
| **DRBD / Pacemaker** | Réplication bloc et gestion de cluster pour la HA. |
| **TCMU / LIO** | Cible iSCSI moderne (kernel + `tcmu-runner` user space). |
