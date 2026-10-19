/* =============================================================================
 * File          : chat.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   AI-assistant endpoints proxying the local Ollama service: send a
 *   message (blocking) or stream the reply token-by-token over SSE, list
 *   installed models, and pull new models (admin — multi-gigabyte
 *   download). The system prompt follows the user's UI language and is
 *   enriched with knowledge-base context when the RAG index is available;
 *   the active model lives in a process-wide setting.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - futures-util
 *   - once_cell
 *   - parking_lot (>= 0.12)
 *   - reqwest
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   Routes /api/chat in src/web/mod.rs; chat/streams/models are
 *   session-auth, model pull is admin-only. See docs/guides/chat.md and
 *   docs/guides/rag.md (grounding).
 * =============================================================================
 */

//! AI-assistant endpoints (Ollama proxy): blocking and SSE-streamed chat,
//! model list, admin-only model pull, language-aware + RAG-enriched prompts.

use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::{extract::State, Json};
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Chat (send message) ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub message: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub history: Vec<ChatMessage>,
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub success: bool,
    pub data: Option<ChatResponseData>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ChatResponseData {
    pub reply: String,
    pub conversation_id: String,
    pub model: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    message: OllamaMessage,
    model: String,
    done: bool,
}

const OLLAMA_BASE: &str = "http://localhost:11434";
const DEFAULT_MODEL: &str = "gemma3:1b";

pub static AI_MODEL: Lazy<RwLock<String>> = Lazy::new(|| RwLock::new(DEFAULT_MODEL.to_string()));

/// Resets the active model back to the factory default — used by "Reset to
/// Defaults" on the Settings page, which otherwise never touched AI_MODEL.
pub fn reset_ai_model() {
    *AI_MODEL.write() = DEFAULT_MODEL.to_string();
}

/// Builds the system prompt in the user's selected UI language so the
/// assistant's replies match the rest of the interface instead of always
/// answering in French regardless of the `language` setting.
fn system_prompt(language: &str) -> String {
    match language {
        "fr" => r#"Tu es un assistant IA intégré à HomeVault, un logiciel de gestion de serveur domestique.

Tu aides l'utilisateur à utiliser HomeVault. Voici les fonctionnalités principales :
- Dashboard : vue d'ensemble du système (CPU, RAM, température, réseau)
- Explorateur de fichiers : navigation, recherche, upload, download, preview
- Stockage : disques, partitions, RAID, pools ZFS
- Docker : gestion des conteneurs, images, réseaux, Compose
- Sauvegardes : création, restauration, vérification
- Utilisateurs & Groupes : gestion des comptes et permissions
- Partages : FTP, WebDAV avec permissions
- Cloud Sync : synchronisation avec services cloud via Rclone
- Syncthing : synchronisation pair-à-pair
- Réseau : monitoring des interfaces
- Media : streaming vidéo avec HLS, transcodage
- Santé : surveillance de l'état du système
- Terminal : accès shell (admin seulement)
- Snapshots : gestion des snapshots ZFS
- Paramètres : configuration de l'apparence et notifications

Réponds en français de façon concise et utile. Si l'utilisateur demande une action précise, guide-le étape par étape.
Si tu ne sais pas répondre, indique-le honnêtement."#.to_string(),
        "de" => r#"Du bist ein in HomeVault integrierter KI-Assistent, eine Software zur Verwaltung von Heimservern.

Du hilfst dem Benutzer bei der Nutzung von HomeVault. Hier sind die wichtigsten Funktionen:
- Dashboard: Systemübersicht (CPU, RAM, Temperatur, Netzwerk)
- Datei-Explorer: Navigation, Suche, Upload, Download, Vorschau
- Speicher: Festplatten, Partitionen, RAID, ZFS-Pools
- Docker: Verwaltung von Containern, Images, Netzwerken, Compose
- Backups: Erstellung, Wiederherstellung, Überprüfung
- Benutzer & Gruppen: Konten- und Berechtigungsverwaltung
- Freigaben: FTP, WebDAV mit Berechtigungen
- Cloud Sync: Synchronisation mit Cloud-Diensten über Rclone
- Syncthing: Peer-to-Peer-Synchronisation
- Netzwerk: Überwachung der Schnittstellen
- Media: Video-Streaming mit HLS, Transkodierung
- Health: Überwachung des Systemzustands
- Terminal: Shell-Zugriff (nur Administratoren)
- Snapshots: Verwaltung von ZFS-Snapshots
- Einstellungen: Konfiguration von Erscheinungsbild und Benachrichtigungen

Antworte auf Deutsch, prägnant und hilfreich. Wenn der Benutzer eine konkrete Aktion verlangt, führe ihn Schritt für Schritt an.
Wenn du die Antwort nicht kennst, sag es ehrlich."#.to_string(),
        "es" => r#"Eres un asistente de IA integrado en HomeVault, un software de gestión de servidores domésticos.

Ayudas al usuario a utilizar HomeVault. Estas son las funciones principales:
- Dashboard: vista general del sistema (CPU, RAM, temperatura, red)
- Explorador de archivos: navegación, búsqueda, subida, descarga, vista previa
- Almacenamiento: discos, particiones, RAID, pools ZFS
- Docker: gestión de contenedores, imágenes, redes, Compose
- Copias de seguridad: creación, restauración, verificación
- Usuarios y grupos: gestión de cuentas y permisos
- Recursos compartidos: FTP, WebDAV con permisos
- Cloud Sync: sincronización con servicios en la nube mediante Rclone
- Syncthing: sincronización entre pares
- Red: monitorización de interfaces
- Media: streaming de vídeo con HLS, transcodificación
- Salud: supervisión del estado del sistema
- Terminal: acceso shell (solo administradores)
- Snapshots: gestión de instantáneas ZFS
- Configuración: apariencia y notificaciones

Responde en español de forma concisa y útil. Si el usuario pide una acción concreta, guíalo paso a paso.
Si no sabes la respuesta, dilo honestamente."#.to_string(),
        _ => r#"You are an AI assistant integrated into HomeVault, home server management software.

You help the user operate HomeVault. Here are the main features:
- Dashboard: system overview (CPU, RAM, temperature, network)
- File Explorer: navigation, search, upload, download, preview
- Storage: disks, partitions, RAID, ZFS pools
- Docker: container, image, network and Compose management
- Backups: creation, restoration, verification
- Users & Groups: account and permission management
- Shares: FTP, WebDAV with permissions
- Cloud Sync: synchronization with cloud services via Rclone
- Syncthing: peer-to-peer synchronization
- Network: interface monitoring
- Media: video streaming with HLS, transcoding
- Health: system health monitoring
- Terminal: shell access (admins only)
- Snapshots: ZFS snapshot management
- Settings: appearance and notification configuration

Reply in English, concisely and helpfully. If the user asks for a specific action, guide them step by step.
If you don't know the answer, say so honestly."#.to_string(),
    }
}

