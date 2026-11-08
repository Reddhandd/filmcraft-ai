//! CPAL devices stay on the voice worker. The existing stream resampler adapts device rates.
use super::pcm;
use cpal::traits::{DeviceTrait, HostTrait};
use filmcraft_audio_dsp::resample::StreamResampler;
use filmcraft_engine::voiceover::AudioInput;
use filmcraft_ui_egui::AudioOut;
use filmcraft_ui_egui::console::VoiceInputStats;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};
pub const RATE: u32 = 8000;
pub const FRAME: usize = 160;
pub const MAX_SPEAKER: usize = RATE as usize * 30;
#[derive(Default)]
pub struct Shared {
    pub microphone_gate: Arc<super::MicrophoneGate>,
    pub speaker: Mutex<VecDeque<f32>>,
    pub last_speech: Mutex<Option<Instant>>,
    pub error: Mutex<Option<String>>,
}
impl Shared {
    pub fn fail(&self, error: &str) {
        *self.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(error.into());
    }
    pub fn push(&self, payload: &[u8]) -> Result<(), String> {
        if payload.len() > 8192 {
            return Err("Voice RTP payload is too large".into());
        }
        let mut queue = self.speaker.lock().unwrap_or_else(PoisonError::into_inner);
        if queue.len().saturating_add(payload.len()) > MAX_SPEAKER {
            return Err("Voice playback queue is full; stop and retry".into());
        }
        queue.extend(payload.iter().map(|sample| pcm::decode(*sample)));
        Ok(())
    }
}
pub struct Devices {
    input: crate::audio_in::CpalIn,
    output: crate::audio::CpalOut,
    input_resampler: StreamResampler,
    next_input: i64,
    input_gate_seen: u64,
    shared: Arc<Shared>,
    channel: u16,
    stats: VoiceInputStats,
}
impl Devices {
    pub fn start(shared: Arc<Shared>, channel: u16) -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        {
            use objc2_foundation::{NSBundle, NSString};
            // macOS can abort an unbundled executable on privacy-sensitive API access.
            // Reject that launch before CPAL touches the microphone instead of relying on a panic guard.
            if NSBundle::mainBundle().objectForInfoDictionaryKey(&NSString::from_str("NSMicrophoneUsageDescription")).is_none() {
                return Err(
                    "Microphone access requires a FilmCraft .app bundle with its microphone privacy description. Launch the separate desktop bundle.".into()
                );
            }
        }
        let host = cpal::default_host();
        let device = host.default_input_device().ok_or("No system microphone is available")?;
        let device_name = device.name().unwrap_or_else(|_| "System microphone".into());
        if host.default_output_device().is_none() {
            return Err("No system speaker or headphones are available".into());
        }
        let mut input = crate::audio_in::CpalIn::new("");
        let format = input.start("", RATE)?;
        if format.sample_rate == 0 || format.sample_rate > 384000 || format.channels == 0 || format.channels > 256 {
            return Err("Invalid microphone format".into());
        }
        if channel >= format.channels {
            return Err(format!("Microphone input {} is unavailable; this device has {} channels", channel.saturating_add(1), format.channels));
        }
        let mut output = crate::audio::CpalOut::new();
        let output_shared = Arc::clone(&shared);
        let output_rate = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let callback_rate = Arc::clone(&output_rate);
        let mut output_resampler = None;
        let mut next_output = 0i64;
        let rate = output.start(Box::new(move |data, channels| {
            data.fill(0.0);
            let rate = callback_rate.load(std::sync::atomic::Ordering::Relaxed);
            if rate == 0 {
                return;
            }
            if channels == 0 || channels > 256 || data.len() > 1_048_576 {
                output_shared.fail("Invalid voice output buffer");
                return;
            }
            let frames = data.len() / channels;
            if frames > 32768 {
                output_shared.fail("Voice output buffer is too large");
                return;
            }
            let Some(mut queue) = output_shared.speaker.try_lock().ok() else { return };
            let resampler = output_resampler.get_or_insert_with(|| StreamResampler::new(RATE, rate));
            let mut spoke = false;
            let mono = resampler.process(next_output, frames, |start, want| {
                let mut samples = Vec::with_capacity(want);
                for i in 0..want {
                    let sample = if start.saturating_add(i as i64) < 0 { 0.0 } else { queue.pop_front().unwrap_or(0.0) };
                    spoke |= audible(sample);
                    samples.push(sample);
                }
                vec![samples]
            });
            next_output = next_output.saturating_add(frames as i64);
            if let Some(mono) = mono.first() {
                for (frame, sample) in data.chunks_exact_mut(channels).zip(mono) {
                    frame.fill(*sample);
                }
            }
            if spoke && let Ok(mut last) = output_shared.last_speech.try_lock() {
                *last = Some(Instant::now());
            }
        }))?;
        if rate == 0 || rate > 384000 {
            return Err("Invalid speaker sample rate".into());
        }
        output_rate.store(rate, std::sync::atomic::Ordering::Relaxed);
        Ok(Self {
            input,
            output,
            input_resampler: StreamResampler::new(format.sample_rate, RATE),
            next_input: 0,
            input_gate_seen: shared.microphone_gate.snapshot(),
            shared,
            channel,
            stats: VoiceInputStats { device: device_name, channel: channel.saturating_add(1), channels: format.channels, ..VoiceInputStats::default() },
        })
    }
    pub fn take_stats(&mut self) -> VoiceInputStats {
        let stats = self.stats.clone();
        self.stats.input_peak = 0.0;
        self.stats.transmit_peak = 0.0;
        stats
    }
    pub fn frame(&mut self) -> Result<Vec<u8>, String> {
        if let Some(error) = self.input.error() {
            return Err(error);
        }
        self.output.recover();
        if let Some(error) = self.shared.error.lock().unwrap_or_else(PoisonError::into_inner).take() {
            return Err(error);
        }
        if prepare_microphone_frame(&mut self.input, &mut self.input_resampler, &mut self.next_input, &mut self.input_gate_seen, &self.shared.microphone_gate) {
            self.stats.input_peak = 0.0;
            self.stats.transmit_peak = 0.0;
            self.stats.gated = true;
            return Ok(vec![255; FRAME]);
        }
        let input = &mut self.input;
        let channel = usize::from(self.channel);
        let mono = self.input_resampler.process(self.next_input, FRAME, |start, want| {
            let pad = usize::try_from(start.saturating_neg()).unwrap_or(0).min(want);
            let channels = input.read(want.saturating_sub(pad));
            let mut samples = vec![0.0; want];
            for (index, sample) in samples.iter_mut().enumerate().skip(pad) {
                *sample = selected_sample(&channels, channel, index - pad);
            }
            vec![samples]
        });
        self.next_input = self.next_input.saturating_add(FRAME as i64);
        // Speaker mode suppresses echo; headphones mode keeps near-end speech live.
        let speaking = self.shared.last_speech.lock().unwrap_or_else(PoisonError::into_inner).is_some_and(|last| last.elapsed() < Duration::from_millis(300));
        Ok(super::input_frame::encode(&mono, speaking, &self.shared.microphone_gate, &mut self.stats))
    }
}

