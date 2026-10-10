# HomeVault — Suivi des tests

Croise les 27 écrans listés dans `SCREENS.md` avec trois niveaux de test
distincts, pour répondre à "qu'est-ce qui est vraiment testé, qu'est-ce
qui reste à tester" :

1. **Audit manuel live** — backend réel, session réelle, souvent
   Playwright piloté à la main. Documenté en prose/changelog daté dans
   `CLAUDE.md` et `docs/guides/` (chaque invariant documenté = un audit de ce
   type). C'est un
   audit ponctuel, pas une garantie de non-régression future.
2. **Tests Rust automatisés** — unitaires (`#[cfg(test)]` dans le fichier
   source, logique pure) et intégration (`tests/*.rs`, DB SQLite en
   mémoire + `tower::ServiceExt::oneshot`).
3. **Tests E2E Playwright** — `tests/e2e/<écran>/<écran>.spec.ts`, contre
   un vrai serveur, aucun mock.

Seuls les niveaux 2 et 3 protègent contre une régression future ; le
niveau 1 est une photo à une date donnée. Compilé le 2026-09-05, mis à
jour le 2026-09-06 (ajout des suites **Réseau**, **VPN**,
**Parts/Protocoles**, **Suite sécurité**, **Downloader**, **Stockage**,
**Sauvegarde**, **Cloud Sync**, **Syncthing**, **Chat**, **Explorateur**
et les tests Rust de **Terminal**), puis le **2026-09-07** (comblement des
trous dans les écrans déjà partiellement testés : nouveau
`tests/raid_integration.rs` (gardes d'auth) ; `storage_integration.rs`
étendu aux handlers `virtual_disk_*` ; `explorer_integration.rs` étendu à
`/roots` `/save` `/eject` `/snapshots` favoris et `?strategy=` ; tests
unitaires `transfer_quota.rs::check_quota`, `handlers/storage.rs::vd_name`,
`browser.rs` collision-strategy, `handlers/explorer.rs`
`kind_for_root`/`resolve_dest`), par lecture directe de `CLAUDE.md`, de
`tests/*.rs` et de `tests/e2e/**` (comptage réel des fichiers et du nombre
de `test(...)`/`#[test]`, pas une estimation).

**2026-09-07 (test réel « en conditions host » — 4 zones, 10 bugs
corrigés).** Sur demande explicite, les 4 zones dont les tests
automatisés s'arrêtaient à « rejet propre » faute d'accès `sudo`/matériel
— **Pare-feu (ufw)**, **Shares/SFTP**, **RAID/Storage (disques virtuels)**
et **Journal système** — ont été exercées de bout en bout contre le vrai
host (drop-in `sudoers` NOPASSWD temporaire, 6 loop devices, un vrai cycle
`ufw enable`/`disable` avec le port 22 protégé, un vrai `sshd`
`homevault-sftp`). **10 bugs trouvés et corrigés dans des fichiers
frozen** (autorisation explicite), plus les corrections de qualité de
données associées dans `PoolService::scan_pools` (device_count / point de
montage / taille mdadm / point de montage ZFS) — voir la section
« ## Test réel live 2026-09-07 » en fin de fichier. Chaque correctif a été
re-vérifié en direct sur le binaire reconstruit.

**2026-09-07 (2ᵉ passe « implémente ce qu'il reste » — même drop-in
NOPASSWD).** i18n **de/es** corrigés (accents, valeurs seulement) ;
**Rescue** exercé en réel pour la 1ʳᵉ fois (6 bugs frozen) ; **Storage/
LUKS** cycle complet en réel (2 bugs frozen) ; **Réseau** : reads
re-vérifiés + chemin d'application `src/network.rs` corrigé (`sudo` sur
les mutations `ip`/`dhclient`, `resolv.conf` via `sudo tee` sans clobber)
et vérifié live sur une interface `dummy` jetable. Détail dans la section
« ## 2026-09-07 (suite) » et les addenda datés de `CLAUDE.md`. Tests :
rescue unit 8/8, rescue_integration 28/28, storage_integration 24/24,
network unit +3/3, network_integration 23/23.

**2026-09-13 (transferts v2).** Durcissement des méthodes de transfert de
l'Explorateur et des liens de partage : `Range` HTTP (206) + digest
`X-Content-Sha256` sur les downloads, tickets courts (download mono-usage,
thumbnail multi-usage par répertoire) en remplacement du `?token=` dans les
URLs, upload chunké résumable (session → chunks → complete), quotas
verrouillés par-sujet (réservation + remboursement, anti-course), chemin
canonique `PathGuard` dans tous les handlers (TOCTOU symlink), `rename`
valide la destination, `/explorer/delete` admin-only, corbeille en
`copy_file_verified`, `fsync` du répertoire après chaque rename atomique,
nettoyage de la destination partielle d'un copy/move de dossier échoué,
rate-limit par token sur `/share/{token}` + token sans biais modulo. Détail
dans `docs/guides/explorer.md` et `docs/guides/transfer-quotas.md`. Tests :
`explorer_integration.rs` 49/49, `share_links_integration.rs` 12/12,
unitaires `media_ticket.rs` 5/5, `handlers/explorer.rs` 17/17,
`browser.rs` 11/11, lib 592/592, `svelte-check` 0 erreur. E2E Explorer à
relancer sur serveur frais.

