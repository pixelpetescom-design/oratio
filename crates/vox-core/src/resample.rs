use crate::segmenter::SAMPLE_RATE;

/// Streaming box-filter resampler to 16 kHz mono. Cheap and alias-resistant
/// enough for speech.
pub struct Resampler {
    ratio: f32,
    pos: f32,
    acc: f32,
    n: u32,
}

impl Resampler {
    pub fn new(in_rate: u32) -> Self {
        Self { ratio: in_rate as f32 / SAMPLE_RATE as f32, pos: 0.0, acc: 0.0, n: 0 }
    }

    pub fn push(&mut self, sample: f32, out: &mut Vec<f32>) {
        self.acc += sample;
        self.n += 1;
        self.pos += 1.0;
        if self.pos >= self.ratio {
            out.push(self.acc / self.n as f32);
            self.acc = 0.0;
            self.n = 0;
            self.pos -= self.ratio;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rate: u32, samples: usize, value: f32) -> Vec<f32> {
        let mut r = Resampler::new(rate);
        let mut out = vec![];
        for _ in 0..samples {
            r.push(value, &mut out);
        }
        out
    }

    #[test]
    fn same_rate_is_identity() {
        assert_eq!(run(16_000, 100, 0.5).len(), 100);
    }

    #[test]
    fn downsamples_48k_by_three_and_preserves_level() {
        let out = run(48_000, 4800, 0.25);
        assert_eq!(out.len(), 1600);
        assert!(out.iter().all(|v| (v - 0.25).abs() < 1e-6));
    }

    #[test]
    fn handles_non_integer_ratio() {
        let out = run(44_100, 44_100, 1.0);
        assert!((out.len() as i64 - 16_000).abs() <= 1);
    }
}
