# Comparatif HomeVault ↔ DSM 7.4.1 — apps et améliorations

> **Document de comparaison** — racine du dépôt, à la demande explicite du
> mainteneur. Il ne remplace ni `CLAUDE.md` ni `docs/`, qui restent la source de
> vérité. Il complète `ANALYSE-DSM-vs-HOMEVAULT.md` en se concentrant sur le
> **comparatif par application** et sur les **améliorations à mener**.
>
> **Règle de lecture (2026‑11‑08).** Les sections §1–§2 décrivent ce que DSM
> possède — c'est un **constat**, pas une feuille de route. La **décision**
> (garder / recadrer / geler / rejeter) est en §3, et le cadre d'évaluation en
> §6. **§6 fait foi** en cas d'ambiguïté, et la revue de valeur du backlog
> (`HOMEVAULT_TASKLIST_A_FAIRE.md` §1bis) reste la référence opérationnelle.

| Champ | Valeur |
|---|---|
| Baseline HomeVault | `910a84f`, 09/11/2026 — plus les correctifs de vérification (§8) et le lot intégrité (§9) |
| Firmware DSM analysé | 7.4.1‑90080 (DS925+), image `/media/nicolas/500GB/Projets/DSM7/DSM_DS925+_90080` |
| Périmètre | 30 apps intégrées du launcher + 16 apps du catalogue Docker |
| Méthode | Vérification du code (`src/`, `frontend/`, `catalog/`), des guides `docs/guides/` et du backlog `HOMEVAULT_TASKLIST_A_FAIRE.md` |
| Légende | ⭐ HomeVault au‑delà · ✅ parité · 🟡 partiel · ❌ absent |
| Revue de valeur | 2026‑11‑08 — filtre §6, aligné sur le backlog §1bis |

---

## 0. Mise à jour par rapport à `ANALYSE-DSM-vs-HOMEVAULT.md`

L'analyse du 04/11/2026 (baseline `c37c803`) est en partie périmée. Ces manques
qu'elle listait sont **désormais livrés** et ne doivent plus être comptés :

| Ancien « manque » | État réel au HEAD `910a84f` (plus §8) |
|---|---|
| Routes statiques, proxy sortant, client WireGuard | ✅ livrés (P1‑7, P1‑8, P1‑17) |
| GeoIP, rétention métriques, expiration mot de passe | ✅ livrés (P1‑14, P1‑3, P1‑2) |
| Migration de niveau RAID, `restic mount` | ✅ livrés (P1‑11, P1‑10) |
| Snapshots par partage, rollback Btrfs, snapshots immuables | ✅ livrés (P1‑12, P1‑13, P2‑5) |
| SMB multicanal, AD/Kerberos (config + join), NFS `sec=krb5` | ✅ livrés (P2 partiel assumé) |
| ACL par partage, quotas par partage, Previous Versions, Time Machine, serveur rsync | ✅ livrés |
| OpenAPI, DDNS 7 fournisseurs, SNMP v3 | ✅ livrés |
| Table d'audit | ✅ **livrée le 2026‑11‑08, filtres structurés UI le 2026‑11‑09** (P1‑18 : table, API, export, source Journal, filtres gravité/utilisateur/action/dates, pagination, rétention 90 j) |
| Signature + atomicité de mise à jour | ✅ **installeur 2026‑11‑08 + Apply update dans Settings 2026‑11‑09** (P2‑8 ; reste la clé de production) |
| Signature des stores communautaires | ✅ **livrée le 2026‑11‑09** (P1‑27 : Ed25519 détachée sur le descripteur et chaque manifeste d’app, vérifiée au sync et au chargement, badge Signed/Unsigned/refused) |
| Stabilité système / monitoring hôte | ✅ **livrée le 2026‑11‑09** (P1‑28 : `sysctl` noyau + cache d’écriture borné, `smartd`/`mdmonitor` opt‑in pour ne jamais doubler les alertes de HomeVault) |
| OAuth2 e‑mail (Gmail / Microsoft 365) | ✅ **livré le 2026‑11‑09** (P1‑16 : flux code, refresh token scellé, msmtp `oauthbearer`/`xoauth2`, carte Settings — aucun mot de passe applicatif) |
| Isolation d'exécution | 🟡 **installeur livré le 2026‑11‑08** (P2‑7 partiel : systemd durci + AppArmor opt‑in ; validation terrain/enforce restante) |
| Restauration bare‑metal | 🟡 **drill non destructif livré le 2026‑11‑08** (P2‑9 partiel : snapshot DB + archive config + état backup vérifiés ; test de reconstruction VM restant) |
| Scrub planifié, reprise après arrêt sale, récupération de montage/fsck, historique SMART/I‑O | ✅ **mergés dans `main` le 2026‑11‑09** (lot intégrité, §9 ; tests à exécuter) |

**Ce que la revue du 2026‑11‑08 ajoute.** Le diagnostic de ce document reste
juste : les vrais manques sont la **confiance** (mise à jour, isolation, audit,
restauration), pas les fonctionnalités. En revanche, plusieurs écarts DSM listés
en §1–§2 ne sont **pas** des objectifs pour HomeVault : ce sont des parités
d'entreprise, de niche ou de dette. Le §3 révisé les classe explicitement, et le
§6 fournit le filtre qui doit s'appliquer à **toute future implémentation**. En
cas de désaccord avec le backlog, `HOMEVAULT_TASKLIST_A_FAIRE.md` §1bis gagne.

---

## 1. Apps intégrées — comparatif

> **Constat, pas engagement.** La colonne « Ce qu'il faut améliorer » liste les
> écarts fonctionnels constatés face à DSM. Toute décision d'implémentation est
> prise en §3 / §6 — une ligne présente ici n'est pas, à elle seule, une raison
> de coder.

### 1.1 Stockage et fichiers

| App HomeVault | Équivalent DSM 7.4 | Verdict | Ce qu'il faut améliorer |
|---|---|---|---|
| **Storage** | Storage Manager | 🟡 | **Cache SSD ZFS L2ARC/SLOG livré** (P2‑16, opt‑in, SSD obligatoire) ; restent SHR/SHR2, thin provisioning complet (seuls les zvol ZFS sont couverts), déduplication (design seulement), KMIP, WORM, extension DX525, mise à jour firmware disques |
| **RAID** | Storage Manager (création de volume) / Storage Advisor | 🟡 | SHR/SHR2, ajout automatique de spare, extension à chaud, RAID F1, vitesse et moniteur de resync (migration de niveau déjà faite) |
| **Explorer** | File Station | ⭐ / ✅ | **Emplacements réseau CIFS/NFS montés et parcourus** et **versions Btrfs dans Rewind** livrés (P1‑35) ; cloud via rclone ; restent les vignettes vidéo/PDF (P2 opt‑in), les ACL Windows et le transcodage |
| **Shares** | Control Panel > Dossier partagé + Services de fichiers | 🟡 | ACL Windows non simulées (POSIX uniquement) ; validation réelle DC/keytab AD ; NFSv4 Kerberos à valider sur matériel. **WS‑Discovery livré** (P1‑29) ; SMB multicanal, chiffrement, Previous Versions, Time Machine, rsync, FTP jails : **faits** |
| **Quotas** | Control Panel > Quota | ✅ / ⭐ | Parité solide (utilisateur, partage, transfert) **+ historique horaire et alerte à 90 %** (P1‑32, rétention 90 j) |
| **USB Copy** | USB Copy | ✅ | Ajouter déclenchement physique one‑touch et règles multiples si le matériel le permet |
| **iSCSI** | SAN Manager | ✅ / 🟡 | **LUN fins ZFS avec snapshots et clones livrés** (P2‑17) ; restent Fibre Channel, multipath, VAAI. CHAP et apply‑now sont faits |
| **Backup** | Hyper Backup | 🟡 | Pas d'image **bare‑metal/block‑level**, pas d'agent ABB (Windows/Linux/M365), pas de C2. Restic, dédup, chiffrement, rétention, `mount`, vérification : faits |
| **Backup destination** | Hyper Backup Vault | ✅ | Ajouter multi‑protocole (rsync/WebDAV) et limite de bande passante par tenant |
| **Replication** | Snapshot Replication | ✅ / 🟡 | Basculement/reprise, granularité par partage (faite), destinations multiples, planification de bande passante |
| **Rescue** | Storage Manager (réparation) / Synology Assistant | ⭐ | Ajouter un parcours guidé « réparer un pool » au‑dessus de ddrescue/TestDisk/mdadm |

### 1.2 Système, réseau, identité

