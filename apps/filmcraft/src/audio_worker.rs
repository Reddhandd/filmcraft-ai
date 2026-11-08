//! Own the native audio backend on one bounded worker, never on the window thread.
use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError, TryLockError};
use std::time::{Duration, Instant};

use filmcraft_engine::settings::AudioHardwarePrefs;
use filmcraft_ui_egui::{AudioDevices, AudioOut};

type Fill = Box<dyn FnMut(&mut [f32], usize) + Send>;
const DEADLINE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Format {
    rate: u32,
    channels: usize,
}
impl Default for Format {
    fn default() -> Self {
        Self { rate: 48_000, channels: 2 }
    }
}
struct Metadata {
    format: Format,
    revision: u64,
    ready: bool,
    clock: Option<(u64, Arc<AtomicU64>)>,
    busy: Option<Instant>,
    note: Option<String>,
    devices: AudioDevices,
}
struct Shared {
    generation: AtomicU64,
    requested: AtomicU64,
    closed: AtomicBool,
    failed: AtomicBool,
    enumerate: AtomicBool,
    metadata: Mutex<Metadata>,
    ctx: egui::Context,
}
enum Request {
    Configure { revision: u64, hardware: AudioHardwarePrefs, document_rate: Option<u32>, language: filmcraft_ui_egui::i18n::Language },
    Start { generation: u64, revision: u64, format: Format, fill: Fill },
}

pub struct Output {
    shared: Arc<Shared>,
    send: SyncSender<Request>,
    pending: RefCell<Option<Request>>,
    observed: Cell<Format>,
    configuration_error: Option<String>,
}

fn text_limit(mut text: String, limit: usize) -> String {
    if let Some((boundary, _)) = text.char_indices().nth(limit) {
        text.truncate(boundary);
    }
    text
}
impl Shared {
    fn busy(&self) {
        self.metadata.lock().unwrap_or_else(PoisonError::into_inner).busy = Some(Instant::now());
        self.ctx.request_repaint_after(DEADLINE);
    }
    fn live(&self, generation: u64) -> bool {
        !self.closed.load(Ordering::Acquire) && !self.failed.load(Ordering::Acquire) && self.generation.load(Ordering::Acquire) == generation
    }
}

