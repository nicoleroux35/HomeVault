# HomeVault

[![CI](https://github.com/nicoleroux35/HomeVault/actions/workflows/ci.yml/badge.svg)](https://github.com/nicoleroux35/HomeVault/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

HomeVault est un serveur personnel qui vous permet d'administrer, stocker et
partager vos données depuis une interface web simple et accessible. Il se
compose d'un backend écrit en Rust et d'un frontend moderne en Svelte.

## Fonctionnalités

- **Tableau de bord** : surveillez l'activité de votre serveur (processeur,
  mémoire, disques, réseau, capteurs).
- **Utilisateurs et groupes** : gérez les comptes, leurs droits et leurs
  dossiers personnels.
- **Stockage** : gérez vos disques, partitions et espaces de stockage.
- **RAID** : créez et surveillez des volumes RAID (mdadm).
- **Partages** : exposez vos fichiers en SMB, NFS, FTP et WebDAV, avec une
  gestion fine des permissions.
- **Explorateur de fichiers** : parcourez, recherchez, envoyez et organisez
  vos fichiers depuis le navigateur, avec une corbeille intégrée.
- **Sauvegardes** : planifiez et restaurez des sauvegardes avec restic.
- **Synchronisation cloud** : reliez des stockages distants via rclone.
- **Réseau** : configurez vos interfaces, le Wi-Fi et diagnostiquez la
  connexion.
- **Sécurité** : antivirus (ClamAV), protection anti-ransomware et pare-feu
  (ufw).
- **Docker et catalogue d'applications** : conteneurs, piles Compose et
  catalogue d'apps en un clic.
- **Machines virtuelles** : VMs et conteneurs Incus, images cloud, console.
- **VPN** : WireGuard et OpenVPN, synchronisation Syncthing, téléchargeur
  aria2.
- **Assistant IA** : chat local via Ollama, base de connaissances RAG.
- **Santé, journal et quotas** : supervision système, journal d'événements
  et quotas de transfert.
- **Terminal** : accédez à un terminal sécurisé directement dans le navigateur.
- **Paramètres** : personnalisez l'apparence, les notifications, les mises à
  jour et plus encore.

## Prérequis

- Système **Debian 10+** ou **Ubuntu 20.04+**
- Architecture **x86_64**, **aarch64** ou **armv7l**
- Accès **root** (sudo) pour l'installation

Configuration matérielle minimale et recommandée détaillée (RAM, disque,
coût des fonctions optionnelles ZFS / antivirus / IA) : voir
[`HARDWARE.md`](HARDWARE.md).

## Installation

Installez HomeVault et toutes ses dépendances en une seule commande.
Choisissez `curl` **ou** `wget` selon votre préférence :

Avec `curl` :

```bash
curl -fsSL https://codeberg.org/nicleroux35/HomeVault-Svelt/raw/branch/main/installer/install.sh | sudo bash
```

Avec `wget` :

```bash
wget -qO- https://codeberg.org/nicleroux35/HomeVault-Svelt/raw/branch/main/installer/install.sh | sudo bash
```

> Ajustez la branche (`main`) si besoin. Le script se déroule en **5 étapes** :
>
> 1. **Étape 1** — vérifie la compatibilité : `sudo`, distribution (Debian 10+
>    ou Ubuntu 20.04+), architecture (`x86_64`, `aarch64`, `armv7l`) et espace
>    disque.
> 2. **Étape 2** — vérifie puis installe les paquetages système (les paquets
>    essentiels sont bloquants, les paquets optionnels dégradent en avertissement).
> 3. **Étape 3** — télécharge l'archive compressée de HomeVault, la décompresse
>    dans `/opt/homevault`, installe le service systemd
>    (`/etc/systemd/system/homevault.service`) et configure l'automontage des
>    disques externes.
> 4. **Étape 4** — vérification globale : paquetages installés, services à
>    démarrer, démarrage réel de HomeVault (réponse HTTP sur le port) et
>    emplacement de l'application.
> 5. **Étape 5** — message de fin **en anglais** confirmant l'installation et le
>    démarrage, avec l'adresse **locale** (`http://<ip-locale>:8080`) et
>    l'adresse **internet** (`http://<ip-publique>:8080`, sous réserve d'un
>    routage de port vers cette machine).
>
> Tout est journalisé dans `/var/log/homevault-install.log`.

L'archive applicative est téléchargée depuis l'URL `HV_RELEASE_URL`
(par défaut : `…/installer/release/homevault-latest.tar.gz`, produite par
`installer/Makefile`). Vous pouvez la rediriger sans éditer le script :

```bash
sudo HV_RELEASE_URL=https://miroir/homevault.tar.gz bash -c "$(curl -fsSL https://codeberg.org/nicleroux35/HomeVault-Svelt/raw/branch/main/installer/install.sh)"
```

Méthode alternative (depuis une copie locale du dépôt, avec une archive déjà
téléchargée) :

```bash
sudo bash installer/install.sh /chemin/vers/homevault.tar.gz
```

Après l'installation, ouvrez votre navigateur à l'adresse affichée en fin
d'exécution. Lors du premier lancement, un assistant de configuration vous
guidera pour créer le compte administrateur. Commandes utiles :
`systemctl status homevault` et `journalctl -u homevault -f`.

## Structure du projet

| Dossier      | Contenu                                              |
| ------------ | ---------------------------------------------------- |
| `src/`       | Code backend Rust (serveur Axum, handlers, services) |
| `frontend/`  | Interface web Svelte + Vite                          |
| `sqlx/`      | Migrations de la base de données SQLite              |
| `installer/` | Script d'installation et fichiers systemd            |
| `static/`    | Ressources statiques servies par le backend          |
| `docs/`      | Documentation complémentaire                         |
| `tests/`     | Tests d'intégration Rust + suites E2E Playwright     |
| `catalog/`   | Catalogue d'applications Docker                      |
| `scripts/`   | Scripts utilitaires                                  |

## Développement

```bash
cargo test                         # backend : unit + integration
cd frontend && npm run check && npm run build   # frontend : types + build
npx playwright test tests/e2e/<screen>/         # E2E d'un écran
```

Contributions : voir [`CONTRIBUTING.md`](CONTRIBUTING.md). Sécurité : voir
[`SECURITY.md`](SECURITY.md). Changements : voir [`CHANGELOG.md`](CHANGELOG.md).

## Roadmap

L'état d'avancement, les bugs connus non corrigés et les fonctionnalités
incomplètes sont suivis dans [`ROADMAP.md`](ROADMAP.md).

## Langues

L'interface est disponible en **français**, **anglais**, **allemand** et
**espagnol**. La langue peut être changée à tout moment dans les paramètres.

## Licence

Distribué sous licence [MIT](LICENSE). Copyright (c) 2026 HomeVault Team.
