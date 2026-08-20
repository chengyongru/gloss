use serde::Serialize;
use std::{ffi::c_void, ptr, slice};
use uiautomation::{
    UIAutomation, UIElement,
    patterns::{UITextPattern, UITextRange},
};
use windows::Win32::{
    System::{
        Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize, SAFEARRAY},
        Ole::{
            SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetDim, SafeArrayGetLBound,
            SafeArrayGetUBound, SafeArrayUnaccessData,
        },
    },
    UI::Accessibility::IUIAutomationTextRange,
};

const MAX_PARENT_DEPTH: usize = 8;

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
    // SAFETY: the blocking worker owns this COM initialization until this function returns.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|error| format!("Could not initialize Windows UI Automation: {error}"))?;
    let _com = ComGuard;

    let automation = UIAutomation::new_direct()
        .map_err(|error| format!("Could not start Windows UI Automation: {error}"))?;
    let focused = automation
        .get_focused_element()
        .map_err(|error| format!("Could not inspect the focused control: {error}"))?;
    let walker = automation
        .get_raw_view_walker()
        .map_err(|error| format!("Could not inspect the accessibility tree: {error}"))?;

    let mut element = focused;
    for depth in 0..=MAX_PARENT_DEPTH {
        if let Some(capture) = capture_from_element(&element)? {
            return Ok(capture);
        }
        if depth == MAX_PARENT_DEPTH {
            break;
        }
        element = match walker.get_parent(&element) {
            Ok(parent) => parent,
            Err(_) => break,
        };
    }

    Err("Couldn't read the selected text in this app.".to_owned())
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
}
