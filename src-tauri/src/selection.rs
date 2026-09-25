use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRect {
    // Windows uses UI Automation's global physical pixels. macOS uses the AX/Quartz global
    // logical desktop space; overlay.rs consumes that platform coordinate space directly.
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

pub fn capture_selected_text(clipboard_owner: isize) -> Result<SelectionCapture, String> {
    platform::capture_selected_text(clipboard_owner)
}

fn needs_copy_fallback(text: &str) -> bool {
    text.chars()
        .any(|character| matches!(character, '\u{fffc}' | '\u{fffd}'))
}

#[cfg(windows)]
mod platform {
    use super::{SelectionCapture, SelectionRect, needs_copy_fallback};
    use std::{ffi::c_void, mem::size_of, ptr, slice, thread, time::Duration};
    use uiautomation::{
        UIAutomation, UIElement,
        patterns::{UITextPattern, UITextRange},
        types::Point as UiPoint,
    };
    use windows::Win32::{
        Foundation::{HANDLE, HGLOBAL, HWND, POINT},
        System::{
            Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize, SAFEARRAY},
            DataExchange::{
                CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
                GetClipboardSequenceNumber, OpenClipboard, SetClipboardData,
            },
            Memory::{GLOBAL_ALLOC_FLAGS, GlobalLock, GlobalSize, GlobalUnlock},
            Ole::{
                CF_UNICODETEXT, CLIPBOARD_FORMAT, OleDuplicateData, SafeArrayAccessData,
                SafeArrayDestroy, SafeArrayGetDim, SafeArrayGetLBound, SafeArrayGetUBound,
                SafeArrayUnaccessData,
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
    const MODIFIER_RELEASE_ATTEMPTS: usize = 25;
    const MODIFIER_RELEASE_DELAY: Duration = Duration::from_millis(20);

    struct ComGuard;

    impl Drop for ComGuard {
        fn drop(&mut self) {
            // SAFETY: this balances the successful CoInitializeEx call on this thread.
            unsafe { CoUninitialize() };
        }
    }

    struct ClipboardOpenGuard;

    impl Drop for ClipboardOpenGuard {
        fn drop(&mut self) {
            // SAFETY: this balances the successful OpenClipboard call on this thread.
            let _ = unsafe { CloseClipboard() };
        }
    }

    struct ClipboardFormat {
        format: u32,
        handle: HANDLE,
    }

    struct ClipboardSnapshot {
        owner: isize,
        formats: Vec<ClipboardFormat>,
        restored: bool,
    }

    impl ClipboardSnapshot {
        fn capture(owner: isize) -> Result<Self, String> {
            let _clipboard = open_clipboard(None)
                .ok_or_else(|| "Could not open the clipboard for preservation.".to_owned())?;
            let mut formats = Vec::new();
            let mut current = 0;
            let mut available = 0;
            loop {
                let format = unsafe { EnumClipboardFormats(current) };
                if format == 0 {
                    break;
                }
                available += 1;
                current = format;
                let Ok(source) = (unsafe { GetClipboardData(format) }) else {
                    continue;
                };
                let duplicate = unsafe {
                    OleDuplicateData(
                        source,
                        CLIPBOARD_FORMAT(format as u16),
                        GLOBAL_ALLOC_FLAGS(0),
                    )
                };
                if !duplicate.is_invalid() {
                    formats.push(ClipboardFormat {
                        format,
                        handle: duplicate,
                    });
                }
            }
            if available > 0 && formats.is_empty() {
                return Err("Could not preserve the current clipboard formats.".to_owned());
            }
            Ok(Self {
                owner,
                formats,
                restored: false,
            })
        }

        fn restore(&mut self) -> Result<(), String> {
            if self.restored {
                return Ok(());
            }
            let owner = HWND(self.owner as *mut c_void);
            let _clipboard = open_clipboard(Some(owner))
                .ok_or_else(|| "Could not reopen the clipboard for restoration.".to_owned())?;
            unsafe { EmptyClipboard() }
                .map_err(|error| format!("Could not clear the temporary clipboard: {error}"))?;

            let mut failed = 0;
            for item in &mut self.formats {
                if item.handle.is_invalid() {
                    continue;
                }
                if unsafe { SetClipboardData(item.format, Some(item.handle)) }.is_ok() {
                    item.handle = HANDLE::default();
                } else {
                    failed += 1;
                }
            }
            self.restored = true;
            if failed == 0 {
                Ok(())
            } else {
                Err(format!("Could not restore {failed} clipboard format(s)."))
            }
        }
    }

    impl Drop for ClipboardSnapshot {
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

    pub(super) fn capture_selected_text(
        clipboard_owner: isize,
    ) -> Result<SelectionCapture, String> {
        let target = unsafe { GetForegroundWindow() };
        if target.0.is_null() {
            return Err("Couldn't find the app containing the selection.".to_owned());
        }

        let uia_capture = capture_with_uia().ok().flatten();
        if let Some(selection) = uia_capture.as_ref()
            && !needs_copy_fallback(&selection.text)
        {
            return Ok(selection.clone());
        }

        capture_with_copy_shortcut(target, clipboard_owner)?
            .or(uia_capture)
            .ok_or_else(|| "Couldn't read the selected text in this app.".to_owned())
    }

    fn capture_with_uia() -> Result<Option<SelectionCapture>, String> {
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

    fn capture_with_copy_shortcut(
        target: HWND,
        clipboard_owner: isize,
    ) -> Result<Option<SelectionCapture>, String> {
        if clipboard_owner == 0 {
            return Err("The Gloss window is unavailable for clipboard capture.".to_owned());
        }
        let mut snapshot = ClipboardSnapshot::capture(clipboard_owner)?;
        let sequence = unsafe { GetClipboardSequenceNumber() };

        if unsafe { GetForegroundWindow() } != target {
            snapshot.restore()?;
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
        snapshot.restore()?;

        Ok(text
            .filter(|value| !value.trim().is_empty())
            .map(|text| SelectionCapture { text, anchor: None }))
    }

    fn send_ctrl_insert() -> Result<(), String> {
        for _ in 0..MODIFIER_RELEASE_ATTEMPTS {
            if modifiers_released() {
                break;
            }
            thread::sleep(MODIFIER_RELEASE_DELAY);
        }
        if !modifiers_released() {
            return Err("Release the shortcut keys, then try again.".to_owned());
        }

        let inputs = ctrl_insert_inputs();

        let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
        if sent == inputs.len() as u32 {
            Ok(())
        } else {
            Err("Windows blocked the fallback copy shortcut.".to_owned())
        }
    }

    fn modifiers_released() -> bool {
        [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
            .into_iter()
            .all(|modifier| unsafe { GetAsyncKeyState(i32::from(modifier.0)) } >= 0)
    }

    fn ctrl_insert_inputs() -> [INPUT; 4] {
        [
            key_input(VK_CONTROL, false),
            key_input(VK_INSERT, false),
            key_input(VK_INSERT, true),
            key_input(VK_CONTROL, true),
        ]
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
        let _clipboard = open_clipboard(None)?;
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
        let units =
            unsafe { slice::from_raw_parts(data.cast::<u16>(), byte_len / size_of::<u16>()) };
        let end = units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units.len());
        let text = String::from_utf16_lossy(&units[..end]);
        let _ = unsafe { GlobalUnlock(global) };
        Some(text)
    }

    fn open_clipboard(owner: Option<HWND>) -> Option<ClipboardOpenGuard> {
        for _ in 0..8 {
            if unsafe { OpenClipboard(owner) }.is_ok() {
                return Some(ClipboardOpenGuard);
            }
            thread::sleep(Duration::from_millis(10));
        }
        None
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
            .as_chunks::<4>()
            .0
            .iter()
            .filter_map(|&[left, top, width, height]| {
                let rect = SelectionRect {
                    left,
                    top,
                    width,
                    height,
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
        fn fallback_copy_does_not_synthesize_unrelated_modifier_releases() {
            let inputs = ctrl_insert_inputs();
            let keys = inputs.map(|input| unsafe { input.Anonymous.ki.wVk });
            assert_eq!(keys, [VK_CONTROL, VK_INSERT, VK_INSERT, VK_CONTROL]);
            assert_eq!(unsafe { inputs[2].Anonymous.ki.dwFlags }, KEYEVENTF_KEYUP);
            assert_eq!(unsafe { inputs[3].Anonymous.ki.dwFlags }, KEYEVENTF_KEYUP);
        }

        #[test]
        fn clipboard_fallback_requires_an_owner_window() {
            let error = capture_with_copy_shortcut(HWND(ptr::null_mut()), 0).unwrap_err();
            assert!(error.contains("Gloss window"));
        }

        #[test]
        fn embedded_object_placeholder_requests_copy_semantics() {
            assert!(needs_copy_fallback("any color you want \u{fffc}"));
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{SelectionCapture, SelectionRect, needs_copy_fallback};
    use objc2::{rc::autoreleasepool, runtime::ProtocolObject};
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardItem, NSPasteboardTypeString, NSPasteboardWriting, NSWorkspace,
    };
    use objc2_core_foundation::{
        CFGetTypeID, CFRange, CFRetained, CFString, CFType, CGPoint, CGRect,
    };
    use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventSource, CGEventSourceStateID};
    use objc2_foundation::{NSArray, NSData, NSString};
    use std::{
        ffi::c_void,
        ptr::{self, NonNull},
        thread,
        time::{Duration, Instant},
    };

    const MAX_ANCESTOR_DEPTH: usize = 64;
    const AX_MESSAGING_TIMEOUT: f32 = 0.3;
    const AX_CAPTURE_BUDGET: Duration = Duration::from_millis(1_200);
    const COPY_CAPTURE_BUDGET: Duration = Duration::from_millis(1_400);
    const COPY_RESULT_WAIT: Duration = Duration::from_millis(500);
    const POLL_DELAY: Duration = Duration::from_millis(20);
    const MAX_CLIPBOARD_ITEMS: usize = 256;
    const MAX_CLIPBOARD_FORMATS: usize = 1_024;
    const MAX_CLIPBOARD_BYTES: usize = 256 * 1024 * 1024;

    const AX_ERROR_SUCCESS: i32 = 0;
    const AX_ERROR_INVALID_UI_ELEMENT: i32 = -25_202;
    const AX_ERROR_CANNOT_COMPLETE: i32 = -25_204;
    const AX_ERROR_ATTRIBUTE_UNSUPPORTED: i32 = -25_205;
    const AX_ERROR_NOT_IMPLEMENTED: i32 = -25_208;
    const AX_ERROR_API_DISABLED: i32 = -25_211;
    const AX_ERROR_NO_VALUE: i32 = -25_212;
    const AX_ERROR_PARAMETERIZED_ATTRIBUTE_UNSUPPORTED: i32 = -25_213;
    const AX_VALUE_TYPE_CGRECT: u32 = 3;
    const AX_VALUE_TYPE_CF_RANGE: u32 = 4;

    const KEY_CODE_C: u16 = 8;
    const AX_FOCUSED_UI_ELEMENT: &str = "AXFocusedUIElement";
    const AX_PARENT: &str = "AXParent";
    const AX_SELECTED_TEXT: &str = "AXSelectedText";
    const AX_SELECTED_TEXT_RANGE: &str = "AXSelectedTextRange";
    const AX_BOUNDS_FOR_RANGE: &str = "AXBoundsForRange";
    const AX_SELECTED_TEXT_MARKER_RANGE: &str = "AXSelectedTextMarkerRange";
    const AX_BOUNDS_FOR_TEXT_MARKER_RANGE: &str = "AXBoundsForTextMarkerRange";

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> u8;
        fn AXUIElementGetTypeID() -> usize;
        fn AXUIElementCreateApplication(pid: i32) -> *const c_void;
        fn AXUIElementSetMessagingTimeout(element: *const c_void, timeout: f32) -> i32;
        fn AXUIElementCopyAttributeValue(
            element: *const c_void,
            attribute: *const c_void,
            value: *mut *const c_void,
        ) -> i32;
        fn AXUIElementCopyElementAtPosition(
            application: *const c_void,
            x: f32,
            y: f32,
            element: *mut *const c_void,
        ) -> i32;
        fn AXUIElementCopyParameterizedAttributeValue(
            element: *const c_void,
            attribute: *const c_void,
            parameter: *const c_void,
            value: *mut *const c_void,
        ) -> i32;
        fn AXValueGetTypeID() -> usize;
        fn AXValueCreate(value_type: u32, value: *const c_void) -> *const c_void;
        fn AXValueGetType(value: *const c_void) -> u32;
        fn AXValueGetValue(value: *const c_void, value_type: u32, output: *mut c_void) -> u8;
    }

    struct ClipboardFormat {
        data_type: String,
        data: Vec<u8>,
    }

    struct ClipboardItem {
        formats: Vec<ClipboardFormat>,
    }

    struct ClipboardSnapshot {
        items: Vec<ClipboardItem>,
    }

    impl ClipboardSnapshot {
        fn capture(pasteboard: &NSPasteboard, deadline: Instant) -> Result<Self, String> {
            let Some(source_items) = pasteboard.pasteboardItems() else {
                return Err("Could not retrieve the current clipboard items safely.".to_owned());
            };
            let item_count = source_items.count();
            if item_count > MAX_CLIPBOARD_ITEMS {
                return Err("The clipboard contains too many items to preserve safely.".to_owned());
            }

            let mut items = Vec::with_capacity(item_count);
            let mut format_count = 0usize;
            let mut byte_count = 0usize;
            for item_index in 0..item_count {
                ensure_before(deadline, "Clipboard preservation timed out")?;
                let source_item = source_items.objectAtIndex(item_index);
                let source_types = source_item.types();
                let mut formats = Vec::with_capacity(source_types.count());
                for type_index in 0..source_types.count() {
                    ensure_before(deadline, "Clipboard preservation timed out")?;
                    format_count += 1;
                    if format_count > MAX_CLIPBOARD_FORMATS {
                        return Err(
                            "The clipboard contains too many formats to preserve safely."
                                .to_owned(),
                        );
                    }
                    let data_type = source_types.objectAtIndex(type_index);
                    let Some(data) = source_item.dataForType(&data_type) else {
                        return Err(format!(
                            "Could not preserve clipboard format {}.",
                            data_type
                        ));
                    };
                    byte_count = byte_count.saturating_add(data.len());
                    if byte_count > MAX_CLIPBOARD_BYTES {
                        return Err(
                            "The clipboard is too large to preserve safely for selection capture."
                                .to_owned(),
                        );
                    }
                    let data = data.to_vec();
                    formats.push(ClipboardFormat {
                        data_type: data_type.to_string(),
                        data,
                    });
                }
                items.push(ClipboardItem { formats });
            }
            ensure_before(deadline, "Clipboard preservation timed out")?;
            Ok(Self { items })
        }

        fn restore_if_unchanged(
            &self,
            pasteboard: &NSPasteboard,
            expected_change_count: isize,
        ) -> Result<bool, String> {
            if pasteboard.changeCount() != expected_change_count {
                return Ok(false);
            }

            let mut restored_items = Vec::with_capacity(self.items.len());
            for source_item in &self.items {
                let item = NSPasteboardItem::new();
                for format in &source_item.formats {
                    let data_type = NSString::from_str(&format.data_type);
                    let data = NSData::with_bytes(&format.data);
                    if !item.setData_forType(&data, &data_type) {
                        return Err(format!(
                            "Could not prepare clipboard format {} for restoration.",
                            format.data_type
                        ));
                    }
                }
                restored_items.push(item);
            }

            if pasteboard.changeCount() != expected_change_count {
                return Ok(false);
            }

            let writing_items: Vec<&ProtocolObject<dyn NSPasteboardWriting>> = restored_items
                .iter()
                .map(|item| ProtocolObject::from_ref(&**item))
                .collect();
            let writing_items = NSArray::from_slice(&writing_items);
            pasteboard.clearContents();
            if !restored_items.is_empty() && !pasteboard.writeObjects(&writing_items) {
                return Err(
                    "Could not restore the clipboard after reading the selection.".to_owned(),
                );
            }
            Ok(true)
        }
    }

    pub(super) fn capture_selected_text(
        _clipboard_owner: isize,
    ) -> Result<SelectionCapture, String> {
        autoreleasepool(|_| capture_selected_text_inner())
    }

    fn capture_selected_text_inner() -> Result<SelectionCapture, String> {
        // This check deliberately does not prompt. The caller can present the actionable error
        // without causing a system dialog every time a shortcut is pressed.
        if unsafe { AXIsProcessTrusted() } == 0 {
            return Err(
                "Gloss needs Accessibility permission to read selected text. Enable Gloss in System Settings > Privacy & Security > Accessibility, then try again."
                    .to_owned(),
            );
        }

        let target_pid = frontmost_pid()
            .ok_or_else(|| "Couldn't find the app containing the selection.".to_owned())?;
        let ax_capture = capture_with_accessibility(target_pid);
        if let Ok(Some(capture)) = ax_capture.as_ref()
            && !needs_copy_fallback(&capture.text)
        {
            return Ok(capture.clone());
        }

        let fallback = capture_with_copy_shortcut(target_pid)?;
        if let Some(capture) = fallback {
            return Ok(capture);
        }
        if let Ok(Some(capture)) = ax_capture {
            return Ok(capture);
        }
        if let Err(error) = ax_capture {
            return Err(format!(
                "Couldn't read the selected text in this app. Accessibility reported: {error}"
            ));
        }
        Err("Couldn't read any selected text in this app.".to_owned())
    }

    fn frontmost_pid() -> Option<i32> {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|application| application.processIdentifier())
    }

    fn capture_with_accessibility(pid: i32) -> Result<Option<SelectionCapture>, String> {
        let deadline = Instant::now() + AX_CAPTURE_BUDGET;
        let application = create_ax_application(pid)?;

        if let Some(focused) = copy_ax_attribute(&application, AX_FOCUSED_UI_ELEMENT, deadline)?
            && let Some(capture) = capture_from_ancestor_chain(focused, deadline)?
        {
            return Ok(Some(capture));
        }

        ensure_before(deadline, "Accessibility selection capture timed out")?;
        let event = CGEvent::new(None)
            .ok_or_else(|| "Could not read the current pointer position.".to_owned())?;
        let location = CGEvent::location(Some(&event));
        if let Some(element) = copy_ax_element_at_position(&application, location, deadline)?
            && let Some(capture) = capture_from_ancestor_chain(element, deadline)?
        {
            return Ok(Some(capture));
        }
        Ok(None)
    }

    fn capture_from_ancestor_chain(
        mut element: CFRetained<CFType>,
        deadline: Instant,
    ) -> Result<Option<SelectionCapture>, String> {
        let mut best = None;
        for depth in 0..=MAX_ANCESTOR_DEPTH {
            ensure_before(deadline, "Accessibility selection capture timed out")?;
            if let Some(capture) = capture_from_ax_element(&element, deadline)? {
                best = Some(capture);
            }
            if depth == MAX_ANCESTOR_DEPTH {
                break;
            }
            element = match copy_ax_attribute(&element, AX_PARENT, deadline)? {
                Some(parent) => parent,
                None => break,
            };
        }
        Ok(best)
    }

    fn capture_from_ax_element(
        element: &CFType,
        deadline: Instant,
    ) -> Result<Option<SelectionCapture>, String> {
        let Some(value) = copy_ax_attribute(element, AX_SELECTED_TEXT, deadline)? else {
            return Ok(None);
        };
        let Ok(text) = value.downcast::<CFString>() else {
            return Ok(None);
        };
        let text = text.to_string();
        if text.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(SelectionCapture {
            text,
            anchor: selection_anchor(element, deadline),
        }))
    }

    fn selection_anchor(element: &CFType, deadline: Instant) -> Option<SelectionRect> {
        let range = copy_ax_attribute(element, AX_SELECTED_TEXT_RANGE, deadline)
            .ok()
            .flatten();
        if let Some(range) = range.as_deref() {
            if let Some(tail_range) = last_character_range(range)
                && let Some(bounds) =
                    copy_ax_parameterized(element, AX_BOUNDS_FOR_RANGE, &tail_range, deadline)
                        .ok()
                        .flatten()
                        .and_then(|value| ax_rect(&value))
                        .and_then(selection_rect_from_ax)
            {
                return Some(bounds);
            }
            if let Some(bounds) =
                copy_ax_parameterized(element, AX_BOUNDS_FOR_RANGE, range, deadline)
                    .ok()
                    .flatten()
                    .and_then(|value| ax_rect(&value))
                    .and_then(selection_rect_from_ax)
            {
                return Some(bounds);
            }
        }

        let marker_range = copy_ax_attribute(element, AX_SELECTED_TEXT_MARKER_RANGE, deadline)
            .ok()
            .flatten()?;
        copy_ax_parameterized(
            element,
            AX_BOUNDS_FOR_TEXT_MARKER_RANGE,
            &marker_range,
            deadline,
        )
        .ok()
        .flatten()
        .and_then(|value| ax_rect(&value))
        .and_then(selection_rect_from_ax)
    }

    fn last_character_range(range_value: &CFType) -> Option<CFRetained<CFType>> {
        if !is_ax_value(range_value)
            || unsafe { AXValueGetType(ax_raw_ref(range_value)) } != AX_VALUE_TYPE_CF_RANGE
        {
            return None;
        }
        let mut range = CFRange {
            location: 0,
            length: 0,
        };
        if unsafe {
            AXValueGetValue(
                ax_raw_ref(range_value),
                AX_VALUE_TYPE_CF_RANGE,
                (&mut range as *mut CFRange).cast(),
            )
        } == 0
            || range.length <= 0
        {
            return None;
        }
        let tail = CFRange {
            location: range.location.checked_add(range.length - 1)?,
            length: 1,
        };
        let raw =
            unsafe { AXValueCreate(AX_VALUE_TYPE_CF_RANGE, (&tail as *const CFRange).cast()) };
        retained_cf_type(raw)
    }

    fn ax_rect(value: &CFType) -> Option<CGRect> {
        if !is_ax_value(value)
            || unsafe { AXValueGetType(ax_raw_ref(value)) } != AX_VALUE_TYPE_CGRECT
        {
            return None;
        }
        let mut rect = CGRect::ZERO;
        (unsafe {
            AXValueGetValue(
                ax_raw_ref(value),
                AX_VALUE_TYPE_CGRECT,
                (&mut rect as *mut CGRect).cast(),
            )
        } != 0)
            .then_some(rect)
    }

    fn selection_rect_from_ax(rect: CGRect) -> Option<SelectionRect> {
        let right = rect.origin.x + rect.size.width;
        let bottom = rect.origin.y + rect.size.height;
        if !rect.origin.x.is_finite()
            || !rect.origin.y.is_finite()
            || !right.is_finite()
            || !bottom.is_finite()
            || rect.size.width <= 0.0
            || rect.size.height <= 0.0
        {
            return None;
        }

        Some(SelectionRect {
            left: rect.origin.x,
            top: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
        })
    }

    fn create_ax_application(pid: i32) -> Result<CFRetained<CFType>, String> {
        retained_cf_type(unsafe { AXUIElementCreateApplication(pid) })
            .ok_or_else(|| "Could not inspect the selected app with Accessibility.".to_owned())
    }

    fn copy_ax_attribute(
        element: &CFType,
        attribute: &str,
        deadline: Instant,
    ) -> Result<Option<CFRetained<CFType>>, String> {
        prepare_ax_call(element, deadline)?;
        let attribute = CFString::from_str(attribute);
        let mut value = ptr::null();
        let result = unsafe {
            AXUIElementCopyAttributeValue(ax_raw_ref(element), ax_raw_ref(&attribute), &mut value)
        };
        ax_copy_result(result, value)
    }

    fn copy_ax_element_at_position(
        application: &CFType,
        point: CGPoint,
        deadline: Instant,
    ) -> Result<Option<CFRetained<CFType>>, String> {
        prepare_ax_call(application, deadline)?;
        let mut value = ptr::null();
        let result = unsafe {
            AXUIElementCopyElementAtPosition(
                ax_raw_ref(application),
                point.x as f32,
                point.y as f32,
                &mut value,
            )
        };
        ax_copy_result(result, value)
    }

    fn copy_ax_parameterized(
        element: &CFType,
        attribute: &str,
        parameter: &CFType,
        deadline: Instant,
    ) -> Result<Option<CFRetained<CFType>>, String> {
        prepare_ax_call(element, deadline)?;
        let attribute = CFString::from_str(attribute);
        let mut value = ptr::null();
        let result = unsafe {
            AXUIElementCopyParameterizedAttributeValue(
                ax_raw_ref(element),
                ax_raw_ref(&attribute),
                ax_raw_ref(parameter),
                &mut value,
            )
        };
        ax_copy_result(result, value)
    }

    fn prepare_ax_call(element: &CFType, deadline: Instant) -> Result<(), String> {
        ensure_before(deadline, "Accessibility selection capture timed out")?;
        if CFGetTypeID(Some(element)) != unsafe { AXUIElementGetTypeID() } {
            return Err("Accessibility returned an invalid UI element.".to_owned());
        }
        let timeout = deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f32()
            .min(AX_MESSAGING_TIMEOUT);
        if timeout <= 0.0 {
            return Err("Accessibility selection capture timed out.".to_owned());
        }
        let result = unsafe { AXUIElementSetMessagingTimeout(ax_raw_ref(element), timeout) };
        match result {
            AX_ERROR_SUCCESS => Ok(()),
            AX_ERROR_API_DISABLED => {
                Err("macOS Accessibility access is disabled for Gloss.".to_owned())
            }
            code => Err(format!(
                "Could not set a safe Accessibility timeout (error {code})."
            )),
        }
    }

    fn is_ax_value(value: &CFType) -> bool {
        CFGetTypeID(Some(value)) == unsafe { AXValueGetTypeID() }
    }

    fn ax_copy_result(
        result: i32,
        value: *const c_void,
    ) -> Result<Option<CFRetained<CFType>>, String> {
        match result {
            AX_ERROR_SUCCESS => retained_cf_type(value)
                .map(Some)
                .ok_or_else(|| "Accessibility returned an empty value.".to_owned()),
            AX_ERROR_ATTRIBUTE_UNSUPPORTED
            | AX_ERROR_NO_VALUE
            | AX_ERROR_PARAMETERIZED_ATTRIBUTE_UNSUPPORTED
            | AX_ERROR_NOT_IMPLEMENTED
            | AX_ERROR_INVALID_UI_ELEMENT => Ok(None),
            AX_ERROR_CANNOT_COMPLETE => {
                Err("the selected app did not answer before the macOS timeout".to_owned())
            }
            AX_ERROR_API_DISABLED => Err("Accessibility access is disabled for Gloss".to_owned()),
            code => Err(format!("macOS Accessibility error {code}")),
        }
    }

    fn retained_cf_type(raw: *const c_void) -> Option<CFRetained<CFType>> {
        let raw = NonNull::new(raw.cast_mut().cast::<CFType>())?;
        // SAFETY: AX Copy/Create functions return a +1 retained Core Foundation object.
        Some(unsafe { CFRetained::from_raw(raw) })
    }

    fn ax_raw_ref(value: &CFType) -> *const c_void {
        (value as *const CFType).cast()
    }

    fn capture_with_copy_shortcut(pid: i32) -> Result<Option<SelectionCapture>, String> {
        let deadline = Instant::now() + COPY_CAPTURE_BUDGET;
        wait_for_modifier_release(deadline)?;
        if frontmost_pid() != Some(pid) {
            return Err(
                "The selected app changed before Gloss could copy the selection.".to_owned(),
            );
        }

        let pasteboard = NSPasteboard::generalPasteboard();
        let initial_change_count = pasteboard.changeCount();
        let snapshot = ClipboardSnapshot::capture(&pasteboard, deadline)?;
        ensure_before(deadline, "Selection copy timed out")?;
        if pasteboard.changeCount() != initial_change_count {
            return Err(
                "The clipboard changed while Gloss was preserving it; the newer clipboard content was left untouched."
                    .to_owned(),
            );
        }
        if frontmost_pid() != Some(pid) {
            return Err(
                "The selected app changed before Gloss could copy the selection.".to_owned(),
            );
        }
        if pasteboard.changeCount() != initial_change_count {
            return Err(
                "The clipboard changed before Gloss could copy the selection; the newer clipboard content was left untouched."
                    .to_owned(),
            );
        }
        if deadline.saturating_duration_since(Instant::now()) < COPY_RESULT_WAIT {
            return Err(
                "Clipboard preservation took too long, so Gloss did not send the copy shortcut."
                    .to_owned(),
            );
        }
        send_command_c(pid)?;

        let result_deadline = Instant::now() + COPY_RESULT_WAIT;
        let (copied_change_count, focus_changed_after_copy) = loop {
            if Instant::now() >= result_deadline {
                return Ok(None);
            }
            let change_count = pasteboard.changeCount();
            if change_count != initial_change_count {
                break (change_count, frontmost_pid() != Some(pid));
            }
            if frontmost_pid() != Some(pid) {
                return Err("The selected app changed before it provided copied text.".to_owned());
            }
            thread::sleep(
                POLL_DELAY.min(result_deadline.saturating_duration_since(Instant::now())),
            );
        };

        if initial_change_count.checked_add(1) != Some(copied_change_count) {
            return Err(
                "The clipboard changed more than once while Gloss was copying the selection; it was left untouched to avoid overwriting a newer copy."
                    .to_owned(),
            );
        }

        let string_type = unsafe { NSPasteboardTypeString };
        let text = pasteboard
            .stringForType(string_type)
            .map(|value| value.to_string());
        let count_after_read = pasteboard.changeCount();
        if count_after_read != copied_change_count {
            return Err(
                "The clipboard changed while Gloss was reading the selection; the newer clipboard content was left untouched."
                    .to_owned(),
            );
        }

        if focus_changed_after_copy || frontmost_pid() != Some(pid) {
            return Err(
                "The selected app changed while Gloss was copying the selection; the clipboard was left untouched to avoid overwriting a newer copy."
                    .to_owned(),
            );
        }
        snapshot.restore_if_unchanged(&pasteboard, copied_change_count)?;

        Ok(text
            .filter(|value| !value.trim().is_empty())
            .map(|text| SelectionCapture { text, anchor: None }))
    }

    fn wait_for_modifier_release(deadline: Instant) -> Result<(), String> {
        loop {
            if modifiers_released() {
                return Ok(());
            }
            ensure_before(deadline, "Release the shortcut keys, then try again")?;
            thread::sleep(POLL_DELAY.min(deadline.saturating_duration_since(Instant::now())));
        }
    }

    fn modifiers_released() -> bool {
        let flags = CGEventSource::flags_state(CGEventSourceStateID::CombinedSessionState);
        !flags.intersects(
            CGEventFlags::MaskShift
                | CGEventFlags::MaskControl
                | CGEventFlags::MaskAlternate
                | CGEventFlags::MaskCommand,
        )
    }

    fn send_command_c(pid: i32) -> Result<(), String> {
        let key_down = CGEvent::new_keyboard_event(None, KEY_CODE_C, true)
            .ok_or_else(|| "Could not create the macOS copy shortcut.".to_owned())?;
        let key_up = CGEvent::new_keyboard_event(None, KEY_CODE_C, false)
            .ok_or_else(|| "Could not create the macOS copy shortcut.".to_owned())?;
        CGEvent::set_flags(Some(&key_down), CGEventFlags::MaskCommand);
        CGEvent::set_flags(Some(&key_up), CGEventFlags::MaskCommand);
        CGEvent::post_to_pid(pid, Some(&key_down));
        CGEvent::post_to_pid(pid, Some(&key_up));
        Ok(())
    }

    fn ensure_before(deadline: Instant, message: &str) -> Result<(), String> {
        if Instant::now() < deadline {
            Ok(())
        } else {
            Err(format!("{message}."))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn invalid_ax_rect_uses_cursor_fallback() {
            let rect = CGRect {
                origin: CGPoint { x: 10.0, y: 20.0 },
                size: objc2_core_foundation::CGSize {
                    width: 0.0,
                    height: 12.0,
                },
            };
            assert!(selection_rect_from_ax(rect).is_none());
        }

        #[test]
        fn embedded_object_placeholder_requests_copy_semantics() {
            assert!(needs_copy_fallback("text \u{fffc}"));
            assert!(needs_copy_fallback("text \u{fffd}"));
            assert!(!needs_copy_fallback("ordinary text"));
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::SelectionCapture;

    pub(super) fn capture_selected_text(
        _clipboard_owner: isize,
    ) -> Result<SelectionCapture, String> {
        Err("Selection capture is only supported on Windows and macOS.".to_owned())
    }
}
