//! AppKit window callbacks, class registration and application menu.
use super::{
    Id, Rect, Sel,
    ffi::*,
    host_ptr,
    input_events::{
        attributed_substring, character_index, command, first_rect, flags_changed, has_marked,
        insert_text, key_down, key_up, marked_range, mouse, selected_range, set_marked, unmark,
        valid_attributes, wheel,
    },
    wake,
};
use crate::os;
use core::ffi::CStr;
use zterm::input;

pub(super) unsafe extern "C" fn yes(_: Id, _: Sel) -> bool {
    true
}

pub(super) unsafe extern "C" fn no(_: Id, _: Sel) -> bool {
    false
}

pub(super) unsafe extern "C" fn yes_event(_: Id, _: Sel, _: Id) -> bool {
    true
}

pub(super) unsafe extern "C" fn draw_rect(_: Id, _: Sel, _: Rect) {
    let pointer = host_ptr();
    if !pointer.is_null() {
        unsafe { (&mut *pointer).draw() };
    }
}

pub(super) unsafe extern "C" fn window_close(_: Id, _: Sel, _: Id) -> bool {
    let pointer = host_ptr();
    if !pointer.is_null() {
        unsafe { (*pointer).running = false };
        wake();
    }
    true
}

pub(super) unsafe extern "C" fn quit(_: Id, _: Sel, _: Id) {
    let pointer = host_ptr();
    if !pointer.is_null() {
        unsafe { (*pointer).running = false };
        wake();
    }
}

pub(super) unsafe extern "C" fn window_resize(_: Id, _: Sel, _: Id) {
    let pointer = host_ptr();
    if !pointer.is_null() {
        let view = unsafe {
            (&mut *pointer).resize();
            (*pointer).view
        };
        let context = send!(view, c"inputContext", Id);
        send!(context, c"invalidateCharacterCoordinates", ());
        wake();
    }
}

pub(super) unsafe extern "C" fn window_focus(_: Id, selector: Sel, _: Id) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let focus = unsafe { sel_isEqual(selector, self::selector(c"windowDidBecomeKey:")) };
    unsafe {
        let host = &mut *pointer;
        host.focused = focus;
        host.blink_visible = true;
        host.blink_at = os::milliseconds() + host.appearance.cursor_blink_interval;
        host.force = true;
        let encoded = input::encode_focus(focus, &host.core.terminal.modes().input());
        host.queue(&encoded);
    }
    wake();
}

pub(super) fn add_method(
    class: Id,
    name: &CStr,
    function: *const (),
    encoding: &CStr,
) -> Result<(), &'static str> {
    if unsafe { class_addMethod(class, selector(name), function.cast(), encoding.as_ptr()) } {
        Ok(())
    } else {
        Err("cannot register native view methods")
    }
}