/// Translates the "no/invalid session" chat error into the user's selected
/// UI language, matching `system_prompt`'s own per-language handling instead
/// of always reporting the message in French regardless of `language`.
fn session_error_message(language: &str) -> &'static str {
    match language {
        "fr" => "Session invalide ou manquante",
        "de" => "Ungültige oder fehlende Sitzung",
        "es" => "Sesión inválida o ausente",
        _ => "Invalid or missing session",
    }
}

/// Enrich the system prompt with RAG context from the knowledge base.
/// If no index exists or embedding fails, falls back to the plain prompt.
async fn system_prompt_with_rag(language: &str, message: &str, pool: &sqlx::SqlitePool) -> String {
    let base = system_prompt(language);

    // Try to generate query embedding and search
    let context = match crate::rag::generate_embeddings(&[message.to_string()]).await {
        Ok(embeddings) => {
            if let Some(query_emb) = embeddings.first() {
                match crate::rag::search_similar(pool, query_emb, 5).await {
                    Ok(results) => crate::rag::build_rag_context(&results),
                    Err(_) => String::new(),
                }
            } else {
                String::new()
            }
        }
        Err(_) => String::new(),
    };

    if context.is_empty() {
        base
    } else {
        format!("{}\n{}", base, context)
    }
}

pub async fn handle_chat_send(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ChatRequest>,
) -> Json<ChatResponse> {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let language = req.language.as_deref().unwrap_or("en");

    if session_token.is_empty() || state.auth.get_session(session_token).await.is_err() {
        return Json(ChatResponse {
            success: false,
            data: None,
            error: Some(session_error_message(language).to_string()),
        });
    }

    let model = AI_MODEL.read().clone();
    let conv_id = req
        .conversation_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let mut ollama_messages: Vec<OllamaMessage> = Vec::new();

    ollama_messages.push(OllamaMessage {
        role: "system".to_string(),
        content: system_prompt_with_rag(language, &req.message, &state.db.pool()).await,
    });

    for msg in &req.history {
        ollama_messages.push(OllamaMessage {
            role: msg.role.clone(),
            content: msg.content.clone(),
        });
    }

    ollama_messages.push(OllamaMessage {
        role: "user".to_string(),
        content: req.message.clone(),
    });

    let ollama_req = OllamaChatRequest {
        model: model.clone(),
        messages: ollama_messages,
        stream: false,
    };

    match send_to_ollama(&ollama_req).await {
        Ok(response) => Json(ChatResponse {
            success: true,
            data: Some(ChatResponseData {
                reply: response.message.content,
                conversation_id: conv_id,
                model: response.model,
            }),
            error: None,
        }),
        Err(e) => Json(ChatResponse {
            success: false,
            data: None,
            error: Some(format!("AI communication error: {}", e)),
        }),
    }
}

async fn send_to_ollama(req: &OllamaChatRequest) -> Result<OllamaChatResponse, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .post(format!("{}/api/chat", OLLAMA_BASE))
        .json(req)
        .send()
        .await
        .map_err(|e| format!("{}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Ollama a répondu avec le statut {}: {}",
            status, body
        ));
    }

    response
        .json::<OllamaChatResponse>()
        .await
        .map_err(|e| format!("Erreur de parsing de la réponse Ollama: {}", e))
}

