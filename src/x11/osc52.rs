//! OSC 52 reads use a short-lived InputOnly requestor. A cancelled transfer
//! cannot be confused with a later ordinary paste or a new clipboard query.
use super::*;
impl Window {
    pub(super) fn selection_requestor(&self) -> Xid {
        if self.osc_requestor != 0 {
            self.osc_requestor
        } else {
            self.state.id
        }
    }
    pub(super) fn close_osc_requestor(&mut self) {
        if self.osc_requestor != 0 {
            unsafe {
                XDestroyWindow(self.state.display, self.osc_requestor);
            }
            self.osc_requestor = 0;
        }
    }
    pub fn accepts_event(&self, window: Xid, kind: c_int) -> bool {
        window == self.state.id
            || self.osc_requestor != 0
                && window == self.osc_requestor
                && matches!(kind, SELECTION_NOTIFY | PROPERTY_NOTIFY)
    }
    pub fn paste_pending(&self) -> bool {
        self.paste_selection != 0
    }
    pub fn clipboard_available(&self, primary: bool) -> bool {
        self.state.id != 0
            && unsafe {
                XGetSelectionOwner(self.state.display, if primary { 1 } else { self.clipboard })
                    != 0
            }
    }
    pub fn read_osc_clipboard(&mut self, primary: bool) -> bool {
        self.cancel_paste();
        if self.state.id == 0 {
            return false;
        }
        let requestor = unsafe {
            XCreateWindow(
                self.state.display,
                self.state.id,
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
            XSelectInput(self.state.display, requestor, 1 << 22);
            XConvertSelection(
                self.state.display,
                self.paste_selection,
                self.utf8,
                self.transfer,
                requestor,
                0,
            );
            XFlush(self.state.display);
        }
        true
    }
}
