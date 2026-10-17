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

/// Waits for the user to press the keys they want (one key or several held together) and returns them as a
/// trigger. The keys are recorded until all of them are let go again. Fails if the combination can't be used,
/// if Escape is pressed, or if nothing is pressed in time.
pub fn capture_trigger(timeout: Duration) -> Result<Trigger, String> {
    let keyboard = DeviceState::new();
    let deadline = Instant::now() + timeout;
    // Let go of whatever was held to click the button.
    while Instant::now() < deadline && !keyboard.get_keys().is_empty() {
        std::thread::sleep(POLL);
    }
    let mut chosen: Vec<Keycode> = Vec::new();
    while Instant::now() < deadline {
        let keys = keyboard.get_keys();
        if keys.contains(&Keycode::Escape) {
            return Err("Cancelled.".into());
        }
        if keys.is_empty() && !chosen.is_empty() {
            let names: Vec<String> = chosen.iter().map(|k| format!("{k:?}")).collect();
            return Trigger::from_keys(&names);
        }
        // The combination is the most keys that were down at once.
        if keys.len() >= chosen.len() {
            chosen = keys;
        }
        std::thread::sleep(POLL);
    }
    Err("No key was pressed.".into())
}

/// The physical keys of a custom trigger (None for the Ctrl+Win chord). A name this keyboard layer doesn't
/// know yields an empty list, which never counts as held.
fn codes_of(trigger: &Trigger) -> Option<Vec<Keycode>> {
    match trigger {
        Trigger::CtrlWin => None,
        Trigger::Keys(names) => {
            let codes: Vec<Keycode> = names.iter().filter_map(|n| n.parse().ok()).collect();
            Some(if codes.len() == names.len() { codes } else { Vec::new() })
        }
    }
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
            let mut codes = codes_of(&active);
            loop {
                std::thread::sleep(POLL);
                let wanted = trigger();
                if wanted != active {
                    detector = ChordDetector::new(wanted.hold_ms());
                    codes = codes_of(&wanted);
                    active = wanted;
                }
                let keys = keyboard.get_keys();
                let search = search_key();
                let searching = search.is_some_and(|key| keys.iter().any(|k| is_search_key(k, key)));
                // The search key is allowed alongside the trigger; any other key still cancels it.
                let extra = |k: &Keycode| search.is_some_and(|key| is_search_key(k, key));
                let (held, other) = match &codes {
                    None => (keys.iter().any(is_ctrl) && keys.iter().any(is_win), keys.iter().any(|k| !is_ctrl(k) && !is_win(k) && !extra(k))),
                    Some(wanted) => (!wanted.is_empty() && wanted.iter().all(|k| keys.contains(k)), keys.iter().any(|k| !wanted.contains(k) && !extra(k))),
                };
                match detector.update(clock.elapsed().as_millis() as u64, held, other) {
                    Some(ChordEvent::Pressed) => {
                        // Only Ctrl+Win opens the Start menu on release; single keys have nothing to suppress.
                        if on_chord(searching) && matches!(active, Trigger::CtrlWin) {
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
    fn every_name_a_trigger_can_hold_is_a_real_key() {
        let t = Trigger::parse("keys:LControl+LAlt+PageDown").unwrap();
        assert_eq!(codes_of(&t), Some(vec![Keycode::LAlt, Keycode::LControl, Keycode::PageDown]));
        assert_eq!(codes_of(&Trigger::CtrlWin), None);
        for (_, names) in oratio_core::hotkey::PRESETS {
            assert!(names.iter().all(|n| n.parse::<Keycode>().is_ok()), "{names:?}");
        }
    }
}
