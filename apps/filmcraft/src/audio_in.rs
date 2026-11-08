//! cpal audio input for voice-over recording ([`filmcraft_engine::voiceover::AudioInput`]).
//!
//! The stream lives on its own thread (cpal streams are not `Send` on every backend); the
//! callback de-interleaves into a shared planar buffer that the engine drains when recording stops.
//! Discovery is cached off the UI thread; one process-wide capture slot bounds blocked startup
//! and teardown, with cancellation gates preventing stale callback writes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
#[path = "input_capture.rs"]
mod capture_worker;
#[path = "input_discovery.rs"]
mod discovery;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use filmcraft_engine::voiceover::{AudioInput, InputFormat};

pub struct CpalIn {
    /// Host name (Settings ▸ Audio Hardware ▸ Device Class; empty = default host).
    host: String,
    buf: Arc<Mutex<Vec<Vec<f32>>>>,
    discovery: discovery::Discovery,
    capture: capture_worker::Capture<InputFormat>,
    error: Arc<Mutex<Option<String>>>,
}

impl CpalIn {
    pub fn new(host: &str) -> Self {
        Self {
            host: host.to_string(),
            buf: Arc::default(),
            discovery: discovery::Discovery::new(discover),
            capture: capture_worker::Capture::new(),
            error: Arc::default(),
        }
    }
}

fn discover(host_name: &str, name: &str) -> Result<discovery::Snapshot, String> {
    let h = host(host_name);
    let devices = h.input_devices().map_err(|e| e.to_string())?.take(128).filter_map(|d| d.name().ok()).collect();
    let channels = device(&h, name).map(|d| d.default_input_config().map(|c| c.channels()).map_err(|e| e.to_string())).transpose()?.unwrap_or(0);
    Ok(discovery::Snapshot { devices, channels })
}

fn host(name: &str) -> cpal::Host {
    if !name.is_empty()
        && let Some(id) = cpal::available_hosts().into_iter().find(|h| h.name() == name)
        && let Ok(h) = cpal::host_from_id(id)
    {
        return h;
    }
    cpal::default_host()
}

