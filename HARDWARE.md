# HomeVault — Matériel et systèmes pris en charge

Ce document décrit la **configuration minimale** et la **configuration
recommandée** pour faire tourner HomeVault, ainsi que les systèmes
d'exploitation et architectures officiellement pris en charge.

HomeVault est un **binaire unique** (Rust / Axum / Tokio, base SQLite
embarquée, interface web incluse). Le serveur lui‑même est léger ; ce sont
les **fonctions optionnelles** (antivirus, ZFS, assistant IA, Docker) qui
déterminent réellement les besoins en mémoire et en disque.

> Les besoins réseau se limitent à **une interface** (filaire de préférence).
> Aucun accès Internet permanent n'est requis : les vérifications de mises à
> jour sont sur *Manuel* par défaut et tout le traitement est local.

---

## 1. Systèmes d'exploitation pris en charge

| | Version minimale | Recommandé |
|---|---|---|
| **Debian** | 10 (Buster) | 12 (Bookworm) ou plus récent |
| **Ubuntu Server** | 20.04 LTS | 24.04 LTS |

Autres prérequis logiciels :

- **systemd** (HomeVault s'installe comme service systemd)
- **Accès root** pour l'installation et l'exécution
  (le service tourne en `root` — il gère RAID, ZFS, Samba/NFS, chiffrement
  de disque, comptes Linux réels, etc.)
- Distribution basée sur **APT** (l'installateur utilise `apt-get`)
- Installation **64 bits fortement conseillée** (obligatoire si ZFS)

Distributions dérivées non testées mais généralement compatibles si elles
sont basées sur Debian/Ubuntu **et** APT (Raspberry Pi OS 64‑bit, Linux
Mint, Zorin OS…). Les distributions non‑APT (Fedora, openSUSE, Arch,
Alpine) **ne sont pas prises en charge** par l'installateur.

### Architectures processeur

| Architecture | Prise en charge | Remarque |
|---|---|---|
| `x86_64` (amd64) | ✅ Complète | Cible principale |
| `aarch64` (ARM64) | ✅ Complète | Raspberry Pi 4/5 64‑bit, mini‑PC ARM |
| `armv7l` (ARM 32 bits) | ⚠️ Fonctionnelle | ZFS indisponible ; RAM plus limitée |

---

## 2. Configuration minimale

**Cible : OS serveur + HomeVault seul, sans antivirus, sans assistant IA,
sans conteneurs Docker.**

| Ressource | Minimum |
|---|---|
| **CPU** | 1 cœur x86‑64 / ARM64 / ARMv7 |
| **RAM** | **1 Go** (512 Mo possible avec un peu de swap, sans marge) |
| **Disque système** | **8 Go** (OS + `/opt/homevault` + dépendances APT : `build-essential`, `ffmpeg`, `smartmontools`, `mdadm`…) |
| **Disques de données** | selon vos besoins, ajoutés séparément |
| **Réseau** | 1 interface Ethernet |

Ce que couvre cette configuration : comptes et 2FA, code de récupération,
partages SMB / NFS / SFTP, explorateur de fichiers (copie, corbeille,
upload), quotas de transfert, pare‑feu `ufw`, supervision SMART, RAID
`mdadm`, tableau de bord, journal système, réseau, sauvegardes `restic`,
Cloud Sync `rclone`, Syncthing, téléchargeur.

Empreinte mémoire typique du processus au repos : **40–120 Mo résident**
(le service systemd est plafonné à `MemoryHigh=1536M` / `MemoryMax=2G`).

---

## 3. Configuration recommandée

**Cible : usage NAS complet — partages, RAID, chiffrement, antivirus,
notifications, quelques applications.**

| Ressource | Recommandé |
|---|---|
| **CPU** | 2 cœurs x86‑64 (4 cœurs si compression/sauvegardes fréquentes) |
| **RAM** | **4 Go** |
| **Disque système** | **32 Go SSD** (hors disques de données) |
| **Disques de données** | SSD/HDD dédiés ; 2+ disques pour RAID/redondance |
| **Réseau** | Gigabit Ethernet |

Marge incluse pour : `clamd` (antivirus ClamAV, ~1–1,5 Go résidents pour
la base de signatures — **le poste mémoire le plus lourd**),
l'anti‑ransomware, le cache de stockage, les opérations d'archive de
l'explorateur, et plusieurs sauvegardes `restic` en parallèle.

---

## 4. Besoins des fonctions optionnelles

Chaque fonction ci‑dessous est **désactivée par défaut** ou sans effet tant
qu'elle n'est pas configurée. Ajoutez son coût à la configuration de base.

| Fonction | RAM supplémentaire | Disque | Notes |
|---|---|---|---|
| **Antivirus (ClamAV)** | **+1 à 1,5 Go** | +2 Go (signatures) | `clamd` garde toute la base en mémoire |
| **ZFS** (disques virtuels) | **+1 Go minimum**, idéalement +1 Go par To de pool | — | 64 bits obligatoire ; **8 Go de RAM recommandés** pour des pools multi‑To. Btrfs et mdadm n'ont pas ce besoin |
| **Chiffrement LUKS2 (cryptsetup)** | négligeable | — | Coût CPU à l'écriture/lecture |
| **Assistant IA (Ollama)** | **+4 Go minimum** | +2 à 30 Go selon le modèle | Aucun modèle téléchargé automatiquement. gemma3:1b ≈ 2 Go RAM ; modèle 7–8B ≈ 8–16 Go RAM. GPU facultatif |
| **Docker** (applications) | selon les conteneurs | +1 Go (moteur) + images | À ne pas installer si non utilisé — l'app reste alors simplement inactive |
| **Sauvegardes `restic`** | +100 à 500 Mo pendant un job | dépôt séparé | Pic mémoire proportionnel au volume |
| **Syncthing / rclone** | +100 à 300 Mo par service actif | — | Démons séparés |
| **Transcodage média (`ffmpeg`)** | ponctuel, CPU‑intensif | — | Installé par défaut ; usage à la demande |

---

## 5. Exemples de plateformes

| Plateforme | Adaptée à |
|---|---|
| Raspberry Pi 4 (4 Go) / Pi 5, disque USB‑SATA SSD | Config minimale à recommandée, **sans** ZFS ni IA |
| Mini‑PC x86 (N100/N150, 8–16 Go, NVMe) | Config recommandée **avec** ZFS, antivirus, quelques apps Docker |
| Serveur / tour avec ECC, 16–32 Go, plusieurs disques | ZFS multi‑To, RAID, antivirus, assistant IA local |
| VM (Proxmox, KVM) : 2 vCPU, 4 Go, disque 32 Go | Config recommandée hors IA |

---

## 6. Notes d'installation

- L'installateur (`installer/install.sh`) tire une chaîne de compilation
  (`build-essential`, `libsqlite3-dev`) et `ffmpeg` **sans
  `--install-recommends`** — prévoir la place sur le disque système.
- Aucun **swap** n'est créé automatiquement : en créer sur les
  configurations à 1 Go de RAM ou moins.
- Le service ne fixe **pas** de quota CPU par défaut (le débit des
  sauvegardes profite de tous les cœurs) ; `CPUQuota=` peut être ajouté
  dans `installer/homevault.service` si HomeVault partage la machine avec
  d'autres services.
- Les limites mémoire du service (`MemoryHigh` / `MemoryMax`) sont des
  points de départ prudents : les ajuster avec `systemd-cgtop` sur votre
  matériel réel avant de s'y fier en production.
