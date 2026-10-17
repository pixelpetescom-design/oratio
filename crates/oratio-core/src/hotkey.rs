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

/// Keys that may be the dictation trigger: (id, label, how long it must be held). Keys you type with
/// (letters, digits, Space, Enter, arrows…) are deliberately missing, as are Alt (a lone tap focuses the
/// window's menu bar), Shift and Win. The Fn key is not here either: on nearly every keyboard it is handled
/// inside the keyboard and Windows never sees it.
pub const KEYS: &[(&str, &str, Millis)] = &[
    ("left_ctrl", "Left Ctrl", 200),
    ("right_ctrl", "Right Ctrl", 200),
    ("f1", "F1", 30),
    ("f2", "F2", 30),
    ("f3", "F3", 30),
    ("f4", "F4", 30),
    ("f5", "F5", 30),
    ("f6", "F6", 30),
    ("f7", "F7", 30),
    ("f8", "F8", 30),
    ("f9", "F9", 30),
    ("f10", "F10", 30),
    ("f11", "F11", 30),
    ("f12", "F12", 30),
    ("f13", "F13", 30),
    ("f14", "F14", 30),
    ("f15", "F15", 30),
    ("f16", "F16", 30),
    ("f17", "F17", 30),
    ("f18", "F18", 30),
    ("f19", "F19", 30),
    ("f20", "F20", 30),
    ("insert", "Insert", 30),
    ("home", "Home", 30),
    ("end", "End", 30),
    ("page_up", "Page Up", 30),
    ("page_down", "Page Down", 30),
    ("numpad_0", "Numpad 0", 30),
    ("numpad_1", "Numpad 1", 30),
    ("numpad_2", "Numpad 2", 30),
    ("numpad_3", "Numpad 3", 30),
    ("numpad_4", "Numpad 4", 30),
    ("numpad_5", "Numpad 5", 30),
    ("numpad_6", "Numpad 6", 30),
    ("numpad_7", "Numpad 7", 30),
    ("numpad_8", "Numpad 8", 30),
    ("numpad_9", "Numpad 9", 30),
    ("numpad_add", "Numpad +", 30),
    ("numpad_subtract", "Numpad -", 30),
    ("numpad_multiply", "Numpad *", 30),
    ("numpad_divide", "Numpad /", 30),
    ("numpad_decimal", "Numpad .", 30),
];

/// The key (or chord) that starts and stops dictation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    CtrlWin,
    /// One key from [`KEYS`], by id.
    Key(&'static str),
}

impl Default for Trigger {
    fn default() -> Self {
        Trigger::DEFAULT
    }
}

impl Trigger {
    pub const DEFAULT: Trigger = Trigger::CtrlWin;

    pub fn parse(id: &str) -> Option<Trigger> {
        if id == "ctrl_win" {
            return Some(Trigger::CtrlWin);
        }
        KEYS.iter().find(|(key, _, _)| *key == id).map(|(key, _, _)| Trigger::Key(key))
    }

    pub fn id(self) -> &'static str {
        match self {
            Trigger::CtrlWin => "ctrl_win",
            Trigger::Key(id) => id,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Trigger::CtrlWin => "Ctrl + Win",
            Trigger::Key(id) => KEYS.iter().find(|(key, _, _)| *key == id).map_or("?", |(_, label, _)| label),
        }
    }

    /// How long it must be held before it counts. A modifier is used in many shortcuts, so it waits longer
    /// to be sure it was pressed on its own.
    pub fn hold_ms(self) -> Millis {
        match self {
            Trigger::CtrlWin => 80,
            Trigger::Key(id) => KEYS.iter().find(|(key, _, _)| *key == id).map_or(80, |(_, _, hold)| *hold),
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
    fn triggers_round_trip_through_their_ids_and_reject_unknown_ones() {
        for id in ["ctrl_win", "right_ctrl", "f13", "f20", "numpad_add", "page_down"] {
            assert_eq!(Trigger::parse(id).unwrap().id(), id);
        }
        for id in ["fn", "f21", "a", "space", "right_alt", "left_shift", "left_win"] {
            assert_eq!(Trigger::parse(id), None, "{id} must not be a trigger");
        }
        assert_eq!(Trigger::parse("f14").unwrap().label(), "F14");
        assert!(Trigger::parse("right_ctrl").unwrap().hold_ms() > Trigger::parse("f13").unwrap().hold_ms(), "modifiers wait longer");
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
