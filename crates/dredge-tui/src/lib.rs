//! `dredge tui`: an interactive front end over `Engine`.
//!
//! The split mirrors the rest of dredge: [`state`] is a pure state machine
//! (`update(state, event) -> (state, effects)`), [`view`] draws a state into ratatui cells, and
//! [`runner`] is the thin shell that reads the terminal, calls `Engine` for every number shown,
//! and carries out effects. There is no query logic here, and nothing is ever deleted, moved or
//! executed: `y` only copies text to the clipboard.

pub mod runner;
pub mod state;
pub mod view;

pub use runner::{run, RunError};
