# HomeVault — Tasklist « à faire »

Backlog opérationnel dérivé de [`ANALYSE-DSM-vs-HOMEVAULT.md`](./ANALYSE-DSM-vs-HOMEVAULT.md)
(parité) et de
[`docs/analysis/dsm7-reliability-integrity-review.md`](./docs/analysis/dsm7-reliability-integrity-review.md)
(fiabilité, stabilité, intégrité), trié par **priorité puis facilité
d'implémentation**.

> **Ce fichier n'est pas un engagement de scope.** Un item devient du travail
> quand il passe en `[~]` avec un owner et une branche, et qu'il respecte la
> définition de « terminé » du §0.3. `CLAUDE.md` reste la règle ; `ROADMAP-DSM.md`
> (supprimé du worktree au 04/11/2026) et `TASKLIST NICOLAS.md` ne sont pas
> remplacés par ce document.

| Champ | Valeur |
|---|---|
| Sources | `ANALYSE-DSM-vs-HOMEVAULT.md` (parité, `c37c803`) + `docs/analysis/dsm7-reliability-integrity-review.md` (fiabilité/intégrité, `3afe03e`) |
| Date | 2026‑11‑06 |
| Périmètre | écarts **vérifiés dans le code** : parité fonctionnelle (§14.8/§15.4 de l'analyse) **et** fiabilité/stabilité/intégrité des fichiers et transferts (revue du 05/11), revue de valeur du 06/11 |
| Hors périmètre | ce qui est déjà livré (voir §5, avec les réserves d'intégrité) et les non‑objectifs assumés (voir §6) |

---

## 0. Mode d'emploi

### 0.1 Légende

| Priorité | Signification |
|---|---|
| **P0** | Corrige un comportement **trompeur ou dangereux** : une option acceptée mais non appliquée, une promesse UI fausse, un risque sécurité/données. À faire avant toute nouveauté. |
| **P1** | Valeur utilisateur forte, effort raisonnable. |
| **P2** | Chantier structurant : utile mais lourd, ou destiné à un usage avancé. |
| **P3** | Niche ou rejeté : documenté pour ne pas le rejouer. |

| Effort | Ordre de grandeur |
|---|---|
| **S** | ≤ 2 jours |
| **M** | 3–5 jours |
| **L** | 2–3 semaines |
| **XL** | ≥ 4 semaines, ou nouveau sous‑système |

État : `[ ]` à faire · `[~]` en cours · `[x]` fait · `[-]` rejeté.

### 0.2 Ordre de lecture

Le tableau §1 est trié **P0 → P3**, puis **S → XL** à l'intérieur de chaque
priorité. Les cartes détaillées (§2–§4) suivent le même ordre. Le §7 propose un
découpage en sprints ; le §8 relie chaque tâche à la section de l'analyse qui la
justifie.

### 0.3 Définition de « terminé » (obligatoire pour chaque item)

- [ ] Code + migration SQL si l'état est persistant.
- [ ] `require_auth` sur les lectures, `require_admin` sur les mutations ;
      test `401` sans session et `403` pour un non‑admin.
- [ ] Toute commande privilégiée est en **argv** (jamais de shell), avec
      `tokio::time::timeout` et une constante nommée.
- [ ] Capacité absente → **`503`** nommé (`json_unavailable`) ; fonction non
      supportée par ce build → **`501`** (`json_not_implemented`). Jamais un
      faux succès.
- [ ] Chaînes via clés i18n (`src/i18n/*.toml`), `aria-label` sur les boutons
      icône, cible 44 px, focus visible, un seul `<h1>`.
- [ ] Tests : unitaires (`#[cfg(test)]`), intégration
      (`tests/<area>_integration.rs`), E2E (`tests/e2e/<screen>/`).
- [ ] Docs dans le même changement : `docs/guides/<area>.md`, `docs/API.md`,
      `docs/INDEX.md`, `SCREENS.md`, `TESTING.md`, `FeatureDoc` dans
      `src/rag_indexers.rs`, ligne dans `docs/guides/rag.md`.
- [ ] **Toute promesse de vérification** (checksum, intégrité, « vérifié »)
      est prouvée par un test de **corruption réelle** : octet modifié, pack
      tronqué, superbloc abîmé, coupure entre l'écriture et le `rename`.
- [ ] **Toute écriture de configuration système** est atomique (temp + `fsync`
      + `rename`), validée par l'outil natif (`testparm`, `exportfs`, …), et
      conserve un `.bak` sain.
- [ ] **Toute opération lourde** (scrub, `restic check`, balance, resync) est
      planifiable, reprenable après redémarrage et observable (dernier
      résultat, erreurs, durée).

---

## 1. Tableau de bord (trié)

| # | Tâche | Prio | Effort | Valeur |
|---|---|---|---|---|
| **P0-1** | NFS : appliquer `version`, `root_squash`, `networks`, `default_access`, `custom_options` (ou les refuser) | P0 | S | Empêche une exposition NFS avec la mauvaise politique |
| **P0-2** | SMB : refuser `security = ads/domain/server` tant que winbind n'est pas câblé | P0 | S | Évite un SMB qui ne démarre pas / n'authentifie pas |
| **P0-3** | SMB : `additional_params` et `max protocol` réellement appliqués (ou retirés de l'API/UI) | P0 | S | Supprime une config fantôme |
| **P0-4** | Test de contrat « tout champ persisté est rendu » pour SMB/NFS/FTP | P0 | S | Empêche la réapparition des champs morts |
| **P0-5** | USB Copy : la vérification doit comparer les **contenus**, pas seulement la taille ✅ | P0 | S/M | Une copie corrompue n'est plus déclarée « vérifiée » |
| **P0-6** | Scrub planifié, repris après reboot et déclenché après arrêt sale ✅ | P0 | M/L | Corruption détectée sans attendre une action manuelle |
| **P0-7** | Récupération de montage et fsck au démarrage (Btrfs/ext4/mdadm) ✅ | P0 | L | Un superbloc abîmé ne laisse plus le pool démonté |
| **P0-8** | Historique SMART, erreurs I/O et alertes de seuil ✅ | P0 | M | Disque mourant détecté avant la panne |
| **P0-9** | Écritures de configuration atomiques, validées et auto‑réparées ✅ | P0 | S/M | Une coupure ne tronque plus `smb.conf`/exports |
| **P0-10** | `restic check` planifié avec lecture de données (`--read-data-subset`) ✅ | P0 | M | Une sauvegarde « OK » est réellement vérifiée |
| **P0-11** | SMB : durable handles, chiffrement, `strict sync`, AIO first‑class ✅ | P0 | M | Sessions survivantes et durabilité explicite |
| **P0-12** | iSCSI : FUA/Sync Cache, write cache, UNMAP, protection volume plein ✅ | P0 | M | Écritures acquittées réellement durables |
| **P1-1** | SMB multichannel (`server multi channel support`) | P1 | S | Débit 2,5/10 GbE |
| **P1-2** | Expiration de mot de passe (politique + UI + garde) | P1 | S | Conformité / comptes de service |
| **P1-3** | Rétention des métriques configurable (8/30/90 j) | P1 | S | Supervision utile au‑delà de 8 jours |
| **P1-4** | OpenAPI 3 généré depuis le contrat | P1 | S | Intégrations, MCP, doc vivante |
| **P1-5** | Fournisseurs DDNS supplémentaires (OVH, Namecheap, deSEC) | P1 | S | Moins de « custom » à la main |
| **P1-6** | Incus : clone de VM/conteneur | P1 | S | Duplication rapide |
| **P1-7** | Routes statiques IPv4/IPv6 | P1 | S/M | Réseau domestique segmenté |
| **P1-8** | Proxy client HTTP sortant ✅ (SOCKS refusé, raison nommée) | P1 | S/M | Réseaux contraints |
| **P1-9** | ZFS `raidz3` + hot spare sur pools natifs ✅ | P1 | S/M | Tolérance 3 disques |
| **P1-10** | `restic mount` (navigation d'un dépôt) ✅ | P1 | M | Restauration sélective |
| **P1-11** | Migration de niveau RAID (`mdadm --grow --level`) ✅ | P1 | M | Passer de raid1 à raid5 sans tout recréer |
| **P1-12** | Snapshots par partage (et non par pool) ✅ | P1 | M/L | Politique fine par dossier |
| **P1-13** | Rollback Btrfs en place (layout `@`/`@snapshots`) ✅ | P1 | M/L | Parité ZFS |
| **P1-14** | Pare‑feu GeoIP (pays) ✅ | P1 | M | Réduction de surface exposée |
| **P1-15** | SNMP v3 (auth/priv) | P1 | M | Supervision d'entreprise |
| **P1-16** | E‑mail OAuth2 (Gmail/Microsoft) | P1 | M | SMTP moderne sans mot de passe applicatif |
| **P1-17** | Client WireGuard | P1 | M | Site‑à‑site inverse |
| **P1-18** | Table d'audit interrogeable — livrée 2026‑11‑08 (filtres structurés UI restants) | P1 | M | Enquête après incident |
| **P1-19** | `clamd` temps réel (option, hors profil basse conso) — **rejeté pour la cible** (coût RAM permanent) | P1 | M | Antivirus à l'écriture |
| **P1-20** | PPPoE — **rejeté pour la cible** (la box FAI le fait déjà) | P1 | M | Box en bridge |
| **P1-21** | RAID : vitesse de resync, moniteur `mdadm`, spare auto sur disque « Critical » — **partiel assumé** (suggestion seulement ; ajout auto rejeté) | P1 | M | Reconstruction contrôlée, alerte avant panne |
| **P1-22** | Btrfs : `compress=zstd`, `space_cache=v2`, balance conditionnel, TRIM planifié ✅ | P1 | M | Maintenance et espace disque sains |
| **P1-23** | Import/assemblage mdadm étranger (« online assemble ») ✅ | P1 | S/M | Récupérer un pool sans recréer |
| **P1-24** | Digest des gros téléchargements + checksums de chunks d'upload ✅ | P1 | M | Intégrité prouvée au‑delà de 64 Mio |
| **P1-25** | Vérification finale rsync/rclone (`--checksum`, `rclone check`) ✅ | P1 | S/M | Une sync n'est pas « OK » sans comparaison |
| **P1-26** | Réplication : Btrfs over SSH + vérification destination ✅ | P1 | M/L | Parité ZFS et copie distante prouvée |
| **P1-27** | Chaîne d'approvisionnement : images Docker par digest + signature du catalogue — **partiel assumé** (signature à finir ; pinning obligatoire rejeté) | P1 | M | Pas d'image/catalogue substitué |
| **P1-28** | Stabilité système : `LimitNOFILE`, `sysctl`, `smartd`/`mdmonitor`, write cache + UPS — **partiel assumé** (avertissement installateur suffit ; carte UI rejetée) | P1 | S/M | Moins de coupures évitables |
| **P1-29** | SMB WS-Discovery (découverte Windows) — livré 2026‑11‑08 | P1 | S | Découverte sans config |
| **P1-30** | Mises à jour Docker planifiées/auto — livré 2026‑11‑09 | P1 | S/M | Hygiène de patch |
| **P2-1** | SMB AD/Kerberos complet (winbind, realm, idmap, keytab) — **gelé au partiel** (config ADS + join/leave ; DC réel seulement si cible entreprise) | P2 | L | Entreprise Windows |
| **P2-2** | NFSv4 + Kerberos (`sec=krb5`) — **gelé au partiel** (flavor + validation keytab ; KDC réel seulement si cible entreprise) | P2 | L | Entreprise Unix |
| **P2-3** | Thin provisioning — **gelé au partiel** (zvols ZFS thin/thick ; overcommit/LVM rejetés) | P2 | L | Sur‑allocation |
| **P2-4** | Déduplication — **gelé au design** (planification `duperemove` ; exécution après benchmark seulement) | P2 | XL | Économie d'espace |
| **P2-5** | KMIP / coffre de clés — **rejeté pour la cible** (dépendance critique au boot) | P2 | XL | Clés externalisées |
| **P2-6** | WORM par fichier — **rejeté pour la cible** (les snapshots immuables suffisent) | P2 | M/L | Immuabilité légale |
| **P2-7** | Isolation d'exécution (systemd durci + profils AppArmor) — **priorité**, installeur livré 2026‑11‑08 (enforce/terrain restants) | P2 | L | Défense en profondeur |
| **P2-8** | Mise à jour signée + atomique + rollback — **priorité**, installeur livré 2026‑11‑08 (UI restante) | P2 | L/XL | Intégrité de la chaîne |
| **P2-9** | Réparation/restauration bare‑metal testée — **priorité**, drill non destructif livré 2026‑11‑08 (test VM restant) | P2 | L | Reprise après sinistre |
| **P2-10** | Prédiction de panne disque par régression SMART (pas de ML) ✅ | P2 | M | Anticiper le remplacement |
| **P2-11** | Compression/défragmentation Btrfs (hors dédup P2-4) ✅ | P2 | M | Espace et perf |
| **P2-12** | Rapport de santé hebdomadaire (scrub, SMART, I/O, backups) ✅ | P2 | S/M | Visibilité sans surveillance |
| **P2-13** | Snapshots applicatifs (quiescence bases/VM) ✅ | P2 | L | Passer de crash‑ à application‑consistent |
| **P2-14** | Durabilité SQLite (`synchronous=FULL`/`fullfsync`) ✅ | P2 | S | Dernière transaction survit à la coupure |
| **P2-15** | Témoin de réplication (witness) sans cluster DRBD — **gelé** (client HTTP + lease ; service/recovery externes, hors cible mono-site) | P2 | XL | Éviter le split‑brain sans HA lourd |
| **P3-1** | HA cluster, IDS/IPS, QuickConnect, télémétrie | P3 | — | Rejeté (voir §6) |

> **État au 2026-11-05** : **P0-1 à P0-4, P1-1 à P1-15 et P1-17 sont livrés** (code,
> tests, docs, i18n). `cargo check --lib`, `cargo test --lib --no-run`,
> `cargo test --test proxy_integration --no-run`,
> `cargo test --test raid_integration --no-run`,
> `cargo test --test restic_integration --no-run`, `npm run check` et
> `npm run build` sont verts. Les **tests n'ont pas été exécutés** (consigne
> du mainteneur) :
> lancer `cargo test --lib`, `cargo test --test shares_integration`,
> `cargo test --test password_policy_integration`,
> `cargo test --test metrics_integration`, `cargo test --test
> openapi_integration`, `cargo test --test ddns_integration`,
> `cargo test --test vm_integration`, `cargo test --test network_integration`,
> `cargo test --test proxy_integration`, `cargo test --test raid_integration`,
> `cargo test --test restic_integration`,
> `cargo test --test share_snapshots_integration`,
> `cargo test --test snapshot_schedule_integration`,
> `cargo test --test storage_integration`, `cargo test --test
> security_integration`, `cargo test --test snmp_integration`,
> `cargo test --test vpn_integration`, puis les E2E
> Shares, Settings 12.7, Metrics 2.2, SNMP agent (v2c + v3),
> Dynamic DNS, Outbound proxy, Virtual Machines, Network, RAID, Storage
> 14.6/14.7 (migration de niveau) et 14.8/14.9 (layout/rollback Btrfs),
> Snapshot schedules 5.1/5.2 (snapshots par partage), Security 5.15 (GeoIP),
> Backup et VPN 15.1/15.2 (client WireGuard) avant de considérer le lot vert.
>
> **Nouveau lot intégrité (2026‑11‑05, HEAD `3afe03e`)** : la revue
> `docs/analysis/dsm7-reliability-integrity-review.md` ouvre **P0-5 à P0-12**,
> **P1-21 à P1-28** et **P2-10 à P2-15**.
>
> **P0 : le cœur de tout le lot est livré** — P0-5 (vérification USB
> réelle), P0-6 (scrub planifié, post-arrêt sale et reprise exacte via
> `state/scrub-in-progress`), P0-7 (récupération de montage/fsck), P0-8
> (historique SMART/I/O), P0-9 (écritures atomiques + `testparm`/self-heal),
> P0-10 (`restic check --read-data-subset`), P0-11 (SMB durable
> handles/chiffrement/`strict sync`/AIO) et P0-12 (attributs LIO +
> protection volume plein). **Aucune réserve fonctionnelle P0** : le test
> matériel `dm-flakey`/superbloc Btrfs de P0-7 est livré (opt-in), et la
> récupération en `ro` lève désormais une notification.
>
> **P1 : livrés** P1-22 (options + boucle balance/TRIM), P1-23, P1-24, P1-25
> (vérification + `transfer_history`) et P1-26. **Partiels** : P1-21 (profils,
> moniteur et suggestion de spare ; ajout du spare toujours manuel), P1-27
> (digest mal formé refusé, pinning strict opt-in ; signature du catalogue non
> imposée) et P1-28 (unité/sysctl + avertissement installateur ; avertissements
> UI non exposés).
>
> **P2 : livrés** P2-10 (prédiction + carte UI), P2-11 (politiques de
> compression + onglet Storage + boucle mensuelle), P2-12 (rapport
> hebdomadaire) et P2-13 (hooks de quiescence + UI + exécution autour des
> snapshots). **Partiels** : P2-1 (config ADS + join/leave), P2-2 (flavor
> Kerberos + validation keytab), P2-3 (zvols ZFS thin/thick + saturation),
> P2-4 (conception + planification `duperemove`, sans exécution) et P2-15
> (client HTTP + lease appliqué aux runs ; service témoin et recovery
> split-brain externes).
>
> Les P0 sont fonctionnellement terminés. Après la revue de valeur du
> 2026‑11‑06 (ci-dessous), la suite prioritaire est **P2-7 (enforce/terrain),
> P2-8 (UI), P1-18 (filtres UI), P2-9 (test VM), P1-16**, puis les finitions
> P1-27/P1-28.
> Les autres items sont gelés ou rejetés pour la cible actuelle.

---

## 1bis. Revue de valeur du backlog (2026‑11‑06)

Critères : cible **NAS domestique / petite structure**, 1–3 utilisateurs,
faible consommation, simplicité, sécurité et charge de maintenance limitée.
Un item n’est gardé que s’il apporte un gain concret sans ajouter un mode de
panne majeur.

> **Référence croisée (2026‑11‑08).** `COMPARAISON_DSM_VS_HOMEVAULT.md` §3 et
> §6 appliquent désormais cette revue : les écarts DSM y sont classés
> « garder / recadrer / geler‑rejeter » et le filtre §6 s’applique à **toute**
> future implémentation. En cas d’écart entre les deux documents, **la présente
> §1bis fait foi**.

| Décision | Items | Justification |
|---|---|---|
| **Priorité** | **P2-7** isolation d’exécution (**installeur livré 2026‑11‑08**, enforce/terrain restants), **P2-8** mises à jour signées/atomiques (**installeur livré 2026‑11‑08**, UI restante), **P1-18** table d’audit (**livrée 2026‑11‑08**, filtres UI restants), **P2-9** restauration bare-metal (**drill livré 2026‑11‑08**, test VM restant) | Sécurité, intégrité de la chaîne et reprise après sinistre : gains réels, sans dépendance externe. |
| **À faire, plus tard** | **P1-16** OAuth2 e-mail | Gmail/Microsoft retirent les mots de passe applicatifs ; les alertes peuvent cesser de partir. Coût modéré. |
| **Finitions seulement** | **P1-27** signature du catalogue (pas de pinning obligatoire), **P1-28** avertissement installateur (pas de carte UI), **P1-21** suggestion de spare (pas d’ajout auto) | Le cœur est déjà livré ; le reste ajoute du risque ou peu de valeur. |
| **Gelé au stade partiel** | **P2-1** SMB AD, **P2-2** NFS Kerberos, **P2-3** thin provisioning, **P2-4** déduplication, **P2-15** witness | Utiles seulement en entreprise/multi-site ou après benchmark ; ne pas investir sans cible explicite. |
| **Rejeté pour la cible** | **P2-5** KMIP, **P2-6** WORM, **P1-19** clamd temps réel, **P1-20** PPPoE, **P2-3** partie overcommit LVM/UI (le support zvol reste), **P2-4** exécution automatique (le design reste) | Complexité, coût permanent ou mode de panne (boot dépendant du réseau, saturation imprévisible, verrouillage admin) disproportionnés. |

**Décisions détaillées.**

- **P2-5 KMIP : rejeté.** Ajoute un point de défaillance critique au boot
  (serveur KMIP, TLS/mTLS, HA, recovery) pour un bénéfice quasi nul sur un NAS
  familial. Alternatives : TPM 2.0, clé USB hors ligne, Vault Transit.
- **P2-6 WORM : rejeté.** Les snapshots immuables couvrent déjà
  l’anti-ransomware ; `chattr +i` + break-glass ajoute un risque de
  verrouillage administratif.
- **P1-19 clamd : rejeté.** Coût RAM permanent sur une machine low-power pour
  un risque surtout Windows ; l’antivirus à la demande suffit.
- **P1-20 PPPoE : rejeté.** La box FAI fait déjà PPPoE dans la quasi-totalité
  des foyers.
- **P2-3 : arrêté au support zvol.** Pas d’overcommit LVM/UI : une saturation
  imprévisible est exactement ce que HomeVault doit éviter.
- **P2-4 : design conservé, exécution gelée.** Les snapshots/reflinks Btrfs
  couvrent l’essentiel ; ne coder que si le benchmark de
  `docs/analysis/dedup-design.md` démontre un gain réel.
- **P2-15 : gelé.** Suppose deux sites + un troisième témoin ; hors cible
  actuelle. Le client HTTP et le lease restent disponibles mais non promus.
- **P2-1/P2-2 : gelés au partiel.** La configuration et les validations sont
  livrées ; la validation DC/KDC réelle n’a de sens que si la cible
  entreprise est confirmée.
- **P2-7/P2-8 : cœur livré côté installeur le 2026‑11‑08**, avec tests de
  non-régression (HomeVault utilise `sudo`, donc l’isolation doit être mesurée,
  pas supposée). Restent l’enforce AppArmor + la passe terrain pour P2-7, et le
  déclenchement UI + la clé de production pour P2-8.
- **P2-9 : runbook + drill d’abord (livrés le 2026‑11‑08).** Le drill vérifie
  le snapshot DB et l'archive de configuration sans toucher aux données ; reste
  le test de reconstruction complète sur VM, qui vaut mieux qu’une usine à gaz
  d’images système.

---

## 2. P0 — Corriger ce qui ment ou casse

### [x] P0-1 — NFS : les réglages globaux ne sont pas appliqués

**Livré 2026-11-05.** `NfsConfig` est validé (`validate_nfs_config`), rendu par
`nfs_export_line` (clauses client par réseau, `vers=`, squash, options
custom, override par classe), et `save_nfs_config` réécrit toutes les lignes
d'`/etc/exports` puis `exportfs -ra`. `version` par défaut = `auto`
(négociation, comportement historique).

**Constat.** `NfsConfig` expose `networks`, `default_access`, `version`,
`root_squash`, `custom_options` (`src/shares.rs:184‑202`) et l'API les accepte
(`src/handlers/shares.rs:515‑541`). Or `nfs_export_line` n'utilise que les
`options` du partage et retombe sur `rw,sync,no_subtree_check`
(`src/shares.rs:922‑930`) : **aucun des champs globaux n'atteint
`/etc/exports`**. Un utilisateur qui choisit « NFSv4 » ou « no_root_squash »
obtient silencieusement autre chose — potentiellement une politique de root
plus permissive ou plus restrictive que demandée.

**Action.** Rendre la configuration réelle :

1. Construire la clause client à partir de `networks` (remplacer le `*` codé en
   dur), avec validation CIDR/hostname.
2. Ajouter `default_access`, `root_squash`/`no_root_squash` et
   `vers=<version>` aux options par défaut.
3. Ajouter `custom_options` après validation (`validate_nfs_options` existe
   déjà).
4. Si un champ ne peut pas être appliqué sur cette plateforme, le retirer de
   l'API et de l'UI plutôt que de le persister.

**Fichiers.** `src/shares.rs` (`nfs_export_line`, `validate_nfs_config`),
`src/handlers/shares.rs`, `frontend/src/lib/screens/Shares.svelte` (onglet NFS).

**Acceptation.**
- Un test « golden » compare `/etc/exports` généré pour 3 configurations
  (réseau unique, `no_root_squash`, `vers=4.1`).
- L'UI affiche exactement ce qui sera écrit ; un champ non supporté est grisé
  avec une raison.
- `nfs_config_get` renvoie l'état effectif, pas seulement l'état persisté.

**Tests.** Unitaires dans `shares.rs` (golden + injection `*` refusée),
intégration `tests/shares_integration.rs`, E2E onglet NFS.

---

### [x] P0-2 — SMB : `security = ads` est accepté mais winbind n'existe pas

**Livré 2026-11-05.** `domain`, `server` et `ads` sont refusés avec un message
nommant winbind/Kerberos ; l'UI ne propose plus que *User* et *Share*.

**Constat.** `validate_smb_config` accepte `user|share|domain|server|ads`
(`src/shares.rs:591‑598`) et `smb.conf` écrit `security = <valeur>`
(`src/shares.rs:1421`). Aucun `realm`, `winbind`, `idmap`, `net ads join` ni
Kerberos n'est configuré (`grep winbind src/` = 0). Sélectionner « ads » produit
donc un SMB au mieux inopérant, au pire en échec de démarrage.

**Action (S, immédiat).** Refuser `ads`, `domain` et `server` avec un `501`
nommé (« domain authentication is not implemented — use local accounts or
configure winbind support »), retirer ces valeurs du sélecteur UI, et garder
`user`/`share`. Documenter la limitation dans `docs/guides/shares.md`.

**Action (L, P2-1).** Implémenter AD/Kerberos : paquet winbind, `realm`,
`idmap`, keytab, `net ads join`, PAM `winbind`, tests d'intégration avec un
contrôleur de test. À planifier seulement si le besoin entreprise est confirmé.

**Fichiers.** `src/shares.rs`, `src/handlers/shares.rs`,
`frontend/src/lib/screens/Shares.svelte`, `docs/guides/shares.md`.

**Acceptation.** Toute valeur non supportée est refusée **avant** d'écrire
`smb.conf` ; un test vérifie qu'aucun `security = ads` ne peut être généré.

---

### [x] P0-3 — SMB : `additional_params` et `max protocol` fantômes

**Livré 2026-11-05.** `additional_params` sont appliqués via une allowlist
(`SMB_EXTRA_KEYS`), `server max protocol = SMB3` est écrit avec le minimum,
et NT1/SMB1 sont refusés (UI limitée à SMB2/SMB3).

**Constat.** `additional_params` est validé puis seulement persisté dans
`/etc/homevault/smb.json` (`src/shares.rs:586,619`) : il n'est jamais écrit dans
`smb.conf`. Le protocole n'écrit que `server min protocol`
(`src/shares.rs:1422`) ; il n'y a pas de maximum, et `protocol_version` n'est
validé que comme jeton alphanumérique — `SMB1` est donc acceptable.

**Action.**
- Soit appliquer `additional_params` via une **liste blanche** de directives
  SMB sûres (jamais de section, jamais de `include`), avec test d'injection.
- Soit les retirer de l'API et de l'UI.
- Remplacer `protocol_version` par un choix fermé (`SMB2`, `SMB3`) qui écrit
  `server min protocol` **et** `server max protocol` ; refuser SMB1 sauf mode
  « legacy » explicite avec avertissement.

**Fichiers.** `src/shares.rs`, `src/handlers/shares.rs`, `Shares.svelte`,
`docs/guides/shares.md`.

**Acceptation.** Test : `additional_params = ["include = /etc/passwd"]` →
`400` ; `protocol_version = "SMB1"` → refus par défaut ; golden `smb.conf`
contient min **et** max.

---

### [x] P0-4 — Test de contrat « tout champ persisté est rendu »

**Livré 2026-11-05.** `smb_global_params`, `nfs_export_line` et
`ftp_config_text` déstructurent leurs structs **sans `..`** : ajouter un champ
sans le rendre ne compile plus. `allowed_users` (FTP) est désormais appliqué
via `/etc/vsftpd.user_list`.
Tests : `test_smb_global_params_renders_every_field`,
`test_nfs_export_line_applies_networks_and_share_overrides`,
`test_smb_config_deserializes_without_multichannel`.

**Constat.** Les cas P0-1 à P0-3 ont la même cause : un champ ajouté au modèle
et à l'API sans consommateur dans le générateur de configuration. Ce n'est pas
isolé (SMB, NFS, FTP partagent ce pattern).

**Action.** Ajouter un test de contrat par service : pour chaque champ du
`*Config`, soit il apparaît dans le fichier généré, soit il est explicitement
marqué `#[not_rendered(reason = "...")]` (ou une liste blanche équivalente).
Le test échoue si un champ est ajouté sans être traité.

**Fichiers.** `src/shares.rs` (tests), éventuellement un petit utilitaire
`src/config_contract.rs`.

**Acceptation.** Ajouter un champ factice au modèle fait échouer `cargo test` ;
la liste des champs « non rendus » est visible et justifiée.

---

### [x] P0-5 — USB Copy : la vérification doit vraiment vérifier · S/M

**Livré 2026-11-05.** `verify_copy` lit désormais chaque fichier et compare un
**SHA-256** source/destination (taille + hash), collecte **tous** les écarts et
échoue avec la liste. Le cas « source = fichier unique » est traité (le nom est
conservé sous la destination) et `count_files` compte un fichier comme 1.
Tests unitaires : arbre identique, même taille mais contenu différent, taille
différente, fichier manquant, source fichier unique, comptage. Docs :
`docs/guides/usb-copy.md` (l'étape 3 ne promet plus un simple hash).

**Constat.** `UsbCopyProfile.verify_checksums` existe (`src/usb_copy.rs:64`),
mais `verify_copy` ne comparait que la **taille** et l'**existence**
(`src/usb_copy.rs:509`). Un fichier corrompu de même taille était déclaré
« vérifié ».

**Action.**

1. Remplacer `verify_copy` par un parcours streaming : SHA-256 (ou BLAKE3)
   de la source et de la destination, comparaison taille + hash, collecte de
   `{path, expected, actual}` **sans s'arrêter au premier échec**.
2. Vérifier un fichier source unique comme un fichier, pas comme un dossier.
3. Remplacer le booléen par un mode `size | sha256 | none` (ou garder le
   booléen mais le renommer dans l'API/UI pour qu'il ne mente plus).
4. Ajouter les stratégies DSM manquantes si elles ne sont pas déjà couvertes :
   `incremental`, `mirror`, `versioned`, `photo-import`, avec politique de
   conflit `overwrite | skip | rename`. *(Reste ouvert : ce lot ne couvre que
   la vérification.)*
5. Journaliser le résultat par fichier et exposer les échecs dans
   `transfer_history`. *(Reste ouvert.)*

**Fichiers.** `src/usb_copy.rs`, `tests/usb_copy_integration.rs`,
`frontend/src/routes/UsbCopy.svelte`, `docs/guides/usb-copy.md`.

**Acceptation.** Un octet modifié côté destination ⇒ copie refusée, erreur
nommée. Un fichier manquant ⇒ refus. Un fichier identique ⇒ succès.

**Tests.** Unitaires (hash + collecte d'échecs), intégration (corruption réelle
d'un octet), E2E (le toggle « vérifier » envoie le bon mode).

---

### [x] P0-6 — Scrub planifié, repris après reboot et post-arrêt sale · M/L

**Livré 2026-11-05.** Le moteur de tâches planifiées générique gagne un type
**`scrub`** (`{pool, pool_type: zfs|btrfs|mdadm}`), exposé dans l'écran Tasks
(i18n ×4). `src/scrub.rs` centralise l'exécution : verrou mono-scrub, délai de
démarrage de 30 s, dérivation mdadm (`md0` → `/dev/md/md0`), `zpool scrub`,
`btrfs scrub start` ou `mdadm --action check`. Le marqueur
**`clean-shutdown`** et le handler **SIGTERM** sont en place : après un arrêt
sale, `scrub_after_unclean_shutdown` attend deux minutes puis scrube une fois
chaque pool monté et non dégradé, séquentiellement. Le `mdadm --action check`
couvre RAID1/5/6/10, pas seulement RAID5/6.

La reprise exacte utilise `state/scrub-in-progress` (`pool=…`, `kind=…`),
écrit seulement après un démarrage réussi. Au boot, `resume_interrupted`
classe le statut réel : un scrub encore `in progress`/`running` n'est pas
doublé, un scrub `finished`/`repaired` efface le marqueur, et `idle`/une
sortie inconnue relance le scrub enregistré. ZFS peut reprendre son propre
scan ; Btrfs et mdadm repartent du début. L'appel se fait dans
`src/storage/recovery.rs` **avant** le scrub global post-arrêt sale.
*Reste ouvert : le test d'intégration « SIGKILL puis reboot ».*

**Constat.** Le scrub n'existait que comme endpoint manuel
(`POST /storage/virtual-disks/{name}/scrub`, `src/pools.rs:566`,
`src/handlers/storage.rs:579`). Aucune planification, aucune reprise après
redémarrage, aucun déclenchement après un arrêt non propre.

**Action.**

1. ~~Table `scrub_schedules`~~ → réutilise la table `scheduled_tasks` et son
   moteur (`TaskAction::Scrub`), avec cron/timezone/next_run/historique déjà
   en place.
2. Boucle planificatrice : ~~`next_run` recalculé, reprise `running`, une
   opération à la fois~~ → le moteur générique le fait ; `src/scrub.rs` ajoute
   le verrou mono-scrub.
3. Marqueur d'arrêt propre : `data/state/clean-shutdown` écrit sur SIGTERM
   (fait), absent au boot ⇒ scrub des pools montés (fait).
4. mdadm : `--action check` sur RAID1/5/6/10 (fait).
5. UI : type `scrub` + pool + type de stockage dans Tasks (fait) ; la reprise
   exacte utilise le marqueur `scrub-in-progress` (fait).

**Fichiers.** `src/scrub.rs` (nouveau), `src/scheduled_tasks_scheduler.rs`,
`src/storage/recovery.rs`, `src/web/mod.rs`, `frontend/src/routes/Tasks.svelte`,
`src/i18n/*.toml`, `docs/guides/scheduled-tasks.md`.

**Acceptation.** Un scrub planifié s'exécute et laisse une trace dans
l'historique des tâches. Un `SIGKILL` suivi d'un redémarrage programme un
scrub automatique. Un seul scrub tourne à la fois. Un scrub interrompu est
repris au boot, sans doublon s'il tourne déjà.

**Tests.** Unitaires (`src/scrub.rs` : marqueur, décision de reprise ;
`scheduled_tasks_scheduler`) ; intégration création/refus d'une tâche scrub ;
E2E Tasks (à ajouter) ; « SIGKILL puis reboot » (à ajouter).

---

### [x] P0-7 — Récupération de montage et fsck au démarrage · L

**Livré 2026-11-05 (cœur).** `src/storage/recovery.rs` s'exécute avant les
services de partage : `mdadm --assemble --scan`, `btrfs device scan`, puis
chaque entrée fstab marquée HomeVault absente de `/proc/mounts` est rejouée
avec ses options exactes. Btrfs : `btrfs rescue super-recover -y` puis montage
`ro` ; ext4 : `dumpe2fs -h` → `e2fsck -p` (jamais `-y`) → montage normal →
`ro,noload` → `ro`. Le rapport est écrit dans
`data/state/storage-recovery.json` et journalisé. Les grants sudoers couvrent
`mdadm --assemble --scan`, `btrfs device/rescue`, `dumpe2fs -h`,
`e2fsck -p` et `mount /mnt/*`. Le marqueur `clean-shutdown` et le SIGTERM sont
traités dans le même lot.
*Options `x-systemd.*` livrées et retirées au replay ; test d'acceptation
`dm-flakey`/superbloc Btrfs livré en opt-in.*

**Constat.** HomeVault écrivait `defaults,nofail` (`src/disk.rs:2252`,
`src/pools.rs:436`) : le boot ne bloque pas, mais aucun `fsck`, aucune échelle
de superblocs, et un pool non monté n'était pas remonté à l'administrateur.

**Action.**

1. Nouveau module `src/storage/recovery.rs`, appelé avant l'ouverture des
   partages :
   - **mdadm** : `mdadm --assemble --scan`, puis `mdadm --detail` ;
     dégradé ⇒ notifier mais monter si possible.
   - **ZFS** : `zpool import -N`, `zpool status -x` ; jamais `-f` sans
     confirmation. *(Les pools ZFS s'importent via `zpool.cache` ; le pass
     fstab ne les couvre pas.)*
   - **Btrfs** : `btrfs device scan` ; si le montage échoue,
     `btrfs rescue super-recover -y`, puis `mount -o ro`, puis
     `mount -o ro,nologreplay` ; notifier le mode dégradé.
   - **ext4** : `dumpe2fs -h` → si « not cleanly unmounted », `e2fsck -p` ;
     si échec, `e2fsck -fy` **sur action admin explicite** ; essayer les
     superblocs de secours (`-b 32768`, `-b 98304`, …).
2. Ne jamais remonter en écriture un volume récupéré en lecture seule sans
   confirmation.
3. Ajouter `x-systemd.device-timeout=30s`, `x-systemd.mount-timeout=30s`,
   `_netdev` le cas échéant — fait pour les entrées HomeVault (RAID, pools,
   layout Btrfs) ; `_netdev` n'est pas ajouté aux pools locaux. Le replay de
   récupération retire les options `x-systemd.*` avant `mount(8)`.
4. Écrire `data/state/storage-recovery.json` (`pool`, `mode`, `errors`,
   `action`) et notifier — fait : une entrée en échec ou montée en `ro` lève
   `health.storage`.
5. Test avec `dm-flakey`/`dm-error` : superbloc Btrfs corrompu ⇒ montage en
   lecture seule + notification — fait via
   `tests/recovery_hardware_integration.rs` (opt-in `HV_RECOVERY_HW=1`) ; le
   test vérifie le statut `mounted-ro` et l'option `ro` du montage.

**Fichiers.** `src/storage/recovery.rs` (nouveau), `src/web/mod.rs`,
`src/setup.rs`, `src/storage/mod.rs`, `src/scrub.rs`.

**Acceptation.** Un volume Btrfs au superbloc primaire abîmé monte en `ro` et
le rapport le dit. Un ext4 non propre est fscké avant montage. Aucun `nofail`
silencieux.

**Tests.** Unitaires (parsers fstab, `/proc/mounts`, état ext4, échelle de
fallback, marqueurs de boot) ; acceptation matérielle opt-in
`tests/recovery_hardware_integration.rs` (loop Btrfs + superbloc corrompu →
`mounted-ro` + option `ro`).

---

### [x] P0-8 — Historique SMART, erreurs I/O et alertes · M

**Livré 2026-11-05 (cœur).** Migration `082_disk_health.sql` :
`disk_health_samples` + `disk_health_findings` (empreinte unique
`serial:attr:condition`). `src/disk_health.rs` échantillonne les attributs
5/9/177/187/188/190/194/197/198/199/231, applique les seuils (5 +5,
187/197/198 en hausse, usure ≤10 %, température ≥55 °C, `-H` FAILED), collecte
les erreurs I/O (`journalctl` 24 h, `btrfs device stats`, `zpool status -x`,
`mdadm --detail` mismatch) et notifie une seule fois via
`health.disk_smart`. `src/disk_health_scheduler.rs` échantillonne au plus une
fois par 20 h. Endpoints session/admin + carte **Disk health history** dans
Health (liste des findings + **Sample now** admin), i18n ×4.
*Restent ouverts : seuil température sur 3 échantillons (aujourd'hui un seul),
et rapport de santé hebdomadaire agrégé.*

**Constat.** `check_disk_smart` ne lisait que `smartctl -H`
(`src/health.rs:518`). Les attributs 5/187/188/197/198 étaient extraits pour la
liste des disques (`src/disk.rs:1259`) mais n'étaient ni historisés ni alertés.

**Action.**

1. Table `disk_health_samples(serial, model, attr_id, raw, normalized,
   sampled_at)` — fait.
2. Échantillonnage quotidien + au démarrage : attributs 5, 187, 188, 197, 198,
   199, 177, 231, température, heures, état `-H` — fait.
3. Seuils :
   - `-H` passe à `FAILED` ⇒ critique immédiate — fait ;
   - 197/198/187 augmentent de ≥ 1 ⇒ alerte — fait ;
   - 5 augmente de ≥ 5 ⇒ alerte — fait ;
   - température > 55 °C sur 3 échantillons ⇒ alerte — fait
     (`analyze_temperature`, seuil sur 3 échantillons consécutifs) ;
   - usure SSD < 10 % ⇒ alerte — fait.
4. Veille I/O : `journalctl -k`, `btrfs device stats`, `zpool status -x`,
   `mdadm --detail` (`Mismatch Count`) — fait.
5. Rapport hebdomadaire agrégé — livré par P2-12 (`src/health_report.rs`).

**Fichiers.** `src/disk_health.rs` (nouveau),
`src/disk_health_scheduler.rs` (nouveau),
`src/storage/disk_health_repo.rs` (nouveau),
`src/handlers/disk_health.rs` (nouveau), `sqlx/migrations/082_disk_health.sql`,
`src/web/mod.rs`, `src/setup.rs`, `frontend/src/routes/Health.svelte`,
`frontend/src/lib/services/api.ts`, `src/i18n/*.toml`,
`docs/guides/health.md`.

**Acceptation.** Une augmentation simulée de 197 déclenche une alerte une
seule fois. Un `I/O error` noyau apparaît dans Health.

**Tests.** Unitaires (`src/disk_health.rs`, 8 : parsing SMART, santé,
seuils 5/197, usure/température, I/O, btrfs, zpool/mdadm) ;
`src/disk_health_scheduler.rs` (2 : timestamp absent/invalide, fenêtre 20 h) ;
intégration `tests/health_integration.rs` (+3 : findings auth + vide, samples
serial/limite, sample admin-only) ; E2E carte Health (à ajouter).

---

### [x] P0-9 — Écritures de configuration atomiques, validées et auto‑réparées · S/M

**Livré 2026-11-05.** `AtomicWriter` ne supprime plus la cible : fichier
temporaire unique dans le même répertoire, `fsync`, `rename` atomique,
`fsync` du répertoire parent, mode préservé ; un échec laisse l'ancien fichier
intact. `write_config_file` écrit atomiquement (le fallback `sudo tee` ne sert
plus qu'aux fichiers root quand le processus n'est pas root) et conserve un
`.bak`. `append_config_file` fait un read-modify-write atomique sous le même
verrou. `apply_smb_config` valide le candidat avec `testparm -s` **avant** de
remplacer le fichier, et `self_heal_smb_config` restaure un `smb.conf`
vide/invalide depuis `.bak` ou les défauts au démarrage. Les grants sudoers
couvrent `smb.conf.bak`, `vsftpd.conf.bak` et `exports.bak`.
Tests : `src/storage/atomic.rs` (remplacement, mode préservé, binaire,
noms temporaires uniques) + `src/shares.rs` (backup, append, self-heal,
fichier valide intact).

**Constat.** `AtomicWriter` faisait `remove_file` **puis** `rename` : entre les
deux, le fichier n'existait plus. `write_config_file` écrivait `smb.conf` en
direct, `append_config_file` ajoutait sans atomicité, et aucun `testparm` ne
validait avant `systemctl reload smbd`.

**Action.**

1. Corriger `AtomicWriter` :
   - ne pas supprimer la cible : `rename(tmp, target)` remplace atomiquement
     sur POSIX ;
   - `fsync` du fichier **et** du répertoire parent ;
   - nom temporaire unique (`.tmp.<pid>.<random>`) ;
   - préserver le mode ;
   - en cas d'échec, laisser l'ancien fichier intact.
2. Conserver `smb.conf.bak`/`exports.bak` avant remplacement.
3. Valider avant de recharger : SMB `testparm -s` (fait) ; exports
   `exportfs -ra` (déjà en place).
4. Au démarrage : `smb.conf` vide/invalide ⇒ restaurer le `.bak` (ou le défaut),
   journaliser.
5. Test : coupure entre l'écriture du temporaire et le `rename` ; l'ancien
   fichier doit être intact et le service démarrer.

**Fichiers.** `src/storage/atomic.rs`, `src/shares.rs`, `src/setup.rs`, tests.

**Acceptation.** Aucune configuration système n'est tronquée par une coupure.
Un `smb.conf` corrompu est réparé au boot. `testparm` refuse une config
invalide avant reload.

**Tests.** Unitaires (`AtomicWriter`), unitaires `shares` (backup/append/
self-heal), intégration (kill pendant écriture — à ajouter), E2E (self-heal au
démarrage — à ajouter).

---

### [x] P0-10 — `restic check` planifié avec lecture de données · M

**Livré 2026-11-05 (via le moteur de tâches).** Plutôt qu'une seconde
planification dans `restic_scheduler`, le moteur générique gagne
**`TaskAction::ResticCheck { repo_id, subset }`** : `restic check
--read-data-subset=<subset>` (défaut `10%`), validation stricte du subset
(`10%`, `1/4`, `1G`), chargement du dépôt par id et exécution hors runtime.
L'historique de la tâche (statut, sortie, date) sert de « dernier contrôle »,
et l'échec est visible dans Tasks. `ResticService::check_repo_with_subset` a
été ajouté ; `check_repo` reste le chemin `--read-data` complet pour le
bouton manuel. UI Tasks : type **Restic repository check** (repo id + subset),
i18n ×4 ; `docs/guides/scheduled-tasks.md` documente le payload.
*Restent ouverts : les 3 tentatives avec backoff et la colonne
`last_check_ok` dédiée dans l'écran Backup (l'historique de tâche couvre le
besoin immédiat).*

**Constat.** `restic check` existait à la demande (`src/restic.rs:1661`) mais le
planificateur ne faisait que backup + retention (`src/restic_scheduler.rs`).
`check` ne lisait pas les packs (`--read-data` absent). Le contrôle
`backup_target_scheduler` couvre les dépôts **reçus**, pas les dépôts
**locaux**.

**Action.**

1. ~~Colonnes `check_schedule`/`last_check_*`~~ → une tâche `restic_check`
   planifiée dans `scheduled_tasks`, avec historique et `next_run`.
2. Planification : le cron de la tâche ; subset `10%` par défaut, `100%` ou
   `1/4`/`1G` possibles.
3. Un seul contrôle à la fois : `TaskRunGuard` par tâche ; un contrôle manuel
   et une tâche planifiée peuvent toutefois se chevaucher (à verrouiller si
   nécessaire).
4. Reprise/backoff : fait (3 tentatives, 5 s puis 30 s) ; un échec est
   enregistré, notifié via `backup.jobs` et la tâche replanifie au tick
   suivant. Les constats du dépôt ne sont pas réessayés.
5. Notification : l'échec est visible dans l'historique de tâche et lève
   désormais `backup.jobs` avec le nom de la tâche et l'erreur.
6. UI : type + champs repo/subset dans Tasks ; l'écran Backup garde son
   bouton « Check now » manuel.

**Fichiers.** `src/restic.rs`, `src/scheduled_tasks_scheduler.rs`,
`frontend/src/routes/Tasks.svelte`, `frontend/src/lib/services/api.ts`,
`src/i18n/*.toml`, `docs/guides/scheduled-tasks.md`, `SCREENS.md`.

**Acceptation.** Un dépôt avec un pack tronqué échoue au contrôle. Un dépôt
sain passe. Un contrôle interrompu laisse la tâche en échec et replanifie.

**Tests.** Unitaires (`scheduled_tasks_scheduler` : payload repo/subset,
injection `--force` refusée, `10%`/`1/4`/`1G` acceptés) ; intégration
création/refus d'une tâche `restic_check` ; E2E Tasks (à ajouter) ; pack
tronqué réel (à ajouter).

---

### [x] P0-11 — SMB : durable handles, chiffrement, `strict sync`, AIO · M

**Livré 2026-11-05.** `SmbConfig` porte désormais `durable_handles`,
`smb_encrypt` (`off|desired|required`), `server_signing`
(`off|desired|mandatory`), `strict_sync`, `aio_read_size`, `aio_write_size`,
`use_sendfile`, `oplocks`, `kernel_oplocks`, `level2_oplocks` — tous rendus
exhaustivement par `smb_global_params`, validés et normalisés. L'onglet SMB
avancé expose chaque réglage avec son aide i18n ×4. Le test de contrat
`test_smb_global_params_renders_every_field` couvre les nouveaux champs, et
`test_smb_config_deserializes_without_multichannel` vérifie les défauts des
anciens `smb.json`. `docs/guides/shares.md` documente les compromis.
*Réserve : `npm run check` n'a pas été exécuté dans ce worktree (dépendances
frontend absentes) ; `cargo check --lib` et les tests Rust compilent.*

**Constat.** `SmbConfig` n'exposait que `multichannel`. `durable handles`
n'était même pas dans la liste blanche `SMB_EXTRA_KEYS`. Le chiffrement, la
signature et `strict sync` n'étaient pas des réglages first-class.

**Action.**

1. Ajouter à `SmbConfig` :
   - `durable_handles: bool` ⇒ `durable handles = yes` ;
   - `smb_encrypt: off | desired | required` ⇒ `server smb encrypt = …` ;
   - `server_signing: off | desired | mandatory` ⇒ `server signing = …` ;
   - `strict_sync: bool` ⇒ `strict sync = yes` ;
   - `aio_read_size` / `aio_write_size` (0 = auto) ;
   - `use_sendfile: bool` ;
   - `oplocks`, `kernel_oplocks`, `level2_oplocks`.
2. Rendu exhaustif dans `smb_global_params` (le destructuring sans `..` est
   déjà le garde-fou).
3. Documenter les compromis : `durable handles = yes` désactive le verrouillage
   inter-protocoles ; le chiffrement SMB3 coûte du CPU ; `strict sync` protège
   les bases mais réduit le débit.
4. Valider avec `testparm -s` avant `systemctl reload smbd`.
5. Mettre à jour le test de contrat P0-4 pour chaque nouveau champ.

**Fichiers.** `src/shares.rs`, `frontend/src/routes/Shares.svelte`,
`src/i18n/*.toml`, `docs/guides/shares.md`, tests.

**Acceptation.** `testparm` accepte chaque combinaison. Un client qui perd la
connexion retrouve le handle (test manuel documenté). Aucun champ persisté
n'est ignoré.

**Tests.** Unitaires (rendu + contrat + normalisation), intégration
(`testparm` sur fichier généré), E2E (toggles).

---

### [x] P0-12 — iSCSI : FUA/Sync Cache, write cache, UNMAP, volume plein · M

**Livré 2026-11-05.** Migration `083_iscsi_write_semantics.sql` ajoute
`write_policy` (`buffered|direct`), `emulate_fua` et `unmap` aux backstores.
`src/iscsi.rs` rend trois commandes `targetcli … set attribute`
(`emulate_write_cache`, `emulate_fua_write`, `emulate_tpu`) appliquées après
la création du backstore et avant toute reconstruction de target ; une erreur
stoppe l'apply. Le handler valide la politique, l'API/DB la conservent, et le
formulaire iSCSI expose **I/O policy**, **Honour FUA** et **Advertise UNMAP**
(i18n ×4). `docs/guides/iscsi.md` et `SCREENS.md` documentent le compromis
`buffered`/`direct`.

La protection volume plein est livrée dans `src/iscsi_guard.rs` : passe au
démarrage (3 min) puis toutes les 5 min, mesure de l'espace libre
(`free_bytes_at`), et passage en lecture seule via
`IscsiRepository::set_backstore_readonly` quand un backstore **fileio
inscriptible** descend sous `VOLUME_FULL_RESERVE_BYTES` — ou quand l'espace
libre est illisible. Une notification est levée ; le passage est réversible.
Un backstore `block` ou déjà en lecture seule est ignoré.

**Constat.** `src/iscsi.rs` gérait les backstores fileio/block, les portails,
les ACL et CHAP, mais n'exposait ni `emulate_write_cache`, ni
`emulate_fua_write`, ni `write_cache`, ni `unmap`, ni la protection du LUN
quand le volume est plein.

**Action.**

1. Exposer les attributs LIO par backstore :
   - `emulate_write_cache` (1 = cache annoncé) — fait ;
   - `emulate_fua_write` (1 = FUA honoré) — fait ;
   - `write_cache` (`write through` vs `write back`) — fait via
     `write_policy` ;
   - `emulate_tpu`/`unmap` pour la récupération d'espace — fait.
2. Politique volume plein : passer le LUN en lecture seule sous un seuil
   (ex. 1 Gio restant) et notifier — fait (`src/iscsi_guard.rs`).
3. Documenter que `write back` sans UPS augmente le risque de perte — fait.
4. Ne pas exposer ces réglages sans test `configfs` réel ; capacité absente ⇒
   `503` nommé — le chemin reste `targetcli`, avec les mêmes gardes.

**Fichiers.** `src/iscsi.rs`, `src/storage/iscsi_repo.rs`,
`src/handlers/iscsi.rs`, `sqlx/migrations/083_iscsi_write_semantics.sql`,
`frontend/src/routes/Iscsi.svelte`, `src/i18n/*.toml`,
`docs/guides/iscsi.md`, `SCREENS.md`, tests.

**Acceptation.** Un backstore en `direct` rend `emulate_write_cache=0` ;
`emulate_fua_write` et `emulate_tpu` suivent les toggles. Le LUN passe en
lecture seule sous le seuil et une notification est levée.

**Tests.** Unitaires (`src/iscsi.rs` : attributs, seuil, `free_bytes_at` ;
`src/iscsi_guard.rs` : décision), intégration (round-trip des attributs),
E2E (toggles). Le passage `configfs` réel reste à valider sur un hôte équipé.

**Tests.** Unitaires (`src/iscsi.rs` : attributs buffered/direct,
FUA/UNMAP, politique fermée) ; intégration `tests/iscsi_integration.rs`
(aller-retour des champs, politique inconnue refusée) ; E2E iSCSI (à ajouter) ;
test `configfs` réel (à ajouter).

---

## 3. P1 — Quick wins (S → M)

### [x] P1-1 — SMB multichannel · S

**Livré 2026-11-05.** Champ `multichannel` (`#[serde(default)]`) rendu en
`server multi channel support = yes|no`, toggle dans l'onglet SMB avancé,
libellé + aide i18n (4 langues). Off par défaut (un seul lien = inutile).

- [x] `server multi channel support` global piloté par un toggle.
- [x] Rendu testé (`test_smb_global_params_renders_every_field`) et ancien
  `smb.json` sans le champ toujours lisible.
- [x] `docs/guides/shares.md` + `SCREENS.md` mis à jour.
- Fichiers : `src/shares.rs`, `Shares.svelte`, `src/i18n/*.toml`,
  `docs/guides/shares.md`.

### [x] P1-2 — Expiration de mot de passe · S

**Livré 2026-11-05.** Politique globale `password_policy.expiry_days`
(migration `079`, `0` = jamais), `users.password_changed_at` remis à zéro par
toute écriture de mot de passe, et `enforce_password_expiry` au login : le
compte expiré est marqué `must_change_password`, que le garde existant limite
au flux de changement. Endpoints admin `GET/PUT /api/settings/password-policy`
(0..3650, `400` hors bornes), carte Settings → Security, i18n ×4.
Tests : 4 unit (`is_expired`, bornes) + `tests/password_policy_integration.rs`
(3 : matrice admin, expiration + remise à zéro, `0` = jamais) + E2E 12.7.
Fichiers : `sqlx/migrations/079_password_policy.sql`,
`src/storage/password_policy_repo.rs`, `src/auth/service.rs`,
`src/handlers/password_policy.rs`, `PasswordPolicyCard.svelte`.

### [x] P1-3 — Rétention des métriques configurable · S

**Livré 2026-11-05.** Table `metrics_settings.retention_days` (migration
`080`, défaut 8), lue à chaque tick du collecteur (changement sans restart),
bornée `1..=365` par `retention_secs`. Endpoints `GET /api/metrics/settings`
(auth) et `PUT` (admin), sélecteur 8/30/90 dans l'en-tête Metrics (une valeur
hors liste reste affichée), i18n ×4.
Tests : `retention_seconds_are_clamped` + `tests/metrics_integration.rs`
(matrice auth, aller-retour, `0`/`366` refusés) + E2E 2.2.
Fichiers : `sqlx/migrations/080_metrics_retention.sql`, `src/metrics.rs`,
`src/handlers/metrics.rs`, `Metrics.svelte`.

### [x] P1-4 — OpenAPI 3 généré · S/M

**Livré 2026-11-05.** `GET /api/openapi.json` (`require_admin`) sert un
document OpenAPI **3.0.3** construit par `src/openapi.rs` **depuis la table de
routes elle‑même** : `src/web/mod.rs` et les sous‑routeurs
`/api/network`, `/api/vpn`, `/api/rescue`, `/api/terminal`, `/api/vm`. Aucune
seconde table à maintenir : une route ajoutée à `create_app` apparaît au build
suivant. Le document déclare les schémas `X-Session-Token` / `X-CSRF-Token`, le
paramètre `X-HomeVault-Contract`, les réponses `400/401/403/426/500/501/503`,
`security: []` sur les flux d'authentification et `x-homevault-auth` par
opération. Les transports WebSocket/SSE/ticket/binaire sont exclus de `paths`
et nommés dans `x-homevault-excluded` (une fuite de route ticket/terminal fait
échouer les tests unitaires). Build courant : **553 chemins / 696 opérations**,
30 transports exclus.
Tests : 10 unitaires (`src/openapi.rs`) + 2 d'intégration
(`tests/openapi_integration.rs`) + serialisation stable. Docs : `docs/API.md`
(« OpenAPI 3 »), `docs/ARCHITECTURE.md`, `docs/BACKEND.md`, `docs/INDEX.md`,
`TESTING.md`.
Fichiers : `src/openapi.rs`, `src/handlers/openapi.rs`, `src/web/mod.rs`,
`tests/openapi_integration.rs`.

- [x] Partir de `src/api_contract.rs` + la table de routes (`src/web/mod.rs`).
- [x] Générer `/api/openapi.json` (authentifié admin) et l'inclure dans `docs/API.md`.
- [x] Décrire l'auth (`X-Session-Token`, `X-CSRF-Token`) et les statuts
  `400/401/403/501/503` (+ `426`/`500`).
- [x] Tests : snapshot JSON stable, exclusion des routes sensibles.

### [x] P1-5 — DDNS : trois fournisseurs de plus · S

**Livré 2026-11-05.** `DdnsProvider` gagne `ovh`, `namecheap` et `desec` :

- **OVH DynHost** : `GET https://www.ovh.com/nic/update` en HTTP Basic
  (`username` + mot de passe DynHost dans le `token` write‑only), succès
  `good`/`nochg`.
- **Namecheap** : `GET dynamicdns.park-your-domain.com/update` avec hôte dérivé
  de `hostname` et `domain` (domaine enregistré, `@` pour l'apex), succès
  `<ErrCount>0</ErrCount>` — un `200` XML d'erreur est refusé.
- **deSEC** : `GET update.dedyn.io/nic/update` avec
  `Authorization: Token …`, succès `good`/`nochg`.

`HttpRequest` porte désormais une authorization générique et des credentials
Basic : aucun nouveau secret ne transite dans une URL. La mise à jour reste
« change‑only » (worker 5 min + refresh quotidien), le timeout inchangé, et le
token reste masqué (`"set"`) dans l'API. Champs `username` (OVH) et `domain`
(Namecheap, non secrets) ajoutés au payload et à la carte Settings.
Tests : **24 unitaires** (`src/ddns.rs`, +7), **8 d'intégration**
(`tests/ddns_integration.rs`, +1), **8 E2E** (`settings/ddns.spec.ts`, +1).
Docs : `docs/guides/ddns.md`, `docs/API.md`, `docs/INDEX.md`, `SCREENS.md`,
`FeatureDoc` `ddns`.
Fichiers : `src/ddns.rs`, `src/outbound.rs`, `src/handlers/ddns.rs`,
`DdnsCard.svelte`, `frontend/src/lib/services/api.ts`, i18n ×4,
`tests/ddns_integration.rs`, `tests/e2e/settings/ddns.spec.ts`.

- [x] Ajouter OVH, Namecheap, deSEC sur le modèle de `src/ddns.rs:69`.
- [x] Même contrat : mise à jour « change‑only », timeout, secret jamais renvoyé.
- [x] Tests : construction de requête (pas d'appel réseau), erreurs 4xx/5xx.

### [x] P1-6 — Incus : clone · S

**Livré 2026-11-05.** `POST /api/vm/{name}/clone` accepte
`{ name, snapshot?, profiles? }`. Le handler valide le nouveau nom, le snapshot
source, la collision de nom et l'existence de la source avant tout appel Incus,
puis envoie `source.type = "copy"` (`/1.0/instances/<source>` +
`source_snapshot`) : Incus copie config et devices, le clone reste **arrêté**.
L'opération est attendue (`CLONE_WAIT_SECS = 300`) ; un dépassement répond
`status:"cloning"` + l'identifiant d'opération, jamais une fausse erreur. Le
tiroir de détail gagne une action **Cloner** (nouveau nom + état source
optionnel).
Au passage, `incus_err(NotAvailable)` répond désormais **`503`** nommé au lieu
de `400` sur tous les endpoints VM (contrat de capacité).
Tests : **1 unitaire** `src/incus.rs` (sérialisation copy), **1 unitaire**
`src/handlers/vm.rs` (`clone_body`), **2 d'intégration** (`vm_integration.rs`,
matrice + validation + `503`), **2 E2E** (`vm.spec.ts` 4.7/4.8).
Docs : `docs/guides/vm.md`, `docs/API.md`, `SCREENS.md`, `FeatureDoc`
`virtual-machines`, i18n ×4.
Fichiers : `src/incus.rs`, `src/handlers/vm.rs`, `src/web/mod.rs`,
`VirtualMachines.svelte`, `src/i18n/*.toml`, `tests/vm_integration.rs`,
`tests/e2e/vm/vm.spec.ts`.

- [x] `POST /api/vm/{name}/clone` (nom, snapshot source, profils), attente de
  l'opération Incus, rafraîchissement du cache.
- [x] Tests : intégration mock Incus, E2E si Incus présent → sinon `503`.

### [x] P1-7 — Routes statiques · S/M

**Livré 2026-11-05.** `src/static_routes.rs` gère des routes IPv4/IPv6 vers un
autre réseau :

- `GET /api/network/routes` (auth) → `{ available, routes[] }` avec l’état
  `active` de chaque route ;
- `POST /api/network/routes` (admin) `{ destination, via, metric?, interface }`
  → validation (CIDR non‑zéro, next‑hop unicast de même famille, nom
  d’interface ≤ 15 caractères, métrique 0–65535), `sudo ip route replace`,
  relecture du noyau, puis sauvegarde JSON ;
- `DELETE /api/network/routes` (admin) `{ destination, interface }`.

Rollback : une commande refusée est atomique ; un échec de relecture ou de
sauvegarde réapplique l’ancienne route (ou supprime la nouvelle). Une seule
route par destination, comme `ip route replace`. Persistance
`data/static_routes.json`, réappliquée au boot après 20 s, sans effacer les
entrées si une interface n’est pas encore prête. `ip` absent → `503` nommé ;
requête invalide → `400`. Carte **Routes** dans l’onglet Ethernet.
Tests : **6 unitaires** (`src/static_routes.rs`), **1 d’intégration**
(`network_integration.rs`), **2 E2E** (`network.spec.ts` §15).
Docs : `docs/guides/network.md` (« Static routes »), `docs/API.md`,
`SCREENS.md`, `FeatureDoc` `network`, i18n ×4.
Fichiers : `src/static_routes.rs`, `src/handlers/network.rs`, `src/web/mod.rs`,
`Network.svelte`, `frontend/src/lib/services/api.ts`, `src/i18n/*.toml`,
`tests/network_integration.rs`, `tests/e2e/network/network.spec.ts`.

- [x] `GET/POST/DELETE /api/network/routes` (destination CIDR, via, métrique,
  interface), validation, application via `ip route replace`, rollback.
- [x] Tests d'intégration avec réseau simulé ; E2E carte « Routes ».

### [x] P1-8 — Proxy client sortant · S/M

**Livré 2026-11-05.** `src/proxy.rs` porte un réglage unique
(`data/proxy.json` : `enabled`, `http_proxy`, `https_proxy`, `no_proxy`) exposé
par `GET/PUT /api/settings/proxy` (admin) et appliqué aux appels sortants
concernés : ACME (`outbound::ReqwestTransport`), DDNS, vérification de version,
digests d'images du catalogue, webhooks/SMS et Web Push. La boucle locale,
le link-local, les plages privées RFC 1918, l'ULA/link-local IPv6 et les noms
`.local` sont **toujours** exemptés, quel que soit le contenu du `no_proxy`
saisi. Seuls `http://`/`https://` sont acceptés :
un `socks*://` est refusé avec la raison (build reqwest sans feature SOCKS), et
une URL porteuse d'identifiants, de query ou de fragment est refusée pour
qu'aucun secret n'entre dans un fichier ou un journal. Réglage désactivé →
`no_proxy()` explicite sur le client, donc `HTTP_PROXY`/`HTTPS_PROXY` de
l'environnement ne peuvent pas détourner le trafic. Carte **Proxy sortant**
dans Settings → Security, i18n ×4.
Tests : **10 unitaires** (`src/proxy.rs`) + **2 unitaires** (`handlers/proxy.rs`)
+ **8 d'intégration** (`tests/proxy_integration.rs`, dont un faux proxy TCP qui
reçoit la requête en URL absolue et un test de contournement loopback) + **9
E2E** (`settings/proxy.spec.ts`). Docs : `docs/guides/settings.md`,
`docs/API.md`, `docs/INDEX.md`, `SCREENS.md`, `TESTING.md`, `FeatureDoc`
`settings`.
Fichiers : `src/proxy.rs`, `src/handlers/proxy.rs`, `src/outbound.rs`,
`src/notification_engine.rs`, `src/web_push/mod.rs`,
`src/app_catalog/updates.rs`, `src/handlers/settings.rs`,
`src/settings_persistence.rs`, `src/web/mod.rs`, `ProxyCard.svelte`,
`frontend/src/lib/services/api.ts`, `src/i18n/*.toml`,
`tests/proxy_integration.rs`, `tests/e2e/settings/proxy.spec.ts`.

- [x] `http_proxy`/`https_proxy`/`no_proxy` appliqués aux appels sortants.
- [x] Trafic local/loopback jamais proxyfié.
- [x] Test : l'URL de test passe par le proxy configuré (mock).

### [x] P1-9 — ZFS `raidz3` + hot spare natif · S/M

**Livré 2026-11-05.** `raidz3` est un niveau **ZFS-only** (4 disques minimum,
triple parité) mappé sur le vdev `raidz3` dans `zfs_create_args`. Les `spares`
sont désormais acceptés pour ZFS : un seul groupe `spare` est ajouté après le
vdev (`zpool create tank raidz2 d1..d4 spare s1 s2`), y compris après les
mirroirs de raid10 ; zpool affecte automatiquement un spare quand un membre
tombe. Btrfs n'a pas de rôle hot spare : le refus nomme l'alternative (ajouter
le disque plus tard, ou le garder comme disque/pool de secours) au lieu de
l'ignorer. ZFS conserve le refus des créations avec membres manquants. La
validation refuse `raidz3` hors ZFS, les spares sur raid0, et les spares qui ne
comptent pas dans le minimum de disques. L'assistant propose 6 types ; choisir
**RAID Z3** force ZFS et désactive les autres systèmes de fichiers avec la
raison affichée ; le toggle « hot spares » apparaît pour mdadm **et** ZFS (pas
Btrfs). i18n ×4, docs `raid.md`, `API.md`, `SCREENS.md`, `FeatureDoc` `raid`.
Tests : **4 unitaires** (`src/disk.rs` : mapping/argv raidz3 + groupe spare,
raidz3 ZFS-only et minimum 4, spares raid0, acceptation ZFS/refus Btrfs),
**3 d'intégration** (`tests/raid_integration.rs` : refus raidz3 non-ZFS sans
job, ZFS+spare passe le shape gate, refus Btrfs avec alternative), **E2E**
(`tests/e2e/raid/raid.spec.ts` : 2.1 = 6 niveaux dont RAID Z3, 14.4 =
raidz3+ext4 refusé).
Fichiers : `src/disk.rs`, `src/handlers/storage.rs`, `Raid.svelte`,
`frontend/src/lib/services/api.ts`, `src/i18n/*.toml`,
`tests/raid_integration.rs`, `tests/e2e/raid/raid.spec.ts`, `docs/guides/raid.md`,
`docs/API.md`, `SCREENS.md`, `TESTING.md`, `src/rag_indexers.rs`.

- [x] `raidz3` (option « 3 tolérances ») mappé sur le vdev ZFS `raidz3`.
- [x] `spare` natif ZFS appliqué ; refus Btrfs documenté avec une alternative.
- [x] Tests : mapping + argv exact.

### [x] P1-10 — `restic mount` · M

**Livré 2026-11-05.** Un dépôt peut être monté **en lecture seule** pour le
parcourir dans l'Explorer :

- `POST /api/restic/repos/{id}/mount` (admin) lance `restic mount` en
  arrière-plan. Le point de montage est **dérivé** de l'id du dépôt sous
  `{DATA_DIRECTORY}/restic-mounts/<slug>` : jamais fourni par le client, slug
  déterministe sans séparateur ni `..`, et un contrôle structurel refuse tout
  point hors du répertoire. L'appel est idempotent.
- `GET /api/restic/mounts` (auth) liste les montages que le noyau confirme ;
  `DELETE /api/restic/mounts/{id}` (admin) démonte et arrête le processus FUSE.
- **`/proc/mounts` est l'autorité** : le service relit l'entrée du noyau et
  exige `ro,nosuid,nodev` (le vrai `restic mount` les porte). Un montage qui
  n'a pas ces options est démonté et refusé, et `GET /restic/mounts` ne
  l'annonce jamais.
- Délais nommés `RESTIC_FUSE_MOUNT_TIMEOUT` (30 s), `RESTIC_FUSE_UNMOUNT_TIMEOUT`
  (20 s) et `RESTIC_FUSE_POLL` ; démontage via `fusermount3`/`fusermount` avec
  repli `umount`. Un `restic mount` qui sort seul est moissonné par un watcher,
  et une entrée dont le montage noyau a disparu est retirée du registre.
- Capacité absente (`restic` ou `/dev/fuse`) → **`503` nommé**
  (`json_unavailable`) ; la carte remplace l'action par une note explicite.
  `GET /restic/available` expose `fuse_available`.
- UI : action **Parcourir le dépôt** dans le détail du dépôt, chemin monté +
  **Démonter**, et note « FUSE indisponible » sinon. i18n ×4.

Tests : **6 unitaires** (`src/restic.rs` : point de montage confiné, slug
déterministe sans chemin, argv exact, parsing `/proc/mounts` avec la ligne
restic réelle, seuls les montages read-only sous la racine sont annoncés,
id de démontage avec `..`/`/` refusé), **2 d'intégration**
(`tests/restic_integration.rs` : montage d'un dépôt inconnu = 400/503 jamais
500, liste = tableau et ids dangereux = 400), **2 E2E**
(`tests/e2e/backup/backup.spec.ts` : 2.3 action Parcourir ou note FUSE, 3.7
montage inconnu + id de démontage invalide).
Docs : `docs/guides/backup-restic.md`, `docs/API.md`, `SCREENS.md`,
`TESTING.md`, `FeatureDoc` `backup`.
Fichiers : `src/restic.rs`, `src/handlers/restic.rs`, `src/web/mod.rs`,
`Restic.svelte`, `frontend/src/lib/services/api.ts`, `src/i18n/*.toml`,
`tests/restic_integration.rs`, `tests/e2e/backup/backup.spec.ts`.

- [x] `restic mount` en lecture seule, point dérivé, `ro,nosuid,nodev`.
- [x] Délais bornés, démontage garanti, `/proc/mounts` comme autorité.
- [x] Tests argv/confinement ; `503` nommé si FUSE absent.

### [x] P1-11 — Migration de niveau RAID · M

**Livré 2026-11-05.** Un tableau mdadm peut changer de niveau sans être
recréé :

- `GET /api/storage/virtual-disks/{name}/level-options` (auth) lit
  `mdadm --detail` (niveau + nombre de membres réels) et renvoie chaque cible
  avec `allowed` et, sinon, la `reason` affichée par l'UI.
- `POST /api/storage/virtual-disks/{name}/migrate-level` (admin)
  `{ target_level, backup_confirmed }` exécute
  `sudo mdadm --grow <dev> --level <n> --backup-file <hors tableau>`. La
  matrice ne reprend que les conversions documentées par mdadm GROW MODE dont
  les deux extrémités existent ici : **raid0→raid5**, **raid0→raid10**,
  **raid1→raid5**, **raid5→raid6**, **raid6→raid5**. Tout le reste est refusé
  avec la raison (raid0→raid1 change de disposition, raid0/raid1→raid6 passe
  par raid5, raid10 hors 0→10).
- Le minimum de membres de la cible est exigé (« ajoutez un disque d'abord ») :
  raid1 de 2 disques → raid5 est refusé tant qu'un 3e disque n'existe pas.
  Un reshape déjà en cours bloque toute nouvelle conversion.
- `backup_confirmed` est obligatoire : le fichier `--backup-file` est dérivé
  sous `{DATA_DIRECTORY}/raid-backups/` avec un slug sans séparateur ni `..`
  (confinement testé), le répertoire est créé, et un fichier partiel est
  supprimé si mdadm refuse. `mdadm` absent → **`503`** nommé.
- UI Storage → Virtual disks : bouton **Changer le niveau RAID**, sélecteur
  limité aux cibles autorisées, bloc « pourquoi les autres niveaux ne sont pas
  proposés » avec chaque raison, case « J'ai sauvegardé les données »
  obligatoire avant activation ; i18n ×4. Le reshape continue en arrière-plan
  et « Vérifier l'état » le suit.
- Tests : **6 unitaires** (`src/disk.rs` : matrice, refus motivés,
  minima/raid10 pair, options complètes, confinement du chemin de sauvegarde,
  parsing `mdadm --detail` + reshape), **1 d'intégration**
  (`tests/raid_integration.rs` : `401` sur le plan, cible inconnue et absence
  de sauvegarde refusées avant toute commande, plan d'un tableau absent =
  `400` jamais `500`), **2 E2E** (`tests/e2e/storage/storage.spec.ts` :
  14.6 confirmation de sauvegarde obligatoire + payload exact, 14.7 aucune
  conversion possible avec raisons affichées).
- Docs : `docs/guides/raid.md`, `docs/guides/storage.md`, `docs/API.md`,
  `SCREENS.md`, `TESTING.md`, `FeatureDoc` `raid`.
- Fichiers : `src/disk.rs`, `src/handlers/storage.rs`, `src/web/mod.rs`,
  `Storage.svelte`, `frontend/src/lib/services/api.ts`, `src/i18n/*.toml`,
  `tests/raid_integration.rs`, `tests/e2e/storage/storage.spec.ts`.

### [x] P1-12 — Snapshots par partage · M/L

**Livré 2026-11-05.** Le premier mode demandé — **snapshot récursif filtré par
partage + restauration de fichiers** — est en place, sans migration du layout
des pools :

- `snapshot_schedules` gagne `share` + `share_type` (migration `081`) : les
  deux sont nuls pour une planification de pool, ou renseignés ensemble. Un
  demi‑couple, un nom de partage invalide ou un protocole inconnu est refusé
  avant écriture. La planification stocke le nom + protocole, pas le chemin :
  le chemin courant est résolu à la sauvegarde **et à chaque exécution**.
- Résolution de la cible depuis le système de fichiers : ZFS = dataset monté
  le plus profond contenant le partage (restreint au pool), snapshot
  `dataset@nom` (+ `-r`) ; Btrfs = le partage doit être un sous‑volume,
  snapshots sous `<mount>/.snapshots/<slug>-<hash>/<nom>` (slug déterministe
  et hashé, donc `data/photos` ≠ `data-photos`).
- `ShareBackend` implémente `SnapshotBackend` : la planification par partage
  ne liste et ne purge que le backend du partage. Même GFS, même verrou
  mono‑run par pool, mêmes notifications d'échec ; un prune ne peut donc pas
  sélectionner un snapshot de pool ou d'un autre partage.
- Endpoints :
  - `GET /api/shares/{name}/{share_type}/snapshots` (auth) → snapshots du
    partage + `browse_path` read‑only (`.zfs/snapshot/…` ou
    `.snapshots/…`) ;
  - `POST` même chemin (admin) → snapshot manuel
    `manual-YYYYMMDD-HHMMSS` (ou nom validé) ;
  - `DELETE …/{snapshot}` (admin) → suppression limitée au backend du partage ;
  - `POST …/{snapshot}/restore` (admin) `{ path, confirm: true }` →
    restauration d'un fichier/dossier **relatif au partage** depuis le
    snapshot. Refus des chemins absolus, `..`, NUL et symlinks ; source et
    destination canonicalisées et confinées ; fichier copié atomiquement,
    dossier fusionné, écrasement explicitement confirmé.
- UI Storage → Snapshots : sélecteur **Cible** (tout le pool / un partage avec
  protocole + chemin), badge « partage : … » sur la carte, liste des snapshots
  du partage et dialogue **Restaurer** (chemin relatif + case d'écrasement
  obligatoire). i18n ×4.
- Limite documentée : le snapshot reste celui du dataset/sous‑volume contenant
  le partage ; plusieurs partages du même dataset partagent donc les données
  capturées, mais chaque planification a sa propre rétention, son propre
  listing et sa propre restauration. Le dataset par partage reste un chantier
  ultérieur.
- Tests : **6 unitaires** (`src/snapshots.rs` : cible ZFS la plus profonde,
  confinement Btrfs + slug hashé, chemins de restauration, noms de snapshot,
  browse paths, parsers filtrés) + **1 unitaire** (`src/handlers/
  snapshot_schedules.rs` : couple share/share_type incomplet refusé,
  normalisation `CIFS` → `smb`, partage/protocole invalides) + **1 unitaire**
  (`share_snapshots.rs` : parsing strict des types de partage) +
  **1 E2E** (`snapshot-schedules.spec.ts` 5.1 : la cible partage envoie bien
  `share`/`share_type`) + **1 E2E** (5.2 : restauration bloquée avant la case
  d'écrasement, payload exact) + **1 intégration**
  (`tests/share_snapshots_integration.rs` : `401`/`403`, nom de snapshot
  invalide, confirmation de restauration obligatoire, traversée refusée,
  partage inconnu = `400` jamais `500`) + **1 test d'intégration**
  (`snapshot_schedule_integration.rs` : demi‑couple refusé, planification de
  pool par défaut avec champs nuls).
- Docs : `docs/guides/snapshots.md`, `docs/guides/shares.md`, `docs/API.md`,
  `SCREENS.md`, `TESTING.md`, `FeatureDoc` `snapshots`.
- Fichiers : `src/snapshots.rs`, `src/snapshot_scheduler.rs`,
  `src/storage/snapshot_schedule_repo.rs`, `src/handlers/share_snapshots.rs`,
  `src/handlers/snapshot_schedules.rs`, `src/web/mod.rs`,
  `sqlx/migrations/081_share_snapshot_schedules.sql`, `Storage.svelte`,
  `frontend/src/lib/services/api.ts`, `src/i18n/*.toml`,
  `tests/share_snapshots_integration.rs`,
  `tests/snapshot_schedule_integration.rs`,
  `tests/e2e/storage/snapshot-schedules.spec.ts`.

### [x] P1-13 — Rollback Btrfs en place · M/L

**Livré 2026-11-05.** Le layout `@`/`@snapshots` et le rollback en place sont
en œuvre pour les nouveaux pools, avec une migration explicite pour les
existants :

- **Nouveaux pools Btrfs** : `@` porte les données et est monté au point de
  montage (`subvol=@`) ; `@snapshots` est monté à `<mount>/.snapshots`
  (`subvol=@snapshots`). Deux entrées fstab avec marqueurs distincts
  (`# homevault-pool:…` et `# homevault-pool-snapshots:…`), supprimées
  ensemble à la destruction du pool.
- **Migration legacy → `@`/`@snapshots`** :
  - `GET /api/storage/virtual-disks/{name}/btrfs-layout` (auth) renvoie
    `legacy|at`, `rollback_available`, `migration_required` ;
  - `POST` même chemin (admin) déplace `.snapshots` vers `@snapshots`, crée
    `@`, déplace chaque entrée racine dans `@`, réécrit les deux lignes
    fstab, démonte le top‑level et remonte `subvol=@` + `subvol=@snapshots`.
    Opération sérialisée (`BTRFS_LAYOUT_LOCK`) ; un échec avant le remontage
    remet les entrées déplacées en place. Aucun basculement silencieux.
  - Sans migration, le rollback répond **`503`** (`json_unavailable`) en
    nommant la migration.
- **Rollback en place** : `POST
  /api/storage/virtual-disks/{name}/btrfs-rollback` (admin)
  `{ snapshot, confirm: true }` :
  1. snapshot de sécurité `pre-rollback-<timestamp>` de l’`@` courant ;
  2. démontage de `@snapshots` puis du pool ;
  3. montage du top‑level, renommage de `@` en
     `@before-rollback-<timestamp>`, snapshot `@snapshots/<nom>` créé en `@` ;
  4. remontage de `subvol=@` et `subvol=@snapshots`.
  L’ancien état et le snapshot de sécurité sont conservés (récupération
  manuelle possible) ; en cas d’échec après le swap, un chemin de reprise
  remet l’ancien `@` et remonte le pool, et si la reprise échoue aussi le
  message donne le sous‑volume exact à renommer à la main. Point de
  non‑retour : le rollback n’est pas annulable depuis l’UI, d’où la
  confirmation par nom d’instantané.
- UI Storage : bouton **Layout de rollback Btrfs** sur la carte Btrfs
  (statut puis migration confirmée par nom de pool) ; action **Roll back**
  sur chaque snapshot d’un pool Btrfs migré, confirmation par nom
  d’instantané. i18n ×4.
- Tests : **3 unitaires** (`src/snapshots.rs` : parsing `subvol=@` legacy/at,
  source de montage, plan de rollback + commandes de reprise + noms sûrs) +
  **1 d’intégration** (`tests/storage_integration.rs` : `401` sur le statut,
  rollback sans confirmation ou avec nom traversant refusé avant toute
  commande, pool inconnu = `400` jamais `500`) + **2 E2E**
  (`tests/e2e/storage/storage.spec.ts` : 14.8 migration confirmée par nom de
  pool, 14.9 rollback confirmé par nom d’instantané et payload exact).
  Le démontage/remontage réel n’est pas exécuté par la suite d’intégration
  (il exige un pool Btrfs réel) : les plans de commandes, les noms de
  sous‑volumes et les gates sont couverts unitairement/intégration, et le
  parcours UI est couvert par les E2E mockés.
- Docs : `docs/guides/snapshots.md`, `docs/guides/storage.md`, `docs/API.md`,
  `SCREENS.md`, `TESTING.md`, `FeatureDoc` `snapshots`.
- Fichiers : `src/snapshots.rs`, `src/pools.rs`, `src/handlers/storage.rs`,
  `src/web/mod.rs`, `src/setup.rs` (grant `mv` confiné à
  `/mnt/pools/*` et `/mnt/raid/*`), `Storage.svelte`,
  `frontend/src/lib/services/api.ts`, `src/i18n/*.toml`,
  `tests/storage_integration.rs`,
  `tests/e2e/storage/storage.spec.ts`.

### [x] P1-14 — Pare‑feu GeoIP · M

**Livré 2026-11-05.** Le mode retenu est **ufw + base CIDR** (pas de chaîne
iptables personnalisée) : ufw reste la couche de persistance, donc les règles
survivent au reboot comme les règles manuelles.

- Base pays maintenue manuellement :
  `{DATA_DIRECTORY}/data/geoip/countries.json` (ou `HOMEVAULT_GEOIP_DB`),
  format `{"FR": ["192.0.2.0/24"], "US": ["198.51.100.0/24"]}`. Chaque réseau
  est validé par `ipnetwork`, dédupliqué ; maximum **512 CIDR par pays** et
  **2048 règles** au total.
- Politique `{DATA_DIRECTORY}/data/geoip/policy.json` : `allow|deny` par pays,
  écrite atomiquement (temp + rename) uniquement après validation du plan
  complet, donc une politique inapplicable ne remplace jamais celle en place.
- Application : suppression de toutes les règles dont le commentaire commence
  par `hv-geoip:` (uniquement celles gérées par HomeVault), puis ajout d’une
  règle source par CIDR : `ufw allow|deny from <réseau> comment
  'hv-geoip:<action>:<CC>'`. Un échec en cours d’application retire les règles
  déjà ajoutées.
- Endpoints : `GET /api/firewall/geoip` (auth) expose `available`,
  `ufw_available`, `database_present`, `database_path`, `database_error`,
  les pays et le compte de règles ; `POST` fixe un pays et réconcilie ;
  `DELETE /api/firewall/geoip/{country}` retire le pays ; `POST
  /api/firewall/geoip/apply` réapplique après mise à jour manuelle de la base.
  Mutations admin + journal d’événements.
- Capacité absente : ufw ou base manquante → **`503`** nommé (`503` avec le
  chemin de la base) ; pays/action/réseau invalide → **`400`**. Chaque appel
  privilégié est argv-only sous `UFW_TIMEOUT` (30 s).
- UI Settings → Firewall : bloc **Pays GeoIP** (code pays, allow/deny,
  ajouter, pays gérés + nombre de réseaux + retrait, bouton Appliquer),
  message explicite avec chemin si la base manque ; i18n ×4.
- Tests : **4 unitaires** (`src/geoip.rs` : codes/actions stricts, parsing de
  base + CIDR, plan/cap et pays manquant, commentaires gérés/format),
  **1 d’intégration** (`tests/security_integration.rs` : `401`/`403`, pays et
  action invalides avant toute commande, base absente = `503` jamais un faux
  succès, statut lisible), **1 E2E** (`tests/e2e/security/security.spec.ts`
  5.15 : le formulaire GeoIP mocké envoie `{country, action}`).
- Docs : `docs/guides/firewall.md`, `docs/API.md`, `SCREENS.md`,
  `TESTING.md`, `FeatureDoc` `firewall`.
- Fichiers : `src/geoip.rs`, `src/firewall.rs`, `src/handlers/firewall.rs`,
  `src/web/mod.rs`, `frontend/src/lib/components/settings/FirewallCard.svelte`,
  `frontend/src/lib/services/api.ts`, `src/i18n/*.toml`,
  `tests/security_integration.rs`,
  `tests/e2e/security/security.spec.ts`.

### [x] P1-15 — SNMP v3 · M

**Livré 2026-11-05.** L'agent expose désormais `version` (`v2c` par défaut,
`v3`) :

- **v3 = authPriv** : utilisateur dédié (`homevault` par défaut, alphabet
  `[A-Za-z0-9_.-]`), authentification **SHA**, confidentialité **AES**. Rendu
  `createUser <user> SHA "…" AES "…"` + `rouser <user> authpriv systemonly`.
  Aucun `rwuser` ; la vue reste limitée à system/interfaces/storage.
- **Secrets chiffrés au repos** : `PersistedAgentConfig` scelle les deux
  phrases de passe en `hv1:` AES‑256‑GCM (scopes distincts
  `snmp-agent:v3-auth` / `snmp-agent:v3-priv`, donc un blob d'authentification
  ne peut pas être ouvert comme blob de confidentialité). `AgentConfig` saute
  ces champs à la sérialisation : aucun chemin n'écrit un secret en clair dans
  `snmp-agent.json`. Une phrase vide conserve l'actuelle (ou en génère une de
  32 hex).
- **Contrat d'API inchangé sur les secrets** : `GET /api/snmp/agent` renvoie
  `version`, `v3_username`, `has_v3_auth_secret`, `has_v3_priv_secret` —
  jamais la communauté ni une phrase de passe. Un data directory restauré
  ailleurs rend les blobs illisibles : l'API le dit et demande de ressaisir les
  phrases au lieu de rendre un utilisateur à moitié configuré.
- **v2c corrigé au passage** : `rocommunity … default -V systemonly` applique
  réellement la vue annoncée (auparavant `rocommunity` utilisait la vue
  complète par défaut).
- UI Settings → Monitoring → SNMP agent : sélecteur **Version SNMP** qui
  remplace le bloc de credentials (communauté en v2c ; utilisateur + deux
  phrases en v3, champs mot de passe jamais préremplis). i18n ×4.
- Tests : **9 unitaires supplémentaires** (`src/snmp_agent.rs` : alphabet du
  nom v3, bornes et refus d'injection des phrases, rendu `createUser`/`rouser`
  + absence de `rocommunity`, utilisateur incomplet non rendu, génération +
  réutilisation des deux phrases, scellement/round‑trip sans clair dans le
  JSON, refus du blob croisé auth↔priv, payload redacted, ancien document v2c
  toujours lisible) + **2 E2E** (`snmp-agent.spec.ts` : bascule v2c→v3 et
  payload exact sans communauté ; phrase déjà stockée non renvoyée). Le test
  d'intégration `snmp_integration` reste le contrat `503` sans `snmpd`.
- Docs : `docs/guides/snmp-agent.md`, `docs/API.md`, `SCREENS.md`,
  `TESTING.md` (B4), `FeatureDoc` `snmp-agent`.
- Fichiers : `src/snmp_agent.rs`, `src/handlers/snmp.rs`,
  `SnmpAgentCard.svelte`, `frontend/src/lib/services/api.ts`,
  `src/i18n/*.toml`, `tests/e2e/settings/snmp-agent.spec.ts`.

### [ ] P1-16 — E‑mail OAuth2 · M

- Aujourd'hui SMTP par `msmtp` avec mot de passe (`src/email.rs:28`).
- Ajouter OAuth2 (Gmail/Microsoft) : client id/secret, refresh token chiffré,
  renouvellement, `msmtp` via `oauth2token`.
- Tests : refresh, expiration, erreur de scope ; jamais de token renvoyé.

### [x] P1-17 — Client WireGuard · M

**Livré 2026-11-05.** Le tunnel WireGuard **sortant** existe à côté du serveur
`wg0` :

- `POST /api/vpn/wireguard/clients` importe une configuration `.conf`
  (fichier ou contenu collé). Le parseur est une **liste blanche** :
  `PrivateKey`, `Address`, `DNS`, `MTU`, `ListenPort` en `[Interface]` ;
  `PublicKey`, `PresharedKey`, `AllowedIPs`, `Endpoint`,
  `PersistentKeepalive` en `[Peer]`. `PreUp`/`PostUp`/`PreDown`/`PostDown`
  sont **refusés** (wg-quick les exécute en root), ainsi que `SaveConfig`,
  `Table`/`FwMark` et toute directive inconnue ; chaque clé (32 octets), CIDR,
  DNS et `host:port` est validé avant le rendu. La config est **réécrite**
  depuis les valeurs validées, donc aucune injection de ligne n'est possible.
- L'interface est dérivée de l'uuid (`hvc-` + 10 caractères, ≤ 15), jamais du
  nom affiché, donc jamais `wg0`. Le fichier va dans
  `/etc/wireguard/hvc-<id>.conf` (0600) et les grants sudoers sont limités à
  `wg show hvc-*[/dump]` et `wg-quick up|down hvc-*`.
- `kill_switch` ajoute deux hooks **fixes** `PostUp`/`PreDown` (iptables
  OUTPUT, ip6tables si présent) ; sans l'option, aucun hook n'est écrit.
- Démarrage = `wg-quick up` + vérification du noyau pendant 15 s + rollback
  (`down`, suppression fichier, oubli de l'enregistrement) si l'interface ne
  monte pas. `stop` est idempotent ; `delete` déconnecte, supprime le fichier
  puis l'enregistrement.
- Persistance `{DATA_DIRECTORY}/data/wireguard_clients.json`, configuration
  **scellée** (`hv1:`, scope `wireguard-client:<id>`) : la clé privée n'est
  jamais renvoyée par l'API. Un blob illisible (data directory restauré
  ailleurs) est signalé au lieu de lancer un client à moitié configuré.
- `autostart` (toggle par carte) : au boot, après 20 s, chaque client activé
  est reconnecté par `restore_autostart_clients` ; un échec est journalisé et
  l'enregistrement conservé.
- UI : 3ᵉ onglet **WireGuard client**, import (fichier ou collage), kill
  switch, autostart, cartes endpoint/adresse/pairs/état, connexion,
  déconnexion, suppression. i18n ×4.
- Tests : **13 unitaires** (`src/vpn.rs` : parsing complet, plusieurs pairs +
  fusion des routes, refus des 4 hooks, directive inconnue, clé/CIDR/DNS/
  endpoint invalides, sections manquantes, round-trip du rendu, hooks fixes,
  nom d'interface borné, refus d'un nom stocké non `hvc-*`, argv exact,
  ordre du rollback, chemin confiné) +
  **9 d'intégration** (`tests/vpn_integration.rs` : 401/403, nom vide, config
  invalide, `PostUp` refusé, liste vide, 404 sur start/stop/update/delete,
  traversée d'id) + **5 E2E** (`tests/e2e/vpn/vpn.spec.ts` §15 : onglet +
  champs, bouton désactivé, upload `.conf`, config invalide 400, `PostUp`
  400). Le succès réel (interface noyau) reste hors des suites Rust, comme
  le reste du module.
- Docs : `docs/guides/vpn.md`, `docs/API.md`, `SCREENS.md`, `TESTING.md`,
  `FeatureDoc` `vpn`.
- Fichiers : `src/vpn.rs`, `src/handlers/vpn.rs`, `src/web/mod.rs`,
  `src/setup.rs` (grants `hvc-*`), `frontend/src/routes/VPN.svelte`,
  `frontend/src/lib/services/api.ts`, `src/i18n/*.toml`,
  `tests/vpn_integration.rs`, `tests/e2e/vpn/vpn.spec.ts`.

### [x] P1-18 — Table d'audit · M

- Aujourd'hui les événements partent dans `tracing`/syslog
  (`src/events.rs:1`).
- Migration `audit_log` (horodatage, acteur, action, cible, résultat, IP),
  écriture pour les mutations admin, rétention, endpoint paginé + export.
- Tests : écriture sur chaque mutation critique, non‑répudiation, purge.

**Livré 2026‑11‑08.** Migration `086_audit_log.sql` ; `src/audit.rs` écrit
chaque `events::log_event`/`log_event_warn` via un canal borné (jamais
bloquant, compteur de pertes) et purge à 90 jours au démarrage puis toutes les
6 h. Les événements d'authentification (`auth.login`, `auth.login.trusted`,
`auth.login.failed`, `auth.logout`) portent acteur/action/IP structurés. API
admin : `GET /api/audit` (filtres `from`, `to`, `severity`, `actor`, `action`,
`q`, pagination) et `GET /api/audit/export` (CSV/JSON). Le Journal expose la
même table comme source `audit` (recherche texte). Tests : 9 unitaires
(`src/audit.rs`), suite d'intégration `tests/audit_integration.rs` (5),
E2E Journal 2.4 — écrits, non exécutés. Docs : `journal.md`, `API.md`,
`BACKEND.md`, `PRIVACY.md` (RoPA ligne 16), `SCREENS.md`. **Reste** : filtres
structurés (acteur/action/date) dans l'UI Journal ; aujourd'hui l'API les porte
et le Journal offre la recherche texte.

