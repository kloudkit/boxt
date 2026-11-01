//! Renderers: terminal, PDF/HTML, JSON and JSON Schema.

#![forbid(unsafe_code)]

mod escape;
mod layout;
mod terminal;

pub use boxt_core;
pub use escape::escape;
pub use terminal::{MAX_WIDTH, MIN_WIDTH, TerminalOptions, terminal};
