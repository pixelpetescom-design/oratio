//! Speech recogniser adapter for NVIDIA Parakeet (TDT, English), run through ONNX Runtime on the CPU.
//! It implements the same `Transcriber` port as the Whisper adapter, so the rest of Oratio does not
//! know or care which one is in use.

use oratio_core::stt::Transcriber;
use oratio_core::CoreError;
use parakeet_rs::{ExecutionConfig, ParakeetTDT, Transcriber as _};
use std::path::Path;

/// Shorter than this (a tenth of a second at 16 kHz) is a click, not speech, and the model rejects it.
const MIN_SAMPLES: usize = 1_600;

pub struct ParakeetTranscriber {
    model: ParakeetTDT,
}

fn stt_err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Stt(e.to_string())
}

impl ParakeetTranscriber {
    /// Loads the model from a folder holding the encoder, decoder and `vocab.txt`.
    pub fn load(dir: &Path) -> Result<Self, CoreError> {
        let environment = parakeet_rs::ort::init().build().map_err(stt_err)?;
        let config = ExecutionConfig::new().with_environment(environment);
        let model = ParakeetTDT::from_pretrained(dir, Some(config)).map_err(stt_err)?;
        Ok(Self { model })
    }
}

impl Transcriber for ParakeetTranscriber {
    fn transcribe(&mut self, audio: &[f32]) -> Result<String, CoreError> {
        if audio.len() < MIN_SAMPLES {
            return Ok(String::new());
        }
        let result = self.model.transcribe_samples(audio.to_vec(), 16_000, 1, None).map_err(stt_err)?;
        Ok(result.text.trim().to_string())
    }
}
