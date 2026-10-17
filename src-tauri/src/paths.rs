use crate::config::MODEL_FILE;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use oratio_core::stt::Transcriber;
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

/// A Whisper model is one file; a Parakeet model is a folder.
pub fn find_model(paths: &[PathBuf]) -> Result<PathBuf, CoreError> {
    paths.iter().find(|p| p.is_file() || p.is_dir()).cloned().ok_or_else(|| {
        let searched = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n  ");
        CoreError::Stt(format!("Speech model {MODEL_FILE} not found. Run scripts/get-model, or place it in one of:\n  {searched}"))
    })
}

pub fn model_candidates(app: &AppHandle) -> Vec<PathBuf> {
    candidates(app)
}

/// Loads whichever kind of speech model is at `path`.
pub fn load_transcriber(path: &Path) -> Result<Box<dyn Transcriber>, CoreError> {
    if path.is_dir() {
        #[cfg(feature = "parakeet")]
        return Ok(Box::new(oratio_parakeet::ParakeetTranscriber::load(path)?));
        #[cfg(not(feature = "parakeet"))]
        return Err(CoreError::Stt("This build of Oratio does not include Parakeet.".into()));
    }
    Ok(Box::new(oratio_stt::WhisperTranscriber::load(path)?))
}