impl Output {
    /// The backend is constructed *inside* the worker, so its streams need not be Send.
    pub fn new<B: AudioOut + 'static>(ctx: egui::Context, factory: impl FnOnce() -> B + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            generation: AtomicU64::new(0),
            requested: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            enumerate: AtomicBool::new(false),
            ctx,
            metadata: Mutex::new(Metadata {
                format: Format::default(),
                revision: 0,
                ready: false,
                clock: None,
                busy: Some(Instant::now()),
                note: None,
                devices: AudioDevices::default(),
            }),
        });
        let (send, receive) = sync_channel(4);
        let worker = shared.clone();
        let failed = shared.clone();
        if let Err(error) = std::thread::Builder::new().name("filmcraft-audio-device".into()).spawn(move || {
            if catch_unwind(AssertUnwindSafe(|| run(&worker, receive, factory))).is_err() {
                log::error!("native audio worker panicked; audio unavailable");
                failed.failed.store(true, Ordering::Release);
                failed.ctx.request_repaint();
            }
        }) {
            log::warn!("cannot start native audio worker: {error}");
            shared.failed.store(true, Ordering::Release);
        }
        Self { shared, send, pending: RefCell::new(None), observed: Cell::new(Format::default()), configuration_error: None }
    }
    fn metadata(&self) -> Option<std::sync::MutexGuard<'_, Metadata>> {
        match self.shared.metadata.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::Poisoned(error)) => Some(error.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        }
    }
    fn flush(&self) {
        let Ok(mut pending) = self.pending.try_borrow_mut() else { return };
        if let Some(request) = pending.take() {
            match self.send.try_send(request) {
                Ok(()) => {}
                Err(TrySendError::Full(request)) => {
                    *pending = Some(request);
                    self.shared.ctx.request_repaint_after(POLL);
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.shared.failed.store(true, Ordering::Release);
                }
            }
        }
    }
    fn next_generation(&mut self) -> Option<u64> {
        // Only this window-owned Output writes the counters; the worker/callbacks only read.
        let next = self.shared.generation.load(Ordering::Acquire).checked_add(1);
        if let Some(next) = next {
            self.shared.generation.store(next, Ordering::Release);
        } else {
            self.shared.closed.store(true, Ordering::Release);
            self.shared.failed.store(true, Ordering::Release);
        }
        next
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
        // Never join a backend that may be blocked in an OS call.
    }
}
impl AudioOut for Output {
    fn poll(&mut self) {
        self.flush();
    }
    fn start(&mut self, fill: Fill) -> Result<u32, String> {
        self.flush();
        if let Some(error) = &self.configuration_error {
            return Err(error.clone());
        }
        if self.shared.failed.load(Ordering::Acquire) || self.shared.closed.load(Ordering::Acquire) {
            return Err("Audio worker unavailable; restart FilmCraft when convenient".into());
        }
        let generation = self.next_generation().ok_or("Audio request counter exhausted; restart FilmCraft")?;
        let mut metadata = self.metadata().ok_or("Audio state is updating; retry playback")?;
        let revision = self.shared.requested.load(Ordering::Acquire);
        if !metadata.ready || metadata.revision != revision || !self.pending.try_borrow().is_ok_and(|p| p.is_none()) {
            return Err("Audio hardware is initializing or changing; retry playback shortly".into());
        }
        let format = self.observed.get();
        if format != metadata.format {
            return Err("Audio format changed; retry playback".into());
        }
        self.send.try_send(Request::Start { generation, revision, format, fill }).map_err(|error| match error {
            TrySendError::Full(_) => "Audio device is busy; retry playback".to_string(),
            TrySendError::Disconnected(_) => "Audio worker unavailable; restart FilmCraft when convenient".to_string(),
        })?;
        metadata.clock = Some((generation, Arc::new(AtomicU64::new(0))));
        metadata.busy = Some(Instant::now());
        Ok(format.rate)
    }
    fn stop(&mut self) {
        self.next_generation();
    }
    fn sample_rate(&self) -> u32 {
        self.flush();
        if let Some(metadata) = self.metadata()
            && metadata.ready
            && metadata.revision == self.shared.requested.load(Ordering::Acquire)
        {
            self.observed.set(metadata.format);
        }
        self.observed.get().rate
    }
    fn channels(&self) -> usize {
        self.observed.get().channels
    }
    fn played_frames(&self) -> Option<u64> {
        let metadata = self.metadata()?;
        let (generation, frames) = metadata.clock.as_ref()?;
        self.shared.live(*generation).then(|| frames.load(Ordering::Acquire))
    }
    fn recover(&mut self) {
        self.flush();
    }
    fn configure(&mut self, hardware: &AudioHardwarePrefs, document_rate: Option<u32>) {
        self.stop();
        if hardware.device_class.len() > 1024 || hardware.default_output.len() > 1024 || hardware.default_input.len() > 1024 {
            self.configuration_error = Some("Audio device names exceed 1024 bytes; choose a shorter valid device name".into());
            return;
        }
        self.configuration_error = None;
        let Some(revision) = self.shared.requested.load(Ordering::Acquire).checked_add(1) else {
            self.shared.failed.store(true, Ordering::Release);
            return;
        };
        self.shared.requested.store(revision, Ordering::Release);
        let Ok(mut pending) = self.pending.try_borrow_mut() else {
            self.shared.failed.store(true, Ordering::Release);
            return;
        };
        *pending = Some(Request::Configure { revision, hardware: hardware.clone(), document_rate, language: filmcraft_ui_egui::i18n::current() });
        drop(pending);
        self.flush();
    }
    fn devices(&self) -> AudioDevices {
        self.flush();
        self.shared.enumerate.store(true, Ordering::Release);
        self.shared.ctx.request_repaint_after(Duration::from_millis(100));
        self.metadata().map(|m| m.devices.clone()).unwrap_or_default()
    }
    fn note(&self) -> Option<String> {
        self.flush();
        use filmcraft_ui_egui::i18n::t;
        if let Some(error) = &self.configuration_error {
            return Some(error.clone());
        }
        if self.shared.failed.load(Ordering::Acquire) {
            return Some(t("Audio worker unavailable; restart FilmCraft when convenient").into());
        }
        let metadata = self.metadata()?;
        if let Some(since) = metadata.busy {
            self.shared.ctx.request_repaint_after(Duration::from_millis(100));
            if since.elapsed() >= DEADLINE {
                return Some(t("Audio hardware is not responding; the editor remains available. Check Settings > Audio Hardware.").into());
            }
        }
        metadata.note.clone()
    }
}

