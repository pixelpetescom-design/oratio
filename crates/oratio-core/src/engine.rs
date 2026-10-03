//! The recognition engine: one background thread that owns the transcriber,
//! the segmenter and the history writes. A single ordered command channel means
//! audio can never overtake `Begin` or follow `Finish`.
//!
//! Failure policy: text is never lost to a secondary failure. Each utterance is
//! written to history the moment it is recognised; if history is unavailable the
//! text is still returned to the caller; recogniser panics become errors.

use crate::history::{History, RecordingId};
use crate::lexicon::{Fix, Lexicon};
use crate::commands;
use crate::polish::polish;
use crate::vocab::apply_fixes;
use crate::spelling::to_australian;
use crate::segmenter::{normalize, rms, Segmenter, SegmenterConfig, SAMPLE_RATE};
use crate::stt::Transcriber;
use crate::CoreError;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

pub enum Command {
    Begin { started_at_ms: i64, spoken_commands: bool },
    /// 16 kHz mono f32.
    Audio(Vec<f32>),
    Finish,
    Discard,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Ready,
    LoadFailed(String),
    Level(f32),
    /// One utterance recognised (raw, before polishing) — for live preview.
    Segment { id: RecordingId, text: String },
    /// Final polished text. Empty when no speech was detected.
    /// `enter`: the user ended with "press enter".
    Finished { id: Option<RecordingId>, text: String, enter: bool },
    /// "Scratch that": the user wants the previous dictation undone; nothing new is typed.
    Scratched,
    Discarded,
    /// The recording produced nothing usable.
    Failed { reason: String },
    /// Something went wrong but no text was lost.
    Warning(String),
}

pub struct Engine {
    tx: Sender<Command>,
    handle: Option<JoinHandle<()>>,
}

type Loader = Box<dyn FnOnce() -> Result<Box<dyn Transcriber>, CoreError> + Send>;

impl Engine {
    pub fn spawn(load: Loader, history: Arc<dyn History>, lexicon: Arc<dyn Lexicon>, events: Sender<Event>) -> Engine {
        let (tx, rx) = channel();
        let handle = std::thread::Builder::new()
            .name("oratio-engine".into())
            .spawn(move || run(load, history, lexicon, rx, events))
            .ok();
        Engine { tx, handle }
    }

    pub fn sender(&self) -> Sender<Command> {
        self.tx.clone()
    }

    pub fn send(&self, cmd: Command) -> bool {
        self.tx.send(cmd).is_ok()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Closing the channel ends the loop; the thread is detached if it is mid-inference.
        let (dead, _) = channel();
        self.tx = dead;
        drop(self.handle.take());
    }
}

struct Take {
    id: RecordingId,
    texts: Vec<String>,
    last_error: Option<String>,
    /// Loudest chunk heard, for diagnosing "it heard nothing" reports.
    max_level: f32,
    /// The user's learned corrections, read when the recording began.
    fixes: Vec<Fix>,
    /// The user's own words: exempt from spelling rewrites.
    words: Vec<String>,
    /// Spoken trigger → text, expanded after everything else.
    snippets: Vec<Fix>,
    spoken_commands: bool,
}

fn recognise(transcriber: &mut dyn Transcriber, history: &dyn History, take: &mut Take, mut audio: Vec<f32>, segmenter_events: &dyn Fn(Event)) {
        normalize(&mut audio);
        let started = std::time::Instant::now();
        let result = catch_unwind(AssertUnwindSafe(|| transcriber.transcribe(&audio)));
        eprintln!(
            "[oratio] utterance {:.1}s -> {} in {} ms",
            audio.len() as f32 / SAMPLE_RATE as f32,
            match &result {
                Ok(Ok(t)) => format!("{:?}", t.trim()),
                Ok(Err(e)) => format!("error: {e}"),
                Err(_) => "crash".into(),
            },
            started.elapsed().as_millis()
        );
        match result {
            Ok(Ok(text)) => {
                let text = text.trim().to_string();
                if text.is_empty() {
                    return;
                }
                if let Err(e) = history.append_segment(take.id, &text) {
                    segmenter_events(Event::Warning(e.to_string()));
                }
                segmenter_events(Event::Segment { id: take.id, text: text.clone() });
                take.texts.push(text);
            }
            Ok(Err(e)) => take.last_error = Some(e.to_string()),
            Err(_) => take.last_error = Some("recogniser crashed".into()),
        }
    }

