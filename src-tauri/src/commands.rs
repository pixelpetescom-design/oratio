//! UI-facing commands. Thin: they validate nothing the core doesn't, and hold no state.

use crate::config::{HISTORY_LIMIT, TOGGLE_LABEL};
use crate::controller::Handle;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::window::{Color, Effect, EffectsBuilder};
use tauri::{AppHandle, Emitter, Manager, State};
use oratio_core::models::{describe, offered, RemoteFile};
use oratio_core::overlay::{Position, Preset};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_clipboard_manager::ClipboardExt;
use oratio_core::history::{History, Status};
use oratio_core::apps::{AppAction, AppRule, AppRules};
use oratio_core::lexicon::{learn_from_edit, Fix, Lexicon};
use oratio_core::session::{Input, State as Phase};

#[derive(Serialize)]
pub struct Snapshot {
    state: Phase,
    hotkey: &'static str,
    problems: Vec<String>,
    auto_paste: bool,
}

#[derive(Serialize)]
pub struct EntryView {
    id: i64,
    started_at_ms: i64,
    status: Status,
    text: String,
    error: Option<String>,
}

#[tauri::command]
pub fn get_snapshot(ctl: State<'_, Handle>) -> Snapshot {
    Snapshot {
        state: ctl.shared.state.lock().map(|s| *s).unwrap_or(Phase::Unavailable),
        hotkey: TOGGLE_LABEL,
        problems: ctl.shared.problems.lock().map(|p| p.clone()).unwrap_or_default(),
        auto_paste: ctl.shared.auto_paste.load(Ordering::Relaxed),
    }
}

#[tauri::command]
pub fn set_auto_paste(ctl: State<'_, Handle>, enabled: bool) {
    ctl.shared.auto_paste.store(enabled, Ordering::Relaxed);
}

#[tauri::command]
pub fn set_enabled(ctl: State<'_, Handle>, enabled: bool) {
    ctl.send(if enabled { Input::Enable } else { Input::Disable });
}

/// Voice search: on/off, the key to hold ("shift" or "alt"), the default engine id, and a custom address.
#[tauri::command]
pub fn set_search(ctl: State<'_, Handle>, enabled: bool, key: String, engine: String, custom: String) {
    ctl.shared.search_enabled.store(enabled, Ordering::Relaxed);
    ctl.shared.search_key.store(u8::from(key == "alt"), Ordering::Relaxed);
    if let Ok(mut e) = ctl.shared.search_engine.lock() {
        *e = engine;
    }
    if let Ok(mut c) = ctl.shared.search_custom.lock() {
        *c = custom;
    }
}

/// How dictation behaves: spoken commands on/off, and hold-to-talk.
#[tauri::command]
pub fn set_behaviour(ctl: State<'_, Handle>, spoken_commands: bool, hold_to_talk: bool) {
    ctl.shared.spoken_commands.store(spoken_commands, Ordering::Relaxed);
    ctl.shared.hold_to_talk.store(hold_to_talk, Ordering::Relaxed);
}

#[tauri::command]
pub fn list_microphones() -> Vec<String> {
    oratio_audio::input_devices()
}

/// Picks the microphone by name; an empty name means the system default.
#[tauri::command]
pub fn set_microphone(ctl: State<'_, Handle>, name: String) {
    if let Ok(mut m) = ctl.shared.mic.lock() {
        *m = (!name.trim().is_empty()).then_some(name);
    }
}

#[derive(Serialize)]
pub struct RuleView {
    pattern: String,
    action: AppAction,
}

#[tauri::command]
pub fn list_app_rules(rules: State<'_, Arc<dyn AppRules>>) -> Result<Vec<RuleView>, String> {
    Ok(rules.rules().map_err(|e| e.to_string())?.into_iter().map(|r| RuleView { pattern: r.pattern, action: r.action }).collect())
}

