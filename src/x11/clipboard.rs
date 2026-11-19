//! X11 selection ownership and bounded incremental transfers.
use super::*;

impl Window {
    pub fn copy(&mut self, text: String, primary: bool) {
        let selection = if primary {
            self.primary_text = text;
            1
        } else {
            self.clipboard_text = text;
            self.clipboard
        };
        unsafe {
            XSetSelectionOwner(self.display, selection, self.id, 0);
            XFlush(self.display);
        }
    }
    pub fn cancel_paste(&mut self) {
        self.receiving = false;
        self.incoming.clear();
        self.incoming_type = 0;
        self.paste_selection = 0;
        self.paste_target = 0;
        self.paste_error = None;
        if self.selection_requestor() != 0 {
            unsafe {
                XDeleteProperty(self.display, self.selection_requestor(), self.transfer);
            }
        }
        self.close_osc_requestor();
        self.paste_limit = MAX_SELECTION_BYTES;
    }

    pub fn paste(&mut self, primary: bool) {
        self.cancel_paste();
        self.paste_selection = if primary { 1 } else { self.clipboard };
        self.paste_target = self.utf8;
        unsafe {
            XConvertSelection(
                self.display,
                self.paste_selection,
                self.utf8,
                self.transfer,
                self.selection_requestor(),
                0,
            );
            XFlush(self.display);
        }
    }
    pub fn selection_request(&self, request: SelectionRequest) {
        let mut notify = SelectionEvent {
            kind: SELECTION_NOTIFY,
            serial: 0,
            send_event: 1,
            display: self.display,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: 0,
            time: request.time,
        };
        let property = if request.property == 0 {
            request.target
        } else {
            request.property
        };
        unsafe {
            if request.target == self.targets {
                let targets = [self.targets, self.utf8, 31];
                XChangeProperty(
                    self.display,
                    request.requestor,
                    property,
                    4,
                    32,
                    0,
                    targets.as_ptr().cast(),
                    targets.len() as i32,
                );
                notify.property = property;
            } else if request.target == self.utf8 || request.target == 31 {
                let text = if request.selection == 1 {
                    &self.primary_text
                } else {
                    &self.clipboard_text
                };
                let latin: Vec<u8>;
                let data = if request.target == 31 {
                    latin = text
                        .chars()
                        .map(|c| if (c as u32) <= 255 { c as u8 } else { b'?' })
                        .collect();
                    &latin[..]
                } else {
                    text.as_bytes()
                };
                XChangeProperty(
                    self.display,
                    request.requestor,
                    property,
                    request.target,
                    8,
                    0,
                    data.as_ptr(),
                    0,
                );
                for chunk in data.chunks(32768) {
                    XChangeProperty(
                        self.display,
                        request.requestor,
                        property,
                        request.target,
                        8,
                        2,
                        chunk.as_ptr(),
                        chunk.len() as i32,
                    );
                }
                notify.property = property;
            }
            let mut event = Event { selection: notify };
            XSendEvent(self.display, request.requestor, 0, 0, &mut event);
            XFlush(self.display);
        }
    }
    pub fn take_paste_error(&mut self) -> Option<&'static str> {
        self.paste_error.take()
    }

    fn reject_paste(&mut self, error: &'static str) -> Option<Vec<u8>> {
        self.cancel_paste();
        self.paste_error = Some(error);
        None
    }

    fn finish_paste(&mut self, data: Vec<u8>, kind: Atom) -> Option<Vec<u8>> {
        self.receiving = false;
        self.incoming_type = 0;
        self.paste_selection = 0;
        self.paste_target = 0;
        self.close_osc_requestor();
        Some(if kind == 31 {
            data.into_iter()
                .map(char::from)
                .collect::<String>()
                .into_bytes()
        } else {
            data
        })
    }

    fn read_property(&self) -> Result<(Atom, Vec<u8>), &'static str> {
        let mut kind = 0;
        let mut format = 0;
        let mut count = 0;
        let mut remaining = 0;
        let mut data = core::ptr::null_mut();
        let result;
        unsafe {
            let code = XGetWindowProperty(
                self.display,
                self.selection_requestor(),
                self.transfer,
                0,
                (self.paste_limit / 4) as c_long,
                1,
                0,
                &mut kind,
                &mut format,
                &mut count,
                &mut remaining,
                &mut data,
            );
            result = if code != 0 || kind == 0 || (count != 0 && data.is_null()) {
                Err("Could not read clipboard")
            } else if remaining != 0 || count > self.paste_limit as c_ulong {
                Err("Paste exceeds the 4 MiB clipboard limit")
            } else if kind == self.incr {
                if format != 32 || count != 1 {
                    Err("Invalid clipboard transfer")
                } else if *data.cast::<c_ulong>() > self.paste_limit as c_ulong {
                    Err("Paste exceeds the 4 MiB clipboard limit")
                } else {
                    Ok((kind, Vec::new()))
                }
            } else if format != 8 || (kind != self.utf8 && kind != 31) {
                Err("Clipboard did not provide text")
            } else {
                let bytes = if count == 0 {
                    Vec::new()
                } else {
                    core::slice::from_raw_parts(data, count as usize).to_vec()
                };
                Ok((kind, bytes))
            };
            if !data.is_null() {
                XFree(data.cast());
            }
            if remaining != 0 {
                XDeleteProperty(self.display, self.selection_requestor(), self.transfer);
            }
        }
        result
    }
    pub fn selection_notify(&mut self, event: SelectionEvent) -> Option<Vec<u8>> {
        if self.paste_selection == 0
            || event.requestor != self.selection_requestor()
            || event.selection != self.paste_selection
            || event.target != self.paste_target
        {
            return None;
        }
        if event.property == 0 {
            if event.target == self.utf8 {
                self.paste_target = 31;
                unsafe {
                    XConvertSelection(
                        self.display,
                        self.paste_selection,
                        31,
                        self.transfer,
                        self.selection_requestor(),
                        0,
                    );
                    XFlush(self.display);
                }
            } else {
                return self.reject_paste("No text in clipboard");
            }
            return None;
        }
        if event.property != self.transfer {
            return None;
        }
        let (kind, data) = match self.read_property() {
            Ok(value) => value,
            Err(error) => return self.reject_paste(error),
        };
        if kind == self.incr {
            self.receiving = true;
            self.incoming.clear();
            unsafe {
                XDeleteProperty(self.display, self.selection_requestor(), self.transfer);
                XFlush(self.display);
            }
            None
        } else {
            self.finish_paste(data, kind)
        }
    }
    pub fn property(&mut self, event: PropertyEvent) -> Option<Vec<u8>> {
        if !self.receiving
            || event.window != self.selection_requestor()
            || event.atom != self.transfer
            || event.state != 0
        {
            return None;
        }
        let (kind, data) = match self.read_property() {
            Ok(value) => value,
            Err(error) => return self.reject_paste(error),
        };
        if kind == self.incr || (self.incoming_type != 0 && kind != self.incoming_type) {
            return self.reject_paste("Clipboard text format changed");
        }
        self.incoming_type = kind;
        if data.is_empty() {
            let complete = core::mem::take(&mut self.incoming);
            return self.finish_paste(complete, kind);
        }
        if self.incoming.len().saturating_add(data.len()) > self.paste_limit {
            return self.reject_paste("Paste exceeds the 4 MiB clipboard limit");
        }
        self.incoming.extend_from_slice(&data);
        unsafe {
            XFlush(self.display);
        }
        None
    }
}