### [-] P1-19 — `clamd` temps réel · M

**Décision 2026‑11‑06 : rejeté pour la cible.** Coût RAM permanent sur une
machine low-power pour un risque surtout Windows ; l’antivirus à la demande
suffit. Ne pas implémenter sauf changement de cible explicite.

- Option explicite (hors profil basse conso), `OnAccessIncludePath` sur les
  partages choisis, mémoire surveillée, arrêt automatique si < seuil.
- Documenter le coût RAM dans `antivirus.md` ; par défaut **off**.
- Tests : génération `clamd.conf`, refus si binaire absent.

### [-] P1-20 — PPPoE · M

**Décision 2026‑11‑06 : rejeté pour la cible.** La box FAI fait déjà PPPoE
dans la quasi-totalité des foyers ; à ne rouvrir que si HomeVault remplace la
box.

- `pppd` plugin rp‑pppoe, interface WAN, identifiants chiffrés, MTU/MRU,
  reconnexion, statut.
- Tests : génération de conf, rollback, conflit avec une IP statique.

### [~] P1-21 — RAID : vitesse de resync, moniteur, spare auto · M

**Décision 2026‑11‑06 : partiel assumé.** La suggestion de spare suffit ;
l’ajout automatique (`mdadm --add`) est **rejeté** (risque de prendre un
disque non prévu). PPL reste documentaire, pas d’UI.

