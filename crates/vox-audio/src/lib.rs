//! Microphone adapter. The microphone is opened only while a `Capture` exists,
//! and delivered as 16 kHz mono f32 chunks.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use std::sync::mpsc::{channel, Sender};
use std::thread::JoinHandle;
use vox_core::resample::Resampler;
use vox_core::CoreError;

pub struct Capture {
    stop: Option<Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

fn audio_err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Audio(e.to_string())
}

impl Capture {
    /// Opens the default input device. `sink` is called on the audio thread, so it must not block.
    pub fn start(sink: impl FnMut(Vec<f32>) + Send + 'static) -> Result<Capture, CoreError> {
        let (stop_tx, stop_rx) = channel::<()>();
        let (ready_tx, ready_rx) = channel::<Result<(), CoreError>>();
        // cpal streams are not `Send`, so the stream lives and dies on its own thread.
        let handle = std::thread::Builder::new()
            .name("vox-capture".into())
            .spawn(move || match open(sink) {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(audio_err)?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Capture { stop: Some(stop_tx), handle: Some(handle) }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(CoreError::Audio("capture thread died".into())),
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn open(sink: impl FnMut(Vec<f32>) + Send + 'static) -> Result<cpal::Stream, CoreError> {
    let device = cpal::default_host().default_input_device().ok_or_else(|| CoreError::Audio("no microphone found".into()))?;
    let supported = device.default_input_config().map_err(audio_err)?;
    eprintln!(
        "[vox] microphone: {} ({} Hz, {} ch, {:?})",
        device.name().unwrap_or_else(|_| "unknown".into()),
        supported.sample_rate().0,
        supported.channels(),
        supported.sample_format()
    );
    let config: cpal::StreamConfig = supported.clone().into();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, sink)?,
        SampleFormat::I16 => build::<i16>(&device, &config, sink)?,
        SampleFormat::U16 => build::<u16>(&device, &config, sink)?,
        f => return Err(CoreError::Audio(format!("unsupported sample format {f:?}"))),
    };
    stream.play().map_err(audio_err)?;
    Ok(stream)
}

fn build<T>(device: &cpal::Device, config: &cpal::StreamConfig, mut sink: impl FnMut(Vec<f32>) + Send + 'static) -> Result<cpal::Stream, CoreError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels).max(1);
    let mut resampler = Resampler::new(config.sample_rate.0);
    // Averaging channels can cancel the voice when a mic array sends it phase-inverted
    // (or silence on one side), so follow whichever channel is carrying the most signal.
    let mut energy = vec![0.0f32; channels];
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let mut out = Vec::with_capacity(data.len() / channels);
                for frame in data.chunks(channels) {
                    for (e, s) in energy.iter_mut().zip(frame) {
                        let v = f32::from_sample(*s);
                        *e = *e * 0.9995 + v * v * 0.0005;
                    }
                    let best = energy.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(i, _)| i);
                    let mono = frame.get(best).map_or(0.0, |s| f32::from_sample(*s));
                    resampler.push(mono, &mut out);
                }
                if !out.is_empty() {
                    sink(out);
                }
            },
            |e| eprintln!("audio stream error: {e}"),
            None,
        )
        .map_err(audio_err)
}
