//! Tunables in one place. (A settings screen is out of scope for the MVP.)

use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};

/// Human-readable form of `toggle_shortcut`, shown in the UI.
pub const TOGGLE_LABEL: &str = "Ctrl + Shift + Space";

/// How long the "are you cancelling?" window lasts after the first Escape.
pub const CANCEL_GRACE_MS: u64 = 3_000;

/// How long the overlay lingers after a result so the user sees the confirmation.
pub const OVERLAY_LINGER_MS: u64 = 1_400;

pub const HISTORY_LIMIT: u32 = 500;
pub const MODEL_FILE: &str = "ggml-base.en-q5_1.bin";

pub fn toggle_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space)
}

pub fn escape_shortcut() -> Shortcut {
    Shortcut::new(None, Code::Escape)
}
