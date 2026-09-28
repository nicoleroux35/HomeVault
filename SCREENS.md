# HomeVault — Liste des écrans et fonctionnalités

---

## 1. Login (`/login`) TESTS

**Fonctionnalités :**
- Formulaire username/mot de passe avec afficher/masquer le mot de passe
- Coche « Se souvenir de moi »
- Lien « Mot de passe oublié » — demande un token de réinitialisation
- Flux de réinitialisation de mot de passe (via lien avec token)
- Authentification à deux facteurs (2FA) — saisie du code TOTP 6 chiffres
- Modal d'erreur avec messages traduits (identifiants invalides, compte verrouillé, rate limiting, TOTP invalide, timeout réseau)
- État de connexion impossible avec bouton de reconnexion
- Détection premier lancement — affiche l'Assistant de configuration à la place

---

## 2. Assistant de configuration (Setup Wizard) TESTS

**Affiché dans la page Login au premier lancement.**

**Fonctionnalités (4 étapes) :**
- Étape 0 — Accueil : présentation des fonctionnalités (comptes, chiffrement, sécurité)
- Étape 1 — Compte : username, nom complet optionnel, sélection d'avatar (DiceBear)
- Étape 2 — Mot de passe : saisie + confirmation avec indicateur de force en temps réel et checklist des exigences
- Étape 3 — Vérification : résumé du compte, vérification de disponibilité du username côté serveur
- Inscription 2FA (après création du compte) : affichage du secret TOTP, QR code, vérification, codes de secours
- Stepper de progression avec animations

---

## 3. Bureau (Desktop) — `/desktop`

**Coquille principale de l'application.**

### Barre supérieure (Topbar)
- Chip utilisateur (avatar + nom) ; menu : Mon compte, Paramètres, Déconnexion
- Affichage date/heure localisé (horloge + date)
- Bouton **Réseau** (icône filaire/WiFi + libellé) avec popover IP locale + IP publique
- Bouton **Widgets** : ouvre l'application Widgets
- Bouton **Notifications** (icône + libellé) avec compteur et panneau déroulant —
  le compteur est une pastille **ancrée au coin du bouton**, pas au libellé : elle
  vit dans la bande au-dessus de la ligne de texte, donc un « 99+ » s'élargit
  dans le vide sans jamais recouvrir le libellé
- Bouton **Alimentation** : Déconnexion, Redémarrer le serveur, Éteindre le serveur

Les quatre boutons sont des puces **icône + libellé** (maquette DSM) : libellés
visibles à partir de 901px, icône seule en dessous — la cible tactile reste
44×44 dans les deux cas. Ordre : Réseau, Widgets, Notifications, Alimentation.

### Quai d'applications (Dock)
- Apps épinglées, dans l'ordre du dock : Tableau de bord, Terminal, Docker,
  Santé, Assistant IA, Applications (le lanceur), Paramètres
- Apps en cours d'exécution / minimisées avec animation de restauration

### Barre de tâches (Activity taskbar)
- Bouton de dock transitoire (visible uniquement pendant une activité) avec
  pastille = nombre de tâches actives ; clic → panneau au-dessus du dock
- Une ligne par tâche : nom, barre de progression, % (barre coulissante si
  progression indéterminée) — affichage seul
- Sources : téléchargements aria2, copies/déplacements Explorateur, sauvegardes
  Restic, scans antivirus, cache mover, install/màj Docker, clonage/récupération
  Rescue, réindexation RAG, téléchargement modèle IA, sync cloud
- Alimenté par `GET /api/activity` (poll 2 s) ; store `activityStore.ts`

### Recherche globale (Ctrl+K)
- Barre de recherche avec correspondance floue sur les noms/descriptions des apps
- Navigation au clavier (flèches + Entrée)

### 27 applications disponibles