fn run<B: AudioOut>(shared: &Arc<Shared>, receive: Receiver<Request>, factory: impl FnOnce() -> B) {
    let mut backend = factory();
    let mut running = None;
    let mut active_frames = Arc::new(AtomicU64::new(0));
    let mut enumerated = None::<Instant>;
    loop {
        if shared.closed.load(Ordering::Acquire) {
            return;
        }
        if running.is_some_and(|generation| !shared.live(generation)) {
            shared.busy();
            backend.stop();
            running = None;
            let mut metadata = shared.metadata.lock().unwrap_or_else(PoisonError::into_inner);
            metadata.clock = None;
            metadata.busy = None;
        }
        if shared.closed.load(Ordering::Acquire) {
            return;
        }
        match receive.recv_timeout(POLL) {
            Ok(Request::Configure { revision, hardware, document_rate, language }) => {
                if shared.closed.load(Ordering::Acquire) || revision != shared.requested.load(Ordering::Acquire) {
                    continue;
                }
                shared.busy();
                filmcraft_ui_egui::i18n::set_current(language);
                backend.stop();
                running = None;
                backend.configure(&hardware, document_rate);
                let format = Format { rate: backend.sample_rate(), channels: backend.channels() };
                let note = backend.note().map(|s| text_limit(s, 4096));
                let mut metadata = shared.metadata.lock().unwrap_or_else(PoisonError::into_inner);
                metadata.ready =
                    revision == shared.requested.load(Ordering::Acquire) && format.rate > 0 && format.rate <= 768_000 && (1..=256).contains(&format.channels);
                metadata.format = format;
                metadata.revision = revision;
                metadata.note = note;
                metadata.busy = None;
                metadata.clock = None;
                drop(metadata);
                shared.ctx.request_repaint();
            }
            Ok(Request::Start { generation, revision, format, mut fill }) => {
                if !shared.live(generation) || revision != shared.requested.load(Ordering::Acquire) {
                    continue;
                }
                shared.busy();
                let gate = Arc::new(AtomicBool::new(false));
                let callback_gate = gate.clone();
                let state = shared.clone();
                // Silent priming/cancelled buffers must not advance the program clock.
                let delivered = Arc::new(AtomicU64::new(0));
                let counted = delivered.clone();
                let mut total = 0u64;
                let result = backend.start(Box::new(move |buffer, channels| {
                    if callback_gate.load(Ordering::Acquire) && state.live(generation) && channels == format.channels {
                        fill(buffer, channels);
                        if !state.live(generation) {
                            buffer.fill(0.0);
                            return;
                        }
                        total = total.saturating_add((buffer.len() / channels.max(1)) as u64);
                        counted.store(total, Ordering::Release);
                    } else {
                        buffer.fill(0.0);
                    }
                }));
                let actual = Format { rate: backend.sample_rate(), channels: backend.channels() };
                let valid_format = actual.rate > 0 && actual.rate <= 768_000 && (1..=256).contains(&actual.channels);
                let valid = result.as_ref().is_ok_and(|rate| *rate == format.rate)
                    && actual == format
                    && shared.live(generation)
                    && revision == shared.requested.load(Ordering::Acquire);
                if valid {
                    gate.store(true, Ordering::Release);
                    running = Some(generation);
                    active_frames = delivered.clone();
                }
                let note = if valid {
                    backend.note()
                } else {
                    Some(result.err().unwrap_or_else(|| "Audio format changed or playback was cancelled; retry playback".into()))
                };
                let has_clock = valid && backend.played_frames().is_some();
                {
                    let mut metadata = shared.metadata.lock().unwrap_or_else(PoisonError::into_inner);
                    metadata.format = actual;
                    metadata.ready = valid_format && revision == shared.requested.load(Ordering::Acquire);
                    metadata.clock = has_clock.then(|| (generation, delivered.clone()));
                    metadata.busy = None;
                    metadata.note = note.map(|s| text_limit(s, 4096));
                }
                shared.ctx.request_repaint();
                if !valid {
                    shared.busy();
                    backend.stop();
                    shared.metadata.lock().unwrap_or_else(PoisonError::into_inner).busy = None;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        if shared.closed.load(Ordering::Acquire) {
            return;
        }
        if let Some(generation) = running {
            if !shared.live(generation) {
                continue;
            }
            shared.busy();
            backend.recover();
            let has_clock = backend.played_frames().is_some();
            let note = backend.note().map(|s| text_limit(s, 4096));
            let mut metadata = shared.metadata.lock().unwrap_or_else(PoisonError::into_inner);
            metadata.clock = (has_clock && shared.live(generation)).then(|| (generation, active_frames.clone()));
            metadata.note = note;
            metadata.busy = None;
        }
        if shared.enumerate.load(Ordering::Acquire) && enumerated.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) {
            shared.enumerate.store(false, Ordering::Release);
            shared.busy();
            let mut devices = backend.devices();
            for names in [&mut devices.hosts, &mut devices.inputs, &mut devices.outputs] {
                names.truncate(128);
                for name in names {
                    *name = text_limit(std::mem::take(name), 1024);
                }
            }
            devices.output_channels = devices.output_channels.min(256);
            let mut metadata = shared.metadata.lock().unwrap_or_else(PoisonError::into_inner);
            metadata.devices = devices;
            metadata.busy = None;
            enumerated = Some(Instant::now());
            drop(metadata);
            shared.ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;
    use std::sync::atomic::AtomicUsize;

    struct Blocker {
        phase: AtomicUsize,
        entered: SyncSender<usize>,
        release: Mutex<Receiver<()>>,
    }
    impl Blocker {
        fn hit(&self, phase: usize) {
            if self.phase.compare_exchange(phase, 0, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                self.entered.send(phase).unwrap();
                let _ = self.release.lock().unwrap().recv();
            }
        }
    }

    // Deliberately !Send: the backend must be constructed and kept on its owner thread.
    struct Device {
        _local: Rc<()>,
        callback: Arc<Mutex<Option<Fill>>>,
        entered: Option<SyncSender<()>>,
        release: Option<Receiver<()>>,
        panic_config: bool,
        rate: u32,
        change_on_start: Option<u32>,
        configure_entered: Option<SyncSender<()>>,
        configure_release: Option<Receiver<()>>,
        configured: Arc<Mutex<Vec<Option<u32>>>>,
        blocker: Option<Arc<Blocker>>,
        auto_fill: bool,
        dropped: Arc<AtomicBool>,
    }
    impl Device {
        fn new(callback: Arc<Mutex<Option<Fill>>>, dropped: Arc<AtomicBool>) -> Self {
            Self {
                _local: Rc::new(()),
                callback,
                entered: None,
                release: None,
                panic_config: false,
                rate: 48_000,
                change_on_start: None,
                configure_entered: None,
                configure_release: None,
                configured: Arc::new(Mutex::new(Vec::new())),
                blocker: None,
                auto_fill: true,
                dropped,
            }
        }
    }
    impl Drop for Device {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }
    impl AudioOut for Device {
        fn start(&mut self, mut fill: Fill) -> Result<u32, String> {
            if let Some(entered) = self.entered.take() {
                entered.send(()).unwrap();
            }
            if let Some(release) = self.release.take() {
                let _ = release.recv();
            }
            let mut priming = [1.0; 8];
            fill(&mut priming, 2);
            assert_eq!(priming, [0.0; 8], "opening the device must not consume program audio before format validation");
            *self.callback.lock().unwrap() = Some(fill);
            if let Some(rate) = self.change_on_start.take() {
                self.rate = rate;
            }
            Ok(self.rate)
        }
        fn stop(&mut self) {
            if let Some(blocker) = &self.blocker {
                blocker.hit(1);
            }
            // Simulate a driver that can still deliver a late callback.
        }
        fn recover(&mut self) {
            if let Some(blocker) = &self.blocker {
                blocker.hit(2);
            }
            if self.auto_fill
                && let Some(fill) = self.callback.lock().unwrap().as_mut()
            {
                fill(&mut [0.0; 512], 2);
            }
        }
        fn devices(&self) -> AudioDevices {
            if let Some(blocker) = &self.blocker {
                blocker.hit(3);
            }
            AudioDevices::default()
        }
        fn sample_rate(&self) -> u32 {
            self.rate
        }
        fn played_frames(&self) -> Option<u64> {
            Some(256)
        }
        fn configure(&mut self, _: &AudioHardwarePrefs, rate: Option<u32>) {
            assert!(!self.panic_config, "synthetic driver panic");
            if let Some(entered) = self.configure_entered.take() {
                entered.send(()).unwrap();
            }
            if let Some(release) = self.configure_release.take() {
                let _ = release.recv();
            }
            self.configured.lock().unwrap().push(rate);
        }
    }
    fn wait(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !condition() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(condition(), "worker did not reach the expected state");
    }
    fn ready(output: &mut Output) {
        output.configure(&AudioHardwarePrefs::default(), None);
        wait(|| output.metadata().is_some_and(|m| m.ready && m.revision == output.shared.requested.load(Ordering::Acquire)));
        assert_eq!(output.sample_rate(), 48_000);
        assert_eq!(output.channels(), 2);
    }
    #[test]
    fn blocked_constructor_leaves_all_window_calls_and_drop_available() {
        let (entered, entering) = sync_channel(1);
        let (release, blocked) = sync_channel(1);
        let dropped = Arc::new(AtomicBool::new(false));
        let done = dropped.clone();
        let mut output = Output::new(egui::Context::default(), move || {
            entered.send(()).unwrap();
            let _ = blocked.recv();
            Device::new(Arc::new(Mutex::new(None)), done)
        });
        entering.recv_timeout(Duration::from_secs(3)).unwrap();
        output.configure(&AudioHardwarePrefs::default(), None);
        output.poll();
        output.stop();
        output.recover();
        assert_eq!(output.sample_rate(), 48_000);
        assert_eq!(output.channels(), 2);
        assert!(output.devices().outputs.is_empty());
        assert!(output.start(Box::new(|_, _| {})).is_err());
        assert_eq!(output.played_frames(), None);
        output.shared.metadata.lock().unwrap().busy = Some(Instant::now() - DEADLINE);
        assert!(output.note().unwrap().contains("not responding"));
        drop(output); // Must not join the still-blocked backend.
        release.send(()).unwrap();
        wait(|| dropped.load(Ordering::Acquire));
    }
    #[test]
    fn cancelled_start_never_plays_a_late_driver_callback() {
        let callback = Arc::new(Mutex::new(None));
        let worker_callback = callback.clone();
        let (entered, entering) = sync_channel(1);
        let (release, blocked) = sync_channel(1);
        let mut output = Output::new(egui::Context::default(), move || {
            let mut device = Device::new(worker_callback, Arc::new(AtomicBool::new(false)));
            device.entered = Some(entered);
            device.release = Some(blocked);
            device
        });
        ready(&mut output);
        let calls = Arc::new(AtomicUsize::new(0));
        let filling = calls.clone();
        output
            .start(Box::new(move |buffer, _| {
                filling.fetch_add(1, Ordering::AcqRel);
                buffer.fill(1.0);
            }))
            .unwrap();
        entering.recv_timeout(Duration::from_secs(3)).unwrap();
        output.stop();
        assert_eq!(output.played_frames(), None);
        release.send(()).unwrap();
        wait(|| callback.lock().unwrap().is_some());
        let mut buffer = [1.0; 8];
        callback.lock().unwrap().as_mut().unwrap()(&mut buffer, 2);
        assert_eq!(buffer, [0.0; 8]);
        assert_eq!(calls.load(Ordering::Acquire), 0);
    }
    #[test]
    fn ready_clock_and_immediate_stop_preserve_the_normal_audio_path() {
        let callback = Arc::new(Mutex::new(None));
        let worker_callback = callback.clone();
        let mut output = Output::new(egui::Context::default(), move || Device::new(worker_callback, Arc::new(AtomicBool::new(false))));
        ready(&mut output);
        output.start(Box::new(|buffer, _| buffer.fill(0.25))).unwrap();
        wait(|| output.played_frames().is_some_and(|frames| frames >= 256));
        let mut buffer = [0.0; 8];
        callback.lock().unwrap().as_mut().unwrap()(&mut buffer, 2);
        assert_eq!(buffer, [0.25; 8]);
        output.stop();
        callback.lock().unwrap().as_mut().unwrap()(&mut buffer, 2);
        assert_eq!(buffer, [0.0; 8]);
        assert_eq!(output.played_frames(), None);
    }
    #[test]
    fn a_changed_device_format_is_silent_then_can_be_retried_at_the_new_rate() {
        let callback = Arc::new(Mutex::new(None));
        let worker_callback = callback.clone();
        let mut output = Output::new(egui::Context::default(), move || {
            let mut device = Device::new(worker_callback, Arc::new(AtomicBool::new(false)));
            device.change_on_start = Some(44_100);
            device
        });
        ready(&mut output);
        output.start(Box::new(|buffer, _| buffer.fill(0.5))).unwrap();
        wait(|| output.metadata().is_some_and(|m| m.format.rate == 44_100 && m.clock.is_none()));
        let mut buffer = [1.0; 8];
        callback.lock().unwrap().as_mut().unwrap()(&mut buffer, 2);
        assert_eq!(buffer, [0.0; 8], "never play a mixer at the wrong sample rate");
        assert_eq!(output.sample_rate(), 44_100);
        assert_eq!(output.start(Box::new(|buffer, _| buffer.fill(0.5))).unwrap(), 44_100);
        wait(|| output.played_frames().is_some_and(|frames| frames >= 256));
        callback.lock().unwrap().as_mut().unwrap()(&mut buffer, 2);
        assert_eq!(buffer, [0.5; 8]);
    }
    #[test]
    fn blocked_configuration_coalesces_to_the_latest_settings_without_more_workers() {
        let (entered, entering) = sync_channel(1);
        let (release, blocked) = sync_channel(1);
        let configured = Arc::new(Mutex::new(Vec::new()));
        let settings = configured.clone();
        let mut output = Output::new(egui::Context::default(), move || {
            let mut device = Device::new(Arc::new(Mutex::new(None)), Arc::new(AtomicBool::new(false)));
            device.configure_entered = Some(entered);
            device.configure_release = Some(blocked);
            device.configured = settings;
            device
        });
        output.configure(&AudioHardwarePrefs::default(), None);
        entering.recv_timeout(Duration::from_secs(3)).unwrap();
        for n in 0..32 {
            output.configure(&AudioHardwarePrefs::default(), Some(48_000 + n));
        }
        assert!(output.start(Box::new(|_, _| {})).is_err());
        assert_eq!(output.played_frames(), None);
        release.send(()).unwrap();
        wait(|| {
            output.poll();
            configured.lock().unwrap().last() == Some(&Some(48_031))
        });
        assert!(configured.lock().unwrap().len() <= 2, "superseded configurations must not probe the driver");
    }
    #[test]
    fn invalid_device_names_report_an_error_and_valid_settings_recover() {
        let mut output = Output::new(egui::Context::default(), || Device::new(Arc::new(Mutex::new(None)), Arc::new(AtomicBool::new(false))));
        ready(&mut output);
        let hardware = AudioHardwarePrefs { default_input: "x".repeat(1025), ..Default::default() };
        output.configure(&hardware, None);
        assert!(output.note().unwrap().contains("1024"));
        assert!(output.start(Box::new(|_, _| {})).is_err());
        ready(&mut output);
        assert!(!output.shared.failed.load(Ordering::Acquire));
        assert!(output.start(Box::new(|_, _| {})).is_ok());
    }
    #[test]
    fn a_panicking_driver_is_an_audio_error_not_a_process_panic() {
        let mut output = Output::new(egui::Context::default(), || {
            let mut device = Device::new(Arc::new(Mutex::new(None)), Arc::new(AtomicBool::new(false)));
            device.panic_config = true;
            device
        });
        output.configure(&AudioHardwarePrefs::default(), None);
        wait(|| output.shared.failed.load(Ordering::Acquire));
        assert!(output.note().unwrap().contains("unavailable"));
        assert!(output.start(Box::new(|_, _| {})).is_err());
    }

    #[test]
    fn stop_during_a_fill_silences_the_completed_buffer() {
        let callback = Arc::new(Mutex::new(None));
        let worker_callback = callback.clone();
        let mut output = Output::new(egui::Context::default(), move || {
            let mut device = Device::new(worker_callback, Arc::new(AtomicBool::new(false)));
            device.auto_fill = false;
            device
        });
        ready(&mut output);
        let (entered, entering) = sync_channel(1);
        let (release, blocked) = sync_channel(1);
        output
            .start(Box::new(move |buffer, _| {
                entered.send(()).unwrap();
                let _ = blocked.recv();
                buffer.fill(1.0);
            }))
            .unwrap();
        wait(|| output.metadata().is_some_and(|m| m.busy.is_none()) && callback.lock().unwrap().is_some());
        let filling = std::thread::spawn(move || {
            let mut buffer = [0.0; 8];
            callback.lock().unwrap().as_mut().unwrap()(&mut buffer, 2);
            buffer
        });
        entering.recv_timeout(Duration::from_secs(3)).unwrap();
        output.stop();
        release.send(()).unwrap();
        assert_eq!(filling.join().unwrap(), [0.0; 8]);
        assert_eq!(output.played_frames(), None);
    }
    #[test]
    fn blocked_stop_recovery_and_enumeration_cannot_hold_the_window_or_stale_audio() {
        for phase in [1, 2, 3] {
            let (entered, entering) = sync_channel(1);
            let (release, blocked) = sync_channel(1);
            let blocker = Arc::new(Blocker { phase: AtomicUsize::new(0), entered, release: Mutex::new(blocked) });
            let worker_blocker = blocker.clone();
            let callback = Arc::new(Mutex::new(None));
            let worker_callback = callback.clone();
            let dropped = Arc::new(AtomicBool::new(false));
            let done = dropped.clone();
            let mut output = Output::new(egui::Context::default(), move || {
                let mut device = Device::new(worker_callback, done);
                device.blocker = Some(worker_blocker);
                device
            });
            ready(&mut output);
            if phase == 2 {
                blocker.phase.store(phase, Ordering::Release);
            }
            output.start(Box::new(|buffer, _| buffer.fill(0.5))).unwrap();
            if phase != 2 {
                wait(|| output.played_frames().is_some_and(|frames| frames >= 256));
                blocker.phase.store(phase, Ordering::Release);
            }
            if phase == 1 {
                output.stop();
            }
            if phase == 3 {
                output.devices();
            }
            entering.recv_timeout(Duration::from_secs(3)).unwrap();
            {
                let mut metadata = output.shared.metadata.lock().unwrap();
                metadata.busy = Some(Instant::now() - DEADLINE);
            }
            let (finished, finishing) = sync_channel(1);
            let window = std::thread::spawn(move || {
                if phase == 1 {
                    assert_eq!(output.played_frames(), None);
                } else {
                    let before = output.played_frames().unwrap();
                    let mut healthy = [0.0; 8];
                    callback.lock().unwrap().as_mut().unwrap()(&mut healthy, 2);
                    assert_eq!(healthy, [0.5; 8]);
                    assert_eq!(output.played_frames(), Some(before + 4), "a blocked device query must not freeze a healthy callback clock");
                }
                assert!(output.note().unwrap().contains("not responding"));
                assert_eq!(output.sample_rate(), 48_000);
                assert_eq!(output.channels(), 2);
                output.devices();
                output.recover();
                output.stop();
                let mut buffer = [1.0; 8];
                callback.lock().unwrap().as_mut().unwrap()(&mut buffer, 2);
                assert_eq!(buffer, [0.0; 8]);
                output.configure(&AudioHardwarePrefs::default(), None);
                output.poll();
                drop(output);
                finished.send(()).unwrap();
            });
            let available = finishing.recv_timeout(Duration::from_secs(2));
            // Always release the fake driver, including when the assertion will fail.
            release.send(()).unwrap();
            assert!(available.is_ok(), "window blocked by device operation {phase}");
            window.join().unwrap();
            wait(|| dropped.load(Ordering::Acquire));
        }
    }
}
