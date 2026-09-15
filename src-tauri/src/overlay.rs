use crate::{
    app_state::{AppState, OverlayState},
    diagnostics,
    selection::{SelectionRect, capture_selected_text},
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
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
const TOOLBAR_HEIGHT: f64 = 60.0;
const ACTION_MENU_HEIGHT: f64 = 208.0;
const CARD_WIDTH: f64 = 440.0;
const CARD_HEIGHT: f64 = 540.0;
const EDGE_GAP: f64 = 12.0;
const ANCHOR_GAP: f64 = 8.0;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(4);
static CAPTURE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static ACTION_MENU_OPEN: AtomicBool = AtomicBool::new(false);
static ACTION_MENU_ABOVE: AtomicBool = AtomicBool::new(false);

struct CaptureGuard;

impl CaptureGuard {
    fn acquire() -> Option<Self> {
        CAPTURE_IN_PROGRESS
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        CAPTURE_IN_PROGRESS.store(false, Ordering::Release);
    }
}

pub fn handle_global_shortcut(app: &AppHandle) {
    let Some(capture_guard) = CaptureGuard::acquire() else {
        diagnostics::record("shortcut ignored while selection capture is already running");
        return;
    };
    diagnostics::record("shortcut received; selection capture started");
    let clipboard_owner = app
        .get_webview_window("main")
        .and_then(|window| window.hwnd().ok())
        .map(|hwnd| hwnd.0 as isize)
        .unwrap_or_default();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let capture_task = tauri::async_runtime::spawn_blocking(move || {
            let _capture_guard = capture_guard;
            capture_selected_text(clipboard_owner)
        });
        let capture = tokio::time::timeout(CAPTURE_TIMEOUT, capture_task).await;
        let next = match capture {
            Ok(Ok(Ok(selection))) => {
                diagnostics::record(format!(
                    "selection capture completed chars={} anchored={}",
                    selection.text.chars().count(),
                    selection.anchor.is_some()
                ));
                OverlayState::Ready { selection }
            }
            Ok(Ok(Err(message))) => {
                diagnostics::record(format!("selection capture failed: {message}"));
                OverlayState::CaptureError { message }
            }
            Ok(Err(error)) => {
                let message = format!("Windows UI Automation stopped unexpectedly: {error}");
                diagnostics::record(format!("selection capture task failed: {message}"));
                OverlayState::CaptureError { message }
            }
            Err(_) => {
                let message = "Selection capture timed out. Try the shortcut again.".to_owned();
                diagnostics::record("selection capture timed out");
                OverlayState::CaptureError { message }
            }
        };

        if let Some(state) = app.try_state::<AppState>() {
            let _ = state.replace_overlay(next.clone());
        }
        let _ = app.emit("overlay-state", &next);
        let anchor = match &next {
            OverlayState::Ready { selection } => selection.anchor,
            _ => None,
        };
        if let Err(message) = show_toolbar(&app, anchor) {
            diagnostics::record(format!("overlay show failed: {message}"));
        } else {
            diagnostics::record("overlay shown");
        }
    });
}

pub fn show_toolbar(app: &AppHandle, anchor: Option<SelectionRect>) -> Result<(), String> {
    ACTION_MENU_OPEN.store(false, Ordering::Release);
    ACTION_MENU_ABOVE.store(false, Ordering::Release);
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
            if ACTION_MENU_OPEN.load(Ordering::Acquire) || !window.is_visible().unwrap_or(false) {
                break;
            }
            let _ = set_toolbar_geometry(&window, width, height, x, y);
        }
    });
}

pub fn action_menu_placement(app: &AppHandle) -> Result<String, String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "The Gloss window is unavailable.".to_owned())?;
    let scale = window
        .scale_factor()
        .map_err(|error| format!("Could not read the display scale: {error}"))?;
    let expanded_height = (ACTION_MENU_HEIGHT * scale).round() as u32;
    let position = window
        .outer_position()
        .map_err(|error| format!("Could not read the Gloss position: {error}"))?;
    let monitor = window
        .current_monitor()
        .map_err(|error| format!("Could not find the active display: {error}"))?
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "Could not find an active display.".to_owned())?;
    let work = monitor.work_area();
    let edge = (EDGE_GAP * scale).round() as i32;
    let work_bottom = work.position.y + work.size.height as i32 - edge;
    Ok(if position.y + expanded_height as i32 > work_bottom {
        "above"
    } else {
        "below"
    }
    .to_owned())
}

