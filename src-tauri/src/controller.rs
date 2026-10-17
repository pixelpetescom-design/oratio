//! The shell's single writer. Every input (hotkeys, timers, engine events, UI
//! buttons) is queued onto one channel and applied in order on one thread, so
//! there are no races between, say, a double Escape and the cancel timer.
//! The pure rules live in `oratio_core::session`; this file only performs effects.

use crate::config::{escape_shortcut, CANCEL_GRACE_MS, CONTINUATION_WINDOW_MS, OVERLAY_LINGER_MS, PASTE_DELAY_MS, TAIL_GRACE_MS};
use serde::Serialize;
use std::sync::mpsc::{channel, Sender};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::GlobalShortcutExt;
use tauri_plugin_opener::OpenerExt;
use oratio_audio::Capture;
use oratio_core::engine::{Command, Engine, Event};
use oratio_core::polish::{append, continuation};
use oratio_core::apps::{action_for, AppAction, AppRules};
use oratio_core::meeting::{Line, MeetingId, Meetings, Source};
use oratio_core::search::{custom_url, query, route, url, Engine as SearchEngine};
use oratio_core::session::{step, Effect, Input, State};

pub enum Msg {
    Input(Input),
    Engine(Event),
    /// Switch to the speech model in this file.
    UseModel(std::path::PathBuf),
    MeetingStart,
    MeetingStop,
}

/// State readable from any thread (UI commands); written only by the controller thread.
pub struct Shared {
    pub state: Mutex<State>,
    pub problems: Mutex<Vec<String>>,
    /// Press Ctrl+V in the focused app after copying the result.
    pub auto_paste: AtomicBool,
    /// Voice search: hold the search key with Ctrl+Win to look the words up on Google instead of typing them.
    pub search_enabled: AtomicBool,
    /// 0 = Shift, 1 = Alt.
    pub search_key: AtomicU8,
    /// Set when the search key was held at the start or stop press of the current dictation.
    pub search_pending: AtomicBool,
    /// "Spoken commands": new line, full stop, scratch that, press enter…
    pub spoken_commands: AtomicBool,
    /// Holding Ctrl+Win records only while held.
    pub hold_to_talk: AtomicBool,
    /// Type each sentence as soon as it is recognised, instead of everything when dictation stops.
    pub live_typing: AtomicBool,
    /// Default voice-search engine id ("google", "youtube"…, or "custom").
    pub search_engine: Mutex<String>,
    /// Address template for the "custom" engine, with {q} for the query.
    pub search_custom: Mutex<String>,
    /// The key or chord that starts and stops dictation.
    pub trigger: Mutex<oratio_core::hotkey::Trigger>,
    /// Chosen microphone (None = system default).
    pub mic: Mutex<Option<String>>,
    /// Where the wave overlay appears.
    pub overlay_pos: Mutex<oratio_core::overlay::Position>,
    /// True while the user is dragging the overlay to a new spot (it stays visible and clickable).
    pub overlay_moving: AtomicBool,
    /// The last listing fetched from the model host (so a download can use its size and checksum).
    pub remote_models: Mutex<Vec<oratio_core::models::RemoteFile>>,
    /// A model download is running / has been asked to stop.
    pub model_busy: AtomicBool,
    pub model_cancel: AtomicBool,
    /// The meeting being recorded (or still being written up), or 0 when there is none.
    pub meeting_id: AtomicI64,
}

#[derive(Clone)]
pub struct Handle {
    tx: Sender<Msg>,
    pub shared: Arc<Shared>,
}

impl Handle {
    pub fn send(&self, input: Input) {
        let _ = self.tx.send(Msg::Input(input));
    }

    pub fn use_model(&self, path: std::path::PathBuf) {
        let _ = self.tx.send(Msg::UseModel(path));
    }

    pub fn start_meeting(&self) {
        let _ = self.tx.send(Msg::MeetingStart);
    }

    pub fn stop_meeting(&self) {
        let _ = self.tx.send(Msg::MeetingStop);
    }
}

#[derive(Serialize, Clone)]
struct StatePayload {
    #[serde(flatten)]
    state: State,
    remaining_ms: Option<u64>,
}

