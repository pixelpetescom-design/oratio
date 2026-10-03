use crate::config::MODEL_FILE;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};
use oratio_core::CoreError;

/// Everywhere the model may live, most specific first. The installer ships it in
/// the resource dir; power users can drop a different one in the data dir.
fn candidates(app: &AppHandle) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(p) = std::env::var("ORATIO_MODEL") {
        v.push(PathBuf::from(p));
    }
    // The model the user chose in Settings (downloaded or bundled), then the one this build ships with.
    if let Some(chosen) = crate::model_files::active(app) {
        if let Some(path) = crate::model_files::path_of(app, &chosen) {
            v.push(path);
        }
    }
    if let Ok(d) = app.path().app_data_dir() {
        v.push(d.join("models").join(MODEL_FILE));
    }
    if let Ok(d) = app.path().resource_dir() {
        v.push(d.join("models").join(MODEL_FILE));
    }
    if let Ok(d) = std::env::current_dir() {
        v.push(d.join("models").join(MODEL_FILE));
        v.push(d.join("..").join("models").join(MODEL_FILE));
    }
    v
}

pub fn find_model(paths: &[PathBuf]) -> Result<PathBuf, CoreError> {
    paths.iter().find(|p| p.is_file()).cloned().ok_or_else(|| {
        let searched = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n  ");
        CoreError::Stt(format!("Speech model {MODEL_FILE} not found. Run scripts/get-model, or place it in one of:\n  {searched}"))
    })
}

pub fn model_candidates(app: &AppHandle) -> Vec<PathBuf> {
    candidates(app)
}
