#![doc = include_str!("../README.md")]
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

/// Platform-independent terminal input encoders.
pub use zinput as input;
mod screen;
mod terminal;

pub use screen::{
    BOLD, Cell, DEFAULT_BG, DEFAULT_FG, DIM, HIDDEN, INVERSE, ITALIC, STRIKE, UNDERLINE,
};
pub use terminal::{Modes, Terminal};

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;
