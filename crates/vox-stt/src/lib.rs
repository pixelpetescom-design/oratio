//! Whisper (whisper.cpp) adapter for the `Transcriber` port, English only.
//! A different engine (e.g. Parakeet) is a second adapter behind the same trait.

use std::path::Path;
use vox_core::stt::Transcriber;
use vox_core::CoreError;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

/// Biases the decoder toward cased, punctuated prose.
const PROMPT: &str = "Hello. This is a clear, well-punctuated sentence, with proper capitalization.";

pub struct WhisperTranscriber {
    // Field order matters: the state must drop before the context it was created from.
    state: WhisperState,
    _ctx: WhisperContext,
    threads: i32,
}

fn stt_err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Stt(e.to_string())
}

impl WhisperTranscriber {
    pub fn load(model: &Path) -> Result<Self, CoreError> {
        let path = model.to_str().ok_or_else(|| CoreError::Stt("model path is not valid UTF-8".into()))?;
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|e| CoreError::Stt(format!("cannot load model {}: {e}", model.display())))?;
        let state = ctx.create_state().map_err(stt_err)?;
        let threads = std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(4).clamp(1, 8);
        let mut me = Self { state, _ctx: ctx, threads };
        // First inference allocates buffers; pay that cost at startup, not on the first hotkey press.
        me.transcribe(&vec![0.0; 8_000])?;
        Ok(me)
    }
}

impl Transcriber for WhisperTranscriber {
    fn transcribe(&mut self, audio: &[f32]) -> Result<String, CoreError> {
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        p.set_language(Some("en"));
        p.set_translate(false);
        p.set_n_threads(self.threads);
        p.set_no_context(true);
        p.set_suppress_blank(true);
        p.set_initial_prompt(PROMPT);
        p.set_print_special(false);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_timestamps(false);
        self.state.full(p, audio).map_err(stt_err)?;

        let mut text = String::new();
        for i in 0..self.state.full_n_segments().map_err(stt_err)? {
            let seg = self.state.full_get_segment_text(i).map_err(stt_err)?;
            let seg = seg.trim();
            // Whisper marks non-speech as [BLANK_AUDIO], (music), etc.
            let is_tag = (seg.starts_with('[') && seg.ends_with(']')) || (seg.starts_with('(') && seg.ends_with(')'));
            if !seg.is_empty() && !is_tag {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(seg);
            }
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a real model: `VOX_MODEL=models/ggml-base.en-q5_1.bin cargo test -p vox-stt -- --ignored`
    #[test]
    #[ignore = "requires a Whisper model file"]
    fn transcribes_sample() {
        let model = std::env::var("VOX_MODEL").expect("set VOX_MODEL");
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/jfk.wav")).expect("sample");
        let audio: Vec<f32> = bytes[44..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
        let mut t = WhisperTranscriber::load(Path::new(&model)).expect("load");
        let text = t.transcribe(&audio).expect("transcribe").to_lowercase();
        assert!(text.contains("ask not what your country can do for you"), "got: {text}");
    }
}