| App HomeVault | Équivalent DSM 7.4 | Verdict | Ce qu'il faut améliorer |
|---|---|---|---|
| **Dashboard** | Bureau DSM / Info Center | ✅ | Widget sauvegarde/scrub/UPS et alerte unifiée ; le reste est au niveau |
| **Network** | Control Panel > Réseau | 🟡 | DHCPv6‑PD/RA, OVS, modes de bond (2 seulement), PPPoE (rejeté volontairement). Routes statiques, proxy, diagnostic : faits |
| **VPN** | VPN Server + VPN Client | ✅ / ⭐ | WireGuard serveur + client, **QR code et configuration 1‑clic**, **endpoint résolu manuel → DDNS → IP publique avec avertissement LAN/CGNAT** (P1‑33). Serveur OpenVPN/L2TP volontairement absents |
| **Traffic control** | Control Panel > Trafic | 🟡 | Classification par app/protocole, DSCP, shaping entrant (aujourd'hui egress seulement) |
| **Docker** | Container Manager | ⭐ / ✅ | Mises à jour planifiées/auto, graphiques ressources, compose (fait), registries privés (fait), exec/logs (fait) |
| **Virtual Machines** | Virtual Machine Manager | ✅ / 🟡 | **Export/import d'archive (OVA) livré** (P2‑18) ; restent migration à chaud, HA, switches virtuels, passthrough USB/PCI. Clone, snapshots, VNC, cloud‑init : faits |
| **Scheduled Tasks** | Task Scheduler | 🟡 | **5 types** (`command`, `snapshot`, `smart_test`, `power`, `service`) + **5 services** (`docker_update`, `trash_purge`, `db_backup`, `notification`, `downloader_rss`). Restent : rsync, historique consolidé ; scrub et restic gardent leur ordonnanceur dédié (décision P1‑31) |
| **Settings** | Control Panel | 🟡 | **MAJ signée + atomique + rollback livrée** (P2‑8, déclenchable depuis l'écran) ; restent portail de connexion personnalisé, options régionales, périphériques externes, LED/bip/LCM |
| **Users** | Control Panel > Utilisateur & Groupe | ✅ | Groupes imbriqués, historique de mots de passe, verrouillage d'inactivité. Expiration, quotas, grants, managers délégués, sync Samba : faits |
| **Groups** | Control Panel > Utilisateur & Groupe | ✅ | Groupes imbriqués, quotas de groupe |
| **Directory sign-in** | Domaine/LDAP + SSO Server | 🟡 | CAS/Azure/Entra/WebSphere, **serveur LDAP/RADIUS**, fournisseur OAuth, SCIM. LDAP/AD/OIDC/SAML + mapping groupe→rôle : faits |
| **Account** | Paramètres personnels | ✅ | Pas de manque bloquant |
| **Terminal** | (pas d'équivalent web DSM) | ⭐ | Ajouter audit/enregistrement de session si usage administrateur intensif |

### 1.3 Supervision et productivité

| App HomeVault | Équivalent DSM 7.4 | Verdict | Ce qu'il faut améliorer |
|---|---|---|---|
| **Health** | Resource Monitor + Security Advisor + Active Insight | ✅ / 🟡 | Top processus, I/O disque, capteurs matériels, seuils/alertes. Security Advisor : au niveau |
| **Metrics** | Resource Monitor | ✅ | Par processus, I/O disque/volume, rapports planifiés. Rétention configurable : faite |
| **Journal** | Log Center | ✅ / ⭐ | **Table d'audit interrogeable livrée** (P1‑18 : filtres API, export CSV/JSON, source `audit`, **filtres structurés UI gravité/utilisateur/action/dates + pagination**, rétention 90 j) **+ Export CSV de la vue courante** (P1‑32) ; restent les alertes. Syslog distant TLS : fait |
| **Widgets / Edit Widgets** | Widgets DSM | ✅ | Widgets backup/UPS/interface réseau/croissance stockage |
| **Cloud Sync** | Cloud Sync | ✅ | Remotes chiffrés (rclone crypt), présets fournisseurs, limites de bande passante |
| **Syncthing** | Synology Drive | 🟡 / ✅ | **Navigateur de versions et restauration livrés** (P2‑19, politique du démon éditée depuis HomeVault) ; restent sync sélective/à la demande, dossiers d'équipe, apps mobiles, verrouillage de fichiers |
| **Downloader** | Download Station | ✅ | **RSS/Atom, planification, limites par tâche, extraction auto, ratio** livrés (P1‑36) ; HTTP/HTTPS/FTP/SFTP/magnet. **NZB et eMule restent rejetés** (§3.3) |
| **AI Assistant** | (pas d'équivalent dans l'image) | ⭐ | GPU, davantage de sources RAG. Déjà au‑delà |
| **PublicShare** | File Station (liens de partage) | ⭐ | Page de marque, statistiques de consultation. File requests et liens dossier : déjà au‑delà |

---

## 2. Catalogue d'apps vs paquets DSM

HomeVault propose **16 apps** ; DSM référence **63 paquets** (19 installés d'usine
dans l'image).

| Catégorie | HomeVault (catalogue) | Paquet DSM | Verdict | À améliorer |
|---|---|---|---|---|
| Photos/vidéos | **Immich** | Synology Photos | ✅ / ⭐ | **Binding sur un partage en lecture seule** et **transcodage matériel opt‑in** livrés (P1‑34) ; sauvegarde automatique des mobiles et reconnaissance ML à valider côté Immich |
| Vidéo | **Jellyfin** | Video Station | ✅ | **Transcodage HW via `/dev/dri`** livré, bibliothèque sur partage ; accès distant par reverse‑proxy + VPN ; **DLNA non validé** (exigerait `network_mode: host`) |
| Musique | **Navidrome** | Audio Station | ✅ | **Bibliothèque sur partage** livrée ; DLNA/AirPlay non validé |
| Docs/notes | **BookStack, Paperless‑ngx** | Note Station, Documents | 🟡 | OCR/scan, édition collaborative, import |
| Drive/sync | **ownCloud, Syncthing** | Synology Drive | 🟡 | Sync sélective, versions, apps mobiles, édition en ligne |
| Git/dev | **Gitea** | Git Server | ✅ | SSO, sauvegarde |
| Monitoring | **Grafana, Uptime Kuma** | Resource Monitor / Active Insight | ⭐ | Intégration des métriques HomeVault |
| Automatisation/IoT | **n8n, Home Assistant** | (aucun équivalent) | ⭐ | Connecteurs HomeVault |
| Mots de passe | **Vaultwarden** | Secure SignIn | ⭐ | SSO/passkeys |
| Tâches/RSS/DNS/agenda | **Vikunja, FreshRSS, Pi‑hole, Baïkal** | Calendar / Contacts / DNS Server | ✅ | **Calendrier et contacts livrés via Baïkal** (CalDAV + CardDAV, P2‑20) |
| **Absents** | — | MailPlus Server, Synology Office, Chat (équipe), Surveillance Station, Web Station, Directory Server, RADIUS Server, C2/Hybrid Share, ABB Agent, CMS, Migration Assistant, DLNA | ❌ | Décider pour chaque : app catalogue vs développement natif vs non‑objectif assumé (Calendar/Contacts sont désormais couverts par le catalogue) |

> Le « Chat » de HomeVault est un **assistant IA**, pas la messagerie d'équipe de
> DSM. Ne pas confondre les deux dans un comparatif.

> **Décision par défaut sur les « absents » : catalogue, pas natif.** Le backlog
> §6 acte déjà le rejet des photothèques, mail, calendrier, NVR et PHP **en
> natif**, assumés via le catalogue Docker. Une app manquante se traite donc en
> priorité par une entrée de catalogue (ex. calendrier → Radicale/Baïkal), pas
> par un nouveau sous-système dans le binaire. Les exceptions sont listées en §3
> et §6.

---

## 3. Tableau priorisé — décisions

Chaque ligne est passée au filtre §6. La priorité mesure la **valeur pour la
cible** (NAS domestique, 1–3 utilisateurs), pas la présence du bouton chez DSM.
Référence opérationnelle : `HOMEVAULT_TASKLIST_A_FAIRE.md` §1bis.

### 3.1 À garder — vraie avancée (confiance d'abord)

| Priorité | Chantier | Apps | Valeur réelle | Garde‑fou | Effort | Réf. |
|---|---|---|---|---|---|---|
| **P0** | Mise à jour **atomique + rollback**, puis **signée** | Settings / installeur | Seul maillon de confiance faible : pas de retour arrière aujourd'hui | Atomicité d'abord ; la signature dépend de l'infra de release | L/XL | P2‑8 |
| **P0** | **Isolation d'exécution** (systemd durci, AppArmor) | Backend / installeur | Défense en profondeur ; l'app utilise `sudo` | Mesurer, ne rien casser côté `sudo`/Samba | L | P2‑7 |
| **P0** | **Table d'audit interrogeable** | Journal | Accountability RGPD + enquête après incident | Admin‑only, rétention bornée, **jamais de contenu de fichier** | M | P1‑18 |
| **P0** | **Restauration bare‑metal testée** | Backup / DR | Reprise après sinistre réellement prouvée | Runbook + VM d'abord, pas d'usine à images | L | P2‑9 |
| **P1** | **SMB WS‑Discovery** | Shares | Découverte Windows sans configuration | Protocole de découverte, aucune donnée exposée | S | nouveau |
| **P1** | **MAJ Docker planifiées/auto** | Docker | Hygiène de patch = sécurité | Opt‑in, jamais de redémarrage surprise | S/M | nouveau |
| **P1** | **Historique/alertes quotas + filtres/export Journal** | Quotas, Journal | Visibilité sans surveillance | — | S | nouveau |
| **P1** | **WireGuard UX** (QR, 1‑clic, DDNS) | VPN | L'accès distant est le vrai besoin domestique | Remplacer « serveur OpenVPN », pas l'ajouter | S/M | nouveau |
| **P1** | **Unifier les tâches planifiées** sur les ordonnanceurs existants | Scheduled Tasks | 5 types exposés contre 13 boucles déjà présentes | **Un seul moteur** : exposer/rediriger, ne pas dupliquer | S/M | P1‑31 |

**État au 2026‑11‑08.** P2‑8 est livré côté **installeur** : manifeste signé
vérifié avant téléchargement, application atomique, `homevault.previous`
conservé, rollback automatique si le service ne devient pas prêt, résultat
exposé dans Settings (`last_update`). Le **déclenchement depuis l'UI** est
livré le 2026‑11‑09 (canal signé + **Apply update** + unité `homevault-update`,
voir plus bas) ; reste la publication de la clé publique de production.

P2‑7 est livré côté **installeur** : unité systemd durcie et mesurable
(`ProtectSystem`, `ReadOnlyPaths`, `PrivateTmp`, `RestrictSUIDSGID`,
`LockPersonality`, `RestrictRealtime`, `ProtectKernelLogs`, et
`NoNewPrivileges` en mode root), directives qui casseraient une fonction
volontairement absentes et documentées, profil AppArmor **opt‑in** en mode
complain (enforce après validation terrain), score `systemd-analyze security`
dans le journal d'installation. Restent la validation terrain et l'enforce
AppArmor.

P1‑18 est **livré** : table `audit_log` (migration 086), écriture non
bloquante depuis `events::log_event`, champs structurés pour
l'authentification (acteur/action/IP), API admin `GET /api/audit` +
`/api/audit/export` (CSV/JSON), source `audit` dans le Journal et purge à
90 jours. Les **filtres structurés de l'UI Journal sont livrés le 2026‑11‑09**
(gravité/utilisateur/action/dates, pagination, export ; voir plus bas).

**P1‑16 — OAuth2 e‑mail (livré le 2026‑11‑09).** Le relais SMTP peut
s'authentifier auprès de Gmail et de Microsoft 365 **sans mot de passe
applicatif**. *L’URL de redirection refuse désormais tout espace interne
(2026‑11‑09, §8).*

- `src/email_oauth.rs` : flux code d'autorisation, URL par fournisseur
  (Google : `access_type=offline` + `prompt=consent`, scope
  `https://mail.google.com/` ; Microsoft : `offline_access` + `SMTP.Send`),
  échange et renouvellement du jeton, cache mémoire (marge 60 s, délai 20 s,
  redirections désactivées).
- **Secrets scellés** (`hv1:`) dans `data/email_oauth.json` : client secret et
  refresh token. Aucun endpoint ne les renvoie, aucune ligne de journal ne les
  contient ; l'audit ne note que connected/disconnected/registration saved.
- **Callback public mais lié au navigateur** : cookie d'état `HttpOnly`,
  `SameSite=Lax`, usage unique, 10 minutes — le modèle du login OIDC. Un état
  absent, différent du cookie ou rejoué est refusé avant tout appel réseau.
- **Envoi** : le jeton d'accès (jamais le refresh token) est écrit dans
  `msmtp.conf` (0600) avec `auth oauthbearer` (Google) ou `auth xoauth2`
  (Microsoft). Un refresh token tourné est stocké avant de continuer ; un
  échec (`invalid_grant`, consentement révoqué) **nomme la raison** au lieu de
  retomber silencieusement sur le mot de passe.
- **UI** : carte *OAuth2 (Gmail / Microsoft 365)* sous Notifications —
  fournisseur, client ID, secret (jamais réaffiché, vide = conserver), URL de
  redirection exacte, Connecter/Reconnecter/Déconnecter, état
  Connected/Ready/Not configured, date de connexion, dernière erreur, lien de
  réautorisation. **Déconnecter** supprime le fichier et le cache.
- Tests écrits mais **non exécutés** : 11 unitaires (`email_oauth`), 3
  unitaires (`email.rs`), 3 unitaires (page de callback), 6 d'intégration
  (aucun fournisseur contacté), E2E Settings §31 (6). Docs :
  `docs/guides/email-oauth.md` (nouveau), `docs/INDEX.md`,
  `docs/guides/settings.md`, `docs/API.md`, `SCREENS.md`, FeatureDoc
  `settings`.

P2‑9 est **livré côté drill** : `src/restore_drill.rs` vérifie le dernier
snapshot DB (copie temporaire, ouverture en lecture seule,
`PRAGMA integrity_check`, comptage tables/utilisateurs), décode la dernière
archive de configuration (checksums, passphrase si chiffrée) et rapporte le
dernier job restic. Verdict *Ready to restore* seulement si les deux moitiés
sont vérifiées ; API `GET/POST /api/settings/backup/restore-drill`, bouton
**Verify restore** dans Settings, rapport persisté. Reste le test de
reconstruction complète sur VM.

### 3.2 À recadrer — bon objectif, mauvais périmètre

| Priorité | Proposition initiale | Recadrage retenu | Pourquoi | Effort | Réf. |
|---|---|---|---|---|---|
| **P1 ✅** | « Média first‑class » (apps natives) | **Intégration média** : installation 1‑clic + **binding sur un partage** + backup/rétention + reverse‑proxy + accès distant + détection transcodage HW — **partage + transcodage livrés** | Le backlog §6 rejette le natif ; l'usage n°1 d'un NAS reste à couvrir par le catalogue | M | nouveau |
| **P1 ✅** | Explorer : CIFS/NFS/cloud, vignettes vidéo/PDF, versions Btrfs, favoris | **CIFS/NFS + favoris + versions Btrfs** : **livrés** (P1‑35) ; cloud via rclone existant ; **vignettes vidéo/PDF en P2 opt‑in** (ffmpeg/poppler = surface + CPU) | Dernier vrai écart avec File Station | M | nouveau |
| **P1 ✅** | Téléchargeur : NZB, RSS, planification, limites, extraction | **RSS + planification + limites/tâche + extraction auto** : **livrés** (P1‑36) ; **NZB/eMule rejetés** | Niche, credentials, exposition légale | M | nouveau |
| **P2 ✅** | Cache SSD L2ARC/SLOG | **Opt‑in ZFS**, jamais par défaut (exige un SSD) : **livré** (P2‑16) | Gain réel seulement sur matériel adapté | M | nouveau |
| **P2 ✅** | iSCSI snapshots/clones de LUN, thin LUN | **P2 home‑lab** : **livré** (P2‑17) — LUN fin ZFS + snapshots/clones | Usage domestique marginal | M | nouveau |
| **P2 ✅** | VM export/import OVA | **P2** en s'appuyant sur `incus export` natif : **livré** (P2‑18) | Portabilité/backup, pas un sous‑système | M | nouveau |
| **P2 ✅** | Syncthing navigateur de versions | **P2** intégration UI seule (Syncthing gère déjà les versions) : **livré** (P2‑19) | Faible effort, pas de nouveau moteur | S/M | nouveau |
| **P2 ✅** | Calendrier/Contacts manquants | **App catalogue** (Radicale/Baïkal), pas natif : **livré** (P2‑20, Baïkal) | Cohérent avec le backlog §6 | S | nouveau |

**P1‑34 — Intégration média (livrée).** *Corrigé le 2026‑11‑09 (§8) : la
validation refusait tout chemin absolu et le bloc GPU était injecté sans
indentation ; les deux défauts sont fermés et couverts par les tests unitaires
existants.*

- **Le manifeste décide** : Jellyfin (`share_mount = "/media"`), Immich
  (`/mnt/media`) et Navidrome (`/music`) montent `{{SHARE_DIR}}:<montage>:ro`.
  La valeur est validée au parse (absolue, sans `..`, `:` ni espace), et un app
  qui ne la déclare pas refuse tout partage (`400`) — le chemin conteneur
  appartient au paquet, jamais à la requête.
- **L'admin choisit un partage configuré**, jamais un chemin :
  `media_share: {name, share_type}` est résolu par `SharesService::get_share`
  puis monté **en lecture seule** ; le chemin n'entre pas dans
  `data_volume_paths`, donc l'installeur ne crée ni ne `chown` un partage
  utilisateur. Sans choix, l'app reçoit son propre dossier
  (`apps/<clé>/<montage>`) et démarre.
- **Transcodage matériel** opt‑in à trois conditions (`gpu = true` au
  manifeste, `gpu: true` à l'install, `/dev/dri` présent) ; sinon le bloc
  `devices:` disparaît du compose et le bouton est désactivé avec la raison.
- **Les mises à jour conservent les choix** (`.install.toml` :
  `environment`, partage, `gpu`) — corrige au passage la perte des réglages
  `environment` lors d'un update.
- Tests **écrits, non exécutés** : 10 unitaires, 3 d'intégration, E2E Docker
  15.1/15.3b. Docs : `docs/guides/docker.md`, `docs/API.md`, `SCREENS.md`,
  FeatureDoc `docker`.

**P1‑35 — Explorer : emplacements réseau CIFS/NFS + versions Btrfs (livré).**

- **Emplacements réseau** : un partage d'un autre NAS se monte sous
  `/mnt/homevault-remote/<nom>` (slug validé ; le point de montage ne vient
  jamais de la requête) et se parcourt comme un dossier. Le serveur construit
  l'argv de `mount` depuis `data/remote_mounts.json` ; le mot de passe CIFS
  n'existe que dans un fichier **0600** passé en `credentials=<chemin>`
  (jamais argv, log ou réponse). Options en liste blanche par protocole,
  `ro`/`rw` = champ dédié (lecture seule par défaut), délais bornés + stderr de
  l'outil, `503` nommant `mount.cifs`/`mount.nfs` s'il manque. Remontage au
  démarrage des emplacements `enabled`.
- **Versions Btrfs** : Rewind lit `.snapshots/<partage>/<nom>/…` quand le
  partage vit sur Btrfs (résolution partage → pool, frontières de segments
  respectées), uniquement si ZFS n'a rien trouvé.
- Tests **écrits, non exécutés** : 12 unitaires, 7 d'intégration (**aucun
  montage exécuté**), E2E Explorer §19. Docs :
  `docs/guides/remote-mounts.md`, `docs/guides/explorer.md`, `docs/API.md`,
  `SCREENS.md`, FeatureDocs `explorer`/`snapshots`.

**P1‑36 — Téléchargeur : RSS, planification, limites par tâche, extraction auto
(livré).** *La taille d’une enclosure Atom est désormais lue (2026‑11‑09, §8).*

- `src/downloader_rss.rs` : parseur RSS 2.0/Atom pur (seuls les éléments avec
  une URL téléchargeable sont retenus), marqueur `seen` borné (500) par flux,
  refus d'une adresse loopback/RFC 1918 (sinon le refresh serait une primitive
  SSRF), fichier en 0600.
- `src/downloader_extract.rs` : réglage `auto_extract` (**off** par défaut),
  allowlist `.zip`/`.tar.*` alignée sur `tar`/`unzip`, argv sans shell,
  extraction **dans le dossier du téléchargement**, archive conservée, ids
  traités bornés (200).
- `POST /api/downloader/add` gagne `max_download_limit`, `max_upload_limit` et
  `seed_ratio` (validés avant tout appel aria2).
- Planification : `service = downloader_rss` dans le moteur générique (P1‑31),
  qui appelle `refresh_all_feeds` — **un seul ordonnanceur**.
- Tests **écrits, non exécutés** : 14 unitaires, 4 d'intégration (aucun démon
  lancé, aucun flux récupéré), E2E Téléchargeur §15. Docs :
  `docs/guides/downloader.md`, `docs/API.md`, `docs/guides/scheduled-tasks.md`,
  `SCREENS.md`, `PRIVACY.md` (table sortante + RoPA 19), FeatureDocs
  `downloader`/`scheduled-tasks`.

**P2‑16 — Cache SSD ZFS L2ARC/SLOG (livré).**

- `src/zfs_cache.rs` : parseur **pur** de `zpool status -P` (sections
  `logs`/`cache`, lignes d'appareil, compteurs d'erreurs, log miroir),
  validation du chemin et argv `zpool add`/`remove`.
- `src/handlers/zfs_cache.rs` : `GET/POST /api/zfs-cache`,
  `POST /api/zfs-cache/remove` — admin, journalisés. Refus nommés : pool
  inconnu ou non‑ZFS, disque absent de l'inventaire ou déjà utilisé, second
  groupe `logs`/`cache`, et **disque mécanique** (un L2ARC sur HDD ralentit le
  pool, un SLOG n'apporte rien). Le détachement refuse tout appareil qui n'est
  pas un vdev cache/log de ce pool — jamais un disque de données.
- Sudoers : deux sous‑commandes de plus (`zpool add`, `zpool remove`).
- UI : carte dédiée dans l'onglet Cache, distincte du mover ; disque mécanique
  listé mais désactivé avec la raison.
- Tests **écrits, non exécutés** : 8 unitaires, 3 d'intégration (**aucune
  commande `zpool` exécutée**), E2E Storage §14. Docs :
  `docs/guides/cache.md`, `docs/API.md`, `SCREENS.md`, FeatureDoc `cache`.

**P2‑17 — iSCSI : LUN fins ZFS + snapshots/clones (livré).**

- `src/zvol.rs` : dataset `<pool>/homevault-iscsi/<nom>`, appareil
  `/dev/zvol/…` dérivé, `zfs create -p -s -V`, `snapshot`, `clone`,
  `destroy [-r]`, parseurs `zfs list` — purs et testés.
- `kind: "zvol"` sur la création d'un backstore (`{pool, size_bytes, sparse?}`) ;
  un pool non‑ZFS est refusé avec la raison.
- `GET/POST …/snapshots`, `DELETE …/snapshots/{snap}`, `POST …/clone` : le clone
  devient un nouveau backstore exportable en LUN (blocs partagés jusqu'à
  divergence). Ces routes ne touchent **que** les volumes de HomeVault.
- Un zvol n'est pas convertible en place (kind/nom/chemin figés) ; la
  suppression fait `zfs destroy -r` après saisie du nom.
- Sudoers : `zfs create *` et `zfs clone *` en plus.
- Tests **écrits, non exécutés** : 6 unitaires, 4 d'intégration (**aucune
  commande `zfs` exécutée**), E2E iSCSI §7. Docs : `docs/guides/iscsi.md`,
  `SCREENS.md`, FeatureDoc `iscsi`.

**P2‑18 — VM : export/import d'archive (livré).**

- `src/vm_export.rs` : nom d'archive **dérivé** (nom de machine validé +
  horodatage) et re‑validé à chaque usage, argv
  `incus export [--compressed] … <cible>` / `incus import <source> [<nom>]`,
  listing borné.
