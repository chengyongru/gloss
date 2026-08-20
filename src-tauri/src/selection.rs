use serde::Serialize;
use std::{ffi::c_void, mem::size_of, ptr, slice, thread, time::Duration};
use uiautomation::{
    UIAutomation, UIElement,
    patterns::{UITextPattern, UITextRange},
    types::{Point as UiPoint, TreeScope, UIProperty},
    variants::Variant,
};
use windows::Win32::{
    Foundation::{HGLOBAL, HWND, POINT},
    System::{
        Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize, IDataObject, SAFEARRAY},
        DataExchange::{
            CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
        },
        Memory::{GlobalLock, GlobalSize, GlobalUnlock},
        Ole::{
            CF_UNICODETEXT, OleFlushClipboard, OleGetClipboard, OleInitialize, OleSetClipboard,
            OleUninitialize, SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetDim,
            SafeArrayGetLBound, SafeArrayGetUBound, SafeArrayUnaccessData,
        },
    },
    UI::{
        Accessibility::IUIAutomationTextRange,
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
            KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_INSERT, VK_LWIN, VK_MENU,
            VK_RWIN, VK_SHIFT,
        },
        WindowsAndMessaging::{GetCursorPos, GetForegroundWindow},
    },
};

const MAX_ANCESTOR_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRect {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

impl SelectionRect {
    pub fn center_x(self) -> f64 {
        self.left + self.width / 2.0
    }

    pub fn bottom(self) -> f64 {
        self.top + self.height
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionCapture {
    pub text: String,
    pub anchor: Option<SelectionRect>,
}

struct ComGuard;

impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: this balances the successful CoInitializeEx call on this thread.
        unsafe { CoUninitialize() };
    }
}

struct OleGuard;

impl Drop for OleGuard {
    fn drop(&mut self) {
        // SAFETY: this balances the successful OleInitialize call on this thread.
        unsafe { OleUninitialize() };
    }
}

struct ClipboardOpenGuard;

impl Drop for ClipboardOpenGuard {
    fn drop(&mut self) {
        // SAFETY: this balances the successful OpenClipboard call on this thread.
        let _ = unsafe { CloseClipboard() };
    }
}

struct ClipboardRestoreGuard {
    snapshot: Option<IDataObject>,
}

impl ClipboardRestoreGuard {
    fn new(snapshot: IDataObject) -> Self {
        Self {
            snapshot: Some(snapshot),
        }
    }

    fn restore(&mut self) -> Result<(), String> {
        let Some(snapshot) = self.snapshot.take() else {
            return Ok(());
        };
        retry_clipboard_operation(|| unsafe { OleSetClipboard(&snapshot) })
            .and_then(|_| retry_clipboard_operation(|| unsafe { OleFlushClipboard() }))
            .map_err(|error| format!("Could not restore the clipboard: {error}"))
    }
}

impl Drop for ClipboardRestoreGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

struct SafeArrayGuard(*mut SAFEARRAY);

impl Drop for SafeArrayGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the array is returned to this caller by UI Automation.
            let _ = unsafe { SafeArrayDestroy(self.0) };
        }
    }
}

struct SafeArrayAccessGuard(*mut SAFEARRAY);

impl Drop for SafeArrayAccessGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this balances the successful SafeArrayAccessData call.
            let _ = unsafe { SafeArrayUnaccessData(self.0) };
        }
    }
}

pub fn capture_selected_text() -> Result<SelectionCapture, String> {
    let target = unsafe { GetForegroundWindow() };
    if target.0.is_null() {
        return Err("Couldn't find the app containing the selection.".to_owned());
    }

    let uia_capture = capture_with_uia(target).ok().flatten();
    if let Some(selection) = uia_capture.as_ref()
        && !needs_copy_fallback(&selection.text)
    {
        return Ok(selection.clone());
    }

    capture_with_copy_shortcut(target)?
        .or(uia_capture)
        .ok_or_else(|| "Couldn't read the selected text in this app.".to_owned())
}

fn needs_copy_fallback(text: &str) -> bool {
    text.chars()
        .any(|character| matches!(character, '\u{fffc}' | '\u{fffd}'))
}

