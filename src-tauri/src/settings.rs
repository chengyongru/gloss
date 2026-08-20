use crate::{
    app_state::{AppSettings, AppState, DEFAULT_SHORTCUT},
    network, overlay, sessions,
};
use std::fs;
use tauri::{AppHandle, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

pub fn load(data_dir: &std::path::Path) -> Result<AppSettings, String> {
    let path = data_dir.join("settings.json");
    match fs::read_to_string(path) {
        Ok(value) => serde_json::from_str(&value)
            .map_err(|error| format!("The saved settings are damaged: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(AppSettings::default()),
        Err(error) => Err(format!("Could not read settings: {error}")),
    }
}

pub fn activate_shortcut(app: &AppHandle, shortcut: &str) -> Result<(), String> {
    app.global_shortcut()
        .unregister_all()
        .map_err(|error| format!("Could not update the shortcut: {error}"))?;
    app.global_shortcut()
        .on_shortcut(shortcut, |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                overlay::handle_global_shortcut(app);
            }
        })
        .map_err(|error| format!("{shortcut} is unavailable: {error}"))
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    state
        .settings
        .lock()
        .map(|settings| settings.clone())
        .map_err(|_| "The settings state is unavailable.".to_owned())
}

#[tauri::command]
pub fn update_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    mut settings: AppSettings,
) -> Result<AppSettings, String> {
    if settings.shortcut.trim().is_empty() {
        return Err("Choose at least one modifier and a key.".to_owned());
    }
    settings.proxy_url = network::normalize_proxy_url(&settings.proxy_url)?;
    let old = state
        .settings
        .lock()
        .map_err(|_| "The settings state is unavailable.".to_owned())?
        .clone();
    if settings.shortcut != old.shortcut
        && let Err(message) = activate_shortcut(&app, &settings.shortcut)
    {
        let _ = activate_shortcut(&app, &old.shortcut);
        return Err(message);
    }
    let payload = serde_json::to_vec_pretty(&settings)
        .map_err(|error| format!("Could not encode settings: {error}"))?;
    if let Err(message) = sessions::atomic_write(&state.data_dir()?.join("settings.json"), &payload)
    {
        if settings.shortcut != old.shortcut {
            let _ = activate_shortcut(&app, &old.shortcut);
        }
        return Err(message);
    }
    *state
        .settings
        .lock()
        .map_err(|_| "The settings state is unavailable.".to_owned())? = settings.clone();
    Ok(settings)
}

pub fn load_and_activate(app: &AppHandle, state: &AppState) {
    let Ok(data_dir) = state.data_dir() else {
        return;
    };
    let mut active = load(&data_dir).unwrap_or_default();
    if active.shortcut != DEFAULT_SHORTCUT && activate_shortcut(app, &active.shortcut).is_err() {
        let _ = activate_shortcut(app, DEFAULT_SHORTCUT);
        active.shortcut = DEFAULT_SHORTCUT.to_owned();
    }
    if let Ok(mut current) = state.settings.lock() {
        *current = active;
    }
}

#[cfg(test)]
mod tests {
    use crate::app_state::AppSettings;

    #[test]
    fn legacy_settings_fields_are_ignored() {
        let settings: AppSettings = serde_json::from_str(
            r#"{"shortcut":"ctrl+alt+shift+t","saveHistory":false,"theme":"system"}"#,
        )
        .unwrap();
        assert!(settings.proxy_url.is_empty());
        assert!(
            !serde_json::to_string(&settings)
                .unwrap()
                .contains("saveHistory")
        );
    }
}