| # | Application | Catégorie | Description |
|---|---|---|---|
| 1 | Dashboard | Système | Vue d'ensemble et statistiques en temps réel |
| 2 | Stockage (Storage) | Stockage | Disques, volumes et pools |
| 3 | RAID Arrays | Stockage | Gestion des baies RAID |
| 4 | Explorateur (Explorer) | Stockage | Navigation dans les fichiers partagés |
| 5 | Docker | Système | Conteneurs et images |
| 6 | Réseau (Network) | Réseau | Interfaces et routage |
| 7 | VPN | Réseau | Accès WireGuard et OpenVPN |
| 8 | Sauvegarde (Backup) | Stockage | Snapshots et restaurations |
| 9 | Utilisateurs | Système | Comptes et permissions |
| 10 | Groupes | Système | Groupes d'utilisateurs et rôles |
| 11 | Quotas | Système | Aperçu en lecture seule des quotas de transfert |
| 12 | Partages (Shares) | Stockage | Partages réseau |
| 13 | Cloud Sync | Réseau | Stockage cloud distant |
| 14 | Syncthing | Réseau | Synchronisation continue de fichiers |
| 15 | Santé (Health) | Surveillance | État du système et alertes |
| 16 | Assistant IA (Chat) | Productivité | Conversation avec l'assistant IA |
| 17 | Télécharger (Downloader) | Productivité | Gestion des téléchargements |
| 18 | Rescue | Système | Récupération et réparation |
| 19 | Widgets | Système | Ajouter/supprimer des widgets du bureau |
| 20 | Journal système | Surveillance | Journaux système centralisés et événements |
| 21 | Terminal | Système | Terminal PTY complet |
| 22 | Paramètres | Système | Configuration du serveur |
| 23 | Mon compte | Système | Profil et sécurité personnel |
| 24 | Stockage (Storage) | Stockage | Disques, volumes, chiffrement, cache |
| 25 | RAID | Stockage | Tableaux RAID |
| 26 | Parts/Protocoles | Stockage | SMB, NFS, FTP, WebDAV, SFTP |
| 27 | Machines virtuelles (Virtual Machines) | Système | VMs et conteneurs système via Incus — assistant guidé, console shell, instantanés |

### Widgets de surveillance flottants (11 panneaux)

Chaque widget est un panneau redimensionnable, déplaçable, avec persistance de position.

| # | Widget | Données affichées | Rafraîchissement |
|---|---|---|---|
| 1 | Moniteur de ressources | CPU, RAM, température, trafic réseau | 2s |
| 2 | Moniteur RAID | État des baies RAID | 5s |
| 3 | Moniteur Sécurité | Antivirus, pare-feu, anti-ransomware | 15s |
| 4 | Moniteur VPN | Connexions WireGuard/OpenVPN | 15s |
| 5 | Moniteur Downloader | Téléchargements en cours (aria2) | 5s |
| 6 | Moniteur Syncthing | Appareils et dossiers synchronisés | 15s |
| 7 | Moniteur Calendrier | Grille du mois en cours | — |
| 8 | Moniteur Docker | Conteneurs en cours | 10s |
| 9 | Moniteur Météo | Météo via Open-Meteo (navigateur) | 10 min |
| 10 | Moniteur Sessions actives | Nombre d'utilisateurs connectés | 15s |
| 11 | Moniteur Santé système | Compteur healthy/warning/critical | 15s |

### Gestion des fenêtres d'apps
- Fenêtres avec barre de titre, boutons minimiser/maximiser/fermer
- Glisser-déposer, redimensionnement, snap au bord
- Géométrie initiale adaptative (auto-maximise sous 820px)

---

## 4. Mon compte (Account) TESTS

**Fonctionnalités :**
- Affichage du profil : avatar, username, nom complet, email, rôle, date de création
- Régénération d'avatar
- Édition du profil (nom complet, email)
- Changement de mot de passe (invalide toutes les sessions → déconnexion automatique)
- Inscription 2FA : secret TOTP, QR code, vérification, codes de secours
- Désactivation 2FA (avec confirmation du mot de passe)
- Affichage des codes de secours

---

## 5. Utilisateurs (Users)

**Anatomie maître/détail (2026-09-14) :** rail de comptes à gauche, volet détail à
droite ; en fenêtre étroite les deux volets s'échangent (seuil ≈ 1065px de fenêtre).

**Fonctionnalités :**
- Rail : bouton « Nouvel utilisateur », recherche, filtres (tous/actifs/verrouillés/admin),
  compteur, une ligne par compte (avatar, nom, badge de rôle, `@username`, état verrouillé/actif)
