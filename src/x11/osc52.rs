//! OSC 52 reads use a short-lived InputOnly requestor. A cancelled transfer
//! cannot be confused with a later ordinary paste or a new clipboard query.
use super::*;
impl Window {
    fn selection_requestor(&self) -> Xid {
        if self.osc_requestor != 0 {
            self.osc_requestor
        } else {
            self.id
        }
    }
    fn close_osc_requestor(&mut self) {
        if self.osc_requestor != 0 {
            unsafe {
                XDestroyWindow(self.display, self.osc_requestor);
            }
            self.osc_requestor = 0;
        }
    }
    pub fn accepts_event(&self, window: Xid, kind: c_int) -> bool {
        window == self.id
            || self.osc_requestor != 0
                && window == self.osc_requestor
                && matches!(kind, SELECTION_NOTIFY | PROPERTY_NOTIFY)
    }
    pub fn paste_pending(&self) -> bool {
        self.paste_selection != 0
    }
    pub fn clipboard_available(&self, primary: bool) -> bool {
        self.id != 0
            && unsafe {
                XGetSelectionOwner(self.display, if primary { 1 } else { self.clipboard }) != 0
            }
    }
    pub fn read_osc_clipboard(&mut self, primary: bool) -> bool {
        self.cancel_paste();
        if self.id == 0 {
            return false;
        }
        let requestor = unsafe {
            XCreateWindow(
                self.display,
                self.id,
                0,
                0,
                1,
                1,
                0,
                0,
                2,
                core::ptr::null_mut(),
                0,
                core::ptr::null_mut(),
            )
        };
        if requestor == 0 {
            return false;
        }
        self.osc_requestor = requestor;
        self.paste_limit = zterm::MAX_CLIPBOARD_BYTES;
        self.paste_selection = if primary { 1 } else { self.clipboard };
        self.paste_target = self.utf8;
        unsafe {
            XSelectInput(self.display, requestor, 1 << 22);
            XConvertSelection(
                self.display,
                self.paste_selection,
                self.utf8,
                self.transfer,
                requestor,
                0,
            );
            XFlush(self.display);
        }
        true
    }
}