fn capture_with_uia(target: HWND) -> Result<Option<SelectionCapture>, String> {
    // SAFETY: the blocking worker owns this COM initialization until this function returns.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|error| format!("Could not initialize Windows UI Automation: {error}"))?;
    let _com = ComGuard;

    let automation = UIAutomation::new_direct()
        .map_err(|error| format!("Could not start Windows UI Automation: {error}"))?;
    let walker = automation
        .get_raw_view_walker()
        .map_err(|error| format!("Could not inspect the accessibility tree: {error}"))?;

    if let Ok(focused) = automation.get_focused_element()
        && let Some(capture) = capture_from_ancestor_chain(focused, &walker)?
    {
        return Ok(Some(capture));
    }

    if let Some(capture) = capture_from_window_subtree(&automation, target)? {
        return Ok(Some(capture));
    }

    if let Some(element) = element_at_cursor(&automation)
        && let Some(capture) = capture_from_ancestor_chain(element, &walker)?
    {
        return Ok(Some(capture));
    }

    Ok(None)
}

fn capture_from_ancestor_chain(
    mut element: UIElement,
    walker: &uiautomation::UITreeWalker,
) -> Result<Option<SelectionCapture>, String> {
    let mut best = None;
    for depth in 0..=MAX_ANCESTOR_DEPTH {
        if let Some(capture) = capture_from_element(&element)? {
            best = Some(capture);
        }
        if depth == MAX_ANCESTOR_DEPTH {
            break;
        }
        element = match walker.get_parent(&element) {
            Ok(parent) => parent,
            Err(_) => break,
        };
    }
    Ok(best)
}

fn element_at_cursor(automation: &UIAutomation) -> Option<UIElement> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.ok()?;
    automation
        .element_from_point(UiPoint::new(point.x, point.y))
        .ok()
}

fn capture_from_window_subtree(
    automation: &UIAutomation,
    target: HWND,
) -> Result<Option<SelectionCapture>, String> {
    let root = match automation.element_from_handle(target.into()) {
        Ok(root) => root,
        Err(_) => return Ok(None),
    };
    let condition = automation
        .create_property_condition(
            UIProperty::IsTextPatternAvailable,
            Variant::from(true),
            None,
        )
        .map_err(|error| format!("Could not search the foreground app for text: {error}"))?;
    let providers = match root.find_all(TreeScope::Subtree, &condition) {
        Ok(providers) => providers,
        Err(_) => return Ok(None),
    };
    for provider in providers {
        if let Some(capture) = capture_from_element(&provider)? {
            return Ok(Some(capture));
        }
    }
    Ok(None)
}

fn capture_with_copy_shortcut(target: HWND) -> Result<Option<SelectionCapture>, String> {
    // OLE clipboard operations require their own single-threaded apartment. The UIA MTA
    // above has already been released before this fallback starts.
    unsafe { OleInitialize(None) }
        .map_err(|error| format!("Could not initialize clipboard capture: {error}"))?;
    let _ole = OleGuard;
    let snapshot = retry_clipboard_operation(|| unsafe { OleGetClipboard() })
        .map_err(|error| format!("Could not preserve the clipboard: {error}"))?;
    let mut restore = ClipboardRestoreGuard::new(snapshot);
    let sequence = unsafe { GetClipboardSequenceNumber() };

    if unsafe { GetForegroundWindow() } != target {
        restore.restore()?;
        return Ok(None);
    }
    send_ctrl_insert()?;

    let mut text = None;
    for _ in 0..20 {
        thread::sleep(Duration::from_millis(20));
        if unsafe { GetClipboardSequenceNumber() } != sequence {
            text = clipboard_text();
            break;
        }
    }
    restore.restore()?;

    Ok(text
        .filter(|value| !value.trim().is_empty())
        .map(|text| SelectionCapture { text, anchor: None }))
}

fn send_ctrl_insert() -> Result<(), String> {
    let mut inputs = Vec::with_capacity(8);
    inputs.push(key_input(VK_CONTROL, false));
    for modifier in [VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN] {
        if unsafe { GetAsyncKeyState(i32::from(modifier.0)) } < 0 {
            inputs.push(key_input(modifier, true));
        }
    }
    inputs.push(key_input(VK_INSERT, false));
    inputs.push(key_input(VK_INSERT, true));
    inputs.push(key_input(VK_CONTROL, true));

    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        Err("Windows blocked the fallback copy shortcut.".to_owned())
    }
}

