//! Whisper (whisper.cpp) adapter for the `Transcriber` port, English only.
//! A different engine (e.g. Parakeet) is a second adapter behind the same trait.

use std::path::Path;
use oratio_core::stt::Transcriber;
use oratio_core::vocab::glossary_prompt;
use oratio_core::CoreError;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

/// Biases the decoder toward cased, punctuated prose with Australian spellings.
const PROMPT: &str = "Hello. This is a clear, well-punctuated sentence in Australian English, with proper capitalisation: colour, organise, centre.";

pub struct WhisperTranscriber {
    // Field order matters: the state must drop before the context it was created from.
    state: WhisperState,
    _ctx: WhisperContext,
    threads: i32,
    /// Style prompt plus the user's vocabulary.
    prompt: String,
}

/// Whisper's encoder always processes a fixed 30 s window unless told otherwise, so a 3 s
/// phrase costs as much as a 30 s one. 50 encoder frames = 1 s; give the clip its length plus
/// a 1 s margin, rounded up to a multiple of 64, never below ~10 s of context or above the 30 s maximum.
fn audio_ctx_for(samples: usize) -> i32 {
    let frames = (samples as f32 / 16_000.0 + 1.0) * 50.0;
    (((frames / 64.0).ceil() as i32) * 64).clamp(512, 1500)
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
        let mut me = Self { state, _ctx: ctx, threads, prompt: PROMPT.to_string() };
        // First inference allocates buffers; pay that cost at startup, not on the first hotkey press.
        me.transcribe(&vec![0.0; 8_000])?;
        Ok(me)
    }
}

impl Transcriber for WhisperTranscriber {
    fn set_hints(&mut self, words: &[String]) {
        let glossary = glossary_prompt(words);
        self.prompt = if glossary.is_empty() { PROMPT.to_string() } else { format!("{PROMPT} {glossary}") };
    }

    fn transcribe(&mut self, audio: &[f32]) -> Result<String, CoreError> {
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        p.set_language(Some("en"));
        p.set_translate(false);
        p.set_n_threads(self.threads);
        p.set_no_context(true);
        // Speed: skip timestamp decoding, treat each utterance as one segment, never re-decode
        // at higher temperatures, and only encode as much audio as there is.
        p.set_no_timestamps(true);
        p.set_single_segment(true);
        p.set_temperature_inc(0.0);
        p.set_audio_ctx(audio_ctx_for(audio.len()));
        p.set_suppress_blank(true);
        // The segmenter already gated out silence; don't let the model second-guess quiet speech.
        p.set_no_speech_thold(0.9);
        p.set_initial_prompt(&self.prompt);
        p.set_print_special(false);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_timestamps(false);
        self.state.full(p, audio).map_err(stt_err)?;

        let mut text = String::new();
        for segment in self.state.as_iter() {
            let seg = segment.to_str_lossy().map_err(stt_err)?;
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

    #[test]
    fn audio_context_scales_with_clip_length_and_stays_in_bounds() {
        assert_eq!(audio_ctx_for(16_000 * 3), 512); // short clips get the floor
        assert_eq!(audio_ctx_for(16_000 * 10), 576);
        assert_eq!(audio_ctx_for(16_000 * 25), 1344);
        assert_eq!(audio_ctx_for(16_000 * 40), 1500); // never beyond the model's window
        assert!(audio_ctx_for(0) >= 512);
        // The context must always cover the audio, or the end of the clip would be ignored.
        for secs in 1..=29 {
            assert!(audio_ctx_for(16_000 * secs) >= secs as i32 * 50, "{secs}s");
        }
    }

    /// Needs a real model: `ORATIO_MODEL=models/ggml-base.en-q5_1.bin cargo test -p oratio-stt -- --ignored`
    #[test]
    #[ignore = "requires a Whisper model file"]
    fn transcribes_sample() {
        let model = std::env::var("ORATIO_MODEL").expect("set ORATIO_MODEL");
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/jfk.wav")).expect("sample");
        let audio: Vec<f32> = bytes[44..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
        let mut t = WhisperTranscriber::load(Path::new(&model)).expect("load");
        let text = t.transcribe(&audio).expect("transcribe").to_lowercase();
        assert!(text.contains("ask not what your country can do for you"), "got: {text}");
    }
}
