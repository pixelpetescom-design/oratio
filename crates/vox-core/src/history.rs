use crate::polish::polish;
use crate::CoreError;
use serde::Serialize;

pub type RecordingId = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Recording,
    Completed,
    /// Interrupted (crash) or failed; any text already transcribed is kept.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub id: RecordingId,
    pub started_at_ms: i64,
    pub status: Status,
    pub segments: Vec<String>,
    pub final_text: Option<String>,
    pub error: Option<String>,
}

impl Entry {
    /// Best available text: the finished transcript, or whatever was saved so far.
    pub fn text(&self) -> String {
        self.final_text.clone().unwrap_or_else(|| polish(&self.segments))
    }
}

/// Port: durable local storage. Every call must be safe to make from the engine
/// thread and must not lose previously written data on failure.
pub trait History: Send + Sync {
    fn begin(&self, started_at_ms: i64) -> Result<RecordingId, CoreError>;
    fn append_segment(&self, id: RecordingId, text: &str) -> Result<(), CoreError>;
    fn complete(&self, id: RecordingId, final_text: &str) -> Result<(), CoreError>;
    fn fail(&self, id: RecordingId, reason: &str) -> Result<(), CoreError>;
    fn delete(&self, id: RecordingId) -> Result<(), CoreError>;
    fn list(&self, limit: u32) -> Result<Vec<Entry>, CoreError>;
    /// At startup: recordings left `Recording` by a crash become `Failed`, keeping their text.
    fn recover_interrupted(&self) -> Result<usize, CoreError>;
}
