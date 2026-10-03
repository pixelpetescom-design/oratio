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
