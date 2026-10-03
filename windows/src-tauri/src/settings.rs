// Preferences, stored as plain JSON in settings.json under platform::config_dir().
// No secret ever lands here — API keys live in the OS keychain (see secrets.rs).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub sound_enabled: bool,
    pub sound_volume: f64,
    pub auto_close_interval: f64,
    pub absence_interval: f64,
    pub active_integrations: Vec<String>,
    /// "primary" = the main display, "cursor" = whichever display the mouse is on.
    pub screen: String,
    pub autostart: bool,
    pub hooks_installed: bool,
    /// Claude model used by the chat. Changeable in the settings window.
    /// Defaulted explicitly so a settings.json written by an older build still loads.
    #[serde(default = "default_model")]
    pub model: String,
    /// Which backend answers chat: "anthropic" (default) or "openai" (compatible).
    #[serde(default = "default_chat_provider")]
    pub chat_provider: String,
    /// Base URL for `chat_provider == "openai"`. Empty falls back to OpenAI itself.
    #[serde(default = "default_openai_base_url")]
    pub openai_base_url: String,
    /// Model name passed to an OpenAI-compatible endpoint.
    #[serde(default = "default_openai_model")]
    pub openai_model: String,
    /// Global keyboard shortcut that opens/closes the island. Off by default:
    /// a global hotkey is a machine-wide claim, and one nobody asked for can
    /// collide with another app (or with Claude Code's own keys).
    #[serde(default = "default_hotkey_enabled")]
    pub hotkey_enabled: bool,
    /// "Ctrl+Shift+C", "Alt+C", "Ctrl+Shift+Space", … Parsed by the OS layer.
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    /// What a right-click on the island does: "toggle", "hide" or "none".
    #[serde(default = "default_right_click_action")]
    pub right_click_action: String,
    /// What a middle-click on the island does: "toggle", "hide" or "none".
    #[serde(default = "default_middle_click_action")]
    pub middle_click_action: String,
}

fn default_model() -> String {
    crate::claude::DEFAULT_MODEL.to_string()
}

fn default_chat_provider() -> String {
    "anthropic".to_string()
}

fn default_openai_base_url() -> String {
    crate::claude::DEFAULT_OPENAI_BASE.to_string()
}

fn default_openai_model() -> String {
    crate::claude::DEFAULT_OPENAI_MODEL.to_string()
}

fn default_hotkey_enabled() -> bool {
    false
}

fn default_hotkey() -> String {
    "Ctrl+Shift+C".to_string()
}

fn default_right_click_action() -> String {
    "toggle".to_string()
}

fn default_middle_click_action() -> String {
    "hide".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sound_enabled: true,
            sound_volume: 0.12,
            auto_close_interval: 15.0,
            absence_interval: 180.0,
            active_integrations: vec![
                "integration_resend".into(),
                "integration_n8n".into(),
                "integration_vercel".into(),
                "integration_github".into(),
            ],
            screen: "primary".into(),
            autostart: false,
            hooks_installed: false,
            model: default_model(),
            chat_provider: default_chat_provider(),
            openai_base_url: default_openai_base_url(),
            openai_model: default_openai_model(),
            hotkey_enabled: default_hotkey_enabled(),
            hotkey: default_hotkey(),
            right_click_action: default_right_click_action(),
            middle_click_action: default_middle_click_action(),
        }
    }
}

pub use crate::platform::{config_dir, local_dir};

pub fn hook_exe_path() -> PathBuf {
    local_dir().join("bin").join(crate::platform::HOOK_EXE)
}

fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn load() -> Settings {
    match std::fs::read(settings_path()) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

pub fn save(settings: &Settings) -> std::io::Result<()> {
    let dir = config_dir();
    crate::platform::ensure_private_dir(&dir)?;
    let json = serde_json::to_vec_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(settings_path(), json)
}
