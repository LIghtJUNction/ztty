//! Win32 ABI declarations used by the native frontend.

use super::*;
#[repr(C)]
#[derive(Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}
#[repr(C)]
#[derive(Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WindowPos {
    pub hwnd: Handle,
    pub insert_after: Handle,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub flags: u32,
}
#[repr(C)]
pub struct NcCalcSizeParams {
    pub rects: [Rect; 3],
    pub position: *mut WindowPos,
}
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(core::mem::size_of::<NcCalcSizeParams>() == 56);
    assert!(core::mem::offset_of!(NcCalcSizeParams, position) == 48);
};
#[repr(C)]
pub struct WindowClass {
    pub size: u32,
    pub style: u32,
    pub procedure: Option<unsafe extern "system" fn(Handle, u32, usize, isize) -> isize>,
    pub class_extra: i32,
    pub window_extra: i32,
    pub instance: Handle,
    pub icon: Handle,
    pub cursor: Handle,
    pub background: Handle,
    pub menu: *const u16,
    pub name: *const u16,
    pub small_icon: Handle,
}
#[repr(C)]
pub struct Message {
    pub hwnd: Handle,
    pub message: u32,
    pub wparam: usize,
    pub lparam: isize,
    pub time: u32,
    pub point: Point,
    pub private: u32,
}
#[repr(C)]
pub struct PaintStruct {
    pub dc: Handle,
    pub erase: i32,
    pub paint: Rect,
    pub restore: i32,
    pub update: i32,
    pub reserved: [u8; 32],
}
#[repr(C)]
pub struct BitmapInfoHeader {
    pub size: u32,
    pub width: i32,
    pub height: i32,
    pub planes: u16,
    pub bits: u16,
    pub compression: u32,
    pub image_size: u32,
    pub x_ppm: i32,
    pub y_ppm: i32,
    pub colors: u32,
    pub important: u32,
}
#[repr(C)]
pub struct BitmapInfo {
    pub header: BitmapInfoHeader,
    pub palette: [u32; 1],
}
#[repr(C)]
pub struct Bitmap {
    pub kind: i32,
    pub width: i32,
    pub height: i32,
    pub width_bytes: i32,
    pub planes: u16,
    pub bits_per_pixel: u16,
    pub bits: *mut c_void,
}
#[repr(C)]
pub struct TextMetric {
    pub height: i32,
    pub ascent: i32,
    pub descent: i32,
    pub internal_leading: i32,
    pub external_leading: i32,
    pub average_width: i32,
    pub max_width: i32,
    pub weight: i32,
    pub overhang: i32,
    pub aspect_x: i32,
    pub aspect_y: i32,
    pub first: u16,
    pub last: u16,
    pub default: u16,
    pub break_char: u16,
    pub italic: u8,
    pub underlined: u8,
    pub struck_out: u8,
    pub pitch: u8,
    pub charset: u8,
}
#[repr(C)]
pub struct CompositionForm {
    pub style: u32,
    pub position: Point,
    pub area: Rect,
}
#[link(name = "user32")]
unsafe extern "system" {
    pub fn SetProcessDpiAwarenessContext(context: Handle) -> i32;
    pub fn GetDpiForSystem() -> u32;
    pub fn GetDpiForWindow(hwnd: Handle) -> u32;
    pub fn RegisterClassExW(class: *const WindowClass) -> u16;
    pub fn UnregisterClassW(name: *const u16, instance: Handle) -> i32;
    pub fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Handle,
        menu: Handle,
        instance: Handle,
        parameter: *mut c_void,
    ) -> Handle;
    pub fn DefWindowProcW(hwnd: Handle, message: u32, wparam: usize, lparam: isize) -> isize;
    pub fn SetWindowLongPtrW(hwnd: Handle, index: i32, value: isize) -> isize;
    pub fn GetWindowLongPtrW(hwnd: Handle, index: i32) -> isize;
    pub fn AdjustWindowRectEx(rect: *mut Rect, style: u32, menu: i32, ex: u32) -> i32;
    pub fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
    pub fn ShowWindow(hwnd: Handle, show: i32) -> i32;
    pub fn DestroyWindow(hwnd: Handle) -> i32;
    pub fn GetClientRect(hwnd: Handle, rect: *mut Rect) -> i32;
    pub fn GetWindowRect(hwnd: Handle, rect: *mut Rect) -> i32;
    pub fn GetUpdateRect(hwnd: Handle, rect: *mut Rect, erase: i32) -> i32;
    pub fn GetDC(hwnd: Handle) -> Handle;
    pub fn ReleaseDC(hwnd: Handle, dc: Handle) -> i32;
    pub fn InvalidateRect(hwnd: Handle, rect: *const Rect, erase: i32) -> i32;
    pub fn BeginPaint(hwnd: Handle, paint: *mut PaintStruct) -> Handle;
    pub fn EndPaint(hwnd: Handle, paint: *const PaintStruct) -> i32;
    pub fn SetWindowTextW(hwnd: Handle, text: *const u16) -> i32;
    pub fn SetWindowPos(
        hwnd: Handle,
        after: Handle,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
    pub fn PeekMessageW(
        message: *mut Message,
        hwnd: Handle,
        min: u32,
        max: u32,
        remove: u32,
    ) -> i32;
    pub fn TranslateMessage(message: *const Message) -> i32;
    pub fn DispatchMessageW(message: *const Message) -> isize;
    pub fn PostQuitMessage(code: i32);
    pub fn GetKeyState(vk: i32) -> i16;
    pub fn MapVirtualKeyW(code: u32, map: u32) -> u32;
    pub fn SetCapture(hwnd: Handle) -> Handle;
    pub fn ReleaseCapture() -> i32;
    pub fn ScreenToClient(hwnd: Handle, point: *mut Point) -> i32;
    pub fn OpenClipboard(hwnd: Handle) -> i32;
    pub fn EmptyClipboard() -> i32;
    pub fn SetClipboardData(format: u32, memory: Handle) -> Handle;
    pub fn GetClipboardData(format: u32) -> Handle;
    pub fn CloseClipboard() -> i32;
}
#[link(name = "gdi32")]
unsafe extern "system" {
    pub fn CreateCompatibleDC(dc: Handle) -> Handle;
    pub fn DeleteDC(dc: Handle) -> i32;
    pub fn SelectObject(dc: Handle, object: Handle) -> Handle;
    pub fn DeleteObject(object: Handle) -> i32;
    pub fn CreateDIBSection(
        dc: Handle,
        info: *const BitmapInfo,
        usage: u32,
        bits: *mut *mut c_void,
        section: Handle,
        offset: u32,
    ) -> Handle;
    pub fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike: u32,
        charset: u32,
        out: u32,
        clip: u32,
        quality: u32,
        pitch: u32,
        face: *const u16,
    ) -> Handle;
    pub fn GetTextMetricsW(dc: Handle, metrics: *mut TextMetric) -> i32;
    pub fn SetBkMode(dc: Handle, mode: i32) -> i32;
    pub fn SetTextColor(dc: Handle, color: u32) -> u32;
    pub fn ExtTextOutW(
        dc: Handle,
        x: i32,
        y: i32,
        options: u32,
        rect: *const Rect,
        text: *const u16,
        count: u32,
        advances: *const i32,
    ) -> i32;
    pub fn BitBlt(
        target: Handle,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        source: Handle,
        sx: i32,
        sy: i32,
        operation: u32,
    ) -> i32;
    pub fn GdiFlush() -> i32;
    pub fn GetCurrentObject(dc: Handle, kind: u32) -> Handle;
    pub fn GetObjectW(object: Handle, bytes: i32, result: *mut c_void) -> i32;
    pub fn GetPixel(dc: Handle, x: i32, y: i32) -> u32;
    pub fn GetClipBox(dc: Handle, rect: *mut Rect) -> i32;
    pub fn GetDCOrgEx(dc: Handle, point: *mut Point) -> i32;
    pub fn SaveDC(dc: Handle) -> i32;
    pub fn RestoreDC(dc: Handle, saved: i32) -> i32;
    pub fn IntersectClipRect(dc: Handle, left: i32, top: i32, right: i32, bottom: i32) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> Handle;
    pub fn GetCurrentProcessId() -> u32;
    pub fn ReadProcessMemory(
        process: Handle,
        source: *const c_void,
        target: *mut c_void,
        size: usize,
        read: *mut usize,
    ) -> i32;
    pub fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        attributes: u32,
        template: Handle,
    ) -> Handle;
    pub fn WriteFile(
        file: Handle,
        data: *const c_void,
        size: u32,
        written: *mut u32,
        overlapped: Handle,
    ) -> i32;
    pub fn CloseHandle(handle: Handle) -> i32;
    pub fn GlobalAlloc(flags: u32, size: usize) -> Handle;
    pub fn GlobalLock(memory: Handle) -> Handle;
    pub fn GlobalUnlock(memory: Handle) -> i32;
    pub fn GlobalSize(memory: Handle) -> usize;
    pub fn GlobalFree(memory: Handle) -> Handle;
}
#[link(name = "shell32")]
unsafe extern "system" {
    pub fn ShellExecuteW(
        hwnd: Handle,
        verb: *const u16,
        file: *const u16,
        arguments: *const u16,
        directory: *const u16,
        show: i32,
    ) -> Handle;
}
#[link(name = "dwmapi")]
unsafe extern "system" {
    pub fn DwmSetWindowAttribute(
        hwnd: Handle,
        attribute: u32,
        value: *const c_void,
        size: u32,
    ) -> i32;
}
#[link(name = "imm32")]
unsafe extern "system" {
    pub fn ImmGetContext(hwnd: Handle) -> Handle;
    pub fn ImmReleaseContext(hwnd: Handle, context: Handle) -> i32;
    pub fn ImmSetCompositionWindow(context: Handle, form: *const CompositionForm) -> i32;
}
