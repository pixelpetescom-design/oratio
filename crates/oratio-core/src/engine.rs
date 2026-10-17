//! The recognition engine: one background thread that owns the transcriber,
//! the segmenter and the history writes. A single ordered command channel means
//! audio can never overtake `Begin` or follow `Finish`.
//!
//! Failure policy: text is never lost to a secondary failure. Each utterance is
//! written to history the moment it is recognised; if history is unavailable the
//! text is still returned to the caller; recogniser panics become errors.

use crate::history::{History, RecordingId};
use crate::lexicon::{Fix, Lexicon};
use crate::meeting::Source;
use crate::backtrack;
use crate::commands;
use crate::numbers;
use crate::polish::{append, casual, collapse_repeats, follow_on, polish, LOOP_RUN};
use crate::vocab::{apply_fixes, replace_phrase};
use crate::spelling::to_australian;
use crate::segmenter::{normalize, rms, Segmenter, SegmenterConfig, SAMPLE_RATE};
use crate::stt::Transcriber;
use crate::CoreError;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

pub enum Command {
    Begin {
        started_at_ms: i64,
        spoken_commands: bool,
        /// What was typed just before, so the new text can carry on from it (lower case after no full stop).
        previous: Option<String>,
        /// Chat style: lower case, no closing full stop.
        casual: bool,
        /// Report each sentence as soon as it is recognised (`LiveText`) so it can be typed while the user is still talking.
        live: bool,
    },
    /// 16 kHz mono f32.
    Audio(Vec<f32>),
    Finish,
    Discard,
    /// Load a different speech model while running. Only sent when no dictation is in progress.
    SwapModel(Loader),
    /// A meeting starts: the next `MeetingAudio` belongs to it.
    MeetingBegin,
    /// 16 kHz mono audio from one side of the call. `at_ms` is when the chunk ended, counted from the
    /// start of the meeting, so speech is timed correctly even when the computer plays no sound for a while.
    MeetingAudio { source: Source, at_ms: u64, samples: Vec<f32> },
    MeetingEnd,
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
    /// The new model could not be loaded; the previous one is still in use.
    ModelSwapFailed(String),
    /// One sentence or so of a meeting, already tidied.
    MeetingLine { source: Source, start_ms: u64, text: String },
    /// Everything said in the meeting has been recognised.
    MeetingEnded,
    /// Live typing: the next sentence, finished and ready to type now. `enter`: press Enter after it.
    LiveText { text: String, enter: bool },
    /// Live typing: "scratch that", so the last typed sentence should be undone.
    LiveScratch,
    /// "Correction, change X to Y": put Y where X is in what was just typed.
    Replace { from: String, to: String },
    /// Live typing: the dictation is over. `text` is everything that was typed (empty if nothing was said).
    LiveFinished { id: Option<RecordingId>, text: String },
}

pub struct Engine {
    tx: Sender<Command>,
    handle: Option<JoinHandle<()>>,
}

pub type Loader = Box<dyn FnOnce() -> Result<Box<dyn Transcriber>, CoreError> + Send>;

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
    previous: Option<String>,
    casual: bool,
    live: bool,
    /// Live mode: the sentences already handed over for typing, in order.
    chunks: Vec<String>,
    /// Corrections the user spoke, to be remembered as learned fixes.
    learn: Vec<Fix>,
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
                // A model stuck in a loop repeats one phrase: keep one copy of a short repeat, cut a long loop out
                // entirely (the real words around it stay).
                let (text, longest_run) = collapse_repeats(text.trim());
                if longest_run >= LOOP_RUN {
                    eprintln!("[oratio] removed a repeating hallucination ({longest_run} repeats)");
                }
                if text.is_empty() {
                    return;
                }
                if let Err(e) = history.append_segment(take.id, &text) {
                    segmenter_events(Event::Warning(e.to_string()));
                }
                segmenter_events(Event::Segment { id: take.id, text: text.clone() });
                if take.live {
                    live_segment(take, &text, segmenter_events);
                }
                take.texts.push(text);
            }
            Ok(Err(e)) => take.last_error = Some(e.to_string()),
            Err(_) => take.last_error = Some("recogniser crashed".into()),
        }
    }