- Endpoints admin : `POST /api/vm/{name}/export`, `GET /api/vm/exports`,
  `POST /api/vm/import`, `GET /api/vm/exports/{file}` (flux),
  `DELETE /api/vm/exports/{file}` ; jobs suivis `vm_export`/`vm_import`.
- Les grants sudoers **épinglent le chemin** de l'archive au dossier
  `<DATA>/vm/exports` : même un argv compromis ne peut lire/écrire ailleurs.
- UI : dialogue **Exporter / importer** ; sans Incus, les actions sont
  désactivées avec la raison et la liste reste lisible.
- Tests **écrits, non exécutés** : 6 unitaires, 3 d'intégration (**aucune
  commande `incus` exécutée**), E2E VM §11. Docs : `docs/guides/vm.md`,
  `SCREENS.md`, FeatureDoc `virtual-machines`.

**P2‑19 — Syncthing : navigateur de versions (livré).**

- `src/syncthing_versions.rs` : politique de versionnage (off / corbeille /
  N versions / âge max), `params` en chaînes (`maxAge` en secondes), relecture
  tolérante, bornes, parsing de la liste et validation d'une restauration.
- `GET /api/syncthing/folders/{id}/versions`, `PUT …/versioning` (read‑modify‑
  write du config du dossier) et `POST …/versions/restore` — admin, journalisés.