/// Streaming counterpart of `handle_chat_send`: proxies Ollama's
/// newline-delimited-JSON `/api/chat` stream out as SSE `chunk` events (one
/// per incremental token/word) followed by a final `done` event, so the
/// frontend can render the reply as it's generated instead of waiting for
/// the whole thing — a 7B model on CPU can otherwise take 10-30s of silence.
pub async fn handle_chat_send_stream(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ChatRequest>,
) -> Response {
    use axum::response::sse::{Event, Sse};
    use axum::response::IntoResponse;
    use futures_util::StreamExt;
    use std::convert::Infallible;

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    let language = req.language.clone().unwrap_or_else(|| "en".to_string());

    if session_token.is_empty() || state.auth.get_session(&session_token).await.is_err() {
        return ResponseHelper::json_unauthorized(session_error_message(&language).to_string());
    }

    let model = AI_MODEL.read().clone();
    let conv_id = req
        .conversation_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let mut ollama_messages: Vec<OllamaMessage> = Vec::new();
    ollama_messages.push(OllamaMessage {
        role: "system".to_string(),
        content: system_prompt_with_rag(&language, &req.message, &state.db.pool()).await,
    });
    for msg in &req.history {
        ollama_messages.push(OllamaMessage {
            role: msg.role.clone(),
            content: msg.content.clone(),
        });
    }
    ollama_messages.push(OllamaMessage {
        role: "user".to_string(),
        content: req.message.clone(),
    });

    let ollama_req = OllamaChatRequest {
        model,
        messages: ollama_messages,
        stream: true,
    };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
    {
        Ok(c) => c,
        Err(e) => return ResponseHelper::json_error(format!("HTTP client error: {}", e)),
    };

    let ollama_resp = match client
        .post(format!("{}/api/chat", OLLAMA_BASE))
        .json(&ollama_req)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => resp,
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return ResponseHelper::json_error(format!(
                "Ollama responded with status {}: {}",
                status, body
            ));
        }
        Err(e) => return ResponseHelper::json_error(format!("Cannot reach Ollama: {}", e)),
    };

    let stream = async_stream::stream! {
        let mut byte_stream = ollama_resp.bytes_stream();
        let mut buffer = String::new();

        loop {
            let chunk = match byte_stream.next().await {
                Some(Ok(bytes)) => bytes,
                Some(Err(e)) => {
                    yield Ok::<Event, Infallible>(Event::default().event("error").data(e.to_string()));
                    return;
                }
                None => break,
            };
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Ollama streams one JSON object per line (NDJSON), not real SSE.
            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim().to_string();
                buffer.drain(..=pos);
                if line.is_empty() {
                    continue;
                }

                let parsed: OllamaChatResponse = match serde_json::from_str(&line) {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                if !parsed.message.content.is_empty() {
                    let payload = serde_json::json!({ "content": parsed.message.content }).to_string();
                    yield Ok(Event::default().event("chunk").data(payload));
                }

                if parsed.done {
                    let payload = serde_json::json!({
                        "conversation_id": conv_id,
                        "model": parsed.model,
                    }).to_string();
                    yield Ok(Event::default().event("done").data(payload));
                    return;
                }
            }
        }
    };

    Sse::new(stream).into_response()
}

// ── Model management ────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct OllamaModel {
    pub name: String,
    #[serde(default)]
    pub modified_at: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModel>,
}

