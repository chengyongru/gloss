mod agent;
mod app_state;
mod network;
mod oauth;
mod overlay;
mod profile;
mod prompts;
mod responses;
mod selection;
mod sessions;
mod settings;

use app_state::{AppState, DEFAULT_SHORTCUT, OverlayState};
use tauri::{Emitter, Manager, State};
use tauri_plugin_global_shortcut::ShortcutState;

#[tauri::command]
fn get_overlay_state(state: State<'_, AppState>) -> Result<OverlayState, String> {
    state.overlay_snapshot()
}

#[tauri::command]
fn expand_overlay(app: tauri::AppHandle) -> Result<(), String> {
    overlay::expand_to_card(&app)
}

#[tauri::command]
fn collapse_overlay(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    match state.overlay_snapshot()? {
        OverlayState::Ready { selection } => overlay::show_toolbar(&app, selection.anchor),
        OverlayState::CaptureError { .. } => overlay::show_toolbar(&app, None),
        OverlayState::Idle => overlay::show_toolbar(&app, None),
    }
}

#[tauri::command]
fn hide_overlay(app: tauri::AppHandle) -> Result<(), String> {
    overlay::hide(&app)
}

#[tauri::command]
fn show_settings(app: tauri::AppHandle) -> Result<(), String> {
    overlay::expand_to_card(&app)?;
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "The Gloss window is unavailable.".to_owned())?;
    window
        .show()
        .and_then(|_| window.set_focus())
        .map_err(|error| format!("Could not show Gloss settings: {error}"))?;
    app.emit("show-settings", ())
        .map_err(|error| format!("Could not open Gloss settings: {error}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let shortcut_plugin = tauri_plugin_global_shortcut::Builder::new()
        .with_shortcut(DEFAULT_SHORTCUT)
        .expect("the default Gloss shortcut must be valid")
        .with_handler(|app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                overlay::handle_global_shortcut(app);
            }
        })
        .build();

    tauri::Builder::default()
        .manage(AppState::default())
        .plugin(shortcut_plugin)
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| format!("Could not resolve the Gloss data directory: {error}"))?;
            std::fs::create_dir_all(&data_dir)?;
            prompts::ensure_editable_templates(&data_dir).map_err(std::io::Error::other)?;
            app.state::<AppState>()
                .set_data_dir(data_dir)
                .map_err(std::io::Error::other)?;
            settings::load_and_activate(app.handle(), &app.state::<AppState>());
            let settings_item =
                tauri::menu::MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let quit_item =
                tauri::menu::MenuItem::with_id(app, "quit", "Quit Gloss", true, None::<&str>)?;
            let tray_menu = tauri::menu::Menu::with_items(app, &[&settings_item, &quit_item])?;
            let mut tray = tauri::tray::TrayIconBuilder::new()
                .menu(&tray_menu)
                .tooltip("Gloss — Ctrl+Alt+Shift+T");
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.on_menu_event(|app, event| {
                if event.id() == "settings" {
                    let _ = show_settings(app.clone());
                } else if event.id() == "quit" {
                    app.exit(0);
                }
            })
            .build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_overlay_state,
            expand_overlay,
            collapse_overlay,
            hide_overlay,
            show_settings,
            oauth::start_oauth_login,
            oauth::get_auth_status,
            oauth::logout_oauth,
            agent::start_action,
            agent::submit_follow_up,
            agent::explain_selection,
            agent::retry_turn,
            agent::get_active_session,
            agent::list_history,
            agent::load_history,
            agent::delete_history,
            settings::get_settings,
            settings::update_settings,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Gloss");
}
