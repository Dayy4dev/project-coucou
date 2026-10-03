// Claude API client — the same integration as ClaudeService.swift: multi-turn
// chat with web search, and files sent as document/image/text blocks.
//
// Everything happens here rather than in the island: the API key never leaves
// the Credential Manager, and file bytes never cross the IPC boundary.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::secrets;

const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Server-side fallback: on a policy decline the API retries the same request on
/// a fallback model inside the same call, so the island never shows a dead end.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MAX_TOKENS: u32 = 4096;
/// Text and code files are inlined; anything larger is skipped, as on macOS.
const MAX_INLINE_TEXT: u64 = 200_000;

pub const DEFAULT_MODEL: &str = "claude-opus-5";
/// Where an OpenAI-compatible provider is reached when the base URL is left
/// empty. Anything that speaks `POST /chat/completions` fits: OpenAI, OpenRouter,
/// Groq, Together, DeepSeek, Mistral, LM Studio, Ollama, vLLM, …
pub const DEFAULT_OPENAI_BASE: &str = "https://api.openai.com/v1";
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-4o";
/// The endpoint that lists models, relative to the provider's base URL.
const OPENAI_MODELS_TAIL: &str = "models";
const OPENAI_CHAT_TAIL: &str = "chat/completions";
const ANTHROPIC_MODELS_URL: &str = "https://api.anthropic.com/v1/models?limit=100";

const SYSTEM_PROMPT: &str = "You are Mochi, a personal AI assistant living at the top of the user's screen. \
You have web search access and can help with absolutely anything — research, coding, finding places, recommendations, tasks, questions. \
Respond in the user's language. Be thorough and complete — use as much detail as the task requires. \
No markdown formatting (no **, no ##, no bullet dashes). Use plain text with line breaks.";

#[derive(Default)]
pub struct Chat {
    /// Full multi-turn history, including tool_use / tool_result blocks.
    messages: Mutex<Vec<Value>>,
}

impl Chat {
    pub fn reset(&self) {
        self.messages.lock().unwrap().clear();
    }

    fn is_empty(&self) -> bool {
        self.messages.lock().unwrap().is_empty()
    }

    fn push(&self, message: Value) {
        self.messages.lock().unwrap().push(message);
    }

    fn pop(&self) {
        self.messages.lock().unwrap().pop();
    }