#[derive(Debug, Serialize)]
pub struct ModelsListResponse {
    pub success: bool,
    pub data: Option<ModelsListData>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ModelsListData {
    pub models: Vec<OllamaModel>,
    pub current: String,
    pub ollama_running: bool,
}

pub async fn handle_chat_models_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Json<ModelsListResponse> {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() || state.auth.get_session(session_token).await.is_err() {
        return Json(ModelsListResponse {
            success: false,
            data: None,
            error: Some("Session invalide ou manquante".to_string()),
        });
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string());

    let client = match client {
        Ok(c) => c,
        Err(e) => {
            return Json(ModelsListResponse {
                success: false,
                data: Some(ModelsListData {
                    models: vec![],
                    current: AI_MODEL.read().clone(),
                    ollama_running: false,
                }),
                error: Some(e),
            })
        }
    };

    let response = client.get(format!("{}/api/tags", OLLAMA_BASE)).send().await;

    match response {
        Ok(resp) if resp.status().is_success() => match resp.json::<OllamaTagsResponse>().await {
            Ok(tags) => Json(ModelsListResponse {
                success: true,
                data: Some(ModelsListData {
                    models: tags.models,
                    current: AI_MODEL.read().clone(),
                    ollama_running: true,
                }),
                error: None,
            }),
            Err(e) => Json(ModelsListResponse {
                success: false,
                data: Some(ModelsListData {
                    models: vec![],
                    current: AI_MODEL.read().clone(),
                    ollama_running: true,
                }),
                error: Some(format!("Erreur de parsing: {}", e)),
            }),
        },
        Ok(_) => Json(ModelsListResponse {
            success: false,
            data: Some(ModelsListData {
                models: vec![],
                current: AI_MODEL.read().clone(),
                ollama_running: false,
            }),
            error: Some("Ollama ne répond pas".to_string()),
        }),
        Err(e) => Json(ModelsListResponse {
            success: false,
            data: Some(ModelsListData {
                models: vec![],
                current: AI_MODEL.read().clone(),
                ollama_running: false,
            }),
            error: Some(format!("Impossible de contacter Ollama: {}", e)),
        }),
    }
}

#[derive(Debug, Deserialize)]
pub struct PullModelRequest {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct PullModelResponse {
    pub success: bool,
    pub message: Option<String>,
    pub error: Option<String>,
}

/// One NDJSON line of Ollama's `POST /api/pull` stream.
#[derive(Deserialize)]
struct OllamaPullLine {
    status: Option<String>,
    total: Option<u64>,
    completed: Option<u64>,
    error: Option<String>,
}

pub async fn handle_chat_models_pull(
    State(_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<PullModelRequest>,
) -> Response {
    use futures_util::StreamExt;

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match _state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let model_name = req.name.trim().to_string();
    if model_name.is_empty() {
        return ResponseHelper::json_error("Nom de modèle requis".to_string());
    }

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
    {
        Ok(c) => c,
        Err(e) => return ResponseHelper::json_error(format!("Erreur client HTTP: {}", e)),
    };

    // Register a progress job so the download shows in the desktop activity
    // taskbar with a real percentage (multi-GB models). We stream Ollama's
    // NDJSON progress but still answer the request synchronously, once done,
    // with the same `{success, message}` envelope the frontend expects.
    let job = crate::sse::PROGRESS_MANAGER.create_job(
        crate::sse::JobType::Custom("ollama_pull".to_string()),
        &format!("Downloading {}", model_name),
    );

    let pull_body = serde_json::json!({
        "name": model_name,
        "stream": true,
    });

    let response = client
        .post(format!("{}/api/pull", OLLAMA_BASE))
        .json(&pull_body)
        .send()
        .await;

    let resp = match response {
        Ok(resp) if resp.status().is_success() => resp,
        Ok(resp) => {
            let body = resp.text().await.unwrap_or_default();
            crate::sse::PROGRESS_MANAGER.fail_job(&job, &body);
            return ResponseHelper::json_error(format!("Échec de l'installation: {}", body));
        }
        Err(e) => {
            crate::sse::PROGRESS_MANAGER.fail_job(&job, &e.to_string());
            return ResponseHelper::json_error(format!("Erreur de connexion à Ollama: {}", e));
        }
    };

    let mut byte_stream = resp.bytes_stream();
    let mut buffer = String::new();
    let mut stream_error: Option<String> = None;

    'outer: while let Some(chunk) = byte_stream.next().await {
        let bytes = match chunk {
            Ok(b) => b,
            Err(e) => {
                stream_error = Some(e.to_string());
                break;
            }
        };
        buffer.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim().to_string();
            buffer.drain(..=pos);
            if line.is_empty() {
                continue;
            }
            let Ok(p) = serde_json::from_str::<OllamaPullLine>(&line) else {
                continue;
            };
            if let Some(err) = p.error {
                stream_error = Some(err);
                break 'outer;
            }
            // Only emit a percentage for the layer-download lines; the
            // "pulling manifest" / "verifying" phases stay indeterminate.
            if let (Some(done), Some(total)) = (p.completed, p.total) {
                if total > 0 {
                    let pct = (done as f64 / total as f64) * 100.0;
                    crate::sse::PROGRESS_MANAGER.emit_progress(
                        &job,
                        pct,
                        p.status.as_deref().unwrap_or("downloading"),
                    );
                }
            }
        }
    }

    if let Some(err) = stream_error {
        crate::sse::PROGRESS_MANAGER.fail_job(&job, &err);
        return ResponseHelper::json_error(format!("Échec de l'installation: {}", err));
    }

    crate::sse::PROGRESS_MANAGER.complete_job(&job, &format!("{} installé", model_name));
    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "message": format!("Modèle '{}' installé avec succès", model_name),
    }))
}

#[derive(Debug, Serialize)]
pub struct ModelGetData {
    pub model: String,
}

pub async fn handle_chat_model_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }
    if state.auth.get_session(session_token).await.is_err() {
        return ResponseHelper::json_unauthorized("Session invalid".to_string());
    }

    let model = AI_MODEL.read().clone();
    ResponseHelper::json_success(ModelGetData { model })
}

#[derive(Debug, Deserialize)]
pub struct ModelSetRequest {
    pub model: String,
}

#[derive(Debug, Serialize)]
pub struct ModelSetResponse {
    pub success: bool,
    pub message: Option<String>,
    pub error: Option<String>,
}

pub async fn handle_chat_model_set(
    State(_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ModelSetRequest>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match _state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let model_name = req.model.trim().to_string();
    if model_name.is_empty() {
        return ResponseHelper::json_error(crate::i18n::t("chat.model_name_required"));
    }

    {
        let mut current = AI_MODEL.write();
        *current = model_name.clone();
    }

    crate::settings_persistence::save_ai_model(&model_name);

    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "message": crate::i18n::t_var("chat.model_changed", "model", &model_name),
    }))
}

// ── Model recommendations ──────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct RecommendResponse {
    pub success: bool,
    pub data: RecommendData,
}

#[derive(Debug, Serialize)]
pub struct RecommendData {
    pub recommendations: Vec<ModelRecommendation>,
    pub system: SystemSpecs,
}

#[derive(Debug, Serialize)]
pub struct ModelRecommendation {
    pub name: &'static str,
    pub description: &'static str,
    pub min_ram_gb: u64,
    pub min_vram_gb: Option<u64>,
    pub requires_gpu: bool,
    pub speed: &'static str,
    pub size_gb: f64,
    /// False when the model needs more memory than this host has: still
    /// listed (the operator may plan a RAM upgrade), never the top pick.
    pub fits: bool,
}

/// How many models the Settings card proposes. Five gives the operator a
/// real choice — the smallest thing that fits this host, a middle step, and
/// the best it can actually run — without turning the card into a catalog.
const RECOMMENDATION_COUNT: usize = 5;

