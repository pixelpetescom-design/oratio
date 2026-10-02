//! The dictation lifecycle as a pure state machine.
//!
//! `step` takes the current state and an input and returns the next state plus at
//! most one side effect for the shell to perform. Nothing here touches the clock,
//! threads or the OS, so every rule below is unit-tested.
//!
//! Escape semantics: one Escape while recording starts a short cancel countdown
//! (recording keeps going so no audio is lost); a second Escape inside the window
//! resumes; if the countdown expires the recording is discarded.

use serde::Serialize;

pub type Millis = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum State {
    Loading,
    Unavailable,
    Idle,
    Recording,
    CancelPending { deadline: Millis },
    Finalizing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    EngineReady,
    EngineFailed,
    Toggle,
    Escape,
    Tick,
    Finished,
    FinishFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    StartRecording,
    /// Stop the microphone and transcribe what remains.
    Finish,
    /// Stop the microphone and delete everything recorded.
    Discard,
    /// Stop the microphone only (engine died); keep whatever was saved.
    Abort,
}

impl State {
    /// Escape is registered globally only while it means something.
    pub fn wants_escape(&self) -> bool {
        matches!(self, State::Recording | State::CancelPending { .. })
    }

    pub fn deadline(&self) -> Option<Millis> {
        match self {
            State::CancelPending { deadline } => Some(*deadline),
            _ => None,
        }
    }
}

pub fn step(state: State, input: Input, now: Millis, cancel_grace: Millis) -> (State, Option<Effect>) {
    use Effect::*;
    use Input::*;
    use State::*;
    match (state, input) {
        (Loading, EngineReady) => (Idle, None),
        (Unavailable, _) => (Unavailable, None),
        (Recording | CancelPending { .. }, EngineFailed) => (Unavailable, Some(Abort)),
        (_, EngineFailed) => (Unavailable, None),

        (Idle, Toggle) => (Recording, Some(StartRecording)),

        (Recording, Toggle) => (Finalizing, Some(Finish)),
        (Recording, Escape) => (CancelPending { deadline: now + cancel_grace }, None),

        (CancelPending { .. }, Escape) => (Recording, None),
        // Stopping while a cancel is pending means "I want the text after all".
        (CancelPending { .. }, Toggle) => (Finalizing, Some(Finish)),
        (CancelPending { deadline }, Tick) if now >= deadline => (Idle, Some(Discard)),

        (Finalizing, Finished | FinishFailed) => (Idle, None),
        // The engine could not even open a recording: stop listening rather than talk into the void.
        (Recording | CancelPending { .. }, FinishFailed) => (Idle, Some(Abort)),

        (s, _) => (s, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: Millis = 3000;

    fn run(mut s: State, steps: &[(Input, Millis)]) -> (State, Vec<Effect>) {
        let mut fx = vec![];
        for (i, t) in steps {
            let (n, e) = step(s, *i, *t, G);
            s = n;
            fx.extend(e);
        }
        (s, fx)
    }

    #[test]
    fn loads_then_records_and_finishes() {
        let (s, fx) = run(State::Loading, &[(Input::EngineReady, 0), (Input::Toggle, 1), (Input::Toggle, 2)]);
        assert_eq!(s, State::Finalizing);
        assert_eq!(fx, vec![Effect::StartRecording, Effect::Finish]);
        let (s, _) = run(s, &[(Input::Finished, 3)]);
        assert_eq!(s, State::Idle);
    }

    #[test]
    fn toggle_before_ready_does_nothing() {
        let (s, fx) = run(State::Loading, &[(Input::Toggle, 0), (Input::Escape, 0)]);
        assert_eq!((s, fx), (State::Loading, vec![]));
    }

    #[test]
    fn escape_starts_countdown_and_expiry_discards() {
        let (s, fx) = run(State::Recording, &[(Input::Escape, 100)]);
        assert_eq!(s, State::CancelPending { deadline: 3100 });
        assert!(fx.is_empty());
        let (s, fx) = run(s, &[(Input::Tick, 3099)]);
        assert_eq!(s, State::CancelPending { deadline: 3100 }, "early tick is ignored");
        assert!(fx.is_empty());
        let (s, fx) = run(s, &[(Input::Tick, 3100)]);
        assert_eq!((s, fx), (State::Idle, vec![Effect::Discard]));
    }

    #[test]
    fn second_escape_resumes_and_stale_tick_is_harmless() {
        let (s, _) = run(State::Recording, &[(Input::Escape, 0), (Input::Escape, 500)]);
        assert_eq!(s, State::Recording);
        // old timer fires while recording again
        let (s, fx) = run(s, &[(Input::Tick, 3000)]);
        assert_eq!((s, fx), (State::Recording, vec![]));
        // a fresh cancel gets a fresh deadline; the old timer cannot cut it short
        let (s, _) = run(s, &[(Input::Escape, 2000), (Input::Tick, 3000)]);
        assert_eq!(s, State::CancelPending { deadline: 5000 });
    }

    #[test]
    fn toggle_during_countdown_keeps_the_text() {
        let (s, fx) = run(State::Recording, &[(Input::Escape, 0), (Input::Toggle, 10)]);
        assert_eq!((s, fx), (State::Finalizing, vec![Effect::Finish]));
    }

    #[test]
    fn finalizing_ignores_user_input() {
        let (s, fx) = run(State::Finalizing, &[(Input::Toggle, 0), (Input::Escape, 0), (Input::Tick, 99999)]);
        assert_eq!((s, fx), (State::Finalizing, vec![]));
    }

    #[test]
    fn finish_failure_returns_to_idle() {
        assert_eq!(run(State::Finalizing, &[(Input::FinishFailed, 0)]).0, State::Idle);
    }

    #[test]
    fn engine_death_while_recording_stops_the_mic() {
        let (s, fx) = run(State::Recording, &[(Input::EngineFailed, 0)]);
        assert_eq!((s, fx), (State::Unavailable, vec![Effect::Abort]));
        let (s, fx) = run(State::Idle, &[(Input::EngineFailed, 0), (Input::Toggle, 1)]);
        assert_eq!((s, fx), (State::Unavailable, vec![]));
    }

    #[test]
    fn engine_refusing_a_recording_stops_the_mic() {
        let (s, fx) = run(State::Recording, &[(Input::FinishFailed, 0)]);
        assert_eq!((s, fx), (State::Idle, vec![Effect::Abort]));
    }

    #[test]
    fn escape_is_only_wanted_while_it_means_something() {
        assert!(State::Recording.wants_escape());
        assert!(State::CancelPending { deadline: 1 }.wants_escape());
        for s in [State::Idle, State::Loading, State::Finalizing, State::Unavailable] {
            assert!(!s.wants_escape());
        }
    }
}