    fn snapshot(&self) -> Vec<Value> {
        self.messages.lock().unwrap().clone()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChatContext {
    File {
        name: String,
        path: String,
    },
    Window {
        app_name: String,
        title: String,
        url: Option<String>,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReply {
    pub text: String,
}

/// Which backend the chat talks to. Stored in settings.json as "anthropic" or
/// "openai" — the latter meaning *any* OpenAI-compatible endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Anthropic,
    Openai,
}

/// Everything a turn needs to reach the provider, assembled from Settings so
/// the model/base URL can change between turns without a restart.
#[derive(Debug, Clone)]
pub struct ChatConfig {
    pub provider: Provider,
    pub model: String,
    /// Only meaningful for `Provider::Openai`. Empty falls back to OpenAI itself.
    pub base_url: String,
}

/// A model as the picker shows it. `label` is the human name when the API
/// provides one, otherwise the id.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
}

/// One chat turn, routed to whichever provider the settings select.
pub async fn send(
    chat: &Chat,
    config: &ChatConfig,
    query: String,
    context: Option<ChatContext>,
) -> Result<ChatReply, String> {
    match config.provider {
        Provider::Anthropic => send_anthropic(chat, config, query, context).await,
        Provider::Openai => send_openai(chat, config, query, context).await,
    }
}

/// Anthropic Messages API: multi-turn, web search, files as document/image/text
/// blocks. The history is kept in Anthropic block form, which is also the
/// neutral shape the OpenAI path converts *from*.
async fn send_anthropic(
    chat: &Chat,
    config: &ChatConfig,
    query: String,
    context: Option<ChatContext>,
) -> Result<ChatReply, String> {
    let key = secrets::get("anthropic-api-key")
        .ok_or_else(|| "Anthropic API key missing. Open settings.".to_string())?;

    let mut content: Vec<Value> = Vec::new();

    // File / window context rides along with the first message only, exactly
    // like ClaudeService.chat().
    if chat.is_empty() {
        match &context {
            Some(ChatContext::File { name, path }) => {
                if let Some(block) = file_block(path) {
                    content.push(block);
                }
                content.push(json!({ "type": "text", "text": format!("File: {name}") }));
            }
            Some(ChatContext::Window {
                app_name,
                title,
                url,
            }) => {
                let mut text = format!("Context — App: {app_name}, Window: {title}");
                if let Some(url) = url {
                    text.push_str(&format!(", URL: {url}"));
                }
                content.push(json!({ "type": "text", "text": text }));
            }
            None => {}
        }
    }
    content.push(json!({ "type": "text", "text": query }));

    chat.push(json!({ "role": "user", "content": content }));

    let body = json!({
        "model": config.model,
        "max_tokens": MAX_TOKENS,
        "system": SYSTEM_PROMPT,
        "tools": [{ "type": "web_search_20260209", "name": "web_search", "max_uses": 5 }],
        "fallbacks": "default",
        "messages": chat.snapshot(),
    });

    let response = match call(&key, &body).await {
        Ok(v) => v,
        Err(err) => {
            chat.pop(); // keep the history consistent with what the model saw
            return Err(err);
        }
    };

    // A policy decline comes back as HTTP 200 with stop_reason "refusal".
    if response.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
        chat.pop();
        let why = response
            .get("stop_details")
            .and_then(|d| d.get("explanation"))
            .and_then(Value::as_str)
            .unwrap_or("Claude declined this one.");
        return Err(why.to_string());
    }

    let Some(blocks) = response.get("content").and_then(Value::as_array).cloned() else {
        chat.pop();
        return Err("Unexpected API response.".into());
    };

    // Store the whole content — tool_use / tool_result blocks included — so the
    // next turn has the right context.
    chat.push(json!({ "role": "assistant", "content": blocks.clone() }));

    let text = text_of(&blocks);
    if text.is_empty() {
        return Err("No response text.".into());
    }
    Ok(ChatReply { text })
}

/// OpenAI-compatible Chat Completions: `POST {base}/chat/completions` with a
/// bearer token. No tools — a compatible server may not offer web search, and
/// silently dropping a requested tool is worse than not asking for one.
async fn send_openai(
    chat: &Chat,
    config: &ChatConfig,
    query: String,
    context: Option<ChatContext>,
) -> Result<ChatReply, String> {
    // A local server (Ollama, LM Studio, vLLM) wants no Authorization header at
    // all, so a missing entry is not an error here the way it is for Anthropic:
    // no key simply means "send no bearer token". Cloud endpoints reject that
    // with a 401, which the response below reports with the provider's wording.
    let key = secrets::get("openai-api-key").unwrap_or_default();
    let key = key.trim();

    let mut parts: Vec<Value> = Vec::new();
    if chat.is_empty() {
        match &context {
            Some(ChatContext::File { name, path }) => {
                if let Some(block) = file_block(path) {
                    parts.push(block);
                }
                parts.push(json!({ "type": "text", "text": format!("File: {name}") }));
            }
            Some(ChatContext::Window {
                app_name,
                title,
                url,
            }) => {
                let mut text = format!("Context — App: {app_name}, Window: {title}");
                if let Some(url) = url {
                    text.push_str(&format!(", URL: {url}"));
                }
                parts.push(json!({ "type": "text", "text": text }));
            }
            None => {}
        }
    }
    parts.push(json!({ "type": "text", "text": query }));

    chat.push(json!({ "role": "user", "content": parts }));

    let messages = to_openai_messages(&chat.snapshot());

    let body = json!({
        "model": config.model,
        "max_tokens": MAX_TOKENS,
        "messages": messages,
    });

    let url = format!("{}/{}", base_url(config), OPENAI_CHAT_TAIL);
    let response = match call_openai(&url, key, &body).await {
        Ok(v) => v,
        Err(err) => {
            chat.pop();
            return Err(err);
        }
    };

    let text = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    if text.is_empty() {
        chat.pop();
        return Err("No response text.".into());
    }

    chat.push(json!({ "role": "assistant", "content": [{ "type": "text", "text": text }] }));
    Ok(ChatReply { text })
}

/// The provider's base URL, without a trailing slash, defaulting to OpenAI.
fn base_url(config: &ChatConfig) -> String {
    let raw = config.base_url.trim();
    let raw = if raw.is_empty() {
        DEFAULT_OPENAI_BASE
    } else {
        raw
    };
    raw.trim_end_matches('/').to_string()
}

/// Pull the visible text out of a block array (shared by both providers).
fn text_of(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Neutral history (Anthropic blocks) → OpenAI messages. Text blocks are joined
/// into one string; images become `image_url` data URLs so vision models see
/// them; documents (PDFs) are dropped — the `File:` text block already names the
/// file, and not every compatible server accepts a PDF part.
fn to_openai_messages(history: &[Value]) -> Vec<Value> {
    let mut out = vec![json!({ "role": "system", "content": SYSTEM_PROMPT })];
    for message in history {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("user");
        let content = message.get("content");
        match content.and_then(Value::as_array) {
            Some(blocks) => {
                let mut texts: Vec<String> = Vec::new();
                let mut images: Vec<Value> = Vec::new();
                for b in blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(t) = b.get("text").and_then(Value::as_str) {
                                texts.push(t.to_string());
                            }
                        }
                        Some("image") => {
                            if let Some(src) = b.get("source") {
                                let media = src
                                    .get("media_type")
                                    .and_then(Value::as_str)
                                    .unwrap_or("image/png");
                                let data = src.get("data").and_then(Value::as_str).unwrap_or("");
                                images.push(json!({
                                    "type": "image_url",
                                    "image_url": { "url": format!("data:{media};base64,{data}") },
                                }));
                            }
                        }
                        _ => {}
                    }
                }
                let text = texts.join("\n\n");
                if images.is_empty() {
                    out.push(json!({ "role": role, "content": text }));
                } else {
                    let mut parts: Vec<Value> = vec![json!({ "type": "text", "text": text })];
                    parts.extend(images);
                    out.push(json!({ "role": role, "content": parts }));
                }
            }
            None => {
                let text = content.and_then(Value::as_str).unwrap_or("").to_string();
                out.push(json!({ "role": role, "content": text }));
            }
        }
    }
    out
}

