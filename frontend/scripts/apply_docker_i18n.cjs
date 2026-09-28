global.BT = String.fromCharCode(96); // backtick, defined globally so String.raw sees it
const fs = require('fs');
const p = 'src/routes/Docker.svelte';
let s = fs.readFileSync(p, 'utf8');
const R = String.raw;

const subs = [];
const add = (a, b) => subs.push([a, b]);

// ---- script-section (translate) ---- (backticks escaped as \`, literal ${ as \${) ----
add(R`dockerAvailable === false\n      ? 'Docker non disponible sur ce serveur'`,
     R`dockerAvailable === false\n      ? translate('docker.unavailable')`);
add(R`containers.length === 0\n        ? 'Aucun conteneur'`,
     R`containers.length === 0\n        ? translate('docker.no_containers2')`);
add(R`        : \`${containers.length} conteneurs · ${runningCount} actifs\``,
     R`        : translate('docker.containers_summary', { total: String(containers.length), running: String(runningCount) })`);
add(`res.message || res.error || 'Échec du chargement des conteneurs'`, `res.message || res.error || translate('docker.load_containers_failed')`);
add(`res.message || res.error || 'Échec du chargement des images'`, `res.message || res.error || translate('docker.load_images_failed')`);
add(`res.message || res.error || 'Échec du chargement des réseaux'`, `res.message || res.error || translate('docker.load_networks_failed')`);
add(`res.message || res.error || 'Échec du chargement des projets Compose'`, `res.message || res.error || translate('docker.load_projects_failed')`);
add(R`if (res.success) { setBanner('ok', \`Conteneur « ${name} » ${action === 'start' ? 'démarré' : action === 'stop' ? 'arrêté' : 'redémarré'}\`); }`,
     R`if (res.success) { setBanner('ok', translate(\`docker.container_${action}ed\`, { name })); }`);
add(R`else setBanner('error', res.message || res.error || \`Échec de ${action}\`)`,
     R`else setBanner('error', res.message || res.error || translate('docker.action_failed', { action }))`);
add(R`if (!confirm(\`Supprimer le conteneur « ${name} » ?${force ? ' (force)' : ''}\`)) return;`,
     R`if (!confirm(translate('docker.container_delete_confirm', { name }) + (force ? translate('docker.container_force') : ''))) return;`);
add(R`if (res.success) setBanner('ok', \`Conteneur « ${name} » supprimé\`);\n    else setBanner('error', res.message || res.error || 'Échec de la suppression');`,
     R`if (res.success) setBanner('ok', translate('docker.container_deleted', { name }));\n    else setBanner('error', res.message || res.error || translate('docker.container_delete_failed'));`);
add(`containerLogs = 'Chargement des journaux…';`, `containerLogs = translate('docker.logs_loading');`);
add(`if (res.success && res.data) containerLogs = res.data.logs || '(aucun journal)';`, `if (res.success && res.data) containerLogs = res.data.logs || translate('docker.logs_empty');`);
add(`else containerLogs = res.message || res.error || 'Impossible de récupérer les journaux';`, `else containerLogs = res.message || res.error || translate('docker.logs_failed');`);
add(`const image = prompt('Nom de l’image à tirer (ex : nginx:latest) :');`, `const image = prompt(translate('docker.pull_prompt'));`);
add(R`if (res.success) setBanner('ok', \`Image « ${image} » tirée\`);\n    else setBanner('error', res.message || res.error || 'Échec du tirage');`,
     R`if (res.success) setBanner('ok', translate('docker.image_pulled', { name: image }));\n    else setBanner('error', res.message || res.error || translate('docker.pull_failed2'));`);
add(R`if (!confirm(\`Supprimer l’image « ${id} » ?\`)) return;`,
     R`if (!confirm(translate('docker.image_delete_confirm', { id }))) return;`);
add(R`if (res.success) setBanner('ok', 'Image supprimée');\n    else setBanner('error', res.message || res.error || 'Échec de la suppression');`,
     R`if (res.success) setBanner('ok', translate('docker.image_deleted'));\n    else setBanner('error', res.message || res.error || translate('docker.delete_failed'));`);
add(`if (!nName.trim()) { setBanner('error', 'Le nom du réseau est requis'); return; }`, `if (!nName.trim()) { setBanner('error', translate('docker.network_name_required')); return; }`);
add(R`if (res.success) { setBanner('ok', \`Réseau « ${nName} » créé\`); netModalOpen = false; }\n    else setBanner('error', res.message || res.error || 'Échec de la création');`,
     R`if (res.success) { setBanner('ok', translate('docker.network_created', { name: nName })); netModalOpen = false; }\n    else setBanner('error', res.message || res.error || translate('docker.network_create_failed'));`);
