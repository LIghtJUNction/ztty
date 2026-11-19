//! Native OSC 52 clipboard bridge, independent of bracketed user paste.
use super::*;
pub(super) fn process(hwnd: ffi::Handle, core: &mut SessionCore, visible: bool) {
    if !visible || core.eof() || !core.owns("local") {
        core.deny_clipboard_requests();
        return;
    }
    while let Some(request) = core.terminal.next_clipboard_request() {
        let selected = request
            .selectors()
            .iter()
            .copied()
            .find(|s| matches!(s, b'c' | b's'));
        let generation = core.control_epoch();
        if let Some(text) = request.text.as_ref() {
            if selected.is_some() {
                write(hwnd, text);
            }
        } else {
            let text = selected.and_then(|_| read(hwnd));
            if core.owns("local") && core.control_epoch() == generation {
                let _ = core.input.push(&request.reply(
                    selected.unwrap_or(request.selectors()[0]),
                    text.as_ref().map(|s| s.as_bytes()),
                ));
            }
        }
    }
}
fn write(hwnd: ffi::Handle, text: &str) {
    if unsafe { ffi::OpenClipboard(hwnd) } == 0 {
        return;
    }
    let wide = wide(text);
    let memory = unsafe { ffi::GlobalAlloc(2, wide.len() * 2) };
    if !memory.is_null() {
        let target = unsafe { ffi::GlobalLock(memory) }.cast::<u16>();
        let mut transferred = false;
        if !target.is_null() {
            unsafe {
                ptr::copy_nonoverlapping(wide.as_ptr(), target, wide.len());
                ffi::GlobalUnlock(memory);
                if ffi::EmptyClipboard() != 0 {
                    transferred = !ffi::SetClipboardData(13, memory).is_null();
                }
            }
        }
        if !transferred {
            unsafe {
                ffi::GlobalFree(memory);
            }
        }
    }
    unsafe {
        ffi::CloseClipboard();
    }
}
fn read(hwnd: ffi::Handle) -> Option<String> {
    if unsafe { ffi::OpenClipboard(hwnd) } == 0 {
        return None;
    }
    let result = (|| {
        let memory = unsafe { ffi::GetClipboardData(13) };
        if memory.is_null() {
            return None;
        }
        let bytes = unsafe { ffi::GlobalSize(memory) };
        if bytes == 0 || !bytes.is_multiple_of(2) || bytes > 2 * (zterm::MAX_CLIPBOARD_BYTES + 1) {
            return None;
        }
        let data = unsafe { ffi::GlobalLock(memory) }.cast::<u16>();
        if data.is_null() {
            return None;
        }
        let text = (|| {
            let units = unsafe { core::slice::from_raw_parts(data, bytes / 2) };
            let length = units.iter().position(|&c| c == 0)?;
            let mut text = String::new();
            for scalar in core::char::decode_utf16(units[..length].iter().copied()) {
                let scalar = scalar.ok()?;
                if text.len() + scalar.len_utf8() > zterm::MAX_CLIPBOARD_BYTES {
                    return None;
                }
                text.push(scalar);
            }
            Some(text)
        })();
        unsafe {
            ffi::GlobalUnlock(memory);
        }
        text
    })();
    unsafe {
        ffi::CloseClipboard();
    }
    result
}
