use alloc::collections::VecDeque;
use alloc::string::String;

/// Maximum number of prompt/command records retained by a terminal.
pub const MAX_COMMANDS: usize = 128;
/// Maximum UTF-8 bytes returned by one command-text request.
pub const MAX_COMMAND_TEXT: usize = 65536;

/// A position in primary-screen history. Lines do not change when history scrolls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPosition {
    pub line: u64,
    pub column: usize,
}

/// Shell-supplied OSC 133 boundaries. These are untrusted display metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRecord {
    pub id: u64,
    pub prompt: TextPosition,
    pub input: Option<TextPosition>,
    pub output: Option<TextPosition>,
    pub end: Option<TextPosition>,
    /// Absent when the shell did not report a status.
    pub exit_status: Option<u32>,
    /// False after screen operations make the original range unrecoverable.
    pub available: bool,
}

/// Rendered text from a command's input or output range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandText {
    pub text: String,
    /// History eviction, destructive screen editing, or the byte limit lost text.
    pub truncated: bool,
    /// The shell supplied an end marker or began its next primary prompt.
    pub complete: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShellPhase {
    #[default]
    Unknown,
    Prompt,
    Input,
    Output,
}

#[derive(Default)]
pub(crate) struct ShellIntegration {
    pub(crate) records: VecDeque<CommandRecord>,
    pub(crate) phase: ShellPhase,
    next_id: u64,
}

impl ShellIntegration {
    fn begin(&mut self, position: TextPosition) {
        if self.records.len() == MAX_COMMANDS {
            self.records.pop_front();
        }
        self.next_id = self.next_id.saturating_add(1);
        self.records.push_back(CommandRecord {
            id: self.next_id,
            prompt: position,
            input: None,
            output: None,
            end: None,
            exit_status: None,
            available: true,
        });
        self.phase = ShellPhase::Prompt;
    }

    pub(crate) fn clear(&mut self) {
        self.records.clear();
        self.phase = ShellPhase::Unknown;
    }

    pub(crate) fn invalidate_screen(&mut self, line: u64) {
        let start = TextPosition { line, column: 0 };
        for record in &mut self.records {
            if record.end.is_none_or(|end| end > start) {
                record.available = false;
            }
        }
    }

    pub(crate) fn mark(&mut self, fields: &[&[u8]], position: TextPosition) {
        let Some(mark) = fields.first() else { return };
        match *mark {
            b"A" => {
                // A secondary prompt belongs to the current multiline command.
                if fields[1..].contains(&b"k=s".as_slice()) {
                    return;
                }
                if self.phase == ShellPhase::Output
                    && let Some(record) = self.records.back_mut()
                {
                    record.end = Some(position);
                    self.phase = ShellPhase::Unknown;
                }
                if matches!(self.phase, ShellPhase::Prompt | ShellPhase::Input)
                    && let Some(record) = self.records.back_mut()
                {
                    // Prompt repaint/empty input must not consume command IDs.
                    record.prompt = position;
                    record.input = None;
                    record.available = true;
                    self.phase = ShellPhase::Prompt;
                } else {
                    self.begin(position);
                }
            }
            b"B" if matches!(self.phase, ShellPhase::Prompt | ShellPhase::Input) => {
                if let Some(record) = self.records.back_mut() {
                    record.input = Some(position);
                    self.phase = ShellPhase::Input;
                }
            }
            b"C" if self.phase != ShellPhase::Output => {
                if self.phase == ShellPhase::Unknown {
                    self.begin(position);
                }
                if let Some(record) = self.records.back_mut() {
                    record.output = Some(position);
                    self.phase = ShellPhase::Output;
                }
            }
            b"D" if self.phase == ShellPhase::Output => {
                let status = match fields.get(1) {
                    None | Some(&b"") => None,
                    Some(value) if value.iter().all(u8::is_ascii_digit) => {
                        let Some(status) = core::str::from_utf8(value)
                            .ok()
                            .and_then(|value| value.parse::<u32>().ok())
                        else {
                            return;
                        };
                        Some(status)
                    }
                    _ => return,
                };
                if let Some(record) = self.records.back_mut() {
                    record.end = Some(position);
                    record.exit_status = status;
                    self.phase = ShellPhase::Unknown;
                }
            }
            _ => {}
        }
    }
}
