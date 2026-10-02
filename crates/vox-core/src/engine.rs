//! The recognition engine: one background thread that owns the transcriber,
//! the segmenter and the history writes. A single ordered command channel means
//! audio can never overtake `Begin` or follow `Finish`.
//!
//! Failure policy: text is never lost to a secondary failure. Each utterance is
//! written to history the moment it is recognised; if history is unavailable the
//! text is still returned to the caller; recogniser panics become errors.

use crate::history::{History, RecordingId};
use crate::polish::polish;
use crate::segmenter::{normalize, rms, Segmenter, SegmenterConfig, SAMPLE_RATE};
use crate::stt::Transcriber;
use crate::CoreError;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

pub enum Command {
    Begin { started_at_ms: i64 },
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
    Finished { id: Option<RecordingId>, text: String },
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
    pub fn spawn(load: Loader, history: Arc<dyn History>, events: Sender<Event>) -> Engine {
        let (tx, rx) = channel();
        let handle = std::thread::Builder::new()
            .name("vox-engine".into())
            .spawn(move || run(load, history, rx, events))
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
}

fn run(load: Loader, history: Arc<dyn History>, rx: Receiver<Command>, events: Sender<Event>) {
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

    let mut recognise = |take: &mut Take, mut audio: Vec<f32>, segmenter_events: &dyn Fn(Event)| {
        normalize(&mut audio);
        let started = std::time::Instant::now();
        let result = catch_unwind(AssertUnwindSafe(|| transcriber.transcribe(&audio)));
        eprintln!(
            "[vox] utterance {:.1}s -> {} in {} ms",
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
    };

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::Begin { started_at_ms } => {
                segmenter = Segmenter::new(SegmenterConfig::default());
                if let Some(old) = take.take() {
                    let _ = history.fail(old.id, "superseded by a new recording");
                }
                match history.begin(started_at_ms) {
                    Ok(id) => take = Some(Take { id, texts: vec![], last_error: None, max_level: 0.0 }),
                    Err(e) => emit(Event::Failed { reason: e.to_string() }),
                }
            }
            Command::Audio(chunk) => {
                let Some(t) = take.as_mut() else { continue };
                let level = rms(&chunk);
                t.max_level = t.max_level.max(level);
                emit(Event::Level(level));
                for utterance in segmenter.push(&chunk) {
                    recognise(t, utterance, &emit);
                }
            }
            Command::Finish => {
                let Some(mut t) = take.take() else {
                    emit(Event::Finished { id: None, text: String::new() });
                    continue;
                };
                if let Some(rest) = segmenter.flush() {
                    recognise(&mut t, rest, &emit);
                }
                eprintln!(
                    "[vox] stopped: {} utterance(s) recognised, loudest level {:.4}, background noise {:.4}, short sounds ignored {}",
                    t.texts.len(),
                    t.max_level,
                    segmenter.noise_floor(),
                    segmenter.dropped()
                );
                if t.texts.is_empty() {
                    let _ = history.delete(t.id);
                    match t.last_error {
                        Some(reason) => emit(Event::Failed { reason }),
                        None => emit(Event::Finished { id: None, text: String::new() }),
                    }
                    continue;
                }
                let text = polish(&t.texts);
                if let Err(e) = history.complete(t.id, &text) {
                    emit(Event::Warning(e.to_string()));
                }
                if let Some(reason) = t.last_error {
                    emit(Event::Warning(reason));
                }
                emit(Event::Finished { id: Some(t.id), text });
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
    use crate::history::{Entry, Status};
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Default)]
    struct MemHistory {
        rows: Mutex<Vec<Entry>>,
        fail_writes: bool,
    }
    impl History for MemHistory {
        fn begin(&self, started_at_ms: i64) -> Result<RecordingId, CoreError> {
            let mut r = self.rows.lock().unwrap();
            let id = r.len() as i64 + 1;
            r.push(Entry { id, started_at_ms, status: Status::Recording, segments: vec![], final_text: None, error: None });
            Ok(id)
        }
        fn append_segment(&self, id: RecordingId, text: &str) -> Result<(), CoreError> {
            if self.fail_writes {
                return Err(CoreError::History("disk full".into()));
            }
            self.rows.lock().unwrap().iter_mut().find(|e| e.id == id).unwrap().segments.push(text.into());
            Ok(())
        }
        fn complete(&self, id: RecordingId, t: &str) -> Result<(), CoreError> {
            if self.fail_writes {
                return Err(CoreError::History("disk full".into()));
            }
            let mut r = self.rows.lock().unwrap();
            let e = r.iter_mut().find(|e| e.id == id).unwrap();
            e.status = Status::Completed;
            e.final_text = Some(t.into());
            Ok(())
        }
        fn fail(&self, _: RecordingId, _: &str) -> Result<(), CoreError> {
            Ok(())
        }
        fn delete(&self, id: RecordingId) -> Result<(), CoreError> {
            self.rows.lock().unwrap().retain(|e| e.id != id);
            Ok(())
        }
        fn list(&self, _: u32) -> Result<Vec<Entry>, CoreError> {
            Ok(self.rows.lock().unwrap().clone())
        }
        fn recover_interrupted(&self) -> Result<usize, CoreError> {
            Ok(0)
        }
    }

    struct Fake(Vec<Result<&'static str, &'static str>>);
    impl Transcriber for Fake {
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
        let (etx, erx) = channel();
        let eng = Engine::spawn(Box::new(move || Ok(Box::new(Fake(script)) as Box<dyn Transcriber>)), hist, etx);
        assert_eq!(erx.recv_timeout(Duration::from_secs(2)).unwrap(), Event::Ready);
        (eng, erx)
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
        eng.send(Command::Begin { started_at_ms: 1 });
        eng.send(Command::Audio([tone(800), vec![0.0; 16 * 900]].concat()));
        eng.send(Command::Audio(tone(400))); // still talking when stop is pressed
        eng.send(Command::Finish);
        let (seen, done) = until_finished(&rx);
        assert!(seen.iter().any(|e| matches!(e, Event::Segment { text, .. } if text == "hello there")));
        assert_eq!(done, Event::Finished { id: Some(1), text: "Hello there how are you.".into() });
        let rows = hist.list(10).unwrap();
        assert_eq!(rows[0].status, Status::Completed);
        assert_eq!(rows[0].segments.len(), 2);
    }

    #[test]
    fn silence_yields_empty_result_and_no_history_row() {
        let hist = Arc::new(MemHistory::default());
        let (eng, rx) = start(vec![], hist.clone());
        eng.send(Command::Begin { started_at_ms: 1 });
        eng.send(Command::Audio(vec![0.0; 16_000]));
        eng.send(Command::Finish);
        assert_eq!(until_finished(&rx).1, Event::Finished { id: None, text: String::new() });
        assert!(hist.list(10).unwrap().is_empty());
    }

    #[test]
    fn discard_deletes_the_recording() {
        let hist = Arc::new(MemHistory::default());
        let (eng, rx) = start(vec![Ok("x")], hist.clone());
        eng.send(Command::Begin { started_at_ms: 1 });
        eng.send(Command::Audio([tone(800), vec![0.0; 16 * 900]].concat()));
        eng.send(Command::Discard);
        assert_eq!(until_finished(&rx).1, Event::Discarded);
        assert!(hist.list(10).unwrap().is_empty());
    }

    #[test]
    fn text_survives_a_broken_history() {
        let hist = Arc::new(MemHistory { fail_writes: true, ..Default::default() });
        let (eng, rx) = start(vec![Ok("save me")], hist);
        eng.send(Command::Begin { started_at_ms: 1 });
        eng.send(Command::Audio(tone(500)));
        eng.send(Command::Finish);
        let (seen, done) = until_finished(&rx);
        assert_eq!(done, Event::Finished { id: Some(1), text: "Save me.".into() });
        assert!(seen.iter().any(|e| matches!(e, Event::Warning(_))));
    }

    #[test]
    fn recogniser_error_with_no_text_is_reported() {
        let hist = Arc::new(MemHistory::default());
        let (eng, rx) = start(vec![Err("boom")], hist);
        eng.send(Command::Begin { started_at_ms: 1 });
        eng.send(Command::Audio(tone(500)));
        eng.send(Command::Finish);
        assert!(matches!(until_finished(&rx).1, Event::Failed { reason } if reason.contains("boom")));
    }

    #[test]
    fn load_failure_is_reported_not_panicked() {
        let (etx, erx) = channel();
        let _eng = Engine::spawn(Box::new(|| Err(CoreError::Stt("no model".into()))), Arc::new(MemHistory::default()), etx);
        assert!(matches!(erx.recv_timeout(Duration::from_secs(2)).unwrap(), Event::LoadFailed(m) if m.contains("no model")));
    }
}
