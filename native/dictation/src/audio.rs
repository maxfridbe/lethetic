use crate::conditioning::{Conditioner, Meter};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
pub const RATE: usize = 16_000;
pub const MAX_SAMPLES: usize = RATE * 120;

/// Streaming mono conversion keeps capture bounded regardless of device sample rate.
struct Samples {
    values: Vec<f32>,
    step: f64,
    next: f64,
    index: u64,
    previous: f32,
}
impl Samples {
    fn new(rate: u32) -> Self {
        Self {
            values: Vec::with_capacity(RATE * 10),
            step: f64::from(rate) / RATE as f64,
            next: 0.0,
            index: 0,
            previous: 0.0,
        }
    }
    fn push(&mut self, sample: f32) {
        let sample = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        while self.next <= self.index as f64 && self.values.len() < MAX_SAMPLES {
            let mix = (self.next - (self.index as f64 - 1.0)).clamp(0.0, 1.0) as f32;
            self.values
                .push(self.previous + (sample - self.previous) * mix);
            self.next += self.step;
        }
        self.index += 1;
        self.previous = sample;
    }
}
#[derive(Default)]
pub(crate) struct Control {
    // Keep the operation lease until the audio thread has really released its device.
    pub _lease: Option<Arc<tokio::sync::OwnedSemaphorePermit>>,
    pub stop: AtomicBool,
    pub cancelled: AtomicBool,
    pub level: Meter,
}
pub(crate) struct Capture {
    control: Arc<Control>,
    worker: Option<thread::JoinHandle<Result<Vec<f32>, String>>>,
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.control.stop.store(true, Ordering::Release);
    }
}
impl Capture {
    pub fn start(control: Arc<Control>) -> Result<Self, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        let state = control.clone();
        let worker = thread::Builder::new()
            .name("lethetic-microphone".into())
            .spawn(move || {
                let run = || -> Result<Vec<f32>, String> {
                    let device = cpal::default_host()
                        .default_input_device()
                        .ok_or("No default microphone. Select one in system sound settings.")?;
                    let supported = device.default_input_config().map_err(|e| {
                        format!("Microphone unavailable: {e}. Check system microphone permission.")
                    })?;
                    let format = supported.sample_format();
                    let config: cpal::StreamConfig = supported.into();
                    if config.channels == 0
                        || config.channels > 32
                        || config.sample_rate < 8000
                        || config.sample_rate > 192000
                    {
                        return Err("Unsupported microphone channel count or sample rate".into());
                    }
                    let data = Arc::new(Mutex::new(Samples::new(config.sample_rate)));
                    let error = Arc::new(Mutex::new(None));
                    let stream = match format {
                        cpal::SampleFormat::F32 => input::<f32>(
                            &device,
                            config,
                            data.clone(),
                            error.clone(),
                            state.clone(),
                        ),
                        cpal::SampleFormat::I16 => input::<i16>(
                            &device,
                            config,
                            data.clone(),
                            error.clone(),
                            state.clone(),
                        ),
                        cpal::SampleFormat::I32 => input::<i32>(
                            &device,
                            config,
                            data.clone(),
                            error.clone(),
                            state.clone(),
                        ),
                        cpal::SampleFormat::U16 => input::<u16>(
                            &device,
                            config,
                            data.clone(),
                            error.clone(),
                            state.clone(),
                        ),
                        _ => return Err(format!("Unsupported microphone format: {format:?}")),
                    }?;
                    if state.cancelled.load(Ordering::Acquire) || state.stop.load(Ordering::Acquire)
                    {
                        return Err("Dictation cancelled".into());
                    }
                    stream
                        .play()
                        .map_err(|e| format!("Cannot start microphone: {e}"))?;
                    let _ = tx.send(Ok(()));
                    let deadline = Instant::now() + Duration::from_secs(120);
                    while !state.stop.load(Ordering::Acquire) && Instant::now() < deadline {
                        thread::sleep(Duration::from_millis(20));
                    }
                    drop(stream);
                    if let Some(error) = error
                        .lock()
                        .map_err(|_| "Microphone state unavailable")?
                        .take()
                    {
                        return Err(error);
                    }
                    let values = std::mem::take(
                        &mut data
                            .lock()
                            .map_err(|_| "Microphone data unavailable")?
                            .values,
                    );
                    Ok(values)
                };
                let result = run();
                if let Err(error) = &result {
                    let _ = tx.try_send(Err(error.clone()));
                }
                result
            })
            .map_err(|e| e.to_string())?;
        let capture = Self {
            control,
            worker: Some(worker),
        };
        rx.recv_timeout(Duration::from_secs(10))
            .map_err(|_| "Microphone startup timed out. Check system permission.".to_string())??;
        Ok(capture)
    }
    pub fn finish(mut self) -> Result<Vec<f32>, String> {
        self.control.stop.store(true, Ordering::Release);
        self.worker
            .take()
            .ok_or("Recording already consumed")?
            .join()
            .map_err(|_| "Microphone worker failed".to_string())?
    }
}
fn input<T: cpal::SizedSample + Send + 'static>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    data: Arc<Mutex<Samples>>,
    error: Arc<Mutex<Option<String>>>,
    control: Arc<Control>,
) -> Result<cpal::Stream, String>
where
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels);
    let mut conditioner = Conditioner::new(config.sample_rate);
    let on_error = control.clone();
    device
        .build_input_stream(
            config,
            move |input: &[T], _| {
                if control.stop.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(mut data) = data.lock() else {
                    return;
                };
                let mut energy = 0_f32;
                let mut peak = 0_f32;
                let mut count = 0_usize;
                for frame in input.chunks_exact(channels) {
                    let mono = frame
                        .iter()
                        .map(|sample| sample.to_sample::<f32>())
                        .sum::<f32>()
                        / channels as f32;
                    let mono = conditioner.process(mono);
                    data.push(mono);
                    energy += mono * mono;
                    peak = peak.max(mono.abs());
                    count += 1;
                }
                if count > 0 {
                    let envelope = 0.60 * (energy / count as f32).sqrt() + 0.40 * peak;
                    control
                        .level
                        .record(((envelope - 0.0038).max(0.0) * 8.0).clamp(0.0, 1.0));
                }
                if data.values.len() >= MAX_SAMPLES {
                    control.stop.store(true, Ordering::Release);
                }
            },
            move |e| {
                if let Ok(mut value) = error.lock() {
                    *value = Some(format!("Microphone stream failed: {e}"));
                }
                on_error.stop.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|e| format!("Cannot open microphone: {e}. Check system microphone permission."))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn converts_rate_across_callback_boundaries_and_bounds_recording() {
        let mut samples = Samples::new(48_000);
        for index in 0..48_000 {
            samples.push((index % 100) as f32 / 100.0);
        }
        assert_eq!(samples.values.len(), RATE);
        assert!((samples.values[1] - 0.03).abs() < 0.00001);
        for _ in 0..48_000 * 120 {
            samples.push(0.2);
        }
        assert_eq!(samples.values.len(), MAX_SAMPLES);
    }
    #[test]
    fn rejects_non_finite_and_clamps_samples() {
        let mut samples = Samples::new(16_000);
        for value in [f32::NAN, 2.0, -2.0] {
            samples.push(value);
        }
        assert_eq!(samples.values, [0.0, 1.0, -1.0]);
    }
}