**Livré partiel 2026-11-05.** `src/raid_stability.rs` :
- profils de vitesse `low` (1 000–25 000 kB/s), `balanced`
  (5 000–100 000) et `fast` (25 000–500 000), écrits dans
  `/proc/sys/dev/raid/speed_limit_min|max` via `sudo tee` ; la fonction
  `apply_speed_profile` existe mais aucun endpoint/UI ne l'appelle encore,
  donc rien n'est imposé au démarrage ;
- parsing des événements `md/array <event> <device>` (`Fail`,
  `DegradedArray`, `SpareActive`, `RebuildStarted`, `RebuildFinished`) ;
- `poll_once` compare l'état des arrays au passage précédent et notifie quand
  un array perd sa redondance ; boucle web au démarrage (2 min) puis toutes
  les 5 min ;
- `pick_spare` choisit un candidat hors membres/spares existants, et
  `spare_suggestion` l'ajoute à la notification de dégradation — ou dit
  explicitement qu'aucun disque libre n'est disponible.
*Reste ouvert : l'ajout du spare (`mdadm --add`) reste humain, volontairement
— choisir un remplaçant modifie l'inventaire de la machine. `--consistency-
policy=ppl` (write hole) n'est pas évalué dans l'UI.*

- Écrire `/proc/sys/dev/raid/speed_limit_min|max` depuis un profil
  « impact faible / rapide / personnalisé » ; le bitmap write‑intent existe
  déjà (`src/disk.rs:1751`).
- `mdadm --monitor --scan --program` pour transformer `Fail`,
  `DegradedArray`, `SpareActive` en notifications HomeVault.
- RAID5 : évaluer `--consistency-policy=ppl` (write hole) et documenter le
  coût.
- Sur un disque `Critical` (SMART), proposer/allouer automatiquement un spare,
  comme DSM. Ne pas dupliquer avec `mdmonitor` si P1-28 l'active.
- Tests : argv exact, parsing des événements, refus sans spare disponible.

### [x] P1-22 — Btrfs : compression, `space_cache=v2`, balance, TRIM · M

**Livré 2026-11-05.** `src/btrfs_maintenance.rs` définit
`defaults,nofail,compress=zstd:3,space_cache=v2,noatime` (+ `subvol=@`/
`@snapshots`) et `src/pools.rs` applique ces options aux nouveaux pools et aux
remontages de récupération. `parse_usage`, `balance_needed` (>4 % de chunks
utilisés **et** <5 Gio non alloués), `balance_command`/`balance_dry_run_command`,
`trim_command` et `device_stats_command` sont livrés ; `btrfs device stats`
alimente déjà les constats de `src/disk_health.rs`.
Une boucle intégrée (premier passage après 1 h, puis toutes les 24 h) lance le
balance **uniquement** si `balance_needed`, et `fstrim` une fois par semaine
(marqueur `state/btrfs-last-trim`) sur les montages inscriptibles hors
`.snapshots`. `HOMEVAULT_BTRFS_MAINTENANCE=0` désactive les deux.

- Monter les pools Btrfs avec `compress=zstd:3,space_cache=v2,noatime`
  (aujourd'hui `defaults,nofail,subvol=@`, `src/pools.rs:1698`).
- `fstrim.timer` hebdomadaire pour les pools SSD (ou job HomeVault).
- Balance conditionnelle si `Metadata,DUP / Data,single > 4 %` et
  `Device unallocated < 5 Gio` (`btrfs balance start -dusage=50`).
- Exposer `btrfs device stats` dans Health.
- Ne pas confondre avec la déduplication (P2-4).
- Tests : options de montage, parsing `btrfs filesystem usage`, dry‑run.

### [x] P1-23 — Import/assemblage mdadm étranger (« online assemble ») · S/M

**Livré 2026-11-05.** `list_importable_mdadm_pools` exécute
`mdadm --examine --scan`, `parse_mdadm_examine_scan` lit les lignes
`ARRAY … UUID=…`, et `assembled_md_names` écarte les arrays déjà assemblés.
L'import lance `mdadm --assemble --scan` puis vérifie le résultat avec
`mdadm --detail` ; un nom absent est une erreur, pas un succès silencieux.
Aucune recréation ni formatage. *Réserve : un array dégradé n'est pas
automatiquement soumis à `--action check` à l'import.*

- `list_importable_pools` ne couvre que ZFS/Btrfs (`src/pools.rs:148`).
- Ajouter `mdadm --assemble --scan` et la détection des arrays « foreign » ;
  import avec confirmation, puis `mdadm --detail` et `--action check` si
  dégradé.
- Tests : array absent/présent, refus si déjà monté, `503` sans mdadm.

### [x] P1-24 — Digest des gros téléchargements + checksums de chunks · M

**Livré 2026-11-05.** Téléchargement : les fichiers ≤64 Mio sont hachés
automatiquement, les plus gros seulement avec `?digest=true` (pas de double
lecture imposée) ; le SHA-256 part dans `Repr-Digest`/`Content-Digest`
(RFC 9530, base64). Upload par chunks : `X-Chunk-Sha256` optionnel est validé
avant commit ; un mismatch répond `422` et supprime le chunk temporaire sans
toucher aux octets précédents. Un en-tête mal formé est refusé avant écriture.

- Au‑delà de `DIGEST_THRESHOLD` (64 Mio), calculer le SHA‑256 en streaming
  et l'exposer via `Repr-Digest`/`Content-Digest` (RFC 9530) ou un `.sha256`
  à côté du fichier.
- Upload par chunks : accepter `X-Chunk-Sha256` optionnel et rejeter un chunk
  corrompu **avant** écriture, avec l'offset de reprise.
- Le digest reste optionnel : ne pas relire deux fois un fichier de plusieurs
  Go sans demande.
- Tests : streaming, absence de buffering mémoire, offset de reprise.

### [x] P1-25 — Vérification finale rsync/rclone · S/M

**Livré 2026-11-05.** Rclone ajoute `--checksum`, `--retries 3`,
`--low-level-retries 10`, `--timeout 5m`, `--contimeout 1m`, puis après succès
`rclone check <src> <dst> --one-way --checksum` ; une différence — ou une
vérification impossible — fait échouer le run. Le mover de cache exécute
`rsync -a --checksum --dry-run --itemize-changes` après copie (sauté si
`--remove-source-files`, faute de source à comparer) et journalise les
différences. Chaque sync rclone écrit désormais une ligne `transfer_history`
(`operation=copy`, `method=rclone-check`, `verified` seulement si le check a
réussi hors dry-run).

- Rsync : après un job, `rsync --checksum --dry-run` ou `--itemize-changes`,
  et journaliser les différences.
- Rclone : `--checksum`, `--retries 3`, `--low-level-retries 10`,
  `--timeout 5m`, `--contimeout 1m`, puis `rclone check`.
- Enregistrer le résultat dans `transfer_history`.
- Tests : sync identique = 0 différence ; fichier divergent signalé.

### [x] P1-26 — Réplication : Btrfs over SSH + vérification destination · M/L

**Livré 2026-11-05.** Btrfs over SSH est accepté : `validate_job` ne le
refuse plus, le send reste local et la réception devient
`ssh … sudo btrfs receive <dataset>` ; list/delete utilisent le même
`ssh_prefix` (qui refuse une cible SSH incomplète). Un transfert échoué est
retenté 3 fois (backoff 2/4/8 s). Après succès,
`destination_verify_command` lit la propriété `checksum` ZFS ou exécute
`btrfs subvolume show` (local ou SSH) ; un échec fait échouer le run.
*Réserve : la vérification prouve que la destination existe et est lisible,
pas une comparaison octet-à-octet (les checksums ZFS/Btrfs couvrent les
blocs) ; aucun scrub destination n'est lancé.*

- `src/replication.rs:274` refuse Btrfs over SSH. Ajouter le pipeline
  `btrfs send | ssh … btrfs receive`.
- Après réception : `zfs get -H -o value checksum <dataset>` /
  `btrfs subvolume show` ; lancer un scrub destination ou au moins
  `btrfs device stats`.
- Reprise : `zfs send -s`/`receive -s` existe déjà ; ajouter un retry avec
  backoff sur coupure SSH.
- Tests : plan de commandes, parsing destination, reprise après coupure.

### [~] P1-27 — Chaîne d'approvisionnement : digest + signature · M

**Décision 2026‑11‑06 : finir la signature seulement.** Le pinning obligatoire
de toutes les images est rejeté (il casserait les mises à jour du catalogue) ;
la signature des manifestes de stores externes reste utile.

**Livré partiel 2026-11-05.** `src/supply_chain.rs` fournit la syntaxe SHA-256
stricte, `pinned_digest`/`is_pinned`, `verify_ed25519` et `verify_artifact`
(signature absente ou clé absente = refus). `DockerService::pull_image` appelle
`validate_image_reference` avant `docker pull`, donc un `@sha256:` mal formé
est refusé côté HomeVault. `HOMEVAULT_REQUIRE_PINNED_IMAGES=1` fait en plus
refuser une installation catalogue dont un `image:` du compose rendu n'est pas
épinglé `@sha256:…`, avec la liste des images fautives.
*Restent ouverts : signature et vérification des manifestes du catalogue, et
clé Ed25519 publique configurée dans ce build.*

- Épingler les images Docker par digest (`@sha256:…`) et vérifier avant
  `docker pull`.
- Signer les manifestes du catalogue (minisign/ed25519) et vérifier avant
  chargement.
- Vérifier les archives de mise à jour par SHA‑256 + signature. Complète
  P2-8 (mise à jour signée/atomique) sans le remplacer.
- Tests : image non épinglée refusée, signature absente/invalide refusée.

### [~] P1-28 — Stabilité système : unité, `sysctl`, `smartd`/`mdmonitor`, UPS · S/M

**Décision 2026‑11‑06 : partiel assumé.** L’avertissement installateur suffit ;
la carte UI write-cache/UPS est gelée (peu de valeur, surface en plus).

**Livré partiel 2026-11-05.** `installer/install.sh` écrit
`LimitNOFILE=65536`, `TimeoutStopSec=30`, `StartLimitIntervalSec=60`,
`StartLimitBurst=5`, `OOMPolicy=stop`, et
`/etc/sysctl.d/99-homevault.conf` (`kernel.panic=10`,
`kernel.panic_on_oops=1`, `vm.dirty_background_ratio=5`, `vm.dirty_ratio=10`).
`src/system_stability.rs` garde les mêmes valeurs et fournit
`parse_write_cache` (`hdparm -W`), `write_cache_warning` (cache actif sans
UPS) et `duplicate_monitor_warning` (smartd/mdmonitor + samplers HomeVault).
L'installateur vérifie `systemctl is-enabled smartd|mdmonitor` et avertit
quand l'un tourne en double.
*Restent ouverts : activer/choisir explicitement smartd/mdmonitor dans
l'installateur, et afficher les avertissements write cache/UPS/duplicate dans
l'UI.*

- Unité HomeVault : `LimitNOFILE=65536`, `TimeoutStopSec=30`,
  `StartLimitIntervalSec=60`, `StartLimitBurst=5`, `OOMPolicy=stop`.
- `sysctl` : `kernel.panic=10`, `kernel.panic_on_oops=1`,
  `vm.dirty_background_ratio=5`, `vm.dirty_ratio=10` (à ajuster).
- Activer `smartd` et `mdmonitor` **ou** les jobs HomeVault — jamais les deux
  en double.
- Avertir si le write cache d'un disque est actif sans UPS.
- Tests : `systemd-analyze verify`, parsing `hdparm -W`, non‑régression.

### [x] P1-29 — SMB WS-Discovery · S

**Livré 2026‑11‑08.** `SmbConfig` gagne `ws_discovery` (défaut **on**, comme la
découverte réseau Windows de DSM). `src/ws_discovery.rs` écrit
`/etc/systemd/system/homevault-wsdd.service` et pilote `wsdd2` (nom NetBIOS et
workgroup sanitizés, `DynamicUser`, `NoNewPrivileges`, `ProtectSystem=strict`,
familles d'adresses étroites) ; désactiver supprime l'unité. L'enregistrement
SMB applique le réglage en best-effort, et un reconcile au démarrage le
rétablit. `GET /api/shares/ws-discovery` expose
`available/enabled/running/detail` ; la carte SMB affiche « wsdd2 is not
installed » plutôt qu'un interrupteur mort. Installateur : `wsdd2` en paquet
optionnel ; `dependencies.rs` le liste. Tests : 6 unitaires
(`src/ws_discovery.rs`), 1 (`src/shares.rs`), 1 d'intégration
(`tests/shares_integration.rs`), E2E Shares 4.9b — écrits, non exécutés.
Docs : `docs/guides/shares.md`, `docs/API.md`, `SCREENS.md`, FeatureDoc
`shares`.

### [x] P1-30 — Mises à jour Docker planifiées/auto · S/M

**Livré 2026‑11‑09.** `src/docker_update_scheduler.rs` : planificateur
**opt‑in** (off par défaut) pour les apps du catalogue, jamais les conteneurs
arbitraires. Réglage dans `data/docker_updates.json` (`interval_hours` 6–168,
dernier résultat/erreur). Le premier tick après activation pose une
**baseline** et attend un intervalle complet ; un run dû réutilise
`update_all_pending` (le même chemin que « Tout mettre à jour ») et ne
redémarre que les apps ayant réellement une mise à jour. Un échec avance quand
même l'horloge et enregistre `last_error` (pas de martèlement toutes les
5 min). API admin `GET/PUT /api/docker/updates/schedule` et
`POST /api/docker/updates/run-now` ; divulgation repliée **Scheduled updates**
dans l'onglet App Catalog. Tests : 5 unitaires, 3 d'intégration
(`tests/docker_update_schedule_integration.rs`), E2E Docker 15.6 — écrits, non
exécutés. Docs : `docs/guides/docker.md`, `docs/API.md`, `SCREENS.md`,
FeatureDoc `docker`.

---

## 4. P2 — Chantiers structurants

### [~] P2-1 — SMB AD/Kerberos complet · L

**Décision 2026‑11‑06 : gelé au partiel.** La configuration ADS et join/leave
sont livrés ; la validation DC/PAM/SID/ACL n’a de sens que si la cible
entreprise est confirmée. Ne pas investir davantage en attendant.

**Livré partiel 2026-11-05.** `SmbConfig` gagne `realm` et `idmap_range`.
`security_mode = ads` (et `domain`) est accepté avec un realm DNS et une plage
idmap valides ; `server` reste refusé. Le bloc `[global]` rend `realm`,
`idmap config * : backend = tdb`, la plage, `winbind use default domain = yes`
et le trio ACL Windows (`vfs objects = acl_xattr`, `map acl inherit = yes`,
`store dos attributes = yes`). Une config ADS sans realm est normalisée en
`user`. `POST /api/shares/smb/domain/join|leave` exécute
`net ads join|leave -U <compte>` avec le mot de passe sur **stdin** (jamais
dans argv) ; sans winbind, refus nommé. Le formulaire SMB avancé propose ADS
et les champs realm/idmap.
*Décision : gelé au partiel. Validation DC/PAM/SID/ACL seulement si la cible
entreprise est confirmée.*

### [~] P2-2 — NFSv4 + Kerberos · L

**Décision 2026‑11‑06 : gelé au partiel.** Le flavor et la validation keytab
sont livrés ; KDC réel, keytab et clients `sec=krb5*` restent hors cible
domestique. Ne pas investir davantage.

**Livré partiel 2026-11-05.** `NfsConfig` gagne `security_flavor`
(`sys|krb5|krb5i|krb5p`). Une valeur non-`sys` est rendue `sec=…` sur chaque
export et refusée si `version` n'est pas explicitement 4/4.1/4.2 (`auto` ne
garantit pas v4). `save_nfs_config` refuse aussi Kerberos sans
`/etc/krb5.keytab` et `rpc.svcgssd`/`gssproxy`. Le formulaire NFS expose le
choix et l'aide keytab/gssproxy.
*Décision : gelé au partiel. KDC/keytab/idmapd/clients seulement si la cible
entreprise est confirmée.*

### [~] P2-3 — Thin provisioning · L

**Décision 2026‑11‑06 : gelé au support zvol.** L’overcommit LVM/UI est
**rejeté** : une saturation imprévisible est exactement ce que HomeVault doit
éviter. Le support zvol actuel suffit.

**Livré partiel 2026-11-05.** `src/zfs_volumes.rs` gère les zvols ZFS : création
**thin** (`zfs create -s -V … -o refreservation=none`) ou **thick**, liste
(`zfs list -H -p -t volume`) avec marqueur `saturated` à ≥90 % de la taille
nominale, destruction. Noms et tailles validés avant toute commande. API :
`GET/POST /api/storage/zfs/volumes`, `DELETE /api/storage/zfs/volumes/{*name}`.
*Décision : LVM thin, UI d’overcommit et snapshots de zvol ne sont pas
prévus ; le support zvol actuel est la portée finale.*

### [~] P2-4 — Déduplication · XL

**Décision 2026‑11‑06 : design conservé, exécution gelée.** Les
snapshots/reflinks Btrfs couvrent l’essentiel ; ne coder l’exécution que si le
benchmark de `docs/analysis/dedup-design.md` démontre un gain réel.

**Livré partiel 2026-11-05 (conception + planification).** Note
`docs/analysis/dedup-design.md` : options Btrfs `duperemove` vs ZFS
`dedup=on`, coût RAM irréversible, plan de benchmark et règles de sûreté.
`src/dedup.rs` valide les chemins (absolus, sans `..`, jamais `.snapshots`,
≤16), choisit la stratégie et construit l'argv `duperemove -r -d -h
--hashfile=…` ; **aucune exécution** n'est branchée avant le benchmark.
*Décision : benchmark seulement si un besoin de déduplication est confirmé ;
pas d’endpoint/scheduler/UI avant.*

### [-] P2-5 — KMIP / coffre de clés · XL

**Décision 2026‑11‑06 : rejeté pour la cible.** Dépendance réseau critique au
boot (KMIP/TLS/HA/recovery) pour un bénéfice quasi nul sur un NAS familial.
Alternatives : TPM 2.0, clé USB hors ligne, Vault Transit.

Client KMIP, enrôlement, rotation, déverrouillage au boot, HA. Enterprise ;
ne pas livrer sans menace/coût documentés.

### [-] P2-6 — WORM par fichier · M/L

**Décision 2026‑11‑06 : rejeté pour la cible.** Les snapshots immuables
couvrent déjà l’anti-ransomware ; `chattr +i` + break-glass ajoute un risque de
verrouillage administratif.

Verrou `chattr +i`/immutable + journal, déverrouillage break‑glass avec phrase
tapée, audit. Complète les snapshots immuables.

### [~] P2-7 — Isolation d'exécution · L

**Décision 2026‑11‑06 : priorité.** Gain sécurité réel ; à faire avec des tests
de non-régression car HomeVault utilise `sudo`.

- Découper les privilèges : un utilisateur système par sous‑système
  (stockage, réseau, sauvegarde), `NoNewPrivileges`, `ProtectSystem`,
  `ProtectHome`, `PrivateTmp`, capacités minimales.
- Profils AppArmor par unit si disponible ; refus si le LSM est absent.
- Tests : `systemd-analyze security` avant/après, non‑régression.

**Livré 2026‑11‑08 (partiel).** L'unité est rendue par `render_service_unit`
et embarque `ProtectSystem=true`, `ReadOnlyPaths=-/opt/homevault`,
`PrivateTmp=true`, `RestrictSUIDSGID=true`, `LockPersonality=true`,
`RestrictRealtime=true`, `ProtectKernelLogs=true`, et
`NoNewPrivileges=true` en mode root uniquement. Les directives qui casseraient
une fonction livrée sont **volontairement absentes** et documentées :
`ProtectKernelTunables` (sysctl WireGuard), `ProtectKernelModules` (modprobe
iSCSI), `ProtectHostname`, `ProtectClock`, `PrivateDevices`, `ProtectHome`
(dossiers utilisateurs), `CapabilityBoundingSet` (sudo setuid). Profil AppArmor
**opt‑in** (`HV_APPARMOR=1`, complain par défaut, enforce après validation), et
score `systemd-analyze security` imprimé par l'installeur. Tests :
`installer/tests/unit-hardening.test.sh` (non exécutés). Docs :
`docs/guides/security.md` → « Process isolation ». **Reste** : passe terrain
`systemd-analyze security`/`aa-logprof`, enforce AppArmor, et l'étude d'un
utilisateur système par sous‑système (aujourd'hui un seul compte de service).

### [~] P2-8 — Mise à jour signée + atomique · L/XL

**Décision 2026‑11‑06 : priorité.** Intégrité de la chaîne et rollback : gain
élevé, pas de dépendance externe.

Manifeste signé, vérification avant application, binaire précédent conservé,
rollback automatique si le health check échoue. Aligne HomeVault sur la chaîne
de confiance DSM (PKCS#7) sans en copier la complexité.

**Livré 2026‑11‑08 (partiel).** `installer/install.sh` vérifie un manifeste
signé (Ed25519 si OpenSSL 3, sinon RSA/ECDSA SHA‑256), applique l'archive
atomiquement (staging + smoke test `--version` + rename), conserve
`homevault.previous` et **rollback automatiquement** si
`/api/auth/setup-status` ne répond pas en 20 s. Le résultat est écrit dans
`data/updates/last-result.json`, exposé par `GET /settings/version`
(`last_update`) et affiché dans Settings. Nouveau mode `--verify-manifest`
(sans root) et signature au `--package` via `HV_SIGNING_KEY`. Tests :
`installer/tests/update-engine.test.sh` (non exécutés). Docs :
`docs/guides/settings.md` → « System → Signed, atomic updates ».
**Reste** : déclenchement depuis l'UI (le bouton « Check for Updates » ne fait
que vérifier) et publication de la clé publique de production.

### [~] P2-9 — Réparation / restauration bare‑metal · L

**Décision 2026‑11‑06 : priorité, runbook d’abord.** Une restauration testée
sur VM vaut mieux qu’une usine à gaz d’images système.

Équivalent minimal de `md0`/rootbackup : image système restaurable, vérification
d'intégrité, procédure testée sur VM. Aujourd'hui : runbook + restic.

**Livré 2026‑11‑08 (drill non destructif).** `src/restore_drill.rs` vérifie la
chaîne de reprise locale sans toucher aux données : copie du dernier snapshot
`data/backups/*.db` dans un répertoire temporaire, ouverture **en lecture
seule**, `PRAGMA integrity_check`, comptage des tables/utilisateurs ;
décodage de la dernière archive `data/config-backups/*.hvbak` avec vérification
des checksums (archive chiffrée → passphrase requise) ; état du dernier job
restic. Verdict **Ready to restore** seulement si les deux moitiés sont
vérifiées. API `GET/POST /api/settings/backup/restore-drill` et bouton
**Verify restore** dans Settings → System → Database snapshots ; rapport
persisté dans `data/restore-drill.json`. Runbook mis à jour
(`docs/guides/disaster-recovery.md`). Tests : 4 unitaires
(`src/restore_drill.rs`), 5 d'intégration
(`tests/restore_drill_integration.rs`), E2E Settings 8.6 — écrits, non
exécutés. **Reste** : le test de reconstruction complète sur VM (boot d'une VM
jetable, installation, restauration DB + config + données, vérification) et,
si la cible le justifie, une image système équivalente à `md0`.

### [x] P2-10 — Prédiction de panne disque par régression SMART · M

**Livré 2026-11-05.** `src/disk_prediction.rs` fait une régression
linéaire (moindres carrés) sur 5/187/188/194/197/198 avec au moins 5 points,
seuils 10/1/1/60/1/1, et renvoie pente/jour, `r_squared`, `days_remaining` et
un intervalle basé sur les résidus. Une pente nulle ou un fit bruité ne produit
pas d'alerte. `GET /api/health/disk-health/prediction?serial=…` expose une
série et `GET /api/health/disk-health/predictions` toutes les séries
échantillonnées. L'écran Health affiche une carte **Failure prediction**
(attribut, jours estimés, qualité de l'ajustement), masquée tant qu'aucune
tendance n'existe.

- Dépend de P0-8 (historique). Régression simple sur 5/187/188/197/198 +
  température, par modèle/série ; pas de modèle ML.
- Afficher une tendance et un « jours restants estimés » avec intervalle de
  confiance ; jamais une alerte sur un point isolé.
- Tests : jeu de données synthétique (disque sain / mourant), pas de faux
  positif sur un attribut stable.

### [x] P2-11 — Compression/défragmentation Btrfs · M

**Livré 2026-11-05.** `validate_compression_level` accepte
`zstd:1`…`zstd:15`/`none` ; `defrag_command` refuse tout chemin sous
`/.snapshots` **avant** d'exécuter. La migration `084_btrfs_compression.sql`
ajoute `btrfs_compression_policies` (path, level, defrag, last_applied_at) :
`POST/PUT/GET/DELETE /api/storage/btrfs/policies` valident un répertoire
absolu sans `..`, puis `apply_compression_policy` défragmente
(`-r -c<level>`) et pose `chattr +c`/`-c`. La boucle quotidienne applique une
politique due une fois par mois. L'écran Storage gagne un onglet
**Compression** (liste, formulaire, Appliquer maintenant, Supprimer).
*Réserve : l'écran n'a pas pu être vérifié avec `svelte-check` dans ce
worktree, et la vérification automatique du maintien des reflinks n'est pas
instrumentée.*

- Complète P1-22 : politique de compression par partage (`zstd:1..15`),
  défragmentation **hors snapshots** (sinon consommation d'espace), et
  vérification que les reflinks/`instant-copy` restent possibles.
- Tests : options de montage, dry‑run, espace avant/après.

### [x] P2-12 — Rapport de santé hebdomadaire · S/M

**Livré 2026-11-05.** `src/health_report.rs` agrège les constats disque (dont
critiques), le dernier échantillon SMART, l'espace libre par pool, un scrub en
attente et le dernier `restic_check` (statut + fin, lu depuis
`scheduled_task_runs`), puis envoie au plus une notification par semaine
(`health.storage`, état dans `state/weekly-health-report`). Toute mesure
absente est rendue `unknown`, jamais `OK`. Vérification toutes les 6 h après
15 min de démarrage ; `HOMEVAULT_WEEKLY_HEALTH_REPORT=0` désactive le rapport.

- Agrège : dernier scrub, dernier test SMART, secteurs réalloués, erreurs I/O,
  état RAID, dernier `restic check`, espace par pool, température.
- Envoi e‑mail/notification, une fois par semaine, désactivable.
- Tests : rendu, absence de données = « unknown » jamais « OK ».

### [x] P2-13 — Snapshots applicatifs (quiescence) · L

**Livré 2026-11-05.** `src/snapshot_quiescence.rs` définit
`Consistency::{CrashConsistent, ApplicationConsistent}`, un `QuiesceHook`
(pre/post), `plan(...)`, `after_hook_failure` et l'exécution argv
(`run_pre_hooks`/`run_post_hooks`, 60 s, sans shell). Les hooks sont persistés
dans `snapshot_quiescence_hooks` (migration 085) et gérés depuis Storage →
Snapshots ou `/api/storage/snapshot-hooks`. Le planificateur exécute les hooks
pre avant le snapshot et les post en ordre inverse ; un pre en échec
rétrograde le run en `crash-consistent` avec la raison, sans annuler le
snapshot. Le résumé de run porte toujours le label de consistance.
*Réserve : l'écran n'a pas été vérifié avec `svelte-check` dans ce worktree.*

- Hooks pre/post snapshot pour bases (PostgreSQL/SQLite) et VM (Incus) :
  `fsfreeze` ou `pg_start_backup`, flush, snapshot, thaw.
- Distinguer explicitement `crash-consistent` (défaut) et
  `application-consistent` (option).
- Dépend de P1-12/P1-13. Tests : hook absent = snapshot crash‑consistent
  annoncé, jamais silencieusement « applicatif ».

### [x] P2-14 — Durabilité SQLite · S

**Livré 2026-11-05.** `HOMEVAULT_SQLITE_FULL_SYNC` accepte
`1`/`true`/`yes`/`full` et switch `PRAGMA synchronous` de `NORMAL` à `FULL` ;
toute autre valeur garde `NORMAL`. Le coût (un fsync par transaction) et le
compromis sont documentés dans `docs/guides/development.md`.

- `PRAGMA synchronous = NORMAL` (`src/storage/database.rs:134`) protège de la
  corruption, mais la dernière transaction peut être perdue à la coupure.
- Ajouter un réglage `full` (ou `fullfsync` sur macOS) pour la base de
  métadonnées, avec mesure du coût ; documenter le compromis.
- Tests : coupure simulée après commit, vérifier la présence de la ligne.

### [~] P2-15 — Témoin de réplication (witness) · XL

**Décision 2026‑11‑06 : gelé.** Suppose deux sites + un troisième témoin ;
hors cible mono-site. Le client HTTP et le lease restent disponibles mais ne
sont pas promus.

**Livré partiel 2026-11-05.** `src/replication_witness.rs` définit
`WitnessLease` (holder/epoch/expires_at), `may_promote`, `take_lease` et
`split_brain`, plus un client HTTP réel (`WitnessClient`) :
`GET/POST/DELETE /lease`, jeton bearer optionnel, délai de 10 s. La
configuration vient de `HOMEVAULT_WITNESS_URL`, `HOMEVAULT_WITNESS_NODE`,
`HOMEVAULT_WITNESS_TOKEN` et `HOMEVAULT_WITNESS_TTL` (10–86 400 s). Quand un
témoin est configuré, **chaque run de réplication acquiert/renouvelle le lease
avant de créer un snapshot** ; un refus (`409` ou holder différent) fait
échouer le run. `GET/POST /api/replication/witness` exposent l'état et
l'acquisition manuelle (admin).
*Décision : gelé. Le service témoin, la promotion et la recovery split-brain
restent externes ; ne pas investir sans scénario multi-site réel.*

- Alternative légère au HA DRBD/corosync rejeté en P3 : un troisième site
  témoin (ou un verrou S3) empêche les deux nœuds de se croire actifs.
- Dépend de P1-26 (réplication vérifiée). Ne pas livrer sans scénario
  split‑brain testé et procédure de recovery.

---

## 5. Déjà livré — ne pas refaire

Vérifié dans le code au 04/11/2026 (réserves d'intégrité ajoutées le
05/11/2026) ; ces items ne doivent **pas** apparaître comme tâches :

- DHCP/TFTP/PXE, DNS local, reverse proxy, UPnP‑IGD, Web Push, SMS, ISO,
  tags, corbeille, checksums, liens de partage, Time Machine, rsync serveur,
  SFTP/WebDAV/FTP, iSCSI, LUKS2, snapshots GFS + immuables, réplication,
  restic, cible append‑only, rclone, Syncthing, WireGuard/OpenVPN serveur,
  bond/VLAN, trafic shaping, WOL/mDNS, SNMP agent v1/v2c, syslog, UPS/NUT,
  ventilateurs, power, terminal, tâches planifiées, métriques, journal,
  config backup, PWA, Docker/Compose, catalogue, Incus (base), IA/RAG/MCP,
  passkeys, trusted devices, rescue e‑mail, code hors ligne, 2FA, SSO
  LDAP/AD/OIDC/SAML, antivirus à la demande, anti‑ransomware, Security
  Advisor, auto‑block, quotas.

> **Réserves d'intégrité (revue du 2026‑11‑05).** « Déjà livré » ne veut pas
> dire « déjà fiable ». Ces fonctions existent, mais une lacune est couverte
> par un nouvel item :
>
> - **USB Copy** existe et sa vérification compare désormais un SHA-256 réel
>   (P0-5) ; restent les stratégies de copie (incremental/mirror/versioned) et
>   l'historique par fichier.
> - **Checksums** : le digest des gros téléchargements et les checksums de
>   chunks sont livrés (P1-24) ; reste le test E2E navigateur.
> - **SMB multichannel**, durable handles, chiffrement, signature, `strict
>   sync` et AIO sont livrés (P0-11) ; restent les tests E2E/UI sur un hôte
>   avec Samba.
> - **iSCSI** : FUA/Sync Cache, write cache, UNMAP et la protection volume
>   plein sont livrés (P0-12) ; le passage `configfs` réel reste à valider sur
>   un hôte équipé.
> - **Restic** : `check` planifié avec `--read-data-subset` (P0-10) ; reste le
>   test E2E sur un vrai dépôt.
> - **Config backup** est solide et les écritures de `smb.conf`/exports sont
>   atomiques (P0-9) ; reste le test de coupure `kill -9` en intégration.
> - **SMART** est historisé et alerté (P0-8) ; la prédiction de panne est
>   affichée dans une carte Health (P2-10).
> - **Snapshots GFS** existent ; la quiescence applicative est branchée via
>   des hooks pre/post (P2-13).
> - **Réplication** : Btrfs over SSH et la vérification destination sont
>   livrés (P1-26) ; aucun scrub destination n'est lancé.
> - **Rclone** : la vérification finale et l’écriture dans `transfer_history`
>   sont livrées (P1-25).
> - **Catalogue/Docker** : un digest mal formé est refusé avant `docker pull`
>   (P1-27) ; le pinning strict est opt-in et la signature du catalogue reste
>   à faire (décision : signature seulement, pas de pin obligatoire).

---

## 6. P3 — Rejeté / non‑objectifs

| Sujet | Décision | Raison |
|---|---|---|
| QuickConnect (relais) | **Rejeter** | Infrastructure tierce, dépendance de compte ; ACME + DDNS + reachability + VPN couvrent le besoin. |
| Active Insight / SynoAnalytics | **Rejeter** | Télémétrie et canal d'actions distantes ; contraire au positionnement vie privée. |
| HA cluster (DRBD/Pacemaker) | **Rejeter** | Complexité et coût matériel hors cible « petit NAS » ; réplication + snapshots suffisent. Un **témoin de réplication** léger reste possible (P2-15). |
| IDS/IPS (Threat Prevention) | **Rejeter** | Faux sentiment de sécurité à faible trafic ; le pare‑feu + mises à jour priment. |
| CMS multi‑NAS | **Rejeter** | Non pertinent pour 1 à 3 appareils. |
| Imprimante / DLNA / photothèque / mail / calendrier / contacts / office / chat d’équipe / NVR / Web Station / PHP | **Rejeter en natif** | Assumé via le catalogue Docker (Jellyfin, Immich, etc.) ; un calendrier/contacts passe par une app catalogue (Radicale, Baïkal). |
| Écosystème de paquets `.spk` | **Ne pas copier** | Le modèle Docker + catalogue est plus sûr et plus maintenable. |
| ExtJS 3.4 / 21 locales / aide 30 Mo | **Ne pas copier** | Dette de sécurité et de maintenance. |
| L2TP/PPTP serveur | **Rejeter PPTP** ; L2TP en P3 | PPTP est cassé cryptographiquement ; WireGuard/OpenVPN suffisent. |
| **Serveur OpenVPN** | **Rejeter** | Le client OpenVPN existe ; le serveur WireGuard couvre l’accès distant en plus simple et plus rapide. Un second serveur = PKI et démon root supplémentaires sans gain domestique. |
| **SHR / SHR2** | **Rejeter** | Abstraction propriétaire Synology ; ZFS raidz et mdadm couvrent la cible avec des outils standard et réparables. |
| **NZB / eMule (téléchargeur)** | **Rejeter** | Niche, credentials à stocker, exposition légale ; garder HTTP/HTTPS/FTP/SFTP/magnet + RSS/planification/limites/extraction. |
| KMIP à court terme | **Rejeter** | Dépendance réseau critique au boot pour un bénéfice quasi nul sur un NAS familial ; TPM/USB/Vault Transit couvrent le besoin. |
| WORM par fichier | **Rejeter** | Les snapshots immuables couvrent l’anti-ransomware ; risque de verrouillage administratif. |
| `clamd` temps réel | **Rejeter** | Coût RAM permanent sur une machine low-power pour un risque surtout Windows. |
| PPPoE | **Rejeter** | La box FAI le fait déjà dans la quasi-totalité des foyers. |
| Overcommit thin (LVM/UI) | **Rejeter** | Saturation imprévisible ; le support zvol ZFS actuel suffit. |
| Déduplication automatique | **Rejeter pour l’instant** | Snapshots/reflinks Btrfs couvrent l’essentiel ; uniquement si le benchmark prouve un gain. |
| Witness multi-site | **Geler** | Suppose deux sites + un troisième témoin ; hors cible mono-site. |

---

## 7. Sprints recommandés

### Sprint 1 — « Arrêter de mentir » (2–3 jours)

- [x] P0-1 NFS réellement appliqué
- [x] P0-2 refus SMB `ads/domain/server`
- [x] P0-3 `additional_params` + min/max protocol
- [x] P0-4 test de contrat des champs
- [x] P1-1 multichannel SMB
- [x] P1-3 rétention métriques

**Sortie** : `cargo test`, `npm run check`, E2E Shares/Users verts ; docs à jour.

### Sprint 2 — « Confort » (1 semaine)

- [x] P1-2 expiration de mot de passe
- [x] P1-4 OpenAPI
- [x] P1-5 DDNS
- [x] P1-6 clone Incus
- [x] P1-7 routes statiques
- [x] P1-8 proxy client

### Sprint 3 — « Stockage & réseau » (2–3 semaines)

- [x] P1-9 raidz3/spare
- [x] P1-10 restic mount
- [x] P1-11 migration RAID
- [x] P1-12 snapshots par partage
- [x] P1-13 rollback Btrfs en place
- [x] P1-14 GeoIP
- [x] P1-15 SNMP v3
- [ ] P1-16 OAuth2 e‑mail
- [x] P1-17 client WG

### Sprint 4 — « Intégrité prouvable » (2–3 semaines)

- [x] P0-5 USB Copy : vérification réelle
- [x] P0-6 scrub planifié + post‑arrêt sale + reprise exacte (`scrub-in-progress`)
- [x] P0-7 récupération de montage + fsck
- [x] P0-8 SMART/I/O + alertes
- [x] P0-9 écritures de configuration atomiques
- [x] P0-10 `restic check` planifié + `--read-data-subset`
- [x] P0-11 SMB durable handles/chiffrement/`strict sync`
- [x] P0-12 iSCSI FUA/write cache/UNMAP + protection volume plein

**Sortie** : tests de corruption réelle verts (octet modifié, pack tronqué,
superbloc abîmé, kill pendant `rename`), `cargo test`, E2E Storage/Backup/
Shares, docs et `TESTING.md` à jour.

### Sprint 5 — « Consolidation stockage & transferts » (2 semaines)

- [~] P1-21 RAID resync/moniteur/spare auto (profils + moniteur ; ajout du spare manuel)
- [x] P1-22 Btrfs compression/balance/TRIM (boucle balance/TRIM ; politique par partage en P2-11)
- [x] P1-23 import mdadm étranger
- [x] P1-24 digest gros téléchargements/chunks
- [x] P1-25 vérification rsync/rclone + `transfer_history`
- [x] P1-26 réplication Btrfs over SSH + vérification destination
- [~] P1-27 digest/signature chaîne d'approvisionnement (pinning strict opt-in ; signature non imposée)
- [~] P1-28 stabilité système (unité, sysctl, helpers ; avertissements non exposés)

### Trimestre — « Structure »

- [~] P2-1 AD/Kerberos (gelé au partiel : config ADS + join/leave)
- [~] P2-2 NFSv4/Kerberos (gelé au partiel : flavor + validation keytab)
- [~] P2-7 isolation d'exécution — installeur livré 2026‑11‑08, enforce/terrain restants
- [~] P2-8 mises à jour signées — installeur livré 2026‑11‑08, déclenchement UI restant
- [~] P2-9 restauration bare-metal — drill livré 2026‑11‑08, test VM restant
- [~] P2-3 thin (gelé au support zvol ; overcommit rejeté)
- [~] P2-4 déduplication (design conservé ; exécution gelée)
- [-] P2-5 KMIP (rejeté pour la cible)
- [-] P2-6 WORM (rejeté pour la cible)
- [x] P2-10 prédiction SMART + carte de tendance
- [x] P2-11 compression/défrag Btrfs + onglet Compression
- [x] P2-12 rapport de santé hebdomadaire (`restic check` lu, désactivable par env)
- [x] P2-13 snapshots applicatifs (hooks pre/post + UI)
- [x] P2-14 durabilité SQLite (`HOMEVAULT_SQLITE_FULL_SYNC`)
- [~] P2-15 témoin de réplication (client HTTP + lease au démarrage ; service/recovery externe)

---

## 8. Traçabilité vers l'analyse

| Tâche | Section source |
|---|---|
| P0-1, P1-12, P1-13 | §14.3, §14.4, §14.8 |
| P0-2, P1-1, P2-1 | §14.4, §14.8, §15.4 |
| P0-3, P0-4 | §14.4, §14.8 (constats `src/shares.rs`) |
| P1-2 | §14.2 |
| P1-3 | §14.8 (métriques) |
| P1-4 | §6.3 (OpenAPI DSM) |
| P1-5 | §9 (25 fournisseurs DSM) |
| P1-7, P1-8, P1-20 | §14.6, §14.8 |
| P1-9, P1-11 | §14.3, §8.2 |
| P1-10 | §14.8 (restic mount) |
| P1-14, P1-15, P1-16, P1-18, P1-19 | §10, §14.2, §14.8 |
| P1-17 | §9, §14.6 |
| P1-6 | §14.8 (Incus) |
| P2-1…P2-2 | §15.4 |
| P2-3…P2-6 | §14.3, §15.4 |
| P2-7…P2-9 | §15.4 |
| P0-5, P0-12 | Revue 2026‑11‑05 §4.2 (USB/iSCSI), §5 P0‑1/P0‑12 |
| P0-6, P0-7, P0-8 | Revue 2026‑11‑05 §3.2, §4.1, §5 P0‑2/P0‑3/P0‑4 |
| P0-9, P0-10 | Revue 2026‑11‑05 §4.3, §4.4, §5 P0‑5/P0‑6 |
| P0-11 | Revue 2026‑11‑05 §3.2, §4.2, §5 P0‑7 |
| P1-21…P1-28 | Revue 2026‑11‑05 §4.1, §4.2, §4.4, §5 P1‑1…P1‑9 |
| P2-10…P2-15 | Revue 2026‑11‑05 §5 P2‑1…P2‑6 |
| P3 | §15.3, §6 |

---

## 9. Checklist de revue (à coller dans chaque PR)

- [ ] `require_auth` / `require_admin` + tests `401`/`403`.
- [ ] `tokio::time::timeout` sur chaque commande, constante nommée.
- [ ] `Command::new(...).args([...])`, aucun shell, aucun `unwrap` sur une
      entrée utilisateur.
- [ ] Capability absente → `503` nommé ; non supporté → `501` nommé.
- [ ] Migration SQL idempotente et réversible (ou documentée).
- [ ] i18n : aucune chaîne en dur ; `aria-label` sur les icônes ; 44 px ;
      focus visible ; un `<h1>`.
- [ ] Tests unit + intégration + E2E ; `TESTING.md` à jour.
- [ ] Test de **corruption réelle** pour toute promesse de checksum/intégrité.
- [ ] Écriture de configuration atomique + validation native + `.bak`.
- [ ] Opération lourde planifiable, reprenable et observable.
- [ ] `docs/guides/<area>.md`, `docs/API.md`, `docs/INDEX.md`, `SCREENS.md`.
- [ ] `FeatureDoc` RAG + ligne dans `docs/guides/rag.md`.
- [ ] Aucun secret loggé ; aucun fichier de travail ajouté au dépôt.

---

## 10. Notes

- Les estimations supposent un seul développeur, tests et docs inclus.
- Les items P1-12/P1-13 touchent le **layout des pools** : les livrer derrière
  un drapeau et avec une migration explicite, jamais en silence.
- Les items P2-4/P2-5 exigent une **note de conception** (coût RAM, menace,
  retour arrière) avant toute ligne de code.
- Ce backlog est volontairement **plus court** que la liste DSM : la parité
  n'est pas l'objectif, la simplicité et la sûreté le sont.
- Les **P0-5 à P0-12** sont des corrections de promesses déjà visibles dans
  l'UI (vérification USB, intégrité, durabilité). **Le cœur du lot P0 est
  livré le 2026-11-05** : P0-5 à P0-12, y compris la reprise exacte d'un scrub
  et la protection volume plein iSCSI ; **aucune réserve P0** (`dm-flakey`
  de P0-7, `x-systemd.*`, température sur 3 échantillons, backoff et
  notification `backup.jobs` sont livrés). Après la **revue de valeur du
  2026‑11‑06**, les prochains chantiers ouverts sont **P2-7 (enforce +
  terrain), P2-8 (UI), P1-18 (filtres UI), P2-9 (test VM), P1-16**, puis les finitions
  P1-27/P1-28 ; les autres items sont gelés ou rejetés.
- **Tests** : `cargo check --lib`, `cargo test --lib --no-run` et les
  `--no-run` de `replication_integration`, `iscsi_integration`,
  `storage_integration`, `raid_integration`, `rclone_integration`,
  `health_integration`, `cache_integration`, `explorer_integration`,
  `setup_integration` et `scheduled_tasks_integration` compilent (avertissements
  préexistants uniquement) ; **les suites n'ont pas été exécutées** (consigne
  du mainteneur). `npm run check` n'a pas pu tourner dans le worktree
  (dépendances frontend absentes).
- **Ne pas copier les compromis DSM** : `^metadata_csum` sur ext4,
  `enable_periodic_fsck = 0`, MD5 comme checksum utilisateur, USB Copy sans
  vérification, NFSv4 désactivé par défaut, SHA‑1 dans les manifestes,
  signature/chiffrement SMB « Client defined » par défaut.
- **Ne pas doubler la surveillance** : `smartd`/`mdmonitor` *ou* les jobs
  HomeVault (P0-8/P1-21), pas les deux ; sinon alertes en double et
  consommation inutile.
- Les tests d'intégrité doivent utiliser des **corruptions réelles** (octet
  modifié, pack tronqué, superbloc abîmé, `kill -9` pendant `rename`) ; un
  mock qui renvoie `false` ne prouve pas la promesse.
