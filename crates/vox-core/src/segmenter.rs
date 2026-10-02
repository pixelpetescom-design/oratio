//! Energy-based utterance segmenter. Splits a live 16 kHz mono stream into
//! utterances at natural pauses so each can be transcribed while the user is
//! still talking; stopping then only has to transcribe the last piece.

use std::collections::VecDeque;

pub const SAMPLE_RATE: usize = 16_000;

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Boost quiet recordings (typical laptop mics peak far below full scale) so the
/// recogniser gets a healthy signal. Never amplifies near-silence.
pub fn normalize(audio: &mut [f32]) {
    let peak = audio.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.002 && peak < 0.5 {
        let gain = (0.8 / peak).min(20.0);
        for s in audio.iter_mut() {
            *s *= gain;
        }
    }
}

#[derive(Debug, Clone)]
pub struct SegmenterConfig {
    pub frame_samples: usize,
    /// Frames kept from before speech starts so first syllables aren't clipped.
    pub pre_roll_frames: usize,
    /// Silent frames that close an utterance.
    pub end_silence_frames: usize,
    /// Utterances with fewer speech frames are treated as noise while recording...
    pub min_speech_frames: usize,
    /// ...but the final flush is more lenient so a short "yes" isn't lost.
    pub min_flush_frames: usize,
    pub max_samples: usize,
    pub min_threshold: f32,
    pub noise_multiplier: f32,
}

impl Default for SegmenterConfig {
    fn default() -> Self {
        Self {
            frame_samples: SAMPLE_RATE * 30 / 1000,
            pre_roll_frames: 10,
            end_silence_frames: 18,
            min_speech_frames: 8,
            min_flush_frames: 3,
            max_samples: SAMPLE_RATE * 25,
            min_threshold: 0.0015,
            noise_multiplier: 3.0,
        }
    }
}

pub struct Segmenter {
    cfg: SegmenterConfig,
    pending: Vec<f32>,
    pre: VecDeque<Vec<f32>>,
    current: Vec<f32>,
    speaking: bool,
    speech_frames: usize,
    silence_frames: usize,
    noise_floor: f32,
}

impl Segmenter {
    pub fn new(cfg: SegmenterConfig) -> Self {
        Self {
            cfg,
            pending: Vec::new(),
            pre: VecDeque::new(),
            current: Vec::new(),
            speaking: false,
            speech_frames: 0,
            silence_frames: 0,
            noise_floor: 0.001,
        }
    }