- Pied du rail : bouton « Vue des quotas » → ouvre l'app Quotas
- Création d'utilisateur : username, mot de passe, nom complet, email, rôle, groupes, dossier personnel
- Volet détail en 6 cartes, chacune dimensionnée par son contenu :
  1. **Identité** — username en lecture seule (clé du compte), nom complet, email
  2. **Rôle et groupes** — sélecteur de rôle + puces de groupes en lignes pleine largeur
  3. **Dossier personnel** — activation, disque ou dossier explicite, curseur de taille + barre alloué/libre
  4. **Quotas de transfert** — upload/download (bascule + valeur + unité + barre d'usage) et bouton « Tous les quotas »
  5. **Sécurité** — 2FA (état + réinitialisation), changement de mot de passe
  6. **Actions** — verrouiller/déverrouiller, supprimer (option conserver/supprimer le dossier)
- Appartenance aux groupes : ajout/retrait ; `homevault_admin` en puce lecture seule dérivée du rôle
- Changement de mot de passe (admin pour d'autres utilisateurs)
- Suppression d'utilisateur (purge dossier personnel, jail SFTP, sessions, ligne de quota)
- Activation/désactivation de la 2FA (récupération admin)

---

## 6. Quotas (Quotas)

**Écran en lecture seule — aucune écriture, aucun bouton d'édition.**

**Fonctionnalités :**
- Une ligne par sujet configuré : utilisateurs d'abord, puis groupes, triés par nom
- Filtres : tous / utilisateurs / groupes
- Par ligne : type de sujet, nom, date de dernière mise à jour
- Par direction (upload, download) : octets utilisés / limite + barre de progression réelle
- Une direction désactivée affiche « Illimité » (pas de limite, et non « zéro autorisé »)
- Ligne de compteur : nombre de sujets, dont combien portent une limite
- Bouton « Gérer dans Utilisateurs » → ouvre l'app Utilisateurs (événement `hv-open-app`)

**Source de données :** `GET /transfer-quotas/{subject_type}` (une requête par type),
le même endpoint que la modale « Tous les quotas » de l'écran Utilisateurs.
L'écran est malgré tout réservé aux admins, comme Utilisateurs.
L'édition des quotas reste dans la carte « Quotas de transfert » d'Utilisateurs
et dans l'écran Groupes.

---

## 7. Groupes (Groups)

**Fonctionnalités :**
- Liste des groupes avec recherche, compteur de membres
- Création de groupe : nom + membres initiaux
- Gestion des membres : ajout/retrait d'utilisateurs avec recherche
- Suppression de groupe (groupe `homevault_admin` protégé)
- Gestion des quotas de transfert par groupe

---

## 8. Dashboard

**Fonctionnalités (rafraîchissement toutes les 5s) :**
- Jauge de score de santé (SVG circulaire)
- Ligne de statut LED : CPU, RAM, Disque, Réseau
- Panneau CPU : modèle, fréquence, bars par cœur, température, nombre de cœurs/threads
- Panneau Mémoire : barre d'utilisation, type/vitesse RAM, swap
- Panneau GPU (un par GPU) : modèle, type (NVIDIA/AMD), utilisation, VRAM, fréquence, température, puissance
- Panneau Carte mère : fabricant, modèle, version/date BIOS
- Panneau Stockage : bars d'utilisation des systèmes de fichiers
- Panneau Réseau : par interface — IP, MAC, vitesse, driver, RX/TX + taux en temps réel
- Panneau Santé disques : comptage SMART healthy/total
- Températures capteurs matériels
- Bouton de rafraîchissement manuel

---

## 9. Stockage (Storage)

**5 onglets :**

### Disques physiques
- Cartes de disques détaillées : nom, taille, vendeur, modèle, numéro de série, type, statut SMART, température
- Modal avec table des partitions et diagramme circulaire
- Tests SMART (court/long/conveyance)

### Partitions
- Liste de toutes les partitions avec point de montage, système de fichiers, taille, utilisé, disponible, pourcentage

### Amovibles
- Liste des disques USB/amovibles avec montage/démontage

### Chiffrement (LUKS2)
- Volumes chiffrés (statut déverrouillage, mapper, point de montage)
- Création de volume chiffré (périphérique, système de fichiers, étiquette, phrase secrète)
- Verrouillage/déverrouillage, suppression

### Niveau cache (Cache Tier)
- Pool de cache sélectionnable (y compris RAID)
- Planification du déplacement (cron, âge)
- Mode cache par partage
- Historique des déplacements

---

## 10. RAID

**Fonctionnalités :**
- Liste des baies RAID : nom, niveau, statut (healthy/dégradé/échoué), nombre de disques, taille/utilisé/libre
- Création de RAID (assistant 3 étapes) : type (0/1/5/6/10), sélection des disques, nom + système de fichiers
- Suppression de baie RAID
- Reconstruction d'une baie dégradée : sélection du disque de remplacement

---

## 11. Santé (Health)

**Fonctionnalités :**
- 8 contrôles de santé affichés dans l'ordre :
  - Authentification, Stockage, Mémoire, Espace disque, SMART disques, Réseau, Services, Températures
- Chaque contrôle : icône de statut, label traduit, message, détails extensibles
- Badge de statut global (healthy/warning/critical/unknown)
- Bouton de rafraîchissement

---

## 12. Terminal

**Fonctionnalités :**
- Terminal PTY complet (xterm.js)
- Connexion WebSocket avec ticket d'authentification
- Reconnexion automatique
- Redimensionnement adaptatif
- Liens cliquables dans les sorties
- Indicateur de statut (Connecté/Déconnecté)

---

## 13. Journal système (Journal)

**Fonctionnalités :**
- Sélecteur de source : `app` (journaux d'événements métier), `system` (journald), fichiers texte (syslog/auth.log/kern.log/daemon.log)
- Liste des entrées : horodatage, unité, priorité, PID, message
- Filtrage par unité, par priorité, recherche texte
- Rafraîchissement automatique toggle
- Effacement des journaux par source avec confirmation
- Paramètres de nettoyage programmé (activation, intervalle en jours)

---

## 14. Réseau (Network)

**2 onglets :**

### Ethernet
- Cartes d'interface : nom, type, MAC, statut, IPv4/IPv6, MTU, vitesse, duplex, driver, RX/TX + taux
- Configuration : DHCP/Statique, IP/netmask/passerelle/DNS manuels
- Résumé de connexion : IP locale, passerelle, DNS, IP publique
- Diagnostics : accessibilité passerelle, test DNS, accessibilité Internet, latence

### WiFi
- Liste des réseaux : SSID, force du signal, fréquence, canal, sécurité
- Scan des réseaux
- Connexion/déconnexion
- Affichage de l'état WiFi

---

## 15. VPN

**2 onglets :**

### WireGuard
- Statut serveur : clé publique, port d'écoute, état
- Liste des pairs (appareils) : nom, IP, date de création, statut, dernier handshake
- Ajout d'appareil : génération de config + QR code
- Revisualisation de config/QR pour un pair existant
- Suppression de pair
- Démarrage/Arrêt du serveur

### OpenVPN
- Liste des connexions : nom, statut, interface, IP locale/distant
- Ajout de connexion : nom, fichier config, identifiants
- Suppression, Démarrage/Arrêt

---

## 16. Explorateur de fichiers (Explorer)

**Fonctionnalités :**
- Double vue : liste et grille
- Navigation par dossier : fil d'Ariane, dossier parent, entrées avec taille, permissions, propriétaire
- Barre latérale : disques physiques, amovibles, emplacements favoris
- Opérations sur fichiers :
  - Créer un dossier
  - Renommer
  - Supprimer (vers la corbeille)
  - Copier / Déplacer (avec sélecteur de dossier)
  - Téléverser des fichiers (avec barre de progression, quota-aware)
  - Télécharger (quota-aware)
- Prévisualisation de fichier texte
- Gestion des permissions (chmod)
- Recherche de fichiers
- Tri : par nom, taille, date de modification
- Système de corbeille : voir, restaurer, purger, vider
- Extraction d'archives (.zip/.tar/.gz/.bz2/.xz)
- Compression d'éléments sélectionnés

---

## 17. Parts/Protocoles (Shares)

**Fonctionnalités :**

### Liste des parts
- Liste : nom, chemin, type (SMB/NFS/FTP/WebDAV/SFTP), statut activé, commentaire
- Création de part : nom, chemin, type, commentaire

### Onglets de configuration (6 protocoles)
- **Shares** — CRUD, démarrage/arrêt des services, badges de statut
- **SMB** — workgroup, mode sécurité, version protocole, accès invité, options avancées
- **NFS** — réseaux, accès par défaut, version, root squash, options custom
- **FTP** — port, ports passifs, max clients, TLS, anonyme, chroot, message d'accueil
- **WebDAV** — port, préfixe chemin, SSL, auth, compression, verrouillage, méthodes autorisées
- **SFTP** — port, liste des bénéficiaires (utilisateur + parts + état clé), ajout d'utilisateur, ajout de clé publique, reconstruction des jails

### Gestion des permissions
- Voir les permissions actuelles (bénéficiaire, niveau r/rw, récursif)
- Accorder une permission (sélecteur utilisateur/groupe)
- Révoquer une permission
- Synchroniser les permissions

---

## 18. Cloud Sync (rclone)

**3 onglets :**

### Remotes
- Liste des remotes configurés (nom, type)
- Ajout de remote : sélection du fournisseur, nom, config brute
- Suppression de remote
- Visualisation de la config rclone

### Montages
- Liste des montages actifs : remote, point de montage, statut, PID
- Monter/démonter un remote

### Synchronisation
- Source (chemin local ou remote cloud)
- Destination (remote cloud ou chemin local)
- Direction : local→cloud, cloud→local, bidirectionnel
- Mode test à blanc
- Option suppression après sync
- Exécution et statut

---

## 19. Docker

**4 onglets :**

### Conteneurs
- Cartes de conteneurs : nom, image, statut, date de création, ports, mémoire/CPU, indicateurs d'available update
- Démarrage/Arrêt/Redémarrage
- Suppression
- Journaux (flux en temps réel)
- Édition du `docker-compose.yml` (conteneurs issus d'un projet Compose géré par HomeVault) : éditeur modal, validation de syntaxe, enregistrement, enregistrement + redéploiement
- Création : nom, image, ports, volumes (chemin via FolderPicker), variables d'environnement, réseaux
- Rafraîchissement toutes les 5s

### Images
- Liste des images : dépôt, tag, taille, date
- Téléchargement depuis Docker Hub (recherche + résultats)
- Suppression

### Réseaux
- Liste des réseaux : nom, driver, scope, subnet, passerelle, conteneurs connectés

### Compose / Catalogue
- Projets Compose : liste avec services et statut, démarrage/arrêt/redémarrage/suppression, édition du fichier compose
- Catalogue d'apps : installation/désinstallation en un clic (Plex, Jellyfin, Nextcloud, Pi-hole, Vaultwarden, Home Assistant, Grafana, etc.)

---

## 20. Syncthing

**3 onglets :**

### Vue d'ensemble
- Statut Syncthing : disponible, version, ID appareil, uptime
- Statistiques rapides
- Redémarrage/Pause/Reprise

### Appareils
- Liste des appareils : nom, ID, adresses, statut de connexion, version, complétion, données transférées
- Pause/Reprise par appareil

### Dossiers
- Liste des dossiers : label, chemin, statut, compteurs de fichiers, appareils connectés
- Pause/Reprise par dossier
- Scan manuel (déclenchement resync)

---

## 21. Télécharger (Downloader)

**Fonctionnalités :**
- Statistiques globales : vitesse download/upload, nombre actifs/en attente/stoppés
- Liste des tâches : GID, nom, statut, progression, vitesse, message d'erreur
- Ajout d'URL avec répertoire de destination
- Actions : pause, retrait, suppression (chaîne de fallback 3 étapes)
- Icônes de statut : actif, en pause, terminé, erreur, en attente
- Rafraîchissement toutes les 2s

---

## 22. Sauvegarde / Restic

**3 vues :**

### Liste des dépôts
- Cartes de dépôts : nom, URL, chemins source, politique de rétention, nombre de snapshots, dernière sauvegarde
- Création de dépôt : nom, URL, mot de passe, chemins source, patterns d'exclusion, tags, source (répertoire/snapshot pool), politique de rétention
- Suppression de dépôt

### Détail d'un dépôt

#### Snapshots
- Liste des snapshots : ID, horodatage, chemins, hostname, tags
- Navigation dans le contenu d'un snapshot
- Restauration avec cible (FolderPicker), mode test à blanc

#### Tâches (Jobs)
- Historique des tâches : statut, fichiers, données, durée, erreurs
- Déclenchement d'une nouvelle sauvegarde
- Détails de tâche

#### Planification
- Sauvegardes planifiées : expression cron, activation, dernière/prochaine exécution
- Création/édition/suppression de planifications

### Politiques de rétention
- Nombre max, âge max (jours), taille max (Go)

---

## 23. Rescue

**4 outils (mode Simple/Avancé pour chacun) :**

### ddrescue
- Clonage/récupération de disques
- Sélection source/destination (disque ou fichier image)
- Options avancées : taille de bloc, passes de retry, I/O direct, inverse
- Suivi de progression SSE

### TestDisk
- Analyse et reconstruction de partitions
- Sélection du périphérique
- Options : recherche profonde, chemin de log, mode forcé

### PhotoRec
- Récupération de fichiers
- Sélection du périphérique/répertoire de sortie
- Options : sélection de types de fichiers, conserver les fichiers corrompus, taille max

### mdadm
- Diagnostic et réparation de baies RAID
- Nom de la baie, diagnostic affiché
- Options : type d'action (repair/assemble), forcé, verbose
- Réparation avec progression SSE

---

## 24. Assistant IA (Chat)

**Fonctionnalités :**
- Interface de conversation avec bulles de messages
- Réponses en streaming (SSE) avec bouton annuler
- Sélecteur de modèle
- Historique des conversations (localStorage)
- Copie des messages
- Régénération de la réponse
- Nouvelle conversation
- Rendu Markdown dans les réponses
- Prompt système localisé

---

## 25. Widgets

**Fonctionnalités :**
- Catalogue de 11 widgets de surveillance
- Chaque widget : icône, couleur, nom et description traduits
- Toggle d'activation/désactivation (persisté)
- Boutons « Tout afficher » / « Tout masquer » / « Réinitialiser »

**Widgets disponibles :**
1. Moniteur de ressources (CPU/RAM/Temp/Réseau)
2. Moniteur RAID
3. Moniteur Sécurité
4. Moniteur VPN
5. Moniteur Downloader
6. Moniteur Syncthing
7. Moniteur Calendrier
8. Moniteur Docker
9. Moniteur Météo
10. Moniteur Sessions actives
11. Moniteur Santé système

---

## 26. Paramètres (Settings)

**8 catégories dans la barre latérale** (regroupées 2026-09-09 depuis 12 onglets plats, re-thémées 2026-09-14 d'après la maquette Figma « Settings Screen ») :

1. **Général** — Nom du serveur, langue (EN/FR/ES/DE), fuseau horaire, démarrage automatique
2. **Sécurité** (4 cartes intégrées) :
   - **HTTPS / TLS** — Activation du listener HTTPS intégré (port, certificat auto-signé ou fourni, redirection HTTP→HTTPS, infos certificat, régénération) ; s'applique après redémarrage
   - **Antivirus (ClamAV)** — Statut, scans planifiés, exécution, historique, menaces, quarantine, mises à jour de définitions
   - **Anti-Ransomware** — Activation/désactivation, honeypot, seuils, mode réponse auto, chemins protégés, historique
   - **Pare-feu (ufw)** — Activation/désactivation avec prévisualisation, règles (port/protocol/action/source), protection des règles obligatoires
3. **Apparence** — Fond d'écran (15 présets)
4. **Widgets** — Moniteurs actifs (les 11 moniteurs réels, pilotés par `monitorStore` → `/api/monitor-layout`) + intervalle de rafraîchissement (2/5/10/30/60 s, stocké dans le même blob) + widget Météo (localisation Open-Meteo, champs d'affichage) — la Météo a été déplacée d'Apparence le 2026-09-09
5. **Notifications** — Serveur e-mail/SMTP (serveur, port, identifiants, TLS, test d'envoi) + alertes (seuil, canaux) + webhook
6. **Assistant IA** — Modèles Ollama, modèle actuel, recommandations selon RAM/GPU, installation rapide
7. **Supervision** (2 cartes) :
   - **Monitoring UPS** — Liste des UPS : batterie/autonomie/tension/charge/statut
   - **SNMP** — Communauté, version, info système, interfaces, requêtes stockage
   - _(la carte « Quotas fichiers » par groupe a été supprimée le 2026-09-09 — les quotas se gèrent depuis Utilisateurs/Groupes)_
8. **Système** — Mises à jour logicielles (fréquence, version actuelle/dernière) + informations système (IP/port, plateforme, version) + gestion des données (export/import/reset des paramètres) + confidentialité (traitement local, cookies, hors-ligne)

---

## Résumé par catégorie

| Catégorie | Nombre d'écrans |
|---|---|
| Authentification / Configuration | 2 (Login, Assistant de configuration) |
| Bureau | 1 (Desktop + 11 widgets flottants + recherche + notifications) |
| Système | 6 (Dashboard, Utilisateurs, Quotas, Groupes, Mon compte, Terminal) |
| Stockage | 5 (Stockage, RAID, Explorateur, Parts/Protocoles, Sauvegarde) |
| Réseau | 4 (Réseau, VPN, Cloud Sync, Syncthing) |
| Surveillance | 2 (Santé, Journal système) |
| Productivité | 2 (Assistant IA, Télécharger) |
| Infrastructure | 2 (Docker, Rescue) |
| Configuration | 1 (Paramètres — 8 catégories) |
| **Total** | **26 écrans principaux** |
