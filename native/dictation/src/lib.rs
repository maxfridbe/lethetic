//! Local Whisper dictation. No microphone audio is written to disk or sent over HTTP.
mod audio;
mod conditioning;
mod model;
use audio::{Capture, Control};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

type Loaded = Arc<Mutex<WhisperContext>>;
struct Job {
    id: String,
    control: Arc<Control>,
    capture: Mutex<Option<Capture>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.control.stop.store(true, Ordering::Release);
    }
}
struct Inner {
    foreground: AtomicBool,
    loaded: Mutex<Option<(String, Loaded)>>,
    job: Mutex<Option<Arc<Job>>>,
    gate: Arc<Semaphore>,
}
#[derive(Clone)]
pub struct Dictation(Arc<Inner>);
impl Default for Dictation {
    fn default() -> Self {
        Self(Arc::new(Inner {
            foreground: AtomicBool::new(true),
            loaded: Mutex::new(None),
            job: Mutex::new(None),
            gate: Arc::new(Semaphore::new(1)),
        }))
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub ready: bool,
    pub model: Option<String>,
    pub level: Option<f32>,
}
impl Dictation {
    /// Native activity lifecycle gate, independent of WebView timers and events.
    pub fn set_foreground(&self, active: bool) {
        self.0.foreground.store(active, Ordering::Release);
        if !active {
            let _ = self.cancel(None);
        }
    }
    pub fn status(&self) -> Result<Status, String> {
        let model = self
            .0
            .loaded
            .lock()
            .map_err(|_| "Whisper state unavailable")?
            .as_ref()
            .map(|(name, _)| name.clone());
        let level = self
            .0
            .job
            .lock()
            .map_err(|_| "Microphone state unavailable")?
            .as_ref()
            .and_then(|job| job.control.level.take());
        Ok(Status {
            ready: model.is_some(),
            model,
            level,
        })
    }
    pub async fn prepare(
        &self,
        directory: PathBuf,
        name: String,
        download: bool,
    ) -> Result<(), String> {
        let _permit = self
            .0
            .gate
            .clone()
            .try_acquire_owned()
            .map_err(|_| "Dictation is busy; finish or cancel before changing model")?;
        if self.status()?.model.as_deref() == Some(name.as_str()) {
            return Ok(());
        }
        let path = model::obtain(&directory, &name, download).await?;
        let context = tokio::task::spawn_blocking(move || load(&path))
            .await
            .map_err(|_| "Whisper model loader failed")??;
        *self
            .0
            .loaded
            .lock()
            .map_err(|_| "Whisper state unavailable")? =
            Some((name, Arc::new(Mutex::new(context))));
        Ok(())
    }
    pub async fn start(&self, id: String) -> Result<(), String> {
        if !self.0.foreground.load(Ordering::Acquire) {
            return Err("Return to Lethetic before recording".into());
        }
        if id.is_empty() || id.len() > 64 {
            return Err("Invalid dictation identifier".into());
        }
        let permit = self
            .0
            .gate
            .clone()
            .try_acquire_owned()
            .map_err(|_| "Dictation is busy; wait for the current operation")?;
        if !self.status()?.ready {
            return Err("Load a local Whisper model in Speech to text first".into());
        }
        let permit = Arc::new(permit);
        let job = Arc::new(Job {
            id,
            control: Arc::new(Control {
                _lease: Some(permit),
                ..Default::default()
            }),
            capture: Mutex::new(None),
        });
        *self
            .0
            .job
            .lock()
            .map_err(|_| "Microphone state unavailable")? = Some(job.clone());
        // Pause may have raced registration after the first foreground check.
        if !self.0.foreground.load(Ordering::Acquire) {
            self.cancel(Some(&job.id))?;
            return Err("Return to Lethetic before recording".into());
        }
        let state = job.control.clone();
        let captured = tokio::task::spawn_blocking(move || Capture::start(state))
            .await
            .map_err(|_| "Microphone worker failed".to_string())
            .and_then(|v| v);
        match captured {
            Ok(capture) if !job.control.cancelled.load(Ordering::Acquire) => {
                *job.capture
                    .lock()
                    .map_err(|_| "Microphone state unavailable")? = Some(capture);
                Ok(())
            }
            result => {
                self.cancel(Some(&job.id))?;
                Err(result.err().unwrap_or_else(|| "Dictation cancelled".into()))
            }
        }
    }
    pub fn cancel(&self, id: Option<&str>) -> Result<(), String> {
        let mut active = self
            .0
            .job
            .lock()
            .map_err(|_| "Microphone state unavailable")?;
        if active
            .as_ref()
            .is_some_and(|job| id.is_none_or(|id| job.id == id))
            && let Some(job) = active.take()
        {
            job.control.cancelled.store(true, Ordering::Release);
            job.control.stop.store(true, Ordering::Release);
        }
        Ok(())
    }
    pub async fn finish(&self, id: &str, language: Option<String>) -> Result<String, String> {
        let job = self
            .0
            .job
            .lock()
            .map_err(|_| "Microphone state unavailable")?
            .as_ref()
            .filter(|j| j.id == id)
            .cloned()
            .ok_or("Recording is no longer active")?;
        let capture = job
            .capture
            .lock()
            .map_err(|_| "Microphone state unavailable")?
            .take()
            .ok_or("Microphone is still starting")?;
        let context = self
            .0
            .loaded
            .lock()
            .map_err(|_| "Whisper state unavailable")?
            .as_ref()
            .map(|(_, ctx)| ctx.clone())
            .ok_or("Whisper model unavailable")?;
        let state = job.control.clone();
        let result = tokio::task::spawn_blocking(move || {
            let samples = capture.finish()?;
            transcribe(
                &*context.lock().map_err(|_| "Whisper state unavailable")?,
                &samples,
                language.as_deref(),
                state,
            )
        })
        .await
        .map_err(|_| "Whisper transcription worker failed".to_string())
        .and_then(|v| v);
        let cancelled = job.control.cancelled.load(Ordering::Acquire);
        self.cancel(Some(&job.id))?;
        if cancelled {
            Err("Dictation cancelled".into())
        } else {
            result
        }
    }
}
fn load(path: &Path) -> Result<WhisperContext, String> {
    // No backend enabled: suppress Whisper debug logs, which can contain speech text.
    whisper_rs::install_logging_hooks();
    let mut parameters = WhisperContextParameters::default();
    parameters.use_gpu(false);
    WhisperContext::new_with_params(
        path.to_str().ok_or("Model path is not valid UTF-8")?,
        parameters,
    )
    .map_err(|e| format!("Could not load local Whisper model: {e}"))
}
fn transcribe(
    context: &WhisperContext,
    samples: &[f32],
    language: Option<&str>,
    cancel: Arc<Control>,
) -> Result<String, String> {
    if cancel.cancelled.load(Ordering::Acquire) {
        return Err("Dictation cancelled".into());
    }
    if let Some(language) = language
        && (language.len() > 3
            || !language.bytes().all(|b| b.is_ascii_lowercase())
            || whisper_rs::get_lang_id(language).is_none())
    {
        return Err("Unsupported Whisper language code".into());
    }
    if samples.len() < audio::RATE / 4 {
        return Ok(String::new());
    }
    if samples.len() > audio::MAX_SAMPLES || samples.iter().any(|s| !s.is_finite()) {
        return Err("Invalid recording".into());
    }
    let rms =
        (samples.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / samples.len() as f64).sqrt();
    if rms < 0.001 {
        return Ok(String::new());
    }
    let mut state = context.create_state().map_err(|e| e.to_string())?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(std::thread::available_parallelism().map_or(4, |n| {
        n.get()
            .clamp(1, if cfg!(target_os = "android") { 4 } else { 8 })
    }) as i32);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_no_context(true);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    params.set_language(language);
    let abort = Box::new(Abort {
        control: cancel.clone(),
        deadline: Instant::now() + Duration::from_secs(180),
    });
    // SAFETY: abort has a stable heap address and remains alive through synchronous
    // state.full(), which joins its inference workers before returning. The callback
    // only reads thread-safe state and never unwinds. whisper-rs 0.16's convenience
    // callback setter erases the closure type incorrectly; use its explicit FFI API.
    unsafe {
        params.set_abort_callback(Some(abort_inference));
        params.set_abort_callback_user_data((&*abort as *const Abort).cast_mut().cast());
    }
    let inference = state.full(params, samples);
    drop(abort);
    inference.map_err(|e| {
        if cancel.cancelled.load(Ordering::Acquire) {
            "Dictation cancelled".into()
        } else {
            format!("Local transcription failed: {e}")
        }
    })?;
    let mut text = String::new();
    for segment in state.as_iter() {
        if segment.no_speech_probability() > 0.85 {
            continue;
        }
        let value = segment.to_str_lossy().map_err(|e| e.to_string())?;
        let clean = value.trim();
        if !clean.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.extend(clean.chars().filter(|c| !c.is_control() || *c == '\n'));
        }
        if text.len() > 64 * 1024 {
            return Err("Transcript exceeds 64 KiB; dictate a shorter passage".into());
        }
    }
    Ok(text)
}
struct Abort {
    control: Arc<Control>,
    deadline: Instant,
}
unsafe extern "C" fn abort_inference(data: *mut std::ffi::c_void) -> bool {
    // SAFETY: installed only by transcribe(), with a live immutable Abort allocation.
    let abort = unsafe { &*data.cast::<Abort>() };
    abort.control.cancelled.load(Ordering::Acquire) || Instant::now() >= abort.deadline
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_backgrounding_cancels_without_any_webview_callback() {
        let engine = Dictation::default();
        let control = Arc::new(Control::default());
        *engine.0.job.lock().unwrap() = Some(Arc::new(Job {
            id: "android-recording".into(),
            control: control.clone(),
            capture: Mutex::new(None),
        }));
        engine.set_foreground(false);
        assert!(control.stop.load(Ordering::Acquire));
        assert!(control.cancelled.load(Ordering::Acquire));
        assert!(
            engine
                .start("late-start".into())
                .await
                .unwrap_err()
                .contains("Return")
        );
        engine.set_foreground(true);
        assert!(engine.0.job.lock().unwrap().is_none());
        // Returning to the app never resumes a recording automatically.
        assert!(
            engine
                .start("fresh".into())
                .await
                .unwrap_err()
                .contains("Load")
        );
    }
    #[test]
    fn cancelled_capture_keeps_its_lease_until_the_audio_worker_exits() {
        let engine = Dictation::default();
        let control = Arc::new(Control {
            _lease: Some(Arc::new(engine.0.gate.clone().try_acquire_owned().unwrap())),
            ..Default::default()
        });
        *engine.0.job.lock().unwrap() = Some(Arc::new(Job {
            id: "current".into(),
            control: control.clone(),
            capture: Mutex::new(None),
        }));
        engine.cancel(Some("old")).unwrap();
        assert!(!control.cancelled.load(Ordering::Acquire));
        engine.cancel(Some("current")).unwrap();
        assert!(control.cancelled.load(Ordering::Acquire));
        assert!(control.stop.load(Ordering::Acquire));
        // The audio worker retains Control while closing or finishing startup.
        assert!(engine.0.gate.clone().try_acquire_owned().is_err());
        drop(control);
        assert!(engine.0.gate.clone().try_acquire_owned().is_ok());
    }
    #[tokio::test]
    async fn no_microphone_before_model_setup_and_unknown_jobs_are_safe() {
        let engine = Dictation::default();
        assert!(!engine.status().unwrap().ready);
        assert!(
            engine
                .start("test".into())
                .await
                .unwrap_err()
                .contains("Load")
        );
        assert!(
            engine
                .finish("unknown", None)
                .await
                .unwrap_err()
                .contains("no longer")
        );
        engine.cancel(Some("unknown")).unwrap();
        engine.cancel(None).unwrap();
    }
    #[test]
    #[ignore = "requires the explicitly downloaded official tiny model and a speech WAV fixture"]
    fn real_whisper_transcribes_a_speech_fixture_offline() {
        let model = std::env::var("LETHETIC_STT_TEST_MODEL").expect("model fixture");
        let wav = std::env::var("LETHETIC_STT_TEST_WAV").expect("audio fixture");
        let mut reader = hound::WavReader::open(wav).unwrap();
        assert_eq!(reader.spec().sample_rate, 16000);
        assert_eq!(reader.spec().channels, 1);
        let mut conditioner = conditioning::Conditioner::new(16000);
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| conditioner.process(f32::from(s.unwrap()) / 32768.0))
            .collect();
        let context = load(Path::new(&model)).unwrap();
        let text = transcribe(&context, &samples, Some("en"), Arc::default()).unwrap();
        assert!(text.to_lowercase().contains("country"), "{text}");
        let mid_flight = Arc::new(Control::default());
        let signal = mid_flight.clone();
        let cancellation = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            signal.cancelled.store(true, Ordering::Release);
        });
        assert!(
            transcribe(&context, &samples, Some("en"), mid_flight)
                .unwrap_err()
                .contains("cancelled")
        );
        cancellation.join().unwrap();
        let cancelled = Arc::new(Control::default());
        cancelled.cancelled.store(true, Ordering::Release);
        assert!(
            transcribe(&context, &samples, Some("en"), cancelled)
                .unwrap_err()
                .contains("cancelled")
        );
        assert_eq!(
            transcribe(&context, &[0.0; 16000], Some("en"), Arc::default()).unwrap(),
            ""
        );
    }
}
