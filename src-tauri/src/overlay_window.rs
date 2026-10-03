//! Places the little wave overlay where the user wants it (a preset spot or wherever they dragged it).

use crate::controller::Shared;
use oratio_core::overlay::{resolve, Rect};
use tauri::{AppHandle, Manager, Monitor, PhysicalPosition};

fn rect_of(m: &Monitor) -> Rect {
    Rect { x: m.position().x, y: m.position().y, w: m.size().width as i32, h: m.size().height as i32 }
}

/// Moves the overlay window to the chosen position. Call before showing it.
pub fn position(app: &AppHandle, shared: &Shared) {
    let Some(window) = app.get_webview_window("overlay") else { return };
    let Ok(size) = window.outer_size() else { return };
    let monitors = window.available_monitors().unwrap_or_default();
    let screens: Vec<Rect> = monitors.iter().map(rect_of).collect();
    let primary = window.primary_monitor().ok().flatten().or_else(|| monitors.first().cloned());
    let Some(primary) = primary else { return };
    let chosen = shared.overlay_pos.lock().map(|p| *p).unwrap_or_default();
    let margin = (24.0 * primary.scale_factor()) as i32;
    let (x, y) = resolve(chosen, rect_of(&primary), &screens, (size.width as i32, size.height as i32), margin);
    let _ = window.set_position(PhysicalPosition::new(x, y));
}