/// Ranking tier of a model on one host: does it fit at all, and how
/// comfortably.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Fit {
    /// Needs more memory than the host has — never proposed.
    TooBig,
    /// Fits in RAM only: usable, CPU inference, slow on a small board.
    CpuOnly,
    /// Fits in GPU VRAM: the preferred case.
    Gpu,
}

#[derive(Debug, Serialize)]
pub struct SystemSpecs {
    pub total_ram_gb: u64,
    pub has_gpu: bool,
    pub gpu_name: Option<String>,
    pub gpu_vram_gb: Option<u64>,
}

/// Every entry exists in the public Ollama library (a name that does not
/// resolve would fail at `ollama pull`, which is the one failure mode this
/// catalog must never ship). Sizes are the published Q4 quantisation, RAM
/// floors are size + ~40 % for the KV cache and the runtime.
const MODEL_CATALOG: &[ModelRecommendation] = &[
    ModelRecommendation {
        name: "qwen3:0.6b",
        description:
            "Le plus petit modèle utile. Pour un Raspberry Pi ou une machine à 1 Go de RAM.",
        min_ram_gb: 1,
        min_vram_gb: None,
        requires_gpu: false,
        speed: "instantané",
        size_gb: 0.4,
        fits: true,
    },
    ModelRecommendation {
        name: "gemma3:1b",
        description:
            "Ultra-léger et multilingue (dont le français). Le bon défaut d'une petite configuration.",
        min_ram_gb: 2,
        min_vram_gb: Some(1),
        requires_gpu: false,
        speed: "très rapide",
        size_gb: 0.8,
        fits: true,
    },
    ModelRecommendation {
        name: "deepseek-r1:1.5b",
        description:
            "Petit modèle de raisonnement : justifie ses réponses étape par étape, utile pour diagnostiquer un incident.",
        min_ram_gb: 2,
        min_vram_gb: Some(1),
        requires_gpu: false,
        speed: "rapide",
        size_gb: 1.1,
        fits: true,
    },
    ModelRecommendation {
        name: "llama3.2:1b",
        description:
            "Généraliste léger et très bien outillé. À l'aise en anglais et sur le code.",
        min_ram_gb: 2,
        min_vram_gb: Some(1),
        requires_gpu: false,
        speed: "très rapide",
        size_gb: 1.3,
        fits: true,
    },
    ModelRecommendation {
        name: "qwen3:1.7b",
        description:
            "Meilleur compromis taille/qualité sous 4 Go de RAM. Bon suivi d'instructions.",
        min_ram_gb: 3,
        min_vram_gb: Some(2),
        requires_gpu: false,
        speed: "rapide",
        size_gb: 1.4,
        fits: true,
    },
    ModelRecommendation {
        name: "llama3.2:3b",
        description:
            "Solide généraliste pour un serveur de 4 Go. Bon en résumé et en explication de commandes.",
        min_ram_gb: 4,
        min_vram_gb: Some(2),
        requires_gpu: false,
        speed: "rapide",
        size_gb: 2.0,
        fits: true,
    },
    ModelRecommendation {
        name: "phi4-mini",
        description:
            "Petit modèle orienté raisonnement et mathématiques, très précis pour sa taille.",
        min_ram_gb: 5,
        min_vram_gb: Some(3),
        requires_gpu: false,
        speed: "moyen",
        size_gb: 2.5,
        fits: true,
    },
    ModelRecommendation {
        name: "qwen3:4b",
        description:
            "Excellent rapport qualité/ressources, multilingue. Le choix par défaut d'un serveur de 6 Go.",
        min_ram_gb: 5,
        min_vram_gb: Some(3),
        requires_gpu: false,
        speed: "moyen",
        size_gb: 2.6,
        fits: true,
    },
    ModelRecommendation {
        name: "gemma3:4b",
        description:
            "Très bon en français et en rédaction. Demande 6 Go de RAM et de la patience sur un CPU.",
        min_ram_gb: 6,
        min_vram_gb: Some(4),
        requires_gpu: false,
        speed: "moyen",
        size_gb: 3.3,
        fits: true,
    },
    ModelRecommendation {
        name: "mistral:7b",
        description:
            "Vétéran fiable et concis. Confortable à partir de 10 Go de RAM, ou sur un GPU 6 Go.",
        min_ram_gb: 10,
        min_vram_gb: Some(6),
        requires_gpu: false,
        speed: "lent (CPU) / rapide (GPU)",
        size_gb: 4.4,
        fits: true,
    },
    ModelRecommendation {
        name: "llama3.1:8b",
        description:
            "Généraliste haut de gamme : le meilleur raisonnement accessible sur un serveur de 12 Go.",
        min_ram_gb: 12,
        min_vram_gb: Some(6),
        requires_gpu: false,
        speed: "lent (CPU) / rapide (GPU)",
        size_gb: 4.9,
        fits: true,
    },
    ModelRecommendation {
        name: "qwen3:8b",
        description:
            "Le plus capable de la sélection. Pour un serveur de 12 Go ou un GPU 8 Go.",
        min_ram_gb: 12,
        min_vram_gb: Some(8),
        requires_gpu: false,
        speed: "lent (CPU) / rapide (GPU)",
        size_gb: 5.2,
        fits: true,
    },
];

