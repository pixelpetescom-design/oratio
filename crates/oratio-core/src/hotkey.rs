//! Detects the dictation trigger (Ctrl+Win by default, or a single key) from raw key state, as a pure function of time.
//!
//! Modifier-only combinations can't be registered as OS hotkeys, so the shell polls
//! the keyboard and feeds snapshots here. The chord fires once, after both keys have
//! been held briefly with nothing else pressed, so Ctrl+Win+Arrow (switch desktop)
//! and friends keep working. It re-arms only after the keys are released.

use crate::session::Millis;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Arming(Millis),
    /// Fired at this time; waiting for the keys to be released.
    Active(Millis),
    /// The keys disappeared at `lost_at`; a real release only if they stay gone (keys can flicker).
    Lost { fired_at: Millis, lost_at: Millis },
    /// Disqualified by another key; wait for release.
    Spent,
}

/// Key names are the ones the keyboard adapter reports ("LControl", "F13", "PageUp", "A"…).
const MODIFIERS: &[&str] = &["LControl", "RControl", "LAlt", "RAlt", "LShift", "RShift", "LMeta", "RMeta"];

/// Keys that are fine to use on their own: they are not typed, and tapping them does nothing else.
/// (A lone Alt, Shift or Win key is not here: a tap of those opens a menu or the Start menu.)
fn is_safe_alone(name: &str) -> bool {
    matches!(name, "LControl" | "RControl" | "Insert" | "Home" | "End" | "PageUp" | "PageDown")
        || name.strip_prefix('F').is_some_and(|n| n.parse::<u8>().is_ok_and(|n| (1..=20).contains(&n)))
        || name.starts_with("Numpad")
}

/// Common presets offered in the menu: (id, key names).
pub const PRESETS: &[(&str, &[&str])] = &[("right_ctrl", &["RControl"]), ("f13", &["F13"]), ("f14", &["F14"]), ("f15", &["F15"]), ("f16", &["F16"])];

const MAX_KEYS: usize = 4;

/// What the user has to press to start and stop dictating.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Trigger {
    /// Either Ctrl and either Win key together (the default).
    #[default]
    CtrlWin,
    /// One key or several held together, by name.
    Keys(Vec<String>),
}

fn friendly(name: &str) -> String {
    match name {
        "LControl" => "Left Ctrl".into(),
        "RControl" => "Right Ctrl".into(),
        "LAlt" => "Left Alt".into(),
        "RAlt" => "Right Alt".into(),
        "LShift" => "Left Shift".into(),
        "RShift" => "Right Shift".into(),
        "LMeta" => "Left Win".into(),
        "RMeta" => "Right Win".into(),
        "PageUp" => "Page Up".into(),
        "PageDown" => "Page Down".into(),
        "NumpadAdd" => "Numpad +".into(),
        "NumpadSubtract" => "Numpad -".into(),
        "NumpadMultiply" => "Numpad *".into(),
        "NumpadDivide" => "Numpad /".into(),
        "NumpadDecimal" => "Numpad .".into(),
        "NumpadEnter" => "Numpad Enter".into(),
        other => other.strip_prefix("Numpad").map_or_else(|| other.strip_prefix("Key").unwrap_or(other).to_string(), |n| format!("Numpad {n}")),
    }
}

impl Trigger {
    pub const DEFAULT: Trigger = Trigger::CtrlWin;

    /// Builds a trigger from the keys that were pressed together, or says why they can't be used.
    pub fn from_keys(names: &[String]) -> Result<Trigger, String> {
        let mut keys: Vec<String> = names.to_vec();
        keys.sort_by_key(|k| (!MODIFIERS.contains(&k.as_str()), k.clone()));
        keys.dedup();
        if keys.is_empty() || keys.len() > MAX_KEYS {
            return Err(format!("Choose between one and {MAX_KEYS} keys."));
        }
        let ok = if keys.len() == 1 { is_safe_alone(&keys[0]) } else { keys.iter().any(|k| MODIFIERS.contains(&k.as_str()) || is_safe_alone(k)) };
        if !ok {
            return Err("That key on its own would get in the way of typing (or of Windows). Hold it together with Ctrl, Alt, Shift or Win, or pick an F key, Insert, Home, End, Page Up/Down, a numpad key or Ctrl.".into());
        }
        Ok(Trigger::Keys(keys))
    }

