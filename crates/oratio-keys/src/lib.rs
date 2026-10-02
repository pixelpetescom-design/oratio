//! Global keyboard adapter: watches for the Ctrl+Win chord by polling key state
//! (no hook, no admin rights needed) and reports it through a callback.

use device_query::{DeviceQuery, DeviceState, Keycode};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};
use oratio_core::hotkey::ChordDetector;

const POLL: Duration = Duration::from_millis(8);
const HOLD_MS: u64 = 80;

fn is_ctrl(k: &Keycode) -> bool {
    matches!(k, Keycode::LControl | Keycode::RControl)
}

fn is_win(k: &Keycode) -> bool {
    matches!(k, Keycode::LMeta | Keycode::RMeta)
}

/// Starts watching on a background thread. `on_chord` runs each time Ctrl+Win is
/// pressed and returns whether the app claimed it (when it didn't, e.g. dictation is off,
/// the keys are left entirely to Windows); `on_failure` runs once if the watcher dies, so the app can say so.
pub fn spawn(on_chord: impl Fn() -> bool + Send + 'static, on_failure: impl FnOnce(String) + Send + 'static) {
    let started = std::thread::Builder::new().name("oratio-keys".into()).spawn(move || {
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let keyboard = DeviceState::new();
            let clock = Instant::now();
            let mut detector = ChordDetector::new(HOLD_MS);
            loop {
                std::thread::sleep(POLL);
                let keys = keyboard.get_keys();
                let (ctrl, win) = (keys.iter().any(is_ctrl), keys.iter().any(is_win));
                let other = keys.iter().any(|k| !is_ctrl(k) && !is_win(k));
                if detector.update(clock.elapsed().as_millis() as u64, ctrl, win, other) && on_chord() {
                    suppress_start_menu();
                }
            }
        }));
        if outcome.is_err() {
            on_failure("keyboard watcher stopped unexpectedly".into());
        }
    });
    if started.is_err() {
        // Nothing useful can be done here; the window's Start button still works.
    }
}

/// Releasing the Windows key opens the Start menu unless another key was pressed while
/// it was down. Tapping an unassigned key (0xFF, the same trick PowerToys uses) makes
/// Windows treat Win as "used" so our chord doesn't pop the menu open.
#[cfg(target_os = "windows")]
fn suppress_start_menu() {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};
    if let Ok(mut keyboard) = Enigo::new(&Settings::default()) {
        let _ = keyboard.key(Key::Other(0xFF), Direction::Click);
    }
}

/// Only Windows has a Start menu to suppress.
#[cfg(not(target_os = "windows"))]
fn suppress_start_menu() {}

/// Blocks until Ctrl and Win are both released, or `timeout` passes. Pasting while Win
/// is still down would send Win+V (clipboard history) instead of Ctrl+V.
pub fn wait_for_chord_release(timeout: Duration) {
    let keyboard = DeviceState::new();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let keys = keyboard.get_keys();
        if !keys.iter().any(|k| is_ctrl(k) || is_win(k)) {
            return;
        }
        std::thread::sleep(POLL);
    }
}
