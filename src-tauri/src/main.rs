#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod commands;
mod config;
mod controller;
mod paths;

use std::sync::mpsc::channel;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, PhysicalPosition, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use vox_core::engine::Engine;
use vox_core::history::History;
use vox_core::session::Input;
use vox_core::stt::Transcriber;
use vox_store::SqliteHistory;
use vox_stt::WhisperTranscriber;

fn main() {
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::toggle_recording,
            commands::set_auto_paste,
            commands::list_history,
            commands::copy_text,
            commands::delete_entry,
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
            let history: Arc<dyn History> = match data_dir
                .as_ref()
                .ok_or_else(|| "no data directory".to_string())
                .and_then(|d| std::fs::create_dir_all(d).map(|_| d.join("history.db")).map_err(|e| e.to_string()))
                .and_then(|p| SqliteHistory::open(&p).map_err(|e| e.to_string()))
            {
                Ok(h) => Arc::new(h),
                Err(e) => {
                    startup_problems.push(format!("History cannot be saved to disk ({e}); using memory only for this session."));
                    Arc::new(SqliteHistory::open_in_memory().map_err(|e| e.to_string())?)
                }
            };
            let _ = history.recover_interrupted();
            app.manage(history.clone());

            // Engine: loads the model off the UI thread; hotkey is ignored until it is ready.
            let (etx, erx) = channel();
            let candidates = paths::model_candidates(&handle);
            let loader: Box<dyn FnOnce() -> Result<Box<dyn Transcriber>, vox_core::CoreError> + Send> = Box::new(move || {
                let model = paths::find_model(&candidates)?;
                Ok(Box::new(WhisperTranscriber::load(&model)?))
            });
            let engine = Engine::spawn(loader, history, etx);
            let ctl = controller::spawn(handle.clone(), engine, erx);
            if let Ok(mut p) = ctl.shared.problems.lock() {
                p.extend(startup_problems);
            }

            // Global hotkeys. Escape is registered by the controller only while recording.
            let tx = ctl.clone();
            let toggle = config::toggle_shortcut();
            handle.plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(move |_app, shortcut, event| {
                        if event.state() != ShortcutState::Pressed {
                            return;
                        }
                        if shortcut == &toggle {
                            tx.send(Input::Toggle);
                        } else if shortcut == &config::escape_shortcut() {
                            tx.send(Input::Escape);
                        }
                    })
                    .build(),
            )?;
            if let Err(e) = handle.global_shortcut().register(config::toggle_shortcut()) {
                if let Ok(mut p) = ctl.shared.problems.lock() {
                    p.push(format!("Hotkey {} is unavailable ({e}). Use the button in the window.", config::TOGGLE_LABEL));
                }
            }
            app.manage(ctl);

            // Overlay: a click-through pill at the top centre of the primary screen.
            if let Some(overlay) = app.get_webview_window("overlay") {
                if let (Ok(Some(m)), Ok(size)) = (overlay.primary_monitor(), overlay.outer_size()) {
                    let x = m.position().x + (m.size().width as i32 - size.width as i32) / 2;
                    let y = m.position().y + (24.0 * m.scale_factor()) as i32;
                    let _ = overlay.set_position(PhysicalPosition::new(x, y));
                }
            }

            // Tray: reopen the window or quit.
            let show = MenuItem::with_id(app, "show", "Open Vox", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::new().tooltip("Vox").menu(&menu).show_menu_on_left_click(false);
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
            Ok(())
        })
        .run(tauri::generate_context!());

    if let Err(e) = result {
        eprintln!("vox failed to start: {e}");
    }
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}
