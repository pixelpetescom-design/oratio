//! Pure domain logic for Oratio. No OS, audio, ML, database or UI dependencies:
//! adapters implement the ports defined here, and the shell wires them together.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod apps;
pub mod commands;
pub mod engine;
pub mod error;
pub mod history;
pub mod hotkey;
pub mod lexicon;
pub mod models;
pub mod overlay;
pub mod polish;
pub mod resample;
pub mod search;
pub mod segmenter;
pub mod session;
pub mod spelling;
pub mod stt;
#[cfg(test)]
pub mod testing;
pub mod vocab;

pub use error::CoreError;