- Une restauration est refusée quand le versionnage est désactivé ; un nom qui
  sort du dossier n'atteint jamais le démon.
- UI : dialogue **Versions** dans le détail d'un dossier.
- Tests **écrits, non exécutés** : 6 unitaires, 3 d'intégration (**démon jamais
  contacté**), E2E Syncthing §15. Docs : `docs/guides/syncthing.md`,
  `SCREENS.md`, FeatureDoc `syncthing`.

**P2‑20 — Calendrier/Contacts en app catalogue (livré).**

- **Baïkal** rejoint le catalogue (16 apps) : un seul conteneur (nginx + PHP)
  qui sert **CalDAV** et **CardDAV**, avec l'interface d'administration qui
  crée les utilisateurs, calendriers et carnets.
- Deux volumes séparés : `config` (réglages, mot de passe admin) et `data`
  (calendriers/contacts) — une réinitialisation de config ne touche jamais les
  données.
- **Pas de sous‑système natif** : c'est la décision §6 (le natif est réservé à
  ce qui doit partager l'identité, les partages, la sauvegarde ou la
  supervision). Les clients (iOS, Android, Thunderbird, DAVx⁵) se connectent
  directement au conteneur, derrière le reverse‑proxy et le VPN existants.
- Tests : les tests de catalogue existants couvrent le nouveau paquet
  (manifeste + `[security]` + rendu compose) ; le compte intégré passe de 15 à
  16 dans les tests et la doc.

### 3.3 Geler ou rejeter — parité sans valeur pour la cible

| Proposition | Décision | Raison |
|---|---|---|
| **Validation DC/Kerberos + ACL Windows en P0** | **Geler** | Cible entreprise non confirmée ; déjà gelé au §1bis |
| **Serveur OpenVPN + L2TP** | **Rejeter** | WireGuard serveur couvre le besoin, en plus simple/rapide ; L2TP = legacy |
| **NZB / eMule** | **Rejeter** | Niche, credentials, exposition légale |
| **SHR / SHR2** | **Rejeter** | Abstraction propriétaire ; ZFS raidz + mdadm suffisent |
| **Déduplication / thin overcommit** | **Geler** | Saturation imprévisible ; déjà tranché |
| **KMIP / WORM** | **Rejeter** | Dépendance réseau au boot / verrouillage admin ; déjà tranché |
| **Suite productivité native** (mail, office, NVR, chat d'équipe, Web Station) | **Rejeter en natif** | Contredit le backlog §6 ; surface d'attaque énorme → catalogue |
| **Identité d'entreprise** (RADIUS/LDAP server, CAS/Azure/Entra, SCIM) | **Geler** | Hors cible ; le client OIDC/SAML suffit |
| **Apps mobiles natives** | **Rejeter** | Contredit « rien à installer » → **PWA** |
| **21 locales** | **Rejeter** | Dette de maintenance (déjà backlog §6) |
| **HA cluster / IDS‑IPS** | **Rejeter** | Déjà backlog §6 |

### 3.4 Ce qu'il ne faut pas copier de DSM

QuickConnect, Active Insight/SynoAnalytics, le canal gRPC de SupportService
(actions distantes), ExtJS 3.4, la prolifération de paquets, le verrouillage
C2/Hybrid Share, le noyau 4.4 patché, PostgreSQL 11/OpenSSL 1.1.1/Samba 4.15/
PHP 8.1/Redis 4.0.

---

## 4. Verdict global

1. **HomeVault n'est plus « en retard fonctionnel »** sur l'essentiel d'un NAS
   domestique : fichiers, partages, snapshots, sauvegarde, réplication, réseau,
   DNS/DHCP, VPN, Docker, VM, supervision, sécurité produit. Sur plusieurs axes
   il est **au‑delà** (passkeys, snapshots immuables, anti‑ransomware, file
   requests, RAG/MCP, accessibilité, sobriété).
2. **Les vrais écarts restants sont peu nombreux mais structurants** : chaîne de
   mise à jour (**livrée**, reste la clé de production), isolation d'exécution
   (livrée, enforcement AppArmor à valider), audit requêtable (**livré**, filtres
   UI compris), restauration bare‑metal (drill livré, test VM restant) — c'est‑à‑dire la **confiance**
   plus que les fonctionnalités.
3. **Le plus gros écart « visible pour l'utilisateur » — le média — est désormais
   comblé côté intégration** : les apps du catalogue s'installent en 1‑clic,
   sont **liées à un partage en lecture seule** et **accélérées par le GPU**
   quand la machine en a un (P1‑34), derrière le reverse‑proxy et le VPN.
   Restent la rétention/sauvegarde média (le partage est couvert par Restic) et
   la validation DLNA/reconnaissance ML — pas des apps natives, conformément au
   §3.2.
3bis. **Le dernier écart de fichier est comblé** : l'Explorer monte des
   partages CIFS/NFS d'un autre serveur (emplacements réseau, lecture seule par
   défaut, mot de passe en fichier 0600) et affiche les versions Btrfs dans
   Rewind, comme ZFS (P1‑35). Restent les vignettes vidéo/PDF (P2 opt‑in) et les
   ACL Windows.
4. **Les écarts entreprise et de niche sont gelés ou rejetés**, pas reportés
   indéfiniment : AD/Kerberos/ACL Windows, serveur OpenVPN, L2TP, SHR/SHR2,
   NZB/eMule, KMIP/WORM, HA, IDS. Les traiter comme des régressions serait
   confondre parité et progrès.
5. **DSM garde l'avantage** sur la profondeur stockage (SHR/SHR2, extents,
   dédup, cache noyau), l'écosystème de paquets, l'identité d'entreprise, la
   chaîne de mise à jour et la suite média/productivité. **HomeVault garde
   l'avantage** sur la simplicité, la sécurité produit, l'intégrité des données,
   l'IA et la testabilité — et **choisit de ne pas copier** ce qui contredit ces
   avantages.

---

## 5. Preuves principales dans le code

- Apps et catégories : `frontend/src/routes/Desktop.svelte:1077-1108`
- SMB AD/Kerberos, NFS `sec=`, multicanal : `src/shares.rs:204-247, 686-689, 766-775, 1577-1731` ;
  guide `docs/guides/shares.md:492-575`
- Vignettes images uniquement : `src/handlers/explorer.rs:4297-4319`
- Protocoles du téléchargeur : `src/handlers/downloader.rs:78`
- Types de tâches : `src/scheduled_tasks_scheduler.rs:83-160`
- Clone VM : `src/handlers/vm.rs:163` ; routes VM : `src/web/mod.rs:2355-2426`
- **Audit** : table `audit_log` (`sqlx/migrations/086_audit_log.sql`),
  `src/audit.rs` (canal borné, purge 90 j), `src/handlers/audit.rs`
  (`/api/audit`, `/api/audit/export`) et source `audit` du Journal
  (`src/handlers/journal.rs`)
- **Isolation** : `installer/install.sh` → `render_service_unit` (durcissement
  P2‑7) et `render_apparmor_profile` (opt‑in) ; score
  `systemd-analyze security` imprimé par l'installeur
- **Signature/atomicité** : `hv_load_manifest`, `hv_swap_binary`,
  `hv_rollback_binary` dans `installer/install.sh` (P2‑8) ; **déclenchement
  UI** : `src/update_engine.rs` (canal, demande, moteur), `render_update_unit`
  dans l'installeur, `src/handlers/update_apply.rs`
  (`/api/settings/update/status|apply`), carte dans `Settings.svelte`
- **Restauration** : `src/restore_drill.rs` (copie temporaire du snapshot DB,
  `PRAGMA integrity_check` en lecture seule, décodage de l'archive config,
  état du dernier job restic), `src/handlers/restore_drill.rs`
  (`/api/settings/backup/restore-drill`) et le bouton *Verify restore* dans
  Settings (P2‑9)
- **WS‑Discovery** : `src/ws_discovery.rs` (unité `homevault-wsdd`, `wsdd2`
  sandboxé), champ `ws_discovery` de `SmbConfig`, `GET
  /api/shares/ws-discovery`, paquet optionnel `wsdd2` dans l'installeur
  (P1‑29)
- **MAJ Docker planifiées** : `src/docker_update_scheduler.rs`
  (`plan_tick`, baseline, `run_now`), `update_all_pending` partagé avec
  « Tout mettre à jour », `GET/PUT /api/docker/updates/schedule` et
  `POST /api/docker/updates/run-now` (P1‑30)
- **Tâches planifiées unifiées** : `TaskAction::Service` +
  `ServiceKind` (`docker_update`, `trash_purge`, `db_backup`,
  `notification`) dans `src/scheduled_tasks_scheduler.rs`, migration `087`
  (CHECK élargi, historique conservé), sélecteur dans Tasks (P1‑31)