    /// Reads an id saved earlier: "ctrl_win", a preset, or "keys:LControl+LAlt+D".
    pub fn parse(id: &str) -> Option<Trigger> {
        if id == "ctrl_win" {
            return Some(Trigger::CtrlWin);
        }
        if let Some((_, names)) = PRESETS.iter().find(|(preset, _)| *preset == id) {
            return Trigger::from_keys(&names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).ok();
        }
        let names: Vec<String> = id.strip_prefix("keys:")?.split('+').map(str::to_string).collect();
        Trigger::from_keys(&names).ok()
    }

    pub fn id(&self) -> String {
        match self {
            Trigger::CtrlWin => "ctrl_win".into(),
            Trigger::Keys(keys) => format!("keys:{}", keys.join("+")),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Trigger::CtrlWin => "Ctrl + Win".into(),
            Trigger::Keys(keys) => keys.iter().map(|k| friendly(k)).collect::<Vec<_>>().join(" + "),
        }
    }

    /// How long it must be held before it counts. A lone modifier is used in many shortcuts, so it waits longer
    /// to be sure it was pressed on its own.
    pub fn hold_ms(&self) -> Millis {
        match self {
            Trigger::CtrlWin => 80,
            Trigger::Keys(keys) if keys.len() == 1 && MODIFIERS.contains(&keys[0].as_str()) => 200,
            Trigger::Keys(keys) if keys.len() == 1 => 30,
            Trigger::Keys(_) => 80,
        }
    }
}

/// What the keys just did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordEvent {
    /// Ctrl+Win was pressed (and held briefly).
    Pressed,
    /// The chord was let go after `held_ms` since it fired. A long hold means push-to-talk.
    Released { held_ms: Millis },
}

/// Keys must stay up this long before a release counts (guards against one-poll flickers).
const RELEASE_GRACE_MS: Millis = 40;

pub struct ChordDetector {
    hold_ms: Millis,
    phase: Phase,
}

impl ChordDetector {
    pub fn new(hold_ms: Millis) -> Self {
        Self { hold_ms, phase: Phase::Idle }
    }

    /// `held` is true while the whole trigger is down (both Ctrl and Win, or the single key);
    /// `other_keys` is true when any key that is not part of the trigger is down.
    pub fn update(&mut self, now: Millis, held: bool, other_keys: bool) -> Option<ChordEvent> {
        let both = held;
        match self.phase {
            Phase::Active(fired_at) if !both => {
                self.phase = Phase::Lost { fired_at, lost_at: now };
                return None;
            }
            Phase::Lost { fired_at, .. } if both => {
                self.phase = Phase::Active(fired_at);
                return None;
            }
            Phase::Lost { fired_at, lost_at } => {
                if now.saturating_sub(lost_at) >= RELEASE_GRACE_MS {
                    self.phase = Phase::Idle;
                    return Some(ChordEvent::Released { held_ms: lost_at.saturating_sub(fired_at) });
                }
                return None;
            }
            _ => {}
        }
        if !both {
            self.phase = Phase::Idle;
            return None;
        }
        match self.phase {
            Phase::Idle if other_keys => self.phase = Phase::Spent,
            Phase::Idle => self.phase = Phase::Arming(now),
            Phase::Arming(_) if other_keys => self.phase = Phase::Spent,
            Phase::Arming(since) if now.saturating_sub(since) >= self.hold_ms => {
                self.phase = Phase::Active(now);
                return Some(ChordEvent::Pressed);
            }
            _ => {}
        }
        None
    }
}