#[derive(Serialize, Clone)]
struct Finished {
    text: String,
    copied: bool,
    pasted: bool,
    /// The text was sent to a Google search instead of being typed.
    searched: bool,
    /// Typing is switched off for the focused app, so the text was only copied.
    copy_only: bool,
    /// From pressing Stop until the text was ready (before pasting).
    elapsed_ms: Option<u64>,
}

/// A meeting being recorded: both sides of the call, until Stop drops the captures.
struct ActiveMeeting {
    id: MeetingId,
    /// Microphone and computer audio. None once Stop has been pressed and the rest is being written up.
    captures: Option<(Capture, Capture)>,
}

struct Controller {
    app: AppHandle,
    meetings: Arc<dyn Meetings>,
    meeting: Option<ActiveMeeting>,
    engine: Engine,
    app_rules: Arc<dyn AppRules>,
    shared: Arc<Shared>,
    tx: Sender<Msg>,
    clock: Instant,
    state: State,
    capture: Option<Capture>,
    escape_registered: bool,
    last_level: Instant,
    /// When the user pressed Stop, to report how long the text took to arrive.
    stopped_at: Option<Instant>,
    /// When we last typed text into another app, to space consecutive dictations.
    last_paste: Option<Instant>,
    /// What that text was, so the next dictation can carry on from it.
    last_text: String,
    /// Something was typed live during the current dictation.
    live_typed: bool,
}

/// Starts the controller thread. `engine_events` is the receiving end of the
/// channel the engine was spawned with.
pub fn spawn(app: AppHandle, engine: Engine, app_rules: Arc<dyn AppRules>, meetings: Arc<dyn Meetings>, engine_events: std::sync::mpsc::Receiver<Event>) -> Handle {
    let (tx, rx) = channel::<Msg>();
    let shared = Arc::new(Shared { state: Mutex::new(State::Loading), problems: Mutex::new(vec![]), auto_paste: AtomicBool::new(true),
        search_enabled: AtomicBool::new(false),
        search_key: AtomicU8::new(0),
        search_pending: AtomicBool::new(false),
        spoken_commands: AtomicBool::new(true),
        hold_to_talk: AtomicBool::new(false),
        live_typing: AtomicBool::new(true),
        search_engine: Mutex::new("google".into()),
        search_custom: Mutex::new(String::new()),
        trigger: Mutex::new(oratio_core::hotkey::Trigger::DEFAULT),
        mic: Mutex::new(None),
        overlay_pos: Mutex::new(oratio_core::overlay::Position::default()),
        overlay_moving: AtomicBool::new(false),
        remote_models: Mutex::new(Vec::new()),
        model_busy: AtomicBool::new(false),
        model_cancel: AtomicBool::new(false),
        meeting_id: AtomicI64::new(0),
    });

    let forward = tx.clone();
    std::thread::spawn(move || {
        for ev in engine_events {
            if forward.send(Msg::Engine(ev)).is_err() {
                break;
            }
        }
    });

    let mut ctl = Controller {
        app,
        meetings,
        meeting: None,
        engine,
        app_rules,
        shared: shared.clone(),
        tx: tx.clone(),
        clock: Instant::now(),
        state: State::Loading,
        capture: None,
        escape_registered: false,
        last_level: Instant::now(),
        stopped_at: None,
        last_paste: None,
        last_text: String::new(),
        live_typed: false,
    };
    let _ = std::thread::Builder::new().name("oratio-controller".into()).spawn(move || {
        for msg in rx {
            match msg {
                Msg::Input(i) => ctl.apply(i),
                Msg::Engine(e) => ctl.on_engine(e),
                Msg::UseModel(path) => ctl.use_model(path),
                Msg::MeetingStart => ctl.start_meeting(),
                Msg::MeetingStop => ctl.stop_meeting(),
            }
        }
    });
    Handle { tx, shared }
}

impl Controller {
    fn now(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }

    fn apply(&mut self, input: Input) {
        if self.meeting.is_some() && matches!(input, Input::Toggle) {
            self.problem("A meeting is being recorded. Stop it before dictating.".into());
            return;
        }
        let prev = self.state;
        let (next, effect) = step(prev, input, self.now(), CANCEL_GRACE_MS);
        self.state = next;
        if let Some(fx) = effect {
            self.run(fx);
        }
        if self.state != prev {
            self.after_transition(prev);
        }
    }

    fn run(&mut self, fx: Effect) {
        match fx {
            Effect::StartRecording => {
                let started_at_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
                // What the focused app wants (chat style, or no typing at all) and what we last typed there,
                // so the new words fit in.
                let rule = oratio_window::active_app().and_then(|focused| self.app_rules.rules().ok().and_then(|rules| action_for(&rules, &focused)));
                let casual = rule.is_some_and(AppAction::is_casual);
                let previous = self.last_paste.filter(|t| t.elapsed() < Duration::from_millis(CONTINUATION_WINDOW_MS)).map(|_| self.last_text.clone());
                // Typing as you speak needs the keys to be up while it types, and a plain typed result (not a search).
                let live = self.shared.live_typing.load(Ordering::Relaxed)
                    && self.shared.auto_paste.load(Ordering::Relaxed)
                    && !self.shared.hold_to_talk.load(Ordering::Relaxed)
                    && !self.shared.search_pending.load(Ordering::Relaxed)
                    && rule != Some(AppAction::CopyOnly);
                self.live_typed = false;
                self.engine.send(Command::Begin { started_at_ms, spoken_commands: self.shared.spoken_commands.load(Ordering::Relaxed), previous, casual, live });
                let audio = self.engine.sender();
                let mic = self.shared.mic.lock().ok().and_then(|m| m.clone());
                match Capture::start(mic, move |chunk| {
                    let _ = audio.send(Command::Audio(chunk));
                }) {
                    Ok(c) => self.capture = Some(c),
                    Err(e) => {
                        self.engine.send(Command::Discard);
                        self.state = State::Idle;
                        self.problem(e.to_string());
                    }
                }
            }
            // Dropping the capture stops the mic and joins its thread, so every
            // chunk is already queued ahead of `Finish`.
            Effect::Finish => {
                self.stopped_at = Some(Instant::now());
                std::thread::sleep(Duration::from_millis(TAIL_GRACE_MS));
                self.capture = None;
                self.engine.send(Command::Finish);
            }
            Effect::Discard => {
                self.shared.search_pending.store(false, Ordering::Relaxed);
                self.capture = None;
                self.engine.send(Command::Discard);
            }
            Effect::Abort => {
                self.shared.search_pending.store(false, Ordering::Relaxed);
                self.capture = None;
            }
        }
    }

    fn after_transition(&mut self, prev: State) {
        if let Ok(mut s) = self.shared.state.lock() {
            *s = self.state;
        }
        let remaining_ms = self.state.deadline().map(|d| d.saturating_sub(self.now()));
        let _ = self.app.emit("state", StatePayload { state: self.state, remaining_ms });

        // Escape is grabbed globally only while it can mean "cancel".
        let want = self.state.wants_escape();
        if want != self.escape_registered {
            let gs = self.app.global_shortcut();
            let ok = if want { gs.register(escape_shortcut()) } else { gs.unregister(escape_shortcut()) };
            match ok {
                Ok(()) => self.escape_registered = want,
                Err(e) => self.problem(format!("Escape key: {e}")),
            }
        }

        if let (None, Some(deadline)) = (prev.deadline(), self.state.deadline()) {
            self.arm_timer(deadline);
        } else if prev.deadline().is_some() && self.state.deadline().is_some() && prev.deadline() != self.state.deadline() {
            if let Some(d) = self.state.deadline() {
                self.arm_timer(d);
            }
        }

        if matches!(self.state, State::Recording | State::CancelPending { .. } | State::Finalizing) {
            if let Some(w) = self.app.get_webview_window("overlay") {
                crate::overlay_window::position(&self.app, &self.shared);
                let _ = w.show();
            }
        }
    }