- **Historique quotas + export Journal** : `src/quota_history.rs` (échantillon
  horaire, alerte au franchissement de 90 %, purge 90 j), migration `088`,
  `GET /api/transfer-quotas/history`, `GET /api/journal/export` et bouton
  *Export CSV* (P1‑32)
- **WireGuard UX** : `WgEndpointConfig` + `resolve_endpoint`/`endpoint_warning`
  dans `src/vpn.rs`, `GET/PUT /api/vpn/wireguard/endpoint`, carte endpoint
  dans VPN (P1‑33)
- Catalogue : `catalog/*/homevault-app.toml` (16 apps, dont **Baïkal** — CalDAV/CardDAV — P2‑20)
- **Intégration média** : `share_mount` + `gpu` dans le manifeste
  (`src/app_catalog/manifest.rs`), `RenderOptions`/`render_with_options` dans
  `src/app_catalog/mod.rs`, `media_share`/`gpu` à l'install et mémorisés dans
  `.install.toml` (`src/handlers/docker_catalog.rs`), `apply_update` rend
  depuis le marqueur (`src/app_catalog/updates.rs`) — P1‑34
- **Emplacements réseau** : `src/remote_mounts.rs` (validation, argv, 0600
  credentials, `/proc/mounts`, réconciliation),
  `src/handlers/remote_mounts.rs` (`/api/remote-mounts`), racine `network`
  dans `explorer_roots_for_session`, panneau
  `frontend/src/lib/components/explorer/RemoteMountPanel.svelte` (P1‑35)
- **Versions Btrfs** : `btrfs_versions_for_path` + `version_path_in_snapshot`
  + `deepest_containing_root` (`src/snapshots.rs`), branchés dans
  `handle_explorer_history` (`src/handlers/explorer.rs`) — P1‑35
- **Téléchargeur RSS + extraction** : `src/downloader_rss.rs` (parseur,
  `seen`, garde SSRF), `src/downloader_extract.rs` (`auto_extract`, allowlist,
  argv), `src/handlers/downloader.rs` (`/feeds*`, `/settings`, `/extract`,
  limites par tâche, `run_extract_loop`), `ServiceKind::DownloaderRss`
  (`src/scheduled_tasks_scheduler.rs`) — P1‑36
- **Cache ZFS L2ARC/SLOG** : `src/zfs_cache.rs` (parseur `zpool status -P`,
  validation, argv), `src/handlers/zfs_cache.rs` (`/api/zfs-cache`), carte
  dans `frontend/src/routes/Storage.svelte`, grants `zpool add|remove`
  (`src/setup.rs`) — P2‑16
- **LUN fins iSCSI** : `src/zvol.rs` (dataset/appareil/argv/parseurs),
  `src/handlers/iscsi.rs` (`kind: zvol`, `/snapshots`, `/clone`), panneau
  snapshots dans `frontend/src/routes/Iscsi.svelte`, grants `zfs create|clone`
  (`src/setup.rs`) — P2‑17
- **Export/import VM** : `src/vm_export.rs` (nom/argv/listing),
  `src/handlers/vm.rs` (`/exports`, `/{name}/export`, `/import`), dialogue dans
  `frontend/src/routes/VirtualMachines.svelte`, grants `incus export|import`
  épinglés au dossier d'export (`src/setup.rs`) — P2‑18
- **Versions Syncthing** : `src/syncthing_versions.rs` (politique, parsing,
  validation), `src/handlers/syncthing.rs` (`/folders/{id}/versions*`), dialogue
  dans `frontend/src/routes/Syncthing.svelte` — P2‑19
- **13 ordonnanceurs déjà existants** (`src/web/mod.rs:3011-3050`) : une
  « tâche planifiée backup/scrub/corbeille » doit les exposer, pas les dupliquer
- Serveur OpenVPN absent, client seul : `src/handlers/vpn.rs:953-1002` ;
  le commentaire d'en‑tête `src/vpn.rs:9` a été corrigé le 2026‑11‑08
- Backlog à jour : `HOMEVAULT_TASKLIST_A_FAIRE.md` (§1, §1bis et §6)

---

## 6. Filtre de valeur — applicable à toute future implémentation

HomeVault est un **NAS domestique accessible, simple, fiable, sécurisé et
respectueux de la vie privée**. Une fonctionnalité ne devient du travail que si
elle sert cette phrase. Le filtre ci‑dessous s'applique **avant** d'ouvrir un
item dans le backlog, et se relit **à la livraison**.

### 6.1 Les six questions

| # | Question | Échec typique |
|---|---|---|
| 1 | **Usage** — un foyer la rencontre‑t‑il dans le premier mois ? | fonction d'entreprise ou de home‑lab avancé |
| 2 | **Simplicité** — retire‑t‑elle une étape ou un doute, au lieu d'ajouter un concept ? | nouvel écran, nouvel acronyme, nouveau mode |
| 3 | **Fiabilité** — survit‑elle à une coupure, un disque HS, une restauration ? | état en mémoire, écriture non atomique, job non reprenable |
| 4 | **Vie privée/sécurité** — évite‑t‑elle un appel sortant, un démon root ou un stock de secrets en plus ? | télémétrie, cloud par défaut, PKI inutile |
| 5 | **Testabilité/reversibilité** — couvrable aux 3 niveaux, avec retour arrière ? | « vérifié » sans test de corruption, pas de rollback |
| 6 | **Bon endroit** — natif ou catalogue Docker ? | réimplémenter une app qui existe déjà |

**Règle de décision :** au moins **4 « oui » sur 6** pour entrer dans le
backlog ; **3 échecs ou plus = rejet**. Un « oui » obtenu en dégradant une autre
réponse ne compte pas.

### 6.2 Les trois pièges à refuser

1. **La parité comme objectif.** « DSM a un bouton » n'est pas une raison. Une
   case cochée uniquement pour égaler DSM est de la dette déguisée en roadmap.
2. **La fonctionnalité qui ajoute un mode de panne.** Un nouveau démon root, un
   nouveau stock de secrets, un nouveau protocole réseau : chacun doit être
   justifié par un usage domestique réel, sinon il est rejeté ou opt‑in.
3. **Le natif par réflexe.** Le §6 du backlog tranche : photothèque, mail,
   calendrier, NVR, PHP → **catalogue**. Le natif est réservé à ce qui doit
   partager l'identité, les partages, la sauvegarde ou la supervision.

### 6.3 Impact vie privée — à documenter dans le même changement

Tout item retenu qui touche des données personnelles met à jour `PRIVACY.md` :

| Item | Donnée | Garde‑fou obligatoire |
|---|---|---|
| Table d'audit (P1‑18) | acteur, IP, action, horodatage | admin‑only, rétention bornée, pas de contenu de fichier |
| Intégration média | photos/vidéos, métadonnées, visages (ML Immich) | traitement **local**, aucune API cloud, opt‑in |
| Montages CIFS/NFS (P1‑35) | credentials distants | fichier **0600**, jamais en argv/log/réponse, opt‑in, documenté (RoPA ligne 18) |
| Téléchargeur RSS (P1‑36) | URLs de flux (jeton possible dans le chemin) | fichier 0600, adresse loopback privée refusée, pas de télémétrie, limites par tâche (RoPA ligne 19) |
| WireGuard UX | clés privées | jamais loguées, jamais renvoyées en clair |
| OAuth2 e‑mail (P1‑16) | client secret + refresh token | scellés (`hv1:`) dans `data/email_oauth.json` en 0600, jamais renvoyés par l’API ni journalisés ; jeton d’accès en mémoire seulement ; état OAuth à usage unique lié au navigateur (RoPA ligne 20) |

### 6.4 Définition d'une « vraie avancée »

Un item est terminé quand : il passe les 6 questions ; il a ses tests unit +
intégration + E2E ; il est réversible ou accompagné d'un rollback ; il n'ajoute
aucun appel sortant non documenté ; sa doc (`docs/guides/`, `FeatureDoc`,
`TESTING.md`, `PRIVACY.md` si applicable) est à jour dans le même changement.
Tout le reste est un prototype, pas une livraison.

---

## 7. Corrections appliquées le 2026‑11‑08

- §3 réécrit en **décisions** (garder / recadrer / geler‑rejeter) ; les écarts
  de parité ne sont plus promus en P0/P1 par défaut.
- §6 ajouté : filtre de valeur, pièges, impact vie privée, définition de
  « vraie avancée ».
- `src/vpn.rs:9` : commentaire d'en‑tête corrigé (« OpenVPN client‑only »).
- `HOMEVAULT_TASKLIST_A_FAIRE.md` : §1bis renvoie désormais à ce document, et
  le §6 du backlog acte le rejet explicite de SHR/SHR2, serveur OpenVPN,
  NZB/eMule et de la suite productivité native.

### Travail d'implémentation démarré le 2026‑11‑08

**P2‑8 — mise à jour signée + atomique + rollback automatique (partiel, côté
installeur).**

- `installer/install.sh` accepte `HV_RELEASE_MANIFEST` (chemin ou URL) et
  vérifie une signature détachée **avant** tout téléchargement :
  Ed25519 (`pkeyutl -rawin`, OpenSSL 3) puis RSA/ECDSA SHA‑256 en repli. Le
  manifeste fixe `version`, `arch`, `url`, `sha256`, `size` ; toute divergence
  refuse l'update sans toucher au binaire en service.
- Extraction en staging, smoke test `homevault --version` (nouveau drapeau
  `--version` dans `src/bin/server.rs`), puis remplacement atomique par rename
  et conservation de `homevault.previous`.
- Rollback **automatique** si `/api/auth/setup-status` ne répond pas en 20 s ;
  le binaire fautif est conservé en `homevault.failed`, le service est relancé
  sur la version précédente, et l'échec est enregistré.
- Résultat + historique écrits dans `data/updates/last-result.json` et
  `history.jsonl` *(l’historique est désormais un objet JSON compact par ligne,
  §8)* ; `GET /settings/version` expose `last_update` et l'écran
  Settings affiche *Applied / Rolled back / Failed*.