fn key_input(key: VIRTUAL_KEY, released: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: if released {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn clipboard_text() -> Option<String> {
    let _clipboard = open_clipboard()?;
    let handle = unsafe { GetClipboardData(CF_UNICODETEXT.0.into()) }.ok()?;
    let global = HGLOBAL(handle.0);
    let byte_len = unsafe { GlobalSize(global) };
    if byte_len < size_of::<u16>() {
        return None;
    }
    let data = unsafe { GlobalLock(global) };
    if data.is_null() {
        return None;
    }
    let units = unsafe { slice::from_raw_parts(data.cast::<u16>(), byte_len / size_of::<u16>()) };
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    let text = String::from_utf16_lossy(&units[..end]);
    let _ = unsafe { GlobalUnlock(global) };
    Some(text)
}

fn open_clipboard() -> Option<ClipboardOpenGuard> {
    for _ in 0..8 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            return Some(ClipboardOpenGuard);
        }
        thread::sleep(Duration::from_millis(10));
    }
    None
}

fn retry_clipboard_operation<T>(
    mut operation: impl FnMut() -> windows::core::Result<T>,
) -> windows::core::Result<T> {
    let mut last_error = None;
    for _ in 0..8 {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => last_error = Some(error),
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(last_error.unwrap_or_else(windows::core::Error::from_thread))
}

fn capture_from_element(element: &UIElement) -> Result<Option<SelectionCapture>, String> {
    let pattern = match element.get_pattern::<UITextPattern>() {
        Ok(pattern) => pattern,
        Err(_) => return Ok(None),
    };
    let ranges = match pattern.get_selection() {
        Ok(ranges) => ranges,
        Err(_) => return Ok(None),
    };

    for range in ranges {
        let text = match range.get_text(-1) {
            Ok(text) => text,
            Err(_) => continue,
        };
        if text.trim().is_empty() {
            continue;
        }
        return Ok(Some(SelectionCapture {
            text,
            anchor: selection_rectangles(&range)
                .ok()
                .and_then(|rects| rects.last().copied()),
        }));
    }
    Ok(None)
}

fn selection_rectangles(range: &UITextRange) -> Result<Vec<SelectionRect>, String> {
    let raw: &IUIAutomationTextRange = range.as_ref();
    // SAFETY: UI Automation returns an owned SAFEARRAY of f64 values.
    let array = unsafe { raw.GetBoundingRectangles() }
        .map_err(|error| format!("Could not read the selection bounds: {error}"))?;
    if array.is_null() {
        return Ok(Vec::new());
    }
    let _array = SafeArrayGuard(array);

    // SAFETY: all operations below target the live one-dimensional SAFEARRAY.
    if unsafe { SafeArrayGetDim(array) } != 1 {
        return Ok(Vec::new());
    }
    let lower = unsafe { SafeArrayGetLBound(array, 1) }
        .map_err(|error| format!("Could not read the selection bounds: {error}"))?;
    let upper = unsafe { SafeArrayGetUBound(array, 1) }
        .map_err(|error| format!("Could not read the selection bounds: {error}"))?;
    if upper < lower {
        return Ok(Vec::new());
    }

    let len = (upper - lower + 1) as usize;
    let mut data: *mut c_void = ptr::null_mut();
    unsafe { SafeArrayAccessData(array, &mut data) }
        .map_err(|error| format!("Could not read the selection bounds: {error}"))?;
    let _access = SafeArrayAccessGuard(array);
    if data.is_null() {
        return Ok(Vec::new());
    }

    // SAFETY: UI Automation documents this SAFEARRAY as packed doubles in groups of four.
    let values = unsafe { slice::from_raw_parts(data.cast::<f64>(), len) };
    Ok(values
        .chunks_exact(4)
        .filter_map(|chunk| {
            let rect = SelectionRect {
                left: chunk[0],
                top: chunk[1],
                width: chunk[2],
                height: chunk[3],
            };
            (rect.left.is_finite()
                && rect.top.is_finite()
                && rect.width.is_finite()
                && rect.height.is_finite()
                && rect.width > 0.0
                && rect.height > 0.0)
                .then_some(rect)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_helpers_use_physical_coordinates() {
        let rect = SelectionRect {
            left: 10.0,
            top: 20.0,
            width: 40.0,
            height: 12.0,
        };
        assert_eq!(rect.center_x(), 30.0);
        assert_eq!(rect.bottom(), 32.0);
    }

    #[test]
    fn key_input_marks_only_release_events() {
        let pressed = key_input(VK_INSERT, false);
        let released = key_input(VK_INSERT, true);
        assert_eq!(
            unsafe { pressed.Anonymous.ki.dwFlags },
            KEYBD_EVENT_FLAGS(0)
        );
        assert_eq!(unsafe { released.Anonymous.ki.dwFlags }, KEYEVENTF_KEYUP);
    }

    #[test]
    fn embedded_object_placeholder_requests_copy_semantics() {
        assert!(needs_copy_fallback("any color you want \u{fffc}"));
    }
}
