use crate::selection::SelectionCapture;
use crate::sessions::ConversationSession;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf, sync::Mutex};

pub const DEFAULT_SHORTCUT: &str = "ctrl+alt+shift+t";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub shortcut: String,
    pub theme: ThemePreference,
    #[serde(default)]
    pub proxy_url: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            shortcut: DEFAULT_SHORTCUT.to_owned(),
            theme: ThemePreference::System,
            proxy_url: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum OverlayState {
    #[default]
    Idle,
    Ready {
        selection: SelectionCapture,
    },
    CaptureError {
        message: String,
    },
}

pub struct AppState {
    pub overlay: Mutex<OverlayState>,
    pub settings: Mutex<AppSettings>,
    pub data_dir: Mutex<Option<PathBuf>>,
    pub sessions: tokio::sync::Mutex<HashMap<String, ConversationSession>>,
    pub active_session_id: Mutex<Option<String>>,
    pub auth_lock: tokio::sync::Mutex<()>,
    pub profile_lock: tokio::sync::Mutex<()>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            overlay: Mutex::new(OverlayState::default()),
            settings: Mutex::new(AppSettings::default()),
            data_dir: Mutex::new(None),
            sessions: tokio::sync::Mutex::new(HashMap::new()),
            active_session_id: Mutex::new(None),
            auth_lock: tokio::sync::Mutex::new(()),
            profile_lock: tokio::sync::Mutex::new(()),
        }
    }
}

impl AppState {
    pub fn overlay_snapshot(&self) -> Result<OverlayState, String> {
        self.overlay
            .lock()
            .map(|state| state.clone())
            .map_err(|_| "The overlay state is unavailable.".to_owned())
    }

    pub fn replace_overlay(&self, next: OverlayState) -> Result<(), String> {
        *self
            .overlay
            .lock()
            .map_err(|_| "The overlay state is unavailable.".to_owned())? = next;
        Ok(())
    }

    pub fn set_data_dir(&self, path: PathBuf) -> Result<(), String> {
        *self
            .data_dir
            .lock()
            .map_err(|_| "The data directory state is unavailable.".to_owned())? = Some(path);
        Ok(())
    }

    pub fn data_dir(&self) -> Result<PathBuf, String> {
        self.data_dir
            .lock()
            .map_err(|_| "The data directory state is unavailable.".to_owned())?
            .clone()
            .ok_or_else(|| "The Gloss data directory is not ready.".to_owned())
    }
}
