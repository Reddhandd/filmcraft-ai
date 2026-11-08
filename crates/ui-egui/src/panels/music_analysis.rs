//! Background song analysis, reviewable labels/boundaries, and optional video-only cuts.
use crate::FilmcraftApp;
use filmcraft_engine::music_analysis::{self as engine, Plan};
use filmcraft_project::{ClipId, TrackKind};
use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub song: ClipId,
    pub videos: Vec<ClipId>,
    pub plan: Option<Plan>,
    pub running: bool,
    pub status: String,
    pub beat_markers: bool,
    pub section_markers: bool,
    pub every: usize,
}
type Completion = Arc<Mutex<Option<Result<Plan, String>>>>;
fn job_id() -> egui::Id {
    egui::Id::unique("music-analysis-worker")
}
pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    let song = engine::selected(&app.session).map_err(|e| e.to_string())?;
    engine::prepare(&app.session, song).map_err(|e| e.to_string())?;
    let videos = app
        .session
        .state
        .selection
        .iter()
        .copied()
        .filter(|id| app.session.active_sequence().and_then(|q| q.find_item(*id).and_then(|(t, _)| q.track(t))).is_some_and(|t| t.kind == TrackKind::Video))
        .collect();
    app.ui.music_analysis = Some(Draft {
        song,
        videos,
        plan: None,
        running: false,
        status: "Analyze the studio song, review the suggestions, then apply.".into(),
        beat_markers: true,
        section_markers: true,
        every: 0,
    });
    Ok(json!({"dialog":"musicAnalysis"}))
}
fn start(app: &FilmcraftApp, ctx: &egui::Context, d: &mut Draft) {
    let input = match engine::prepare(&app.session, d.song) {
        Ok(v) => v,
        Err(e) => {
            d.status = e.to_string();
            return;
        }
    };
    d.plan = None;
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (input, ctx);
        d.status = "Music analysis currently requires the desktop app.".into();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let result: Completion = Arc::new(Mutex::new(None));
        let worker = result.clone();
        let repaint = ctx.clone();
        match std::thread::Builder::new().name("music-analysis".into()).spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| engine::analyze(input)))
                .map_err(|_| "Analysis failed internally. Your timeline was not changed.".to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            *worker.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
            repaint.request_repaint();
        }) {
            Ok(_) => {
                ctx.data_mut(|m| m.insert_temp(job_id(), result));
                d.running = true;
                d.status = "Finding beats and repeated musical sections…".into();
            }
            Err(e) => d.status = format!("Cannot start analysis: {e}"),
        }
    }
}
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.music_analysis.clone() else { return };
    if d.running {
        if let Some(result) = ctx.data(|m| m.get_temp::<Completion>(job_id())) {
            let outcome = result.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(outcome) = outcome {
                d.running = false;
                match outcome {
                    Ok(plan) => {
                        d.status = format!("{:.1} BPM · {} beats · {} suggested sections", plan.bpm, plan.beats.len(), plan.sections.len());
                        d.plan = Some(plan);
                    }
                    Err(e) => d.status = e,
                }
                ctx.data_mut(|m| m.remove::<Completion>(job_id()));
            }
        } else {
            d.running = false;
            d.status = "Analysis interrupted. Click Analyze again.".into();
        }
    }
    let (mut close, mut analyze, mut apply) = (false, false, false);
    let mut elements: Vec<(String, egui::Rect, String)> = Vec::new();
    egui::Window::new("Beats & Song Sections").collapsible(false).default_width(620.0).show(ctx, |ui| {
        ui.label("Beat markers and editable song-section suggestions.");
        ui.weak("Experimental: repeated, louder sections suggest choruses. Listen and correct the labels and boundaries.");
        let status = ui.label(&d.status);
        elements.push(("musicAnalysis.status".into(), status.rect, d.status.clone()));
        if d.running {
            ui.spinner();
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some(plan) = &mut d.plan {
            ui.weak("Section times are relative to the trimmed song. Click Listen to hear a boundary.");
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for index in 0..plan.sections.len().min(64) {
                    let previous = if index > 0 {
                        plan.sections.get(index - 1).map(|s| s.start.seconds() + 0.01).unwrap_or(0.0)
                    } else {
                        0.0
                    };
                    let Some(sec) = plan.sections.get_mut(index) else { continue };
                    let mut new_start = None;
                    ui.horizontal(|ui| {
                        let mut seconds = sec.start.seconds();
                        let hi = (sec.end.seconds() - 0.01).max(previous);
                        let r = ui.add_enabled(
                            index > 0,
                            egui::DragValue::new(&mut seconds).speed(0.05).range(previous..=hi).max_decimals(3).suffix(" s"),
                        );
                        if r.changed() && seconds.is_finite() {
                            new_start = Some(Tick::from_seconds_f64(seconds));
                        }
                        elements.push((format!("musicAnalysis.section.{index}.start"), r.rect, format!("{seconds:.3}")));
                        let name_label = ui.label("Section name");
                        let r = ui.text_edit_singleline(&mut sec.label).labelled_by(name_label.id);
                        elements.push((format!("musicAnalysis.section.{index}.label"), r.rect, sec.label.clone()));
                        let r = ui.button("Listen");
                        elements.push((format!("musicAnalysis.section.{index}.listen"), r.rect, "Listen".into()));
                        if r.clicked() {
                            app.stop();
                            if let Err(e) = app.session.execute("playhead.set", json!({"time":plan.song.start.0.saturating_add(sec.start.0)})) {
                                d.status = e.to_string();
                            } else {
                                app.play(1.0);
                            }
                        }
                    });
                    if let Some(t) = new_start {
                        if let Some(sec) = plan.sections.get_mut(index) {
                            sec.start = t;
                        }
                        if let Some(prev) = plan.sections.get_mut(index - 1) {
                            prev.end = t;
                        }
                    }
                }
            });
        }
        for (id, label, value) in [
            ("musicAnalysis.beats", "Add beat markers", &mut d.beat_markers),
            ("musicAnalysis.sections", "Add section markers", &mut d.section_markers),
        ] {
            let r = ui.checkbox(value, label);
            elements.push((id.into(), r.rect, value.to_string()));
        }
        ui.label(format!("{} selected video clips available for optional cuts.", d.videos.len()));
        ui.horizontal(|ui| {
            ui.label("Cut video every:");
            for (n, label) in [(0, "Off"), (1, "Beat"), (2, "2 beats"), (4, "4 beats"), (8, "8 beats"), (16, "16 beats")] {
                let r = ui.add_enabled(n == 0 || !d.videos.is_empty(), egui::RadioButton::new(d.every == n, label));
                if r.clicked() {
                    d.every = n;
                }
                elements.push((format!("musicAnalysis.every.{n}"), r.rect, label.into()));
            }
        });
        ui.weak("Each selected clip starts on beat one. Split off pickups or silence first; leave them unselected.");
        ui.weak("The first cut follows a complete beat group. Video cuts retain timing and speed; audio stays intact. Cuts unlink video from live audio. This does not choose camera angles.");
        ui.weak("Apply is one undo step. Reapplying replaces this song's generated markers.");
        ui.horizontal(|ui| {
            let r = ui.add_enabled(!d.running, egui::Button::new("Analyze"));
            analyze = r.clicked();
            elements.push(("musicAnalysis.analyze".into(), r.rect, "Analyze".into()));
            let r = ui.add_enabled(!d.running && d.plan.is_some(), egui::Button::new("Apply to timeline"));
            apply = r.clicked();
            elements.push(("musicAnalysis.apply".into(), r.rect, "Apply".into()));
            let r = ui.button("Close");
            close = r.clicked();
            elements.push(("musicAnalysis.close".into(), r.rect, "Close".into()));
        });
    });
    for (id, rect, label) in elements {
        app.auto.add(&id, rect, &label);
    }
    if analyze {
        start(app, ctx, &mut d);
    }
    if apply && let Some(plan) = &d.plan {
        app.stop();
        match app.session.execute(
            "clip.analyzeMusic.apply",
            json!({"plan":plan,"beatMarkers":d.beat_markers,"sectionMarkers":d.section_markers,"every":d.every,"videos":d.videos}),
        ) {
            Ok(v) => {
                app.ui.status = format!("Music analysis applied: {} markers, {} cuts. Command-Z undoes it.", v["markers"], v["cuts"]);
                close = true;
            }
            Err(e) => d.status = e.to_string(),
        }
    }
    if close {
        ctx.data_mut(|m| m.remove::<Completion>(job_id()));
        app.ui.music_analysis = None;
    } else {
        app.ui.music_analysis = Some(d);
    }
}