add(R`if (!confirm(\`Supprimer le réseau « ${name} » ?\`)) return;`,
     R`if (!confirm(translate('docker.network_delete_confirm', { name }))) return;`);
add(R`if (res.success) setBanner('ok', 'Réseau supprimé');\n    else setBanner('error', res.message || res.error || 'Échec de la suppression');`,
     R`if (res.success) setBanner('ok', translate('docker.network_deleted'));\n    else setBanner('error', res.message || res.error || translate('docker.delete_failed'));`);
add(`else setBanner('error', res.message || res.error || 'Échec de la recherche Docker Hub');`, `else setBanner('error', res.message || res.error || translate('docker.hub_search_failed'));`);
add(`if (!cName.trim()) { setBanner('error', 'Le nom du conteneur est requis'); return; }`, `if (!cName.trim()) { setBanner('error', translate('docker.container_name_required')); return; }`);
add(`if (!cImage.trim()) { setBanner('error', 'L’image est requise'); return; }`, `if (!cImage.trim()) { setBanner('error', translate('docker.image_required')); return; }`);
add(R`if (res.success) { setBanner('ok', \`Conteneur « ${cName} » créé\`); closeModal(); await loadContainers(); }\n    else setBanner('error', res.message || res.error || 'Échec de la création');`,
     R`if (res.success) { setBanner('ok', translate('docker.container_created', { name: cName })); closeModal(); await loadContainers(); }\n    else setBanner('error', res.message || res.error || translate('docker.create_failed2'));`);
add(`if (!composeName.trim()) { composeStatus = { text: 'Le nom de la pile est requis.', color: '#ef4444' }; return; }`, `if (!composeName.trim()) { composeStatus = { text: translate('docker.stack_name_required'), color: '#ef4444' }; return; }`);
add(`if (!composeYaml.trim()) { composeStatus = { text: 'Colle un fichier docker-compose.yml avant de déployer.', color: '#ef4444' }; return; }`, `if (!composeYaml.trim()) { composeStatus = { text: translate('docker.paste_compose_first'), color: '#ef4444' }; return; }`);
add(`composeStatus = { text: 'Déploiement en cours…', color: '#64748b' };`, `composeStatus = { text: translate('docker.deploying'), color: '#64748b' };`);
add(R`if (res.success) { composeStatus = { text: 'Pile déployée avec succès ✓', color: '#0d9668' }; composeName = ''; composeYaml = ''; await loadProjects(); }`,
     R`if (res.success) { composeStatus = { text: translate('docker.stack_deployed'), color: '#0d9668' }; composeName = ''; composeYaml = ''; await loadProjects(); }`);
add(R`else composeStatus = { text: res.message || res.error || 'Échec du déploiement', color: '#ef4444' };`,
     R`else composeStatus = { text: res.message || res.error || translate('docker.deploy_failed'), color: '#ef4444' };`);
add(R`else setBanner('error', res.message || res.error || \`Échec de ${action}\`)`,
     R`else setBanner('error', res.message || res.error || translate('docker.action_failed', { action }))`);
add(R`if (!confirm(\`Supprimer la pile Compose « ${name} » ?\`)) return;`,
     R`if (!confirm(translate('docker.stack_delete_confirm', { name }))) return;`);
add(R`if (res.success) setBanner('ok', 'Pile supprimée');\n    else setBanner('error', res.message || res.error || 'Échec de la suppression');`,
     R`if (res.success) setBanner('ok', translate('docker.stack_deleted'));\n    else setBanner('error', res.message || res.error || translate('docker.stack_delete_failed'));`);