pub(super) fn register_classes() -> Result<(Id, Id), &'static str> {
    let view = unsafe { objc_allocateClassPair(class(c"NSView"), c"ZttyTerminalView".as_ptr(), 0) };
    let delegate =
        unsafe { objc_allocateClassPair(class(c"NSObject"), c"ZttyWindowDelegate".as_ptr(), 0) };
    if view.is_null() || delegate.is_null() {
        return Err("cannot register AppKit classes");
    }
    let protocol = unsafe { objc_getProtocol(c"NSTextInputClient".as_ptr()) };
    if protocol.is_null() || !unsafe { class_addProtocol(view, protocol) } {
        return Err("NSTextInputClient is unavailable");
    }
    add_method(view, c"acceptsFirstResponder", yes as *const (), c"c@:")?;
    add_method(view, c"acceptsFirstMouse:", yes_event as *const (), c"c@:@")?;
    add_method(view, c"isOpaque", no as *const (), c"c@:")?;
    add_method(
        view,
        c"drawRect:",
        draw_rect as *const (),
        c"v@:{CGRect={CGPoint=dd}{CGSize=dd}}",
    )?;
    add_method(view, c"keyDown:", key_down as *const (), c"v@:@")?;
    add_method(view, c"keyUp:", key_up as *const (), c"v@:@")?;
    add_method(view, c"flagsChanged:", flags_changed as *const (), c"v@:@")?;
    for name in [
        c"mouseDown:",
        c"mouseUp:",
        c"mouseDragged:",
        c"mouseMoved:",
        c"rightMouseDown:",
        c"rightMouseUp:",
        c"rightMouseDragged:",
        c"otherMouseDown:",
        c"otherMouseUp:",
        c"otherMouseDragged:",
    ] {
        add_method(view, name, mouse as *const (), c"v@:@")?;
    }
    add_method(view, c"scrollWheel:", wheel as *const (), c"v@:@")?;
    add_method(
        view,
        c"insertText:replacementRange:",
        insert_text as *const (),
        c"v@:@{_NSRange=QQ}",
    )?;
    add_method(
        view,
        c"setMarkedText:selectedRange:replacementRange:",
        set_marked as *const (),
        c"v@:@{_NSRange=QQ}{_NSRange=QQ}",
    )?;
    add_method(view, c"unmarkText", unmark as *const (), c"v@:")?;
    add_method(view, c"hasMarkedText", has_marked as *const (), c"c@:")?;
    add_method(
        view,
        c"markedRange",
        marked_range as *const (),
        c"{_NSRange=QQ}@:",
    )?;
    add_method(
        view,
        c"selectedRange",
        selected_range as *const (),
        c"{_NSRange=QQ}@:",
    )?;
    add_method(
        view,
        c"validAttributesForMarkedText",
        valid_attributes as *const (),
        c"@@:",
    )?;
    add_method(
        view,
        c"attributedSubstringForProposedRange:actualRange:",
        attributed_substring as *const (),
        c"@@:{_NSRange=QQ}^{_NSRange=QQ}",
    )?;
    add_method(
        view,
        c"characterIndexForPoint:",
        character_index as *const (),
        c"Q@:{CGPoint=dd}",
    )?;
    add_method(
        view,
        c"firstRectForCharacterRange:actualRange:",
        first_rect as *const (),
        c"{CGRect={CGPoint=dd}{CGSize=dd}}@:{_NSRange=QQ}^{_NSRange=QQ}",
    )?;
    add_method(view, c"doCommandBySelector:", command as *const (), c"v@::")?;
    add_method(
        delegate,
        c"windowShouldClose:",
        window_close as *const (),
        c"c@:@",
    )?;
    add_method(
        delegate,
        c"windowDidResize:",
        window_resize as *const (),
        c"v@:@",
    )?;
    add_method(
        delegate,
        c"windowDidChangeBackingProperties:",
        window_resize as *const (),
        c"v@:@",
    )?;
    add_method(
        delegate,
        c"windowDidBecomeKey:",
        window_focus as *const (),
        c"v@:@",
    )?;
    add_method(
        delegate,
        c"windowDidResignKey:",
        window_focus as *const (),
        c"v@:@",
    )?;
    add_method(delegate, c"quit:", quit as *const (), c"v@:@")?;
    unsafe {
        objc_registerClassPair(view);
        objc_registerClassPair(delegate);
    }
    Ok((view, delegate))
}

pub(super) fn menu(app: Id, target: Id) {
    let title = ns_string("ztty");
    let menubar = send!(send!(class(c"NSMenu"), c"alloc", Id), c"initWithTitle:", Id, Id => title);
    let application = send!(class(c"NSMenuItem"), c"new", Id);
    let submenu = send!(send!(class(c"NSMenu"), c"alloc", Id), c"initWithTitle:", Id, Id => title);
    let quit_title = ns_string("Quit ztty");
    let equivalent = ns_string("q");
    let quit_item = send!(send!(class(c"NSMenuItem"), c"alloc", Id), c"initWithTitle:action:keyEquivalent:", Id, Id => quit_title, Sel => selector(c"quit:"), Id => equivalent);
    send!(quit_item, c"setTarget:", (), Id => target);
    send!(submenu, c"addItem:", (), Id => quit_item);
    send!(application, c"setSubmenu:", (), Id => submenu);
    send!(menubar, c"addItem:", (), Id => application);
    send!(app, c"setMainMenu:", (), Id => menubar);
    for value in [
        quit_item,
        equivalent,
        quit_title,
        submenu,
        application,
        menubar,
        title,
    ] {
        release(value);
    }
}
