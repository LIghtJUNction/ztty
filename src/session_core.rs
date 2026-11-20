//! PTY state shared by native frontends, without window or event-loop policy.
use crate::{
    input_queue::InputQueue,
    output_tail::{OutputChunk, OutputTail},
    session_control::SessionControl,
};
use alloc::string::String;
use alloc::vec::Vec;
use zterm::{ResizeAnchor, Terminal};

#[derive(Debug, Eq, PartialEq)]
pub enum ReadStep {
    Data {
        bytes: usize,
        history_before: usize,
        history_after: usize,
        /// Primary-screen rows moved into history, including evicted rows.
        history_scrolled: usize,
    },
    Pending,
    Eof,
}

pub struct SessionCore {
    pub pty: zpty::Pty,
    pub terminal: Terminal,
    pub input: InputQueue,
    control: SessionControl,
    output: OutputTail,
    eof: bool,
    control_epoch: u64,
}

/// Process-independent session state beside the terminal model. Raw descriptors
/// are transferred separately; creating this value performs no PTY I/O.
pub struct CoreCheckpoint {
    pub input: Vec<u8>,
    pub input_limit: usize,
    pub output: Vec<u8>,
    pub output_end: u64,
    pub owner: Option<String>,
    pub eof: bool,
    pub control_epoch: u64,
}

impl CoreCheckpoint {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.input_limit > 8 * 1024 * 1024 || self.input.len() > self.input_limit {
            return Err("reload input queue exceeds its budget");
        }
        if self.output.len() > 256 * 1024 || self.output.len() as u64 > self.output_end {
            return Err("reload output cursors are invalid");
        }
        if self
            .owner
            .as_deref()
            .is_some_and(|s| !crate::session_control::valid_actor(s))
        {
            return Err("reload input owner is invalid");
        }
        Ok(())
    }
}

impl SessionCore {
    pub fn checkpoint(&self) -> CoreCheckpoint {
        let (input, input_limit) = self.input.checkpoint();
        let (output, output_end) = self.output.checkpoint();
        CoreCheckpoint {
            input: input.to_vec(),
            input_limit,
            output,
            output_end,
            owner: self.owner().map(String::from),
            eof: self.eof,
            control_epoch: self.control_epoch,
        }
    }

    /// Restore already validated state using the inherited, uniquely owned PTY.
    /// The caller checks `state.validate()` before taking ownership of any FD.
    pub fn restore_checkpoint(
        pty: zpty::Pty,
        mut terminal: Terminal,
        state: CoreCheckpoint,
    ) -> Self {
        terminal.set_graphics_reader(crate::graphics_io::reader());
        let mut control =
            SessionControl::new(state.owner.clone().unwrap_or_else(|| "local".into()));
        if state.owner.is_none() {
            let _ = control.release("local");
        }
        Self {
            pty,
            terminal,
            input: InputQueue::restore_checkpoint(state.input, state.input_limit),
            control,
            output: OutputTail::restore_checkpoint(state.output, state.output_end),
            eof: state.eof,
            control_epoch: state.control_epoch,
        }
    }

    pub fn new(pty: zpty::Pty, mut terminal: Terminal, owner: String, input_limit: usize) -> Self {
        terminal.set_graphics_reader(crate::graphics_io::reader());
        Self {
            pty,
            terminal,
            input: InputQueue::new(input_limit),
            control: SessionControl::new(owner),
            output: OutputTail::new(),
            eof: false,
            control_epoch: 1,
        }
    }

    pub fn control_epoch(&self) -> u64 {
        self.control_epoch
    }

    pub fn owner(&self) -> Option<&str> {
        self.control.owner()
    }

    pub fn owns(&self, actor: &str) -> bool {
        self.control.owns(actor)
    }

