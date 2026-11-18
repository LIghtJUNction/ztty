//! PTY state shared by native frontends, without window or event-loop policy.
use crate::{
    input_queue::InputQueue,
    output_tail::{OutputChunk, OutputTail},
    session_control::SessionControl,
};
use alloc::string::String;
use zterm::Terminal;

#[derive(Debug, Eq, PartialEq)]
pub enum ReadStep {
    Data {
        bytes: usize,
        history_before: usize,
        history_after: usize,
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
}

impl SessionCore {
    pub fn new(pty: zpty::Pty, terminal: Terminal, owner: String, input_limit: usize) -> Self {
        Self {
            pty,
            terminal,
            input: InputQueue::new(input_limit),
            control: SessionControl::new(owner),
            output: OutputTail::new(),
            eof: false,
        }
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
        }
        Ok(changed)
    }

    pub fn release(&mut self, actor: &str) -> Result<(), &'static str> {
        self.control.release(actor)?;
        self.input.clear();
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
                return Ok(ReadStep::Eof);
            }
            Ok(bytes) => bytes,
            Err(error) if error.is_would_block() => return Ok(ReadStep::Pending),
            Err(error) => {
                self.eof = true;
                return Err(error);
            }
        };
        let history_before = self.terminal.history_len();
        self.terminal.feed(&buffer[..bytes]);
        self.output.append(&buffer[..bytes], persistent);
        Ok(ReadStep::Data {
            bytes,
            history_before,
            history_after: self.terminal.history_len(),
        })
    }

    /// Keep replies bounded. Preserve the frontends' existing reject-and-clear
    /// policy so a full input queue cannot accumulate terminal responses.
    pub fn queue_responses(&mut self) -> bool {
        let accepted = self.input.push(self.terminal.responses());
        self.terminal.clear_responses();
        accepted
    }

    pub fn flush_input(&mut self) -> Result<(), zpty::Error> {
        self.input.flush(&self.pty)
    }

    /// Commit the terminal grid only after the native PTY accepts the size.
    /// Retained EOF views may resize only their terminal in frontend code.
    pub fn resize(&mut self, size: zpty::Size) -> Result<(), zpty::Error> {
        self.pty.resize(size)?;
        self.terminal.resize(size.cols as usize, size.rows as usize);
        Ok(())
    }

    /// A font/DPI change commits cell metrics between the accepted PTY size
    /// and grid reflow, so graphics scroll with the new physical cell size.
    pub fn resize_with_cell_size(
        &mut self,
        size: zpty::Size,
        cell_size: (u32, u32),
    ) -> Result<(), zpty::Error> {
        self.pty.resize(size)?;
        self.terminal.set_cell_size(cell_size.0, cell_size.1);
        self.terminal.resize(size.cols as usize, size.rows as usize);
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
