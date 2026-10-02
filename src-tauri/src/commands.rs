//! UI-facing commands. Thin: they validate nothing the core doesn't, and hold no state.

use crate::config::{HISTORY_LIMIT, TOGGLE_LABEL};
use crate::controller::Handle;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{AppHandle, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use vox_core::history::{History, Status};
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