/// Remembers spoken corrections: the next time the wrong phrase is heard it is fixed automatically.
fn learn(lexicon: &dyn Lexicon, fixes: &[Fix]) {
    for fix in fixes {
        let _ = lexicon.add_fix(fix);
        if fix.to.split_whitespace().count() == 1 && fix.to.chars().count() >= 3 {
            let _ = lexicon.add_word(&fix.to);
        }
    }
}

/// Live typing: tidies one recognised sentence the way the whole-dictation pipeline would, and hands it over.
fn live_segment(take: &mut Take, raw: &str, emit: &dyn Fn(Event)) {
    let said = backtrack::apply(raw);
    let text = numbers::format(&apply_fixes(&to_australian(&polish(&[said]), &take.words), &take.fixes));
    let parsed = if take.spoken_commands { commands::apply(&text) } else { commands::Parsed { text, ..Default::default() } };
    if parsed.scratch {
        take.chunks.pop();
        emit(Event::LiveScratch);
        return;
    }
    if let Some((from, to)) = parsed.replace {
        // Fix the last sentence that has the phrase, so the saved text matches what is now on screen.
        if let Some((i, fixed)) = take.chunks.iter().enumerate().rev().find_map(|(i, c)| replace_phrase(c, &from, &to).map(|f| (i, f))) {
            take.chunks[i] = fixed;
        }
        take.learn.push(Fix { from: from.clone(), to: to.clone() });
        emit(Event::Replace { from, to });
        return;
    }
    let before = take.chunks.last().map(String::as_str).or(take.previous.as_deref());
    let styled = match before {
        _ if take.casual => casual(&parsed.text, &take.words),
        Some(previous) => follow_on(previous, &parsed.text, &take.words),
        None => parsed.text.clone(),
    };
    let text = apply_fixes(&styled, &take.snippets);
    if text.is_empty() && !parsed.enter {
        return;
    }
    if !text.is_empty() {
        take.chunks.push(text.clone());
    }
    emit(Event::LiveText { text, enter: parsed.enter });
}

/// What the engine keeps while a meeting is being recorded: one segmenter per side of the call.
struct MeetingRun {
    you: Segmenter,
    others: Segmenter,
    last_at: [u64; 2],
    fixes: Vec<Fix>,
    words: Vec<String>,
}

impl MeetingRun {
    fn side(&mut self, source: Source) -> (&mut Segmenter, &mut u64) {
        match source {
            Source::You => (&mut self.you, &mut self.last_at[0]),
            Source::Others => (&mut self.others, &mut self.last_at[1]),
        }
    }
}