- Nouveau mode `--verify-manifest` (sans root) et signature du manifeste au
  `--package` via `HV_SIGNING_KEY` (la clé privée n'est jamais commitée).
- Tests écrits mais **non exécutés** :
  `installer/tests/update-engine.test.sh` (comparaison de versions, signature
  et altération, champs du manifeste, digest/taille, smoke test, swap,
  rollback, résultat/historique).
- Docs : `docs/guides/settings.md` → « System → Signed, atomic updates »,
  `docs/API.md`, `docs/guides/settings.md` (tableau des endpoints), FeatureDoc
  `settings` dans `src/rag_indexers.rs`.

**P2‑8 — moitié UI livrée le 2026‑11‑09.** Le canal de publication signé
(`release_manifest`) se règle dans Settings et **Apply update** déclenche le
moteur depuis l'interface :

- le serveur ne remplace jamais son propre binaire : il écrit
  `data/updates/request.json` (0600) puis lance l'unité root
  `homevault-update`, dont l'`ExecStart` est l'argv **fixe**
  `homevault --update-engine` ;
- `KillMode=process` fait survivre l'unité au redémarrage qu'elle provoque, et
  le moteur exécute l'installeur avec `HV_BOOTSTRAP=0` — la signature reste
  vérifiée **avant** le premier téléchargement ;
- `GET /settings/update/status` expose disponibilité, demande en attente,
  dernier code de sortie et fin du journal ; l'écran les affiche et désactive
  le bouton avec la raison quand il manque un canal ou le moteur ;
- sudoers : uniquement `systemctl start homevault-update` (une unité fixe,
  jamais une commande paramétrée) ; `stop`/`restart` non accordés ;
- une installation `curl | bash` copie désormais l'installeur dans
  `/opt/homevault/installer/install.sh`, donc le bouton existe aussi là.

**Reste sur P2‑8** : publication de la clé publique de production et validation
terrain sur un hôte réel.

**P2‑7 — isolation d'exécution (partiel, côté installeur).**

- L'unité systemd est désormais rendue par `render_service_unit` (source
  unique, testée) et embarque un bloc de durcissement **mesuré** :
  `ProtectSystem=true`, `ReadOnlyPaths=-/opt/homevault`, `PrivateTmp=true`,
  `RestrictSUIDSGID=true`, `LockPersonality=true`, `RestrictRealtime=true`,
  `ProtectKernelLogs=true`, et `NoNewPrivileges=true` **en mode root
  uniquement**.
- Les directives qui casseraient une fonction livrée sont volontairement
  **absentes** et documentées : `ProtectKernelTunables` (sysctl WireGuard),
  `ProtectKernelModules` (modprobe iSCSI), `ProtectHostname` (renommage),
  `ProtectClock` (fuseau), `PrivateDevices` (disques), `ProtectHome`
  (dossiers utilisateurs), `CapabilityBoundingSet` (sudo setuid).
- Profil AppArmor **opt‑in** (`HV_APPARMOR=1`), **complain par défaut**,
  enforce après validation terrain (`HV_APPARMOR_ENFORCE=1`). LSM absent →
  avertissement, jamais de confinement revendiqué à tort.
- Le journal d'installation affiche le score
  `systemd-analyze security homevault`.
- Tests écrits mais **non exécutés** :
  `installer/tests/unit-hardening.test.sh` (root vs non‑root, présence du
  durcissement, absence des directives cassantes, complain/enforce AppArmor).
- Docs : `docs/guides/security.md` → « Process isolation ».

**Reste sur P2‑7** : passe terrain sur un hôte réel (`systemd-analyze security`
avant/après, `aa-logprof` puis enforce), et un profil AppArmor resserré après
cette mesure.

**P1‑18 — table d'audit interrogeable (livrée).**

- Migration `086_audit_log.sql` : `created_at`, `severity`, `actor`, `action`,
  `target`, `client_ip`, `message`, avec index sur la date, la sévérité,
  l'acteur et l'action.
- `src/audit.rs` : écriture par canal borné (jamais bloquante, compteur de
  pertes), normalisation (contrôle, troncature), purge à 90 jours au démarrage
  et toutes les 6 h.
- `events::log_event`/`log_event_warn` alimentent la table ; les événements
  d'authentification (`auth.login`, `auth.login.trusted`,
  `auth.login.failed`, `auth.logout`) portent acteur/action/IP structurés.
- API admin : `GET /api/audit` (`from`, `to`, `severity`, `actor`, `action`,
  `q`, `limit` ≤ 500, `offset`) et `GET /api/audit/export` (CSV/JSON, ≤ 5 000
  lignes). Toute valeur invalide est un `400` nommé.
- Journal : nouvelle source `audit`, admin-only, avec la recherche texte
  existante ; aucune dépendance à journald ou à un fichier hôte.
- Tests écrits mais **non exécutés** : 9 unitaires (`src/audit.rs`), 5
  d'intégration (`tests/audit_integration.rs`), E2E Journal 2.4.
- Docs : `docs/guides/journal.md`, `docs/API.md`, `docs/BACKEND.md`,
  `SCREENS.md`, `PRIVACY.md` (RoPA ligne 16), FeatureDoc `journal`.

**P1‑18 — filtres structurés livrés dans l'UI le 2026‑11‑09.** La source
`audit` du Journal expose désormais une rangée de filtres étiquetés —
**gravité**, **utilisateur**, **action**, **du**, **au** — qui se combinent
avec la recherche texte et alimentent `GET /journal/entries?source=audit&…` :

- le pied de page montre le nombre exact de résultats (`meta.total`) et un
  paginateur précédent/suivant (200 par page) ;
- les lignes affichent `actor`/`target`/`client_ip` en badges, en plus de
  `[action]` devant la phrase ;
- l'export CSV reprend les mêmes filtres, donc le fichier ne peut pas
  contredire l'écran ;
- un filtre structuré sur `app`/`system`/`/var/log` est un `400` nommant le
  champ — jamais des lignes non filtrées qui *semblent* filtrées ; une plage
  de dates inversée est refusée côté écran (message, aucune requête) et côté
  API.

Implémentation : `JournalQuery` → `audit_query_from_journal`
(`src/handlers/journal.rs`) ; la validation des dates/gravités reste dans
`src/audit.rs`, donc la même valeur invalide donne le même message sur
`/api/audit` et sur `/api/journal/entries`. Tests écrits, non exécutés :
2 unitaires, 5 d'intégration, E2E Journal §13 (5).

**P1‑27 — signature des stores communautaires (livrée le 2026‑11‑09).**

- `src/catalog_signature.rs` : signatures Ed25519 **détachées**
  (`<fichier>.sig`, hex ou base64, sur les octets exacts) du descripteur
  `homevault-store.toml` et de chaque `homevault-app.toml`.