fn prepare_microphone_frame(
    input: &mut impl AudioInput,
    resampler: &mut StreamResampler,
    next: &mut i64,
    seen: &mut u64,
    gate: &super::MicrophoneGate,
) -> bool {
    let snapshot = gate.snapshot();
    if snapshot != *seen {
        // Drop side speech and interpolation history even if mute and unmute both happened
        // while the worker was delayed. Speaker state is independent of this input reset.
        input.discard();
        *resampler = StreamResampler::new(resampler.rates().0, RATE);
        *next = 0;
        *seen = snapshot;
    }
    let muted = super::MicrophoneGate::muted(snapshot);
    if muted {
        input.discard();
    }
    muted
}

fn selected_sample(channels: &[Vec<f32>], channel: usize, frame: usize) -> f32 {
    channels.get(channel).and_then(|samples| samples.get(frame)).copied().filter(|sample| sample.is_finite()).unwrap_or(0.0).clamp(-1.0, 1.0)
}
fn audible(sample: f32) -> bool {
    sample.is_finite() && sample.abs() > 0.005
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unmute_discards_device_backlog_and_resampler_history_without_muting_speaker() {
        struct Input {
            queued: Vec<f32>,
            discarded: usize,
        }
        impl AudioInput for Input {
            fn devices(&self) -> Vec<String> {
                Vec::new()
            }
            fn channels(&self, _: &str) -> u16 {
                1
            }
            fn start(&mut self, _: &str, _: u32) -> Result<filmcraft_engine::voiceover::InputFormat, String> {
                Ok(filmcraft_engine::voiceover::InputFormat { sample_rate: RATE, channels: 1 })
            }
            fn read(&mut self, frames: usize) -> Vec<Vec<f32>> {
                let n = frames.min(self.queued.len());
                vec![self.queued.drain(..n).collect()]
            }
            fn discard(&mut self) {
                self.queued.clear();
                self.discarded += 1;
            }
            fn stop(&mut self) {}
        }
        let shared = Shared::default();
        shared.push(&[pcm::encode(0.5); 160]).unwrap();
        let speaker = shared.speaker.lock().unwrap().clone();
        let mut input = Input { queued: vec![0.9; 8000], discarded: 0 };
        let mut resampler = StreamResampler::new(RATE, RATE);
        let _ = resampler.process(0, FRAME, |_, n| vec![vec![0.8; n]]);
        let mut next = FRAME as i64;
        let mut seen = shared.microphone_gate.snapshot();
        shared.microphone_gate.set(true).unwrap();
        shared.microphone_gate.set(false).unwrap();
        assert!(!prepare_microphone_frame(&mut input, &mut resampler, &mut next, &mut seen, &shared.microphone_gate));
        assert_eq!(input.discarded, 1);
        assert!(input.queued.is_empty());
        assert_eq!(next, 0);
        let fresh = resampler.process(0, FRAME, |_, n| input.read(n));
        assert!(fresh.iter().flatten().all(|v| *v == 0.0));
        assert_eq!(*shared.speaker.lock().unwrap(), speaker);
    }

    #[test]
    fn selected_interface_channel_preserves_speech_without_cancellation() {
        let mut channels = vec![vec![0.0]; 12];
        channels[0][0] = 0.6;
        channels[1][0] = -0.6;
        assert_eq!(selected_sample(&channels, 0, 0), 0.6);
        assert_eq!(selected_sample(&channels, 1, 0), -0.6);
        assert_eq!(selected_sample(&channels, 12, 0), 0.0);
        assert_eq!(selected_sample(&channels, 0, 1), 0.0);
        channels[0][0] = f32::NAN;
        assert_eq!(selected_sample(&channels, 0, 0), 0.0);
    }
    #[test]
    fn comfort_noise_does_not_hold_microphone_gate() {
        assert!(!audible(pcm::decode(255)));
        assert!(!audible(0.001));
        assert!(!audible(f32::NAN));
        assert!(audible(-0.1));
    }
}