pub fn set_action_menu_open(app: &AppHandle, open: bool) -> Result<String, String> {
    diagnostics::record(format!("action menu resize requested open={open}"));
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "The Gloss window is unavailable.".to_owned())?;
    let scale = window
        .scale_factor()
        .map_err(|error| format!("Could not read the display scale: {error}"))?;
    let toolbar_height = (TOOLBAR_HEIGHT * scale).round() as u32;
    let expanded_height = (ACTION_MENU_HEIGHT * scale).round() as u32;
    let size = window
        .outer_size()
        .map_err(|error| format!("Could not read the Gloss size: {error}"))?;

    if !open {
        ACTION_MENU_OPEN.store(false, Ordering::Release);
        let above = ACTION_MENU_ABOVE.swap(false, Ordering::AcqRel);
        if size.height.abs_diff(expanded_height) > 2 {
            diagnostics::record("action menu already collapsed");
            return Ok(if above { "above" } else { "below" }.to_owned());
        }
        let position = window
            .outer_position()
            .map_err(|error| format!("Could not read the Gloss position: {error}"))?;
        window
            .set_size(PhysicalSize::new(size.width, toolbar_height))
            .map_err(|error| format!("Could not collapse the action menu: {error}"))?;
        if above {
            let y = position.y + expanded_height as i32 - toolbar_height as i32;
            window
                .set_position(PhysicalPosition::new(position.x, y))
                .map_err(|error| format!("Could not restore the toolbar position: {error}"))?;
        }
        diagnostics::record("action menu collapsed");
        return Ok(if above { "above" } else { "below" }.to_owned());
    }

    if ACTION_MENU_OPEN.load(Ordering::Acquire) {
        return Ok(if ACTION_MENU_ABOVE.load(Ordering::Acquire) {
            "above"
        } else {
            "below"
        }
        .to_owned());
    }
    if size.height.abs_diff(toolbar_height) > 2 {
        return Err("Gloss cannot open the action menu in the current view.".to_owned());
    }

    let position = window
        .outer_position()
        .map_err(|error| format!("Could not read the Gloss position: {error}"))?;
    let monitor = window
        .current_monitor()
        .map_err(|error| format!("Could not find the active display: {error}"))?
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "Could not find an active display.".to_owned())?;
    let work = monitor.work_area();
    let edge = (EDGE_GAP * scale).round() as i32;
    let work_bottom = work.position.y + work.size.height as i32 - edge;
    let above = position.y + expanded_height as i32 > work_bottom;
    let expanded_y = if above {
        position.y - expanded_height as i32 + toolbar_height as i32
    } else {
        position.y
    };

    ACTION_MENU_OPEN.store(true, Ordering::Release);
    ACTION_MENU_ABOVE.store(above, Ordering::Release);
    let resized = if above {
        window
            .set_position(PhysicalPosition::new(position.x, expanded_y))
            .and_then(|_| window.set_size(PhysicalSize::new(size.width, expanded_height)))
    } else {
        window.set_size(PhysicalSize::new(size.width, expanded_height))
    };
    if let Err(error) = resized {
        ACTION_MENU_OPEN.store(false, Ordering::Release);
        ACTION_MENU_ABOVE.store(false, Ordering::Release);
        return Err(format!("Could not open the action menu: {error}"));
    }
    diagnostics::record(format!(
        "action menu expanded placement={}",
        if above { "above" } else { "below" }
    ));
    Ok(if above { "above" } else { "below" }.to_owned())
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
    let menu_was_open = ACTION_MENU_OPEN.swap(false, Ordering::AcqRel);
    let menu_was_above = ACTION_MENU_ABOVE.swap(false, Ordering::AcqRel);
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "The Gloss window is unavailable.".to_owned())?;
    let mut old_position = window
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
    let toolbar_height = (TOOLBAR_HEIGHT * scale).round() as u32;
    let menu_height = (ACTION_MENU_HEIGHT * scale).round() as u32;
    if menu_was_open && menu_was_above && old_size.height.abs_diff(menu_height) <= 2 {
        old_position.y += menu_height as i32 - toolbar_height as i32;
    }
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
    ACTION_MENU_OPEN.store(false, Ordering::Release);
    ACTION_MENU_ABOVE.store(false, Ordering::Release);
    app.get_webview_window("main")
        .ok_or_else(|| "The Gloss window is unavailable.".to_owned())?
        .hide()
        .map_err(|error| format!("Could not hide Gloss: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_guard_rejects_overlapping_capture() {
        let first = CaptureGuard::acquire().expect("first capture should acquire the guard");
        assert!(CaptureGuard::acquire().is_none());
        drop(first);
        assert!(CaptureGuard::acquire().is_some());
    }
}
