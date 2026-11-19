#![doc = include_str!("../README.md")]
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

/// Platform-independent Kitty and Sixel graphics decoding, storage and placement.
pub use zgraphics as graphics;
/// Platform-independent terminal input encoders.
pub use zinput as input;
/// Bounded native ZUI/1 scenes, component catalog and action intents.
pub use ztty_ui as ui;
mod grapheme;
mod screen;
mod shell;
mod terminal;

pub use grapheme::{CellChars, MAX_GRAPHEME_BYTES};
pub use screen::{
    BLINK, BOLD, Cell, DEFAULT_BG, DEFAULT_FG, DIM, EXPLICIT_BG, EXPLICIT_FG, HIDDEN, INVERSE,
    ITALIC, PALETTE_BG, PALETTE_FG, RAPID_BLINK, STRIKE, UNDERLINE,
};
pub use shell::{
    CommandRecord, CommandText, MAX_COMMAND_TEXT, MAX_COMMANDS, ShellPhase, TextPosition,
};
pub use terminal::{CursorShape, Modes, ResizeAnchor, Terminal, Theme};

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod shell_tests;

#[cfg(test)]
mod theme_tests;

#[cfg(test)]
mod sixel_tests;

#[cfg(test)]
mod wrap_tests;

#[cfg(test)]
mod ui_tests;

#[cfg(test)]
mod grapheme_tests;

#[cfg(test)]
mod blink_tests;

#[cfg(test)]
mod reflow_tests;