- Vérification **au sync** (un store refusé n'est pas enregistré) **et au
  chargement** (`scan_store`) : un checkout modifié après le clone cesse
  d'être proposé, sans toucher aux conteneurs déjà installés.
- Ancre de confiance : `HV_CATALOG_PUBKEY` (inline ou chemin), puis
  `HOMEVAULT_CATALOG_PUBKEY` à la compilation, puis
  `/usr/local/lib/homevault/catalog-key.ed25519`. L'installeur copie la valeur
  d'environnement à cet emplacement ; `HV_REQUIRE_SIGNED_STORES=1` rend la clé
  obligatoire, sans clé les stores restent chargeables et affichés *Unsigned*.
- `GET /docker/stores` expose `signature` (`verified`/`unconfigured`/
  `invalid`), `signature_error` et `app_count` (0 pour un store refusé) ;
  l'écran Docker affiche **Signed / Unsigned / Signature refused**.
- Les références d’images sont validées avant `docker pull` (`@sha256:` +
  64 hex minuscules, grammaire d’étiquette, pas de `:tag` ajouté à un digest) ;
  `HOMEVAULT_REQUIRE_PINNED_IMAGES=1` refuse une installation dont un service
  n’est pas épinglé, en le nommant. Le pinning **obligatoire** reste rejeté
  (décision du 2026‑11‑06).
- Tests écrits mais **non exécutés** : 7 unitaires (`catalog_signature`), 1
  d'intégration (`app_catalog`), `bash -n` de l'installeur. Docs :
  `docs/guides/docker.md` (commandes de signature), `docs/API.md`,
  `SCREENS.md`, FeatureDoc `docker`.

**P1‑28 — stabilité système (livrée côté installeur le 2026‑11‑09).**

- `/etc/sysctl.d/99-homevault.conf` :
  `kernel.panic=10`, `kernel.panic_on_oops=1`,
  `vm.dirty_background_ratio=5`, `vm.dirty_ratio=10`, appliqué par
  `sysctl -q -p` (`HV_SKIP_SYSCTL=1` écrit sans appliquer) — un noyau qui
  panique redémarre au lieu de rester bloqué avec des partages à moitié
  montés, et un disque lent ne peut pas accumuler des minutes d’écriture non
  synchronisée.
- `smartd` et `mdmonitor` restent **désactivés par défaut** : HomeVault
  échantillonne SMART et le RAID lui‑même, donc lancer les deux doublerait
  chaque alerte. `HV_SMARTD=1`/`HV_MDMONITOR=1` les active ; un démon déjà
  actif est signalé, jamais coupé en silence.
- La décision est une fonction pure `monitor_decision`
  (`unavailable`/`already`/`enable`/`leave`) couverte par le test d’installeur.
- La carte UI write cache/UPS reste **gelée** (revue de valeur du 2026‑11‑06) ;
  les données SMART/RAID de l’écran Health couvrent le besoin.
- Tests : 9 assertions d’installeur (`bash -n`, non exécuté). Docs :
  `docs/ARCHITECTURE.md`.

**P2‑9 — restauration bare‑metal (drill livré, test VM restant).**

- `src/restore_drill.rs` : copie le dernier `data/backups/*.db` dans un
  répertoire temporaire, l'ouvre **en lecture seule**, vérifie l'en-tête SQLite
  puis `PRAGMA integrity_check` et compte tables/utilisateurs. Décode la
  dernière archive `data/config-backups/*.hvbak` (checksums par entrée ;
  archive chiffrée → passphrase requise) et rapporte le dernier job restic.
  Ne touche jamais à la base live et supprime sa copie temporaire.
- Verdict **Ready to restore** seulement si le snapshot DB et l'archive de
  configuration sont tous deux vérifiés ; un job restic ancien/échoué/absent
  est un avertissement, pas un blocage (réplication/snapshots peuvent couvrir).
- API admin `GET/POST /api/settings/backup/restore-drill` (en-tête optionnel
  `x-backup-passphrase`), bouton **Verify restore** dans Settings → System →
  Database snapshots, rapport persisté dans `data/restore-drill.json`.
- Runbook mis à jour : `docs/guides/disaster-recovery.md` (drill + répétition
  trimestrielle + périmètre).
- Tests écrits mais **non exécutés** : 4 unitaires (`src/restore_drill.rs`),
  5 d'intégration (`tests/restore_drill_integration.rs`), E2E Settings 8.6.
- Docs : `docs/guides/disaster-recovery.md`, `docs/API.md`, `SCREENS.md`,
  FeatureDocs `disaster-recovery` et `settings`.

**P1‑29 — SMB WS‑Discovery (livré).**

- `SmbConfig` gagne `ws_discovery` (défaut **on**, comme la découverte réseau
  Windows de DSM), appliqué par `src/ws_discovery.rs` et non par `smb.conf`.
- `homevault-wsdd.service` exécute `wsdd2` (nom NetBIOS + workgroup
  sanitizés, `DynamicUser`, `NoNewPrivileges`, `ProtectSystem=strict`,
  `RestrictAddressFamilies` étroit) ; désactiver l'unité la supprime.
- Enregistrement SMB en best-effort + reconcile au démarrage ;
  `GET /api/shares/ws-discovery` expose `available/enabled/running/detail`, et
  la carte SMB dit « wsdd2 is not installed » plutôt que d'afficher un
  interrupteur mort. `wsdd2` est un paquet optionnel de l'installeur et une
  dépendance listée.
- Tests écrits mais **non exécutés** : 6 unitaires (`src/ws_discovery.rs`), 1
  (`src/shares.rs`), 1 d'intégration (`tests/shares_integration.rs`), E2E
  Shares 4.9b.
- Docs : `docs/guides/shares.md`, `docs/API.md`, `SCREENS.md`, FeatureDoc
  `shares`.

**P1‑30 — mises à jour Docker planifiées/auto (livré).**

- `src/docker_update_scheduler.rs` : planificateur **opt‑in** (off par défaut)
  pour les apps du catalogue, jamais les conteneurs arbitraires. Réglage dans
  `data/docker_updates.json` (`interval_hours` 6–168, dernier résultat/erreur).
- Le premier tick après activation pose une **baseline** et attend un
  intervalle complet ; un run dû réutilise `update_all_pending` (le même chemin
  que « Tout mettre à jour ») et ne redémarre que les apps ayant réellement une
  mise à jour. Un échec avance quand même l'horloge et enregistre `last_error`.
- API admin `GET/PUT /api/docker/updates/schedule` et
  `POST /api/docker/updates/run-now` ; divulgation repliée **Scheduled
  updates** dans l'onglet App Catalog.
- Tests écrits mais **non exécutés** : 5 unitaires, 3 d'intégration
  (`tests/docker_update_schedule_integration.rs`), E2E Docker 15.6.
- Docs : `docs/guides/docker.md`, `docs/API.md`, `SCREENS.md`, FeatureDoc
  `docker`.

**P1‑31 — unification des tâches planifiées (livré).**

- Le moteur générique gagne `service`, un cinquième type qui **déclenche** un
  sous-système existant au lieu de créer un second ordonnanceur :
  `docker_update`, `trash_purge`, `db_backup` (snapshot `VACUUM INTO`) et
  `notification` (message obligatoire, ≤ 500 caractères).
- Restic, scrub, snapshots planifiés et les autres boucles **gardent leur
  propre ordonnanceur** : leur planification est déjà la source de vérité.
- Migration `087` reconstruit `scheduled_tasks` / `scheduled_task_runs` avec un
  CHECK élargi, en conservant l'historique (renommage, pas de cascade).
- UI : sélecteur de service ; champ message uniquement pour la notification.
- Tests écrits mais **non exécutés** : 3 unitaires, 1 d'intégration
  (`tests/scheduled_tasks_integration.rs`), E2E Tasks 3.5.
- Docs : `docs/guides/scheduled-tasks.md`, `docs/API.md`, `SCREENS.md`,
  FeatureDoc `scheduled-tasks`.

**P1‑32 — historique/alertes quotas + export Journal (livré).**

- `src/quota_history.rs` échantillonne chaque quota de transfert toutes les
  heures dans `transfer_quota_history` (migration `088`, rétention 90 j). Une
  seule notification par franchissement de 90 % dans une période ; rester
  au-dessus ne re-alerte pas, une remise à zéro est une nouvelle base.
- `GET /api/transfer-quotas/history` (admin) sert les points ; l'écran Quotas
  ouvre un dialogue d'historique en lecture seule par ligne.
- `GET /api/journal/export` (mêmes filtres et privilèges que l'écran, CSV/JSON,
  5 000 entrées) + bouton **Export CSV** dans le Journal.
- Tests écrits mais **non exécutés** : 2 unitaires quotas + 2 Journal,
  2 d'intégration, E2E Quotas 4.4 / Journal 3.1b.
- Docs : `docs/guides/transfer-quotas.md`, `docs/guides/journal.md`,
  `docs/API.md`, `PRIVACY.md` (RoPA ligne 17), FeatureDocs `quotas`/`journal`.

**P1‑33 — WireGuard UX (livré).**

- Les configs de pairs (et leurs QR codes, déjà livrés) utilisent un
  **endpoint public résolu** : override manuel (`data/wg_endpoint.json`), nom
  **DNS dynamique** configuré, puis IP publique détectée.
- `GET/PUT /api/vpn/wireguard/endpoint` expose `endpoint_host`,
  `endpoint_source` (`manual`/`ddns`/`public_ip`/`unset`) et
  `endpoint_warning`. La carte VPN affiche la valeur et sa source et avertit
  quand elle est absente ou privée/LAN/CGNAT.
- Tests écrits mais **non exécutés** : 2 unitaires (`src/vpn.rs`), 1
  d'intégration (`tests/vpn_integration.rs`), E2E VPN 1.5.
- Docs : `docs/guides/vpn.md`, `docs/API.md`, `SCREENS.md`, FeatureDoc `vpn`.

**P1‑34 — intégration média : partage + transcodage matériel (livré).**

- Les apps média du catalogue (Jellyfin, Immich, Navidrome) déclarent
  `share_mount` dans leur manifeste et montent `{{SHARE_DIR}}:<montage>:ro`.
  La valeur est validée au parse (absolue, sans `..`, `:` ni espace).
- L'admin choisit un **partage configuré** (`media_share: {name, share_type}`),
  résolu par `SharesService::get_share` puis monté **en lecture seule**. Un app
  qui ne déclare pas de `share_mount` refuse la demande (`400`) ; un partage
  inconnu aussi. Sans choix, l'app reçoit son propre dossier
  (`apps/<clé>/<montage>`).
- **Transcodage matériel** opt‑in (`gpu = true` au manifeste, `gpu: true`,
  `/dev/dri` présent) ; sinon le bouton est désactivé et le bloc `devices:`
  absent du compose.
- `.install.toml` enregistre `environment`, le partage et `gpu` ;
  `apply_update` rend depuis eux — corrige la perte des réglages lors d'un
  update.
- Tests écrits mais **non exécutés** : 8 unitaires, 3 d'intégration, E2E
  Docker 15.1/15.3b.
- Docs : `docs/guides/docker.md` (§ « Media integration »), `docs/API.md`,
  `SCREENS.md`, FeatureDoc `docker`.

**P1‑35 — Explorer : emplacements réseau CIFS/NFS + versions Btrfs (livré).**

- `src/remote_mounts.rs` : nom = slug `[a-z0-9-]` (≤ 32) devenant
  `/mnt/homevault-remote/<nom>`, validation du serveur (nom DNS ou IPv4),
  du partage/export et des options (liste blanche par protocole), argv
  `mount -t <fs> -o …` construit côté serveur, mot de passe en fichier 0600
  passé en `credentials=`, parsing de `/proc/mounts` (échappements octaux),
  réconciliation au démarrage des emplacements `enabled`.
- `src/handlers/remote_mounts.rs` : `GET/POST /api/remote-mounts`,
  `PUT/DELETE /{name}`, `POST /{name}/mount|unmount`, `POST /reconcile` —
  tous admin, journalisés ; un helper absent est un `503` nommé.
- `src/snapshots.rs` : `btrfs_versions_for_path` (partage → pool → liste des
  snapshots, chemins de version pointant sur le fichier), branché dans
  `handle_explorer_history` quand ZFS n'a rien trouvé.
- Panneau `RemoteMountPanel.svelte` dans la barre latérale (admin), racines
  `kind: "network"` dans `explorer_roots_for_session`.
- Tests écrits mais **non exécutés** : 10 unitaires, 7 d'intégration (aucun
  montage), E2E Explorer §19.
- Docs : `docs/guides/remote-mounts.md` (nouveau), `docs/guides/explorer.md`,
  `docs/INDEX.md`, `docs/API.md`, `SCREENS.md`, FeatureDocs
  `explorer`/`snapshots`.

**Reste sur P2‑9** : le test de reconstruction complète sur une VM jetable
(boot, installation, restauration DB + configuration + données, vérification)
et, si la cible le justifie, une image système équivalente à `md0`. **Toute la
feuille §3.2 est livrée** : les trois P1 (média, Explorer, téléchargeur) et les
cinq P2 (cache SSD ZFS L2ARC/SLOG, LUN fins iSCSI, export/import VM, versions
Syncthing, calendrier/contacts en app catalogue). Restent les finitions
P2‑7/P2‑8 (terrain, clé de production) et le test VM de P2‑9.

---

## 8. Correctifs de vérification — 2026‑11‑09

> **Contexte.** Le premier passage complet de `cargo test --lib` sur le HEAD
> `910a84f` a rendu **2 804 tests verts et 23 échecs**. Les causes étaient
> réelles, pas des tests obsolètes dans leur majorité : l’intégration média
> (P1‑34) refusait ses propres manifestes et le bloc GPU cassait le YAML. Le
> firmware DSM 7.4.1 reste la **référence de comportement** ; aucun code du
> firmware n’est copié, et les décisions §3.3 (rejets/gels) sont inchangées.

### 8.1 Catalogue média — P1‑34 débloqué

| Défaut | Correction | Test |
|---|---|---|
| `validate_share_mount` refusait **tout** chemin absolu (`"/media".split('/')` commence par un segment vide) : Jellyfin, Immich et Navidrome ne chargeaient pas (catalogue 16 → 13) | `split('/').skip(1)` : seuls les composants vides **après** le premier slash sont refusés (`/mnt//media`) | `manifest::tests::test_share_mount_validation_and_default_folder` (déjà écrit, désormais valide) |
| `replace_block` n’indentait pas la **première** ligne du bloc injecté : un `{{GPU_DEVICES}}` indenté produisait `devices:` en colonne 0, donc un compose invalide | chaque ligne reçoit le préfixe de la ligne porteuse, première comprise | `app_catalog::tests::test_replace_block_reindents_and_drops_whole_lines` |

### 8.2 Bugs isolés corrigés

