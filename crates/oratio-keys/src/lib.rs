//! Global keyboard adapter: watches for the Ctrl+Win chord by polling key state
//! (no hook, no admin rights needed) and reports it through a callback.

use device_query::{DeviceQuery, DeviceState, Keycode};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};
use oratio_core::hotkey::{ChordDetector, ChordEvent};
use oratio_core::search::SearchKey;

const POLL: Duration = Duration::from_millis(8);
const HOLD_MS: u64 = 80;

fn is_ctrl(k: &Keycode) -> bool {
    matches!(k, Keycode::LControl | Keycode::RControl)
}

fn is_win(k: &Keycode) -> bool {
    matches!(k, Keycode::LMeta | Keycode::RMeta)
}

fn is_search_key(k: &Keycode, key: SearchKey) -> bool {
    match key {
        SearchKey::Shift => matches!(k, Keycode::LShift | Keycode::RShift),
        SearchKey::Alt => matches!(k, Keycode::LAlt | Keycode::RAlt),
    }
}

/// Starts watching on a background thread. `on_chord` runs each time Ctrl+Win is
/// pressed, told whether the voice-search key was also held, and returns whether the app claimed it
/// (when it didn't, e.g. dictation is off, the keys are left entirely to Windows). `search_key` says
/// which key currently means "search" (None = voice search off, so extra keys cancel the chord as
/// usual). `on_release` gets how many milliseconds the chord was held when it was let go. `on_failure` runs once if the watcher dies, so the app can say so.
pub fn spawn(
    search_key: impl Fn() -> Option<SearchKey> + Send + 'static,
    on_chord: impl Fn(bool) -> bool + Send + 'static,
    on_release: impl Fn(u64) + Send + 'static,
    on_failure: impl FnOnce(String) + Send + 'static,
) {
    let started = std::thread::Builder::new().name("oratio-keys".into()).spawn(move || {
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let keyboard = DeviceState::new();
            let clock = Instant::now();
            let mut detector = ChordDetector::new(HOLD_MS);
            loop {
                std::thread::sleep(POLL);
                let keys = keyboard.get_keys();
                let (ctrl, win) = (keys.iter().any(is_ctrl), keys.iter().any(is_win));
                let search = search_key();
                let searching = search.is_some_and(|key| keys.iter().any(|k| is_search_key(k, key)));
                // The search key is allowed alongside the chord; any other key still cancels it.
                let other = keys.iter().any(|k| !is_ctrl(k) && !is_win(k) && !search.is_some_and(|key| is_search_key(k, key)));
                match detector.update(clock.elapsed().as_millis() as u64, ctrl, win, other) {
                    Some(ChordEvent::Pressed) => {
                        if on_chord(searching) {
                            suppress_start_menu();
                        }
                    }
                    // Tells the app how long the chord was held, so it can treat a long hold as push-to-talk.
                    Some(ChordEvent::Released { held_ms }) => on_release(held_ms),
                    None => {}
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

/// Blocks until Ctrl, Win, Shift and Alt are all physically up, or `timeout` passes. Returns whether
/// they were released in time. Typing while Win is still down would send Win+V (clipboard history)
/// instead of Ctrl+V, so callers wait first rather than faking key-ups.
pub fn wait_for_modifiers_released(timeout: Duration) -> bool {
    let keyboard = DeviceState::new();
    let deadline = Instant::now() + timeout;
    let any_modifier = |keys: &[Keycode]| {
        keys.iter().any(|k| {
            is_ctrl(k) || is_win(k) || is_search_key(k, SearchKey::Shift) || is_search_key(k, SearchKey::Alt)
        })
    };
    while Instant::now() < deadline {
        if !any_modifier(&keyboard.get_keys()) {
            return true;
        }
        std::thread::sleep(POLL);
    }
    false
}
