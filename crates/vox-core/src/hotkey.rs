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
    /// Fired, or disqualified by another key; wait for release.
    Spent,
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
    /// Returns true exactly once per chord press.
    pub fn update(&mut self, now: Millis, ctrl: bool, win: bool, other_keys: bool) -> bool {
        if !(ctrl && win) {
            self.phase = Phase::Idle;
            return false;
        }
        match self.phase {
            Phase::Idle if other_keys => self.phase = Phase::Spent,
            Phase::Idle => self.phase = Phase::Arming(now),
            Phase::Arming(_) if other_keys => self.phase = Phase::Spent,
            Phase::Arming(since) if now.saturating_sub(since) >= self.hold_ms => {
                self.phase = Phase::Spent;
                return true;
            }
            _ => {}
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_once_after_the_hold_time() {
        let mut d = ChordDetector::new(80);
        assert!(!d.update(0, true, true, false));
        assert!(!d.update(79, true, true, false));
        assert!(d.update(80, true, true, false));
        assert!(!d.update(500, true, true, false), "no repeat while held");
    }

    #[test]
    fn rearms_after_release() {
        let mut d = ChordDetector::new(50);
        d.update(0, true, true, false);
        assert!(d.update(60, true, true, false));
        assert!(!d.update(100, false, false, false));
        d.update(200, true, true, false);
        assert!(d.update(260, true, true, false));
    }

    #[test]
    fn one_modifier_alone_never_fires() {
        let mut d = ChordDetector::new(0);
        assert!(!d.update(0, true, false, false));
        assert!(!d.update(1000, false, true, false));
    }

    #[test]
    fn another_key_disqualifies_the_chord() {
        // Ctrl+Win+Right is a Windows shortcut, not ours.
        let mut d = ChordDetector::new(80);
        d.update(0, true, true, false);
        assert!(!d.update(40, true, true, true));
        assert!(!d.update(500, true, true, false), "stays spent until released");
        // ...and a key already held before the chord also blocks it.
        let mut d = ChordDetector::new(0);
        assert!(!d.update(0, true, true, true));
        assert!(!d.update(100, true, true, true));
    }

    #[test]
    fn releasing_early_cancels() {
        let mut d = ChordDetector::new(80);
        d.update(0, true, true, false);
        d.update(40, false, true, false);
        assert!(!d.update(100, true, true, false), "restarted the hold");
        assert!(d.update(180, true, true, false));
    }
}
