//! User-controlled, local rolling diagnostics. No collection while disabled.
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const WINDOW: u64 = 600;
const MAX_EVENTS: usize = 12000;
// Live diagnostic data must not grow without limit during a long session.
const MAX_LIVE_BYTES: u64 = 4 * 1024 * 1024;
const REWRITE_INTERVAL: u64 = 30;

pub struct FlightRecorder {
    pub enabled: bool,
    pub window_open: bool,
    events: VecDeque<(u64, Value)>,
    last_revision: u64,
    journal_seen: usize,
    last_sample: u64,
    last_compaction: u64,
    live_file: Option<PathBuf>,
    pub notice: String,
    folder: PathBuf,
}
impl Default for FlightRecorder {
    fn default() -> Self {
        Self {
            enabled: false,
            window_open: false,
            events: VecDeque::new(),
            last_revision: 0,
            journal_seen: 0,
            last_sample: 0,
            last_compaction: 0,
            live_file: None,
            notice: String::new(),
            folder: std::env::temp_dir().join("FilmCraft Diagnostics"),
        }
    }
}
fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}
impl FlightRecorder {
    pub fn start(&mut self, revision: u64, journal_len: usize, folder: PathBuf) -> std::io::Result<()> {
        let result: std::io::Result<PathBuf> = (|| {
            fs::create_dir_all(&folder)?;
            let path = folder.join(format!("recording-{}-{}.jsonl", now(), std::process::id()));
            OpenOptions::new().write(true).create_new(true).open(&path)?;
            Ok(path)
        })();
        let path = match result {
            Ok(path) => path,
            Err(error) => {
                self.notice = format!("Could not start diagnostics: {error}");
                return Err(error);
            }
        };
        self.folder = folder;
        self.enabled = true;
        self.events.clear();
        self.last_sample = 0;
        self.last_compaction = now();
        self.last_revision = revision;
        self.journal_seen = journal_len;
        self.notice.clear();
        self.live_file = Some(path);
        self.push(json!({"event":"recording_started"}));
        Ok(())
    }
    pub fn stop(&mut self) -> std::io::Result<()> {
        self.enabled = false;
        self.events.clear();
        let result = match self.live_file.take() {
            Some(path) => fs::remove_file(path),
            None => Ok(()),
        };
        self.notice = match &result {
            Ok(()) => "Monitoring off. Unsaved diagnostic buffer discarded.".into(),
            Err(error) => format!("Monitoring off; could not remove live diagnostics: {error}"),
        };
        result
    }
    fn push(&mut self, value: Value) {
        if !self.enabled {
            return;
        }
        let timestamp = now();
        let entry = json!({"timestamp": timestamp, "data": value});
        self.events.push_back((timestamp, entry.clone()));
        while self.events.len() > MAX_EVENTS || self.events.front().is_some_and(|(t, _)| timestamp.saturating_sub(*t) > WINDOW) {
            self.events.pop_front();
        }
        // Write short structured records immediately; if FilmCraft crashes this file survives.
        if let Some(path) = self.live_file.clone() {
            match OpenOptions::new().append(true).open(&path).and_then(|mut file| writeln!(file, "{entry}")) {
                Ok(()) => {}
                Err(e) => {
                    self.notice = format!("Diagnostic write failed: {e}");
                    self.live_file = None;
                }
            }
            let overdue = timestamp.saturating_sub(self.last_compaction) >= REWRITE_INTERVAL;
            let oversized = fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LIVE_BYTES);
            if overdue || oversized {
                self.last_compaction = timestamp;
                if let Err(e) = self.compact_live_file(&path) {
                    self.notice = format!("Diagnostic log compaction failed: {e}");
                    self.live_file = None;
                }
            }
        }
    }
    // Atomic replacement keeps the most recent complete log if FilmCraft exits
    // during compaction. No screenshot/video capture or recording while Off.
    fn compact_live_file(&self, path: &std::path::Path) -> std::io::Result<()> {
        let tmp = path.with_extension("jsonl.tmp");
        let result = (|| {
            let mut file = File::create(&tmp)?;
            for (_, entry) in &self.events {
                writeln!(file, "{entry}")?;
            }
            file.sync_all()?;
            fs::rename(&tmp, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
    pub fn sample(&mut self, revision: u64, journal: &[(String, Value)], fps: f32, project_dirty: bool) {
        if !self.enabled {
            return;
        }
        if journal.len() < self.journal_seen {
            self.journal_seen = 0;
        }
        for (id, _params) in journal.iter().skip(self.journal_seen).take(200) {
            // Deliberately omit command parameters: they may contain project names/media paths.
            self.push(json!({"event":"command", "id":id}));
        }
        self.journal_seen = journal.len();
        if self.last_revision != revision {
            self.push(json!({"event":"project_changed", "revision":revision}));
            self.last_revision = revision;
        }
        let timestamp = now();
        if timestamp.saturating_sub(self.last_sample) >= 1 {
            self.last_sample = timestamp;
            self.push(json!({"event":"performance", "fps":(fps*10.0).round()/10.0,"dirty":project_dirty}));
        }
    }
    pub fn capture(&mut self, seconds: u64) -> std::io::Result<PathBuf> {
        let result: std::io::Result<PathBuf> = (|| {
            if !self.enabled {
                return Err(std::io::Error::other("Turn monitoring on first"));
            }
            if !(1..=WINDOW).contains(&seconds) {
                return Err(std::io::Error::other("Capture window must be between 1 and 600 seconds"));
            }
            fs::create_dir_all(&self.folder)?;
            let path = self.folder.join(format!("FilmCraft-last-{}s-{}-{}.jsonl", seconds, now(), std::process::id()));
            let cutoff = now().saturating_sub(seconds);
            let mut file = OpenOptions::new().write(true).create_new(true).open(&path)?;
            for (_, entry) in self.events.iter().filter(|(t, _)| *t >= cutoff) {
                writeln!(file, "{entry}")?;
            }
            file.sync_all()?;
            Ok(path)
        })();
        self.notice = match &result {
            Ok(path) => format!("Report saved: {}", path.display()),
            Err(error) => format!("Report failed: {error}"),
        };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_recording_stays_empty() {
        let mut rec = FlightRecorder::default();
        rec.push(json!({"event":"ignored"}));
        rec.sample(9, &[], 60.0, false);
        assert!(rec.events.is_empty());
        assert!(rec.live_file.is_none());
    }
    #[test]
    fn rolling_buffer_keeps_latest_entries() {
        let mut rec = FlightRecorder { enabled: true, ..Default::default() };
        for i in 0..(MAX_EVENTS + 25) {
            rec.push(json!({"n":i}));
        }
        assert_eq!(rec.events.len(), MAX_EVENTS);
        assert_eq!(rec.events.back().unwrap().1["data"]["n"], (MAX_EVENTS + 24) as u64);
    }
    #[test]
    fn compaction_rewrites_only_current_buffer() {
        let dir = std::env::temp_dir().join(format!("filmcraft-recorder-test-{}-{}", std::process::id(), now()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("live.jsonl");
        fs::write(&file, b"old line\n").unwrap();
        let mut rec = FlightRecorder { enabled: true, ..Default::default() };
        rec.push(json!({"event":"new"}));
        rec.compact_live_file(&file).unwrap();
        let written = fs::read_to_string(&file).unwrap();
        assert!(!written.contains("old line"));
        assert!(written.contains("new"));
        fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod privacy_tests {
    use super::*;
    #[test]
    fn command_parameters_never_enter_report_and_write_failure_stays_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = FlightRecorder::default();
        recorder.start(0, 0, dir.path().to_path_buf()).unwrap();
        recorder.sample(1, &[("file.import".into(), json!({"paths":["PRIVATE-MEDIA-PATH"]}))], 60.0, true);
        let report = serde_json::to_string(&recorder.events).unwrap();
        assert!(report.contains("file.import"));
        assert!(!report.contains("PRIVATE-MEDIA-PATH"));
        let live = recorder.live_file.clone().unwrap();
        fs::remove_file(&live).unwrap();
        fs::create_dir(&live).unwrap();
        recorder.push(json!({"event":"write-failure"}));
        assert!(recorder.notice.contains("failed"));
        assert!(recorder.live_file.is_none(), "stop disk writes after a filesystem error");
        recorder.stop().unwrap();
        assert!(!recorder.enabled && recorder.events.is_empty());
    }
}

/// Explicit local report capture; monitoring remains opt-in.
pub fn show(app: &mut crate::FilmcraftApp, ctx: &egui::Context) {
    if !app.flight_recorder.window_open {
        return;
    }
    let mut open = true;
    let mut close = false;
    egui::Window::new(crate::i18n::t("Flight Recorder")).open(&mut open).show(ctx, |ui| {
        ui.label(if app.flight_recorder.enabled { "Monitoring on: rolling 10 minutes" } else { "Monitoring off" });
        ui.label("Reports contain command IDs and performance counters; no command arguments or media paths.");
        for seconds in [30, 60, 120, 600] {
            let response = ui.add_enabled(app.flight_recorder.enabled, egui::Button::new(format!("Capture last {seconds} seconds")));
            app.auto.add_response(&format!("diagnostics.capture.{seconds}"), &response, "Capture diagnostic report");
            if response.clicked()
                && let Err(error) = app.flight_recorder.capture(seconds)
            {
                app.ui.status = error.to_string();
            }
        }
        if !app.flight_recorder.notice.is_empty() {
            ui.label(&app.flight_recorder.notice);
        }
        let response = ui.button(crate::i18n::t("Close"));
        app.auto.add_response("diagnostics.close", &response, "Close diagnostics");
        close = response.clicked();
    });
    app.flight_recorder.window_open = open && !close && !ctx.input(|i| i.key_pressed(egui::Key::Escape) && !i.pointer.any_down());
}
