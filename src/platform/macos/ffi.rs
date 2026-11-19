//! Objective-C and Apple framework bindings with typed message helpers.
use super::{FdContext, Id, Rect, Sel, Size};
use alloc::string::String;
use core::ffi::{CStr, c_char, c_int, c_void};

#[link(name = "objc")]
unsafe extern "C" {
    pub(super) fn objc_getClass(name: *const c_char) -> Id;
    pub(super) fn objc_getProtocol(name: *const c_char) -> Id;
    pub(super) fn objc_allocateClassPair(parent: Id, name: *const c_char, extra: usize) -> Id;
    pub(super) fn objc_registerClassPair(class: Id);
    pub(super) fn class_addMethod(
        class: Id,
        selector: Sel,
        implementation: *const c_void,
        encoding: *const c_char,
    ) -> bool;
    pub(super) fn class_addProtocol(class: Id, protocol: Id) -> bool;
    pub(super) fn sel_registerName(name: *const c_char) -> Sel;
    pub(super) fn sel_isEqual(a: Sel, b: Sel) -> bool;
    pub(super) fn objc_msgSend();
    #[cfg(target_arch = "x86_64")]
    pub(super) fn objc_msgSend_stret();
}
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}
#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub(super) fn CFRelease(value: Id);
    pub(super) fn CFRetain(value: Id) -> Id;
    pub(super) fn CFStringCreateWithBytes(
        allocator: Id,
        bytes: *const u8,
        count: isize,
        encoding: u32,
        external: bool,
    ) -> Id;
    pub(super) fn CFDictionaryCreate(
        allocator: Id,
        keys: *const Id,
        values: *const Id,
        count: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> Id;
    pub(super) fn CFAttributedStringCreate(allocator: Id, string: Id, attributes: Id) -> Id;
    pub(super) static kCFBooleanTrue: Id;
    pub(super) static kCFTypeDictionaryKeyCallBacks: [usize; 6];
    pub(super) static kCFTypeDictionaryValueCallBacks: [usize; 5];
    pub(super) static kCFRunLoopCommonModes: Id;
    pub(super) static kCFRunLoopDefaultMode: Id;
    pub(super) fn CFRunLoopGetMain() -> Id;
    pub(super) fn CFRunLoopAddSource(run_loop: Id, source: Id, mode: Id);
    pub(super) fn CFRunLoopRemoveSource(run_loop: Id, source: Id, mode: Id);
    pub(super) fn CFFileDescriptorCreate(
        allocator: Id,
        fd: c_int,
        close_on_invalidate: bool,
        callback: unsafe extern "C" fn(Id, usize, Id),
        context: *const FdContext,
    ) -> Id;
    pub(super) fn CFFileDescriptorCreateRunLoopSource(allocator: Id, fd: Id, order: isize) -> Id;
    pub(super) fn CFFileDescriptorEnableCallBacks(fd: Id, types: usize);
    pub(super) fn CFFileDescriptorDisableCallBacks(fd: Id, types: usize);
    pub(super) fn CFFileDescriptorInvalidate(fd: Id);
}
#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    pub(super) static kCTFontAttributeName: Id;
    pub(super) static kCTForegroundColorFromContextAttributeName: Id;
    pub(super) fn CTFontCreateWithName(name: Id, size: f64, matrix: *const c_void) -> Id;
    pub(super) fn CTFontCreateCopyWithSymbolicTraits(
        font: Id,
        size: f64,
        matrix: *const c_void,
        value: u32,
        mask: u32,
    ) -> Id;
    pub(super) fn CTFontGetAscent(font: Id) -> f64;
    pub(super) fn CTFontGetDescent(font: Id) -> f64;
    pub(super) fn CTFontGetLeading(font: Id) -> f64;
    pub(super) fn CTFontGetGlyphsForCharacters(
        font: Id,
        characters: *const u16,
        glyphs: *mut u16,
        count: isize,
    ) -> bool;
    pub(super) fn CTFontGetAdvancesForGlyphs(
        font: Id,
        orientation: u32,
        glyphs: *const u16,
        advances: *mut Size,
        count: isize,
    ) -> f64;
    pub(super) fn CTLineCreateWithAttributedString(string: Id) -> Id;
    pub(super) fn CTLineDraw(line: Id, context: Id);
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub(super) fn CGContextSaveGState(context: Id);
    pub(super) fn CGContextRestoreGState(context: Id);
    pub(super) fn CGContextClearRect(context: Id, rect: Rect);
    pub(super) fn CGContextClipToRect(context: Id, rect: Rect);
    pub(super) fn CGContextSetRGBFillColor(
        context: Id,
        red: f64,
        green: f64,
        blue: f64,
        alpha: f64,
    );
    pub(super) fn CGContextFillRect(context: Id, rect: Rect);
    pub(super) fn CGContextSetTextPosition(context: Id, x: f64, y: f64);
    pub(super) fn CGContextSetShouldAntialias(context: Id, value: bool);
    pub(super) fn CGContextSetInterpolationQuality(context: Id, quality: i32);
    pub(super) fn CGContextDrawImage(context: Id, rect: Rect, image: Id);
    pub(super) static kCGColorSpaceSRGB: Id;
    pub(super) fn CGColorSpaceCreateWithName(name: Id) -> Id;
    pub(super) fn CGColorSpaceRelease(space: Id);
    pub(super) fn CGDataProviderCreateWithData(
        info: Id,
        data: *const c_void,
        bytes: usize,
        release: Option<unsafe extern "C" fn(Id, *const c_void, usize)>,
    ) -> Id;
    pub(super) fn CGDataProviderRelease(provider: Id);
    pub(super) fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: Id,
        bitmap_info: u32,
        provider: Id,
        decode: *const f64,
        interpolate: bool,
        intent: i32,
    ) -> Id;
    pub(super) fn CGImageCreateWithImageInRect(image: Id, rect: Rect) -> Id;
    pub(super) fn CGImageRelease(image: Id);
}