    /// Starts recording a meeting: your microphone and whatever the computer is playing, kept apart.
    fn start_meeting(&mut self) {
        if self.meeting.is_some() {
            return;
        }
        if !matches!(self.state, State::Idle) {
            self.problem("Wait until Oratio is ready and not dictating, then start the meeting.".into());
            return;
        }
        let started_at_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        let id = match self.meetings.begin_meeting(started_at_ms) {
            Ok(id) => id,
            Err(e) => return self.problem(format!("Could not start a meeting: {e}")),
        };
        self.engine.send(Command::MeetingBegin);
        let started = Instant::now();
        let sink = |source: Source| {
            let audio = self.engine.sender();
            move |samples: Vec<f32>| {
                let _ = audio.send(Command::MeetingAudio { source, at_ms: started.elapsed().as_millis() as u64, samples });
            }
        };
        let mic = self.shared.mic.lock().ok().and_then(|m| m.clone());
        let captures = Capture::start(mic, sink(Source::You)).and_then(|you| Capture::start_system(sink(Source::Others)).map(|others| (you, others)));
        match captures {
            Ok(c) => {
                self.meeting = Some(ActiveMeeting { id, captures: Some(c) });
                self.shared.meeting_id.store(id, Ordering::Relaxed);
                let _ = self.app.emit("meeting-state", Some(id));
            }
            Err(e) => {
                let _ = self.meetings.delete_meeting(id);
                self.engine.send(Command::MeetingEnd);
                self.problem(format!("Could not record the meeting: {e}"));
            }
        }
    }

    /// Stops listening. The sentences still being recognised are written up before the meeting is closed.
    fn stop_meeting(&mut self) {
        let Some(m) = self.meeting.as_mut() else { return };
        // Dropping the captures stops both streams and joins their threads, so all audio is queued before the end marker.
        if m.captures.take().is_some() {
            self.engine.send(Command::MeetingEnd);
        }
    }

    /// Loads another speech model in the background. Only allowed when nothing is being dictated.
    fn use_model(&mut self, path: std::path::PathBuf) {
        if !matches!(self.state, State::Idle | State::Off) {
            self.problem("Finish dictating before switching the speech model.".into());
            return;
        }
        self.apply(Input::EngineLoading);
        self.engine.send(Command::SwapModel(Box::new(move || crate::paths::load_transcriber(&path))));
    }

