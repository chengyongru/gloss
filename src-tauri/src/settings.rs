use crate::{
    app_state::{AppSettings, AppState, DEFAULT_SHORTCUT},
    network, overlay, sessions,
};
use std::fs;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;
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

fn set_launch_at_startup(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|error| format!("Could not update launch at startup: {error}"))
}

#[tauri::command]
pub fn get_settings(app: AppHandle, state: State<'_, AppState>) -> Result<AppSettings, String> {
    let enabled = launch_at_startup_enabled(&app)?;
    let mut settings = state
        .settings
        .lock()
        .map_err(|_| "The settings state is unavailable.".to_owned())?;
    settings.launch_at_startup = enabled;
    Ok(settings.clone())
}

fn launch_at_startup_enabled(app: &AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|error| format!("Could not read launch at startup: {error}"))
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
    let mut old = state
        .settings
        .lock()
        .map_err(|_| "The settings state is unavailable.".to_owned())?
        .clone();
    old.launch_at_startup = launch_at_startup_enabled(&app)?;
    if settings.shortcut != old.shortcut
        && let Err(message) = activate_shortcut(&app, &settings.shortcut)
    {
        let _ = activate_shortcut(&app, &old.shortcut);
        return Err(message);
    }
    let startup_changed = settings.launch_at_startup != old.launch_at_startup;
    if startup_changed && let Err(message) = set_launch_at_startup(&app, settings.launch_at_startup)
    {
        if settings.shortcut != old.shortcut {
            let _ = activate_shortcut(&app, &old.shortcut);
        }
        return Err(message);
    }
    let payload = serde_json::to_vec_pretty(&settings)
        .map_err(|error| format!("Could not encode settings: {error}"))?;
    if let Err(message) = sessions::atomic_write(&state.data_dir()?.join("settings.json"), &payload)
    {
        if settings.shortcut != old.shortcut {
            let _ = activate_shortcut(&app, &old.shortcut);
        }
        if startup_changed {
            let _ = set_launch_at_startup(&app, old.launch_at_startup);
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
    if let Ok(enabled) = app.autolaunch().is_enabled() {
        active.launch_at_startup = enabled;
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
        assert!(!settings.launch_at_startup);
        assert!(
            !serde_json::to_string(&settings)
                .unwrap()
                .contains("saveHistory")
        );
    }

    #[test]
    fn launch_at_startup_defaults_off_and_round_trips() {
        let mut settings = AppSettings::default();
        assert!(!settings.launch_at_startup);
        settings.launch_at_startup = true;

        let encoded = serde_json::to_string(&settings).unwrap();
        let decoded: AppSettings = serde_json::from_str(&encoded).unwrap();
        assert!(decoded.launch_at_startup);
        assert!(encoded.contains(r#""launchAtStartup":true"#));
    }
}