pub(super) fn selector(name: &CStr) -> Sel {
    unsafe { sel_registerName(name.as_ptr()) }
}

pub(super) fn class(name: &CStr) -> Id {
    unsafe { objc_getClass(name.as_ptr()) }
}

pub(super) fn release(object: Id) {
    if !object.is_null() {
        send!(object, c"release", ());
    }
}

pub(super) fn cf_release(object: Id) {
    if !object.is_null() {
        unsafe { CFRelease(object) };
    }
}

pub(super) fn rect0(object: Id, name: &CStr) -> Rect {
    #[cfg(target_arch = "x86_64")]
    {
        let f: unsafe extern "C" fn(*mut Rect, Id, Sel) =
            unsafe { core::mem::transmute(objc_msgSend_stret as *const ()) };
        let mut result = Rect::default();
        unsafe { f(&mut result, object, selector(name)) };
        result
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        send!(object, name, Rect)
    }
}

pub(super) fn rect_convert(object: Id, name: &CStr, rect: Rect, view: Option<Id>) -> Rect {
    #[cfg(target_arch = "x86_64")]
    {
        let mut result = Rect::default();
        if let Some(view) = view {
            let f: unsafe extern "C" fn(*mut Rect, Id, Sel, Rect, Id) =
                unsafe { core::mem::transmute(objc_msgSend_stret as *const ()) };
            unsafe { f(&mut result, object, selector(name), rect, view) };
        } else {
            let f: unsafe extern "C" fn(*mut Rect, Id, Sel, Rect) =
                unsafe { core::mem::transmute(objc_msgSend_stret as *const ()) };
            unsafe { f(&mut result, object, selector(name), rect) };
        }
        result
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        match view {
            Some(view) => send!(object, name, Rect, Rect => rect, Id => view),
            None => send!(object, name, Rect, Rect => rect),
        }
    }
}

pub(super) fn ns_string(text: &str) -> Id {
    let object = send!(class(c"NSString"), c"alloc", Id);
    send!(object, c"initWithBytes:length:encoding:", Id, *const u8 => text.as_ptr(), usize => text.len(), usize => 4)
}

pub(super) fn text_from_object(mut object: Id, limit: usize) -> String {
    if object.is_null() {
        return String::new();
    }
    if send!(object, c"isKindOfClass:", bool, Id => class(c"NSAttributedString")) {
        object = send!(object, c"string", Id);
    }
    let n = send!(object, c"lengthOfBytesUsingEncoding:", usize, usize => 4);
    if n > limit {
        return String::new();
    }
    let bytes = send!(object, c"UTF8String", *const u8);
    if bytes.is_null() {
        return String::new();
    }
    String::from_utf8_lossy(unsafe { core::slice::from_raw_parts(bytes, n) }).into_owned()
}
