//! Which speech models are on this computer and which one is in use. The installer ships one model in
//! the resource folder; downloaded ones live in the app's data folder, and `active-model.txt` there
//! records the choice so it applies from the next launch too.

use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const ACTIVE_FILE: &str = "active-model.txt";
const SEEN_FILE: &str = "seen-models.txt";

pub struct Installed {
    pub file: String,
    pub size: u64,
    /// Shipped with the installer (can't be deleted).
    pub bundled: bool,
    pub path: PathBuf,
}

fn data_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok()
}

pub fn models_dir(app: &AppHandle) -> Option<PathBuf> {
    data_dir(app).map(|d| d.join("models"))
}

fn bundled_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().resource_dir().ok().map(|d| d.join("models"))
}

/// File names that are safe to treat as a model in our folders.
pub fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name.ends_with(".bin") && !name.contains(['/', '\\', ':']) && !name.contains("..")
}

fn scan(dir: &Path, bundled: bool, out: &mut Vec<Installed>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        let file = e.file_name().to_string_lossy().into_owned();
        if is_plain_name(&file) && path.is_file() && !out.iter().any(|i| i.file == file) {
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(Installed { file, size, bundled, path });
        }
    }
}

/// Models on this computer; a downloaded copy wins over a bundled one with the same name.
pub fn installed(app: &AppHandle) -> Vec<Installed> {
    let mut out = Vec::new();
    if let Some(d) = models_dir(app) {
        scan(&d, false, &mut out);
    }
    if let Some(d) = bundled_dir(app) {
        scan(&d, true, &mut out);
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    out
}

pub fn active(app: &AppHandle) -> Option<String> {
    let name = fs::read_to_string(data_dir(app)?.join(ACTIVE_FILE)).ok()?.trim().to_string();
    is_plain_name(&name).then_some(name)
}

pub fn set_active(app: &AppHandle, file: &str) -> std::io::Result<()> {
    let dir = data_dir(app).ok_or_else(|| std::io::Error::other("no data folder"))?;
    fs::create_dir_all(&dir)?;
    fs::write(dir.join(ACTIVE_FILE), file)
}

/// Model files the user has already been told about, so only genuinely new ones are flagged.
/// `None` means this is the first check (everything is the baseline, nothing is "new").
pub fn seen(app: &AppHandle) -> Option<Vec<String>> {
    let text = fs::read_to_string(data_dir(app)?.join(SEEN_FILE)).ok()?;
    Some(text.lines().map(str::to_string).collect())
}

pub fn remember_seen(app: &AppHandle, files: &[String]) {
    if let Some(dir) = data_dir(app) {
        let _ = fs::create_dir_all(&dir);
        let _ = fs::write(dir.join(SEEN_FILE), files.join("\n"));
    }
}

/// Where an installed model's file is.
pub fn path_of(app: &AppHandle, file: &str) -> Option<PathBuf> {
    installed(app).into_iter().find(|i| i.file == file).map(|i| i.path)
}
