//! Cocoa clipboard I/O is synchronous; replies still check the control epoch.
use super::*;
pub(super) fn process(core: &mut SessionCore, visible: bool) {
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
        let pasteboard = send!(class(c"NSPasteboard"), c"generalPasteboard", Id);
        let kind = ns_string("public.utf8-plain-text");
        if let Some(text) = request.text.as_ref() {
            if selected.is_some() {
                let string = ns_string(text);
                if !string.is_null() {
                    send!(pasteboard, c"clearContents", isize);
                    send!(pasteboard,c"setString:forType:",bool,Id=>string,Id=>kind);
                    release(string);
                }
            }
        } else {
            let text = selected.map(|_| {
                let string = send!(pasteboard,c"stringForType:",Id,Id=>kind);
                text_from_object(string, zterm::MAX_CLIPBOARD_BYTES)
            });
            if core.owns("local") && core.control_epoch() == generation {
                let _ = core.input.push(&request.reply(
                    selected.unwrap_or(request.selectors()[0]),
                    text.as_ref().map(|s| s.as_bytes()),
                ));
            }
        }
        release(kind);
    }
}
