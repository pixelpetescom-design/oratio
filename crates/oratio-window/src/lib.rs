//! Which app has focus right now (for per-app rules). Best-effort: if the OS won't say, the
//! answer is `None` and Oratio behaves as if there were no rules.

use oratio_core::apps::ActiveApp;

pub fn active_app() -> Option<ActiveApp> {
    let w = active_win_pos_rs::get_active_window().ok()?;
    // "Discord", plus the executable's file name ("Discord.exe") so rules can use either.
    let exe = w.process_path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    Some(ActiveApp { name: format!("{} {}", w.app_name, exe).trim().to_string(), title: w.title })
}