    /// Feed audio; returns any utterances completed by it.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Vec<f32>> {
        self.pending.extend_from_slice(samples);
        let mut done = Vec::new();
        while self.pending.len() >= self.cfg.frame_samples {
            let frame: Vec<f32> = self.pending.drain(..self.cfg.frame_samples).collect();
            if let Some(seg) = self.frame(frame) {
                done.push(seg);
            }
        }
        done
    }

    pub fn noise_floor(&self) -> f32 {
        self.noise_floor
    }

    /// Close out whatever is in flight (the user pressed stop).
    pub fn flush(&mut self) -> Option<Vec<f32>> {
        if self.speaking {
            let rest = std::mem::take(&mut self.pending);
            self.current.extend(rest);
        }
        let keep = self.speaking && self.speech_frames >= self.cfg.min_flush_frames;
        let out = keep.then(|| std::mem::take(&mut self.current));
        self.reset_utterance();
        self.pending.clear();
        self.pre.clear();
        out
    }

    fn frame(&mut self, frame: Vec<f32>) -> Option<Vec<f32>> {
        let level = rms(&frame);
        let is_speech = level > (self.noise_floor * self.cfg.noise_multiplier).max(self.cfg.min_threshold);
        if !is_speech {
            self.noise_floor = self.noise_floor * 0.95 + level * 0.05;
        }

        if self.speaking {
            self.current.extend_from_slice(&frame);
            if is_speech {
                self.speech_frames += 1;
                self.silence_frames = 0;
            } else {
                self.silence_frames += 1;
            }
            if self.silence_frames >= self.cfg.end_silence_frames || self.current.len() >= self.cfg.max_samples {
                let keep = self.speech_frames >= self.cfg.min_speech_frames;
                let out = keep.then(|| std::mem::take(&mut self.current));
                self.reset_utterance();
                return out;
            }
        } else if is_speech {
            self.speaking = true;
            self.speech_frames = 1;
            for f in self.pre.drain(..) {
                self.current.extend(f);
            }
            self.current.extend_from_slice(&frame);
        } else {
            self.pre.push_back(frame);
            if self.pre.len() > self.cfg.pre_roll_frames {
                self.pre.pop_front();
            }
        }
        None
    }

    fn reset_utterance(&mut self) {
        self.current.clear();
        self.speaking = false;
        self.speech_frames = 0;
        self.silence_frames = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(ms: usize) -> Vec<f32> {
        (0..SAMPLE_RATE * ms / 1000).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin() * 0.3).collect()
    }
    fn silence(ms: usize) -> Vec<f32> {
        vec![0.0; SAMPLE_RATE * ms / 1000]
    }
    fn feed(seg: &mut Segmenter, parts: &[Vec<f32>]) -> Vec<Vec<f32>> {
        parts.iter().flat_map(|p| seg.push(p)).collect()
    }

    #[test]
    fn normalize_boosts_quiet_audio_but_not_silence_or_loud() {
        let mut quiet = vec![0.02, -0.04, 0.01];
        normalize(&mut quiet);
        assert!((quiet[1].abs() - 0.8).abs() < 1e-6);
        let mut floor = vec![0.0005, -0.001];
        normalize(&mut floor);
        assert_eq!(floor, vec![0.0005, -0.001]);
        let mut loud = vec![0.9, -0.7];
        normalize(&mut loud);
        assert_eq!(loud, vec![0.9, -0.7]);
    }

    #[test]
    fn quiet_speech_is_still_detected() {
        let quiet: Vec<f32> = tone(800).iter().map(|s| s * 0.02).collect(); // rms ~0.004, like a very quiet mic
        let mut s = Segmenter::new(SegmenterConfig::default());
        let out = feed(&mut s, &[silence(500), quiet, silence(900)]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn silence_produces_nothing() {
        let mut s = Segmenter::new(SegmenterConfig::default());
        assert!(feed(&mut s, &[silence(3000)]).is_empty());
        assert!(s.flush().is_none());
    }

    #[test]
    fn speech_then_pause_closes_one_utterance_with_pre_roll() {
        let mut s = Segmenter::new(SegmenterConfig::default());
        let out = feed(&mut s, &[silence(1000), tone(1000), silence(1000)]);
        assert_eq!(out.len(), 1);
        let secs = out[0].len() as f32 / SAMPLE_RATE as f32;
        assert!(secs > 1.0 && secs < 2.0, "got {secs}");
    }

    #[test]
    fn two_phrases_make_two_utterances() {
        let mut s = Segmenter::new(SegmenterConfig::default());
        let out = feed(&mut s, &[tone(800), silence(900), tone(800), silence(900)]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn short_blip_is_ignored_while_recording() {
        let mut s = Segmenter::new(SegmenterConfig::default());
        assert!(feed(&mut s, &[silence(500), tone(90), silence(1000)]).is_empty());
    }

    #[test]
    fn flush_returns_speech_in_progress_even_if_short() {
        let mut s = Segmenter::new(SegmenterConfig::default());
        assert!(feed(&mut s, &[tone(200)]).is_empty());
        assert!(s.flush().is_some());
        assert!(s.flush().is_none(), "flush resets state");
    }

    #[test]
    fn very_long_speech_is_force_split() {
        let mut s = Segmenter::new(SegmenterConfig::default());
        let out = feed(&mut s, &[tone(26_000)]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn chunk_size_does_not_matter() {
        let audio: Vec<f32> = [silence(300), tone(700), silence(900)].concat();
        let mut a = Segmenter::new(SegmenterConfig::default());
        let whole = a.push(&audio);
        let mut b = Segmenter::new(SegmenterConfig::default());
        let pieces: Vec<_> = audio.chunks(137).flat_map(|c| b.push(c)).collect();
        assert_eq!(whole, pieces);
    }
}
