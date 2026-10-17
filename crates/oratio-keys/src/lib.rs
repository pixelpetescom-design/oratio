//! Global keyboard adapter: watches for the Ctrl+Win chord by polling key state
//! (no hook, no admin rights needed) and reports it through a callback.

use device_query::{DeviceQuery, DeviceState, Keycode};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};
use oratio_core::hotkey::{ChordDetector, ChordEvent, Trigger};
use oratio_core::search::SearchKey;

const POLL: Duration = Duration::from_millis(8);

fn is_ctrl(k: &Keycode) -> bool {
    matches!(k, Keycode::LControl | Keycode::RControl)
}

fn is_win(k: &Keycode) -> bool {
    matches!(k, Keycode::LMeta | Keycode::RMeta)
}

/// The physical key behind each id in `oratio_core::hotkey::KEYS`.
const KEY_TABLE: &[(&str, Keycode)] = &[
    ("left_ctrl", Keycode::LControl),
    ("right_ctrl", Keycode::RControl),
    ("f1", Keycode::F1),
    ("f2", Keycode::F2),
    ("f3", Keycode::F3),
    ("f4", Keycode::F4),
    ("f5", Keycode::F5),
    ("f6", Keycode::F6),
    ("f7", Keycode::F7),
    ("f8", Keycode::F8),
    ("f9", Keycode::F9),
    ("f10", Keycode::F10),
    ("f11", Keycode::F11),
    ("f12", Keycode::F12),
    ("f13", Keycode::F13),
    ("f14", Keycode::F14),
    ("f15", Keycode::F15),
    ("f16", Keycode::F16),
    ("f17", Keycode::F17),
    ("f18", Keycode::F18),
    ("f19", Keycode::F19),
    ("f20", Keycode::F20),
    ("insert", Keycode::Insert),
    ("home", Keycode::Home),
    ("end", Keycode::End),
    ("page_up", Keycode::PageUp),
    ("page_down", Keycode::PageDown),
    ("numpad_0", Keycode::Numpad0),
    ("numpad_1", Keycode::Numpad1),
    ("numpad_2", Keycode::Numpad2),
    ("numpad_3", Keycode::Numpad3),
    ("numpad_4", Keycode::Numpad4),
    ("numpad_5", Keycode::Numpad5),
    ("numpad_6", Keycode::Numpad6),
    ("numpad_7", Keycode::Numpad7),
    ("numpad_8", Keycode::Numpad8),
    ("numpad_9", Keycode::Numpad9),
    ("numpad_add", Keycode::NumpadAdd),
    ("numpad_subtract", Keycode::NumpadSubtract),
    ("numpad_multiply", Keycode::NumpadMultiply),
    ("numpad_divide", Keycode::NumpadDivide),
    ("numpad_decimal", Keycode::NumpadDecimal),
];

/// The physical key behind a single-key trigger (None for the Ctrl+Win chord).
fn key_of(trigger: Trigger) -> Option<Keycode> {
    match trigger {
        Trigger::CtrlWin => None,
        Trigger::Key(id) => KEY_TABLE.iter().find(|(key, _)| *key == id).map(|(_, code)| *code),
    }
}

/// Waits for the user to press one key on its own and returns it as a trigger, so they can choose "any key"
/// by pressing it. Fails if the key is not allowed, if Escape is pressed, or if nothing is pressed in time.
pub fn capture_trigger(timeout: Duration) -> Result<Trigger, String> {
    let keyboard = DeviceState::new();
    let deadline = Instant::now() + timeout;
    // Let go of whatever was held to click the button.
    while Instant::now() < deadline && !keyboard.get_keys().is_empty() {
        std::thread::sleep(POLL);
    }
    while Instant::now() < deadline {
        let keys = keyboard.get_keys();
        if keys.contains(&Keycode::Escape) {
            return Err("Cancelled.".into());
        }
        if let Some(first) = keys.first() {
            return KEY_TABLE
                .iter()
                .find(|(_, code)| code == first)
                .and_then(|(id, _)| Trigger::parse(id))
                .ok_or_else(|| "That key can’t be used, as it would get in the way of typing. Try an F key, Insert, Home, End, Page Up/Down, a numpad key or Ctrl.".to_string());
        }
        std::thread::sleep(POLL);
    }
    Err("No key was pressed.".into())
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
    trigger: impl Fn() -> Trigger + Send + 'static,
    search_key: impl Fn() -> Option<SearchKey> + Send + 'static,
    on_chord: impl Fn(bool) -> bool + Send + 'static,
    on_release: impl Fn(u64) + Send + 'static,
    on_failure: impl FnOnce(String) + Send + 'static,
) {
    let started = std::thread::Builder::new().name("oratio-keys".into()).spawn(move || {
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let keyboard = DeviceState::new();
            let clock = Instant::now();
            let mut active = trigger();
            let mut detector = ChordDetector::new(active.hold_ms());
            loop {
                std::thread::sleep(POLL);
                let wanted = trigger();
                if wanted != active {
                    (active, detector) = (wanted, ChordDetector::new(wanted.hold_ms()));
                }
                let keys = keyboard.get_keys();
                let search = search_key();
                let searching = search.is_some_and(|key| keys.iter().any(|k| is_search_key(k, key)));
                // The search key is allowed alongside the trigger; any other key still cancels it.
                let extra = |k: &Keycode| search.is_some_and(|key| is_search_key(k, key));
                let (held, other) = match key_of(active) {
                    None => (keys.iter().any(is_ctrl) && keys.iter().any(is_win), keys.iter().any(|k| !is_ctrl(k) && !is_win(k) && !extra(k))),
                    Some(only) => (keys.contains(&only), keys.iter().any(|k| *k != only && !extra(k))),
                };
                match detector.update(clock.elapsed().as_millis() as u64, held, other) {
                    Some(ChordEvent::Pressed) => {
                        // Only Ctrl+Win opens the Start menu on release; single keys have nothing to suppress.
                        if on_chord(searching) && active == Trigger::CtrlWin {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used)]
    fn every_allowed_trigger_has_a_physical_key() {
        for (id, _, _) in oratio_core::hotkey::KEYS {
            assert!(key_of(Trigger::parse(id).unwrap()).is_some(), "{id} has no key code");
        }
        assert_eq!(KEY_TABLE.len(), oratio_core::hotkey::KEYS.len());
    }
}