/// Push-to-talk: letting go of Ctrl+Win ends a dictation only if it is on, one is running,
/// and the chord was held long enough to mean "talk" (a quick tap just toggles).
pub fn stops_on_release(hold_to_talk: bool, recording: bool, held_ms: Millis, threshold_ms: Millis) -> bool {
    hold_to_talk && recording && held_ms >= threshold_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRESSED: Option<ChordEvent> = Some(ChordEvent::Pressed);

    #[test]
    fn fires_once_after_the_hold_time() {
        let mut d = ChordDetector::new(80);
        assert_eq!(d.update(0, true, false), None);
        assert_eq!(d.update(79, true, false), None);
        assert_eq!(d.update(80, true, false), PRESSED);
        assert_eq!(d.update(500, true, false), None, "no repeat while held");
    }

    #[test]
    fn release_reports_how_long_it_was_held() {
        let mut d = ChordDetector::new(50);
        d.update(0, true, false);
        assert_eq!(d.update(60, true, false), PRESSED);
        assert_eq!(d.update(960, false, false), None, "a release must persist briefly to count");
        assert_eq!(d.update(1000, false, false), Some(ChordEvent::Released { held_ms: 900 }));
        assert_eq!(d.update(1100, false, false), None, "only one release per press");
    }

    #[test]
    fn a_quick_tap_reports_a_short_hold() {
        let mut d = ChordDetector::new(50);
        d.update(0, true, false);
        d.update(60, true, false);
        d.update(160, false, false);
        assert_eq!(d.update(210, false, false), Some(ChordEvent::Released { held_ms: 100 }));
    }

    #[test]
    fn rearms_after_release() {
        let mut d = ChordDetector::new(50);
        d.update(0, true, false);
        assert_eq!(d.update(60, true, false), PRESSED);
        d.update(100, false, false);
        d.update(150, false, false); // released for real
        d.update(200, true, false);
        assert_eq!(d.update(260, true, false), PRESSED);
    }

    #[test]
    fn a_brief_flicker_while_held_is_not_a_release_or_a_second_press() {
        let mut d = ChordDetector::new(50);
        d.update(0, true, false);
        assert_eq!(d.update(60, true, false), PRESSED);
        assert_eq!(d.update(300, false, false), None, "Win reads as up for one poll");
        assert_eq!(d.update(308, true, false), None, "...then down again");
        for t in (400..2000).step_by(8) {
            assert_eq!(d.update(t, true, false), None, "no repeat while still held");
        }
        d.update(2000, false, false);
        assert_eq!(d.update(2050, false, false), Some(ChordEvent::Released { held_ms: 1940 }));
    }

    #[test]
    fn push_to_talk_needs_the_setting_a_recording_and_a_long_hold() {
        assert!(stops_on_release(true, true, 600, 450));
        assert!(!stops_on_release(false, true, 600, 450), "setting off");
        assert!(!stops_on_release(true, false, 600, 450), "nothing recording");
        assert!(!stops_on_release(true, true, 200, 450), "a quick tap just toggles");
    }

    #[test]
    fn one_modifier_alone_never_fires() {
        let mut d = ChordDetector::new(0);
        assert_eq!(d.update(0, false, false), None);
        assert_eq!(d.update(1000, false, false), None);
    }

    #[test]
    fn another_key_disqualifies_the_chord_and_its_release_is_silent() {
        // Ctrl+Win+Right is a Windows shortcut, not ours.
        let mut d = ChordDetector::new(80);
        d.update(0, true, false);
        assert_eq!(d.update(40, true, true), None);
        assert_eq!(d.update(500, true, false), None, "stays spent until released");
        assert_eq!(d.update(600, false, false), None, "no release event for a chord that never fired");
        // ...and a key already held before the chord also blocks it.
        let mut d = ChordDetector::new(0);
        assert_eq!(d.update(0, true, true), None);
        assert_eq!(d.update(100, true, true), None);
    }

    #[test]
    fn triggers_round_trip_and_unsafe_ones_are_refused() {
        for id in ["ctrl_win", "keys:RControl", "keys:F13", "keys:LAlt+LControl+D", "keys:PageDown", "keys:NumpadAdd"] {
            assert_eq!(Trigger::parse(id).unwrap().id(), id);
        }
        assert_eq!(Trigger::parse("right_ctrl"), Trigger::parse("keys:RControl"), "ids saved by earlier versions still work");
        assert_eq!(Trigger::parse("f14").unwrap().label(), "F14");
        for id in ["fn", "keys:F21", "keys:A", "keys:Space", "keys:LAlt", "keys:LShift", "keys:LMeta", "keys:A+B", "keys:", "keys:LControl+LAlt+LShift+LMeta+A"] {
            assert_eq!(Trigger::parse(id), None, "{id} must not be a trigger");
        }
    }

    #[test]
    fn a_chord_is_labelled_in_plain_words_with_modifiers_first() {
        let t = Trigger::from_keys(&["D".into(), "LAlt".into(), "LControl".into()]).unwrap();
        assert_eq!(t.id(), "keys:LAlt+LControl+D");
        assert_eq!(t.label(), "Left Alt + Left Ctrl + D");
        assert_eq!(Trigger::parse("keys:PageUp").unwrap().label(), "Page Up");
        assert_eq!(Trigger::parse("keys:Numpad7").unwrap().label(), "Numpad 7");
    }

    #[test]
    fn a_lone_modifier_waits_longer_than_a_function_key() {
        assert!(Trigger::parse("keys:RControl").unwrap().hold_ms() > Trigger::parse("keys:F13").unwrap().hold_ms());
        assert_eq!(Trigger::CtrlWin.hold_ms(), 80);
    }

    #[test]
    fn releasing_early_cancels() {
        let mut d = ChordDetector::new(80);
        d.update(0, true, false);
        d.update(40, false, false);
        assert_eq!(d.update(100, true, false), None, "restarted the hold");
        assert_eq!(d.update(180, true, false), PRESSED);
    }
}