// ---- markup-section ($t) ---- (no backticks in these) ----
add(`Nouveau conteneur</button>`, `{$t('docker.new_container')}</button>`);
add(`Actualiser\n          </button>`, `{$t('docker.refresh')}\n          </button>`);
add(`>Conteneurs</button>`, `>{$t('docker.tab_containers')}</button>`);
add(`>Images</button>`, `>{$t('docker.tab_images')}</button>`);
add(`>Réseaux</button>`, `>{$t('docker.tab_networks')}</button>`);
add(`>Découvrir</button>`, `>{$t('docker.tab_discover')}</button>`);
add(`>Compose</button>`, `>{$t('docker.tab_compose')}</button>`);
add(`<h3>Docker n’est pas disponible</h3>`, `<h3>{$t('docker.unavailable')}</h3>`);
add(`<p>Le démon Docker n’est pas accessible sur ce serveur. Installe et démarre Docker, puis relie l’utilisateur au groupe <code>docker</code>.</p>`, `<p>{$t('docker.unavailable_desc')}</p>`);
add(`<p>Vérification de Docker…</p>`, `<p>{$t('docker.checking_status')}</p>`);
add(`<h3>Aucun conteneur pour l’instant</h3>`, `<h3>{$t('docker.no_containers2')}</h3>`);
add(`<p>Crée un conteneur ou installe une application depuis « Découvrir ».</p>`, `<p>{$t('docker.no_containers_desc')}</p>`);
add(`Nouveau conteneur</button>\n        <button class="dk-btn dk-btn-primary" onclick={() => openCreateModal()}>Nouveau conteneur</button>`, `Nouveau conteneur</button>\n        <button class="dk-btn dk-btn-primary" onclick={() => openCreateModal()}>{$t('docker.new_container')}</button>`);
add(`{c.state === 'Running' ? 'En cours' : c.state === 'Exited' ? 'Arrêté' : c.state}`, `{c.state === 'Running' ? $t('docker.status_running') : c.state === 'Exited' ? $t('docker.status_stopped') : c.state}`);
add(`<span>Image</span>`, `<span>{$t('docker.meta_image')}</span>`);
add(`<span>CPU</span>`, `<span>{$t('docker.meta_cpu')}</span>`);
add(`<span>RAM</span>`, `<span>{$t('docker.meta_ram')}</span>`);
add(`<span>Stack</span>`, `<span>{$t('docker.meta_stack')}</span>`);
add(`title="Journaux"`, `title={$t('docker.logs')}`);
add(`title="Arrêter"`, `title={$t('docker.stop')}`);
add(`title="Démarrer"`, `title={$t('docker.start')}`);
add(`title="Redémarrer"`, `title={$t('docker.restart')}`);
add(`title="Supprimer"`, `title={$t('docker.delete')}`);
add(`Tirer une image</button>`, `{$t('docker.pull_image')}</button>`);
add(`<h3>Aucune image locale</h3>`, `<h3>{$t('docker.no_images2')}</h3>`);
add(`<p>Tire une image depuis un registre pour commencer.</p>`, `<p>{$t('docker.pull_image_desc')}</p>`);
add(`Créer un réseau</button>`, `{$t('docker.create_network')}</button>`);
add(`<h3>Aucun réseau</h3>`, `<h3>{$t('docker.no_networks')}</h3>`);
add(`placeholder="Rechercher sur Docker Hub (ex : nginx, postgres…)"`, `placeholder={$t('docker.search_placeholder')}`);
add(`{hubLoading ? 'Recherche…' : 'Rechercher'}`, `{$t('docker.searching') : $t('docker.search')}`);
add(`Tape un mot-clé et lance une recherche pour découvrir des images officielles.`, `{$t('docker.no_results')}`);
add(`>OFFICIEL</span>`, `>{$t('docker.official')}</span>`);
add(`{r.description || 'Pas de description.'}`, `{r.description || $t('docker.no_description')}`);
add(`>Docker Hub</span>`, `>{$t('docker.catalog_source')}</span>`);
add(`Installer</button>`, `{$t('docker.install')}</button>`);
add(`Déployer avec Docker Compose</h3>`, `{$t('docker.deploy_title')}</h3>`);
add(`Colle ton fichier <code>docker-compose.yml</code>. La pile est créée et démarrée réellement via le démon Docker.`, `{$t('docker.deploy_desc')}`);
add(`Nom de la pile (stack)</label>`, `{$t('docker.stack_name')}</label>`);
add(`Fichier docker-compose.yml</label>`, `{$t('docker.compose_file')}</label>`);
add(`Déployer</button>`, `{$t('docker.deploy')}</button>`);
add(`Piles déployées</h3>`, `{$t('docker.deployed_stacks')}</h3>`);
add(`Journaux : {logContainerName}`, `{$t('docker.title_logs', { name: logContainerName })}`);
add(`<h3>Nouveau conteneur</h3>`, `<h3>{$t('docker.modal_new_container')}</h3>`);
add(`<label class="dk-field-label" for="cName">Nom</label>`, `<label class="dk-field-label" for="cName">{$t('docker.name')}</label>`);
add(`placeholder="mon-conteneur"`, `placeholder={$t('docker.ph_name')}`);
add(`<label class="dk-field-label" for="cImage">Image</label>`, `<label class="dk-field-label" for="cImage">{$t('docker.image')}</label>`);
add(`placeholder="nginx:latest"`, `placeholder={$t('docker.ph_image')}`);
add(`<span>Ports</span>`, `<span>{$t('docker.ports_section')}</span>`);
add(`+ Port</button>`, `{$t('docker.add_port2')}</button>`);
add(`placeholder="Hôte"`, `placeholder={$t('docker.ph_host')}`);
add(`placeholder="Conteneur"`, `placeholder={$t('docker.ph_container')}`);
add(`<span>Volumes</span>`, `<span>{$t('docker.volumes_section')}</span>`);
add(`+ Volume</button>`, `{$t('docker.add_volume2')}</button>`);
add(`placeholder="Chemin hôte"`, `placeholder={$t('docker.ph_host_path')}`);
add(`placeholder="Chemin conteneur"`, `placeholder={$t('docker.ph_container_path')}`);
add(`<span>Variables d’environnement</span>`, `<span>{$t('docker.env_section')}</span>`);
add(`+ Var</button>`, `{$t('docker.add_env2')}</button>`);
add(`placeholder="CLÉ"`, `placeholder={$t('docker.ph_key')}`);
add(`placeholder="valeur"`, `placeholder={$t('docker.ph_value')}`);
add(`<label class="dk-field-label" for="cRestart">Politique de redémarrage</label>`, `<label class="dk-field-label" for="cRestart">{$t('docker.restart_policy2')}</label>`);
add(`<label class="dk-field-label" for="cNetwork">Réseau</label>`, `<label class="dk-field-label" for="cNetwork">{$t('docker.network_label')}</label>`);
add(`<label class="dk-field-label" for="cMemory">Mémoire (ex : 512m)</label>`, `<label class="dk-field-label" for="cMemory">{$t('docker.memory_label')}</label>`);
add(`placeholder="512m"`, `placeholder={$t('docker.ph_memory')}`);
add(`<label class="dk-field-label" for="cCpu">CPU (ex : 1.5)</label>`, `<label class="dk-field-label" for="cCpu">{$t('docker.cpu_label')}</label>`);
add(`placeholder="1.0"`, `placeholder={$t('docker.ph_cpu')}`);
add(`<div class="dk-t1">Privilégié</div>`, `<div class="dk-t1">{$t('docker.privileged2')}</div>`);
add(`Donne tous les privilèges au conteneur (--privileged).`, `{$t('docker.privileged_desc')}`);
add(`<div class="dk-t1">Suppression auto (--rm)</div>`, `<div class="dk-t1">{$t('docker.auto_remove2')}</div>`);
add(`Supprime le conteneur à l’arrêt.`, `{$t('docker.auto_remove_desc')}`);
add(`>Annuler</button>`, `>{$t('common.cancel')}</button>`);
add(`{modalBusy ? 'Création…' : 'Créer le conteneur'}`, `{$t('docker.creating2') : $t('docker.create_container2')}`);
add(`<h3>Créer un réseau</h3>`, `<h3>{$t('docker.create_network_title')}</h3>`);
add(`<label class="dk-field-label" for="nName">Nom</label>`, `<label class="dk-field-label" for="nName">{$t('docker.name')}</label>`);
add(`placeholder="mon-reseau"`, `placeholder={$t('docker.ph_name')}`);
add(`<label class="dk-field-label" for="nDriver">Pilote</label>`, `<label class="dk-field-label" for="nDriver">{$t('docker.driver')}</label>`);
add(`<label class="dk-field-label" for="nSubnet">Subnet (option)</label>`, `<label class="dk-field-label" for="nSubnet">{$t('docker.subnet_opt')}</label>`);
add(`placeholder="172.20.0.0/16"`, `placeholder={$t('docker.ph_subnet')}`);
add(`<label class="dk-field-label" for="nGateway">Passerelle (option)</label>`, `<label class="dk-field-label" for="nGateway">{$t('docker.gateway_opt')}</label>`);
add(`placeholder="172.20.0.1"`, `placeholder={$t('docker.ph_gateway')}`);
add(`<div class="dk-t1">Réseau interne</div>`, `<div class="dk-t1">{$t('docker.internal_network')}</div>`);
add(`>Annuler</button>`, `>{$t('common.cancel')}</button>`);
add(`>Créer</button>`, `>{$t('docker.create_network')}</button>`);
add(`aria-label="Fermer"`, `aria-label={$t('common.close')}`);

let replaced = 0, missing = [];
for (const [a, b] of subs) {
  if (s.includes(a)) { s = s.split(a).join(b); replaced++; }
  else missing.push(a.slice(0, 70));
}
fs.writeFileSync(p, s);
console.log('replaced', replaced, '/', subs.length);
if (missing.length) { console.log('NOT FOUND:'); missing.forEach(m => console.log('  -', JSON.stringify(m))); }