async fn call(key: &str, body: &Value) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .post(ENDPOINT)
        .header("x-api-key", key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("anthropic-beta", FALLBACK_BETA)
        .header("content-type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        // Surface the API's own message, which is what makes a bad key obvious.
        return Err(format!("Claude API {status}: {}", error_detail(&text)));
    }
    serde_json::from_str(&text).map_err(|e| format!("Bad API response: {e}"))
}

/// `POST {url}` with an optional bearer token — an empty key means "no
/// Authorization header", which is what local servers expect.
async fn call_openai(url: &str, key: &str, body: &Value) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| e.to_string())?;

    let mut request = client
        .post(url)
        .header("content-type", "application/json")
        .json(body);
    if !key.trim().is_empty() {
        request = request.bearer_auth(key.trim());
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("Provider API {status}: {}", error_detail(&text)));
    }
    serde_json::from_str(&text).map_err(|e| format!("Bad API response: {e}"))
}

/// The provider's own `error.message`, else the first 200 characters of the body.
fn error_detail(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| text.chars().take(200).collect())
}

/// Lists the models a provider offers, for the picker in Settings. Anthropic
/// returns display names; OpenAI-compatible servers usually return bare ids.
/// A missing key or a failed request comes back as an error message the
/// settings window can show next to the field it belongs to.
pub async fn fetch_models(config: &ChatConfig) -> Result<Vec<ModelInfo>, String> {
    match config.provider {
        Provider::Anthropic => fetch_anthropic_models().await,
        Provider::Openai => {
            let key = secrets::get("openai-api-key").unwrap_or_default();
            let url = format!("{}/{}", base_url(config), OPENAI_MODELS_TAIL);
            fetch_openai_models(&url, &key).await
        }
    }
}

async fn fetch_anthropic_models() -> Result<Vec<ModelInfo>, String> {
    let key = secrets::get("anthropic-api-key")
        .ok_or_else(|| "Anthropic API key missing. Open settings.".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(ANTHROPIC_MODELS_URL)
        .header("x-api-key", key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("Claude API {status}: {}", error_detail(&text)));
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("Bad API response: {e}"))?;
    let items = value
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(items
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_str)?.to_string();
            let label = item
                .get("display_name")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_string();
            Some(ModelInfo { id, label })
        })
        .collect())
}

async fn fetch_openai_models(url: &str, key: &str) -> Result<Vec<ModelInfo>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let mut request = client.get(url);
    if !key.trim().is_empty() {
        request = request.bearer_auth(key.trim());
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("Provider API {status}: {}", error_detail(&text)));
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("Bad API response: {e}"))?;
    let items = value
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // Drop the families a chat picker can never offer: embeddings, audio,
    // transcription, image/video generation, moderation.
    const EXCLUDED: &[&str] = &[
        "embed", "tts", "stt", "whisper", "dall-e", "audio", "realtime", "moderat", "image",
        "sora", "video", "transcribe",
    ];
    let mut models: Vec<(i64, ModelInfo)> = items
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_str)?.to_string();
            let lower = id.to_lowercase();
            if EXCLUDED.iter().any(|x| lower.contains(x)) {
                return None;
            }
            let created = item.get("created").and_then(Value::as_i64).unwrap_or(0);
            Some((
                created,
                ModelInfo {
                    label: id.clone(),
                    id,
                },
            ))
        })
        .collect();
    // Newest first, like macOS — but a server that reports no timestamps keeps
    // its own order, which is usually curated.
    if models.iter().any(|(c, _)| *c > 0) {
        models.sort_by(|a, b| b.0.cmp(&a.0));
    }
    Ok(models.into_iter().map(|(_, m)| m).collect())
}

/// PDF → document block, image → image block, text/code → inline text.
/// Mirrors readFileAsBlock() in ClaudeService.swift.
fn file_block(path: &str) -> Option<Value> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let media_type = match ext.as_str() {
        "pdf" => Some(("document", "application/pdf")),
        "jpg" | "jpeg" => Some(("image", "image/jpeg")),
        "png" => Some(("image", "image/png")),
        "gif" => Some(("image", "image/gif")),
        "webp" => Some(("image", "image/webp")),
        _ => None,
    };

    if let Some((block_type, media)) = media_type {
        let bytes = std::fs::read(path).ok()?;
        return Some(json!({
            "type": block_type,
            "source": { "type": "base64", "media_type": media, "data": base64(&bytes) },
        }));
    }

    let len = std::fs::metadata(path).ok()?.len();
    if len > MAX_INLINE_TEXT {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    Some(json!({ "type": "text", "text": format!("File contents:\n{text}") }))
}

/// Small standalone base64 encoder — not worth another dependency.
/// Also used for Stripe's basic auth.
pub(crate) fn base64_for(bytes: &[u8]) -> String {
    base64(bytes)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