**2026-09-14 (chrome du bureau — fidélité à la maquette DSM 7).** Dock et
topbar réalignés sur la maquette de référence : chaque pilule a désormais
**son propre** token de hauteur (`--hv-chrome-dock-h: 84px` débloqué de
`--hv-chrome-topbar-h: 62px`), matériau de chrome dédié
(`--hv-chrome-surface-border` / `-shadow`, partagé par les deux pilules et
leurs panneaux), tuiles de dock à 48px avec rebond au survol et tooltip
(`.hv-dock-item::before`, sur `--hv-tooltip-bg`), dégradés d'apps réharmonisés
sur l'échelle Tailwind 500→700 avec `--hv-app-<id>-c` = stop sombre du dégradé.
Aucun changement de comportement. Tests : `desktop.spec.ts` passe à
**51 tests / 14 catégories** — 8.3 réécrit (les deux hauteurs se vérifient
séparément, et « le dock est le plus haut » ne vaut qu'au-dessus du téléphone),
nouvelle catégorie 14 « Chrome geometry & material » (miroir JS `snapUtils.ts`,
pastille = stop sombre, pilules opaques, tooltip). 1.5 couvre enfin
`dock-terminal`, qui était rendu mais jamais asserté. Comptage vérifié via
`npx playwright test --list --project=firefox` — la méthode `grep -c 'test('`
documentée ci-dessous sur-comptait (elle attrapait les `.test(x)` des
assertions) ; elle est corrigée en `grep -cE '^\s*test\('`.

**2026-09-14 (boutons de topbar — puces icône + libellé).** La topbar adopte la
mise en page de la maquette DSM (node `1:24`) : les 4 boutons passent d'icône
seule à **icône + libellé** à partir de 901px, icône seule en dessous (la cible
reste 44×44 dans les deux cas — seule la largeur change). Ordre de la maquette :
Réseau, Widgets, Notifications, Alimentation. Décision autrefois refusée
(`docs/guides/desktop.md`) sur trois motifs dont deux étaient caducs : aucune
clé i18n neuve n'est nécessaire (les 4 libellés réutilisent des clés existantes,
dont `desktop.power`, jusqu'ici orpheline — seule édition i18n : `es.power`
passe de « Apagar » à « Energía » pour s'aligner sur fr/de), et le 44px vient de
la maquette elle-même. **Aucun token ni keep-out ne bouge** — le delta est
purement horizontal, la hauteur de pilule (62px) restant portée par le chip
utilisateur de 48px. La pastille de notification est ré-échelonnée à **16px**
ancrée au bouton (elle partageait auparavant une règle avec `.hv-dock-badge`,
qui garde ses 26px pour sa tuile de 48px). `desktop.spec.ts` passe de
**51 tests / 14 catégories à 58 / 15** : catégorie 15 « Topbar icon + label
chips » (7 tests — géométrie du chip, repli ≤900px sans seconde branche de
markup, ordre DOM, pastille 16px vs dock 26px + non-recouvrement du libellé,
WCAG 2.5.3, 901px en allemand), helper `topbarBtn`, et les 3 lignes 7.1-7.3 qui
visaient l'alimentation via `.hv-topbar-btn').last()` réécrites en
`[data-testid="topbar-power"]`. `8.3`, `14.1`, `14.3` et `14.4` restent verts
**sans modification** — c'est la preuve du zéro-ripple vertical. Comptage
vérifié via `npx playwright test --list --project=firefox` (58) et
`grep -c '^test.describe('` (15).

**2026-09-14 (chrome des fenêtres d'app — maquette « AppWindow »).** La fenêtre
d'app est réalignée sur la maquette Figma « Sans titre » (node `3:76`),
mesurée au pixel sur son export 2×. Le cadre devient du **papier opaque**
(`--hv-window-bg` #FFFFFF, plus aucune transparence), dans une hairline
**slate-300** `--hv-window-border` (#CBD5E1 — volontairement plus sombre que
`--hv-shell-border`, sinon le cadre se dissout dans le fond d'écran) et
l'ombre propre à la maquette (`--hv-window-shadow`, deux couches). La barre
d'en-tête passe à une hauteur **constante de 44px** (`--hv-winhdr-h` =
8 + 28 + 8), la tuile d'icône à **24px / rayon 8** (`--hv-winhdr-icon-*`,
dégradé d'app conservé), le titre à **14px / 600 sur 20px** en `#314158`
(`--hv-winhdr-title`). Le trio min/max/close quitte sa position flottante
48px pour la barre elle-même : **28px**, rayon 8, écart 6, aux stops exacts
de la maquette `--hv-winctrl-min` #FFB900 / `--hv-winctrl-max` #2B7FFF /
`--hv-winctrl-close` #FF6467 — des valeurs **délibérément plus vives** que
les sémantiques `--hv-warning`/`--hv-error`, à ne pas « harmoniser ». Les
glyphes sont ceux de la maquette (14×14, `stroke-width: 1.458`, la valeur du
SVG). Corollaire : `WindowHeader` réserve `--hv-winhdr-btn-clear` **120px**
(3×28 + 2×6 + 12 d'inset + 12 d'écart ; 108px en ≤480px, où la barre passe à
la ligne), et chaque bouton porte un `::after` transparent de **34px** — le
maximum que l'écart de 6px autorise sans que deux cibles voisines se
chevauchent — pour rester utilisable au doigt malgré les 28px de la maquette.
Aucun `data-testid` ne bouge, donc les catégories 3 et 15 restent vertes telles
quelles. `desktop.spec.ts` passe de **58 tests / 15 catégories à 63 / 15** :
catégorie 14 complétée par 5 tests de fidélité à la maquette (14.5 surface
opaque + hairline slate-300 + ombre deux couches + `backdrop-filter:none` ;
14.6 barre 44px, tuile 24px/rayon 8/dégradé, titre 14px/600/#314158 sur
20px ; 14.7 trio 28×28/rayon 8/écart 6 avec écart mesuré inter-boutons et
inset de 12px dans la barre — pas en surimpression ; 14.8 la réserve protège
réellement le titre, **mesuré** à 1440/768/400px plutôt que lu dans les
tokens, parce que le trio est en position absolue : le padding droit de la
barre est la seule chose qui l'empêche de passer dessous ; 14.9 **un seul
en-tête par fenêtre**, sur un échantillon du registre). Trois points de
méthode : 14.6 asserte le `min-height` de 44px et un rendu **≥** 44, pas 44
en dur — **tous** les apps livrées passent un `subtitle`, donc leur barre
est plus haute que la maquette (qui est titre-seul) ; le filet du bas se
mesure sur `.hv-winhdr-wrap`, **pas** sur `.hv-winhdr` — la règle est portée
par le conteneur (qui englobe aussi la rangée d'onglets facultative), donc
lire `borderBottomColor` de la barre renvoie le `currentColor` hérité (l'encre
`#1F2733`) et ne peut jamais correspondre — la première version de 14.6
échouait exactement là-dessus ; et 14.9 a été écrit
**après avoir trouvé un vrai bug** : `quotas`, ajouté le 2026-09-14, manquait
dans le `Set` `OWN_HEADER_APPS` de `WindowFrame.svelte`, donc la fenêtre
Quotas empilait **deux** barres (la sienne + le repli de WindowFrame, dont
le nom de sprite n'a pas de glyphe → barre vide). Corrigé en ajoutant
`'quotas'` au `Set` + un commentaire qui renvoie à 14.9. C'est le seul app du
registre qui manquait ; aucun `data-testid` ni token n'est concerné. Comptage
vérifié via `npx playwright test --list --project=firefox` (63) et
`grep -cE '^\s*test\.describe\('` (15).

**2026-09-14 (standardisation des en-têtes de fenêtre + écart des tuiles du
dock).** Le `WindowHeader` est désormais **icône + titre uniquement** : le prop
`subtitle` est supprimé de `WindowHeader.svelte` et de `PageLayout.svelte`, et
les 22 routes qui en passaient un ne le passent plus. La tuile d'icône vaut
`--hv-winhdr-icon-size` = `--hv-winctrl-size` (28px, la taille du trio
min/max/fermer) et les trois sont centrés sur la même ligne ; `.hv-dock-container`
passe à `--hv-chrome-dock-gap` (10px) parce qu'au survol une tuile grandit de
4.3px de chaque côté et que l'ancien `gap` de 4px les faisait se chevaucher.
Neuf specs ont été réalignés (desktop, account, health, journal, rescue,
dashboard, users, network, groups, shares) : les assertions de sous-titre
deviennent `toHaveCount(0)`, les badge/pastilles décoratifs supprimés sont
retirés des attentes, un test supprimé avec son objet (account 1.3), un ajouté
(desktop 14.10) et un réécrit (rescue 1.3, la disponibilité passe de la bande
d'en-tête à la carte de l'outil). **Aucun de ces specs n'a été relancé et rien
n'a été compilé** — l'instruction de session l'interdisait explicitement ; les
comptes par écran de la section suivante ont donc été produits par
`grep -cE '^\s*test\('` (la méthode du fichier) et non par un run. Les lignes
qui annonçaient un chiffre périmé (account 76→72, groups 61→60, dashboard
39→37, desktop 63→64) sont corrigées.

**2026-09-16 — Terminal : le strip d'onglets quitte l'en-tête de fenêtre.**
`terminal-tabs` était rendu dans le snippet `center` du `WindowHeader` — donc
dans la barre de titre, en concurrence avec le trio min/max/close épinglé à son
bord droit. C'est désormais une **barre du contenu de fenêtre**, enfant direct
du conteneur `terminal-page`, entre l'en-tête et `terminal-infobar` :
`WindowHeader` → `terminal-tabs` → `terminal-infobar` → corps xterm. Corollaire
immédiat : l'en-tête du Terminal ne passait **aucun** `title` (les onglets
occupaient la place), il n'aurait plus affiché que sa tuile d'icône — il reçoit
donc `title={$t('terminal.title')}` (clé déjà présente dans les 4 locales).
Géométrie mesurée en direct : en-tête 44px, strip 44px, infobar 32px, le strip
n'étant pas un descendant de `.hv-winhdr`. Nouveau test **2.8**, et quatre
tests réparés — **déjà rouges avant ce changement**, sélecteurs périmés depuis
la migration Tailwind-on-tokens du 2026-09-12 : 3.2 (`page.locator('hv-tab-active')`,
un sélecteur de *balise* — il manquait le point de `.hv-tab-active`) et 3.4 /
7.3 / 7.4 (ils attendaient les classes sémantiques `hv-tab-dot` / `hv-info-badge`,
supprimées au profit de tokens ; ils assertent maintenant le **remplissage
calculé**, qui est le marqueur d'état réel). Vérifié par un run réel (Firefox,
instance neuve jetable sur port scratch, `BASE_URL` dédié, aucune donnée
réelle) : sections 1, 2, 3, 4, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18
vertes, `svelte-check` à 0 erreur. Quatre échecs subsistent, **tous prouvés
préexistants** par `git stash push -- frontend/src/routes/Terminal.svelte`
(le fichier remis dans son état d'avant, ils échouent à l'identique) :
**4.2** (famille monospace du corps xterm — jamais touchée ici), **10.3**
(« < 80 requêtes réseau » : artefact du serveur de dev Vite, qui sert des
modules ES non bundlés — l'assertion porte sur un bundle de production),
**19.1 / 19.2** (popup de complétion, zone non touchée). Le run global des 79
tests en une passe a été **écarté** : il a été tué par le système pour mémoire
basse, et un rechargement HMR déclenché par une édition en cours l'avait
perturbé — la vérification retenue est donc section par section, sur code
gelé.

**2026-09-17 — Terminal : plus de scroller vertical dans le strip, plus de
bande noire sous le terminal.** Deux défauts d'affichage distincts, tous deux
mesurés dans le DOM avant correction (aucune déduction) :

1. **Scroller vertical dans `terminal-tabs`.** `overflow-x: auto` seul calcule
   `overflow-y` à `auto` lui aussi ; le `min-h-11` (44px) moins la bordure basse
   de 1px laisse la boîte de contenu 1px trop courte, ce qui suffit à faire
   apparaître une barre de défilement verticale. Relevé : `clientHeight 43` /
   `scrollHeight 44`, `overflowY: "auto"`. Ce qui l'a révélé est la migration
   Tailwind elle-même : avant elle, `.hv-terminal-tabs` portait
   `scrollbar-width: none` (qui masquait les deux barres) — une déclaration sans
   équivalent Tailwind, donc perdue. Corrigé par `overflow-y-hidden`, qui
   supprime l'axe à la source plutôt que de masquer des barres : la barre
   horizontale reste disponible quand les onglets débordent réellement.
2. **Bande noire en bas de la fenêtre.** `terminal-body` était resté en
   `#1e1e1e` tandis que le thème xterm était passé au blanc opaque
   (`theme.background: '#FFFFFF'`) : `FitAddon` dimensionne le terminal en
   `floor(hauteur / hauteurDeCellule)` lignes, donc il reste toujours un
   reliquat inférieur à une cellule, qui peignait le fond sombre du conteneur
   sous la dernière ligne. Relevé : `terminal-body` 596px contre 594px pour le
   xterm, fond `rgb(30,30,30)` face à `rgb(255,255,255)`. La dérive date de la
   migration Tailwind-on-tokens en cours : avant elle, `.hv-terminal-content`
   portait `rgba(255, 255, 255, 0.55)` — clair, donc cohérent avec le thème de
   l'époque — et `#1e1e1e` n'existait nulle part dans le fichier. Corrigé en
   alignant le conteneur sur le thème ; au passage, `bg-[#1e1e1e]` était une
   couleur en dur, interdite par la règle « aucune valeur en dur » — il passe
   au token `--hv-shell-glass`.

Deux tests ajoutés. **2.9** verrouille l'axe vertical (`overflow-y` calculé
`hidden`, débordement vertical ≤ 1px) — la valeur calculée est le seul signal
qui vaut sur toutes les plateformes, une barre overlay ne volant aucune
largeur. **2.10** compare en direct la couleur du conteneur à celle du
`xterm-viewport` (ce que le terminal peint réellement) au lieu d'une
constante : changer le thème entraîne le conteneur, la dérive est impossible.
Ces deux assertions discriminent bien — les valeurs qu'elles rejettent
(`overflowY: "auto"`, `rgb(30,30,30)`) sont exactement celles relevées avant
correction. Vérifié par un run réel (Firefox, instance neuve jetable sur port
scratch, `BASE_URL` dédié, aucune donnée réelle) : **81 tests**, sections 2, 4,
8, 12, 13 vertes, plus la suite complète ; seul **4.2** échoue, préexistant
(famille monospace, déjà prouvé tel par `git stash` la veille).

**2026-10-07 — l'écran Quotas exécuté et audité en direct (dernier écran sans
filet).** Le spec écrit le 2026-09-22 est **vert de bout en bout** pour la
première fois : **38/38** (`npx playwright test tests/e2e/quotas/quotas.spec.ts
--project=firefox`, comptage de référence `--list` = **38** ; `grep -cE
'^\s*test\('` = **36**, l'écart venant de la boucle responsive 360/768/1920, une
ligne pour trois tests), contre un serveur scratch (`PORT=8099`,
`DATA_DIRECTORY` isolé, Firefox, aucune donnée réelle, `e2eadmin` seedé via
`POST /api/auth/setup` comme en CI — l'écran s'ouvre en admin). **Aucun
sélecteur mort** : le spec est postérieur à la migration Appliance du
2026-09-12 et cible déjà les `data-testid` de `Quotas.svelte`, il n'a fallu
corriger **aucune** assertion. Audit live (même serveur scratch, Firefox réel,
captures à l'appui) : la fenêtre s'ouvre depuis le lanceur, l'en-tête ne porte
que [ICÔNE] [TITRE], les 3 puces de filtre (Tous/Utilisateurs/Groupes) forment
un `role="group"` à `aria-label`, mutuellement exclusives (`aria-pressed`), la
ligne semée rend « UTILISATEUR », le nom + « Mis à jour le … », la barre d'Envoi
`used/limit` (mesurée : 50 Mo sur 100 Mo → `aria-valuenow="50"`, `aria-valuemax`
100, piste remplie à 50 %) et « Illimité » sur la direction Réception
désactivée ; « Modifier dans Utilisateurs » émet bien `hv-open-app`
(`detail.id = 'users'`), ouvre la fenêtre Utilisateurs **sans quitter**
`/desktop` ni fermer Quotas ; console sans erreur. Détail dans l'addenda du
2026-10-07 de `CLAUDE.md` (invariant « Explorer / transfers »). Conséquence :
plus aucun des 27 écrans n'est sans audit live **ni** sans spec E2E.

## Résumé

| Niveau | Couverture |
|---|---|
| Audit manuel live (CLAUDE.md, invariants) | **27 / 27** écrans (Quotas audité en direct le 2026-10-07) |
| Au moins un test unitaire Rust pertinent | **27 / 27** (Widgets et Quotas : sans objet — Quotas n'a aucun code propre, il lit un endpoint existant ; Desktop/shell couvert depuis 2026-09-09 par `src/sse/progress.rs` + `src/handlers/activity.rs` — barre de tâches) |
| Test d'intégration Rust (`tests/*.rs`) | **40 fichiers** `*_integration.rs` ; un par écran sauf Quotas — Login, Setup Wizard, Mon compte, Docker (+ `compose_integration.rs` — édition du compose d'un conteneur), Utilisateurs, Groupes, Rescue, Réseau, VPN, Parts/Protocoles, Paramètres (cartes sécurité **+ cœur** — `settings_core_integration.rs` **+ HTTPS/TLS intégré — `tls_integration.rs`**), Downloader, Stockage, RAID, Sauvegarde, Cloud Sync, Syncthing, Terminal, Chat, Explorateur, Dashboard, Journal système, Widgets, Desktop/shell (+ `activity_integration.rs` — barre de tâches), Santé, **VirtualMachines** ; **Quotas : aucun fichier propre** — son unique endpoint (`GET /transfer-quotas/{type}`) est couvert par `transfer_quota.rs` + `explorer_integration.rs`. Les fichiers restants sont transverses (`mdns`, `notifications`, `progress`, `snmp`, `ups`, `weather_location`, `app_catalog`, `app_proxy`, `compose`, `security`, `tls`, `share_links`, …) |
| Test E2E Playwright (`tests/e2e/`) | **27 specs d'écran sur 28 fichiers** — Login, Setup Wizard, Mon compte, Utilisateurs, Groupes, Terminal, Paramètres, RAID, Docker, Rescue, Réseau, VPN, Parts/Protocoles, Downloader, Stockage, Sauvegarde, Cloud Sync, Syncthing, Chat, Explorateur, Dashboard, Journal système, Widgets, Desktop/shell, Santé, **VirtualMachines**, **Quotas**, plus `security/` (sous-fonctionnalité, pas une section de `SCREENS.md`) ; le spec Quotas (`tests/e2e/quotas/quotas.spec.ts`, écrit le 2026-09-22) est **vert de bout en bout depuis le 2026-10-07** — **38/38** (Firefox, serveur scratch) — voir le détail par écran ligne 6 |

Les 27 écrans ont tous été vérifiés à la main **et** disposent d'au
moins un test automatisé (Rust ou Playwright) — aucun n'est sans filet de
non-régression (VirtualMachines : vérifié en direct contre un vrai Incus —
conteneurs, VM ISO Linux + écran noVNC, création concurrente, bibliothèque
d'ISO ; seul le boot d'un ISO Windows réel reste à confirmer).

## Détail par écran

| # | Écran | Audit manuel (CLAUDE.md) | Tests unitaires Rust | Tests d'intégration Rust | Tests E2E Playwright |
|---|---|---|---|---|---|
| 1 | Login | [x] 2026-07-13 (+07-15, 07-17, 09-03, 09-04) | [x] `auth/service.rs`, `auth/tests.rs` | [x] `login_integration.rs` (53 — +1 le 2026-09-22 : l'historique de connexion `/security/login-history` reflète désormais les tentatives **réelles** — échec `success=false` + user-agent persistés, succès `success=true`, plus rien de fabriqué), `login_pam_integration.rs` (3), `auth_recovery_integration.rs` (7) | [x] `login.spec.ts` (91 — **+2026-09-23 : 13.6 garde de contraste des jetons Login** : accent / `soft-border` / les 2 dégradés ≥4,5:1 sous blanc, lus sur le `:root` live — voir « contraste standardisé » l. 26 ; 90 avant) ; **+2026-09-12 : migration design Appliance — carte auth + 2FA + récupération par code en Tailwind-on-tokens (inputs natifs conservés pour l'autofill, chaîne de soumission native vérifiée), sélecteurs `data-testid="login-*"`, zéro CSS scopé** ; **+2026-09-12 : §12 anti-zoom iOS — champs de formulaire ≥16px sur pointeur tactile** (`pointer: coarse`, cf. `docs/guides/design-system.md` §5.1) |
| 2 | Setup Wizard | [x] 2026-07-13 (+07-17, 09-03) | [x] `setup.rs` (43) | [x] `setup_integration.rs` (34 — +6 le 2026-09-22 : TOTP `verify-totp` accepte/rejette + gated post-setup, setup refuse un code TOTP invalide sans créer d'utilisateur, setup avec TOTP valide = 2FA activée + backup codes, secret seul ignoré) | [x] `setup.spec.ts` (90 — +19.0 le 2026-09-22 : toggle TOTP à l'étape mot de passe → QR + secret + message apps compatibles, non destructif) ; **+2026-09-12 : migration design Appliance — stepper + étapes + création admin + recovery + 2FA en Tailwind-on-tokens, sélecteurs `data-testid="setup-*"`, zéro CSS scopé** ; **+2026-09-22 : TOTP déplacé DANS l'étape mot de passe (toggle → QR `qrcode` client → code vérifié via `POST /auth/setup/verify-totp` stateless puis re-vérifié dans `POST /auth/setup`), code de récupération généré+affiché à l'étape 4 après création, écran 2FA post-création supprimé — récap backup codes après recovery, section 19 réécrite (19.0 non destructif + 19.1 flux complet)** |
| 3 | Desktop (bureau/shell) | [x] 2026-07-17 (+ ~15 follow-ups jusqu'au 09-04, incl. le correctif glassmorphisme du 09-05) | [x] 2026-09-09 — `src/sse/progress.rs::tests` (11 — dernier événement retenu, `owner`, `list_activity`, cleanup) + `src/handlers/activity.rs::tests` (14 — mapping `kind`, dérivation nom aria2, %, sérialisation) pour la **barre de tâches** | [x] `desktop_integration.rs` (3 — gardes d'auth sur `/api/server/restart` + `/shutdown`) + `activity_integration.rs` (11 — `GET /api/activity` : `require_auth` 400/401, cycle de vie pending/running/completed/failed, `%` indéterminé, cloisonnement par `owner`, jamais de spawn aria2) | [x] `desktop.spec.ts` (**74 tests runtime, 15 catégories** — recompté le 2026-09-24 : +11.3 « a crashing window body shows the alert fallback and Reload recovers it » (garde `svelte:boundary`, W10 — écrit avec le reste de la passe, **non exécuté** ; `grep -cE '^\s*test\('` **et** `npx playwright test --list` donnent 74) ; 73 le 2026-09-17 : +14.11 « every app header bar carries the icon + title only », qui parcourt les 24 app ids du registre et exige icône + titre seuls dans la barre ; 72 le 2026-09-16 via `grep -cE '^\s*test\('` **et** `npx playwright test --list`, les deux donnent 72 (la méthode du fichier) ; +cat. 13 le 2026-09-09 : barre de tâches (bouton dock + pastille, panneau largeur = dock, ligne nom/%/piste, clic ligne = rien, fermeture Échap/dehors/re-clic, responsive, sécurité localStorage/URL, poll `/api/activity` jamais `/downloader/tasks`) ; **ne confirme jamais** un restart/shutdown/logout ; +2026-09-12 : tests de géométrie du chrome — 8.5 pillules dock/topbar même rayon à chaque largeur, 8.6 fenêtre maximisée ne chevauche pas le dock en tablette ; correctif de sélecteur 5.3 (`.jr-page` renommé → dialog `aria-label="System Journal"`) ; **+2026-09-14 : fidélité dock/topbar à la maquette DSM 7 — 8.3 réécrit** (chaque pilule vaut **son propre** token, `--hv-chrome-topbar-h` / `--hv-chrome-dock-h`, et le dock est strictement plus haut : l'ancien invariant « même hauteur » est remplacé par son inverse, sinon le test ne protégerait plus rien) **+ cat. 14 « Chrome geometry & material »** (14.1 le miroir JS `snapUtils.ts` recopie exactement les keep-out CSS ; 14.2 la pastille `--hv-app-<id>-c` de chaque tuile est bien le stop sombre de son dégradé ; 14.3 les deux pilules sont opaques — `backdrop-filter:none` + `rgb()` sans alpha ; 14.4 le tooltip au survol, jusqu'ici non couvert : contenu = `aria-label`, fond `--hv-tooltip-bg`, rayon et taille de police) ; 8.4 et 8.5 restent verts sans modification — l'avatar rétrécit avec son cadre, le rayon des pilules n'a pas bougé ; **+2026-09-14 : boutons de topbar en puces icône + libellé (maquette DSM, node `1:24`) — cat. 15 « Topbar icon + label chips »** : 15.1 à 1440px chaque bouton montre icône **et** libellé (largeur > 44, hauteur = 44, libellé à droite de l'icône et centré à ±2px) ; 15.2 sous 901px le libellé est `toHaveCount(1)` **puis** masqué (présent-mais-masqué, une seule branche de markup) et le bouton redevient un carré 44×44 ; 15.3 ordre DOM réel `[network, widgets, notifications, power]` **et** Power reste le dernier `.hv-topbar-btn` (l'hypothèse positionnelle des 7.1-7.3 est conservée en assertion, même si ces trois tests utilisent maintenant `[data-testid="topbar-power"]`) ; 15.4 la pastille de notification fait **16px** (la sonde est un clone d'un élément du template — le CSS Svelte est scopé par hash, un `createElement` mesurerait un span non stylé), `.hv-dock-badge` garde ses **26px**, et la pastille vit dans la bande **au-dessus** de la ligne du libellé (`badge.bottom <= label.top`) donc un « 99+ » s'élargit dans le vide au lieu de recouvrir le texte ; 15.5 nom accessible traduit (jamais une clé brute) et, à 1440px, contenant le libellé visible — WCAG 2.5.3, ce qui valide le couple `power_options`/`power` ; 15.6 à **901px** (l'angle mort de 8.2, qui n'échantillonne que 360/768/1920) la pilule ancrée à droite reste à `left >= 0` et sans scroll horizontal ; 15.7 les 4 locales rendent 4 libellés non vides et non-clés, puis la pilule **allemande** (le pire cas de largeur) tient encore à 901px ; 8.3 / 14.1 / 14.3 / 14.4 restent verts **sans modification** — preuve que le delta est purement horizontal ; **+2026-09-14 (même jour) : chrome des fenêtres d'app = maquette « AppWindow » (Figma `4pwNRZU5wQjSWQBODFViKz` node `3:76`) — cat. 14 complétée par 14.5 surface opaque `#FFFFFF` + hairline slate-300 + ombre deux couches + `backdrop-filter:none`, 14.6 barre 44px / tuile 24px rayon 8 / titre 14px 600 `#314158` sur 20px, 14.7 trio 28×28 rayon 8 écart 6 aux stops `#FFB900`/`#2B7FFF`/`#FF6467` **dans la barre** (écart inter-boutons et inset 12px mesurés, pas lus dans les tokens) et 14.8 la réserve `--hv-winhdr-btn-clear` protège réellement le titre — **mesuré** à 1440/768/400px, parce que le trio est en position absolue : le padding droit de la barre est la seule chose qui l'empêche de passer dessous. **14.9 (un seul en-tête par fenêtre) a été écrit après avoir trouvé un vrai bug** : `quotas`, ajouté le 2026-09-14, manquait dans le `Set` `OWN_HEADER_APPS` de `WindowFrame.svelte` → la fenêtre Quotas empilait deux barres (la sienne + le repli vide de WindowFrame) ; corrigé en ajoutant `'quotas'` au `Set`. Les catégories 3 et 15 restent vertes **telles quelles** : aucun `data-testid` n'a bougé, seuls la taille, la couleur et la position du trio ont changé ; **+2026-09-14 (même jour) : standardisation des en-têtes + écart des tuiles du dock — 14.6 durci** (la barre est désormais titre-seul : la tuile d'icône vaut `--hv-winhdr-icon-size` = `--hv-winctrl-size`, soit les mêmes 28px que le trio, et la hauteur mesurée est **exactement 44px** — le test passe d'un plancher `>= 44` à une égalité, ce qui transforme tout élément qui reviendrait dans la barre en échec) ; **14.7 complété** (les lignes médianes de la tuile, du trio et du titre sont comparées à celle de la barre, pas seulement leurs tailles) ; **14.10 nouveau** (le `gap` de `.hv-dock-container` passe à `--hv-chrome-dock-gap`, 10px : au survol une tuile grandit de `48 × (1.18 − 1) / 2 ≈ 4.3px` de chaque côté, donc l'ancien `gap` de 4px faisait **réellement** se chevaucher deux voisines ; le test survole une tuile du milieu et exige que le dégagement mesuré aux deux voisines dépasse la croissance réellement produite — il ne lit aucun token) ; 14.1 à 14.5 et 14.8 à 14.9 inchangés. **Specs modifiés mais NON relancés** — l'instruction de session était explicitement de ne ni compiler ni tester) ; **+2026-09-16 : refonte du lanceur d'apps sur la maquette Figma `4pwNRZU5wQjSWQBODFViKz` node `23:6646` (« Desktop launcher ») — `HvLaunchpad.svelte` passe d'une modale opaque centrée (1000×720, rail de catégories vertical, cartes larges avec description) à un **voile plein écran assombri** (`.lp-overlay`, `rgba(15,23,42,0.72)` + `backdrop-filter`, colonne de contenu alignée en haut (704px à la refonte ; **828px** depuis la demande « 6 colonnes » du même jour)), une **rangée de puces de catégorie** (`aria-pressed` dans un `role="group"`, pas un `tablist` : elles filtrent la même grille sur place) et une **grille de tuiles compacte** (`repeat(auto-fill, minmax(124px, 1fr))`, icône 56px au-dessus du libellé sur 2 lignes max). Nouvelle palette page-scopée `--hv-launcher-*` ; **deux valeurs de la maquette ont été corrigées au seuil de contraste WCAG** — le voile `0.55` → **`0.72`** (les libellés blancs tombaient à 3,39:1 sur un fond d'écran clair) et le texte de puce inactif 85 % alpha → **blanc opaque** (4,28:1) ; le placeholder `#90A1B9` (2,6:1 sur la barre blanche) passe à `#64748B` et l'encre secondaire 72 % → 88 % alpha (3,8 → 4,7:1). Les tuiles gardent le **dégradé par app** (`--hv-app-<id>-grad`) et non le carré blanc de la maquette — la règle « une couleur par app » est contraignante ; **aucun badge de compteur inventé** (la maquette en montre, la donnée n'existe pas) : la seule pastille d'angle est verte et **n'est rendue que si le conteneur tourne** (un point gris « arrêté » est indétectable sur le fond de tuile, ~1:1 ; l'état arrêté est déjà porté par `aria-describedby`). Les 8 avertissements `desktop.app_<id>_info` (Rescue, RAID, Storage, Network, VPN, Backup, Users, Docker) survivent en **description accessible** (`.lp-sr` + `aria-describedby`) au lieu d'être orphelins. Le scrim devient un vrai `<button>` frère du dialogue (plus de `role="button"` imbriquant des `<button>`). **Contrat E2E inchangé** : `.lp-overlay`, `.lp-panel`, `.lp-card[title]`, `.lp-search-field`, le menu externe Docker (`.lp-ext-*`), `.lp-empty`/`.lp-reset`, `.lp-count` gardent leurs sélecteurs — 5.1 à 5.4 passent tels quels ; **catégorie 5 étendue de 4 à 12 tests** : 5.5 les puces filtrent la grille sur place + exactement une `aria-pressed="true"`, « Toutes les apps » restaure ; 5.6 recherche sans résultat → `.lp-empty` → `.lp-reset` restaure la grille et vide le champ ; 5.7 le lanceur est un voile `fixed` **translucide sombre** et la colonne de contenu ne peint aucune surface ; 5.8 **contraste mesuré** — le libellé de tuile composite tuile→voile→papier blanc (le pire cas d'écran d'accueil) et exige ≥ 4,5:1, encre opaque ; 5.9 la puce peint < 44px mais porte un `::after` de 44px, le champ de recherche vaut 44px dans la barre de 56px de la maquette, la tuile ≥ 44px ; 5.10 le piège de focus tient 30 Tab et Échap **rend le focus à la tuile du dock** (2.1.1 / 2.4.3) ; **5.11 (téléphone)** grille 3 colonnes, puces `nowrap`/`overflow-x:auto` avec `flex: none` (**la croissance du poste de travail est annulée** — une puce étirée remplirait la rangée et il ne resterait rien à faire défiler) et rangée **effectivement débordante**, le panneau **scrolle en interne** (ni la page ni l'axe horizontal ne bougent) et la dernière tuile reste atteignable ; **5.12 (6 colonnes — demande du 2026-09-16)** la grille fait exactement 6 pistes de ≥ 124px à 1440px, **et** la barre de recherche, la rangée de puces et la grille partagent la même largeur **et** le même `x` (tolérance 1px de sous-pixel : c'est un désalignement de mise en page qui doit échouer, pas un arrondi), **et** le contenu de la rangée de puces atteint le bord droit de la grille (**demande « pas assez de catégories → agrandir le module » du même jour** : la boîte de la rangée faisait déjà 828px, mais une puce ne fait que la largeur de son libellé, donc avec peu de catégories les puces s'arrêtaient aux deux tiers et le module *paraissait* plus étroit que les tuiles ; `flex: 1 1 auto` partage l'espace libre de chaque ligne entre ses puces, et l'assertion est faite **par ligne rendue** pour tenir aussi quand la rangée passe à la ligne — un catalogue complet monte à 13 puces ; `grow` plutôt que `space-between` : les deux étalent la dernière ligne, mais `grow` l'étale en puces un peu plus larges là où `space-between` ouvre un vide entre deux puces éloignées) — `--hv-launcher-content-w` passe de 704 à **828px** (6 × 124 + 5 × 12 = 804 minimum ; 828 tombe sur des colonnes rondes de 128px). Les trois modules étant enfants de la même colonne, ils s'élargissent ensemble — c'est le test qui l'empêche de dériver. Le conteneur scrollable est passé de `.lp-content` à **`.lp-panel`** (avec `scrollbar-gutter: stable` et une barre de recherche `sticky`) précisément pour ça : une barre de défilement qui prend de la largeur (Chrome Windows/Linux, GTK sans overlay) la retirait au seul grid, la barre de recherche et les puces étant hors de ce conteneur ; 5.12 mesure donc les trois largeurs **une seconde fois à 1440×520**, tuiles débordantes — le seul régime où elles peuvent diverger (la barre de défilement du moteur E2E est en overlay, 0px, et masque la dérive). **Deux vrais défauts trouvés par ces tests, pas par relecture** : (1) 5.5 et 5.6 ont échoué parce que le `<svelte:window onclick>` de `Desktop.svelte` referme le lanceur à chaque clic — `.lp-panel` a repris son `onclick` stopPropagation (et surtout pas de `onkeydown`, qui avalait Échap : c'est un bug déjà vu) ; (2) la mesure du `backdrop-filter` de 5.7 a montré que déclarer la paire `backdrop-filter` + `-webkit-backdrop-filter` fait garder au minifieur la **seule** forme préfixée, que Firefox ignore (`CSS.supports('-webkit-backdrop-filter', …)` = false) — le flou `blur(2px)` de la maquette valait donc `none`. Écrit en propriété standard seule, le minifieur émet lui-même les deux formes. **72 tests / 15 catégories** |
| 4 | Mon compte (Account) | [x] 2026-07-13 (+09-03, 09-04) | [ ] partagé avec `auth/` uniquement, rien de spécifique à Account | [x] `account_integration.rs` (25) | [x] `account.spec.ts` (72 — recompté le 2026-09-14 via `grep -cE '^\s*test\('`, la méthode du fichier ; l'entrée disait 76, chiffre périmé) ; **+2026-09-12 : migration design Appliance — `PageLayout` + `SectionCard`, grille 2-col via container queries Tailwind (`@container`/`@max-*`), sélecteurs `data-testid="account-*"`, zéro CSS scopé** ; **+2026-09-14 : standardisation des en-têtes — 1.2 vérifie désormais `.hv-winhdr-title` seul + `.hv-winhdr-subtitle` `toHaveCount(0)` ; le test 1.3 « l'admin affiche le sous-titre administrateur » est SUPPRIMÉ (le sous-titre n'existe plus) et les tests suivants de la catégorie 1 renumérotés 1.3/1.4 pour ne pas laisser de trou — c'est la cause du −1 ; 17.1 vérifie le même invariant. Specs modifiés mais NON relancés** ; **+2026-09-22 : gestion de sessions self-service — deux `SectionCard` « Sessions actives » (liste IP/créée/expire, badge « Cet appareil », révocation par session via `POST /api/security/sessions/revoke`, « Déconnecter tous les autres » via `/revoke-all` avec `ConfirmDialog`, bouton refresh) et « Connexions récentes » (`GET /api/security/login-history/{me}` : IP, succès/échec, horodatage, user-agent) ; 15 clés i18n ×4 locales sous `[account]` ; specs Account existants non relancés** |
| 5 | Utilisateurs (Users) | [x] 2026-07-13 (+ 3 bugs trouvés par l'E2E le 09-04, +1 bug trouvé/corrigé par revue statique/tests le 09-05 ; +2026-09-08 : slider de taille du répertoire personnel = quota disque `setquota -u` ; +2026-09-08 : refonte design mono-vue façon template Umbrel — grille de vignettes → fiche liste-de-réglages → formulaire de création, markup/CSS seulement ; +2026-09-08 : folder picker pour le répertoire perso (validation serveur `validate_custom_home_path`), slider de taille pour tous les rôles, état d'erreur explicite quand la liste des groupes échoue, `homevault_admin` affiché en puce lecture seule dérivée du rôle) | [x] `groups.rs` (5) + `handlers/users.rs` (13, `is_valid_email`) + `quota.rs` (+2 — `bytes_to_quota_kb`) + `handlers/user_homes.rs` (7 — `validate_custom_home_path` : rejets relatif/traversée/caractères de contrôle/racine/dirs système/parent manquant, `containing_partition` plus-long-préfixe) | [x] `users_integration.rs` (60 — +2 : `PUT .../full` avec `home_path` rejette un chemin sous un dossier système + les chemins relatifs/traversée/caractère de contrôle) | [x] `users.spec.ts` (**81** — recompté le 2026-09-14 via `grep -cE '^\s*test\('`, la méthode du fichier ; l'entrée disait 82, chiffre périmé ; `npx playwright test --list` reste la référence et n'a pas été relancé), incl. quotas de transfert + §16 : slider offert pour un admin aussi (16.4 inversé), folder picker ouvre/ferme (16.6) + 4.11 puce `homevault_admin` lecture seule dérivée du rôle — §16 vérifié en direct via Playwright MCP le 2026-09-08 ; le `beforeAll` de tout le fichier exige un sudo NOPASSWD pour `groupadd`/`useradd`, absent ici) ; **+2026-09-12 : migration design Appliance — `PageLayout`, vue grille/vignettes + fiche réglages + création en Tailwind-on-tokens, `FolderPicker`/`Modal` conservés, sélecteurs `data-testid="users-*"`, zéro CSS scopé ; l'erreur TS pré-existante (réduction `selectedUser`→`never` dans `{:else}`) corrigée** ; **+2026-09-13 : master-detail 2 volets persistant (patron Aurora Mail) — grille/vignettes à gauche (colonne 360px), fiche/création à droite ; la liste est retirée du DOM quand le conteneur passe sous 760px ET un détail est ouvert (ResizeObserver sur le split, pas un media query viewport — la fenêtre flotte), test 7.4 « one view at a time » préservé** ; **+2026-09-14 : refonte maquette Figma (`4pwNRZU5wQjSWQBODFViKz` node `3:99`) — rail de comptes à gauche (bouton Nouvel utilisateur, recherche, 4 filtres, compteur, lignes compactes avatar/nom/badge/état, pied « Vue des quotas » → app Quotas via `hv-open-app`) + volet détail en **6 cartes `SectionCard divider`** à hauteur de contenu (Identité / Rôle et groupes / Dossier personnel / Quotas de transfert / Sécurité / Actions) ; les hauteurs fixes de la maquette (152.69 / 216.23 / 173.09 / 887.26 px) ne sont pas recopiées ; le seuil de bascule passe à ≈ 1065px de fenêtre (760px mesurés sur la colonne de contenu : rail 257px + `p-6` 48px déjà déduits) ; **bug pré-existant `canBack` CORRIGÉ** — la variable n'était déclarée nulle part et le bundle livré levait un `ReferenceError` en mode strict à l'ouverture du volet création (le bouton « Nouvel utilisateur » était cassé en production) ; la notion est supprimée, pas déclarée (la barre de retour est toujours rendue quand un volet est ouvert). **Spec non modifié et non relancé** ; **2026-09-14 (même jour) : `npm run check` → 7 erreurs, aucune dans `Users.svelte`/`Quotas.svelte` (les 7 sont pré-existantes : `Restic.svelte` 2, `Syncthing.svelte` 2, `Downloader.svelte` 3) ; `npm run build` OK (32 s, `Users-CCVDHXRe.js` + `Quotas-DbeDd7_t.js` émis) ; `cargo build` OK (9 min 55, aucun warning). Le check a trouvé et fait corriger 2 vrais défauts de `Quotas.svelte` : `t` jamais importé depuis `$lib/stores/i18n` (22 erreurs en cascade) et un `{subtitle}` jamais déclaré. Le volet E2E reste non lancé** ; **+2026-09-14 : standardisation des en-têtes — 1.2 n'attend plus le récapitulatif « %accounts% comptes · %locked% verrouillés » (`users.summary`) dans le sous-titre mais `.hv-winhdr-title` seul + `.hv-winhdr-subtitle` `toHaveCount(0)` ; les compteurs restent visibles, dans les puces de filtre (le test vérifie les 4 `[data-testid="users-filter"]` pour que la lecture ne disparaisse pas silencieusement). Nombre de tests inchangé (81, recompté le 2026-09-14) ; spec modifié mais NON relancé** |
| 6 | Quotas | [x] **2026-10-07 — vérifié en direct** (serveur scratch Firefox, `PORT=8099` + `DATA_DIRECTORY` isolé, `e2eadmin` seedé via `POST /api/auth/setup` comme en CI, aucune donnée réelle) : ouverture depuis le lanceur, en-tête [ICÔNE] [TITRE] seul, les 3 puces de filtre (Tous/Utilisateurs/Groupes) `role="group"` + `aria-label` mutuellement exclusives (`aria-pressed`), ligne semée « UTILISATEUR » + nom + « Mis à jour le … », barre `used/limit` mesurée (50 Mo sur 100 Mo → `aria-valuenow="50"`, `aria-valuemax` 100, piste remplie à 50 %) et « Illimité » sur la direction désactivée, « Modifier dans Utilisateurs » émet `hv-open-app` (`detail.id='users'`) sans quitter `/desktop` ni fermer Quotas, console sans erreur. **Compilation acquise le 2026-09-14** : `npm run check` 0 erreur (2 vrais défauts corrigés : `t` non importé, `{subtitle}` non déclaré), `npm run build` émet `static/dist/assets/Quotas-*.js`, `cargo build` OK. Détail dans l'addenda 2026-10-07 de `CLAUDE.md`. | n/a (écran sans backend propre — lit `GET /transfer-quotas/{type}`) | [ ] **toujours aucun fichier d'intégration propre** — l'endpoint est couvert par `transfer_quota.rs` (unitaires) et `explorer_integration.rs` (gardes de quota) | [x] `tests/e2e/quotas/quotas.spec.ts` — **38/38 verts 2026-10-07** (Firefox, serveur scratch ; comptage de référence `npx playwright test --list --project=firefox` = **38**, `grep -cE '^\s*test\('` = **36**, l'écart venant de la boucle responsive 360/768/1920, 1 ligne → 3 tests). Lecture seule : chargement, filtres tous/utilisateurs/groupes, ligne « Illimité » quand la direction est désactivée, barre de progression = `used/limit`, « Modifier dans Utilisateurs » (`hv-open-app`), §10 sécurité (aucune écriture, aucun jeton dans le DOM/URL), cas limites (500 → état d'erreur, backend vide → état vide), console sans erreur, régression mémoire. **Aucun sélecteur mort trouvé** — le spec est postérieur à la migration Appliance du 2026-09-12 et cible déjà les `data-testid`, il a passé tel quel. |
| 7 | Groupes (Groups) | [x] 2026-07-13 (+07-18, 07-21) | [x] `groups.rs` (5) | [x] `groups_integration.rs` (26) | [x] `groups.spec.ts` (61 — **+2026-09-23 : 16.1 garde de contraste des avatars** (initiales blanches ≥4,5:1 sur les 6 stops de `$lib/utils/avatars.ts`, tuiles ligne + détail) ; 60 avant cet ajout au grep, **63 à `--list`** (2 tests générés en boucle) ; recompté le 2026-09-14 via `grep -cE '^\s*test\('`, la méthode du fichier ; incl. quotas de transfert) ; **+2026-09-12 : migration design Appliance — `PageLayout` (rail liste / panneau détail), `SectionCard`, `FormRow`, Tailwind-on-tokens, zéro CSS scopé, sélecteurs `data-testid="groups-*"` ; le breakpoint 860px (empilement des panneaux) est piloté par `matchMedia` → `sidebarHidden`** |
| 8 | Dashboard | [x] 2026-07-13 (+07-15, 09-02) | [x] `handlers/dashboard.rs` (23 — `format_uptime` (6) + **+2026-09-07** `format_bytes`, `parse_size` (dont fraction + virgule de locale, **bug corrigé**), `calculate_individual_disk_score`, `calculate_disk_summary`, `calculate_disk_score`) | [x] `dashboard_integration.rs` (10, +2026-09-07 — **10/10** — split d'auth sur les 15 endpoints de lecture + widget-CRUD + widget-preferences, cycle widget add/list/reorder/remove, upsert préférences par utilisateur + cloisonnement) | [x] `dashboard.spec.ts` (37 tests runtime, 12 catégories — recompté le 2026-09-14 via `grep -cE '^\s*test\('`, la méthode du fichier ; l'entrée disait 39, chiffre périmé, et le « 39/39 verts » du 2026-09-07 porte donc sur un état antérieur ; **+2026-09-14 : standardisation des en-têtes — 1.7 et 7.1 n'attendent plus la pastille « Live » (élément décoratif supprimé avec le sous-titre) mais vérifient `.hv-winhdr-title` seul, `.hv-winhdr-subtitle` et `[data-testid="dashboard-live"]` à `toHaveCount(0)`** ; lecture seule, l'écran n'a aucune action mutante) ; **+2026-09-12 : migration design Appliance — `PageLayout`, bento-grid 12-col via container queries Tailwind, SVG (jauge/core/mem) conservés, sélecteurs `data-testid="dashboard-*"`, zéro CSS scopé** |
| 9 | Stockage (Storage) | [x] 2026-07-17 (+09-06 suites de tests + re-scope « Disques virtuels » ; +09-07 refonte layout `impeccable` ; **+2026-09-07 test réel : (a) liste/scrub/destruction d'un vrai disque virtuel mdadm + ZFS — `PoolService::scan_pools` corrigé (device_count + mount_point + usage via `df`) ; (b) cycle LUKS complet create/lock/unlock/delete sur `/dev/loopN` + vrai `cryptsetup` — 2 bugs frozen corrigés : `handle_encryption_create` ne validait pas `req.device` (gardé sur `get_available_devices` `!in_use`), `encryption::erase` laissait l'en-tête LUKS (+ `wipefs -a`)**) | [x] `pools.rs` (24), `cache.rs` (13), `encryption.rs` (2), `snapshots.rs` (1), `handlers/storage.rs` (3 — `vd_name`, +2026-09-07) | [x] `storage_integration.rs` (24, +2026-09-07) — inclut `GET/DELETE /api/storage/virtual-disks[/{name}]` + `/scrub` (split d'auth) + `test_encryption_create_rejects_a_device_that_is_not_available` (garde `get_available_devices` avant `cryptsetup`) ; toujours aucun disque virtuel / volume LUKS réel via l'intégration ; **+2026-10-10 : garde d'auth renforcée sur `POST /storage/disks/{name}/smart/test` — `require_auth` → `require_admin` (lance un vrai `smartctl -t` sur l'hôte) + trace dans le journal d'événements ; matrice lecture/écriture étendue aux 3 endpoints SMART** | [x] `storage.spec.ts` (29 `test(` / 31 runtime) — l'onglet « Disques virtuels » est couvert (nav + décompte 6 onglets) ; **34/34 verts 2026-09-07** ; pas de liste/destruction d'un vrai disque virtuel ; **+2026-09-12 : migration design Appliance — `PageLayout` (6 onglets), cartes Tailwind-on-tokens, `Modal`/`Toggle`/`Alert` conservés, sélecteurs `data-testid="storage-*"`, zéro CSS scopé** |
| 10 | RAID | [x] 2026-07-13 (+09-05 re-scope création uniquement ; **+09-07 refonte layout via `impeccable` — assistant 3 étapes re-skinné mono + tokens `--hv-*`, stepper à 3 points, schéma disques par niveau, cibles tactiles ; markup/CSS seulement ; `svelte-check`/`vite`/`cargo` OK ; vérifié live étape 1** ; **+2026-09-07 test réel sur 6 loop devices : création/scrub/destruction mdadm + Btrfs natif + ZFS de bout en bout via l'API ; 4 bugs corrigés — `mount`/`umount`/`blkid` non-`sudo` dans `disk.rs`, ordre des args `zpool create` (nom avant le type de vdev → toute création ZFS échouait), et le décompte de disques / point de montage faux dans `scan_pools` pour mdadm+ZFS**) | [x] `disk.rs` (12 — validation create_raid + `parse_size`) | [x] `raid_integration.rs` (5, +2026-09-07 — gardes d'auth sur les 5 endpoints `/api/storage/raid*` : lecture = session, création/suppression/rebuild = admin ; `require_admin` rejette **avant** le corps du handler donc `mdadm` n'est jamais atteint) ; toujours aucune création/destruction réelle (host requis) | [x] `raid.spec.ts` (41 `test(` / 43 runtime) **réécrit pour l'assistant de création** — **35 passés / 8 sautés 2026-09-07** (les 8 sauts exigent ≥2 disques libres sur l'hôte pour atteindre l'étape 2/3) ; cycle réel opt-in `RAID_E2E_DEVICES` (créer dans RAID → voir dans Storage → détruire) ; **+2026-09-12 : migration design Appliance — `PageLayout` + wizard 3 étapes en Tailwind-on-tokens (stepper, schémas disques, capacité, récap), sélecteurs `data-testid="raid-*"`, zéro CSS scopé** |
| 11 | Santé (Health) | [x] 2026-07-16 (**+2026-09-07 suites de tests complètes — dernier écran sans couverture**) | [x] `health.rs` (13, +2026-09-07 — précédence `determine_overall_status` (dont le cas vide vacuously-`all()`), serde PascalCase de `HealthStatus` (le mapping minuscule du handler en dépend), `run_all_checks` → 8 checks dans l'ordre + agrégat cohérent, `run_check` dispatch + nom inconnu → `Err`, `check_storage` Healthy/Warning selon `DATA_DIRECTORY`, `check_memory` % borné, `check_network_interfaces` liste toujours `lo`) | [x] `health_integration.rs` (10, +2026-09-07 — `GET /api/health` répond **toujours 200** et signale l'échec d'auth dans le corps (`success:false`, pas de `data`) ; session **non-admin** valide → `success:true` ; 8 checks nommés + statuts minuscules valides + messages non vides ; `overall_status` = `status` ; aucune fuite de statut PascalCase ; agrégat cohérent avec ses checks ; `cpu_usage`/`memory_usage` ∈ 0..=100 ; jamais de 500 ; aucun credential/hash/token renvoyé) | [x] `health.spec.ts` (34 `test(` / 36 tests runtime, 12 catégories — **36/36 verts 2026-09-07** ; lecture seule ; +2026-09-08 : bouton Refresh manuel retiré, revalidation auto au focus (`onAppVisible`) — specs 1.6/3.1/3.2/7.1/8.2/10.2 réécrites ; ouverture via le launcher + via l'icône dock « Security » (`handleDockAppClick('health')`, mislabel historique, asserté) ; 8 cartes ; le badge global suit le pire check (lu depuis l'API brute) ; Refresh est GET-only sans polling propre ; 500/payload malformé → état vide, jamais de fenêtre blanche) ; **+2026-09-12 : migration design Appliance — `PageLayout` + `EmptyState`, Tailwind-on-tokens, zéro CSS scopé, sélecteurs sur `data-testid="health-*"`** ; **+2026-09-14 : standardisation des en-têtes + déplacement d'une vraie lecture** — 1.3 n'attend plus de sous-titre mais `.hv-winhdr-title` seul + `.hv-winhdr-subtitle` `toHaveCount(0)` ; le statut global n'est plus une pastille d'en-tête mais la bande `data-testid="health-overall"` en tête du corps, qui conserve l'ancien `data-testid="health-badge"` : 1.5 vérifie donc la bande **et** la pastille. Le libellé passe par la clé i18n renommée `health.overall_status` (ex-`health.subtitle`, désormais orpheline) — renommage plutôt que suppression pour garder la parité des 4 locales. Nombre de tests inchangé ; **spec modifié mais NON relancé** |
| 12 | Terminal | [x] 2026-07-13 (+ ~6 follow-ups jusqu'au 09-02, +09-06 tests Rust) | [x] `handlers/terminal_pty.rs` (13 — `is_valid_tab_id`, `resolve_tab_id`, `generate_ticket`, cycle de ticket du `PtyManager`, `kill_session` purge les tickets, no-ops de cleanup) | [x] `terminal_integration.rs` (10 — matrice d'auth admin sur `/ticket` + `/close` + `/cwd` + `/ls`, validation du tab id, no-op `close_tab`) | [x] `terminal.spec.ts` (**81 tests runtime**, recompté le 2026-09-17 via `npx playwright test tests/e2e/terminal/terminal.spec.ts --list --project=firefox` — 77 par `grep -cE '^\s*test\('`, l'écart de 4 venant des tests paramétrés en boucle ; l'entrée disait 74 avant le 2026-09-16) ; **+2026-09-17 : `terminal-tabs` ne défile plus verticalement (`overflow-y-hidden` — un `overflow-x: auto` seul calcule `overflow-y` à `auto`, et le 1px que le `min-h-11` laisse sous sa boîte de contenu suffisait ; test 2.9) et `terminal-body` peint le même fond que le thème xterm (test 2.10 — le conteneur était resté en `#1e1e1e`, donc le reliquat sous-cellule du `FitAddon` peignait une bande noire sous la dernière ligne)** ; **+2026-09-12 : migration design Appliance — en-tête `WindowHeader` (et non `PageLayout`, l'entrée était fausse) + onglets xterm + infobar en Tailwind-on-tokens, sélecteurs `data-testid="terminal-*"`, zéro CSS scopé** ; **+2026-09-12 : complétion client (ghost text + popup Tab cliquable, `section 19`)** ; **+2026-09-16 : le strip d'onglets sort du `WindowHeader` — `terminal-tabs` est désormais une barre du contenu de fenêtre, entre l'en-tête et `terminal-infobar` (nouveau test 2.8 : le strip n'est pas un descendant de `.hv-winhdr` et son ordre vertical le place entre les deux barres) ; l'en-tête reçoit son `title` ; 3.2/3.4/7.3/7.4 réparés (sélecteurs périmés, **déjà rouges avant**) ; sections 1/2/3/4/7/8/9/10/11/12/13/14/15/16/17/18 relancées sur instance neuve — toutes vertes sauf 4.2/10.3/19.1/19.2, **prouvées préexistantes par `git stash` du fichier modifié** (4.2 famille monospace ; 10.3 « < 80 requêtes » est un artefact du serveur de dev Vite, qui sert des modules ES non bundlés ; 19.1/19.2 = popup de complétion, zone non touchée)** |
| 13 | Journal système | [x] 2026-07-18 (**+2026-09-07 test réel : source `system`/`syslog`/`auth`/`kern` via vrai `sudo journalctl`/`tail`, filtres unit/priorité/search OK ; 1 bug majeur corrigé — le scheduler d'auto-nettoyage wipait le log applicatif actif à chaque démarrage → source « app » vide en permanence**) | [x] `handlers/journal.rs` (9, +2026-09-07 — `priority_text`, `is_app_log_filename`, `text_log_path` (whitelist / anti-traversée), `strip_target_prefix`, `parse_app_log_line` (timestamp+level+target / repli), `seconds_to_iso`) | [x] `journal_integration.rs` (11, +2026-09-07 — **11/11** — split d'auth (lecture=session, `clear`+`cleanup-settings` PUT=admin), `/sources` liste `app`+`system`, source `app` lue **pour de vrai** depuis `DATA_DIRECTORY/logs` (parsing + filtre `search` + vide), source inconnue → `{success:false}` propre, `clear` ne supprime que `homevault.log*`, réglages cleanup défaut (on/90j) + round-trip + clamp `interval_days≥1`) | [x] `journal.spec.ts` (27 `test(` / 29 tests runtime, 12 catégories — **28/28 verts 2026-09-07** ; lecture seule, ne vide jamais les vrais logs ni ne modifie durablement le réglage cleanup) ; **+2026-09-12 : migration design Appliance — `PageLayout` (toolbar filtres + footer statut), priorités → tokens, sélecteurs `data-testid="journal-*"`, zéro CSS scopé** ; **+2026-09-14 : standardisation des en-têtes — 1.2 n'attend plus de sous-titre mais `.hv-winhdr-title` seul + `.hv-winhdr-subtitle` `toHaveCount(0)` ; les actions réelles de la barre (auto-refresh, Vider, Réglages de nettoyage) restent, seuls les éléments décoratifs ont été retirés. Nombre de tests inchangé ; spec modifié mais NON relancé** |
| 14 | Réseau (Network) | [x] 2026-07-14 (+07-15 ; **+2026-09-07 : reads re-vérifiés live ; `src/network.rs` frozen corrigé — `ip`/`dhclient` de *mutation* wrappés dans `sudo` (`ip_sudo`), `/etc/resolv.conf` + toggles `/proc/sys/net/ipv6/*` via `sudo tee` (`write_as_root`), DNS ne réécrit que les lignes `nameserver` (`build_resolv_conf`, garde `search`/`options`). Persistance netplan/interfaces = follow-up délibéré**) | [x] `network.rs` (32 — +3 `build_resolv_conf`) | [x] `network_integration.rs` (23) | [x] `network.spec.ts` (43) ; **+2026-09-14 : standardisation des en-têtes — 1.2 n'attend plus de sous-titre mais `.hv-winhdr-title` seul + `.hv-winhdr-subtitle` `toHaveCount(0)` ; les boutons Diagnostics/Scanner restent (actions réelles), seule la ligne de description (`network.subtitle`) a disparu. Nombre de tests inchangé ; spec modifié mais NON relancé** ; **+2026-09-12 : migration design Appliance — `PageLayout` (onglets ethernet/wifi), sélecteurs `data-testid="network-*"`, zéro CSS scopé** |
| 15 | VPN | [x] 2026-07-16 | [x] `vpn.rs` (14) | [x] `vpn_integration.rs` (22) | [x] `vpn.spec.ts` (39) ; **+2026-09-12 : migration design Appliance — `PageLayout` (WireGuard/OpenVPN), sélecteurs `data-testid="vpn-*"`, zéro CSS scopé** |
| 16 | Explorateur (Explorer) | [x] 2026-07-14 (+07-17, +09-06 : **5 bugs de robustesse corrigés** + 8 fonctionnalités umbrelOS ; compilé & testé **2026-09-07**) | [x] `browser.rs` (11 — +6 collision-strategy le 2026-09-07, +1 le 2026-09-13 : `copy_dir_all_tracked` enregistre uniquement les chemins créés), `config/explorer_config.rs` (4), `handlers/explorer.rs` (17 — +5 le 2026-09-07 : `kind_for_root`, `resolve_dest` ; +5 le 2026-09-13 : `parse_range` — absent/suffixe/bornage/416/malformé), `media_ticket.rs` (4, le 2026-09-13) | [x] `explorer_integration.rs` (49, +11 le 2026-09-07 — `/roots` (racines + `kind`), `/save` (écriture/écrasement/hors-racine/dossier/payload géant), `/eject` (admin only + refus d'un non-point-de-montage), `/snapshots` (vide hors ZFS), favoris (add/list/remove), `?strategy=keep_both\|skip` sur `/copy`) ; **+9 le 2026-09-13 — transferts v2 : `Range` (206/416/suffixe), digest `X-Content-Sha256`, ticket download (mono-usage, sans session), ticket thumbnail (multi-usage, sans session), upload résumable (round-trip chunks → complet, offset mismatch 409, abort sans débris), rename hors-racine refusé, delete admin-only — **49/49 verts 2026-09-13** ; **+2026-09-12 : `share_links_integration.rs` (11 — matrice d'auth, create/list, refus dossier/fichier manquant/mode invalide, garde `PathGuard` non-admin, download public + token inconnu/révoqué/expiré, `signed_in` sans/avec session, revoke owner-only + admin, `/share/{token}` atteignable via le routeur `create_app` réel)** ; **+2026-09-13 : `share_links_integration.rs` (12) — `Range` sur `/share/{token}`** | [x] `explorer.spec.ts` (62 `test(` au grep ; **64 à `--list` le 2026-09-24** — +8.5 « Space activates a focused folder row, not just Enter » (W18), écrit mais **non exécuté**) — **49/49 verts 2026-09-07** ; **+2026-09-12 : migration design Appliance — `PageLayout` (sidebar emplacements + breadcrumb + vue liste/grille + corbeille), sélecteurs `data-testid="explorer-*"`, zéro CSS scopé** + **section 15 « umbrelOS » ajoutée le 2026-09-08 (9 tests, 3 runs isolés `-g "15"` verts sur serveur frais)** : éditeur inline (le modal expose Edit → textarea pré-remplie), `POST /api/explorer/save` (écrit / écrase / rejette > 5 Mio), favoris (étoile de la barre d'outils ⇄ sidebar ⇄ serveur, ouverture depuis une fenêtre neuve), stratégies de collision `skip`/`keep_both`/`overwrite` sur `/copy` et `/move` (via `fetch` in-page), menu Snapshots (« aucun instantané » hors ZFS + endpoint), `/eject` (admin only + refus d'un non-point-de-montage), hand-off « Partager ce dossier » → l'app Shares s'ouvre avec le formulaire pré-rempli. `apiDownloadLen` passe en `cache: 'no-store'` (seul changement d'un helper partagé). **+2026-09-12 : section 16 — partage par lien (create → GET public anonyme → revoke → 401/400), lien `signed_in` (anonyme 401 / header session 200), garde de fermeture de l'éditeur (modifs non enregistrées → confirm « Ignorer/Enregistrer »)**. **+2026-09-13 : standardisation — sidebar refactorée sur `WindowSidebarItem`/`WindowSidebarSection` (`.disk-btn`/`.disk-name` conservés comme hooks de test), **collapse icon-rail** (`sidebarCollapseable` + `bind:sidebarCollapsed` → rail 64px au lieu de disparition), footer de statut via `WindowFooter mode="status"` (résumé dossier + taille totale + sélection en direct) + **drop d'un fichier OS sur une destination de la sidebar (racine/favori/disque monté → upload sur place, anneau `is-drop-active`)** ; tests **1.6** (collapse ⇄ expanded : classe `hv-winsidebar--collapsed`, labels masqués, tooltips, largeur) et **1.7** (footer statut + compteur de sélection) ajoutés** ; **le même footer de statut (patron Puter `explorer-footer`) est généralisé aux écrans de données : `Syncthing` (folders/devices/connected), `Storage` (disques/virtuels/partitions/amovibles), `Shares` (comptes par protocole), `Downloader` (actifs/terminés), `Network` (interfaces/liens/IP)**.** **+2026-09-13 (transfers v2, à relancer live)** : download via ticket → anchor (plus de `fetch`→`blob`), thumbnails authentifiés par ticket de répertoire (plus de `?token=`), upload chunké résumable. **Non couvert** : la navigation *dans* un vrai instantané ZFS (host requis). **Flakiness/stale locators pré-existants (non liés à cette passe)** : `navigateTo`/`currentPath` reposent sur `.bc-item` (classe absente du breadcrumb actuel → sections 2/3/6/8/10/11 intermittentes) et 1.3/1.4 utilisent des sélecteurs obsolètes (`[data-testid="explorer-page"]` imbriqué, `…"]2`). Relancer sur serveur frais ou par `-g`. |
| 17 | Parts/Protocoles (Shares) | [x] 2026-07-13 (+07-18 SFTP réel ; **+2026-09-07 test réel de bout en bout : vrai partage SMB create/toggle/update/delete + service start/stop ; SFTP key → compte Linux + groupe, grant ACL `setfacl` sur dir root, `rebuild_jails` → bind-mount + `/etc/fstab`, revoke, `purge_user`. 5 bugs corrigés — toggle SMB no-op sur la dernière section de `smb.conf`, écriture `/etc/homevault/*.json` sans `sudo`, tous les `setfacl` sans `sudo`, revoke qui laissait l'ACL par défaut, `set key` qui faisait `useradd` pour un nom inexistant**) | [x] `permissions.rs` (10), `shares.rs` (12), `sftp.rs` (6) | [x] `shares_integration.rs` (18) | [x] `shares.spec.ts` (51) ; **+2026-09-14 : standardisation des en-têtes — 1.2 n'attend plus de sous-titre mais `.hv-winhdr-title` seul + `.hv-winhdr-subtitle` `toHaveCount(0)` ; le récapitulatif « %total% partages · %active% actifs » (`shares.summary`) a disparu avec le sous-titre — les onglets de protocoles restent. Nombre de tests inchangé ; spec modifié mais NON relancé** ; **+2026-09-12 : migration design Appliance — `PageLayout` + onglets protocoles, `SectionCard`-comme formes, `PathField`/`Toggle`/`Modal` conservés, sélecteurs `data-testid="shares-*"` (modal → `[data-slot="dialog-content"]`), zéro CSS scopé** |
| 18 | Cloud Sync (rclone) | [x] 2026-07-14 (+09-06 suites de tests) | [x] `handlers/rclone.rs` (11 — `build_rclone_path`, `get_provider_description`, parseur `rclone.conf`, `MountRequest` defaults) | [x] `rclone_integration.rs` (14 — garde session sur les 12 endpoints + les 4 `/jobs` (liste/CRUD), garde admin sur mutations + lecture config, rejet d'injection `[section]`, CRUD jobs serveur + run sans panique) | [x] `cloudsync.spec.ts` (24 — garde « fuite credentials » sur `GET /config` ; cycle réel de remote **opt-in** `CLOUDSYNC_E2E_WRITABLE_CONF=1`) ; **+2026-09-12 : migration design Appliance — `PageLayout` (3 onglets + modales sync/config), sélecteurs `data-testid="cloudsync-*"`, zéro CSS scopé** ; **+2026-09-22 : jobs de sync persistés côté serveur (`sync_jobs`, migration 036) — le frontend a abandonné localStorage (`api.listSyncJobs/createSyncJob/deleteSyncJob/runSyncJob`)** |
| 19 | Docker | [x] 2026-07-16 (+ ~8 follow-ups jusqu'au 09-04) | [x] `docker.rs` (9), `compose.rs` (6), `app_catalog/*` (15), `app_proxy/*` (2) | [x] `app_catalog_integration.rs` (10), `app_proxy_integration.rs` (2), `compose_integration.rs` (6) | [x] `docker.spec.ts` (51, incl. cycle réel opt-in §13 sur un conteneur `hello-world` jetable, + bouton « Edit » compose §4.8) ; **+2026-09-12 : migration design Appliance — nav rail + métriques + 6 onglets (dont catalogue + compose) + modales inline, sélecteurs `data-testid="docker-*"` (modal → `[data-slot="dialog-content"]`, toggle → `[role="switch"]`), zéro CSS scopé** ; **+2026-09-13 : nav rail + section métriques refactorés sur `WindowSidebarItem`/`WindowSidebarSection` (`role="tab"`, `aria-selected`, roving `tabindex`, `docker-nav-item` conservés)** |
| 20 | Syncthing | [x] 2026-07-16 (+09-06 suites de tests) | [x] `handlers/syncthing.rs` (11 — `extract_api_key`, désérialisation des réponses REST + aliases `certName`/`deviceID`/`myID`) | [x] `syncthing_integration.rs` (5 — garde **admin** sur les 10 endpoints, 422 body, shape de `status`) | [x] `syncthing.spec.ts` (21 — état « non disponible », gardes, chemin d.erreur démon éteint ; lecture live **opt-in** `SYNCTHING_E2E=1`) ; **+2026-09-12 : migration design Appliance — `PageLayout` (3 onglets), sélecteurs `data-testid="syncthing-*"`, zéro CSS scopé** ; **+2026-09-13 : master-detail 2 volets pour les onglets Appareils/Dossiers (liste compacte à gauche, volet détail à droite ; `syncthing-device-list-row`/`syncthing-folder-list-row`, split `ResizeObserver` < 720px → un seul volet) ; l'onglet Aperçu reste inchangé ; `syncthing-btn-primary`/`syncthing-empty`/`syncthing-stats` conservés (E2E inchangé)** |
| 21 | Télécharger (Downloader) | [x] 2026-07-16 (+07-17, +09-06 suites de tests) | [x] `handlers/downloader.rs` (15 — mapping serde du format aria2 réel camelCase ↔ JSON snake_case renvoyé au frontend ; bodies de requête ; enveloppe `ApiResponse`) | [x] `downloader_integration.rs` (8 — garde admin sur les 10 endpoints, validation de body 422, chemin sûr `GET /status`) | [x] `downloader.spec.ts` (40 `test(` / 42 runtime — dont un **téléchargement réel** d.un fichier servi par l.app, jusqu.à `complete` sur disque, puis suppression) ; **+2026-09-12 : migration design Appliance — `PageLayout`, sélecteurs `data-testid="downloader-*"`, zéro CSS scopé** ; **+2026-09-13 : master-detail 2 volets — liste compacte des téléchargements (actifs + terminés) à gauche, volet détail de la tâche sélectionnée à droite (re-résolu par gid à chaque poll pour suivre la progression live ; split `ResizeObserver` < 720px → un seul volet) ; `downloader-card`/`downloader-card-name`/`downloader-action-danger`/`downloader-section-title` conservés sur les rangées (E2E 10.x inchangé)** |
| 22 | Sauvegarde (Restic) | [x] 2026-07-14 (+07-17, +09-06 suites de tests ; **3 bugs trouvés et corrigés** dont la page entière morte dans le navigateur, cf. plus bas) | [x] `restic_scheduler.rs` (12 — `compute_next_run` + `normalize_crontab_dow`), `restic.rs` (8 — `parse_backup_summary` NDJSON, désérialisation snapshots) | [x] `restic_integration.rs` (14 — split auth, garde `include_password`, cycles schedule + retention 100% DB, jamais de `restic`) | [x] `backup.spec.ts` (25 — dont un **vrai backup restic** dans un dépôt jetable + vérif du jour de la semaine du cron ; **+2026-09-22 : 14.3 — régression « Restore » audit : le modal agit sur la snapshot dont le bouton a été cliqué (2e ligne), plus jamais la première — API restic entièrement mockée dans le navigateur, tourne sans restic**) ; **+2026-09-12 : migration design Appliance — `PageLayout` (repos / création / détail, snapshots-jobs-schedules, politiques de rétention), sélecteurs `data-testid="restic-*"`, zéro CSS scopé** ; **+2026-09-13 : les onglets snapshots/jobs/schedules passent de la barre inline faite main à la rangée d'onglets unique du `WindowHeader` (`tabs`/`bind:activeTab` sur `PageLayout`, compteurs conservés)** ; **+2026-09-13 : master-detail 2 volets — cartes de dépôts à gauche (colonne 420px, stats en tête), détail du dépôt / formulaire de création à droite (split `ResizeObserver` < 720px → un seul volet), `restic-repo-name` toujours rendu sur le volet liste** |
| 23 | Rescue | [x] 2026-07-17 (+07-17, +09-05 ; **+2026-09-07 : 1er run réel sur 4 loop devices + RAID1 jetable — ddrescue clone / testdisk analyze / photorec / mdadm diagnose+repair de bout en bout ; 6 bugs frozen corrigés — sudo-wrap ddrescue/testdisk/photorec, `lsblk -P -b` (modèle/transport/removable/taille), en-tête `/dev/md127:` dans `mdadm_diagnose`, santé "clean, degraded"→healthy, `parse_testdisk_output` offsets, `testdisk_rebuild` no-op + anti-hang**) | [x] `rescue/validator.rs` (8) + `rescue/service.rs` (8 — `parse_ddrescue_mapfile` ×5, `parse_testdisk_output` ×3) | [x] `rescue_integration.rs` (28) | [x] `rescue.spec.ts` (53) ; **+2026-09-14 : standardisation des en-têtes + déplacement des pastilles de disponibilité** — 1.2 n'attend plus de sous-titre mais `.hv-winhdr-title` seul + `.hv-winhdr-subtitle` `toHaveCount(0)` ; 1.3 est RÉÉCRIT : la bande de 4 pastilles qui vivait dans l'en-tête a disparu (élément décoratif), la disponibilité est maintenant sur la carte de l'outil (`SectionCard`'s `actions`), une par onglet — le `data-testid="rescue-tool-badge"` est conservé mais déplacé sur un `<span>` englobant, sans quoi `Badge` ne pourrait pas garantir son transfert via `$$restProps`. Le test vérifie donc : aucune pastille dans `.hv-winhdr`, exactement une sur l'onglet actif. Nombre de tests inchangé ; spec modifié mais NON relancé** ; **+2026-09-12 : migration design Appliance — `PageLayout` (4 onglets) + `SectionCard`, `PathField` conservé, sélecteurs `data-testid="rescue-*"`, zéro CSS scopé ; les composants legacy `Card`/`CardHeader`/`CardContent`/`ProgressBar`/`Divider` (dernier consommateur) supprimés de `theme/`** |
| 24 | Assistant IA (Chat) | [x] 2026-07-13 (+09-02, +09-06 suites de tests ; **3 correctifs « chat fonctionnel »** ; **vérifié live avec un vrai Ollama 0.33.3 + gemma3:1b**) | [x] `handlers/chat.rs` (13 — `system_prompt` localisé, `session_error_message`, `reset_ai_model`, `recommend_models` (extrait, pur), désérialisation Ollama) | [x] `chat_integration.rs` (9 — garde session sur les 7 endpoints, garde admin sur pull/model-set, cycle model-set) ; `dependencies.rs` (+1 — `install.sh` installe bien Ollama, **renforcé le 2026-09-23** : le test exige aussi `systemctl is-active --quiet ollama` et le sondage `http://127.0.0.1:11434/`, la vérification finale de l'installateur contrôlant binaire + unité active + API comme le veut la doc Ollama Linux) | [x] `chat.spec.ts` (20 `test(` / 22 runtime — Ollama éteint → repli propre + badge « IA hors ligne » ; **+2026-09-12 : migration design Appliance — historique latéral + fil de messages en Tailwind-on-tokens, sélecteurs `data-testid="chat-*"`, zéro CSS scopé** ; **suite `CHAT_E2E=1` 22/22 avec Ollama réel**, dont §13.1 conversation streamée réelle) |
| 25 | Widgets | [x] 2026-07-16 (+07-18, 07-19) | s/o — handlers DB-only (`monitor_layout`, `active-sessions-count`) | [x] `widgets_integration.rs` (6, +2026-09-07 — **6/6** — `GET/POST /api/monitor-layout` : garde session, `null` avant sauvegarde, round-trip du blob, upsert = 1 ligne, cloisonnement par compte ; `GET /api/security/active-sessions-count` : garde admin, dédoublonnage par utilisateur + sessions expirées ignorées) ; **+2026-09-14 : 6 → 8 — `refreshMs` vit dans le même blob `monitor-layout` (aucune migration, le handler stocke le JSON verbatim) ; un blob hérité sans `refreshMs` round-trip à l'octet près sans que le serveur en invente un, et une nouvelle valeur persiste en laissant ses clés voisines intactes** ; **+2026-09-22 : 8 → 9 — `GET /api/widgets/update-info` : garde session + sans `UPDATE_CHECK_URL` le widget ne fabrique pas de mise à jour (`has_app_update=false`), comparaison versionnelle unit-testée (`is_newer_version`)** | [x] `widgets.spec.ts` (21 `test(` / 23 runtime, 9 catégories — **23/23 verts 2026-09-12** ; snapshot+restore du layout ; toggle d'une tuile → panneau moniteur du bureau apparaît/disparaît, footer, Reset, persistance `POST /monitor-layout`) ; **+2026-09-12 : migration design Appliance — `PageLayout` + footer `WindowFooter`, tuiles Tailwind-on-tokens, zéro CSS scopé, sélecteurs `data-testid="widgets-*"`** ; **+2026-09-12 : 2.2 géométrie cohérente des 11 panneaux (rayon carte 12px, header 48px, bouton fermer 44px non rogné, largeurs 280/300/320 via `--hv-monitor-w*`)** ; **+2026-09-15 : en-tête des panneaux = barre d'app — `WidgetHeader.svelte` passe de « icône bleue + libellé + action/fermer » à l'anatomie `WindowHeader` : barre blanche 44px (`--hv-widget-header-h` 48 → 44), pastille d'accent 28px (`--hv-winhdr-*`, couleur par moniteur = celle de `WIDGET_DEFS`, glyphe blanc) + titre 14px/600, **sans bouton fermer/action** (la visibilité se gère dans Settings > Widgets) ; les 11 clés i18n `desktop.close_*_monitor` supprimées ×4 locales ; 2.2 réécrit (header 44, pastille 28 centrée, plus de bouton fermer) — 23/23 verts** |
| 26 | Paramètres (Settings, cœur) | [x] 2026-07-16 (+ ~5 follow-ups jusqu'au 09-04 ; **+2026-09-07 test réel Pare-feu : vrai cycle `ufw enable`/`disable` (port 22 protégé), verrou `{confirmed}`, 403 sur règle obligatoire, add/delete de règle. 1 bug corrigé — `firewall.rs::get_status()` lisait la chaîne anglaise `Status: active` → sur locale non-anglaise (`fr_BE` → `État : actif`) le pare-feu affichait toujours « désactivé »**) | [x] cartes sécurité (`firewall.rs` 9, `antivirus.rs` 8, `ransomware_watcher.rs` 7, `ransomware.rs` 3) ; `snmp.rs` (**5** — +2026-09-15 : parser réécrit sur les OID **numériques** (`snmpwalk -On`) ; l'ancien matchait les noms symboliques `IF-MIB::ifDescr` qui n'existent pas sans les fichiers MIB → interfaces/stockage toujours vides ; nouveaux tests `parse_interface_stats`/`parse_storage_stats`/`split_oid_line`/`normalize_mac` sur de vraies sorties numériques), `ups.rs` (**7** — +5 le 2026-09-15 : `parse_ups_conf`/`upsert_ups_section`/`remove_ups_section`/`validate_ups_entry` pour les sections d'appareil de `ups.conf`), `quota.rs` (7 — `bytes_to_quota_kb`/`format_size_kb` seulement ; les tests des quotas de groupe supprimés avec la fonctionnalité) ; **cœur (2026-09-08) : `handlers/settings.rs` (13 — `parse_version`/`is_newer_version`, `sanitize_hostname`, `UpdateFrequency` défaut = `Manual` (G2), défauts d'apparence, `normalize_accent` qui réécrit l'accent legacy `#4a90d9` → `#1E62AE` §1.4.3) + `settings_persistence.rs` (7 — `export()` masque `smtp_password`, round-trip `import()`, ré-import d'un export masqué ne perd pas le mot de passe)** | [x] `security_integration.rs` (18) ; `ups_integration.rs` (**4**, 2026-09-15 — `/ups/config` GET/POST/DELETE : sans jeton → 401, jeton invalide → 401, non-admin → 403, admin GET → 200 ; le chemin d'écriture admin est couvert en live uniquement) ; **cœur : `settings_core_integration.rs` (13, 2026-09-08 — split admin sur reads+writes, `/settings/language` toute session, `/settings/public` sans auth, round-trip apparence (accent/densité/wallpaper), `GET /settings/export` masque `smtp_password`, export→import, `reset` restaure les défauts (dont G2), clamp du seuil de notification)** | [x] `settings.spec.ts` (94 `test(` / 97 runtime — 77/77 vert le 2026-09-09 ; **+2026-09-12 : migration design Appliance — `PageLayout` (rail catégories) + cartes auto-save en Tailwind-on-tokens, cartes sécurité/TLS/monitoring intégrées conservées, sélecteurs `data-testid="settings-*"` (16.5 réécrit pour l'anatomie PageLayout standard), zéro CSS scopé** (run complet) ; nouveaux 2.5 + 12.2 et les sections 1/2/12/16 re-vérifiées vertes contre le binaire reconstruit ; **(a) barre latérale regroupée 12 → 7 catégories** (email→Notifications, météo→Apparence, MAJ/infos/données/confidentialité→Système) ; `openCategory()` sélectionne par libellé, pas par index ; **(b) suppression du bouton « Enregistrer » — auto-save par groupe (debounce 700 ms)** : section 15 « Save All » → « Auto-save » (15.1 un seul PUT du groupe modifié, 15.2 persistance après réouverture, 15.3 statut d'en-tête, 15.4 aucune modif = aucun PUT), 21.3 → coalescence des éditions rapides, 1.2/1.3/4.6/9.4 adaptés ; test 11.2 retiré — `.settings-privacy-link` = CSS mort dans le `Settings.svelte` gelé ; 4.3 `loading=lazy`→`decoding=async`, 4.2 16→15 vignettes ; **4.4/4.5/18.3 : bug de réactivité des vignettes de fond d'écran CORRIGÉ le 2026-09-09** — `isWallpaperSelected()` (helper lisant `$wallpaperStore`) ne rafraîchissait que `class:is-selected`, pas `aria-checked` ni le badge ✓ ; remplacé par un `$: selectedWpUrl` réactif — les 3 se mettent à jour au clic) ; **+2026-09-09 : test 2.5 ajouté (78) — aucune carte ne tronque son contenu ; bug de layout CORRIGÉ — `.set-content` est un flex colonne et chaque carte a `overflow:hidden`, ce qui ramène sa taille flex mini à 0 → le flexbox écrasait les cartes hautes et leur contenu était coupé ; `.set-content > :global(.settings-card) { flex-shrink: 0 }`** ; **+2026-09-09 : test 12.2 ajouté (79) — dans Sécurité, plus de saisie manuelle de chemin : chaque champ dossier est un affichage lecture seule + bouton « Sélectionner un dossier » qui ouvre `FloatingPathPicker` (panneau flottant déplaçable, non-modal, `use:portal` vers `<body>`, se ferme sur Échap)** ; **+2026-09-09 : bug d'affichage des wallpapers CORRIGÉ — `.settings-wallpaper-thumb` n'avait pas `height:auto`, l'attribut `<img height="200">` fixait la hauteur → vignettes en bandes verticales déformées sur grille étroite ; **+ box-shadow retiré des vignettes** (le `--hv-shell-shadow-card` bavait ~24px et faisait paraître les rangées collées/superposées ; sélection = anneau `inset` qui ne déborde pas) ; grille 2 colonnes sous 560px, gap 12px partout ; test 4.7 étendu (gap col == gap ligne, pas d'ombre externe) ; **8ᵉ catégorie « Widgets »** ajoutée, le widget Météo déplacé d'`appearance` vers `widgets` (`settings.widgets_title`/`_subtitle` ×4 locales) ; `settings.spec.ts` CATEGORIES/GROUP_OF/GROUP_LABEL + test 1.4 « 8 entries » adaptés** ; **+2026-09-09 : carte « Quotas disque » (par groupe) + tout le backend `/api/quotas` SUPPRIMÉS** — `QuotaCard.svelte`, `handlers/quotas.rs`, `storage/quota_repo.rs`, `tests/quotas_integration.rs`, les méthodes de groupe de `QuotaService`, `create_group_with_quota`/`delete_group_cascade` (0 appelant) et les clés i18n `settings.disk_quota_*`/`quota_*` ; catégorie renommée « Supervision et stockage » → « Supervision » ; test 13.1 asserte l'absence de la carte quota ; les quotas se gèrent depuis Utilisateurs/Groupes**), `security.spec.ts` (30) ; **+2026-09-09 : carte « HTTPS / TLS » (Sécurité) — HTTPS intégré opt-in (self-signed `rcgen` + certificat fourni, redirection HTTP→HTTPS, HSTS conditionnel). Rust : `src/tls.rs` (7 — port effectif, validation ports/chemins, génération + introspection self-signed, idempotence) + `src/handlers/tls.rs` (1) + `tests/tls_integration.rs` (8 — guards admin 401/403/200, rejet port==HTTP, mode manuel chemins manquants/`..`, self-signed persiste + `requires_restart`, `/regenerate` écrit le cert, paire PEM réelle acceptée, aucune fuite de clé). Live vérifié : `https://:8443` → 200, HSTS présent en HTTPS / absent en HTTP loopback, redirection 301 hors-loopback, SAN = localhost/hostname/homevault.local/IP LAN. E2E `settings.spec.ts` 12.3–12.6 ajoutés (chargement carte, révélation des champs, port==HTTP → Save désactivé, pas de clé en URL/localStorage)** ; **+2026-09-13 : rail catégories refactoré sur `WindowSidebarItem` (collapse icon-rail `sidebarCollapseable` + `bind:sidebarCollapsed`, `settings-nav-item`/`set-nav-active` conservés)** ; **+2026-09-14 : re-skin complet sur la maquette Figma `Sans titre` node `16-6187` — rail à 2 lignes 208px (`WindowSidebarItem variant="rich"` : pastille 28px teintée à 9 %, aplat + glyphe blanc + barre 4×20 à droite quand actif), en-tête de contenu (pastille 36px + titre + sous-titre), et vocabulaire partagé section + carte + rangée (`components/settings/vocabulary.ts`) appliqué aux **8 catégories** ; les 6 cartes embarquées (TLS, Antivirus, Anti-Ransomware, Pare-feu, UPS, SNMP) **reconstruites** sur ce vocabulaire à comportement gelé (hooks classes/id conservés) ; nouveau `theme/Switch` (vrai `input[type=checkbox]` dans `.settings-toggle-label`, piste 40×22, knob 16px) qui remplace `Toggle` sur **tout** l'écran, carte TLS comprise (ses 2 bascules étaient les 2 seuls appelants de `Toggle` dans Settings ; `theme/Toggle` reste le primitive à bouton `role="switch"` des autres apps — Shares, Docker, Restic, Storage) — le `.hv-toggle-switch` (44×24, knob 20px, bleu de marque) qu'elles ciblaient n'avait plus de producteur, d'où l'échec pré-existant de 12.3/12.4/12.5, réécrits sur `flipSwitch()` ; palette page-scopée `--hv-settings-*` + 8 accents `--hv-settings-cat-*` dans `tokens.css` ; catégorie **Widgets** branchée sur le réel — les 11 moniteurs de `WIDGET_DEFS` et un intervalle de rafraîchissement persisté dans le blob `monitor-layout` (`refreshMs`, **zéro migration** : `POST /api/monitor-layout` stocke le JSON verbatim) ; `settings.spec.ts` **80 → 95 tests** (11 nouveaux en §23, `--list` = 95 — le `grep -cE '^\s*test\('` en compte 92 car la ligne 1128 génère 4 tests par boucle ; les 11 groupes de sélecteurs morts de la §16 et 2.5/12.2 mis à jour pour la nouvelle coquille), `widgets_integration.rs` 6 → **8** (`refresh_ms` round-trip d'un blob hérité + persistance d'une nouvelle valeur)** ; **+2026-09-15 : suppression de TOUTE sauvegarde auto — un bouton « Enregistrer » unique dans le footer du rail (`[data-testid="settings-save"]`) vide chaque groupe modifié via le store partagé `stores/settingsDraft.svelte.ts` (brouillon `draft.*` + baseline JSON par groupe + `saveAll()` séquentiel) ; les 6 cartes embarquées perdent leurs boutons d'enregistrement individuels (leurs champs éditables sont dans le même brouillon, les *actions* — scan, régénération cert, ajout/suppression de règle — restent immédiates) ; autostart/AI/widgets/météo passent au brouillon, poussés uniquement au clic sur Enregistrer ; pilule d'en-tête `settings-save-status` (non enregistré / en cours / échec) ; E2E réécrits : 1.3, 3.4, 4.6, 5.3, 9.4 (commentaire), 12.3, 12.5, §15 « Auto-save » → « Save button » (15.1 la modif seule ne PUT pas, 15.2 persistance via bouton + réouverture, 15.3 statut en vol, 15.4 aucun PUT sans édition, **+15.5 bouton désactivé pendant l'enregistrement**), 21.3 (aucun PUT avant Enregistrer, un seul après), 23.7/23.8 (bascules widgets/intervalle : staged puis POST au clic) ; +1 test (95 → 96)** ; **+2026-09-15 (monitoring SNMP/UPS) : 3 WARN de démarrage supprimés (`snmp.rs`/`ups.rs` : les prédicats `is_available()` ne loggent plus en warn — `debug!` ; `ufw` modélisé présence-only dans `dependencies.rs`, cf. guide Pare-feu). Parser SNMP réécrit (voir tests unitaires) + vue `snmpd` étendue aux interfaces/stockage ; carte `SnmpCard` alignée sur les champs backend (`bytes_in`/`bytes_out`, `mount_point`, `total`/`used`/`usage_percent`) + `formatBytes`. UPS : NUT configuré en auto-détection (`MODE=standalone` + `usbhid-ups port=auto`, aucun id modèle codé en dur) par `install.sh` ; un UPS configuré mais non connecté logge en `debug!`. Vérifié live sur le serveur : `snmpd`+`nut-server` actifs, `/api/snmp/{availability,info,interfaces,storage}` et `/api/ups/{availability,list}` OK.** ; **+2026-09-17 : `settings.spec.ts` **96/96 vert** contre le binaire reconstruit (Firefox, `--project=firefox`) ; le seul rouge restant, **23.5** (libellés de section en micro-libellés, `<= 12px`), est corrigé — trois tailles écrites `text-[var(--hv-text-xs)]` sans le type hint `length:` compilaient en `color:` et les libellés héritaient du 14px de leur parent (détail, portée et pièges de vérification dans la section « troisième bug de la même famille » plus bas). `npm run check` : **7793 fichiers, 0 erreur, 0 warning**. Le run complet a été tué par le noyau à 91/96 (pression mémoire de la machine, pas un échec) ; les 5 derniers (23.7–23.11) ont été relancés seuls et passent — **5 passed (1.0m)**. **+2026-09-23 : le bouton « Enregistrer » devient l'indicateur de sauvegarde unique** — la pastille d'état visible au-dessus du bouton et la légende « N modification(s) non enregistrée(s) » en dessous sont supprimées (composant `SettingsSaveStatus.svelte` supprimé, import + snippet de footer nettoyés dans `Settings.svelte`) ; le bouton expose `data-state` = `clean` (bleu `--hv-primary`, « Save ») → `dirty` (rouge `--hv-error`, « Save changes ») → `saving` (bleu, désactivé, « Saving… ») → `saved` (vert `--hv-success`, « Saved », tenu 2,5 s puis retour au bleu) / `error` (rouge, « Not saved ») ; le libellé et l'icône changent **toujours** avec la couleur (WCAG 1.4.1 ; blanc sur les 3 fonds : bleu 6,15:1 / rouge 5,73:1 / vert 5,14:1 — le bleu suit l'accent `--hv-primary` ré-hydraté au boot, dont le défaut livré vaut `#1E62AE` depuis le 2026-09-23) ; une édition posée pendant le flash vert l'annule, un groupe en échec gare le bouton sur `error` (jamais « Saved »). `settingsBtnSuccess` ajouté à `vocabulary.ts` (une peau par état, jamais deux `bg-` sur le même élément) ; clés `settings.save_changes` / `settings.save_error` ×4 locales en remplacement de `settings.unsaved_changes` ; `settings-save-status` conservé mais en **sr-only `role="status"`** (les lecteurs d'écran entendent chaque changement d'état, le rail n'affiche qu'un élément). E2E réécrits : helper `saveSettings` attend `data-state = saved|clean` (un échec se gare sur `error` et fait donc échouer l'attente), **1.3** sur `data-state` + présence de la ligne sr-only, **15.3** vérifie les 3 couleurs (`toHaveCSS` sur des RGB **lus dans la page** — `--hv-primary` est ré-hydraté depuis l'accent, le jeton par défaut ne correspond pas au rendu) **et** les libellés en bleu → rouge → en vol → vert → bleu (pointeur écarté avant chaque assertion de fond, sinon la survoleture lit `-*-hover`), **14.5** cible `.settings-data-actions .settings-btn-danger` (le bouton de rail porte la même peau quand une modification est en attente — stricte mode sinon). **96 tests inchangés** ; `npm run check` 0 erreur. **Bilan 2026-09-23 : 96/96 verts (9,4 min, Firefox, instance scratch `:8099`)** — le premier tir était intégralement rouge (96/96) parce que les binaires Playwright étaient absents de la machine (`npx playwright install firefox` requis), le second 94/96 avec deux rouges **hors de la modification** : **15.2** a révélé un vrai bug de cache (`put('/settings/<groupe>')` n'évinçait pas l'agrégat `GET:/settings` à TTL 5 s que la fenêtre re-sème à la réouverture → correctif `invalidateForMutation()` dans `api.ts`, désormais couvert par 15.2) et **15.3** comparait sur les RGB codés en dur de `tokens.css` alors qu'`App.svelte` ré-hydrate `--hv-primary` depuis l'accent (`#4a90d9`) → les RGB se lisent maintenant dans la page. **+2026-09-23 (contraste standardisé)** : l'accent par défaut passe de `#4a90d9` (3,34:1 derrière le blanc **et** ≤3,34:1 en texte sur les fonds clairs → échec §1.4.3 dans les deux sens) à `#1E62AE` = `--hv-primary` (6,15:1 / 4,99–5,68:1), réécrit à chaque entrée par `DEFAULT_ACCENT` + `normalize_accent` (défaut, chargement persisté, import, `PUT /settings/appearance` ; l'accent n'a pas d'UI d'édition, aucun choix opérateur n'est écrasé) ; dégradé de marque `--hv-app-accent-grad` : butée claire `#4b8ad4` (3,57:1) → `#3275C4` (4,69:1) ; `theme-color` + manifest alignés ; **+1 test unitaire** (`normalize_accent`) et **+1 test d'intégration** (PUT legacy → `#1E62AE`, agrégat compris) → `settings.rs` 12→13, `settings_core_integration.rs` 12→13, **E2E settings 96 → 97 —
exécuté le 2026-09-23 : 97/97 vert (nouveau garde **17.5** : les 8 carreaux de catégorie du rail
mesurés dans les deux états — glyphe/teinte au repos, glyphe blanc/aplat en actif — ≥3:1 §1.4.11,
RGB lus dans la page). **Palettes étendues au même passage** : avatars centralisés dans `$lib/utils/avatars.ts` (source unique — `#4B8AD4`/`#8b5cf6` → `#3275C4`/`#7C3AED`, initiales **blanches** aussi dans Groupes qui peignait l'encre sombre → 6/6 stops ≥4,5:1 ; garde `groups.spec.ts` **16.1**), anneau + légende disques Storage (`#14b8a6`/`#f97316` → `#0F766E`/`#C2410C`, ≥3:1 §1.4.11 sur blanc ; le gris « non partitionné » `#e5e7eb` reste la valeur *vide*, à côté de son libellé texte), tuile Chat (`#14b8a6` → `#0d9488`, glyphe blanc 2,49→3,74:1), **accents de catégorie** (le carreau plein = repère d'état du contrôle, §1.4.11) : widgets `#A78BFA`→`#7C3AED` (2,72→5,70), notifications `#F59E0B`→`#B45309` (2,15→5,02), ai `#06B6D4`→`#0E7490` (2,43→5,36), monitoring `#10B981`→`#047857` (2,54→5,48) — les 4 autres (\\*-500) passaient déjà (≥3,32:1), et **échelle Login/Wizard** (l. 1) : `--hv-login-accent` `#6366F1`→`#4F46E5` (4,47→6,29 en texte), `accent-deep`→`#4338CA` (6,29→7,90), `soft-border` `#A78BFA`→`#7C3AED` (2,72→5,70 — c'est aussi le texte des étapes terminées), dégradés logo (butées 4,47/2,72 → 6,29/5,70 sous le chiffre blanc) et bouton (4,47→6,29), anneau de focus suivant la nouvelle teinte, garde `login.spec.ts` **13.6** → suite 94→**95** (`--list`) / 90→**91** (grep). |
| 27 | VirtualMachines | [x] **2026-09-10 — vérifié en direct** contre un vrai Incus 6.0.0 (hôte avec `qemu-system-modules-spice` + `incus admin init`) ; seul point non couvert : boot d'un ISO Windows réel (aucun ISO sous la main). **2026-09-09 : app réécrite de fond en comble** (contrat frontend↔backend réaligné ; assistant grand public à **2 modes** : (a) catalogue d'OS curé + presets + compte semé par cloud-init (mot de passe haché SHA-512, jamais en clair), (b) **installation depuis un ISO Linux/Windows** — picker de fichier, VM vide, écran graphique noVNC via `raw.qemu -vnc 127.0.0.1:…`, Windows = disque NVMe + TPM 2.0 + Secure Boot + ISO pilotes VirtIO optionnel) ; attente réelle des opérations Incus ; job de création tracké ; console shell xterm (`tokio-tungstenite`) ; onglets Images/Réseaux supprimés. **Vérifié en direct de bout en bout** contre un vrai Incus (après `apt install qemu-system-modules-spice`) : (a) conteneurs — création via l'assistant/API, console shell xterm réelle, instantanés create/restore/delete, resize, suppression ; (b) **VM ISO Linux** — assistant → `create` → Incus crée la VM vide, `raw.qemu -vnc 127.0.0.1:20` + TPM/NVMe/SecureBoot appliqués, **la VM démarre**, l'ISO Alpine boote ; **l'écran noVNC affiche la console de boot Alpine en direct dans le navigateur** (canvas 1280×800, handshake RFB relayé, poignée Ctrl+Alt+Suppr) ; `delete` arrête puis supprime la VM. **1 bug trouvé et corrigé en direct** : Incus envoie `state.disk: null` sur une VM fraîchement démarrée → `#[serde(default)]` échouait (`invalid type: null, expected a map`) et cassait tout `/api/vm/list` ; helper `null_default` ajouté (couvre `state.disk` + `config`/`devices`/`expanded_devices`). Correctifs `install.sh` + `dependencies.rs` (paquet spice) ajoutés. **2026-09-10 — durcissement « corrige tout »** : (A) contrôle OVMF Secure Boot avant création (`OVMF_VARS_4M.ms.fd` **confirmé présent** sur Ubuntu) + **auto-réparation TPM** — si une VM avec `vtpm` échoue au démarrage (Incus 6.0.0 ne câble pas swtpm), le job supprime la VM ratée (attend l'op) puis recrée sans TPM avec un display VNC neuf → VM Windows-10-capable qui démarre ; (B) job de création passé en « préparation » générique ; (C) `ensure_readable_by_qemu` — `chmod o+r` sur l'ISO + `o+x` sur chaque dossier parent, erreur nommant le dossier bloquant ; (D) `RESERVED_DISPLAYS` (mutex, réservation 60 s) contre la collision de display en création concurrente + auto-réparation du port VNC au `start` + timeout 45 s sur les requêtes Incus (`/api/vm/list` ne peut plus se figer) ; (F) **bibliothèque d'ISO** — téléchargement depuis un lien (`POST /api/vm/isos/download`, job de progression), bouton « pilotes VirtIO » (`fedorapeople.org/.../virtio-win.iso`), `safe_iso_name`, suppression. **Vérifié en direct** : création concurrente Linux + Windows ISO — les deux VM RUNNING, displays 21/22 sans collision, `win1` auto-réparée sans TPM ; téléchargement d'ISO (67 Mo) OK ; chips de bibliothèque + bouton VirtIO rendus dans l'assistant. Reste non vérifié (aucun ISO Windows sous la main) : boot réel d'un ISO Windows + 2ᵉ CD-ROM pilotes VirtIO ; TPM 2.0 réel nécessite un Incus plus récent que 6.0.0 | [x] `incus.rs` (11 — + `null_default` : `state.disk`/`config`/`devices` en `null` explicite tolérés) + `handlers/vm.rs` (14 — +`vnc_display`/`has_screen`, `validate_local_iso` (absolu/traversée/extension/taille/`chmod o+r`), `alloc_vnc_display` dans la plage 20–119, `safe_iso_name`, `human_bytes`, sondes de capacité (`swtpm`/OVMF) sans panique ; `valid_instance_name`/`valid_unix_username`/`valid_snapshot_name`, `size_preset`, `normalise_status`, `parse_size_to_mib`, `instance_to_info` mappe le contrat de fil, `pretty_os` repli) | [x] `vm_integration.rs` (22 — auth manquant (400)/bidon (401)/non-admin (403) sur **tous** les endpoints (dont `screen/ticket`) ; admin atteint chaque handler sans 404/405 ; `available`/`os-options` sans Incus ; `list` = message propre sans fuite de socket ; validation `create` **avant** Incus : nom, OS inconnu, kind inconnu, mot de passe court/absent, login invalide, alpine en VM, **+ ISO : chemin relatif/mauvaise extension/fichier manquant, ISO+OS mutuellement exclusifs, ni l'un ni l'autre, ISO valide → atteint Incus** ; `config` (bornes, corps vide) ; nom d'instantané ; ticket console **et écran** exigent une machine en marche) | [x] `vm.spec.ts` (43 tests, 14 catégories ; **+2026-09-12 : migration design Appliance — `PageLayout` (liste machines) + drawer détail + assistant catalogue/ISO en Tailwind-on-tokens, console/screen overlays conservés, sélecteurs `data-testid="vm-*"` (wizard → `[data-slot="dialog-content"]`), zéro CSS scopé** — 3 états initiaux ; assistant catalogue **et ISO** (bascule de mode, toggle Linux/Windows, 1 vs 2 pickers de fichier, plus de champ mot de passe en ISO, blocage sans ISO) jusqu'à « Annuler » ; contrat backend par `fetch` (`os-options`, shape de `list` dont `has_screen`, rejets `create` catalogue **et ISO**, ticket écran) ; tiroir de détail ; console xterm ; responsive 360/768/1920 ; a11y ; sécurité (aucun secret exposé, non-admin rejeté sur tous les endpoints dont `screen/ticket`, XSS nom non reflété) ; clavier ; réseau ; **cycle réel opt-in `VM_E2E=1`** ; régression mémoire) ; **+2026-09-13 : master-detail 2 volets — le drawer détail (anciennement `fixed` portaled) devient une colonne inline de 420px à droite de la liste (split `ResizeObserver` < 720px → un seul volet), `vm-drawer`/`vm-d-specs`/`vm-d-danger` conservés, fermeture Échap/X préservée (`vm.spec.ts` 5.1/5.2 inchangés)** |

### Sous-fonctionnalités notables (intégrées à un écran ci-dessus)

| Sous-fonctionnalité | Audit manuel | Tests unitaires Rust | Tests d'intégration Rust | Tests E2E Playwright |
|---|---|---|---|---|
| Suite sécurité — Antivirus/Anti-Ransomware/Pare-feu (dans Paramètres) | [x] 2026-07-16 (+ 09-06 : scan EICAR réel + honeypot réel ; **+2026-09-09 : chemins des dossiers = choix uniquement, plus de saisie manuelle — nouveaux `theme/PathPickerField` + `theme/FloatingPathPicker` (panneau flottant non-modal déplaçable, `use:portal` vers `<body>`), composant standard réutilisable (`mode="folder"\|"file"`, barre latérale d'emplacements — disques physiques/USB/RAID/cloud/réseau, chacun affiché par son label FS ou marque+modèle+n° de série + taille — + bascule liste/grille persistée `localStorage`) ; `handle_explorer_disks` (gelé) enrichi de `label`/`vendor`/`model`/`serial` (1 passe `lsblk -P`) ; `PathField`/`FolderPicker` (modal) inchangés**) | [x] `firewall.rs` (9), `ransomware_watcher.rs` (7), `antivirus.rs` (8), `ransomware.rs` (3) | [x] `security_integration.rs` (18) | [x] `security.spec.ts` (30 — 2.4 : le bouton « Sélectionner un dossier » ouvre un panneau `.hv-fpp` non plein écran, aucun `.hv-fp-overlay`, drag-handle, barre latérale d'emplacements (ligne disque = taille/n° série), bascule liste/grille (persistée), ferme sur Échap ; 2.2 renommé : champ `#avScanPath` `readonly`) |
| Quotas de transfert (dans Users/Groups/Explorer) | [x] 2026-07-18 (+07-21 vue d'ensemble) | [x] `transfer_quota.rs` (9, +2026-09-07 — `check_quota` : bornes, quota le premier dépassé, valeurs multi-Tio sans débordement i64, `Display`) | [x] couvert indirectement par `explorer_integration.rs` (gardes de quota au download 429 / à l'upload) | [x] couvert par `users.spec.ts`/`groups.spec.ts` |

**2026-09-13 (transfers v2)** — download : réservation sous verrou par-sujet + remboursement si abandon, ticket download (check quota avant le flux), `Range` (seuls les octets du range sont réservés) ; upload multipart et chunké : verrou par-sujet tenu pendant tout le transfert + re-check sous verrou à la complétion (anti-course TOCTOU).
| Inventaire des dépendances système ↔ `install.sh` | [x] 2026-09-06 (paquets manquants ajoutés : ddrescue, testdisk, wireguard-tools, openvpn, restic, syncthing, cryptsetup, dmidecode, ethtool, snmp) ; **+2026-09-23 : paquets manquants ajoutés — `git` (catalogue d'apps Docker), `unzip`/`zip`/`7z` (compression/extraction Explorer-Browser), `pciutils` (carte GPU), `hostname`, `python3` (login PAM)** ; parseur du test élargi aux `install_packages` monolignes (paquets après le guillemet fermant du label — ex. `ffmpeg`), 7 entrées `System` ajoutées à `specs()` ; **+2026-09-23 : `7z` installé de façon adaptative (`7zip` sur Debian 12+/Ubuntu 23.04+, `p7zip-full` sinon) — `apt-get install` échoue tout ou rien si UN paquet du lot est indisponible, donc le groupe archives ne plantait plus en bloc ; `install_packages` a maintenant un repli paquet-par-paquet + requêtes `apt-cache policy` avec `LC_ALL=C` (étiquette localisée `Candidat` ≠ `Candidate`)** ; **+2026-09-23 : `install.sh` restructuré en 5 étapes type « curl-pipe » — l'archive applicative est désormais téléchargée par le script (`HV_RELEASE_URL`, ou `$1` pour un fichier local, plus d'argument obligatoire), le service systemd et l'automontage udiskie sont **embarqués** (un piped `curl | sudo bash` n'a aucun checkout sur disque) avec repli sur `installer/homevault.service` / `installer/setup-automount.sh` quand le dépôt est présent, Étape 4 = vérification globale (paquets + service + réponse HTTP + emplacement du binaire), Étape 5 = message final en anglais + adresses locale/internet** | [x] `dependencies.rs` (**9** — chaque paquet de l'inventaire doit figurer dans `installer/install.sh`, + `test_embedded_unit_matches_the_tracked_service_file` et `test_embedded_automount_matches_the_tracked_helper` qui verrouillent l'unité systemd et l'automontage embarqués contre les fichiers suivis) | n/a | n/a |

## Reste à tester en priorité (aucun test automatisé du tout)

**2026-10-07 — Quotas sorti de cette liste** (demande explicite « implémente
[la] tâche 01 »). C'était le **dernier écran** qui n'avait ni audit live ni
exécution de spec : le spec écrit le 2026-09-22
(`tests/e2e/quotas/quotas.spec.ts`) est **38/38 vert** sur serveur scratch
(Firefox, `PORT=` + `DATA_DIRECTORY=` isolé) et l'écran a été audité en direct
(détail dans l'addenda 2026-10-07 de `CLAUDE.md` et la ligne 6 du détail par
écran). **Aucun sélecteur mort corrigé** — le spec avait été écrit après la
migration Appliance du 2026-09-12 et cible déjà les `data-testid`, il passait
tel quel. La compilation était acquise depuis le 2026-09-14 (`npm run check`
0 erreur après correction de 2 vrais défauts dans `Quotas.svelte`, chunk
`Quotas-*` émis, `cargo build` OK). **Reste ouvert, hors périmètre de cette
passe :** aucun fichier d'intégration propre — l'unique endpoint
`GET /transfer-quotas/{type}` reste couvert par `transfer_quota.rs` (unitaires)
+ `explorer_integration.rs` (gardes de quota). **Conséquence :** plus aucun des
27 écrans n'est sans test automatisé d'au moins une catégorie.

**2026-09-07 — Santé sorti de cette liste** (demande explicite « implémente
les tests complets pour Santé ») : `#[cfg(test)] mod tests` dans
`src/health.rs` (13), `tests/health_integration.rs` (10) et
`tests/e2e/health/health.spec.ts` (36 tests runtime, 12 catégories,
lecture seule — **36/36 verts** contre le serveur live).
`handle_health_endpoint` est passé `pub` (dans le fichier
frozen `src/web/mod.rs`) pour permettre le test d'intégration à une route
— comportement, check de session et noms de champs `checks`/
`overall_status`/`status` inchangés. Aucun bug trouvé, l'audit du
2026-07-16 tient toujours. **Ce qui n'est pas fait :** forcer un check en
état `warning`/`unhealthy` (dépend du host — l'agrégat est vérifié par
cohérence avec ce que le host renvoie).

**2026-09-07 — Desktop/shell sorti de cette liste** (demande explicite
« continue Desktop/shell ») : `tests/desktop_integration.rs` (3 — **3/3** —
gardes d'auth sur `POST /api/server/restart` + `/shutdown`, jamais avec un
token admin car ça `spawn` un vrai `shutdown -r/-h now`) et
`tests/e2e/desktop/desktop.spec.ts` (35 tests, 12 catégories — **35/35
verts** le 2026-09-08 contre le binaire reconstruit ; test 4.4 ajouté avec
la standardisation des popovers topbar : les 3 (réseau / compte /
notifications) partagent désormais tout le chrome de carte via
`.hv-topbar-panel` ; 2 bugs shell trouvés+corrigés :
le point d'activité du dock ne s'affichait jamais pour une app fraîchement
ouverte — condition `{#if}` non réactive dans `Desktop.svelte` — et Échap
ne fermait pas le lanceur quand le champ de recherche avait le focus —
`stopPropagation` trop large sur `.lp-panel` dans `HvLaunchpad.svelte`).
La logique/unitaire est **sans objet**. E2E : rendu du bureau/dock/pill topbar/horloge/pill
utilisateur ; le dock ouvre une app + point d'activité + dédoublonnage au
re-clic ; fenêtres (fermer / réduire→dock / maximiser→restaurer) ;
panneaux topbar mutuellement exclusifs (menu compte ↔ notifications, clic
extérieur ferme) ; lanceur d'applications (ouverture, filtre de recherche,
clic sur une tuile ouvre l'app + ferme le lanceur, Échap) ; recherche
globale (Ctrl+K → overlay → résultat ouvre l'app + ferme) ; menu Power
(ouverture, l'option Restart montre une confirmation qu'on **annule** —
`/api/server/*` **jamais** appelé) ; responsive (< 480 px cache
date/username), a11y (dock = boutons labellisés, pas d'`id` dupliqué),
console sans erreur, cas limites, régression mémoire. **Jamais fait :**
confirmer un restart/shutdown/logout (le premier redémarre l'hôte, le
dernier terminerait la session en cours de suite).

**2026-09-16 — lanceur d'apps refait sur la maquette Figma `23:6646`.**
`HvLaunchpad.svelte` n'est plus une modale opaque mais un voile plein écran
assombri avec puces de catégorie et grille de tuiles ; détail complet
(corrections de contraste, pastille d'état, survie des avertissements
`desktop.app_*_info` en description accessible, contrat E2E conservé) dans
la ligne Desktop du tableau par écran. La catégorie 5 de
`desktop.spec.ts` passe de 4 à 12 tests — 5.5 les puces filtrent la grille
sur place (`aria-pressed`, une seule active), 5.6 état vide + `Reset`,
5.7 voile `fixed` translucide sombre et colonne de contenu sans surface,
5.8 contraste du libellé **composité et mesuré** (tuile → voile → papier
blanc, pire cas) ≥ 4,5:1, 5.9 cibles 44px (puce 32px + `::after` 44px,
champ 44px dans une barre de 56px, tuile ≥ 44px), 5.10 piège de focus
(30 Tab) + Échap rend le focus à la tuile du dock, 5.11 téléphone — grille
3 colonnes, puces en `nowrap`/`overflow-x:auto` (`flex: none`, la croissance
du poste de travail étant annulée) et **effectivement débordantes**, le
panneau **scrolle en interne** (aucun scroll de la page, aucun scroll
horizontal) et la dernière tuile reste atteignable, 5.12 six colonnes à
1440px **et** barre de recherche / rangée de puces / grille à la même largeur
et au même `x` (demande explicite : les trois modules ne doivent jamais se
désaligner), **et** le contenu de la rangée de puces — pas seulement sa
boîte — va jusqu'au bord droit de la grille, ligne par ligne (demande
« pas assez de catégories → agrandir le module » : une puce ne fait que la
largeur de son libellé, `flex: 1 1 auto` la fait grandir ; l'assertion par
ligne tient aussi quand la rangée passe à la ligne),
puis **la même mesure répétée à 1440×520** — le conteneur scrollable est
le panneau et non le wrapper de la grille, sinon une barre de défilement
qui prend de la largeur retirerait celle-ci au seul grid.
**Deux vrais défauts ont été trouvés en écrivant
ces tests, pas par relecture** : (1) 5.5/5.6 ont révélé que le `<svelte:window onclick>` de
`Desktop.svelte` refermait le lanceur à chaque clic interne — `.lp-panel` a
repris son `onclick` stopPropagation (sans `onkeydown`, qui avalait Échap) ;
(2) la mesure du `backdrop-filter` a montré que la paire
`backdrop-filter` + `-webkit-backdrop-filter` est réduite par le minifieur à
la **seule** forme préfixée, que Firefox ignore → le flou de la maquette
valait `none`. **Troisième demande du même jour — « si le module catégories
est inférieur à la largeur des 6 colonnes car pas assez de catégories =
agrandir le module catégorie »** : la boîte de `.lp-cats` faisait déjà 828px
(5.12 le mesurait), mais une puce ne fait que la largeur de son libellé, donc
avec peu de catégories la rangée s'arrêtait aux deux tiers du chemin et le
module *paraissait* plus étroit que les tuiles. `flex: 1 1 auto` sur
`.lp-cat` (émis `flex: auto` par le minifieur) partage l'espace libre de
chaque ligne entre ses puces — croissance en *delta* égal, donc un libellé
plus long reste la puce plus large — et non `space-between`, qui aurait ouvert
un vide sur une dernière ligne courte. Le bloc `≤640px` reprend `flex: none`
(sinon une puce étirée remplirait la rangée et le défilement latéral du
téléphone n'aurait plus rien à faire défiler). 5.12 a gagné les assertions
« par ligne » et 5.11 le contrôle `flexGrow === '0'` + rangée réellement
débordante. Suite desktop :
**73 tests / 15 catégories** (`grep -cE '^\s*test\('` = 73 =
`npx playwright test --list` (146 = 73 × 2 projets)). Le 2026-09-17, **suite
desktop relancée en entier** contre le binaire reconstruit : **65 verts +
6 sautés + 2 échecs**, les deux échecs étant tous deux des défauts du **test**,
pas du produit, et tous deux corrigés puis revalidés (cf. ci-dessous) — les
6 sautés étant la §13 (« aucune tâche d'arrière-plan observable sur cet hôte »,
pré-existant). Aucun des deux échecs n'est une régression produit :
(1) **14.11** comparait l'attribut `class` brut et butait sur la classe de
portée Svelte (corrigé : filtrage des jetons `svelte-*`) ; relancé seul, il est
**vert** en 1,7 min, ses deux contrôles supplémentaires compris. (2)
**5.6** a échoué sur `browserContext.close: ENOENT … recording41.trace`, une
erreur de **démontage** et non d'assertion : deux processus Playwright lancés
en parallèle écrivent dans le **même** `test-results/.playwright-artifacts-0/`
(ils partagent la racine du dépôt), et l'un nettoie l'artefact pendant que
l'autre le referme. Preuve : relancé **seul**, 5.6 est **vert**. Règle qui en
découle — **ne jamais lancer deux suites Playwright en parallèle sur ce dépôt**,
la seconde corrompt le démontage de la première (l'erreur ressemble à un échec
de test alors que le test a réussi).

**2026-09-17 — en-têtes d'app : la barre ne porte que [ICÔNE] [TITRE]
(demande explicite « vérifie dans toutes les apps que dans le windowHeader
n'est présent que [APP ICON] [WINDOW TITLE] [—] [□] [×], rien d'autre »).**
Audit du registre, **recompté par script sur `HEAD`** : **17** des 24 apps
remplissaient encore la barre via les props de `WindowHeader`. Le comptage doit
couvrir **les deux formes** de passage d'un snippet — l'attribut
`actionLabel={…}` (CloudSync, Downloader, Explorer, Restic) **et** le bloc
`{#snippet …}` enfant (les autres) ; un `grep` sur la seule forme attribut en
oublie 13, et un `grep` qui ne borne pas au bloc `<WindowHeader>…</WindowHeader>`
compte en trop les `{#snippet actions()}` de `SectionCard` (Rescue ×3, Users).
Les 17 : Chat (pastille d'état Ollama), CloudSync, Dashboard (badge « Live »),
Downloader, Explorer (`extra` + `actionLabel`), Groups, Health, Journal (case
« rafraîchissement automatique » + 2 boutons), Network, Rescue (rangée d'état
des outils), Restic (`extra` + `actionLabel`), Settings (statut de sauvegarde),
Shares, Syncthing, Terminal (rangée d'onglets via `center`), Users (bouton
« nouvel utilisateur »), VPN. Toutes vidées, et chaque contrôle a un
emplacement décidé :
- `WindowToolbar` (`toolbarRight` / `toolbarLeft` de `PageLayout`) pour 10
  d'entre elles — Chat, CloudSync, Downloader, Groups, Journal, Network,
  Restic, Shares, Syncthing, VPN ;
- leur propre barre d'outils fichier, déjà au corps de page, pour Explorer ;
- le pied du rail pour `SettingsSaveButton`, qui porte lui-même l'état de
  sauvegarde (couleur + libellé + ligne sr-only `settings-save-status`,
  2026-09-23 — la pastille visible au-dessus et la légende en dessous ont été
  supprimées, il ne reste qu'un seul élément) ;
- le rail latéral pour le bouton « nouvel utilisateur » de Users
  (`data-testid="users-tile-new"`) ;
- le contenu de fenêtre pour la rangée d'onglets de Terminal (`terminal-tabs`,
  entre l'en-tête et `terminal-infobar`) ;
- le corps de page pour le badge d'état global de Health (toujours rendu) et
  pour la disponibilité des outils de Rescue, devenue un badge **par outil**
  dans la carte de chaque outil (`data-testid="rescue-tool-badge"`) ;
- **supprimé** pour le badge « Live » de Dashboard : indicateur décoratif
  (l'app se revalide toute seule), pas une donnée.

À ne pas confondre avec les **12 écrans qui rendent un `WindowToolbar`** : deux
apps qui **ne remplissaient pas** la barre (`Quotas`, `VirtualMachines`) en
rendent un elles aussi pour leurs actions de page, d'où 10 + 2 = 12.

**Nouveau test 14.11 « every app header bar carries the icon + title only »** :
il parcourt **les 24 app ids** de `OWN_HEADER_APPS` et exige, pour chacun,
exactement une tuile d'icône + un titre, **zéro** `.hv-winhdr-actions` et
**zéro** `.hv-winhdr-center`, et aucun élément interactif
(`button`/`input`/`select`/`textarea`/`a`/`label`/`[role=button]`) dans
`.hv-winhdr` — la rangée d'onglets vit dans `.hv-winhdr-wrap`, **hors** de la
barre, et le trio min/max/fermer est un **frère** de la barre (il appartient à
`WindowFrame`), donc les deux restent autorisés. Deux contrôles de plus ferment
l'angle mort des trois premiers, qui ne cherchent que les *slots* et les
contrôles : **(4)** tout élément **non** interactif ajouté dans la barre (un
badge, un point d'état, un wrapper superflu) est attrapé en épinglant la forme
du balisage — hors tuile d'icône, `.hv-winhdr` doit contenir **exactement**
`.hv-winhdr-text` puis `.hv-winhdr-title`, dans cet ordre ; **(5)** la tuile
d'icône, seul endroit où le balisage d'une app est libre, est épinglée à **un
seul** glyphe (`.hv-winhdr-icon > *` = 1), ce qui attrape le cas « badge en
plus ». **Leçon du premier passage de 14.11** : le contrôle (4) comparait
l'attribut `class` **brut**, or le compilateur Svelte ajoute sa classe de portée
(`svelte-19iy2no`) à tout élément visé par le `<style>` du composant — et
`WindowHeader.svelte` style précisément ces deux classes. Le test échouait donc
sur **toutes** les apps, pas seulement la première ; il filtre désormais les
jetons `svelte-*`. **Toutes les assertions de la boucle sont `expect.soft`** :
un garde de registre qui s'arrête à la première app fautive cache les 23 autres,
alors que le but est justement de nommer **toutes** les barres non conformes en
un seul passage (les assertions *web-first* en mode soft gardent leur
auto-retry). Trois mécanismes d'ouverture
sont nécessaires et le test documente pourquoi : `hv-open-app` ne résout que
dans le registre `apps`, qui **ne contient pas** `terminal` ni `settings`
(ajoutés séparément comme `terminalApp` / `settingsApp` dans
`Desktop.svelte`) — ceux-là s'ouvrent par le dock (`dock-<id>`) — et `account`
n'a **aucune** tuile de lanceur : il s'ouvre par le menu de la pastille
utilisateur. Le délai du test est porté à 300 s (24 apps × ~5 s). Deux vraies
trouvailles de cette passe : (1) les locators E2E de downloader (15),
vm (2), backup (1), settings (3) et health (2) ciblaient
`.hv-winhdr-action-btn` / `.hv-winhdr [data-testid=…]`, donc **vides et
silencieusement verts** après le déplacement — réécrits vers les nouveaux
`data-testid` (`downloader-add-toggle`, `vm-new`, `restic-add-repo`,
`settings-save-status`) ; (2) `handleOpenAppRequest` ne peut pas ouvrir
`terminal` / `settings` (cf. ci-dessus) — la faille de la poignée
`hv-open-app` que la doc décrit comme le passage d'app id inter-apps est
réelle, mais **non corrigée** dans cette passe (hors périmètre, aucun appelant
ne s'en sert aujourd'hui) ; à corriger si un jour une app doit ouvrir Settings
ou le Terminal depuis sa fenêtre.

**2026-09-17 (suite) — réparation du cluster de sélecteurs E2E morts.** En
relançant les suites touchées par la vidange des barres, un cluster de
sélecteurs **morts depuis la migration « Appliance » du 2026-09-12** est
apparu : cette migration a remplacé les crochets-classes par des
`data-testid`, et les specs ne l'ont pas suivi. Familles concernées :
`.ef-file-name`, `.bc-item`, `.modal-input`, `.ef-row-action`,
`.toolbar-btn`, `.toolbar-left`, `.modal-dialog`, `.modal-wide`,
`.preview-editor`, `.preview-text`, `.hv-shares-modal`, `.hv-shares-inp` et
`.hv-modal-dialog`. Deux causes distinctes, à connaître avant de conclure
qu'un sélecteur est vivant : (1) **le piège de la const Svelte** —
`class={efRowAction}` / `class={toolbarBtn}` posent dans le DOM la *chaîne
d'utilitaires Tailwind* que la const contient ; `.ef-row-action` /
`.toolbar-btn` sont les **noms** des consts, jamais des jetons de classe, et
un `grep` littéral trouve le nom en faisant croire le sélecteur vivant ;
(2) **les crochets composés au runtime** — `theme/Alert.svelte` rend
`hv-alert hv-alert-{variant}`, et `sidebarTestId="explorer-sidebar"` est une
**prop** que `PageLayout` transmet à `WindowSidebar` (un
`grep 'data-testid="explorer-sidebar"'` dans `Explorer.svelte` ne trouve donc
rien alors que l'attribut est rendu). Seule une vérification au niveau du DOM
tranche. `.hv-modal-dialog` est à part : la règle CSS existe
(`lib/styles/theme/components.css:323`) mais n'est attachée à **rien** —
l'orpheline est désormais documentée plutôt que supposée vivante.
**Un vrai trou trouvé par cette passe** : la garde E2E Explorer 14.3
(« aucun bouton Refresh manuel ») ciblait la classe morte, ne matchait donc
**rien** et était verte à vide — elle ne gardait plus la décision du
2026-09-08. Réécrite vers
`button[title*="efresh" i], button[title*="ctualiser" i]`, elle redevient une
garde. Les 5 appels « cliquer Refresh pour re-lister » sont supprimés : le
mécanisme de re-liste automatique a été **vérifié avant de retirer les
appels** (`connectWatcher()` ouvre une `EventSource` SSE sur
`GET /api/explorer/watch`, re-liste sur `change`, + `visibilitychange`), et
non supposé d'après le commentaire du test. La racine des modales est
désormais `[data-slot="dialog-content"]`
(`ui/dialog/dialog-content.svelte:29`) — `journal.spec.ts` et
`users.spec.ts` (4 sites) sont repointés ; trois étaient de vraies
assertions déjà cassées, la quatrième (`toHaveCount(0)`) passait à vide.
`Explorer.svelte` gagne 17 `data-testid`, `Shares.svelte`
`shares-name-input`.
**Bug de cette passe, corrigé** : 10 de ces attributs avaient été insérés
dans le corps de la fonction fléchée de `onclick` (ou **avant** la balise
`<button`), donc **hors** de la balise — la compilation frontend échouait
(`Assigning to rvalue`). Corrigé ; un scan conscient des balises, passé sur
les **242 `.svelte`** du frontend, ne signale plus aucun `data-testid` hors
balise (le seul hit restant, `SettingsCard.svelte:51`, est dans un
commentaire de documentation).
**14.11 durci** : deux vérifications ajoutées, qui ferment le trou signalé
par l'audit — un élément **non interactif** (badge, pastille d'état, wrapper
parasite) passait entre les mailles des contrôles 2 et 3. Désormais les
éléments de la barre **hors** tuile d'icône doivent être exactement
`['hv-winhdr-text', 'hv-winhdr-title']`, et la tuile d'icône doit contenir
**exactement un** glyphe. Prérequis vérifié au source pour les 24 apps : le
`<svg>` inline de Dashboard est **une** racine (trois `<path>` en enfants),
et Terminal passe `<TerminalIcon>` comme `{#snippet icon()}` **enfant de
`WindowHeader`** — il n'utilise pas `PageLayout`, contrairement aux 23 autres.
**Comptages (2026-09-24)** : `npx playwright test --list --project=firefox` = **1436 tests /
28 fichiers `.spec.ts`** — c'est la référence ; le
`grep -cE '^\s*test\('` documenté plus bas donne **1367**, l'écart (69)
étant les tests **générés par boucle**, que ce grep ne compte pas. Par suite
(`--list`) : desktop **74**, users **83**, explorer **64**, vm **45**,
journal **29**.
**Limite d'environnement (suite groups)** : le `beforeAll` de `groups.spec.ts`
crée un groupe jeton et échoue sur cette machine de dev non-root —
`could not create throwaway group: 400 … sudo: a terminal is required to read
the password`. Ce n'est pas une régression applicative mais l'absence de
`sudo` sans mot de passe pour l'utilisateur de dev. En cascade, Playwright
rapporte **1 échec (1.1) + 62 « did not run »** : tout le fichier (63 tests)
reste lettre morte, **ce qui inclut le garde 16.1 ajouté le 2026-09-23**
(écrit, compté, mais non exécuté ici). Pour exécuter la suite au complet :
drop-in `sudoers` NOPASSWD pour `groupadd`/`groupdel` — même condition
d'exécution que le `beforeAll` de `users.spec` (« absent ici », ligne
Utilisateurs).

**2026-09-17 (troisième passe) — sélecteurs corrompus par la substitution en
masse, et derniers gardes vacants.** Le remplacement global des classes mortes
a laissé **trois sélecteurs syntaxiquement cassés**, tous du même moule :
un sélecteur d'attribut suivi du **reste du nom de classe** qu'il remplaçait.
Aucun n'était visible à la relecture, seulement à l'exécution (ou jamais, pour
le troisième).
- `login.spec.ts` (nouveau mot de passe) : `[data-testid="dialog-content"]`
  — le crochet racine d'une modale est `data-slot`, pas `data-testid`. La
  portée reste nécessaire : `login-recovery-submit` existe **deux fois**
  (`Login.svelte:432` le panneau, `:614` la modale), et seul celui de la
  modale valide le nouveau mot de passe.
- `shares.spec.ts` 4.5 : `'[data-slot="dialog-content"]2 button'` — le `2`
  orphelin rendait le sélecteur **invalide** (CSS : jeton inattendu). Vrai
  crochet : `[data-testid="shares-mode-btn"]` (`Shares.svelte:762-763`).
  Rappel : `segBtn` est une **const** Tailwind (`:490`), donc jamais un jeton
  de classe — le piège déjà documenté.
- `docker.spec.ts` (éditeur Compose) : `[data-slot="dialog-content"]-close`,
  même invalidation. Le crochet vivant de la croix est
  `[data-slot="dialog-close"]` (`ui/dialog/dialog-content.svelte:38`).
- `users.spec.ts` 4.9 cliquait `.hv-modal-close` : **classe déclarée mais
  appliquée par personne** (`components.css:353`) — le clic expirait. Repointé
  sur `[data-slot="dialog-close"]`.
**Deux règles CSS orphelines** dans `lib/styles/theme/components.css` :
`.hv-modal-dialog` (`:323`) et `.hv-modal-close` (`:353`). Leurs voisines
`.hv-modal-title` / `.hv-modal-icon` **sont** vivantes (`theme/Modal.svelte`
`:63,65`) — la famille n'est orpheline qu'à moitié, d'où la vérification par
lecture du composant et non par `grep`. **Laissées en place** (supprimer du CSS
de thème partagé sans contrôle visuel n'est pas vérifiable) ; signalées.
**Trois gardes vacants de plus** convertis en assertions réelles, même famille
que les 10 gardes `.hv-winhdr-subtitle` : `dashboard.spec.ts` 1.7 et 7.1
(`.db-refresh-btn`) et `settings.spec.ts` 1.2 (`.ah-wrap, .ah-top` — `AppHeader`
a disparu du dépôt entier). Pour Dashboard, la règle est désormais assertée sur
le **nom accessible** (`getByRole('button', { name: /refresh|actualiser/i })`),
donc un bouton Refresh réintroduit échoue quelle que soit sa classe ; le
Dashboard ne rend aujourd'hui **aucun** bouton (0 `<button>`, 0 `role="button"`).
**Contrôle croisé des `data-testid`** : **362** utilisés par les specs contre
**454** déclarés dans `frontend/src`. Les 7 utilisés-mais-non-littéraux sont
tous expliqués : `explorer-sidebar` / `groups-list` / `settings-sidebar`
viennent de la prop `sidebarTestId`, `docker-nav-item` / `settings-nav-item` de
la prop `testId` d'un item de rail, `dashboard-live` est la pastille
**supprimée** (assertion négative volontaire) et `dialog-content` était le
crochet mal orthographié, corrigé ci-dessus. Aucun `data-testid` mort ne
subsiste.
**Un troisième crochet manquant, celui-là côté produit** — trouvé en relançant
la suite security après la correction d'`openSecurity()` : **2.1 restait
rouge**, et pour une raison indépendante. `security.spec.ts:302` assert
`.settings-info-row` dans la carte Antivirus ; or `AntivirusCard.svelte`
rendait bien cette classe **à HEAD** (markup + règle scopée), mais la migration
vers le vocabulaire partagé l'a perdue : ses lignes utilisent désormais
`settingsRow` de `lib/components/settings/vocabulary.ts`, qui était **la seule
const du module sans son crochet `settings-*`** (toutes les autres en portent
un — `settings-hint`, `settings-input`, `settings-select`, `settings-btn`…, le
module documentant lui-même que « les noms `settings-*` sont le contrat des
E2E, pas du style »). Ce n'est donc pas un sélecteur de test périmé mais un
**crochet retiré du produit** : le test avait raison, le vocabulaire avait tort.

**Première correction — fausse, et c'est l'erreur à retenir.** Ajouter
`settings-info-row` à `settingsRow` a rendu 2.1 vert mais **cassé
`settings.spec.ts` 10.1** (`toHaveCount(3)` → 4). Ma justification d'alors —
« les 3 lignes du `system` que compte 10.1 n'utilisent pas cette const,
vérifié au source » — était fausse : j'avais lu la liste des usages de
`settingsRow` tronquée par un `head -20`, et **`Settings.svelte:868`** (la ligne
« fréquence de vérification des MAJ », un `<select>`, donc une ligne *de
commande*) tombe bien dans le bloc `{#if activeCategory === 'system'}` ouvert en
865. Le nom même du crochet disait l'erreur : `settings-info-row` désigne une
ligne **lecture seule** (libellé + valeur), et `settingsRow` sert aussi aux
lignes de commande.

**Correction retenue** : `settingsRow` retrouve sa forme sans crochet, et une
const dédiée `settingsInfoRow` (qui préfixe `settings-info-row` à `settingsRow`)
porte le crochet. `AntivirusCard.svelte` l'utilise sur ses **deux seules** lignes lecture
seule (`definitions_status`, `last_scan`) — les autres rangées de la carte
(chemin d'analyse, scan planifié, dossier de quarantaine) sont des lignes de
commande et gardent `settingsRow`/`settingsRowStacked`. `settings-info-row`
n'a donc plus qu'un producteur, et les 3 littéraux de `system` restent seuls
dans leur catégorie. Vérifié : `security.spec.ts` 2.1 **et**
`settings.spec.ts` 10.1 **verts tous les deux** (2 passed, 27.6s, série).
À retenir : quand une carte migre d'un CSS scopé vers le vocabulaire partagé,
**le crochet E2E ne suit que s'il est dans la const** — mais une const partagée
n'est le bon endroit que si **toutes** ses lignes ont la sémantique du crochet.

**Un troisième bug de la même famille, trouvé par l'E2E et non par grep
(2026-09-17).** Après la correction ci-dessus, `settings.spec.ts` passait à
**95/96** : seul **23.5** restait rouge, à `:1807`, `Expected: <= 12`,
`Received: 14`. 23.5 est le seul test de la suite qui lise une taille de police
(`parseFloat(cs.fontSize)`) ; il vérifie que les libellés de section sont des
micro-libellés. Ils rendaient à 14px.

Cause : `--hv-text-*` est un préfixe **surchargé** — il porte des couleurs
(`--hv-text-primary` = `#1f2733`) *et* des tailles (`--hv-text-xs` = `11px`).
Tailwind v4 ne peut pas deviner la propriété visée par un `var()` non typé et
tranche pour la couleur : `text-[var(--hv-text-xs)]` compile en
`color: var(--hv-text-xs)`. La valeur n'étant pas une couleur valide, la
déclaration est écartée et **aucun `font-size` n'est émis** — le libellé hérite
donc du 14px de son parent. La convention du dépôt est la forme typée
`text-[length:var(--hv-text-xs)]`, qui compile bien en `font-size:`
(18 règles `font-size:var(--hv-text-xs` dans les 10 chunks CSS).

Portée : trois usages, tous des **tailles** écrites sans le hint — `secLabel` et
`secLabelFirst` de `components/settings/vocabulary.ts`, et l'en-tête du rail
dans `routes/Settings.svelte:657` (sans crochet de test, donc invisible pour la
suite). Corrigés en ajoutant `length:`.

**Ce que ça apprend sur la documentation.** Le guide
`docs/guides/design-system.md` documentait déjà la règle *et* affirmait que
« l'audit typographique du 2026-09-12 a corrigé toutes les occurrences
existantes ». C'était faux — trois avaient survécu, et c'est un test E2E, pas un
grep, qui les a trouvées. Le guide est corrigé (2026-09-17) avec l'explication
du préfixe surchargé et la commande qui tranche vraiment : lire le CSS
**compilé**, pas la source.

**Piège de méthode, à retenir.** Mon premier commentaire explicatif dans
`Settings.svelte` citait littéralement `text-[var(--hv-text-xs)]` — et Tailwind
extrait les candidats de classes **aussi dans les commentaires** d'un `.svelte`
(`@import "tailwindcss"` = détection automatique de contenu). Écrire
l'orthographe du bug dans un commentaire régénérait donc la règle morte qu'on
venait de supprimer, et faisait diverger la source du CSS réellement livré. Le
commentaire ne nomme plus que le jeton.

**Deux pièges de vérification rencontrés dans la même passe.** (a) Un `grep`
avec `\.` en tête ne peut pas matcher un sélecteur **minifié en liste**
(`.a,.text-\[…\]{…}`) : plusieurs recherches ont renvoyé « propre » à tort ; les
contrôles finaux se font en chaîne littérale (`grep -F`). (b) `grep -c` sur
`Settings.svelte` renvoie 2 pour la forme typée — la ligne 657 corrigée **plus**
un usage légitime préexistant (ligne 950), pas un doublon de la correction.

**2026-09-07 — Widgets sorti de cette liste** (demande explicite « continue
avec Widgets ») : `tests/widgets_integration.rs` (6 — **6/6**) et
`tests/e2e/widgets/widgets.spec.ts` (22 tests runtime, 9 catégories —
**22/22 verts**). La logique/unitaire est
**sans objet** — les seuls endpoints du feature (`GET|POST
/api/monitor-layout`, `GET /api/security/active-sessions-count`) sont des
upserts / requêtes DB sans logique pure. Intégration : garde de session sur
`/monitor-layout` (blob de layout par compte : `null` avant sauvegarde,
round-trip, upsert = 1 ligne, cloisonnement alice/bob) ; garde **admin** sur
`active-sessions-count` + sémantique du décompte (dédoublonnage d'un
utilisateur à plusieurs sessions, sessions expirées ignorées). E2E : la
fenêtre catalogue (11 tuiles), l'état actif/inactif de chaque tuile
cohérent avec le panneau moniteur correspondant sur le bureau, le toggle
qui fait apparaître/disparaître le panneau + met à jour le compteur du
footer, le bouton « Reset », la persistance via `POST /monitor-layout` ;
snapshot+restore du layout de l'admin dans `beforeAll`/`afterAll`.
**Jamais fait :** rien de mutant côté host (le feature n'en a pas).

**2026-09-07 — Journal système sorti de cette liste** (demande explicite
« continue avec Journal système ») : `#[cfg(test)]` dans
`src/handlers/journal.rs` (9 — `priority_text`, `is_app_log_filename`,
`text_log_path` (whitelist + anti-traversée), `strip_target_prefix`,
`parse_app_log_line`, `seconds_to_iso`), `tests/journal_integration.rs`
(11) et `tests/e2e/journal/journal.spec.ts` (28 tests runtime, 12 catégories —
**28/28 verts 2026-09-07**). **Ce qui est
exercé pour de vrai :** la source `app` (les fichiers `homevault.log*` de
`DATA_DIRECTORY/logs`, que le process possède — pas de `sudo`) — parsing
des lignes `tracing`, filtre `search`, état vide ; `DELETE /clear` qui ne
supprime que `homevault.log*` (jamais `/var/log` ni journald) ; les
réglages d'auto-nettoyage (défaut on/90 j, round-trip, clamp
`interval_days ≥ 1`). **Jamais fait :** un vrai `journalctl` / `tail
/var/log/*` (passe par `sudo`) — seul le repli propre `{success:false}`
est vérifié quand `sudo` demande un mot de passe ; le vrai vidage des
logs de l'app ; un déclenchement réel du scheduler d'auto-nettoyage.

**2026-09-07 — Dashboard sorti de cette liste** (demande explicite
« commence par DASHBOARD ») : `#[cfg(test)] mod tests` dans `dashboard.rs`
(17 — `format_bytes`, `parse_size`, `calculate_individual_disk_score`,
`calculate_disk_summary`, `calculate_disk_score`),
`tests/dashboard_integration.rs` (10 — **10/10 verts** — split d'auth sur
les 15 endpoints de lecture + widget-CRUD + widget-preferences, cycle
add/list/reorder/remove d'un widget, upsert des préférences par
utilisateur + leur cloisonnement par compte) et
`tests/e2e/dashboard/dashboard.spec.ts` (37 `test(` / **39 tests runtime**,
12 catégories — **39/39 verts 2026-09-07** contre le binaire de prod ;
lecture seule — l'écran n'a aucune action mutante ; +2026-09-08 : le bouton
Refresh manuel retiré (auto-refresh 5 s conservé), specs alignées). **1 bug
corrigé (fichier frozen, même que
`disk.rs`/`pools.rs`)** : `parse_size` castait le `f64` en `u64` *avant*
de multiplier par l'unité → « 9.8G » de `df -h` tronqué en « 9G »
(sous-comptage jusqu'à ~1 Tio sur une taille en T), et « 465,8G » (virgule
de locale) parsé à 0. Corrigé (`,`→`.`, multiplication en `f64`, cast en
dernier) ; tests `test_parse_size_keeps_the_fraction` /
`_accepts_a_locale_decimal_comma`.

**2026-09-07 — trous comblés dans les écrans déjà partiellement testés**
(demande explicite « termine les fonctions à finir de tester ») :
- **Stockage / disques virtuels** : les handlers `handle_virtual_disks_list`
  / `_delete` / `_scrub` (routes `/api/storage/virtual-disks*`) n'avaient
  **aucun** test Rust. Ajout de `handlers/storage.rs::vd_name` (3 unitaires)
  et de 2 tests d'intégration + l'ajout de ces endpoints aux boucles de
  matrice d'auth de `storage_integration.rs` (23 au total). Toujours pas de
  liste/destruction d'un vrai disque virtuel (mdadm/ZFS réels).
- **RAID** : `tests/raid_integration.rs` créé (5) — les 5 endpoints
  `/api/storage/raid*` sont exercés jusqu'au rejet d'auth uniquement
  (`require_admin` s'exécute **avant** le corps du handler → `mdadm` jamais
  atteint). La création/destruction réelle reste `raid.spec.ts` opt-in.
- **Explorateur / fonctionnalités umbrelOS** : `/roots`, `/save`, `/eject`,
  `/snapshots`, favoris (server-side) et le paramètre `?strategy=` sur
  `/copy` étaient sans couverture. +11 tests dans `explorer_integration.rs`
  (40 au total) + `kind_for_root`/`resolve_dest` unitaires (5) +
  `CollisionStrategy::parse`/`non_colliding_path` dans `browser.rs` (6).
  **2026-09-08 — `explorer.spec.ts` section 15 ajoutée** (9 tests) : éditeur
  inline (UI), `/save`, favoris (étoile ⇄ sidebar ⇄ serveur), stratégies de
  collision `/copy`+`/move`, menu Snapshots, `/eject`, hand-off
  Explorer→Shares. **Reste non couvert** : la navigation *dans* un vrai
  instantané ZFS, un vrai `umount` USB (host requis).
- **Quotas de transfert** : `transfer_quota.rs::check_quota` (8 unitaires).
  Le reste (`applicable_*_quotas`, `record_usage`) touche la DB + `id -nG`
  et reste couvert de bout en bout par `explorer_integration.rs` (429 au
  download / blocage à l'upload) + `users.spec.ts`/`groups.spec.ts`.
- **1 bug corrigé (autorisation explicite, fichier frozen)** : `check_quota`
  faisait `additional_bytes as i64` — un `u64` au-delà de `i64::MAX`
  devenait négatif et passait sous la limite à tort. Corrigé : comparaison
  en `u128`. Test de non-régression
  `test_check_quota_huge_transfer_saturates_instead_of_overflowing`. Voir
  `docs/guides/transfer-quotas.md` et l'invariant « Explorer / transfers »
  de `CLAUDE.md`.
**2026-09-08 — Paramètres (cœur) sorti de « sans le moindre test »**
(demande explicite « termine le point 1 et 2 ») :
- `#[cfg(test)] mod tests` dans `src/handlers/settings.rs` (13 — logique pure :
  `parse_version` / `is_newer_version` (une version illisible ne réclame
  jamais de MAJ), `sanitize_hostname` (collapse/trim des tirets, repli
  `homevault`, limite RFC 1123 à 63), `UpdateFrequency` défaut = `Manual` +
  `as_seconds` (G2 — aucun appel GitHub automatique), défauts d'apparence,
  `normalize_accent` (2026-09-23 — réécrit l'accent legacy `#4a90d9` en
  `#1E62AE` §1.4.3, case-insensible, toute autre valeur est conservée)).
- `#[cfg(test)] mod tests` dans `src/settings_persistence.rs` (7 — `export()`
  masque `smtp_password` (jamais en clair, `null` dans le JSON) sans muter le
  fichier sur disque, round-trip `import()` de l'apparence, `apply_loaded_
  settings` : un mot de passe `None` (export masqué) ré-importé **ne remplace
  pas** le mot de passe configuré).
- `tests/settings_core_integration.rs` (13 — router Axum + SQLite mémoire +
  `init_persistence(tempdir)` : split **admin** sur toutes les lectures
  (`/settings`, `/appearance`, `/email`, `/notifications`, `/update`,
  `/export`) et écritures ; `GET /settings/language` = n'importe quelle
  session ; `GET /settings/public` = sans auth et sans fuite ; round-trip
  PUT→GET de l'apparence (accent / densité / wallpaper) + reflet dans
  l'agrégat ; mise à jour partielle qui préserve les autres champs ;
  `GET /settings/export` masque `smtp_password` ; export→import ;
  `POST /settings/reset` restaure les défauts (accent `#1E62AE`,
  `check_frequency` = `manual`) ; `set-language` accepte en/fr/de/es et
  rejette l'inconnu ; seuil de notification clampé à 100) ; +
  2026-09-23 : `PUT /settings/appearance` avec l'accent legacy `#4a90d9`
  → relu `#1E62AE` (et reflet dans l'agrégat `GET /settings`).
- **Toujours pas couvert** : l'application *runtime* de l'accent
  (`--hv-primary`) et de la densité — purement front, du ressort de
  `settings.spec.ts` ; `POST /settings/general` (→ `hostnamectl`) n'est pas
  exercé en intégration (le shell-out) — `sanitize_hostname` l'est en
  unitaire.

**Syncthing** est sortie de cette liste le 2026-09-06 : tests unitaires
`handlers/syncthing.rs` (11 — `extract_api_key` sur le `config.xml`, +
désérialisation des réponses REST de syncthing v1.27/1.29 avec tous les
`#[serde(alias)]` : `deviceID`, `certName`, `myID`, `globalBytes`, `at`,
`clientVersion`… — plusieurs de ces alias existent parce qu'un alias
manquant cassait silencieusement le parsing contre un vrai démon),
`tests/syncthing_integration.rs` (5), `tests/e2e/syncthing/syncthing.spec.ts`
(21). **Ce qui est verrouillé :** les **10** endpoints exigent une
session **admin** — c'était LA faille de l'audit du 2026-07-16 : zéro
check d'auth sur `restart` (redémarrage du démon), pause/resume
(perturbe n'importe quel appareil appairé), et lecture des IDs/IPs de
tous les appareils. `pause`/`resume`/`scan` rejettent un body malformé
(422) avant tout appel HTTP ; quand le démon est éteint (cas de la
machine actuelle), ces endpoints renvoient une erreur propre « Failed to
connect to Syncthing » (jamais un 500). **Jamais fait :** un vrai
`restart`/`pause`/`resume`/`scan` contre un démon en marche, ni un cycle
appareil/dossier réel (il faut un Syncthing lancé + un pair appairé —
comme l'audit initial, session dédiée) → lecture live laissée en
**opt-in** `SYNCTHING_E2E=1`. Aucun bug applicatif trouvé. `Syncthing.svelte`
est en mode *legacy* Svelte (pas de runes) → pas touché par le bug de
réactivité de la page Sauvegarde.

**Assistant IA (Chat)** est sortie de cette liste le 2026-09-06 : tests
unitaires `handlers/chat.rs` (11 — `system_prompt` par langue fr/de/es/en
+ repli anglais, `session_error_message`, `reset_ai_model`,
`MODEL_CATALOG` + le scoreur de recommandation qui tourne contre le vrai
`/proc/meminfo` de l'hôte, désérialisation des réponses Ollama),
`tests/chat_integration.rs` (9), `tests/e2e/chat/chat.spec.ts` (22).
**Ce qui est verrouillé :** `send` / `send/stream` / `models` / `model`
GET exigent une **session** (n'importe quel rôle — chatter n'est pas
privilégié, c'était le sens du correctif d'audit) ; `models/pull` et
`model` PUT exigent **admin**. Le repli propre « IA injoignable » (jamais
un 500) quand Ollama est éteint est testé comme cas de première classe.
Conversation réelle en **opt-in** `CHAT_E2E=1` (Ollama lancé + un modèle
téléchargé). **Vérifié de bout en bout le 2026-09-06 avec un vrai Ollama
0.33.3 + `gemma3:1b` (inférence CPU) :** suite complète `CHAT_E2E=1`
**22/22** — dont §13.1 (prompt réel → réponse streamée non vide) et §5.1
(la langue est bien transmise, réponse réelle reçue). Dans le navigateur :
badge **« En ligne »** (vert), conseil hors-ligne masqué, un prompt réel
obtient une réponse streamée. `/api/chat/send` → `{"reply":"Pong.",
"model":"gemma3:1b"}`, `/api/chat/send/stream` → SSE `chunk`×N + `done`.
**3 correctifs le 2026-09-06 pour rendre le Chat fonctionnel (demande
explicite) :**
1. `GET /api/chat/models/recommend` — le **seul** endpoint de Chat sans
   check de session (fuite RAM+GPU de l'hôte à tout appelant). Corrigé :
   prend `State`+`headers`, même `get_session` que ses frères. Le scoreur
   est extrait en `recommend_models(&specs)` (pur, testable).
2. `Chat.svelte` — le badge de statut affichait un **« En ligne » vert
   codé en dur**, jamais vérifié : il restait vert alors que chaque
   message échouait. Corrigé : `onMount` appelle `checkOllama()`, le badge
   montre **en ligne** / **aucun modèle** / **IA hors ligne** / **vérif**,
   et l'état vide affiche un conseil actionnable quand Ollama est absent.
3. `installer/install.sh` — Ollama n'était **jamais installé**, l'app
   Chat était morte à la livraison. Ajout de `install_ollama` (script
   officiel, pas apt, puis `systemctl enable --now ollama`). **Aucun
   modèle n'est téléchargé automatiquement** — le choix du modèle revient
   à l'utilisateur (Paramètres > IA, recommandations selon la RAM/le GPU).
   Entrée `ollama` dans `dependencies.rs` (catégorie `AI`) + test
   `test_install_sh_installs_ollama_for_the_chat_app` (qui vérifie aussi
   l'absence de `ollama pull` dans `install.sh`).
   **+2026-09-23, vérification renforcée sur la doc Ollama Linux**
   (`docs.ollama.com/linux`) : installer *et* Étape 4 de vérification
   contrôlent désormais les trois niveaux — binaire présent, unité
   `ollama` active (`systemctl is-active --quiet ollama`, avec boucle
   d'attente de 10 s après `enable --now` pour laisser au daemon le temps
   de binder le port) et réponse HTTP sur `http://127.0.0.1:11434/`. Le
   test Rust exige les trois motifs ; un binaire installé avec un service
   mort échoue désormais la vérification au lieu d'afficher un log d'install
   vert avec Chat hors ligne.

**Terminal** a complété sa couverture le 2026-09-06 (il avait déjà 71
tests E2E) : ajout des tests **Rust** — `handlers/terminal_pty.rs` (13 —
`is_valid_tab_id` / `resolve_tab_id` / `generate_ticket`, et le cycle du
ticket de connexion unique du `PtyManager` : émission → consommation
unique, `kill_session` qui purge aussi les tickets en attente du compte
supprimé, no-ops de `close_tab`/`cleanup_all`/`reap_stale`) et
`tests/terminal_integration.rs` (8 — matrice d'auth **admin** sur
`/api/terminal/ticket` et `/close`, validation du tab id → 400, `close_tab`
d'un onglet inconnu = succès propre). **Pas testé en Rust :** le spawn
d'un vrai shell `sudo -u <user> -i`, donc le plafond `MAX_TABS_PER_USER`
et le kill du reaper d'inactivité — c'est le rôle de `terminal.spec.ts`
(vrai PTY). Aucun bug trouvé.

**Cloud Sync (rclone)** est sortie de cette liste le 2026-09-06 : tests
unitaires `handlers/rclone.rs` (11), `tests/rclone_integration.rs` (13),
`tests/e2e/cloudsync/cloudsync.spec.ts` (24). **Ce qui est verrouillé :**
les 12 endpoints exigent une session ; toutes les mutations **et la
lecture du fichier de config brut** (`GET /rclone/config`, qui renvoie
les credentials « obscurcis » — donc en clair — de chaque remote)
exigent admin ; les gardes anti-injection (`config` contenant une ligne
`[section]`), nom de remote invalide et source/destination vide de
`sync`/`copy` rejettent proprement sans lancer `rclone` ni écrire le
fichier. **Jamais fait :** un vrai `sync`/`copy`/`mount` `rclone`, ni un
vrai `add remote` → il écrit `/etc/rclone.conf` (root sur un vrai
déploiement, non accessible en écriture par le serveur de dev) — cycle
réel de remote laissé en **opt-in** `CLOUDSYNC_E2E_WRITABLE_CONF=1`.
Aucun bug applicatif trouvé (les correctifs de l'audit de 2026-07-14
tiennent).

**Sauvegarde (Restic)** est sortie de cette liste le 2026-09-06 : tests
unitaires `restic_scheduler.rs` (12), `restic.rs` (8),
`tests/restic_integration.rs` (14), `tests/e2e/backup/backup.spec.ts`
(24). **Conditions réelles :** un vrai `restic init` + `restic backup` de
`/etc/hostname` dans un dépôt jetable `/tmp/hv-backup-e2e-*`, la liste des
snapshots (le snapshot existe réellement), puis suppression du dépôt. Les
cycles **schedule** et **retention-policy** sont 100 % DB + `compute_next_
run` — testés de bout en bout en intégration sans jamais lancer `restic`.
**Jamais fait :** `restore` réel, `forget`/`prune` réels, backup planifié
déclenché par le scheduler (boucle 30 s).
**3 bugs trouvés et corrigés (fichiers frozen, demande explicite
« corrige tous les bugs ») :**
0. **La page Sauvegarde était entièrement morte dans le navigateur** —
   spinner de chargement permanent. `Restic.svelte` est un composant
   Svelte 5 *runes mode* mais ~35 variables réactives (`resticLoading`,
   `resticAvailable`, `repos`, tous les flags de modale…) étaient de
   simples `let` : en runes mode une réassignation de `let` ne
   re-rend pas, donc `resticLoading = false` dans `onMount` n'avait
   aucun effet. Même classe de bug que Shares/Downloader/Rescue ;
   audité en 2026-07-14 sous Svelte 4, raté par la migration Svelte 5,
   jamais détecté faute de tests. Corrigé : chaque `let` réactif →
   `$state(...)`. `backup.spec.ts` échoue en masse (24/26 timeouts sur
   le spinner) sans ce correctif.
1. `create_backup` faisait `serde_json::from_str` sur **toute** la sortie
   de `restic backup --json`, qui est du **NDJSON** (des lignes `status`
   puis une ligne `summary`) → « trailing characters at line 2 » → **tout
   backup manuel et planifié était rapporté en échec** alors que le
   snapshot était bien créé. Corrigé : `parse_backup_summary` prend la
   dernière ligne JSON non-`status`. (Champ `total_bytes` → `total_bytes_
   processed` au passage — le nom restic réel.)
2. `compute_next_run` passait le champ jour-de-semaine du cron tel quel à
   la crate `cron`, dont la convention diffère de crontab : `0` était
   **rejeté** (crontab = dimanche) et `1..6` étaient **décalés d'un jour**
   (un planning « tous les lundis » `* * 1` tournait le dimanche). Corrigé
   par `normalize_crontab_dow` (crontab `0/7`→dim, `1..6`→lun..sam) ;
   les noms `SUN`/`MON`/… passaient déjà correctement.

**Stockage** est sorti de cette liste le 2026-09-06 : tests unitaires
`pools.rs` (24), `cache.rs` (13, +2 existants), `encryption.rs` (2),
`tests/storage_integration.rs` (24, +3 aux passes 2026-09-07) et `tests/e2e/storage/storage.spec.ts`
(30). **2026-09-07 (re-scope RAID↔Storage) :** `storage.spec.ts` étendu —
le test 1.3 attend maintenant **6** onglets (« Disques virtuels » ajouté),
la boucle de nav d'onglets couvre le nouveau panneau. Run 2026-09-07
contre le binaire reconstruit : **34 passés / 1 sauté** (cycle LUKS
opt-in). Toujours **aucun** test qui liste ou détruit un vrai disque
virtuel (mdadm/ZFS réels requis) — seul le §13 opt-in de `raid.spec.ts`
couvre ce chemin de bout en bout. **Ce qui est réellement exercé :** tout l'inventaire lecture seule
(disques/partitions/amovibles, réglages + pools + parts de cache, volumes
chiffrés) contre le vrai hôte ; le split d'auth (lecture = n'importe
quelle session, écriture = admin) sur les 18 endpoints ; et **chaque**
voie de mutation jusqu'au rejet propre (400/401/403, jamais 500) —
cron/plages hors bornes des réglages de cache, nom/mode de part de cache
invalide, phrase de passe trop courte / système de fichiers / label LUKS
invalide, id de volume inconnu, suppression sans confirmation. **Jamais
fait :** créer/détruire/scruber un vrai pool ZFS/Btrfs, ni formater/
déverrouiller/effacer un vrai volume LUKS — toutes ces voies passent par
`sudo zpool`/`btrfs`/`cryptsetup`/`mkfs`/`mount`. Un vrai cycle LUKS
(créer → vérifier listé → supprimer) est prévu dans `storage.spec.ts`
§13, **désactivé par défaut** : il faut `STORAGE_E2E_LOOP=1` +
`STORAGE_E2E_LOOP_DEV=/dev/loopN` + un `sudo` sans mot de passe (impossible
sur la machine de dev actuelle — `sudo` y demande un mot de passe). Les
tests unitaires ne touchent jamais un disque : les branches d'entrée
invalide de `create_pool`/`delete_pool`/`start_scrub` sont vérifiées
comme renvoyant `Err` **avant** d'atteindre un `Command`.
**2 bugs latents trouvés puis CORRIGÉS le 2026-09-06 (demande explicite
de l'utilisateur de corriger tous les bugs trouvés) :**
1. `PoolService::parse_size` castait le `f64` analysé en `u64` *avant* de
   multiplier par l'unité, donc toute taille fractionnaire de
   `zpool list -H` (« 1.81T », « 9.06G ») était tronquée à l'entier de
   l'unité — un pool de 1,81 Tio rapporté comme 1 Tio — et la virgule
   décimale d'une locale FR/DE (« 1,81T » → « 181 » → 181 Tio) n'était
   pas gérée. `disk.rs::parse_size` avait **exactement** ces deux bugs et
   avait déjà été corrigé ; `pools.rs` reproduit maintenant le même
   correctif. Tests de non-régression : `test_parse_size_keeps_the_
   fraction`, `test_parse_size_accepts_a_locale_decimal_comma`.
2. `cache.rs::parse_rsync_stats` découpait le décompte d'octets de
   `rsync --stats` sur les espaces avant de retirer le séparateur `,` —
   sous une locale groupant par espace (« 1 234 567 bytes ») il
   n'enregistrait que `1`. Corrigé : ne garde que les chiffres ASCII
   avant le mot d'unité. Test :
   `test_parse_rsync_stats_survives_a_locale_thousands_separator`.

**Downloader** est sorti de cette liste le 2026-09-06 : tests unitaires
`handlers/downloader.rs` (15 — round-trips serde contre des fixtures
capturées telles quelles sur un `aria2c` 1.37.0 vivant : le format RPC
réel en camelCase doit parser, le JSON renvoyé au frontend doit rester en
snake_case — exactement le bug que les `#[serde(rename(deserialize))]`
ont corrigé), `tests/downloader_integration.rs` (8) et
`tests/e2e/downloader/downloader.spec.ts` (41). **Condition réelle
réellement exercée :** un vrai `aria2c` télécharge un fichier que
l'application se sert elle-même (`/wallpapers/hv-01-thumb.webp`, ~25 Ko,
sans Internet) dans un dossier jetable sous `/tmp`, le test attend l'état
`complete` avec le bon décompte d'octets et le fichier réellement présent
via `task.files[].path`, puis supprime la tâche (bouton corbeille de l'UI
_et_ endpoint) et vérifie qu'elle disparaît de la liste. **Jamais fait :**
un torrent/magnet réel, ni une source distante nécessitant Internet.
L'intégration ne déclenche jamais le spawn du démon `aria2c` (tout
endpoint hors `status` le lancerait et parlerait au démon du serveur de
prod sur le port 6800) — elle verrouille uniquement la garde admin sur
les 10 endpoints, la validation de body (422 avant toute logique) et le
seul chemin sans effet de bord (`GET /status`). `downloader.rs` est une
page _frozen_ : seul un bloc `#[cfg(test)]` a été ajouté en fin de
fichier, aucun code de prod modifié.

**Explorateur (File Explorer)** a gagné ses tests d'intégration + E2E le
2026-09-06 (il n'avait que des unitaires `browser.rs`/`config`) :
`tests/explorer_integration.rs` (29), `tests/e2e/explorer/explorer.spec.ts`
(50, **49/49 verts** contre le binaire de prod), + un bloc `#[cfg(test)]`
dans `src/handlers/explorer.rs` (7). **Ce qui est réellement exercé :**
tous les handlers Explorer étant de simples appels filesystem (pas de
`sudo`, pas de service, rien sous `/etc`), l'intégration pilote les vrais
chemins de succès de bout en bout dans un `tempfile::tempdir()` —
mkdir/list/rename/delete, copy+move d'un fichier, aperçu texte, recherche
récursive, chmod get+set, compress→extract, upload multipart (+ rejet de
nom traversant), download octet-exact (+ dossier→400, absent→404), la
garde de quota de transfert au download (429) et à l'upload, et le cycle
corbeille trash→list→restore→purge / empty. L'E2E confine toute mutation
à un seul dossier `/tmp/hv-fx-e2e-*` (jamais `DELETE /api/explorer/trash`
qui viderait la corbeille d'un vrai déploiement) ; un compte non-admin
jetable prouve le confinement `PathGuard`.
**5 bugs de robustesse trouvés puis CORRIGÉS le 2026-09-06 (fichiers
_frozen_, autorisation explicite « corrige ») :**
1. `FileBrowser::restore_trash_item` n'avait aucun repli cross-device
   (contrairement à `trash_item`/`move_item`/`copy_item`) → restaurer un
   fichier corbeillé depuis un montage RAID/pool échouait avec une erreur
   IO opaque. Ajout du même cascade rename→copy+delete sur EXDEV.
2. `find_trash_entry` propageait l'erreur `read_dir` d'un dossier de
   corbeille inexistant au lieu de la traiter comme `NotFound` → dès que
   `/home/.homevault-trash` n'existait pas (cas normal), `restore` **et**
   `purge` par-id renvoyaient « No such file or directory » et la corbeille
   par-id était **cassée sur un vrai déploiement** (vérifié en live).
   Corrigé : `ErrorKind::NotFound` → on passe au dossier suivant.
3. `list_directory` échouait entièrement (`entry?` / `metadata()?`) si une
   seule entrée posait problème (lien cassé, permission, course avec un
   delete concurrent) — un fichier douteux masquait tout le dossier.
   Corrigé : entrée fautive ignorée, la liste continue.
4. Aucun plafond sur le nombre d'entrées (une remontée de 100k fichiers =
   100k lignes DOM). Ajout de `MAX_DIR_ENTRIES = 10_000` (après tri, donc
   tranche stable) + champ `truncated` sur `DirectoryListing` + mention
   dans le pied de page (clé i18n `explorer.listing_truncated` ×4) +
   filtrage des fichiers-déchets d'autres OS (`.DS_Store`, `Thumbs.db`,
   `._*`, …).
5. Double-clic sur une ligne de dossier peu fiable : le 1er clic
   sélectionne → insère la barre d'actions → décale la liste → le 2e clic
   (et l'évènement `dblclick`) rate la ligne. Corrigé dans `Explorer.svelte`
   par une détection de double-clic en JS basée sur l'**identité** de
   l'entrée (chemin) et non la position écran (`handleRowClick`),
   insensible au reflow ; `ondblclick` retiré des lignes/tuiles.
**Pas de tests ajoutés pour ces 5 correctifs ni de recompilation** (demande
explicite de l'utilisateur) — à faire : `cargo test`, `npm run build`,
puis re-run de `explorer.spec.ts` (le test 13.2 pourra alors ré-inclure
restore + purge par-id, aujourd'hui contournés à cause du bug nº2).
**+2026-09-23 : `cargo test --locked` exécuté pour la première fois sur cette
machine** (toolchain Rust installée via rustup — stable 1.98.1, la machine
n'en avait pas) : **vert sur toutes les suites, 0 échec** — le volet `cargo
test` de ce « à faire » est donc couvert ; restent `npm run build` et le
re-run de `explorer.spec.ts`.

**2026-09-06 (8 fonctionnalités inspirées d'umbrelOS ajoutées en 2 lots,
demande explicite, sans compil ni tests) :**
Lot 1 — racines nommées dans la sidebar (`GET /api/explorer/roots`) + fil
d'Ariane avec labels ; éditeur de texte/code inline (`POST
/api/explorer/save`) ; watcher d'auto-refresh (poll 8 s) ; gestion des
collisions copy/move/upload (`?strategy=` : `overwrite`/`keep_both`/`skip`
+ prompt 3 choix).
Lot 2 — favoris épinglés dans la sidebar (localStorage) ; éjection sécurisée
des disques USB (`POST /api/explorer/eject`, admin) ; action « Partager ce
dossier » qui ouvre l'app Shares pré-remplie (nouvelle store
`explorerShareIntent` + 1 hook dans `Desktop.svelte` et `Shares.svelte`) ;
navigation lecture seule dans un instantané ZFS passé (`GET
/api/explorer/snapshots` → `.zfs/snapshot/…`, `snapshotMode`).
**Couverture (rattrapée) :** `explorer_integration.rs` +11 le 2026-09-07
(`/roots`, `/save`, `/eject`, `/snapshots`, favoris, `?strategy=`) ;
`explorer.spec.ts` **section 15 le 2026-09-08** (9 tests, 2 runs
consécutifs verts) — éditeur inline (UI), `/save` (écrit / écrase /
rejette > 5 Mio), favoris (étoile de la barre d'outils ⇄ sidebar ⇄
serveur), stratégies de collision `skip`/`keep_both`/`overwrite` sur
`/copy` et `/move`, menu Snapshots (« aucun instantané » hors ZFS),
`/eject` (admin only + refus d'un non-point-de-montage), hand-off
« Partager ce dossier » → l'app Shares s'ouvre pré-remplie. **Reste non
couvert** : la navigation *dans* un vrai instantané ZFS (`snapshotMode`
avec de vraies données), un vrai `umount` USB — host requis.

**RAID** est sorti de cette liste le 2026-09-05 : audit live avec
loop devices jetables, 9 bugs trouvés et corrigés (dont un blocage total
de la création — voir `CLAUDE.md`), tests unitaires `disk.rs` (12) et
`raid.spec.ts` ajoutés. **2026-09-07 :** après le re-scope RAID↔Storage
(RAID = assistant de création uniquement), `raid.spec.ts` a été
entièrement réécrit (42 `test(`, 43 tests runtime, 14 catégories) pour le
nouvel assistant ; il ne pilote jamais une vraie création (chaque test
s'arrête avant « Confirmer la création », la validation serveur est
exercée par `fetch` direct qui 400 avant tout `mdadm`/`zpool`) sauf le
cycle opt-in `RAID_E2E_DEVICES`. Run 2026-09-07 contre le binaire
reconstruit : **35 passés / 8 sautés / 0 échec** (les 8 sauts exigent
≥2 disques libres sur l'hôte pour atteindre l'étape 2/3). Un test corrigé
après le premier run (`10.1` — un POST sans session renvoie 400, pas
401/403 : c'est la branche « pas de token » de `require_admin`).

**Rescue** est sorti de cette liste le 2026-09-05 : `tests/rescue_integration.rs`
(28) + `tests/e2e/rescue/rescue.spec.ts` (53, jamais de mutation contre un
device réel — voir l'en-tête du fichier) ajoutés. Trois bugs réels trouvés
et corrigés (autorisation explicite) : (1) `get_ddrescue_log` appelait
`ddrescue --status`, une option qui n'existe pas — le statut d'un clonage
en cours était donc toujours faux (running=true/0%) ; remplacé par un vrai
parsing du mapfile ddrescue. (2) 6 flux (ddrescue clone, TestDisk
analyze/rebuild, PhotoRec, mdadm diagnose/repair) n'affichaient aucune
erreur en cas d'échec — branches `else` vides ; corrigé via
`notificationsStore` (mêmes clés i18n déjà présentes mais jamais câblées).
(3) le header d'app partagé (alors `AppHeader.svelte`, depuis fondu dans
`WindowHeader.svelte`, ~13 apps) : la navigation clavier par flèches dans
les onglets comparait `e.key` à `'IconArrowRight'` etc. au lieu de
`'ArrowRight'` — totalement morte partout, corrigé.

**Réseau** et **VPN** sont sortis de cette liste le 2026-09-06 :
`tests/network_integration.rs` (23) + `tests/e2e/network/network.spec.ts`
(43), `tests/vpn_integration.rs` (22) + `tests/e2e/vpn/vpn.spec.ts` (39),
plus les tests unitaires `network.rs` (32, +3 `build_resolv_conf` le
2026-09-07) / `vpn.rs` (14). Les suites automatisées ne pilotent pas de
vraie mutation (changer une IP, couper une interface, connecter un WiFi,
monter un tunnel) — uniquement les gardes d'accès et les rejets de
validation. **2026-09-07** : le chemin d'application de `src/network.rs` a
été corrigé (mutations `ip`/`dhclient` wrappées dans `sudo`,
`/etc/resolv.conf` via `sudo tee` sans clobber) et vérifié en direct sur
une interface `dummy` jetable — voir l'invariant « Network » de `CLAUDE.md`
et `docs/guides/network.md`.

**Parts/Protocoles (Shares)** est sorti de cette liste le 2026-09-06 :
`tests/shares_integration.rs` (18) + `tests/e2e/shares/shares.spec.ts`
(51), plus les tests unitaires du parser de permissions (`permissions.rs`
10), de `ShareType` + configs par défaut (`shares.rs` 12) et du
validateur de clé SSH publique (`sftp.rs` 6). La suite ne crée / supprime
/ bascule jamais un vrai partage, ne démarre / arrête jamais un vrai
service de partage, n'accorde / révoque jamais un vrai ACL et ne
provisionne jamais un vrai compte Linux SFTP ni un bind mount — toutes
ces voies passent par `sudo` (`tee`/`systemctl`/`exportfs`/`useradd`/
`mount --bind`) ou écrivent un fichier root sous `/etc`. Les mutations ne
sont exercées que jusqu'à confirmer un rejet propre (400/401/403, jamais
un panic/500) sur session absente/non-admin ou entrée invalide. Suite
E2E lancée en vrai contre le binaire de prod le 2026-09-06 : **53/53
verts** (Firefox), aucune mutation réelle déclenchée.
**Un bug trouvé et corrigé (autorisation explicite, page frozen) :** la
navigation clavier par flèches dans la barre d'onglets de `Shares.svelte`
(`onTabKeydown`) comparait `e.key` à `'IconArrowRight'`/`'IconArrowLeft'`/…
(noms de composants d'icônes) au lieu de `'ArrowRight'`/`'ArrowLeft'` —
exactement le bug corrigé le 2026-09-05 dans le header d'app partagé
(alors `AppHeader.svelte`, depuis fondu dans `WindowHeader.svelte`), mais
ici dans la copie locale de la page. La navigation par
flèches était totalement morte (seules Home/End marchaient) ; corrigé et
vérifié par `shares.spec.ts` test 11.1 (flèches + wrap + Home/End).

**Suite sécurité (Antivirus / Anti-Ransomware / Pare-feu)** est sortie de
cette liste le 2026-09-06 : tests unitaires (`firewall.rs` 9 — parsing de
`ufw show added` + détection des règles obligatoires ; `antivirus.rs` 6 —
parsing de `clamscan -i` ; `ransomware_watcher.rs` 7 — classification
d'évènements + correspondance de chemin protégé ; `ransomware.rs` 3),
`tests/security_integration.rs` (18) et
`tests/e2e/security/security.spec.ts` (30). **Conditions réelles
réellement exercées :** un vrai `clamscan` d'un fichier EICAR dans un
dossier jetable (détection → quarantaine → ligne de menace → résolution),
et de vrais fichiers-appâts (honeypots) écrits puis supprimés sur disque
et vérifiés. **Jamais fait :** activer/désactiver le vrai pare-feu de
l'hôte ou supprimer une vraie règle `ufw` — quand `sudo ufw` demande un
mot de passe (cas courant) `ufw_available()` vaut `false` et la carte
pare-feu n'est vérifiée que pour l'état « non installé » + les gardes
d'accès ; le verrou `{confirmed:true}` et le 403 sur règle obligatoire ne
sont testés en direct (toujours sans jamais activer) que si un vrai `ufw`
est joignable.
**Une course trouvée et corrigée (autorisation explicite, page frozen) :**
`parse_clamscan_line` lisait `quarantine_path.exists()` à l'instant exact
où `clamscan` imprime la ligne `FOUND`, ce qui courait avec le `--move`
qui suit — sur stockage lent la ligne de menace était enregistrée
`action:"left_in_place"` / `quarantine_path:null` alors que le fichier
*est* bien déplacé une poignée de millisecondes plus tard (donc un
« Résoudre + supprimer » ultérieur ne nettoyait pas la copie en
quarantaine). Corrigé : `parse_clamscan_line(&line)` ne fait plus que
parser chemin+signature, et un nouveau `resolve_move_status()` re-résout
la disposition de chaque menace dans une boucle *après* `child.wait()`,
quand tous les déplacements sont terminés. Vérifié par
`security.spec.ts` test 3.1 (scan EICAR réel de bout en bout) +
2 tests unitaires `resolve_move_status`.

**Docker** est sorti de cette liste le 2026-09-05 : suite E2E complète
`docker.spec.ts` (49 tests, 14 catégories) écrite et débuguée contre un
vrai Docker Engine — aucun bug applicatif trouvé, uniquement des bugs de
fichier de test (locators mal scopés sur le modal qui est un sibling DOM
de `.dk-shell`, pas un descendant ; sélecteur de bouton de fermeture trop
large qui interceptait le bouton "Close" du widget Resource Monitor
masqué ; `fetch` appelé dans `page.evaluate` avant toute navigation).

## Test réel live 2026-09-07 (Pare-feu, Shares/SFTP, RAID/Storage, Journal)

Les 4 zones exercées de bout en bout contre le vrai host (drop-in
`sudoers` NOPASSWD temporaire, 6 loop devices, cycle `ufw` réel avec le
port 22 + l'IP SSH source pré-autorisés, `sshd` `homevault-sftp`). Tout
l'environnement a été restauré ensuite (drop-in retiré, loops détachés,
comptes/partages/règles de test supprimés, `ufw` laissé inactif).

**10 bugs corrigés (fichiers frozen, autorisation explicite), tous
re-vérifiés en direct sur le binaire reconstruit :**

| # | Fichier(s) | Bug | Impact avant correctif |
|---|---|---|---|
| 1 | `journal_cleanup_scheduler.rs`, `storage/journal_cleanup_repo.rs`, `handlers/journal.rs` | Le scheduler d'auto-nettoyage (défaut `on`/90 j depuis G1) tourne au 1er tick immédiat ; table `journal_cleanup_settings` vide → `last_cleanup_at=None` → `due=true` ; `set_last_cleanup` était un `UPDATE … WHERE id=1` no-op | `clear_app_logs()` supprimait `homevault.log.<jour>` juste après que l'appender l'a ouvert → **source « app » du Journal vide en permanence, à chaque redémarrage** ; tout l'audit applicatif (login/2FA/CRUD users-groupes-partages/Docker/sécurité…) perdu. Corrigé : branche `None` = pose une baseline sans nettoyer + `set_last_cleanup` UPSERT + `clear_app_logs` **tronque** le fichier du jour au lieu de le supprimer (garde l'inode ouvert par l'appender) |
| 2 | `disk.rs::mount_and_persist_raid` | `Command::new("mount")` / `Command::new("blkid")` nus | Création RAID mdadm : « seul le superutilisateur peut utiliser mount » → array créé mais non monté puis tear-down. OK en prod (root), cassé hors-root, incohérent avec `sudo mkdir`/`sudo mdadm` juste à côté |
| 3 | `disk.rs::create_zfs_pool_from_devices`, `pools.rs::create_zfs_advanced_pool` | `zpool create -f <type-vdev> <nom> …` — le nom du pool doit précéder le type | **Toute création ZFS échouait** (`cannot open '<nom>': no such device`) pour raid1/5/6/10 ; seul raid0 passait. Casse même en root. Aussi `Command::new("zpool")` sans `sudo` |
| 3b | `pools.rs::scan_pools` (`get_mdadm_devices`, `get_mdadm_mount_point`, `get_mdadm_size`, `get_zfs_devices`, `get_zfs_mount_point`) | Vue « Disques virtuels » du Storage : `device_count` faux (mdadm 2 disques → 1 ; miroir ZFS 2 disques → 5), `mount_point` toujours `null` (mdadm : match sur le symlink pas `/dev/md127` ; ZFS : `zpool get mountpoint` invalide → il fallait `zfs get`), `used_bytes` mdadm = `size/2` fabriqué, `size_bytes` mdadm sous-compté ×1024 (KiB lu comme octets) | Métadonnées trompeuses (jamais bloquant). Corrigé : parse de la table `mdadm --detail`, `canonicalize`, `zfs get`, `df` réel, `blocks*1024` |
| 4 | `disk.rs::delete_raid` | `Command::new("umount")` nu | Destruction d'un disque virtuel mdadm : `umount` échoue en silence → `mdadm --stop` « device busy » → **delete avorté, array + `/etc/fstab` + `mdadm.conf` laissés en place** |
| 5 | `shares.rs::set_share_enabled` (SMB) | Fin de section repérée via `after_section.find("\n[")` — `None` pour la **dernière** section de `smb.conf` | Le partage SMB fraîchement créé (toujours ajouté en fin de fichier) **ne peut jamais être désactivé** tant qu'un autre n'est pas ajouté après. Réponse `"Share disabled"` mensongère. Casse même en root |
| 6 | `shares.rs` (create/update/delete `_sftp`/`_ftp`/`_webdav`_share, `save_*_config`) | `fs::write` / `fs::create_dir_all` nus vers `/etc/homevault/*.json` (dir root) | Création d'un partage SFTP/FTP/WebDAV : `Permission denied` hors-root ; incohérent avec `write_file_as_root` partout ailleurs. Corrigé + nouveau helper `mkdir_as_root` |
| 7 | `acl.rs` (4 sites `setfacl`) | `Command::new("setfacl")` nu | Tout grant/revoke d'ACL de partage : `EPERM` hors-root si le dossier n'appartient pas au process |
| 8 | `acl.rs::remove_acl` | Retire l'entrée d'accès mais pas l'ACL **par défaut** correspondante | Un utilisateur révoqué restait accordé sur tout fichier créé ensuite dans le partage (`default:user:X:rwx` persistait) |
| 9 | `handlers/shares.rs::handle_sftp_key_set` | Aucune vérif que le `username` est un utilisateur HomeVault | `PUT /api/shares/sftp/users/key` avec un nom quelconque → vrai `useradd` → comptes Linux orphelins sur faute de frappe. Corrigé via `state.auth.user_exists()` |
| 10 | `firewall.rs::get_status()` | Grep la chaîne anglaise `Status: active` | Sur host locale non-anglaise (`fr_BE` → `ufw` affiche `État : actif`, et `LANGUAGE` override même `LC_ALL=C`) → `enabled` **toujours `false`** : la carte Pare-feu montre « désactivé » alors qu'`ufw` est actif. Corrigé : helper `ufw_command()` qui force `LC_ALL=C`/`LANGUAGE=` (gardés par le `env_keep` par défaut de `sudo`) |

**Vérifié OK en réel (aucun bug) :** garde `{confirmed:true}` du pare-feu,
403 sur suppression d'une règle obligatoire, règles visibles `ufw`
désactivé, `ensure_mandatory_rules` (SSH survit à l'`enable`) ;
`rebuild_jails` (bind-mount + `fstab`), `purge_user` sur suppression d'un
utilisateur HomeVault, ACL par défaut (ordre `-d`/`-m`) ; Btrfs natif
cycle complet ; `sudo journalctl`/`tail` + filtres unit/priorité/search du
Journal ; rejet de traversée `source=../../etc/passwd`.

**Toujours pas couvert en test automatisé** (nécessite le host réel — reste
E2E/manuel) : un vrai `ufw enable` (redémarrage possible du réseau), un
vrai partage SMB/NFS/SFTP créé, un vrai `mdadm`/`zpool create`. Les suites
d'intégration s'arrêtent toujours au « rejet propre ».

## 2026-09-07 (suite) — i18n de/es + reste des tests host

**i18n de/es (fait).** `src/i18n/de.toml` et `es.toml` avaient le même
défaut que `fr` (sections entières écrites sans diacritiques). Restauration
des accents **valeurs seulement** (clés + `%placeholders%` vérifiés
intacts, `tomllib` + `cargo build --release` clean) : ~350 lignes chacun,
via des maps mot-entier curées (`fix_de.py` / `fix_es.py`, scratch dir).
`de` : digraphes `ae/oe/ue/ss` → `ä/ö/ü/ß` (faux-positifs `aktuell`,
`daemon`, `neue`, `manuell`, `quell`, `steuer`, `virtuelle`, … exclus).
`es` : `-ción/-sión` singulier, `éxito/código/tamaño/batería/contraseña/…`,
quelques futurs, `¿?` sur les questions évidentes ; pluriels `-ciones` et
`solo` laissés tels quels (corrects). Le bloc `de`/`es`/`fr` embarqué du
frontend (`i18n.ts`) était déjà correctement accentué — non touché.

**2ᵉ passe fr + es (« refais un tour du projet »).** La 1ʳᵉ passe fr avait
faussement classé `[settings]` `[shares]` `[users]` `[groups]` `[docker]`
(résiduels), `[journal]` `[common]` `[cache]` `[account]` comme « déjà
accentués » — ils étaient en fait largement nus. Corrigé via `fix_fr.py`
(~155 lignes, whole-word ; toutes les formes verbe/présent ambiguës —
`utilise`, `gere`, `affiche`, `chiffre`, `complete`, `limite`, `detecte`,
`synchronise`, `verifie`, `deploye`, `genere`, `execute`… — **exclues de
la map**). 5 mis-accents impératif-vs-participe (« Installe/Supprime/
Démarre… ») + `À propos`/`À utiliser` + 2 `_synced` corrigés à la main
après revue du diff. `es` : `máscara`, `estática`, `público`, futurs
`aparecerán/aplicarán/formarán/moverán/perderán` (~11 lignes).

**Vérification linguistique complète + suppression des orphelines
(2026-09-08).** Parité des clés désormais **identique** dans les 4
langues (`en:common.browse_button` → `common.browse`) ; `%placeholders%`
cohérents partout (0 écart) ; aucune valeur vide ; aucune fuite d'anglais.
Derniers résidus d'accents corrigés (26 lignes : fr 9, de 7 « voyelle
nue » — `konnte`/`wurde` = prétérit correct, pas touché —, es 9).
**825 clés orphelines supprimées** des 4 fichiers (3048 → **2223**) —
analyse : tous les lookups i18n énumérés (statiques + liste fixe de
préfixes dynamiques + `docker.${actionKey}` à 6 valeurs), 0/825 cité
borné dans le source, 25/25 spot-checks propres, `cargo build` +
`svelte-check` 0 erreur, E2E `dashboard`/`health`/`widgets`/`settings`/
`explorer`/`network` verts.

**2026-09-14 — 2ᵉ passe, orphelines de la standardisation des en-têtes.**
19 clés de plus supprimées des 4 fichiers (2522 → **2503**) : ce sont les
sous-titres retirés des `WindowHeader` — `docker.containers_summary`,
`users.summary`, `quotas.subtitle`, `groups.summary`, `shares.summary`,
`account.subtitle_{admin,user}`, `dashboard.overview_subtitle`,
`rclone.subtitle`, `raid.manager.header_subtitle`, `network.subtitle`,
`chat.subtitle_default`, `chat.model_via`, `vpn.subtitle`,
`downloader.subtitle`, `syncthing.subtitle`, `widgets.subtitle`,
`rescue.subtitle`, `journal.subtitle`. Suppression **par (section, clé)** et
non par nom de feuille : six autres `subtitle` sont vivants et ont été
conservés après vérification un par un (`settings.` = libellé du rail +
`aria-label` de nav, `restic.` = `tabsLabel`, `login.` = page de connexion,
`antivirus.`/`ransomware.`/`firewall.` = sous-titres de `SettingsCard`).
`docker.${actionKey}` revérifié — il ne produit que les 6
`{container,stack}_{started,stopped,restarted}`, `containers_summary` est
donc sûr. Vérifié après coup par script : 2503 clés dans chacune des 4
langues, ensembles de clés **identiques**, ensembles de `%placeholders%`
identiques (0 écart), 19/19 clés absentes partout, 6/6 clés conservées
présentes partout, aucune section vidée. **Rien n'a été compilé ni relancé.**

Les compteurs de tests de ce document ne bougent pas : cette passe ne touche
que `src/i18n/*.toml` et des attributs `size=` de snippets d'icône.

**Reste** : relecture native de `es` (temps verbaux en phrase longue) ;
quelques `a`↔`à` épars dans `fr`.

**Tests host réels (drop-in `/etc/sudoers.d` NOPASSWD temporaire, retiré
après ; 4 loop devices + un RAID1 jetable ; environnement restauré).**

- **Rescue — FAIT (jamais exercé en réel avant). 6 bugs corrigés** (tous
  frozen `src/rescue/*`, re-vérifiés live) : ddrescue/testdisk/photorec
  jamais `sudo`-wrappés (seul mdadm l'était) → sur non-root tout échouait
  en silence (`success:true` + partitions vides) ; `list_block_devices`
  parsait `lsblk -o` positionnellement (MODEL a des espaces → modèle/
  transport/removable faux) + `parse_size` privé avec le bug
  cast-avant-multiplication + virgule de locale (« 465,8G » → `size:0`) —
  réécrit en `lsblk -P -b` ; `mdadm_diagnose` lisait la ligne d'en-tête
  `/dev/md127:` au lieu de la table des membres ; santé d'un miroir
  dégradé (`"clean, degraded"`) retournait `healthy` (ordre des `contains`) ;
  `parse_testdisk_output` offsets faux pour TestDisk 7.1 `/list` (jamais
  exécuté car pas de sudo) → réécrit + 3 tests unitaires ;
  `testdisk_rebuild` n'a jamais marché (script stdin ignoré, device
  manquant) **et pouvait désormais bloquer le worker** (TestDisk 7.1
  `/cmd` `write` attend un Y/N que stdin ne peut donner) → réduit à
  `analyze,search` + `stdin(null)` + timeout 300 s + `kill_on_drop`. Voir
  l'invariant « Storage / RAID / encryption » de `CLAUDE.md` et
  `docs/guides/rescue.md`.
- **Storage / LUKS — FAIT. Cycle complet** create → écrire un fichier →
  `/lock` → `/unlock` (mauvaise phrase rejetée, bonne remonte, données
  intactes) → `DELETE {confirm:true}` contre un vrai `/dev/loopN` +
  `cryptsetup`. Le lifecycle (sudo-wrap + `write_file_as_root` du
  2026-07-17) tient. **2 bugs corrigés** (frozen) : `handle_encryption_
  create` ne validait **pas** `req.device` → un admin pouvait passer
  `/dev/sda` à `cryptsetup luksFormat` (gardé maintenant par
  `get_available_devices` `!in_use`) ; `encryption::erase` laissait
  l'en-tête LUKS → device coincé `in_use` pour toujours (ajout d'un
  `wipefs -a` best-effort). Invariant sous « Storage / RAID / encryption »
  (`CLAUDE.md`) et `docs/guides/storage.md`.
- **Network — reads re-vérifiés live + bugs `sudo` du chemin d'application
  CORRIGÉS** (frozen `src/network.rs`, sur « corrige ce qu'il faut »).
  Reads OK (`/interfaces`, `/diagnostics`, `/summary`, `/services`,
  `/public-ip`, `/wifi/status`, `POST /wifi/scan`), rejet de body invalide
  OK. Corrigé : helper `ip_sudo(program)` — chaque `ip`/`dhclient` de
  *mutation* (`addr add/flush`, `route replace`, `link set up/down`,
  `dhclient -6`) passe par `sudo` (les `ip … show`/`ping`/`ethtool` en
  lecture restent nus) ; `/etc/resolv.conf` + les toggles
  `/proc/sys/net/ipv6/conf/<iface>/*` écrits via `sudo tee`
  (`write_as_root`) au lieu d'un `std::fs::write` silencieux ; le DNS ne
  réécrit plus que les lignes `nameserver` (`build_resolv_conf`, pur,
  3 tests unitaires — `search`/`options`/commentaires conservés).
  **Mutations réelles toujours NON exercées** (host en SSH) et la
  **persistance** (netplan / `/etc/network/interfaces` — `ip addr add` est
  runtime-only) reste un follow-up délibéré (choix de backend + session
  console). Invariant sous « Network » (`CLAUDE.md`) et
  `docs/guides/network.md`.

## Méthode de vérification

- Fichiers `tests/*.rs` listés via `ls`, comptés via
  `grep -cE '#\[tokio::test\]|#\[test\]'` (nombre d'attributs de test =
  nombre de fonctions de test).
- Fichiers `tests/e2e/**/*.spec.ts` listés via `find`, comptés via
  `grep -cE '^\s*test\('` (les tests sont indentés dans leur `test.describe(` ;
  le motif doit être ancré en début de ligne). **Ne pas** utiliser
  `grep -c 'test('` : il attrape aussi les `.test(x)` des assertions à
  expression régulière (`/…/.test(url)`) et sur-compte. Le nombre retenu est
  celui de Playwright — `npx playwright test <spec> --list --project=firefox`
  — qui est la référence. **Les deux ne coïncident pas, et c'est normal** : un
  `test(` écrit **dans une boucle** n'est compté qu'une fois par le grep alors
  que Playwright le développe en N tests à la collecte (ex. la boucle
  responsive 360/768/1920 de `storage.spec.ts` : 1 ligne, 3 tests). Le grep
  est donc une **borne inférieure** ; l'écart mesure les tests générés par
  boucle. Constaté le 2026-09-24 : **1367** au grep contre **1436** à
  `--list --project=firefox` (69 tests générés) sur les 28 fichiers `.spec.ts`.
- Tests unitaires par module : `grep -cE '^\s+fn test_'` dans le fichier
  source, modules trouvés via `grep -rl '#\[cfg(test)\]' src/`.
- Les statuts par zone viennent de `CLAUDE.md` et de `docs/guides/` (non
  re-vérifiés contre le code à cette passe — voir la mise en garde de
  `ROADMAP.md` sur la fraîcheur de ce type d'information).

À remettre à jour à chaque nouveau fichier de test ajouté ou nouvel audit
manuel documenté dans `CLAUDE.md`.
