//! Keyboard adapter: presses Ctrl+V in whichever app has focus, so dictated text
//! lands where the user is working. Pasting is faster and more reliable across
//! apps than typing characters one by one.
//!
//! Limitation (Windows): input cannot be injected into windows running as
//! administrator unless Vox does too.

use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use vox_core::CoreError;

fn input_err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Input(e.to_string())
}

#[cfg(target_os = "windows")]
fn v_key() -> Key {
    Key::V
}

#[cfg(not(target_os = "windows"))]
fn v_key() -> Key {
    Key::Unicode('v')
}

/// Sends Ctrl+V. The text must already be on the clipboard.
pub fn paste_from_clipboard() -> Result<(), CoreError> {
    let mut keyboard = Enigo::new(&Settings::default()).map_err(input_err)?;
    // The hotkey's own modifiers may still be physically down; release them so the
    // target app sees a plain Ctrl+V rather than Ctrl+Shift+V.
    for key in [Key::Shift, Key::Alt, Key::Meta] {
        keyboard.key(key, Direction::Release).map_err(input_err)?;
    }
    keyboard.key(Key::Control, Direction::Press).map_err(input_err)?;
    let pasted = keyboard.key(v_key(), Direction::Click);
    // Always let go of Ctrl, even if the paste failed.
    let released = keyboard.key(Key::Control, Direction::Release);
    pasted.map_err(input_err)?;
    released.map_err(input_err)
}
