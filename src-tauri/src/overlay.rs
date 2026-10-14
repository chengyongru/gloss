use crate::{
    app_state::{AppState, OverlayState},
    diagnostics,
    selection::{SelectionRect, capture_selected_text},
};
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager};
#[cfg(not(target_os = "macos"))]
use tauri::{PhysicalPosition, PhysicalSize};
#[cfg(windows)]
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
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

#[derive(Clone, Copy)]
struct CoordinateRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

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

pub fn configure_app(_app: &AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let app = _app;
        app.set_activation_policy(tauri::ActivationPolicy::Accessory)
            .map_err(|error| format!("Could not configure Gloss as a menu bar app: {error}"))?;
        let window = app
            .get_webview_window("main")
            .ok_or_else(|| "The Gloss window is unavailable.".to_owned())?;
        window
            .set_visible_on_all_workspaces(true)
            .map_err(|error| {
                format!("Could not make Gloss available on every workspace: {error}")
            })?;
        configure_fullscreen_auxiliary(&window)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn configure_fullscreen_auxiliary(window: &tauri::WebviewWindow) -> Result<(), String> {
    let pointer = window
        .ns_window()
        .map_err(|error| format!("Could not access the Gloss window: {error}"))?;
    if pointer.is_null() {
        return Err("Could not access the Gloss window.".to_owned());
    }
    // SAFETY: Tauri owns this NSWindow for the lifetime of `window`; this is a short, non-owning
    // borrow on the setup thread. `ns_window` does not return a retained (+1) pointer.
    let native = unsafe { &*pointer.cast::<NSWindow>() };
    let mut behavior = native.collectionBehavior();
    behavior.remove(
        NSWindowCollectionBehavior::FullScreenPrimary | NSWindowCollectionBehavior::FullScreenNone,
    );
    behavior.insert(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    native.setCollectionBehavior(behavior);
    Ok(())
}

#[cfg(target_os = "macos")]
fn coordinate_units(logical: f64, _scale: f64) -> f64 {
    logical
}

#[cfg(not(target_os = "macos"))]
fn coordinate_units(logical: f64, scale: f64) -> f64 {
    (logical * scale).round()
}

fn coordinate_work_area(monitor: &tauri::Monitor) -> CoordinateRect {
    let work = monitor.work_area();
    #[cfg(target_os = "macos")]
    {
        let scale = monitor.scale_factor();
        let position = work.position.to_logical::<f64>(scale);
        let size = work.size.to_logical::<f64>(scale);
        CoordinateRect {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        }
    }
    #[cfg(not(target_os = "macos"))]
    CoordinateRect {
        x: f64::from(work.position.x),
        y: f64::from(work.position.y),
        width: f64::from(work.size.width),
        height: f64::from(work.size.height),
    }
}

#[cfg(target_os = "macos")]
fn coordinate_outer_position(
    window: &tauri::WebviewWindow,
    scale: f64,
) -> tauri::Result<LogicalPosition<f64>> {
    Ok(window.outer_position()?.to_logical(scale))
}

#[cfg(not(target_os = "macos"))]
fn coordinate_outer_position(
    window: &tauri::WebviewWindow,
    _scale: f64,
) -> tauri::Result<LogicalPosition<f64>> {
    let position = window.outer_position()?;
    Ok(LogicalPosition::new(
        f64::from(position.x),
        f64::from(position.y),
    ))
}

#[cfg(target_os = "macos")]
fn coordinate_outer_size(
    window: &tauri::WebviewWindow,
    scale: f64,
) -> tauri::Result<LogicalSize<f64>> {
    Ok(window.outer_size()?.to_logical(scale))
}

#[cfg(not(target_os = "macos"))]
fn coordinate_outer_size(
    window: &tauri::WebviewWindow,
    _scale: f64,
) -> tauri::Result<LogicalSize<f64>> {
    let size = window.outer_size()?;
    Ok(LogicalSize::new(
        f64::from(size.width),
        f64::from(size.height),
    ))
}

#[cfg(target_os = "macos")]
fn set_coordinate_position(window: &tauri::WebviewWindow, x: f64, y: f64) -> tauri::Result<()> {
    window.set_position(LogicalPosition::new(x, y))
}

#[cfg(not(target_os = "macos"))]
fn set_coordinate_position(window: &tauri::WebviewWindow, x: f64, y: f64) -> tauri::Result<()> {
    window.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32))
}

#[cfg(target_os = "macos")]
fn set_coordinate_size(
    window: &tauri::WebviewWindow,
    width: f64,
    height: f64,
) -> tauri::Result<()> {
    window.set_size(LogicalSize::new(width, height))
}

