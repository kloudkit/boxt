//! Renderers: terminal, PDF/HTML, JSON and JSON Schema.

#![forbid(unsafe_code)]

mod escape;
mod terminal;

pub use boxt_core;
pub use escape::escape;
pub use terminal::terminal;