fn detect_system_specs() -> SystemSpecs {
    let total_ram_kb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|content| {
            content.lines().find_map(|line| {
                if line.starts_with("MemTotal:") {
                    line.split_whitespace()
                        .nth(1)
                        .and_then(|v| v.parse::<u64>().ok())
                } else {
                    None
                }
            })
        })
        .unwrap_or(0);

    let total_ram_gb = total_ram_kb / (1024 * 1024);

    let (gpu_name, gpu_vram_bytes) = detect_gpu();

    let has_gpu = gpu_name.is_some();
    let gpu_vram_gb = gpu_vram_bytes.map(|b| b / (1024 * 1024 * 1024));

    SystemSpecs {
        total_ram_gb,
        has_gpu,
        gpu_name,
        gpu_vram_gb,
    }
}

fn detect_gpu() -> (Option<String>, Option<u64>) {
    if let Ok(output) = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(first_line) = stdout.lines().next() {
                let parts: Vec<&str> = first_line.split(',').map(|s| s.trim()).collect();
                let name = parts.first().map(|s| s.to_string());
                let vram_mib = parts.get(1).and_then(|s| s.parse::<u64>().ok());
                let vram_bytes = vram_mib.map(|m| m * 1024 * 1024);
                return (name, vram_bytes);
            }
        }
    }

    if let Ok(output) = std::process::Command::new("rocm-smi")
        .args(["--showmeminfo", "vram"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("VRAM Total Memory") {
                    if let Some(val) = line
                        .split(':')
                        .next_back()
                        .and_then(|s| s.trim().parse::<u64>().ok())
                    {
                        return (Some("AMD GPU".to_string()), Some(val));
                    }
                }
            }
        }
    }

    (None, None)
}

/// How well one catalog entry fits this host. Pure, so the whole ranking is
/// testable without touching `/proc` or a GPU.
///
/// A model is proposed first when its memory floor is met — telling an
/// operator to pull a 5 GB model on a 2 GB board is not a recommendation, it
/// is a failed download. GPU fit ranks above CPU fit because Ollama keeps the
/// whole model in VRAM when it can, which is the difference between "answers
/// in seconds" and "answers in minutes" on an appliance.
fn fit_for(model: &ModelRecommendation, specs: &SystemSpecs) -> Fit {
    let ram_ok = specs.total_ram_gb >= model.min_ram_gb;
    let vram_ok = specs.has_gpu
        && model
            .min_vram_gb
            .map(|need| specs.gpu_vram_gb.unwrap_or(0) >= need)
            .unwrap_or(false);

    if vram_ok {
        Fit::Gpu
    } else if ram_ok {
        Fit::CpuOnly
    } else {
        Fit::TooBig
    }
}

/// Ranks `MODEL_CATALOG` for a given host and returns
/// [`RECOMMENDATION_COUNT`] models, best fit first. Pure — separated out so it
/// can be unit-tested without an `AppState`, and so the handler stays a thin
/// auth + `detect_system_specs` + this call.
///
/// The order is a ladder the operator can read: what this host runs best
/// (GPU-fit first, then the largest model that fits in RAM), then progressively
/// smaller and faster choices. When fewer than five fit, the list is completed
/// with the next smallest models that do not, flagged `fits: false` — the
/// operator sees the upgrade path instead of an empty half-card, and the UI
/// never labels one of those "best choice".
fn recommend_models(specs: &SystemSpecs) -> Vec<ModelRecommendation> {
    let mut ranked: Vec<(Fit, &ModelRecommendation)> = MODEL_CATALOG
        .iter()
        .map(|m| (fit_for(m, specs), m))
        .collect();

    ranked.sort_by(|a, b| {
        // Models that fit come first.
        let a_fits = a.0 != Fit::TooBig;
        let b_fits = b.0 != Fit::TooBig;
        b_fits
            .cmp(&a_fits)
            // Among fitting models: GPU fit, then the most capable (largest).
            .then(b.0.cmp(&a.0))
            // Among models that do not fit: the closest to usable first.
            .then_with(|| {
                if a_fits {
                    b.1.min_ram_gb.cmp(&a.1.min_ram_gb)
                } else {
                    a.1.min_ram_gb.cmp(&b.1.min_ram_gb)
                }
            })
            .then(a.1.name.cmp(b.1.name))
    });

    ranked
        .into_iter()
        .take(RECOMMENDATION_COUNT)
        .map(|(fit, m)| ModelRecommendation {
            name: m.name,
            description: m.description,
            min_ram_gb: m.min_ram_gb,
            min_vram_gb: m.min_vram_gb,
            requires_gpu: m.requires_gpu,
            speed: m.speed,
            size_gb: m.size_gb,
            fits: fit != Fit::TooBig,
        })
        .collect()
}