fn device(h: &cpal::Host, name: &str) -> Option<cpal::Device> {
    if !name.is_empty()
        && let Ok(devs) = h.input_devices()
        && let Some(d) = devs.take(128).find(|d| d.name().is_ok_and(|n| n == name))
    {
        return Some(d);
    }
    h.default_input_device()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn report_error(error: &Mutex<Option<String>>, mut message: String, active: &AtomicBool) {
    if message.len() > 4096 {
        let mut end = 4096;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    let mut error = lock(error);
    if !active.load(Ordering::Acquire) {
        return;
    }
    if error.is_none() {
        // The UI/control path reports this failure. Never do log I/O from a callback while capture locks are held.
        *error = Some(message);
    }
}

fn append<T: cpal::Sample>(data: &[T], channels: usize, buf: &Mutex<Vec<Vec<f32>>>, error: &Mutex<Option<String>>, active: &AtomicBool)
where
    f32: cpal::FromSample<T>,
{
    if !active.load(Ordering::Acquire) || lock(error).is_some() {
        return;
    }
    let mut b = lock(buf);
    if !active.load(Ordering::Acquire) {
        return;
    }
    if channels == 0 || channels > 256 || data.len() > 1_048_576 || !data.len().is_multiple_of(channels) {
        report_error(error, "invalid input device buffer".into(), active);
        return;
    }
    if b.len() != channels {
        *b = vec![Vec::new(); channels];
    }
    // The recorder drains at stop. Bound accumulated samples and report exhaustion instead of
    // allowing a long take or a broken/unpaced driver to exhaust the application heap.
    let samples = b.first().map_or(0, Vec::len).saturating_mul(channels).saturating_add(data.len());
    let frames = data.len() / channels;
    if samples > 268_435_456 || b.iter_mut().any(|c| c.try_reserve(frames).is_err()) {
        report_error(error, "voice-over input buffer is full; recording failed".into(), active);
        return;
    }
    for frame in data.chunks_exact(channels) {
        for (channel, sample) in b.iter_mut().zip(frame) {
            let sample = sample.to_sample::<f32>();
            channel.push(if sample.is_finite() { sample.clamp(-1.0, 1.0) } else { 0.0 });
        }
    }
}

fn input_stream<T: cpal::SizedSample>(
    dev: &cpal::Device,
    config: &cpal::StreamConfig,
    buf: Arc<Mutex<Vec<Vec<f32>>>>,
    error: Arc<Mutex<Option<String>>>,
    active: Arc<AtomicBool>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels);
    let stream_error = error.clone();
    let error_active = active.clone();
    dev.build_input_stream(
        config,
        move |data: &[T], _| {
            if active.load(Ordering::Acquire) {
                append(data, channels, &buf, &error, &active);
            }
        },
        move |e| {
            if error_active.load(Ordering::Acquire) {
                report_error(&stream_error, e.to_string(), &error_active);
            }
        },
        None,
    )
}

impl AudioInput for CpalIn {
    fn configure_host(&mut self, name: &str) {
        self.host = name.to_string();
    }
    fn devices(&self) -> Vec<String> {
        self.discovery.get(&self.host, None).devices
    }
    fn channels(&self, name: &str) -> u16 {
        self.discovery.get(&self.host, Some(name)).channels
    }
    fn discovery_error(&self) -> Option<String> {
        self.discovery.error()
    }
    fn start(&mut self, name: &str, sample_rate: u32) -> Result<InputFormat, String> {
        if name.len() > 1024 || self.host.len() > 1024 || sample_rate == 0 || sample_rate > 768_000 {
            return Err("Invalid input device name or sample rate".into());
        }
        let (host_name, name) = (self.host.clone(), name.to_string());
        let buf = self.buf.clone();
        let error = self.error.clone();
        self.capture.start(Duration::from_secs(2), move |active| {
            if !active.load(Ordering::Acquire) {
                return Err("Input capture startup was cancelled".into());
            }
            lock(&buf).clear();
            *lock(&error) = None;
            let h = host(&host_name);
            let dev = device(&h, &name).ok_or("no input device")?;
            let default = dev.default_input_config().map_err(|e| e.to_string())?;
            // Prefer f32 at the sequence rate, retaining integer-only input devices.
            let cfg = dev
                .supported_input_configs()
                .ok()
                .and_then(|it| {
                    it.take(1024)
                        .filter(|c| c.channels() > 0 && (c.min_sample_rate().0..=c.max_sample_rate().0).contains(&sample_rate))
                        .max_by_key(|c| (c.channels() == default.channels(), c.sample_format() == cpal::SampleFormat::F32))
                        .and_then(|c| c.try_with_sample_rate(cpal::SampleRate(sample_rate)))
                })
                .unwrap_or(default);
            if cfg.channels() == 0 || cfg.channels() > 256 || cfg.sample_rate().0 == 0 || cfg.sample_rate().0 > 768_000 {
                return Err("the input device returned an invalid format".into());
            }
            let fmt = InputFormat { sample_rate: cfg.sample_rate().0, channels: cfg.channels() };
            let format = cfg.sample_format();
            let config = cfg.into();
            let stream = match format {
                cpal::SampleFormat::I8 => input_stream::<i8>(&dev, &config, buf.clone(), error.clone(), active.clone()),
                cpal::SampleFormat::I16 => input_stream::<i16>(&dev, &config, buf.clone(), error.clone(), active.clone()),
                cpal::SampleFormat::I24 => input_stream::<cpal::I24>(&dev, &config, buf.clone(), error.clone(), active.clone()),
                cpal::SampleFormat::I32 => input_stream::<i32>(&dev, &config, buf.clone(), error.clone(), active.clone()),
                cpal::SampleFormat::I64 => input_stream::<i64>(&dev, &config, buf.clone(), error.clone(), active.clone()),
                cpal::SampleFormat::U8 => input_stream::<u8>(&dev, &config, buf.clone(), error.clone(), active.clone()),
                cpal::SampleFormat::U16 => input_stream::<u16>(&dev, &config, buf.clone(), error.clone(), active.clone()),
                cpal::SampleFormat::U32 => input_stream::<u32>(&dev, &config, buf.clone(), error.clone(), active.clone()),
                cpal::SampleFormat::U64 => input_stream::<u64>(&dev, &config, buf.clone(), error.clone(), active.clone()),
                cpal::SampleFormat::F32 => input_stream::<f32>(&dev, &config, buf.clone(), error.clone(), active.clone()),
                cpal::SampleFormat::F64 => input_stream::<f64>(&dev, &config, buf.clone(), error.clone(), active.clone()),
                other => return Err(format!("unsupported input sample format {other:?}")),
            }
            .map_err(|e| e.to_string())?;
            if !active.load(Ordering::Acquire) {
                return Err("Input capture startup was cancelled".into());
            }
            stream.play().map_err(|e| e.to_string())?;
            Ok((stream, fmt))
        })
    }
    fn read(&mut self, frames: usize) -> Vec<Vec<f32>> {
        let mut b = lock(&self.buf);
        b.iter_mut()
            .map(|c| {
                let n = frames.min(c.len());
                c.drain(..n).collect()
            })
            .collect()
    }
    fn discard(&mut self) {
        lock(&self.buf).iter_mut().for_each(Vec::clear);
    }
    fn error(&self) -> Option<String> {
        lock(&self.error).clone().or_else(|| self.capture.error())
    }
    fn stop(&mut self) {
        self.capture.stop();
    }
}

impl Drop for CpalIn {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_and_unsigned_input_is_deinterleaved_to_float() {
        let buf = Mutex::new(Vec::new());
        let error = Mutex::new(None);
        append(&[i16::MIN, i16::MAX, 0, 16384], 2, &buf, &error, &AtomicBool::new(true));
        let data = lock(&buf);
        assert_eq!(data[0], [-1.0, 0.0]);
        assert!((data[1][0] - 1.0).abs() < 0.0001);
        assert_eq!(data[1][1], 0.5);
        drop(data);
        lock(&buf).clear();
        append(&[0_u16, 32768, 65535, 32768], 2, &buf, &error, &AtomicBool::new(true));
        let data = lock(&buf);
        assert_eq!(data[1], [0.0, 0.0]);
        assert_eq!(data[0][0], -1.0);
        assert!(lock(&error).is_none());
    }

    #[test]
    fn invalid_input_is_reported_without_panicking_or_appending() {
        let buf = Mutex::new(Vec::new());
        let error = Mutex::new(None);
        append(&[0.0_f32], 0, &buf, &error, &AtomicBool::new(true));
        assert!(lock(&error).is_some());
        append(&[1.0_f32], 1, &buf, &error, &AtomicBool::new(true));
        assert!(lock(&buf).is_empty());
        *lock(&error) = None;
        append(&[0.0_f32; 3], 2, &buf, &error, &AtomicBool::new(true));
        assert!(lock(&error).is_some(), "a partial interleaved frame is not silently dropped");
        assert!(lock(&buf).is_empty());
    }
    #[test]
    fn hostile_input_requests_are_rejected_before_driver_access() {
        let mut input = CpalIn::new("");
        assert!(input.start("", 0).is_err());
        assert!(input.start("", u32::MAX).is_err());
        assert!(input.start(&"x".repeat(1025), 48000).is_err());
        assert_eq!(input.channels(&"x".repeat(1025)), 0);
        assert!(input.discovery.error().is_some());
    }
    #[test]
    fn cancelled_callback_cannot_append_or_replace_a_capture_error() {
        let buf = Mutex::new(vec![vec![0.25]]);
        let error = Mutex::new(None);
        let active = AtomicBool::new(false);
        append(&[1.0f32], 1, &buf, &error, &active);
        append(&[1.0f32], 0, &buf, &error, &active);
        assert_eq!(*lock(&buf), vec![vec![0.25]]);
        assert!(lock(&error).is_none());
    }
}