/// Recognises one stretch of speech from a meeting and reports it as a transcript line.
fn meeting_line(transcriber: &mut dyn Transcriber, run: &MeetingRun, source: Source, end_ms: u64, mut audio: Vec<f32>, emit: &dyn Fn(Event)) {
    let length_ms = audio.len() as u64 * 1000 / SAMPLE_RATE as u64;
    normalize(&mut audio);
    match catch_unwind(AssertUnwindSafe(|| transcriber.transcribe(&audio))) {
        Ok(Ok(text)) => {
            let (text, _) = collapse_repeats(text.trim());
            if text.is_empty() {
                return;
            }
            let text = numbers::format(&apply_fixes(&to_australian(&polish(&[text]), &run.words), &run.fixes));
            emit(Event::MeetingLine { source, start_ms: end_ms.saturating_sub(length_ms), text });
        }
        Ok(Err(e)) => emit(Event::Warning(e.to_string())),
        Err(_) => emit(Event::Warning("the recogniser crashed on part of the meeting".into())),
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
    let mut meeting: Option<MeetingRun> = None;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::Begin { started_at_ms, spoken_commands, previous, casual, live } => {
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
                    Ok(id) => take = Some(Take { id, texts: vec![], last_error: None, max_level: 0.0, fixes, words: words.clone(), snippets, spoken_commands, previous, casual, live, chunks: vec![], learn: vec![] }),
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
                learn(lexicon.as_ref(), &std::mem::take(&mut t.learn));
                if t.live {
                    // Everything was typed as it was said; all that is left is to keep the full text.
                    let mut text = String::new();
                    for chunk in &t.chunks {
                        text.push_str(&append(&text, chunk));
                    }
                    if text.is_empty() {
                        let _ = history.delete(t.id);
                        emit(Event::LiveFinished { id: None, text });
                        continue;
                    }
                    if let Err(e) = history.complete(t.id, &text) {
                        emit(Event::Warning(e.to_string()));
                    }
                    emit(Event::LiveFinished { id: Some(t.id), text });
                    continue;
                }
                // Spelling first, so the user's own corrections always have the final word; spoken commands
                // and snippets come last so nothing earlier can disturb the line breaks they create.
                let said = backtrack::apply(&t.texts.join(" "));
                let text = numbers::format(&apply_fixes(&to_australian(&polish(&[said]), &t.words), &t.fixes));
                let parsed = if t.spoken_commands { commands::apply(&text) } else { commands::Parsed { text, ..Default::default() } };
                if parsed.scratch {
                    let _ = history.delete(t.id);
                    emit(Event::Scratched);
                    continue;
                }
                if let Some((from, to)) = parsed.replace {
                    let _ = history.delete(t.id);
                    learn(lexicon.as_ref(), &[Fix { from: from.clone(), to: to.clone() }]);
                    emit(Event::Replace { from, to });
                    emit(Event::Finished { id: None, text: String::new(), enter: false });
                    continue;
                }
                // Fit it to the surroundings: chat apps get a relaxed style, and a dictation that continues
                // the previous one carries on in lower case when that one stopped mid-sentence.
                let styled = match &t.previous {
                    _ if t.casual => casual(&parsed.text, &t.words),
                    Some(previous) => follow_on(previous, &parsed.text, &t.words),
                    None => parsed.text.clone(),
                };
                let text = apply_fixes(&styled, &t.snippets);
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
            Command::SwapModel(load) => match catch_unwind(AssertUnwindSafe(load)) {
                Ok(Ok(new)) => {
                    transcriber = new;
                    emit(Event::Ready);
                }
                Ok(Err(e)) => emit(Event::ModelSwapFailed(e.to_string())),
                Err(_) => emit(Event::ModelSwapFailed("the model loader crashed".into())),
            },
            Command::MeetingBegin => {
                let words = lexicon.words().unwrap_or_default();
                let fixes = lexicon.fixes().unwrap_or_default();
                transcriber.set_hints(&words);
                meeting = Some(MeetingRun { you: Segmenter::new(SegmenterConfig::default()), others: Segmenter::new(SegmenterConfig::default()), last_at: [0; 2], fixes, words });
            }
            Command::MeetingAudio { source, at_ms, samples } => {
                let Some(m) = meeting.as_mut() else { continue };
                let (seg, last) = m.side(source);
                *last = at_ms;
                for utterance in seg.push(&samples) {
                    meeting_line(transcriber.as_mut(), m, source, at_ms, utterance, &emit);
                }
            }
            Command::MeetingEnd => {
                if let Some(mut m) = meeting.take() {
                    for source in [Source::You, Source::Others] {
                        let (seg, last) = m.side(source);
                        let (end, rest) = (*last, seg.flush());
                        if let Some(rest) = rest {
                            meeting_line(transcriber.as_mut(), &m, source, end, rest, &emit);
                        }
                    }
                }
                emit(Event::MeetingEnded);
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
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true, previous: None, casual: false, live: false });
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
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true, previous: None, casual: false, live: false });
        eng.send(Command::Audio(vec![0.0; 16_000]));
        eng.send(Command::Finish);
        assert_eq!(until_finished(&rx).1, Event::Finished { id: None, text: String::new(), enter: false });
        assert!(hist.list(10).unwrap().is_empty());
    }

    #[test]
    fn discard_deletes_the_recording() {
        let hist = Arc::new(MemHistory::default());
        let (eng, rx) = start(vec![Ok("x")], hist.clone());
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true, previous: None, casual: false, live: false });
        eng.send(Command::Audio([tone(800), vec![0.0; 16 * 900]].concat()));
        eng.send(Command::Discard);
        assert_eq!(until_finished(&rx).1, Event::Discarded);
        assert!(hist.list(10).unwrap().is_empty());
    }

    #[test]
    fn text_survives_a_broken_history() {
        let hist = Arc::new(MemHistory { fail_writes: true, ..Default::default() });
        let (eng, rx) = start(vec![Ok("save me")], hist);
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true, previous: None, casual: false, live: false });
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
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true, previous: None, casual: false, live: false });
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
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true, previous: None, casual: false, live: false });
        eng.send(Command::Audio(tone(500)));
        eng.send(Command::Finish);
        assert_eq!(until_finished(&rx).1, Event::Finished { id: Some(1), text: "Open Postiz now.".into(), enter: false });
        assert_eq!(*hints.lock().unwrap(), vec!["Postiz".to_string()]);
    }

    #[test]
    fn output_uses_australian_spelling() {
        let (eng, rx) = start(vec![Ok("i like the color of the center")], Arc::new(MemHistory::default()));
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true, previous: None, casual: false, live: false });
        eng.send(Command::Audio(tone(500)));
        eng.send(Command::Finish);
        assert_eq!(until_finished(&rx).1, Event::Finished { id: Some(1), text: "I like the colour of the centre.".into(), enter: false });
    }

    fn dictate(script: Vec<Result<&'static str, &'static str>>, lexicon: Arc<MemLexicon>, commands: bool) -> Event {
        let (eng, rx, _, _) = start_with_lexicon(script, Arc::new(MemHistory::default()), lexicon);
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: commands, previous: None, casual: false, live: false });
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
    fn spoken_numbers_are_written_as_numbers() {
        let e = dictate(vec![Ok("it costs twenty five dollars um I I think")], Arc::new(MemLexicon::default()), true);
        assert_eq!(e, Event::Finished { id: Some(1), text: "It costs $25 I think.".into(), enter: false });
    }

    fn dictate_after(script: Vec<Result<&'static str, &'static str>>, previous: Option<&str>, casual: bool) -> Event {
        let (eng, rx, _, _) = start_with_lexicon(script, Arc::new(MemHistory::default()), Arc::new(MemLexicon::default()));
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true, previous: previous.map(String::from), casual, live: false });
        eng.send(Command::Audio(tone(500)));
        eng.send(Command::Finish);
        until_finished(&rx).1
    }

    #[test]
    fn dictation_carries_on_from_a_sentence_that_trailed_off() {
        let e = dictate_after(vec![Ok("bought some milk")], Some("We went to the shop and"), false);
        assert_eq!(e, Event::Finished { id: Some(1), text: "bought some milk.".into(), enter: false });
        let e = dictate_after(vec![Ok("bought some milk")], Some("We went to the shop."), false);
        assert_eq!(e, Event::Finished { id: Some(1), text: "Bought some milk.".into(), enter: false });
    }

    fn live_events(script: Vec<Result<&'static str, &'static str>>, previous: Option<&str>) -> Vec<Event> {
        let (eng, rx, _, _) = start_with_lexicon(script, Arc::new(MemHistory::default()), Arc::new(MemLexicon::default()));
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true, previous: previous.map(String::from), casual: false, live: true });
        for _ in 0..2 {
            eng.send(Command::Audio(tone(500)));
            eng.send(Command::Audio(vec![0.0; 16_000]));
        }
        eng.send(Command::Finish);
        let mut events = vec![];
        loop {
            let e = rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let done = matches!(e, Event::LiveFinished { .. });
            if matches!(e, Event::LiveText { .. } | Event::LiveScratch | Event::LiveFinished { .. }) {
                events.push(e);
            }
            if done {
                return events;
            }
        }
    }

    #[test]
    fn live_typing_hands_over_each_sentence_and_remembers_the_whole_text() {
        let events = live_events(vec![Ok("we went to the shop and"), Ok("bought some milk")], None);
        assert_eq!(
            events,
            vec![
                Event::LiveText { text: "We went to the shop and".into(), enter: false },
                Event::LiveText { text: "bought some milk.".into(), enter: false },
                Event::LiveFinished { id: Some(1), text: "We went to the shop and bought some milk.".into() },
            ]
        );
    }

    #[test]
    fn scratch_that_in_live_typing_takes_back_the_last_sentence() {
        let events = live_events(vec![Ok("hello there"), Ok("scratch that")], None);
        assert_eq!(
            events,
            vec![
                Event::LiveText { text: "Hello there.".into(), enter: false },
                Event::LiveScratch,
                Event::LiveFinished { id: None, text: String::new() },
            ]
        );
    }

    #[test]
    fn a_spoken_correction_is_reported_and_remembered() {
        let lexicon = Arc::new(MemLexicon::default());
        let e = dictate(vec![Ok("correction change tomorrow to Friday")], lexicon.clone(), true);
        assert_eq!(e, Event::Finished { id: None, text: String::new(), enter: false });
        assert_eq!(lexicon.fixes().unwrap(), vec![Fix { from: "tomorrow".into(), to: "Friday".into() }]);
    }

    #[test]
    fn chat_style_has_no_capital_or_full_stop() {
        let e = dictate_after(vec![Ok("Sounds good see you at six")], None, true);
        assert_eq!(e, Event::Finished { id: Some(1), text: "sounds good see you at six".into(), enter: false });
    }

    #[test]
    fn a_spoken_self_correction_keeps_only_the_final_thought() {
        let e = dictate(vec![Ok("we should meet tomorrow, no wait, Friday.")], Arc::new(MemLexicon::default()), true);
        assert_eq!(e, Event::Finished { id: Some(1), text: "We should meet Friday.".into(), enter: false });
    }

    #[test]
    fn a_meeting_labels_each_side_and_times_the_lines() {
        let (eng, rx, _, _) = start_with_lexicon(vec![Ok("hello everyone um thanks for joining"), Ok("morning twenty five of us")], Arc::new(MemHistory::default()), Arc::new(MemLexicon::default()));
        eng.send(Command::MeetingBegin);
        eng.send(Command::MeetingAudio { source: Source::You, at_ms: 5_000, samples: tone(500) });
        eng.send(Command::MeetingAudio { source: Source::Others, at_ms: 9_000, samples: tone(500) });
        eng.send(Command::MeetingEnd);
        let mut lines = vec![];
        loop {
            match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
                Event::MeetingLine { source, start_ms, text } => lines.push((source, start_ms, text)),
                Event::MeetingEnded => break,
                _ => {}
            }
        }
        assert_eq!(lines.len(), 2);
        assert_eq!((lines[0].0, lines[0].2.as_str()), (Source::You, "Hello everyone thanks for joining."));
        assert_eq!((lines[1].0, lines[1].2.as_str()), (Source::Others, "Morning 25 of us."));
        assert!(lines[0].1 < 5_000 && lines[1].1 < 9_000 && lines[1].1 > 7_000, "{lines:?}");
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
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true, previous: None, casual: false, live: false });
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

    #[test]
    fn a_pure_hallucination_loop_is_dropped_and_a_loop_inside_real_words_is_cut_out() {
        let looped: &'static str = Box::leak("Listening. ".repeat(40).into_boxed_str());
        let e = dictate(vec![Ok(looped)], Arc::new(MemLexicon::default()), true);
        assert_eq!(e, Event::Finished { id: None, text: String::new(), enter: false });
    }

    #[test]
    fn real_words_around_a_loop_are_kept() {
        let said: &'static str = Box::leak(format!("Peter space. {}Then my address.", "The name of the ".repeat(40)).into_boxed_str());
        let e = dictate(vec![Ok(said)], Arc::new(MemLexicon::default()), true);
        assert!(matches!(e, Event::Finished { ref text, .. } if text == "Peter space. Then my address."), "{e:?}");
    }

    #[test]
    fn swapping_models_switches_the_recogniser_and_a_failed_swap_keeps_the_old_one() {
        let (eng, rx, _, _) = start_with_lexicon(vec![Ok("from the first model")], Arc::new(MemHistory::default()), Arc::new(MemLexicon::default()));
        // A failing swap reports and changes nothing.
        eng.send(Command::SwapModel(Box::new(|| Err(CoreError::Stt("corrupt file".into())))));
        assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Event::ModelSwapFailed(m) if m.contains("corrupt")));
        eng.send(Command::Begin { started_at_ms: 1, spoken_commands: true, previous: None, casual: false, live: false });
        eng.send(Command::Audio(tone(500)));
        eng.send(Command::Finish);
        assert!(matches!(until_finished(&rx).1, Event::Finished { ref text, .. } if text == "From the first model."));

        // A good swap announces Ready and the next dictation uses the new model.
        let new = Fake(vec![Ok("from the second model")], Arc::default());
        eng.send(Command::SwapModel(Box::new(move || Ok(Box::new(new) as Box<dyn Transcriber>))));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Event::Ready);
        eng.send(Command::Begin { started_at_ms: 2, spoken_commands: true, previous: None, casual: false, live: false });
        eng.send(Command::Audio(tone(500)));
        eng.send(Command::Finish);
        assert!(matches!(until_finished(&rx).1, Event::Finished { ref text, .. } if text == "From the second model."));
    }
}
