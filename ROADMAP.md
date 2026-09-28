# Roadmap HomeVault

Dernière mise à jour : 2026-07-21 — version actuelle : 1.0.0

## État actuel

HomeVault couvre aujourd'hui : tableau de bord, utilisateurs/groupes,
stockage (disques/partitions/pools ZFS/Btrfs/chiffrement LUKS2), RAID
(mdadm), partages (SMB/NFS/FTP/WebDAV/SFTP), explorateur de fichiers,
sauvegardes (restic), synchronisation cloud (rclone), réseau, sécurité
(antivirus/anti-ransomware/pare-feu), terminal web, VPN (WireGuard/
OpenVPN), téléchargeur (aria2), Syncthing, assistant IA (Ollama), et une
suite de widgets de bureau.

Chaque page listée ci-dessus a fait l'objet d'un audit "conditions
réelles" (backend réel, session réelle, navigateur réel quand possible)
documenté dans `CLAUDE.md`, qui sert de référence historique — toute
modification dans ces zones nécessite une confirmation explicite avant
d'être appliquée.

> **Note sur la fiabilité de ce fichier :** la première version de cette
> roadmap (2026-07-21, avant la passe de vérification ci-dessous) avait
> été compilée en relisant le texte de `CLAUDE.md` sans re-vérifier le
> code réel. Une seconde passe le même jour a montré que **6 des 7 points
> listés comme "non corrigés" étaient en réalité déjà résolus** dans le
> code (correctifs déjà présents mais jamais reflétés dans le texte de
> `CLAUDE.md`). Les corrections correspondantes ont été ajoutées en
> addenda datés directement dans `CLAUDE.md`. Leçon retenue : toujours
> vérifier contre le code actuel avant de faire confiance à un texte
> d'audit, même dans ce fichier — voir "Processus" plus bas.

## Priorité moyenne — fonctionnalité incomplète

Aucune pour le moment — le seul point restant (i18n du shell desktop)
a été corrigé le 2026-07-21, voir "Résolu" plus bas.

## Idées / backlog

- Continuer d'auditer "en conditions réelles" (backend réel, pas de
  mocks) toute nouvelle page avant de la considérer comme figée, en
  suivant la méthodologie déjà appliquée à chaque section de `CLAUDE.md`.

## Résolu depuis la première version de ce fichier (vérifié 2026-07-21)

Pour mémoire — ces points étaient listés comme "non corrigés" dans la
version précédente de cette roadmap et se sont révélés déjà résolus à la
vérification :

- Faille de session sur `handle_group_add_member` (Groups) — déjà
  corrigée, valide la session pour tout appelant.
- `PermissionService::grant_permission` en mode récursif — déjà corrigé
  (ordre des arguments `setfacl -d -m`).
- Option "rw" sans bit d'exécution sur les partages — déjà corrigé
  (exécution forcée sur les répertoires).
- UPS/SNMP sans interface — déjà résolu, `UpsCard.svelte`/
  `SnmpCard.svelte` existent et sont montés dans `Settings.svelte`.
- Ancienne implémentation Settings orpheline — les fichiers
  (`SettingsLayout.svelte`, etc.) n'existent plus du tout.
- Handlers HTML morts dans `dashboard.rs` — les fonctions elles-mêmes ont
  été supprimées.

**Corrigés le 2026-07-21, sur demande explicite ("corrige les 3 points") :**

- **i18n du shell desktop** — les ~19 noms/descriptions d'applications
  (`appDefs`/`apps` dans `Desktop.svelte`) et l'index de recherche globale
  passent maintenant par des clés dédiées `desktop.app_<id>_name`/`_desc`
  (4 langues), suivant le schéma déjà établi pour le catalogue Docker et
  les widgets. L'entrée "Assistant IA" de la recherche, seule à être
  restée en français pur pendant que le reste était en anglais (le bug
  "mélange EN/FR" exact décrit dans `CLAUDE.md`), est corrigée elle aussi.
- **Vue d'ensemble des quotas de transfert** — nouveau bouton "Tous les
  quotas" dans l'en-tête de la page Users, ouvrant une modale listant tous
  les quotas configurés (utilisateurs et groupes) via
  `api.listTransferQuotas()` (nouvelle méthode, lit l'endpoint de liste
  déjà existant). Vérifié en direct : quota réel configuré pour un
  utilisateur, affiché correctement dans le tableau.
- **Nettoyage mineur** — les clés i18n mortes (`two_factor`,
  `backup_codes*`, anciens libellés `general`/`email`/`network`/`about`)
  supprimées des 4 fichiers TOML. La clé `security` de ce même groupe a
  été **conservée** : vérification a montré qu'elle est en réalité utilisée
  par `Settings.svelte` comme libellé de la catégorie "Sécurité" de son
  propre menu latéral — elle n'était donc pas morte, contrairement à ce
  que `CLAUDE.md` laissait penser.

## Processus

- Toute modification d'une zone listée comme gelée dans `CLAUDE.md`
  nécessite une confirmation explicite avant d'être appliquée.
- Les nouveaux rapports de vérification vont en addenda datés dans
  `CLAUDE.md`, pas dans un fichier séparé (règle "Keep the repository
  clean" en tête de `CLAUDE.md`).
- **Avant d'ajouter un point à ce fichier depuis le texte de `CLAUDE.md`,
  vérifier contre le code actuel (lecture directe du fichier concerné,
  pas seulement `grep`/mémoire) qu'il est toujours d'actualité** — c'est
  précisément ce qui a été sauté lors de la première version de ce
  fichier et qui a produit une roadmap fausse à 85 %.
- Ce fichier doit être mis à jour à chaque bug trouvé-mais-non-corrigé ou
  fonctionnalité incomplète identifiée lors d'un futur audit, et purgé des
  points qui finissent par être corrigés ou abandonnés.
