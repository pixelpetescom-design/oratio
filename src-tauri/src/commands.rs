//! UI-facing commands. Thin: they validate nothing the core doesn't, and hold no state.

use crate::config::{HISTORY_LIMIT, TOGGLE_LABEL};
use crate::controller::Handle;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::window::{Color, Effect, EffectsBuilder};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_clipboard_manager::ClipboardExt;
use vox_core::history::{History, Status};
use vox_core::lexicon::{learn_from_edit, Fix, Lexicon};
use vox_core::session::{Input, State as Phase};

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
}

#[tauri::command]
pub fn list_lexicon(lexicon: State<'_, Arc<dyn Lexicon>>) -> Result<LexiconView, String> {
    Ok(LexiconView { words: lexicon.words().map_err(|e| e.to_string())?, fixes: lexicon.fixes().map_err(|e| e.to_string())? })
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

/// Turns the frosted-glass window effect (Windows acrylic) on or off. Returns whether it is
/// actually showing, so the UI can fall back to a solid background where it isn't supported.
#[tauri::command]
pub fn set_glass(app: AppHandle, enabled: bool) -> bool {
    let Some(window) = app.get_webview_window("main") else { return false };
    let effects = enabled.then(|| EffectsBuilder::new().effect(Effect::Acrylic).color(Color(10, 15, 25, 120)).build());
    window.set_effects(effects).is_ok() && enabled && cfg!(windows)
}

/// Whether Vox is set to launch when the user signs in (read from the OS, the source of truth).
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