| Module | Défaut | Correction | Test |
|---|---|---|---|
| `src/docker.rs` | `redis:tag/slash` accepté (le tag n’était cherché qu’après le dernier slash) | analyse par segments : un `:` n’est valide que comme port de registre (numérique, 1–65535) dans le premier segment ou comme séparateur de tag dans le dernier | matrice accept/refus étendue (`redis:7`, `localhost:5000/app`, `redis:7:8`, `registry:abc/app`, `/app`, `app/`) |
| `src/downloader_rss.rs` | la taille d’un `<link rel="enclosure">` Atom était ignorée | lecture de l’attribut `length` sur l’enclosure Atom, comme pour RSS | `test_parse_atom_uses_the_enclosure_link` |
| `src/email_oauth.rs` | une URL de redirection contenant un espace était acceptée (le parseur l’encodait) | refus explicite de tout espace interne avant l’enregistrement chez le fournisseur | `test_redirect_url_validation` |
| `src/host_accounts.rs` | `smbpasswd` non-root (ou Samba absent) faisait échouer la suppression/le verrouillage d’un compte | `samba_sync_is_noop` reconnaît « When run by root »/« Permission denied » ; une absence du binaire est un no-op ; la base reste autoritaire | test unitaire du prédicat + test async existant |
| `src/handlers/journal.rs` | un message multiligne créait un second enregistrement dans l’export CSV | les sauts de ligne sont remplacés par une espace avant le quoting : un enregistrement = une ligne | test CSV étendu (LF et CRLF, `lines() == 2`) |
| `src/rag_indexers.rs` | `email-oauth.md` et `remote-mounts.md` n’étaient pas indexés | deux `FeatureDoc` ajoutés (`email-oauth`, `remote-mounts`) + table id → guide | `test_every_guide_has_a_feature_doc` |
| `src/dependencies.rs` | `bash`, `net`, `smbpasswd` exécutés sans source d’installation | `bash` en base (Essential) ; `net`/`smbpasswd` rattachés au paquet `samba` avec un probe dédié | `test_every_commanded_binary_has_an_install_source` |
| `installer/install.sh` | `history.jsonl` recopiait le JSON **indenté** de `last-result.json` : 7 lignes par enregistrement, JSONL invalide | l’historique écrit `json.dumps(doc) + "\n"` ; `last-result.json` reste lisible par un humain | test installeur : une ligne + `json.loads` de cette ligne |

### 8.3 Tests périmés réalignés sur le produit

| Test | Pourquoi il était rouge | Réalignement |
|---|---|---|
| `dependencies::test_install_sh_embeds_a_complete_systemd_unit` | P2‑7 a remplacé le heredoc statique par `render_service_unit` | le test **rend la vraie fonction** (sourcée avec `HV_LIB_ONLY=1`) et vérifie le durcissement root vs non-root |
| `dependencies::test_install_sh_elevates_through_sudo_and_forwards_release_knobs` | `--verify-manifest` a rejoint la liste des modes sans élévation | la ligne bootstrap est lue et chaque drapeau vérifié |
| `setup::test_vm_export_grants_are_confined_to_the_export_folder` | `/srv/homevault` était codé en dur alors que le chemin vient de `base_dir()` | le test dérive le préfixe de `config::paths::base_dir()` |
| `shares::test_validate_smb_config_rejects_unimplemented_domain_auth` | `ads`/`domain` sont configurables depuis P2‑1, avec realm et idmap | `server` reste refusé ; `ads`/`domain` sans realm refusés, avec realm + idmap valides acceptés |
| `ws_discovery::test_render_unit_cannot_be_broken_by_a_newline` | l’attente tronquait le workgroup à 14 caractères au lieu de 15 | attente corrigée (`WORKGROUPEExecSt`) |

### 8.4 Ce qui n’a pas été exécuté

Conformément à la consigne du mainteneur, les tests ont été **écrits ou
corrigés mais pas exécutés** dans cette passe. Les vérifications statiques
faites : `bash -n` sur `install.sh` et les deux suites d’installeur,
`cargo check --lib`, `cargo check --tests`. Restent à lancer sur un hôte :
`cargo test` (unitaire + 109 suites d’intégration), `cargo fmt --check`,
`npm run check && npm run build`, les E2E ciblés, puis reporter les compteurs
dans `TESTING.md`.

### 8.5 Documentation et vie privée

- `PRIVACY.md` : **RoPA ligne 20** (client secret + refresh token OAuth2) et
  ligne sortante vers les points d’échange Google/Microsoft 365.
- `docs/guides/rag.md` : la base de connaissances passe à **78 FeatureDocs**,
  avec `email-oauth` et `remote-mounts`.
- `docs/guides/settings.md` : distinction explicite entre
  `last-result.json` (lisible) et `history.jsonl` (un objet JSON compact par
  ligne).
- `TESTING.md` : section dédiée du 2026‑11‑09 (tests écrits, non exécutés).

---

## 9. Lot intégrité intégré — 2026‑11‑09

> **Contexte.** Le lot fiabilité `d46a2a5` (P0‑5→P0‑12, P2‑10→P2‑14) vivait
> sur une branche non mergée, basée 26 commits derrière `main`. Il est
> désormais **fusionné dans `main`**, sans écraser les apports du comparatif
> (média, audit, WS‑Discovery, OAuth2, mises à jour signées, etc.). Le
> firmware DSM reste la référence de comportement ; les décisions §3.3 sont
> inchangées.

### 9.1 Ce que le merge apporte

- **Reprise après incident** : `src/scrub.rs` (scrub planifié, reprise après
  arrêt sale), `src/storage/recovery.rs` (assemblage mdadm, `btrfs rescue`,
  `e2fsck`, montage `ro`), historique SMART/I/O (`src/disk_health.rs`,
  `disk_health_scheduler.rs`), prédiction de panne et rapport hebdomadaire.
- **Intégrité des écritures** : configuration atomique avec `.bak` et
  self-heal (`src/storage/atomic.rs`), `testparm` avant remplacement,
  `synchronous` SQLite explicite, snapshots applicatifs (quiescence).
- **iSCSI** : politiques d’écriture, FUA/UNMAP, garde volume plein.
- **Btrfs** : compression/`space_cache=v2`, balance et TRIM centralisés
  (`src/btrfs_maintenance.rs`) — les options `fstab` ne sont plus dupliquées
  dans `src/pools.rs`.
- **RAID** : moniteur d’événements et suggestion de spare
  (`src/raid_stability.rs`).
- **Nouveaux écrans/onglets** : Health (SMART, I/O, prédiction), Storage
  (compression Btrfs), iSCSI (I/O policy), Shares (acquittement, écritures
  atomiques), Tasks (`scrub`, `restic_check`).

### 9.2 Conflits sémantiques tranchés (le meilleur des deux)

| Conflit | Décision |
|---|---|
| `SmbConfig.realm` : `String` (main) vs `Option<String>` (lot) | **`String`** avec `#[serde(default)]` : une seule forme, le code P2‑1 (validation, rendu, join) reste inchangé |
| `SmbConfig` : champs de fiabilité (durable handles, chiffrement, signing, strict sync, AIO, oplocks) | **tous conservés**, rendus dans `smb.conf` et dans l’UI à côté de WS‑Discovery |
| Tâches planifiées : `service` (main) vs `scrub`/`restic_check` (lot) | **union des 7 types** ; migration **093** reconstruit le CHECK SQLite et conserve l’historique |
| Migrations 082–085 en collision (les deux branches) | migrations du lot **renumérotées 089–092** ; 086–088 (audit, service, quotas) inchangées ; 093 ajoutée |
| `validated_backstore` : tuple (main) vs struct (lot) | **struct** `ValidatedBackstore` ; le chemin zvol renvoie désormais write policy/FUA/UNMAP |
| `render_service_unit` (main) vs heredoc statique (lot) | **`render_service_unit`** conservé : le durcissement P2‑7 et le test qui le rend en direct restent la source de vérité |
| `add_missing` / `create_backstore` : signatures | arguments de fiabilité (write policy/FUA/UNMAP) propagés aux deux appels |
| i18n : clés `realm`/`idmap_range`/`nfs_security` dupliquées | une seule définition par langue, valeurs main conservées |
| `supply_chain` (lot) vs validation P1‑27 (main) | **validation P1‑27** conservée dans `docker.rs`/`docker_catalog.rs` ; le module `supply_chain` reste disponible mais n’est plus le chemin d’installation |
| UI Shares : ADS/join (main) vs toggles de fiabilité (lot) | **une seule carte** : ADS + realm/idmap + join/leave, puis WS‑Discovery, puis les toggles de fiabilité |
| UI iSCSI : zvol + snapshots (main) vs I/O policy (lot) | **un seul éditeur** : kind zvol + sparse, write policy, FUA, UNMAP |

### 9.3 Vérification

- `cargo check --lib` et `cargo check --tests` : **verts** (compilation seule,
  aucune suite exécutée).
- `bash -n installer/install.sh` : vert.
- `npm run check` : **0 erreur, 0 warning**.
- `cargo test` (unitaire + 109 suites d’intégration), les E2E et
  `cargo fmt --check` restent à lancer — même réserve que §8.4.
- Test ajouté (écrit, non exécuté) :
  `scheduled_tasks_scheduler::tests::all_seven_task_types_coexist_after_the_integrity_merge`.

### 9.4 Reste après ce merge

1. Exécuter la suite complète et corriger les échecs d’intégration éventuels
   (les deux branches ne s’étaient jamais rencontrées).
2. P2‑9 : reconstruction bare‑metal sur VM (FIAB‑05).
3. FIAB‑02 : le watchdog systemd est livré (§10) ; reste la supervision des
   boucles de fond (relance/backoff, état dans Health). FIAB‑04 (DB/config
   hors disque système) reste ouvert.
4. Clé de production et AppArmor enforce (P2‑7/P2‑8).

---

## 10. Watchdog applicatif — 2026‑11‑09 (FIAB‑02, moitié systemd)

> Le lot fiabilité signalait l’absence de supervision : l’unité était
> `Type=simple`, sans `WatchdogSec`, et un runtime bloqué gardait le port TCP
> ouvert sans servir. La première moitié est livrée ; la supervision des
> boucles de fond reste ouverte.

- `src/systemd_notify.rs` : `ready()` (READY=1 + STATUS) et un heartbeat
  `WATCHDOG=1` toutes les **10 s**, envoyé par un datagramme Unix sur
  `NOTIFY_SOCKET` (chemin ou socket abstrait `@…`). Sans socket systemd,
  chaque appel est un no‑op : un lancement à la main n’est pas affecté.
- `installer/install.sh` (`render_service_unit`) ajoute `WatchdogSec=30` et
  `NotifyAccess=main`. Trois ticks manqués = runtime bloqué, systemd redémarre
  le service au lieu de laisser une appliance à moitié vivante.
- `src/web/mod.rs` envoie `READY=1` **après** la liaison du listener, puis
  démarre le heartbeat.
- Tests écrits, non exécutés : format du datagramme, socket abstrait/chemin,
  no‑op sans `NOTIFY_SOCKET`, livraison réelle sur un `UnixDatagram` de test,
  et assertions d’installeur (`WatchdogSec`/`NotifyAccess`).
- **Reste FIAB‑02** : envelopper les boucles `tokio::spawn` dans un
  superviseur (handle conservé, relance avec backoff, état exposé dans Health)
  — le watchdog systemd détecte un blocage global, pas la mort silencieuse
  d’une seule boucle.