#[cfg(not(target_os = "macos"))]
fn set_coordinate_size(
    window: &tauri::WebviewWindow,
    width: f64,
    height: f64,
) -> tauri::Result<()> {
    window.set_size(PhysicalSize::new(
        width.round() as u32,
        height.round() as u32,
    ))
}

pub fn handle_global_shortcut(app: &AppHandle) {
    let Some(capture_guard) = CaptureGuard::acquire() else {
        diagnostics::record("shortcut ignored while selection capture is already running");
        return;
    };
    diagnostics::record("shortcut received; selection capture started");
    let clipboard_owner = clipboard_owner(app);
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
                let message = format!("Selection capture stopped unexpectedly: {error}");
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
        global_cursor_position(app)
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
    let width = coordinate_units(TOOLBAR_WIDTH, scale);
    let height = coordinate_units(TOOLBAR_HEIGHT, scale);
    let work = coordinate_work_area(&monitor);
    let gap = coordinate_units(ANCHOR_GAP, scale);
    let edge = coordinate_units(EDGE_GAP, scale);
    let monitor_center = (work.x + work.width / 2.0, work.y + work.height / 2.0);
    let fallback = cursor.unwrap_or(monitor_center);

    let anchor_left = anchor.map_or(fallback.0, SelectionRect::center_x);
    let anchor_top = anchor.map_or(fallback.1, |rect| rect.top);
    let anchor_bottom = anchor.map_or(fallback.1, SelectionRect::bottom);
    let min_x = work.x + edge;
    let max_x = work.x + work.width - width - edge;
    let min_y = work.y + edge;
    let max_y = work.y + work.height - height - edge;
    let x = (anchor_left - width / 2.0).clamp(min_x, max_x.max(min_x));
    let above = anchor_top - height - gap;
    let y = if above >= min_y {
        above
    } else {
        (anchor_bottom + gap).clamp(min_y, max_y.max(min_y))
    };

    #[cfg(windows)]
    configure_as_tool_window(&window)?;
    set_toolbar_geometry(&window, width, height, x, y)?;
    #[cfg(target_os = "macos")]
    app.show()
        .map_err(|error| format!("Could not show the Gloss application: {error}"))?;
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
    width: f64,
    height: f64,
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
    let expanded_height = coordinate_units(ACTION_MENU_HEIGHT, scale);
    let position = coordinate_outer_position(&window, scale)
        .map_err(|error| format!("Could not read the Gloss position: {error}"))?;
    let monitor = window
        .current_monitor()
        .map_err(|error| format!("Could not find the active display: {error}"))?
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "Could not find an active display.".to_owned())?;
    let work = coordinate_work_area(&monitor);
    let edge = coordinate_units(EDGE_GAP, scale);
    let work_bottom = work.y + work.height - edge;
    Ok(if position.y + expanded_height > work_bottom {
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
    let toolbar_height = coordinate_units(TOOLBAR_HEIGHT, scale);
    let expanded_height = coordinate_units(ACTION_MENU_HEIGHT, scale);
    let size = coordinate_outer_size(&window, scale)
        .map_err(|error| format!("Could not read the Gloss size: {error}"))?;

    if !open {
        ACTION_MENU_OPEN.store(false, Ordering::Release);
        let above = ACTION_MENU_ABOVE.swap(false, Ordering::AcqRel);
        if (size.height - expanded_height).abs() > 2.0 {
            diagnostics::record("action menu already collapsed");
            return Ok(if above { "above" } else { "below" }.to_owned());
        }
        let position = coordinate_outer_position(&window, scale)
            .map_err(|error| format!("Could not read the Gloss position: {error}"))?;
        set_coordinate_size(&window, size.width, toolbar_height)
            .map_err(|error| format!("Could not collapse the action menu: {error}"))?;
        if above {
            let y = position.y + expanded_height - toolbar_height;
            set_coordinate_position(&window, position.x, y)
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
    if (size.height - toolbar_height).abs() > 2.0 {
        return Err("Gloss cannot open the action menu in the current view.".to_owned());
    }

    let position = coordinate_outer_position(&window, scale)
        .map_err(|error| format!("Could not read the Gloss position: {error}"))?;
    let monitor = window
        .current_monitor()
        .map_err(|error| format!("Could not find the active display: {error}"))?
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "Could not find an active display.".to_owned())?;
    let work = coordinate_work_area(&monitor);
    let edge = coordinate_units(EDGE_GAP, scale);
    let work_bottom = work.y + work.height - edge;
    let above = position.y + expanded_height > work_bottom;
    let expanded_y = if above {
        position.y - expanded_height + toolbar_height
    } else {
        position.y
    };

    ACTION_MENU_OPEN.store(true, Ordering::Release);
    ACTION_MENU_ABOVE.store(above, Ordering::Release);
    let resized = if above {
        set_coordinate_position(&window, position.x, expanded_y)
            .and_then(|_| set_coordinate_size(&window, size.width, expanded_height))
    } else {
        set_coordinate_size(&window, size.width, expanded_height)
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

#[cfg(windows)]
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
    width: f64,
    height: f64,
    x: f64,
    y: f64,
) -> Result<(), String> {
    set_coordinate_size(window, width, height)
        .map_err(|error| format!("Could not size the Gloss toolbar: {error}"))?;
    set_coordinate_position(window, x, y)
        .map_err(|error| format!("Could not position the Gloss toolbar: {error}"))
}

fn global_cursor_position(app: &AppHandle) -> Option<(f64, f64)> {
    let point = app.cursor_position().ok()?;
    #[cfg(target_os = "macos")]
    let point = point.to_logical::<f64>(app.primary_monitor().ok().flatten()?.scale_factor());
    Some((point.x, point.y))
}

#[cfg(windows)]
fn clipboard_owner(app: &AppHandle) -> isize {
    app.get_webview_window("main")
        .and_then(|window| window.hwnd().ok())
        .map(|hwnd| hwnd.0 as isize)
        .unwrap_or_default()
}

#[cfg(not(windows))]
fn clipboard_owner(_app: &AppHandle) -> isize {
    0
}

pub fn expand_to_card(app: &AppHandle) -> Result<(), String> {
    let menu_was_open = ACTION_MENU_OPEN.swap(false, Ordering::AcqRel);
    let menu_was_above = ACTION_MENU_ABOVE.swap(false, Ordering::AcqRel);
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "The Gloss window is unavailable.".to_owned())?;
    let scale = window
        .scale_factor()
        .map_err(|error| format!("Could not read the display scale: {error}"))?;
    let mut old_position = coordinate_outer_position(&window, scale)
        .map_err(|error| format!("Could not read the Gloss position: {error}"))?;
    let old_size = coordinate_outer_size(&window, scale)
        .map_err(|error| format!("Could not read the Gloss size: {error}"))?;
    let monitor = window
        .current_monitor()
        .map_err(|error| format!("Could not find the active display: {error}"))?
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "Could not find an active display.".to_owned())?;
    let monitor_scale = monitor.scale_factor();
    let toolbar_height = coordinate_units(TOOLBAR_HEIGHT, monitor_scale);
    let menu_height = coordinate_units(ACTION_MENU_HEIGHT, monitor_scale);
    if menu_was_open && menu_was_above && (old_size.height - menu_height).abs() <= 2.0 {
        old_position.y += menu_height - toolbar_height;
    }
    let work = coordinate_work_area(&monitor);
    let edge = coordinate_units(EDGE_GAP, monitor_scale);
    let max_width = (work.width - edge * 2.0).max(0.0);
    let max_height = (work.height - edge * 2.0).max(0.0);
    let width = coordinate_units(CARD_WIDTH, monitor_scale).min(max_width);
    let height = coordinate_units(CARD_HEIGHT, monitor_scale).min(max_height);
    let center_x = old_position.x + old_size.width / 2.0;
    let desired_x = center_x - width / 2.0;
    let min_x = work.x + edge;
    let max_x = work.x + work.width - width - edge;
    let min_y = work.y + edge;
    let max_y = work.y + work.height - height - edge;
    let x = desired_x.clamp(min_x, max_x.max(min_x));
    let y = old_position.y.clamp(min_y, max_y.max(min_y));

    set_coordinate_size(&window, width, height)
        .and_then(|_| set_coordinate_position(&window, x, y))
        .map_err(|error| format!("Could not expand the Gloss card: {error}"))
}

pub fn hide(app: &AppHandle) -> Result<(), String> {
    ACTION_MENU_OPEN.store(false, Ordering::Release);
    ACTION_MENU_ABOVE.store(false, Ordering::Release);
    app.get_webview_window("main")
        .ok_or_else(|| "The Gloss window is unavailable.".to_owned())?
        .hide()
        .map_err(|error| format!("Could not hide Gloss: {error}"))?;
    #[cfg(target_os = "macos")]
    app.hide()
        .map_err(|error| format!("Could not hide the Gloss application: {error}"))?;
    Ok(())
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