    /// Cancel only input not yet accepted by the PTY. The frontend also clears
    /// its pending paste/key state when this returns true.
    pub fn acquire(&mut self, actor: &str, takeover: bool) -> Result<bool, &'static str> {
        let changed = self.control.acquire(actor, takeover)?;
        if changed {
            self.input.clear();
            self.control_epoch = self.control_epoch.wrapping_add(1);
            self.terminal.cancel_clipboard_requests();
        }
        Ok(changed)
    }

    pub fn release(&mut self, actor: &str) -> Result<(), &'static str> {
        self.control.release(actor)?;
        self.input.clear();
        self.control_epoch = self.control_epoch.wrapping_add(1);
        self.terminal.cancel_clipboard_requests();
        Ok(())
    }

    pub fn eof(&self) -> bool {
        self.eof
    }

    /// Feed one read at a time so each frontend can preserve its scroll anchor
    /// across history growth and clears, while keeping its own work budget.
    /// Reaping a child does not imply EOF: buffered output must still be read.
    pub fn read_once(
        &mut self,
        buffer: &mut [u8],
        persistent: bool,
    ) -> Result<ReadStep, zpty::Error> {
        if self.eof {
            return Ok(ReadStep::Eof);
        }
        if buffer.is_empty() {
            return Ok(ReadStep::Pending);
        }
        let bytes = match self.pty.read(buffer) {
            Ok(0) => {
                self.eof = true;
                self.terminal.cancel_clipboard_requests();
                self.terminal.reset_ui();
                return Ok(ReadStep::Eof);
            }
            Ok(bytes) => bytes,
            Err(error) if error.is_would_block() => return Ok(ReadStep::Pending),
            Err(error) => {
                self.eof = true;
                self.terminal.cancel_clipboard_requests();
                self.terminal.reset_ui();
                return Err(error);
            }
        };
        let history_before = self.terminal.history_len();
        let history_position = self.terminal.history_position();
        self.terminal.feed(&buffer[..bytes]);
        self.output.append(&buffer[..bytes], persistent);
        Ok(ReadStep::Data {
            bytes,
            history_before,
            history_after: self.terminal.history_len(),
            history_scrolled: usize::try_from(
                self.terminal
                    .history_position()
                    .saturating_sub(history_position),
            )
            .unwrap_or(usize::MAX),
        })
    }

    /// Keep replies bounded. Preserve the frontends' existing reject-and-clear
    /// policy so a full input queue cannot accumulate terminal responses.
    pub fn queue_responses(&mut self) -> bool {
        let accepted = self.input.push(self.terminal.responses());
        self.terminal.clear_responses();
        accepted
    }

    /// Headless/observing/non-local hosts answer queries without clipboard data.
    /// Writes are ignored; the queue never accumulates until a later attachment.
    pub fn deny_clipboard_requests(&mut self) {
        while let Some(request) = self.terminal.next_clipboard_request() {
            if request.text.is_none() {
                let _ = self
                    .input
                    .push(&request.reply(request.selectors()[0], None));
            }
        }
    }

    pub fn flush_input(&mut self) -> Result<(), zpty::Error> {
        self.input.flush(&self.pty)
    }

    /// Commit the terminal grid only after the native PTY accepts the size.
    /// Retained EOF views may resize only their terminal in frontend code.
    pub fn resize(&mut self, size: zpty::Size) -> Result<(), zpty::Error> {
        self.resize_anchored(size, &mut [])
    }

    /// As resize, remapping selection/viewport positions only after PTY accepts.
    pub fn resize_anchored(
        &mut self,
        size: zpty::Size,
        anchors: &mut [ResizeAnchor],
    ) -> Result<(), zpty::Error> {
        self.pty.resize(size)?;
        self.terminal
            .resize_anchored(size.cols as usize, size.rows as usize, anchors);
        Ok(())
    }

    /// A font/DPI change commits cell metrics between the accepted PTY size
    /// and grid reflow, so graphics scroll with the new physical cell size.
    pub fn resize_with_cell_size(
        &mut self,
        size: zpty::Size,
        cell_size: (u32, u32),
    ) -> Result<(), zpty::Error> {
        self.resize_with_cell_size_anchored(size, cell_size, &mut [])
    }

    pub fn resize_with_cell_size_anchored(
        &mut self,
        size: zpty::Size,
        cell_size: (u32, u32),
        anchors: &mut [ResizeAnchor],
    ) -> Result<(), zpty::Error> {
        self.pty.resize(size)?;
        self.terminal.set_cell_size(cell_size.0, cell_size.1);
        self.terminal
            .resize_anchored(size.cols as usize, size.rows as usize, anchors);
        Ok(())
    }

    pub fn read_output(
        &self,
        cursor: u64,
        max_bytes: Option<u64>,
    ) -> Result<OutputChunk, &'static str> {
        self.output.read(cursor, max_bytes, self.eof)
    }
}
