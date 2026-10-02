//! Tunables in one place. (A settings screen is out of scope for the MVP.)

use tauri_plugin_global_shortcut::{Code, Shortcut};

/// The start/stop chord, shown in the UI. It is detected by `vox-keys` (modifier-only
/// combinations cannot be registered as OS hotkeys).
pub const TOGGLE_LABEL: &str = "Ctrl + Win";

/// How long the "are you cancelling?" window lasts after the first Escape.
pub const CANCEL_GRACE_MS: u64 = 3_000;

/// How long the overlay lingers after a result so the user sees the confirmation.
pub const OVERLAY_LINGER_MS: u64 = 1_400;

/// Keep the microphone open this long after Stop so the last word isn't clipped.
pub const TAIL_GRACE_MS: u64 = 150;

/// A dictation typed within this long of the previous one is assumed to continue the same text,
/// so it gets a leading space.
pub const CONTINUATION_WINDOW_MS: u64 = 3 * 60 * 1000;

/// Lets the clipboard settle before the paste keystroke is sent.
pub const PASTE_DELAY_MS: u64 = 50;

pub const HISTORY_LIMIT: u32 = 500;
/// With a GPU there is headroom for a far more accurate model; the CPU build stays small and quick.
#[cfg(feature = "gpu")]
pub const MODEL_FILE: &str = "ggml-large-v3-turbo-q5_0.bin";
#[cfg(not(feature = "gpu"))]
pub const MODEL_FILE: &str = "ggml-base.en-q5_1.bin";

pub fn escape_shortcut() -> Shortcut {
    Shortcut::new(None, Code::Escape)
}