    fn arm_timer(&self, deadline: u64) {
        let wait = deadline.saturating_sub(self.now());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(wait));
            let _ = tx.send(Msg::Input(Input::Tick));
        });
    }

    fn hide_overlay_later(&self) {
        let app = self.app.clone();
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(OVERLAY_LINGER_MS));
            let idle = shared.state.lock().map(|s| matches!(*s, State::Idle)).unwrap_or(false);
            // Stay put while the user is positioning it.
            if idle && !shared.overlay_moving.load(Ordering::Relaxed) {
                if let Some(w) = app.get_webview_window("overlay") {
                    let _ = w.hide();
                }
            }
        });
    }

    fn on_engine(&mut self, ev: Event) {
        match ev {
            Event::Ready => {
                self.apply(Input::EngineReady);
                let _ = self.app.emit("model-ready", true);
            }
            Event::ModelSwapFailed(m) => {
                // The previous model is still loaded, so we are ready to dictate again.
                self.problem(format!("Could not load that speech model: {m}"));
                self.apply(Input::EngineReady);
                let _ = self.app.emit("model-ready", false);
            }
            Event::LoadFailed(m) => {
                self.problem(m);
                self.apply(Input::EngineFailed);
            }
            Event::Level(l) => {
                if self.last_level.elapsed() >= Duration::from_millis(40) {
                    self.last_level = Instant::now();
                    let _ = self.app.emit("level", l);
                }
            }
            Event::Segment { text, .. } => {
                let _ = self.app.emit("segment", text);
            }
            Event::Finished { text, enter, .. } => {
                let elapsed_ms = self.stopped_at.take().map(|t| t.elapsed().as_millis() as u64);
                let search = self.shared.search_pending.swap(false, Ordering::Relaxed)
                    && self.shared.search_enabled.load(Ordering::Relaxed)
                    && !text.is_empty();
                let (mut searched, mut copied, mut pasted, mut copy_only) = (false, false, false, false);
                if search {
                    let address = self.search_address(&text);
                    match self.app.opener().open_url(address, None::<&str>) {
                        Ok(()) => searched = true,
                        Err(e) => self.problem(format!("Could not open your browser ({e}). The text is saved in history.")),
                    }
                } else {
                    // What the focused app wants (per-app rules): don't type into it, or press Enter after.
                    let rule = oratio_window::active_app()
                        .and_then(|focused| self.app_rules.rules().ok().and_then(|rules| action_for(&rules, &focused)));
                    let auto_paste = self.shared.auto_paste.load(Ordering::Relaxed);
                    copy_only = auto_paste && rule == Some(AppAction::CopyOnly);
                    let type_it = auto_paste && !copy_only;
                    let press_enter = enter || rule.is_some_and(AppAction::presses_enter);

                    if !text.is_empty() {
                        // Clipboard first: the text is the product, everything else is bookkeeping.
                        // When typing straight after a previous dictation, lead with a space so the two
                        // don't run together; a manual paste stays clean.
                        let continuing = type_it && self.last_paste.is_some_and(|t| t.elapsed() < Duration::from_millis(CONTINUATION_WINDOW_MS));
                        let clip = if continuing { continuation(&text) } else { text.clone() };
                        copied = self.app.clipboard().write_text(clip).is_ok();
                        if !copied {
                            self.problem("Could not write to the clipboard; the text is saved in history.".into());
                        }
                    }
                    if type_it && (copied || (text.is_empty() && enter)) {
                        // Still holding Ctrl+Win would turn the paste into Win+V (clipboard history), so wait for
                        // the keys to come up rather than typing over them.
                        let released = oratio_keys::wait_for_modifiers_released(Duration::from_secs(10));
                        std::thread::sleep(Duration::from_millis(PASTE_DELAY_MS));
                        let typed = if !released {
                            Err(oratio_core::CoreError::Input("Ctrl, Win, Shift or Alt was still held down".into()))
                        } else if copied {
                            oratio_paste::paste_from_clipboard()
                        } else {
                            Ok(())
                        };
                        match typed {
                            Ok(()) => {
                                pasted = copied;
                                if copied {
                                    self.last_paste = Some(Instant::now());
                                    self.last_text = text.clone();
                                }
                                if press_enter {
                                    if let Err(e) = oratio_paste::press_enter() {
                                        self.problem(format!("Could not press Enter ({e})."));
                                    }
                                }
                            }
                            Err(e) => self.problem(format!("Could not type into the active app ({e}). The text is on your clipboard.")),
                        }
                    }
                }
                let _ = self.app.emit("finished", Finished { text, copied, pasted, searched, copy_only, elapsed_ms });
                let _ = self.app.emit("history", ());
                self.apply(Input::Finished);
                self.hide_overlay_later();
            }
            Event::Scratched => {
                // "Scratch that": undo the previous dictation's paste, if it is recent enough to be the thing on screen.
                let recent = self.last_paste.take().is_some_and(|t| t.elapsed() < Duration::from_millis(CONTINUATION_WINDOW_MS));
                let undone = recent
                    && self.shared.auto_paste.load(Ordering::Relaxed)
                    && oratio_keys::wait_for_modifiers_released(Duration::from_secs(10))
                    && oratio_paste::undo().is_ok();
                self.stopped_at = None;
                self.shared.search_pending.store(false, Ordering::Relaxed);
                let _ = self.app.emit("scratched", undone);
                let _ = self.app.emit("history", ());
                self.apply(Input::Finished);
                self.hide_overlay_later();
            }
            Event::Failed { reason } => {
                self.problem(reason);
                let _ = self.app.emit("history", ());
                self.apply(Input::FinishFailed);
                self.hide_overlay_later();
            }
            Event::Discarded => {
                let _ = self.app.emit("history", ());
                self.hide_overlay_later();
            }
            Event::Warning(m) => self.problem(m),
            Event::LiveText { text, enter } => self.type_live(&text, enter),
            Event::LiveScratch => {
                let recent = self.last_paste.is_some_and(|t| t.elapsed() < Duration::from_millis(CONTINUATION_WINDOW_MS));
                let undone = recent && oratio_keys::wait_for_modifiers_released(Duration::from_secs(2)) && oratio_paste::undo().is_ok();
                let _ = self.app.emit("scratched", undone);
            }
            Event::LiveFinished { text, .. } => {
                let elapsed_ms = self.stopped_at.take().map(|t| t.elapsed().as_millis() as u64);
                // The clipboard ends up holding the whole dictation, not just its last sentence.
                let copied = !text.is_empty() && self.app.clipboard().write_text(text.clone()).is_ok();
                let pasted = self.live_typed;
                let _ = self.app.emit("finished", Finished { text, copied, pasted, searched: false, copy_only: false, elapsed_ms });
                let _ = self.app.emit("history", ());
                self.apply(Input::Finished);
                self.hide_overlay_later();
            }
            Event::MeetingLine { source, start_ms, text } => {
                if let Some(m) = &self.meeting {
                    let line = Line { source, start_ms, text };
                    if let Err(e) = self.meetings.append_line(m.id, &line) {
                        self.problem(format!("A line of the meeting could not be saved: {e}"));
                    }
                    let _ = self.app.emit("meeting-line", line);
                }
            }
            Event::MeetingEnded => {
                if let Some(m) = self.meeting.take() {
                    let ended = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
                    if let Err(e) = self.meetings.finish_meeting(m.id, ended) {
                        self.problem(format!("The meeting could not be closed: {e}"));
                    }
                }
                self.shared.meeting_id.store(0, Ordering::Relaxed);
                let _ = self.app.emit("meeting-state", None::<MeetingId>);
            }
        }
    }

    /// Types one sentence into the focused app right now, while the user is still talking.
    fn type_live(&mut self, text: &str, enter: bool) {
        let rule = oratio_window::active_app().and_then(|focused| self.app_rules.rules().ok().and_then(|rules| action_for(&rules, &focused)));
        if rule == Some(AppAction::CopyOnly) {
            return; // the whole text is put on the clipboard when dictation ends
        }
        let recent = self.last_paste.is_some_and(|t| t.elapsed() < Duration::from_millis(CONTINUATION_WINDOW_MS));
        let insert = if recent { append(&self.last_text, text) } else { text.to_string() };
        if !insert.is_empty() {
            if self.app.clipboard().write_text(insert).is_err() {
                return;
            }
            // A held Ctrl/Win/Shift/Alt would turn the paste into some other shortcut, so wait for them to come up.
            if !oratio_keys::wait_for_modifiers_released(Duration::from_secs(2)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(PASTE_DELAY_MS));
            if let Err(e) = oratio_paste::paste_from_clipboard() {
                self.problem(format!("Could not type into the active app ({e}). The text is on your clipboard."));
                return;
            }
            self.last_paste = Some(Instant::now());
            self.last_text = text.to_string();
            self.live_typed = true;
        }
        if enter || rule.is_some_and(AppAction::presses_enter) {
            if let Err(e) = oratio_paste::press_enter() {
                self.problem(format!("Could not press Enter ({e})."));
            }
        }
    }

    /// Where a voice search goes: the chosen engine (or custom address), unless the spoken words
    /// start with an engine name ("youtube cute cats").
    fn search_address(&self, text: &str) -> String {
        let choice = self.shared.search_engine.lock().map(|e| e.clone()).unwrap_or_default();
        let custom = self.shared.search_custom.lock().map(|c| c.clone()).unwrap_or_default();
        let default = SearchEngine::parse(&choice).unwrap_or(SearchEngine::Google);
        let (engine, q) = route(&query(text), default);
        let spoken_engine = engine != default;
        if choice == "custom" && !spoken_engine {
            if let Some(address) = custom_url(&custom, &q) {
                return address;
            }
        }
        url(engine, &q)
    }

    fn problem(&self, message: String) {
        if let Ok(mut p) = self.shared.problems.lock() {
            p.push(message.clone());
        }
        let _ = self.app.emit("problem", message);
    }
}
