//! Pure domain logic for Vox. No OS, audio, ML, database or UI dependencies:
//! adapters implement the ports defined here, and the shell wires them together.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod engine;
pub mod error;
pub mod history;
pub mod hotkey;
pub mod polish;
pub mod resample;
pub mod segmenter;
pub mod session;
pub mod stt;

pub use error::CoreError;
