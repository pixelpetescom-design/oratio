use crate::CoreError;

/// Port: turns one utterance of 16 kHz mono f32 audio into text.
/// Implementations may be slow to construct but must be quick per call.
pub trait Transcriber: Send {
    fn transcribe(&mut self, audio: &[f32]) -> Result<String, CoreError>;
}