#[tauri::command]
pub fn add_app_rule(rules: State<'_, Arc<dyn AppRules>>, pattern: String, action: String) -> Result<(), String> {
    let pattern = pattern.trim().to_string();
    let action = AppAction::parse(&action).ok_or("unknown action")?;
    if pattern.is_empty() {
        return Err("enter part of the app's name".into());
    }
    rules.add_rule(&AppRule { pattern, action }).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_app_rule(rules: State<'_, Arc<dyn AppRules>>, pattern: String) -> Result<(), String> {
    rules.remove_rule(&pattern).map_err(|e| e.to_string())
}

/// Snippet triggers are matched as lower-case words, so normalise what the user typed the same way.
#[tauri::command]
pub fn add_snippet(lexicon: State<'_, Arc<dyn Lexicon>>, trigger: String, text: String) -> Result<(), String> {
    let from = trigger
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if from.is_empty() || text.trim().is_empty() {
        return Err("enter a trigger phrase and the text it should become".into());
    }
    lexicon.add_snippet(&Fix { from, to: text.trim_end().to_string() }).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_snippet(lexicon: State<'_, Arc<dyn Lexicon>>, trigger: String) -> Result<(), String> {
    lexicon.remove_snippet(&trigger).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn toggle_recording(ctl: State<'_, Handle>) {
    ctl.send(Input::Toggle);
}

#[tauri::command]
pub fn list_history(history: State<'_, Arc<dyn History>>) -> Result<Vec<EntryView>, String> {
    let entries = history.list(HISTORY_LIMIT).map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .map(|e| {
            let text = e.text();
            EntryView { id: e.id, started_at_ms: e.started_at_ms, status: e.status, text, error: e.error }
        })
        .collect())
}

#[tauri::command]
pub fn copy_text(app: AppHandle, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_entry(history: State<'_, Arc<dyn History>>, id: i64) -> Result<(), String> {
    history.delete(id).map_err(|e| e.to_string())
}

/// Deletes the whole history. Anything left "recording" by an interrupted session is swept up first,
/// unless a dictation is genuinely in progress right now.
#[tauri::command]
pub fn clear_history(ctl: State<'_, Handle>, history: State<'_, Arc<dyn History>>) -> Result<usize, String> {
    let busy = ctl.shared.state.lock().map(|s| matches!(*s, Phase::Recording | Phase::CancelPending { .. } | Phase::Finalizing)).unwrap_or(true);
    if !busy {
        history.recover_interrupted().map_err(|e| e.to_string())?;
    }
    history.clear().map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct LexiconView {
    words: Vec<String>,
    fixes: Vec<Fix>,
    snippets: Vec<Fix>,
}

#[tauri::command]
pub fn list_lexicon(lexicon: State<'_, Arc<dyn Lexicon>>) -> Result<LexiconView, String> {
    Ok(LexiconView { words: lexicon.words().map_err(|e| e.to_string())?, fixes: lexicon.fixes().map_err(|e| e.to_string())?, snippets: lexicon.snippets().map_err(|e| e.to_string())? })
}

#[tauri::command]
pub fn add_word(lexicon: State<'_, Arc<dyn Lexicon>>, word: String) -> Result<(), String> {
    lexicon.add_word(&word).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_word(lexicon: State<'_, Arc<dyn Lexicon>>, word: String) -> Result<(), String> {
    lexicon.remove_word(&word).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_fix(lexicon: State<'_, Arc<dyn Lexicon>>, from: String) -> Result<(), String> {
    lexicon.remove_fix(&from).map_err(|e| e.to_string())
}

/// Saves a corrected transcript and learns from it; returns how many corrections were learned.
#[tauri::command]
pub fn edit_entry(
    history: State<'_, Arc<dyn History>>,
    lexicon: State<'_, Arc<dyn Lexicon>>,
    id: i64,
    text: String,
) -> Result<usize, String> {
    learn_from_edit(history.as_ref(), lexicon.as_ref(), id, &text).map_err(|e| e.to_string())
}

/// Sets the window style: "solid", "clear" (see-through, no blur), "soft" (Windows blur) or "frosted"
/// (Windows acrylic). Returns whether the window should be drawn see-through, so the UI falls back
/// to a solid background where that isn't supported.
#[tauri::command]
pub fn set_glass(app: AppHandle, style: String) -> bool {
    let Some(window) = app.get_webview_window("main") else { return false };
    // The native layer only supplies the blur; the darkness is controlled by the page's own tint.
    let native = |effect| Some(EffectsBuilder::new().effect(effect).color(Color(8, 12, 22, 40)).build());
    let effects = match style.as_str() {
        "frosted" => native(Effect::Acrylic),
        "soft" => native(Effect::Blur),
        _ => None,
    };
    window.set_effects(effects).is_ok() && style != "solid" && cfg!(windows)
}

/// Whether Oratio is set to launch when the user signs in (read from the OS, the source of truth).
#[tauri::command]
pub fn get_autostart(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let launcher = app.autolaunch();
    if enabled { launcher.enable() } else { launcher.disable() }.map_err(|e| e.to_string())?;
    launcher.is_enabled().map_err(|e| e.to_string())
}

/// Where the wave overlay appears: a preset spot ("top_left"…) or, with kind "custom", the pixel position
/// the user dragged it to.
#[tauri::command]
pub fn set_overlay_position(ctl: State<'_, Handle>, kind: String, id: String, x: i32, y: i32) {
    let position = if kind == "custom" { Position::Custom(x, y) } else { Preset::parse(&id).map_or_else(Position::default, Position::Preset) };
    if let Ok(mut p) = ctl.shared.overlay_pos.lock() {
        *p = position;
    }
}

/// Briefly shows the overlay at its current spot so the user can see where it will appear.
#[tauri::command]
pub fn preview_overlay(app: AppHandle, ctl: State<'_, Handle>) {
    let Some(window) = app.get_webview_window("overlay") else { return };
    crate::overlay_window::position(&app, &ctl.shared);
    let _ = window.show();
    let _ = window.set_ignore_cursor_events(true);
    let _ = app.emit("overlay-preview", ());
    let shared = ctl.shared.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1800));
        let busy = shared.state.lock().map(|s| !matches!(*s, Phase::Idle | Phase::Off)).unwrap_or(true);
        if !busy && !shared.overlay_moving.load(Ordering::Relaxed) {
            let _ = window.hide();
        }
    });
}

/// Starts "drag it anywhere" mode: the overlay becomes visible and grabbable until `end_move_overlay`.
#[tauri::command]
pub fn begin_move_overlay(app: AppHandle, ctl: State<'_, Handle>) {
    let Some(window) = app.get_webview_window("overlay") else { return };
    ctl.shared.overlay_moving.store(true, Ordering::Relaxed);
    crate::overlay_window::position(&app, &ctl.shared);
    let _ = window.show();
    let _ = window.set_ignore_cursor_events(false);
    let _ = app.emit("overlay-move", true);
}

/// Ends drag mode, remembers where the overlay was left, and returns that position.
#[tauri::command]
pub fn end_move_overlay(app: AppHandle, ctl: State<'_, Handle>) -> Result<(i32, i32), String> {
    let window = app.get_webview_window("overlay").ok_or("overlay window missing")?;
    let at = window.outer_position().map_err(|e| e.to_string())?;
    ctl.shared.overlay_moving.store(false, Ordering::Relaxed);
    if let Ok(mut p) = ctl.shared.overlay_pos.lock() {
        *p = Position::Custom(at.x, at.y);
    }
    let _ = window.set_ignore_cursor_events(true);
    let _ = window.hide();
    let _ = app.emit("overlay-move", false);
    Ok((at.x, at.y))
}

// ---- speech models ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct ModelView {
    file: String,
    name: String,
    quant: String,
    english_only: bool,
    size_mb: u64,
    installed: bool,
    active: bool,
    bundled: bool,
    /// Published since the last time the user checked.
    is_new: bool,
    /// What this build of Oratio is designed around.
    recommended: bool,
}

fn view(file: &str, size: u64, installed: bool, bundled: bool, active: &str, is_new: bool) -> ModelView {
    let info = describe(&RemoteFile { path: file.to_string(), size, sha256: None });
    ModelView {
        file: file.to_string(),
        name: info.as_ref().map_or_else(|| file.trim_end_matches(".bin").to_string(), |i| i.name.clone()),
        quant: info.as_ref().map_or_else(String::new, |i| i.quant.clone()),
        english_only: info.as_ref().is_some_and(|i| i.english_only),
        size_mb: size / 1_000_000,
        installed,
        active: file == active,
        bundled,
        is_new,
        recommended: file == crate::config::MODEL_FILE,
    }
}

fn active_file(app: &AppHandle) -> String {
    crate::model_files::active(app).unwrap_or_else(|| crate::config::MODEL_FILE.to_string())
}

/// Models on this computer. Works offline.
#[tauri::command]
pub fn list_models(app: AppHandle) -> Vec<ModelView> {
    let active = active_file(&app);
    crate::model_files::installed(&app).into_iter().map(|m| view(&m.file, m.size, true, m.bundled, &active, false)).collect()
}

/// Asks the public model host what is available. This is the only time Oratio goes online, and only
/// because the user pressed the button; nothing about them is sent.
#[tauri::command]
pub async fn check_model_updates(app: AppHandle, ctl: State<'_, Handle>) -> Result<Vec<ModelView>, String> {
    let files = tauri::async_runtime::spawn_blocking(|| oratio_models::list(&oratio_models::Hub::huggingface()))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let offered_models = offered(&files);
    let installed = crate::model_files::installed(&app);
    let active = active_file(&app);
    // The first check sets the baseline; afterwards anything not seen before is flagged as new.
    let previously_seen = crate::model_files::seen(&app);
    let names: Vec<String> = offered_models.iter().map(|m| m.file.clone()).collect();
    let views = offered_models
        .iter()
        .map(|m| {
            let have = installed.iter().find(|i| i.file == m.file);
            let is_new = previously_seen.as_ref().is_some_and(|seen| !seen.contains(&m.file));
            view(&m.file, m.size, have.is_some(), have.is_some_and(|i| i.bundled), &active, is_new)
        })
        .collect();
    crate::model_files::remember_seen(&app, &names);
    if let Ok(mut r) = ctl.shared.remote_models.lock() {
        *r = files;
    }
    Ok(views)
}

#[derive(Serialize, Clone)]
struct Progress {
    file: String,
    done: u64,
    total: u64,
}

/// Starts downloading a model in the background; progress arrives as "model-progress" events and the
/// result as "model-downloaded" / "problem".
#[tauri::command]
pub fn download_model(app: AppHandle, ctl: State<'_, Handle>, file: String) -> Result<(), String> {
    let remote = ctl
        .shared
        .remote_models
        .lock()
        .map_err(|e| e.to_string())?
        .iter()
        .find(|f| f.path == file)
        .cloned()
        .ok_or("check for new models first")?;
    let dir = crate::model_files::models_dir(&app).ok_or("no data folder")?;
    if ctl.shared.model_busy.swap(true, Ordering::Relaxed) {
        return Err("a download is already running".into());
    }
    ctl.shared.model_cancel.store(false, Ordering::Relaxed);
    let shared = ctl.shared.clone();
    std::thread::spawn(move || {
        let name = remote.path.clone();
        let result = oratio_models::download(&oratio_models::Hub::huggingface(), &remote, &dir, &shared.model_cancel, |done, total| {
            let _ = app.emit("model-progress", Progress { file: name.clone(), done, total });
        });
        shared.model_busy.store(false, Ordering::Relaxed);
        match result {
            Ok(_) => {
                let _ = app.emit("model-downloaded", name);
            }
            Err(e) => {
                let cancelled = e.to_string().contains("cancelled");
                let _ = app.emit("model-download-failed", (name, cancelled, e.to_string()));
            }
        }
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_model_download(ctl: State<'_, Handle>) {
    ctl.shared.model_cancel.store(true, Ordering::Relaxed);
}

/// Switches to an installed model (loaded in the background) and remembers the choice.
#[tauri::command]
pub fn use_model(app: AppHandle, ctl: State<'_, Handle>, file: String) -> Result<(), String> {
    let path = crate::model_files::path_of(&app, &file).ok_or("that model isn't installed")?;
    crate::model_files::set_active(&app, &file).map_err(|e| e.to_string())?;
    ctl.use_model(path);
    Ok(())
}

/// Deletes a downloaded model (not the one that came with the installer, and not the one in use).
#[tauri::command]
pub fn delete_model(app: AppHandle, file: String) -> Result<(), String> {
    if file == active_file(&app) {
        return Err("that model is in use; switch to another one first".into());
    }
    let model = crate::model_files::installed(&app).into_iter().find(|m| m.file == file).ok_or("not found")?;
    if model.bundled {
        return Err("that model came with the installer".into());
    }
    std::fs::remove_file(model.path).map_err(|e| e.to_string())
}
