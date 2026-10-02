//! The shell's single writer. Every input (hotkeys, timers, engine events, UI
//! buttons) is queued onto one channel and applied in order on one thread, so
//! there are no races between, say, a double Escape and the cancel timer.
//! The pure rules live in `vox_core::session`; this file only performs effects.

use crate::config::{escape_shortcut, CANCEL_GRACE_MS, OVERLAY_LINGER_MS, TAIL_GRACE_MS};
use serde::Serialize;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::GlobalShortcutExt;
use vox_audio::Capture;
use vox_core::engine::{Command, Engine, Event};
use vox_core::session::{step, Effect, Input, State};

pub enum Msg {
    Input(Input),
    Engine(Event),
}

/// State readable from any thread (UI commands); written only by the controller thread.
pub struct Shared {
    pub state: Mutex<State>,
    pub problems: Mutex<Vec<String>>,
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
}

struct Controller {
    app: AppHandle,
    engine: Engine,
    shared: Arc<Shared>,
    tx: Sender<Msg>,
    clock: Instant,
    state: State,
    capture: Option<Capture>,
    escape_registered: bool,
    last_level: Instant,
}

/// Starts the controller thread. `engine_events` is the receiving end of the
/// channel the engine was spawned with.
pub fn spawn(app: AppHandle, engine: Engine, engine_events: std::sync::mpsc::Receiver<Event>) -> Handle {
    let (tx, rx) = channel::<Msg>();
    let shared = Arc::new(Shared { state: Mutex::new(State::Loading), problems: Mutex::new(vec![]) });

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
        engine,
        shared: shared.clone(),
        tx: tx.clone(),
        clock: Instant::now(),
        state: State::Loading,
        capture: None,
        escape_registered: false,
        last_level: Instant::now(),
    };
    let _ = std::thread::Builder::new().name("vox-controller".into()).spawn(move || {
        for msg in rx {
            match msg {
                Msg::Input(i) => ctl.apply(i),
                Msg::Engine(e) => ctl.on_engine(e),
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
                self.engine.send(Command::Begin { started_at_ms });
                let audio = self.engine.sender();
                match Capture::start(move |chunk| {
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
                std::thread::sleep(Duration::from_millis(TAIL_GRACE_MS));
                self.capture = None;
                self.engine.send(Command::Finish);
            }
            Effect::Discard => {
                self.capture = None;
                self.engine.send(Command::Discard);
            }
            Effect::Abort => self.capture = None,
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
                let _ = w.show();
                // Click-through must be applied once the native window exists, i.e. after show().
                let _ = w.set_ignore_cursor_events(true);
            }
        }
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
            if idle {
                if let Some(w) = app.get_webview_window("overlay") {
                    let _ = w.hide();
                }
            }
        });
    }

    fn on_engine(&mut self, ev: Event) {
        match ev {
            Event::Ready => self.apply(Input::EngineReady),
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
            Event::Finished { text, .. } => {
                // Clipboard first: the text is the product, everything else is bookkeeping.
                let copied = !text.is_empty() && self.app.clipboard().write_text(text.clone()).is_ok();
                if !text.is_empty() && !copied {
                    self.problem("Could not write to the clipboard; the text is saved in history.".into());
                }
                let _ = self.app.emit("finished", Finished { text, copied });
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
        }
    }

    fn problem(&self, message: String) {
        if let Ok(mut p) = self.shared.problems.lock() {
            p.push(message.clone());
        }
        let _ = self.app.emit("problem", message);
    }
}