/// Returns model recommendations for this host. Requires a valid session —
/// like every other chat endpoint (2026-07-13 audit). Without the check an
/// unauthenticated caller could read the host's RAM + GPU via this handler.
pub async fn handle_chat_models_recommend(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Json<RecommendResponse> {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() || state.auth.get_session(session_token).await.is_err() {
        return Json(RecommendResponse {
            success: false,
            data: RecommendData {
                recommendations: vec![],
                system: SystemSpecs {
                    total_ram_gb: 0,
                    has_gpu: false,
                    gpu_name: None,
                    gpu_vram_gb: None,
                },
            },
        });
    }

    let specs = detect_system_specs();
    let recommendations = recommend_models(&specs);

    Json(RecommendResponse {
        success: true,
        data: RecommendData {
            recommendations,
            system: specs,
        },
    })
}

#[cfg(test)]
mod tests {
    //! Pure logic/unit tests for the AI Assistant (Chat) handler module.
    //!
    //! Covers the deterministic pieces: the per-language `system_prompt` and
    //! `session_error_message` (the fix for "always answered in French"),
    //! `reset_ai_model`, the `MODEL_CATALOG` invariants + the recommendation
    //! scorer, and the serde mapping of Ollama's REST responses.
    //!
    //! No HTTP, no Ollama process. `AI_MODEL` is a process-global, so tests
    //! that touch it serialise behind a mutex and restore it.
    //!
    //! Run: `cargo test handlers::chat::tests`

    use super::*;
    use std::sync::Mutex;

    static MODEL_LOCK: Mutex<()> = Mutex::new(());

    // -- system_prompt --------------------------------------------------

    #[test]
    fn test_system_prompt_is_localised() {
        assert!(system_prompt("fr").contains("Réponds en français"));
        assert!(system_prompt("de").contains("Antworte auf Deutsch"));
        assert!(system_prompt("es").contains("Responde en español"));
        assert!(system_prompt("en").contains("Reply in English"));
    }

    #[test]
    fn test_system_prompt_unknown_language_falls_back_to_english() {
        assert_eq!(system_prompt("pt"), system_prompt("en"));
        assert_eq!(system_prompt(""), system_prompt("en"));
        assert!(system_prompt("zz").contains("Reply in English"));
    }

    #[test]
    fn test_system_prompt_always_names_the_product_and_features() {
        for lang in ["fr", "de", "es", "en", "xx"] {
            let p = system_prompt(lang);
            assert!(p.contains("HomeVault"), "{lang}");
            assert!(p.to_lowercase().contains("docker"), "{lang}");
        }
    }

    // -- session_error_message ------------------------------------

    #[test]
    fn test_session_error_message_is_localised() {
        assert_eq!(session_error_message("fr"), "Session invalide ou manquante");
        assert_eq!(
            session_error_message("de"),
            "Ungültige oder fehlende Sitzung"
        );
        assert_eq!(session_error_message("es"), "Sesión inválida o ausente");
        assert_eq!(session_error_message("en"), "Invalid or missing session");
        assert_eq!(session_error_message("ja"), "Invalid or missing session");
    }

    // -- reset_ai_model -----------------------------------------

    #[test]
    fn test_reset_ai_model_restores_the_default() {
        let _guard = MODEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved = AI_MODEL.read().clone();
        *AI_MODEL.write() = "some-custom-model:70b".to_string();
        reset_ai_model();
        assert_eq!(*AI_MODEL.read(), DEFAULT_MODEL);
        assert_eq!(DEFAULT_MODEL, "gemma3:1b");
        *AI_MODEL.write() = saved;
    }

    // -- MODEL_CATALOG + recommendation scorer -----------------

    #[test]
    fn test_model_catalog_invariants() {
        assert!(!MODEL_CATALOG.is_empty());
        for m in MODEL_CATALOG {
            assert!(!m.name.is_empty());
            assert!(!m.description.is_empty());
            assert!(m.min_ram_gb >= 1, "{}", m.name);
            assert!(m.size_gb > 0.0, "{}", m.name);
            assert!(!m.speed.is_empty(), "{}", m.name);
        }
    }

    #[test]
    fn test_recommend_models_returns_five_real_catalog_entries() {
        // A 4 GB host: enough for the light half of the catalog.
        let specs = SystemSpecs {
            total_ram_gb: 4,
            has_gpu: false,
            gpu_name: None,
            gpu_vram_gb: None,
        };
        let recs = recommend_models(&specs);
        assert_eq!(recs.len(), RECOMMENDATION_COUNT);
        for r in &recs {
            assert!(
                MODEL_CATALOG.iter().any(|m| m.name == r.name),
                "{} is not in the catalog",
                r.name
            );
        }
    }

    #[test]
    fn test_recommend_models_never_puts_a_model_that_does_not_fit_first() {
        // 2 GB host: only the <=2 GB entries fit, so the top pick must be one
        // of them and every `fits: false` entry must rank after the fitting
        // ones.
        let specs = SystemSpecs {
            total_ram_gb: 2,
            has_gpu: false,
            gpu_name: None,
            gpu_vram_gb: None,
        };
        let recs = recommend_models(&specs);
        assert_eq!(recs.len(), RECOMMENDATION_COUNT);
        assert!(recs[0].fits, "the best choice must fit this host");
        assert!(recs[0].min_ram_gb <= 2);

        let first_too_big = recs.iter().position(|r| !r.fits).unwrap_or(recs.len());
        for r in &recs[first_too_big..] {
            assert!(!r.fits, "a fitting model cannot rank after a too-big one");
        }
    }

    #[test]
    fn test_recommend_models_always_returns_five_even_on_a_tiny_host() {
        // 1 GB: only the smallest entry fits, the four others are the upgrade
        // path, flagged. The card must never be half empty.
        let specs = SystemSpecs {
            total_ram_gb: 1,
            has_gpu: false,
            gpu_name: None,
            gpu_vram_gb: None,
        };
        let recs = recommend_models(&specs);
        assert_eq!(recs.len(), RECOMMENDATION_COUNT);
        assert!(recs[0].fits);
        assert_eq!(recs[0].min_ram_gb, 1);
        // The non-fitting ones are ordered from the closest to usable.
        let too_big: Vec<u64> = recs.iter().filter(|r| !r.fits).map(|r| r.min_ram_gb).collect();
        let mut sorted = too_big.clone();
        sorted.sort_unstable();
        assert_eq!(too_big, sorted);
    }

    #[test]
    fn test_recommend_models_prefers_vram_fit() {
        // A 24 GB GPU with 8 GB of RAM: the GPU decides, so the top pick is a
        // model that needs more RAM than the host has but fits in VRAM.
        let specs = SystemSpecs {
            total_ram_gb: 8,
            has_gpu: true,
            gpu_name: Some("RTX 4090".into()),
            gpu_vram_gb: Some(24),
        };
        let recs = recommend_models(&specs);
        assert_eq!(recs.len(), RECOMMENDATION_COUNT);
        assert!(recs[0].fits);
        assert!(
            recs[0].min_vram_gb.unwrap_or(0) <= 24,
            "the top pick must fit in VRAM"
        );
    }

    #[test]
    fn test_catalog_names_are_unique_and_sized() {
        // Guards the catalog itself: a duplicated name would show the same
        // model twice in the card, and a zero size would break the size chip.
        for (i, m) in MODEL_CATALOG.iter().enumerate() {
            assert!(!m.name.is_empty());
            assert!(m.size_gb > 0.0, "{} has no size", m.name);
            assert!(m.min_ram_gb > 0, "{} has no RAM floor", m.name);
            assert!(
                !MODEL_CATALOG[..i].iter().any(|o| o.name == m.name),
                "{} appears twice",
                m.name
            );
        }
    }

    fn test_recommend_models_prefers_bigger_models_on_a_beefy_host() {
        let big = SystemSpecs {
            total_ram_gb: 64,
            has_gpu: true,
            gpu_name: Some("RTX 4090".into()),
            gpu_vram_gb: Some(24),
        };
        let small = SystemSpecs {
            total_ram_gb: 2,
            has_gpu: false,
            gpu_name: None,
            gpu_vram_gb: None,
        };
        let big_top = recommend_models(&big)[0].min_ram_gb;
        let small_top = recommend_models(&small)[0].min_ram_gb;
        assert!(
            big_top >= small_top,
            "a 64GB+GPU host should not be told to run a smaller model than a 2GB host"
        );
    }

    #[test]
    fn test_detect_system_specs_reads_real_ram() {
        // /proc/meminfo exists on any Linux test host.
        assert!(detect_system_specs().total_ram_gb >= 1);
    }

    // -- Ollama response deserialization ----------------------

    #[test]
    fn test_ollama_chat_response_parses() {
        let raw = r#"{
          "model":"gemma3:1b",
          "created_at":"2026-09-06T12:00:00Z",
          "message":{"role":"assistant","content":"Bonjour !"},
          "done":true,
          "total_duration":123456
        }"#;
        let r: OllamaChatResponse = serde_json::from_str(raw).expect("must parse");
        assert_eq!(r.message.content, "Bonjour !");
        assert_eq!(r.model, "gemma3:1b");
        assert!(r.done);
    }

    #[test]
    fn test_ollama_tags_response_parses_a_model_list() {
        let raw = r#"{
          "models":[
            {"name":"gemma3:1b","modified_at":"2026-09-01T00:00:00Z","size":815319791,
             "digest":"abc","details":{"family":"gemma3"}},
            {"name":"llama3.2:3b","modified_at":"2026-08-01T00:00:00Z","size":2019393792}
          ]
        }"#;
        let r: OllamaTagsResponse = serde_json::from_str(raw).expect("must parse");
        assert_eq!(r.models.len(), 2);
        assert_eq!(r.models[0].name, "gemma3:1b");
        assert_eq!(r.models[0].size, 815319791);
        // `modified_at`/`size` are #[serde(default)] — a sparse entry still parses
        let sparse: OllamaModel = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        assert_eq!(sparse.name, "x");
        assert_eq!(sparse.size, 0);
    }

    // -- request bodies ---------------------------------------

    #[test]
    fn test_chat_request_optional_fields_default() {
        let req: ChatRequest = serde_json::from_str(r#"{"message":"hi"}"#).unwrap();
        assert_eq!(req.message, "hi");
        assert!(req.conversation_id.is_none());
        assert!(req.history.is_empty());
        assert!(req.language.is_none());

        let full: ChatRequest = serde_json::from_str(
            r#"{"message":"hi","conversation_id":"c1","language":"fr",
                "history":[{"role":"user","content":"a"},{"role":"assistant","content":"b"}]}"#,
        )
        .unwrap();
        assert_eq!(full.history.len(), 2);
        assert_eq!(full.language.as_deref(), Some("fr"));
    }

    #[test]
    fn test_pull_and_set_model_request_bodies() {
        let p: PullModelRequest = serde_json::from_str(r#"{"name":"mistral:7b"}"#).unwrap();
        assert_eq!(p.name, "mistral:7b");
        let s: ModelSetRequest = serde_json::from_str(r#"{"model":"gemma3:4b"}"#).unwrap();
        assert_eq!(s.model, "gemma3:4b");
    }
}
