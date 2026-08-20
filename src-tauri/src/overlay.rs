use crate::{
    app_state::{AppState, OverlayState},
    selection::{SelectionRect, capture_selected_text},
};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize};
use windows::Win32::{
    Foundation::{HWND, POINT},
    UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetCursorPos, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_APPWINDOW,
        WS_EX_TOOLWINDOW,
    },
};

const TOOLBAR_WIDTH: f64 = 440.0;
const TOOLBAR_HEIGHT: f64 = 72.0;
const CARD_WIDTH: f64 = 440.0;
const CARD_HEIGHT: f64 = 540.0;
const EDGE_GAP: f64 = 12.0;
const ANCHOR_GAP: f64 = 8.0;

pub fn handle_global_shortcut(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let capture = tauri::async_runtime::spawn_blocking(capture_selected_text).await;
        let next = match capture {
            Ok(Ok(selection)) => OverlayState::Ready { selection },
            Ok(Err(message)) => OverlayState::CaptureError { message },
            Err(error) => OverlayState::CaptureError {
                message: format!("Windows UI Automation stopped unexpectedly: {error}"),
            },
        };

        if let Some(state) = app.try_state::<AppState>() {
            let _ = state.replace_overlay(next.clone());
        }
        let _ = app.emit("overlay-state", &next);
        let anchor = match &next {
            OverlayState::Ready { selection } => selection.anchor,
            _ => None,
        };
        let _ = show_toolbar(&app, anchor);
    });
}

pub fn show_toolbar(app: &AppHandle, anchor: Option<SelectionRect>) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "The Gloss window is unavailable.".to_owned())?;
    let cursor = if anchor.is_none() {
        global_cursor_position()
    } else {
        None
    };
    let probe = anchor.map(|rect| (rect.center_x(), rect.top)).or(cursor);
    let monitor = probe
        .map(|(x, y)| window.monitor_from_point(x, y))
        .transpose()
        .map_err(|error| format!("Could not find the active display: {error}"))?
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "Could not find an active display.".to_owned())?;
    let scale = monitor.scale_factor();
    let width = (TOOLBAR_WIDTH * scale).round() as u32;
    let height = (TOOLBAR_HEIGHT * scale).round() as u32;
    let work = monitor.work_area();
    let gap = ANCHOR_GAP * scale;
    let edge = EDGE_GAP * scale;
    let monitor_center = (
        work.position.x as f64 + work.size.width as f64 / 2.0,
        work.position.y as f64 + work.size.height as f64 / 2.0,
    );
    let fallback = cursor.unwrap_or(monitor_center);

    let anchor_left = anchor.map_or(fallback.0, SelectionRect::center_x);
    let anchor_top = anchor.map_or(fallback.1, |rect| rect.top);
    let anchor_bottom = anchor.map_or(fallback.1, SelectionRect::bottom);
    let min_x = work.position.x as f64 + edge;
    let max_x = (work.position.x + work.size.width as i32) as f64 - width as f64 - edge;
    let min_y = work.position.y as f64 + edge;
    let max_y = (work.position.y + work.size.height as i32) as f64 - height as f64 - edge;
    let x = (anchor_left - width as f64 / 2.0).clamp(min_x, max_x.max(min_x));
    let above = anchor_top - height as f64 - gap;
    let y = if above >= min_y {
        above
    } else {
        (anchor_bottom + gap).clamp(min_y, max_y.max(min_y))
    };

    configure_as_tool_window(&window)?;
    set_toolbar_geometry(&window, width, height, x, y)?;
    window
        .show()
        .and_then(|_| window.set_focus())
        .map_err(|error| format!("Could not show the Gloss toolbar: {error}"))?;
    set_toolbar_geometry(&window, width, height, x, y)?;
    stabilize_toolbar_geometry(window.clone(), width, height, x, y);
    Ok(())
}

fn stabilize_toolbar_geometry(
    window: tauri::WebviewWindow,
    width: u32,
    height: u32,
    x: f64,
    y: f64,
) {
    tauri::async_runtime::spawn(async move {
        for delay in [150, 200, 350] {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            if !window.is_visible().unwrap_or(false) {
                break;
            }
            let _ = set_toolbar_geometry(&window, width, height, x, y);
        }
    });
}

fn configure_as_tool_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    let native = window
        .hwnd()
        .map_err(|error| format!("Could not access the Gloss window: {error}"))?;
    let hwnd = HWND(native.0);
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let tool_style = (style & !(WS_EX_APPWINDOW.0 as isize)) | WS_EX_TOOLWINDOW.0 as isize;
    unsafe { SetWindowLongPtrW(hwnd, GWL_EXSTYLE, tool_style) };
    Ok(())
}

fn set_toolbar_geometry(
    window: &tauri::WebviewWindow,
    width: u32,
    height: u32,
    x: f64,
    y: f64,
) -> Result<(), String> {
    window
        .set_size(PhysicalSize::new(width, height))
        .map_err(|error| format!("Could not size the Gloss toolbar: {error}"))?;
    window
        .set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32))
        .map_err(|error| format!("Could not position the Gloss toolbar: {error}"))
}

fn global_cursor_position() -> Option<(f64, f64)> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.ok()?;
    Some((f64::from(point.x), f64::from(point.y)))
}

pub fn expand_to_card(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "The Gloss window is unavailable.".to_owned())?;
    let old_position = window
        .outer_position()
        .map_err(|error| format!("Could not read the Gloss position: {error}"))?;
    let old_size = window
        .outer_size()
        .map_err(|error| format!("Could not read the Gloss size: {error}"))?;
    let monitor = window
        .current_monitor()
        .map_err(|error| format!("Could not find the active display: {error}"))?
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "Could not find an active display.".to_owned())?;
    let scale = monitor.scale_factor();
    let work = monitor.work_area();
    let edge = (EDGE_GAP * scale).round() as i32;
    let max_width = work.size.width.saturating_sub((edge * 2).max(0) as u32);
    let max_height = work.size.height.saturating_sub((edge * 2).max(0) as u32);
    let width = ((CARD_WIDTH * scale).round() as u32).min(max_width);
    let height = ((CARD_HEIGHT * scale).round() as u32).min(max_height);
    let center_x = old_position.x as i64 + old_size.width as i64 / 2;
    let desired_x = center_x - width as i64 / 2;
    let min_x = work.position.x + edge;
    let max_x = work.position.x + work.size.width as i32 - width as i32 - edge;
    let min_y = work.position.y + edge;
    let max_y = work.position.y + work.size.height as i32 - height as i32 - edge;
    let x = (desired_x as i32).clamp(min_x, max_x.max(min_x));
    let y = old_position.y.clamp(min_y, max_y.max(min_y));

    window
        .set_size(PhysicalSize::new(width, height))
        .and_then(|_| window.set_position(PhysicalPosition::new(x, y)))
        .map_err(|error| format!("Could not expand the Gloss card: {error}"))
}

pub fn hide(app: &AppHandle) -> Result<(), String> {
    app.get_webview_window("main")
        .ok_or_else(|| "The Gloss window is unavailable.".to_owned())?
        .hide()
        .map_err(|error| format!("Could not hide Gloss: {error}"))
}
