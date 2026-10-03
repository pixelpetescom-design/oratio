#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod commands;
mod config;
mod controller;
mod model_files;
mod overlay_window;
mod paths;

use std::sync::mpsc::channel;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};
use tauri_plugin_global_shortcut::ShortcutState;
use oratio_core::engine::Engine;
use oratio_core::apps::AppRules;
use oratio_core::history::History;
use oratio_core::lexicon::Lexicon;
use oratio_core::search::SearchKey;
use oratio_core::session::{Input, State};
use oratio_core::stt::Transcriber;
use oratio_store::SqliteStore;
use oratio_stt::WhisperTranscriber;

fn main() {
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        // Launched at sign-in with --hidden, Oratio starts quietly in the tray.
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::toggle_recording,
            commands::set_auto_paste,
            commands::set_enabled,
            commands::set_glass,
            commands::set_search,
            commands::set_behaviour,
            commands::list_models,
            commands::check_model_updates,
            commands::download_model,
            commands::cancel_model_download,
            commands::use_model,
            commands::delete_model,
            commands::set_overlay_position,
            commands::preview_overlay,
            commands::begin_move_overlay,
            commands::end_move_overlay,
            commands::list_microphones,
            commands::set_microphone,
            commands::list_app_rules,
            commands::add_app_rule,
            commands::remove_app_rule,
            commands::add_snippet,
            commands::remove_snippet,
            commands::get_autostart,
            commands::set_autostart,
            commands::list_history,
            commands::copy_text,
            commands::delete_entry,
            commands::clear_history,
            commands::list_lexicon,
            commands::add_word,
            commands::remove_word,
            commands::remove_fix,
            commands::edit_entry,
        ])
        .on_window_event(|window, event| {
            // Closing the main window keeps dictation alive in the tray.
            if let (WindowEvent::CloseRequested { api, .. }, "main") = (event, window.label()) {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();

            // History: durable on disk; an unusable disk degrades to memory instead of failing.
            let mut startup_problems = Vec::new();
            let data_dir = handle.path().app_data_dir().ok();
            let store: Arc<SqliteStore> = match data_dir
                .as_ref()
                .ok_or_else(|| "no data directory".to_string())
                .and_then(|d| std::fs::create_dir_all(d).map(|_| d.join("history.db")).map_err(|e| e.to_string()))
                .and_then(|p| SqliteStore::open(&p).map_err(|e| e.to_string()))
            {
                Ok(h) => Arc::new(h),
                Err(e) => {
                    startup_problems.push(format!("History cannot be saved to disk ({e}); using memory only for this session."));
                    Arc::new(SqliteStore::open_in_memory().map_err(|e| e.to_string())?)
                }
            };
            let history: Arc<dyn History> = store.clone();
            let lexicon: Arc<dyn Lexicon> = store.clone();
            let app_rules: Arc<dyn AppRules> = store;
            let _ = history.recover_interrupted();
            app.manage(history.clone());
            app.manage(lexicon.clone());
            app.manage(app_rules.clone());

            // Engine: loads the model off the UI thread; hotkey is ignored until it is ready.
            let (etx, erx) = channel();
            let candidates = paths::model_candidates(&handle);
            let loader: Box<dyn FnOnce() -> Result<Box<dyn Transcriber>, oratio_core::CoreError> + Send> = Box::new(move || {
                let model = paths::find_model(&candidates)?;
                Ok(Box::new(WhisperTranscriber::load(&model)?))
            });
            let engine = Engine::spawn(loader, history, lexicon, etx);
            let ctl = controller::spawn(handle.clone(), engine, app_rules, erx);
            if let Ok(mut p) = ctl.shared.problems.lock() {
                p.extend(startup_problems);
            }

            // Escape is a normal global hotkey, registered by the controller only while recording.
            let tx = ctl.clone();
            handle.plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(move |_app, shortcut, event| {
                        if event.state() == ShortcutState::Pressed && shortcut == &config::escape_shortcut() {
                            tx.send(Input::Escape);
                        }
                    })
                    .build(),
            )?;

            // Start/stop is the Ctrl+Win chord, which has to be watched for rather than registered.
            let (watch_key, on_chord, on_release, on_failure) = (ctl.clone(), ctl.clone(), ctl.clone(), ctl.clone());
            oratio_keys::spawn(
                move || {
                    watch_key.shared.search_enabled.load(Ordering::Relaxed).then(|| {
                        if watch_key.shared.search_key.load(Ordering::Relaxed) == 1 { SearchKey::Alt } else { SearchKey::Shift }
                    })
                },
                move |search_held| {
                    // Leave Ctrl+Win to Windows unless dictation is actually available.
                    let ready = on_chord.shared.state.lock().map(|s| matches!(*s, State::Idle | State::Recording | State::CancelPending { .. })).unwrap_or(false);
                    if ready {
                        if search_held {
                            on_chord.shared.search_pending.store(true, Ordering::Relaxed);
                        }
                        on_chord.send(Input::Toggle);
                    }
                    ready
                },
                // Hold-to-talk: letting go after a long hold ends the dictation. A quick tap leaves it
                // running, so tapping to start and tapping to stop still works.
                move |held_ms| {
                    let recording = on_release.shared.state.lock().map(|s| matches!(*s, State::Recording | State::CancelPending { .. })).unwrap_or(false);
                    if oratio_core::hotkey::stops_on_release(on_release.shared.hold_to_talk.load(Ordering::Relaxed), recording, held_ms, config::HOLD_TO_TALK_MS) {
                        on_release.send(Input::Toggle);
                    }
                },
                move |msg| {
                    if let Ok(mut p) = on_failure.shared.problems.lock() {
                        p.push(format!("{msg}. Use the Start button in this window."));
                    }
                },
            );
            app.manage(ctl);

            // Tray: reopen the window or quit.
            let show = MenuItem::with_id(app, "show", "Open Oratio", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::new().tooltip("Oratio").menu(&menu).show_menu_on_left_click(false);
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.on_menu_event(|app, event| match event.id.as_ref() {
                "show" => show_main(app),
                "quit" => app.exit(0),
                _ => {}
            })
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click { button: MouseButton::Left, .. } = event {
                    show_main(tray.app_handle());
                }
            })
            .build(app)?;

            // The window starts hidden so a sign-in launch (--hidden) stays in the tray.
            if !std::env::args().any(|a| a == "--hidden") {
                show_main(&handle);
            }
            Ok(())
        })
        .run(tauri::generate_context!());

    if let Err(e) = result {
        eprintln!("oratio failed to start: {e}");
    }
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}
