use crate::CoreError;

/// Port: turns one utterance of 16 kHz mono f32 audio into text.
/// Implementations may be slow to construct but must be quick per call.
pub trait Transcriber: Send {
    /// The user's personal vocabulary, newest first. Engines that can use it as a hint should;
    /// the default ignores it.
    fn set_hints(&mut self, _words: &[String]) {}

    fn transcribe(&mut self, audio: &[f32]) -> Result<String, CoreError>;
}
