//! Detects the Ctrl+Win chord from raw key state, as a pure function of time.
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
    /// Disqualified by another key; wait for release.
    Spent,
}

/// What the keys just did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordEvent {
    /// Ctrl+Win was pressed (and held briefly).
    Pressed,
    /// The chord was let go after `held_ms` since it fired. A long hold means push-to-talk.
    Released { held_ms: Millis },
}

pub struct ChordDetector {
    hold_ms: Millis,
    phase: Phase,
}

impl ChordDetector {
    pub fn new(hold_ms: Millis) -> Self {
        Self { hold_ms, phase: Phase::Idle }
    }

    /// `other_keys` is true when any key besides Ctrl and Win is down.
    pub fn update(&mut self, now: Millis, ctrl: bool, win: bool, other_keys: bool) -> Option<ChordEvent> {
        if !(ctrl && win) {
            let released = match self.phase {
                Phase::Active(since) => Some(ChordEvent::Released { held_ms: now.saturating_sub(since) }),
                _ => None,
            };
            self.phase = Phase::Idle;
            return released;
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

#[cfg(test)]
mod tests {
    use super::*;

    const PRESSED: Option<ChordEvent> = Some(ChordEvent::Pressed);

    #[test]
    fn fires_once_after_the_hold_time() {
        let mut d = ChordDetector::new(80);
        assert_eq!(d.update(0, true, true, false), None);
        assert_eq!(d.update(79, true, true, false), None);
        assert_eq!(d.update(80, true, true, false), PRESSED);
        assert_eq!(d.update(500, true, true, false), None, "no repeat while held");
    }

    #[test]
    fn release_reports_how_long_it_was_held() {
        let mut d = ChordDetector::new(50);
        d.update(0, true, true, false);
        assert_eq!(d.update(60, true, true, false), PRESSED);
        assert_eq!(d.update(960, false, true, false), Some(ChordEvent::Released { held_ms: 900 }));
        assert_eq!(d.update(1000, false, false, false), None, "only one release per press");
    }

    #[test]
    fn a_quick_tap_reports_a_short_hold() {
        let mut d = ChordDetector::new(50);
        d.update(0, true, true, false);
        d.update(60, true, true, false);
        assert_eq!(d.update(160, false, false, false), Some(ChordEvent::Released { held_ms: 100 }));
    }

    #[test]
    fn rearms_after_release() {
        let mut d = ChordDetector::new(50);
        d.update(0, true, true, false);
        assert_eq!(d.update(60, true, true, false), PRESSED);
        d.update(100, false, false, false);
        d.update(200, true, true, false);
        assert_eq!(d.update(260, true, true, false), PRESSED);
    }

    #[test]
    fn one_modifier_alone_never_fires() {
        let mut d = ChordDetector::new(0);
        assert_eq!(d.update(0, true, false, false), None);
        assert_eq!(d.update(1000, false, true, false), None);
    }

    #[test]
    fn another_key_disqualifies_the_chord_and_its_release_is_silent() {
        // Ctrl+Win+Right is a Windows shortcut, not ours.
        let mut d = ChordDetector::new(80);
        d.update(0, true, true, false);
        assert_eq!(d.update(40, true, true, true), None);
        assert_eq!(d.update(500, true, true, false), None, "stays spent until released");
        assert_eq!(d.update(600, false, false, false), None, "no release event for a chord that never fired");
        // ...and a key already held before the chord also blocks it.
        let mut d = ChordDetector::new(0);
        assert_eq!(d.update(0, true, true, true), None);
        assert_eq!(d.update(100, true, true, true), None);
    }

    #[test]
    fn releasing_early_cancels() {
        let mut d = ChordDetector::new(80);
        d.update(0, true, true, false);
        d.update(40, false, true, false);
        assert_eq!(d.update(100, true, true, false), None, "restarted the hold");
        assert_eq!(d.update(180, true, true, false), PRESSED);
    }
}