fn run(load: Loader, history: Arc<dyn History>, lexicon: Arc<dyn Lexicon>, rx: Receiver<Command>, events: Sender<Event>) {
    let emit = |e: Event| {
        let _ = events.send(e);
    };
    let mut transcriber = match catch_unwind(AssertUnwindSafe(load)) {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return emit(Event::LoadFailed(e.to_string())),
        Err(_) => return emit(Event::LoadFailed("model loader crashed".into())),
    };
    emit(Event::Ready);

    let mut segmenter = Segmenter::new(SegmenterConfig::default());
    let mut take: Option<Take> = None;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::Begin { started_at_ms, spoken_commands } => {
                segmenter = Segmenter::new(SegmenterConfig::default());
                if let Some(old) = take.take() {
                    let _ = history.fail(old.id, "superseded by a new recording");
                }
                // Read what the user has taught us fresh each time, so edits apply immediately.
                let words = lexicon.words().unwrap_or_else(|e| {
                    emit(Event::Warning(e.to_string()));
                    vec![]
                });
                let fixes = lexicon.fixes().unwrap_or_else(|e| {
                    emit(Event::Warning(e.to_string()));
                    vec![]
                });
                let snippets = lexicon.snippets().unwrap_or_else(|e| {
                    emit(Event::Warning(e.to_string()));
                    vec![]
                });
                transcriber.set_hints(&words);
                match history.begin(started_at_ms) {
                    Ok(id) => take = Some(Take { id, texts: vec![], last_error: None, max_level: 0.0, fixes, words: words.clone(), snippets, spoken_commands }),
                    Err(e) => emit(Event::Failed { reason: e.to_string() }),
                }
            }
            Command::Audio(chunk) => {
                let Some(t) = take.as_mut() else { continue };
                let level = rms(&chunk);
                t.max_level = t.max_level.max(level);
                emit(Event::Level(level));
                for utterance in segmenter.push(&chunk) {
                    recognise(transcriber.as_mut(), history.as_ref(), t, utterance, &emit);
                }
            }
            Command::Finish => {
                let Some(mut t) = take.take() else {
                    emit(Event::Finished { id: None, text: String::new(), enter: false });
                    continue;
                };
                if let Some(rest) = segmenter.flush() {
                    recognise(transcriber.as_mut(), history.as_ref(), &mut t, rest, &emit);
                }
                eprintln!(
                    "[oratio] stopped: {} utterance(s) recognised, loudest level {:.4}, background noise {:.4}, short sounds ignored {}",
                    t.texts.len(),
                    t.max_level,
                    segmenter.noise_floor(),
                    segmenter.dropped()
                );
                if t.texts.is_empty() {
                    let _ = history.delete(t.id);
                    match t.last_error {
                        Some(reason) => emit(Event::Failed { reason }),
                        None => emit(Event::Finished { id: None, text: String::new(), enter: false }),
                    }
                    continue;
                }
                // Spelling first, so the user's own corrections always have the final word; spoken commands
                // and snippets come last so nothing earlier can disturb the line breaks they create.
                let text = apply_fixes(&to_australian(&polish(&t.texts), &t.words), &t.fixes);
                let parsed = if t.spoken_commands { commands::apply(&text) } else { commands::Parsed { text, ..Default::default() } };
                if parsed.scratch {
                    let _ = history.delete(t.id);
                    emit(Event::Scratched);
                    continue;
                }
                let text = apply_fixes(&parsed.text, &t.snippets);
                if text.is_empty() {
                    // Nothing but a command (e.g. just "press enter"): no history entry.
                    let _ = history.delete(t.id);
                    emit(Event::Finished { id: None, text, enter: parsed.enter });
                    continue;
                }
                if let Err(e) = history.complete(t.id, &text) {
                    emit(Event::Warning(e.to_string()));
                }
                if let Some(reason) = t.last_error {
                    emit(Event::Warning(reason));
                }
                emit(Event::Finished { id: Some(t.id), text, enter: parsed.enter });
            }
            Command::Discard => {
                segmenter = Segmenter::new(SegmenterConfig::default());
                if let Some(t) = take.take() {
                    if let Err(e) = history.delete(t.id) {
                        emit(Event::Warning(e.to_string()));
                    }
                }
                emit(Event::Discarded);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Status;
    use crate::testing::{MemHistory, MemLexicon};
    use std::sync::Mutex;
    use std::time::Duration;

    struct Fake(Vec<Result<&'static str, &'static str>>, Arc<Mutex<Vec<String>>>);
    impl Transcriber for Fake {
        fn set_hints(&mut self, words: &[String]) {
            *self.1.lock().unwrap() = words.to_vec();
        }

        fn transcribe(&mut self, _: &[f32]) -> Result<String, CoreError> {
            match self.0.remove(0) {
                Ok(t) => Ok(t.into()),
                Err(e) => Err(CoreError::Stt(e.into())),
            }
        }
    }

    fn tone(ms: usize) -> Vec<f32> {
        (0..16 * ms).map(|i| (i as f32 * 0.17).sin() * 0.3).collect()
    }

    fn start(script: Vec<Result<&'static str, &'static str>>, hist: Arc<MemHistory>) -> (Engine, Receiver<Event>) {
        let (eng, rx, _, _) = start_with_lexicon(script, hist, Arc::new(MemLexicon::default()));
        (eng, rx)
    }

    type Hints = Arc<Mutex<Vec<String>>>;

    fn start_with_lexicon(
        script: Vec<Result<&'static str, &'static str>>,
        hist: Arc<MemHistory>,
        lexicon: Arc<MemLexicon>,
    ) -> (Engine, Receiver<Event>, Hints, Arc<MemLexicon>) {
        let (etx, erx) = channel();
        let hints = Hints::default();
        let h = hints.clone();
        let eng = Engine::spawn(
            Box::new(move || Ok(Box::new(Fake(script, h)) as Box<dyn Transcriber>)),
            hist,
            lexicon.clone(),
            etx,
        );
        assert_eq!(erx.recv_timeout(Duration::from_secs(2)).unwrap(), Event::Ready);
        (eng, erx, hints, lexicon)
    }

    fn until_finished(rx: &Receiver<Event>) -> (Vec<Event>, Event) {
        let mut seen = vec![];
        loop {
            let e = rx.recv_timeout(Duration::from_secs(2)).unwrap();
            if matches!(e, Event::Finished { .. } | Event::Failed { .. } | Event::Discarded) {
                return (seen, e);
            }
            seen.push(e);
        }
    }

    #[test]
    fn transcribes_while_speaking_then_flushes_the_tail() {
        let hist = Arc::new(MemHistory::default());
        let (eng, rx) = start(vec![Ok("hello there"), Ok("how are you")], hist.clone());
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true });
        eng.send(Command::Audio([tone(800), vec![0.0; 16 * 900]].concat()));
        eng.send(Command::Audio(tone(400))); // still talking when stop is pressed
        eng.send(Command::Finish);
        let (seen, done) = until_finished(&rx);
        assert!(seen.iter().any(|e| matches!(e, Event::Segment { text, .. } if text == "hello there")));
        assert_eq!(done, Event::Finished { id: Some(1), text: "Hello there how are you.".into(), enter: false });
        let rows = hist.list(10).unwrap();
        assert_eq!(rows[0].status, Status::Completed);
        assert_eq!(rows[0].segments.len(), 2);
    }

    #[test]
    fn silence_yields_empty_result_and_no_history_row() {
        let hist = Arc::new(MemHistory::default());
        let (eng, rx) = start(vec![], hist.clone());
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true });
        eng.send(Command::Audio(vec![0.0; 16_000]));
        eng.send(Command::Finish);
        assert_eq!(until_finished(&rx).1, Event::Finished { id: None, text: String::new(), enter: false });
        assert!(hist.list(10).unwrap().is_empty());
    }

    #[test]
    fn discard_deletes_the_recording() {
        let hist = Arc::new(MemHistory::default());
        let (eng, rx) = start(vec![Ok("x")], hist.clone());
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true });
        eng.send(Command::Audio([tone(800), vec![0.0; 16 * 900]].concat()));
        eng.send(Command::Discard);
        assert_eq!(until_finished(&rx).1, Event::Discarded);
        assert!(hist.list(10).unwrap().is_empty());
    }

    #[test]
    fn text_survives_a_broken_history() {
        let hist = Arc::new(MemHistory { fail_writes: true, ..Default::default() });
        let (eng, rx) = start(vec![Ok("save me")], hist);
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true });
        eng.send(Command::Audio(tone(500)));
        eng.send(Command::Finish);
        let (seen, done) = until_finished(&rx);
        assert_eq!(done, Event::Finished { id: Some(1), text: "Save me.".into(), enter: false });
        assert!(seen.iter().any(|e| matches!(e, Event::Warning(_))));
    }

    #[test]
    fn recogniser_error_with_no_text_is_reported() {
        let hist = Arc::new(MemHistory::default());
        let (eng, rx) = start(vec![Err("boom")], hist);
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true });
        eng.send(Command::Audio(tone(500)));
        eng.send(Command::Finish);
        assert!(matches!(until_finished(&rx).1, Event::Failed { reason } if reason.contains("boom")));
    }

    #[test]
    fn load_failure_is_reported_not_panicked() {
        let (etx, erx) = channel();
        let _eng = Engine::spawn(Box::new(|| Err(CoreError::Stt("no model".into()))), Arc::new(MemHistory::default()), Arc::new(MemLexicon::default()), etx);
        assert!(matches!(erx.recv_timeout(Duration::from_secs(2)).unwrap(), Event::LoadFailed(m) if m.contains("no model")));
    }

    #[test]
    fn learned_words_become_hints_and_learned_fixes_correct_the_result() {
        let lexicon = Arc::new(MemLexicon::default());
        lexicon.add_word("Postiz").unwrap();
        lexicon.add_fix(&Fix { from: "post is".into(), to: "Postiz".into() }).unwrap();
        let (eng, rx, hints, _) = start_with_lexicon(vec![Ok("open post is now")], Arc::new(MemHistory::default()), lexicon);
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true });
        eng.send(Command::Audio(tone(500)));
        eng.send(Command::Finish);
        assert_eq!(until_finished(&rx).1, Event::Finished { id: Some(1), text: "Open Postiz now.".into(), enter: false });
        assert_eq!(*hints.lock().unwrap(), vec!["Postiz".to_string()]);
    }

    #[test]
    fn output_uses_australian_spelling() {
        let (eng, rx) = start(vec![Ok("i like the color of the center")], Arc::new(MemHistory::default()));
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true });
        eng.send(Command::Audio(tone(500)));
        eng.send(Command::Finish);
        assert_eq!(until_finished(&rx).1, Event::Finished { id: Some(1), text: "I like the colour of the centre.".into(), enter: false });
    }

    fn dictate(script: Vec<Result<&'static str, &'static str>>, lexicon: Arc<MemLexicon>, commands: bool) -> Event {
        let (eng, rx, _, _) = start_with_lexicon(script, Arc::new(MemHistory::default()), lexicon);
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: commands });
        eng.send(Command::Audio(tone(500)));
        eng.send(Command::Finish);
        until_finished(&rx).1
    }

    #[test]
    fn spoken_commands_shape_the_text() {
        let e = dictate(vec![Ok("dear John comma new line thanks for the colour")], Arc::new(MemLexicon::default()), true);
        assert_eq!(e, Event::Finished { id: Some(1), text: "Dear John,\nThanks for the colour.".into(), enter: false });
    }

    #[test]
    fn spoken_commands_can_be_switched_off() {
        let e = dictate(vec![Ok("hello comma world")], Arc::new(MemLexicon::default()), false);
        assert_eq!(e, Event::Finished { id: Some(1), text: "Hello comma world.".into(), enter: false });
    }

    #[test]
    fn press_enter_sets_the_flag() {
        let e = dictate(vec![Ok("send the report press enter")], Arc::new(MemLexicon::default()), true);
        assert!(matches!(e, Event::Finished { enter: true, ref text, .. } if text == "Send the report"));
    }

    #[test]
    fn scratch_that_removes_the_entry_and_reports_it() {
        let hist = Arc::new(MemHistory::default());
        let (eng, rx, _, _) = start_with_lexicon(vec![Ok("scratch that")], hist.clone(), Arc::new(MemLexicon::default()));
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true });
        eng.send(Command::Audio(tone(500)));
        eng.send(Command::Finish);
        let mut scratched = false;
        loop {
            match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
                Event::Scratched => {
                    scratched = true;
                    break;
                }
                Event::Finished { .. } => break, // wrong: a scratch must not finish as text
                _ => {}
            }
        }
        assert!(scratched);
        assert!(hist.list(10).unwrap().is_empty());
    }

    #[test]
    fn snippets_expand_after_everything_else_and_keep_their_line_breaks() {
        let lexicon = Arc::new(MemLexicon::default());
        lexicon.add_snippet(&Fix { from: "my address".into(), to: "1 George St\nSydney NSW".into() }).unwrap();
        let e = dictate(vec![Ok("send it to my address")], lexicon, true);
        assert_eq!(e, Event::Finished { id: Some(1), text: "Send it to 1 George St\nSydney NSW.".into(), enter: false });
    }
}
