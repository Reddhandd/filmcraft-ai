//! Reviewed, local performance matching. UI state is serializable; the worker only analyzes.
use crate::FilmcraftApp;
use filmcraft_engine::performance_match::{self as engine, PassageSearch, Plan};
use filmcraft_project::ClipId;
use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub video: ClipId,
    pub song: ClipId,
    pub plan: Option<Plan>,
    pub mute_live: bool,
    #[serde(default)]
    pub lyrics_differ: bool,
    pub status: String,
    #[serde(default)]
    pub show_trim_warning: bool,
    #[serde(default)]
    pub dont_show_trim_warning: bool,
    #[serde(default)]
    pub warning_error: Option<String>,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub passages: Option<PassageSearch>,
    #[serde(default)]
    pub candidate: usize,
    #[serde(default)]
    pub confirmed: [bool; 3],
    #[serde(default)]
    pub boundaries_reviewed: bool,
    #[serde(default)]
    pub passage_start: Tick,
    #[serde(default)]
    pub passage_end: Tick,
}

enum Analysis {
    Match(Plan),
    Passages(PassageSearch),
}
#[derive(Clone)]
struct Completion {
    result: Arc<Mutex<Option<Result<Analysis, String>>>>,
    cancelled: Arc<AtomicBool>,
}
fn job_id() -> egui::Id {
    egui::Id::unique("performance-match-worker")
}

pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    // Reopening the menu must not orphan an active worker and start another.
    if app.ui.performance_match.is_some() {
        return Ok(json!({"dialog":"performanceMatch"}));
    }
    let (video, song) = engine::selected(&app.session).map_err(|e| e.to_string())?;
    engine::prepare(&app.session, video, song).map_err(|e| e.to_string())?;
    app.ui.performance_match = Some(Draft {
        video,
        song,
        plan: None,
        mute_live: true,
        lyrics_differ: false,
        status: "Find passages if the arrangement or trim is uncertain. Confirm corresponding phrases before matching.".into(),
        show_trim_warning: app.session.prefs.general.show_performance_match_trim_warning,
        dont_show_trim_warning: false,
        warning_error: None,
        running: false,
        passages: None,
        candidate: 0,
        confirmed: [false; 3],
        boundaries_reviewed: false,
        passage_start: Tick::ZERO,
        passage_end: Tick::ZERO,
    });
    Ok(json!({"dialog":"performanceMatch"}))
}

fn start(app: &FilmcraftApp, ctx: &egui::Context, draft: &mut Draft, passages: bool) {
    let input = match engine::prepare(&app.session, draft.video, draft.song) {
        Ok(v) => v,
        Err(e) => {
            draft.status = e.to_string();
            return;
        }
    };
    draft.plan = None;
    draft.passages = None;
    draft.confirmed = [false; 3];
    draft.boundaries_reviewed = false;
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (input, ctx, passages);
        draft.status = "Performance matching currently requires the desktop app.".into();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let result = Completion { result: Arc::new(Mutex::new(None)), cancelled: Arc::new(AtomicBool::new(false)) };
        let worker = result.clone();
        let repaint = ctx.clone();
        match std::thread::Builder::new().name("performance-match".into()).spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if passages {
                    engine::find_passages_cancellable(input, || worker.cancelled.load(Ordering::Relaxed)).map(Analysis::Passages)
                } else {
                    engine::analyze(input).map(Analysis::Match)
                }
            }))
            .map_err(|_| "Analysis failed internally. Your timeline was not changed.".to_string())
            .and_then(|v| v.map_err(|e| e.to_string()));
            *worker.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
            repaint.request_repaint();
        }) {
            Ok(_) => {
                ctx.data_mut(|m| m.insert_temp(job_id(), result));
                draft.running = true;
                draft.status = "Analyzing musical timing locally…".into();
            }
            Err(e) => draft.status = format!("Could not start analysis: {e}"),
        }
    }
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut draft) = app.ui.performance_match.clone() else {
        return;
    };
    if draft.show_trim_warning {
        let mut proceed = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("performance-match-trim-warning")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading("Before Performance Match");
            ui.label("For best results, trim the live clip and studio song to corresponding start and end beats before Analyze. FilmCraft can suggest a passage and ending, but these estimates can be wrong. Check the musical phrases and both boundaries before applying a match.");
            if let Some(error) = &draft.warning_error {
                ui.colored_label(egui::Color32::RED, error);
            }
            let r = ui.checkbox(&mut draft.dont_show_trim_warning, "Don't show this again");
            app.auto.add("performanceMatch.trimWarning.dontShowAgain", r.rect, "Don't show this again");
            ui.horizontal(|ui| {
                let r = ui.button("Cancel");
                app.auto.add("performanceMatch.trimWarning.cancel", r.rect, "Cancel");
                cancel = r.clicked();
                let r = ui.button("Continue");
                app.auto.add("performanceMatch.trimWarning.continue", r.rect, "Continue");
                proceed = r.clicked();
            });
        });
        if proceed {
            if draft.dont_show_trim_warning {
                match app.session.execute("prefs.set", json!({"key":"general.showPerformanceMatchTrimWarning","value":false})) {
                    Ok(_) => draft.show_trim_warning = false,
                    Err(error) => draft.warning_error = Some(format!("Could not save preference: {error}")),
                }
            } else {
                draft.show_trim_warning = false;
            }
        }
        if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            app.ui.performance_match = None;
        } else {
            app.ui.performance_match = Some(draft);
        }
        return;
    }
    if draft.running {
        if let Some(result) = ctx.data(|m| m.get_temp::<Completion>(job_id())) {
            let outcome = result.result.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(outcome) = outcome {
                draft.running = false;
                match outcome {
                    Ok(Analysis::Match(plan)) => {
                        draft.status = format!(
                            "Audio-pattern ranking score {:.3}. Visible performance sync is not verified; review singing and drum hits.",
                            plan.similarity
                        );
                        draft.plan = Some(plan);
                    }
                    Ok(Analysis::Passages(report)) => {
                        draft.candidate = 0;
                        if let Some(c) = report.candidates.first() {
                            draft.passage_start = c.live_start;
                            draft.passage_end = c.live_end;
                        }
                        draft.status =
                            "Unverified passage hypotheses. Compare phrases; rankings do not establish the correct repeat or exact boundaries.".into();
                        draft.passages = Some(report);
                    }
                    Err(e) => draft.status = e,
                }
                ctx.data_mut(|m| m.remove::<Completion>(job_id()));
            }
        } else {
            draft.running = false;
            draft.status = "Analysis was interrupted. Click Analyze again.".into();
        }
    }
    let mut close = false;
    let mut analyze = false;
    let mut apply = false;
    let mut find_passages = false;
    let mut trim_passage = false;
    let mut preview = None;
    let mut elements: Vec<(String, egui::Rect, String)> = Vec::new();
    egui::Window::new("Match Performance to Song").collapsible(false).default_width(610.0).show(ctx, |ui| {
        ui.set_min_width(560.0);
        ui.set_max_width(680.0);
        ui.label("Match a live rendition to the studio song by adjusting the video's speed.");
        ui.weak("Experimental: same key and arrangement work best. Trim count-ins, applause and extra sections first.");
        if let Some(seq) = app.session.active_sequence() {
            let video = seq.find_item(draft.video).map(|(_, it)| it.name.as_str()).unwrap_or("Missing video");
            let song = seq.find_item(draft.song).map(|(_, it)| it.name.as_str()).unwrap_or("Missing song");
            ui.label(format!("Live video: {video}"));
            ui.label(format!("Studio song: {song}"));
        }
        ui.add_space(8.0);
        let r = ui.label(&draft.status);
        elements.push(("performanceMatch.status".into(), r.rect, draft.status.clone()));
        if draft.running {
            ui.spinner();
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some(report) = &draft.passages {
            ui.separator();
            ui.label("Candidate passages — review required");
            ui.weak("These may be different repeats. Missing sections are not detected. Choose a candidate only after comparing its musical context.");
            for (index, c) in report.candidates.iter().take(5).enumerate() {
                let r = ui.radio_value(&mut draft.candidate, index, format!("Candidate {}: {:.2}–{:.2} s from live In; ranking {:.3}", index+1, c.live_start.seconds(), c.live_end.seconds(), c.ranking_score));
                if r.changed() {
                    draft.passage_start = c.live_start;
                    draft.passage_end = c.live_end;
                    draft.confirmed = [false; 3];
                    draft.boundaries_reviewed = false;
                }
                elements.push((format!("performanceMatch.candidate.{index}"), r.rect, format!("Candidate {}", index+1)));
            }
            ui.weak("Open each phrase in Source, then play/listen there. Preview locations are approximate; seek for context. Matching phrases do not establish frame-accurate sync.");
            for (i, label) in ["Start", "Middle", "End"].into_iter().enumerate() {
                ui.horizontal(|ui| {
                    for (is_song, name) in [(true, "Studio"), (false, "Live")] {
                        let r = ui.button(format!("{label}: {name}"));
                        if r.clicked() {
                            let (item, base, length) = if is_song {
                                (report.song.item, report.song.source_in, report.song.duration)
                            } else {
                                (report.video.item, Tick(report.video.source_in.0.saturating_add(draft.passage_start.0)), Tick(draft.passage_end.0.saturating_sub(draft.passage_start.0)))
                            };
                            // 5%, 50%, 95% are coarse context previews, not event anchors.
                            let percent = [5i64, 50, 95].get(i).copied().unwrap_or(50);
                            let offset = length.0.saturating_mul(percent) / 100;
                            preview = Some((item, Tick(base.0.saturating_add(offset))));
                        }
                        elements.push((format!("performanceMatch.phrase.{i}.{name}"), r.rect, format!("{label}: {name}")));
                    }
                    let r = ui.checkbox(&mut draft.confirmed[i], "Same musical phrase");
                    elements.push((format!("performanceMatch.phrase.{i}.confirmed"), r.rect, "Same musical phrase".into()));
                });
            }
            ui.weak("If a phrase differs, leave it unchecked and try another candidate. Missing or extra sections require a separate reviewed edit.");
            ui.horizontal(|ui| {
                for (name, tick) in [("In", &mut draft.passage_start), ("Out", &mut draft.passage_end)] {
                    let mut seconds = tick.seconds();
                    let r = ui.add(egui::DragValue::new(&mut seconds).range(0.0..=report.video.duration.seconds().max(0.0)).speed(0.01).prefix(format!("{name}: ")).suffix(" s"));
                    if r.changed() && seconds.is_finite() {
                        *tick = Tick::from_seconds_f64(seconds);
                        draft.boundaries_reviewed = false;
                        draft.confirmed = [false; 3];
                    }
                    elements.push((format!("performanceMatch.passage.{name}"), r.rect, format!("{seconds:.3}")));
                    let can_capture = app.session.state.source_item == Some(report.video.item);
                    let r = ui.add_enabled(can_capture, egui::Button::new(format!("Use Source for {name}")));
                    if r.clicked() {
                        if let Some(relative) = app.session.state.source_playhead.0.checked_sub(report.video.source_in.0)
                            && (0..=report.video.duration.0).contains(&relative)
                        {
                            *tick = Tick(relative);
                            draft.confirmed = [false; 3];
                            draft.boundaries_reviewed = false;
                        } else {
                            draft.status = "Source playhead is outside this live clip. Choose a boundary inside the clip.".into();
                        }
                    }
                    elements.push((format!("performanceMatch.capture.{name}"), r.rect, format!("Use Source for {name}")));
                }
            });
            let r = ui.checkbox(&mut draft.boundaries_reviewed, "I reviewed the exact In/Out and the complete arrangement");
            elements.push(("performanceMatch.boundariesReviewed".into(), r.rect, "Reviewed boundaries and arrangement".into()));
            let r = ui.add_enabled(draft.confirmed.iter().all(|v| *v) && draft.boundaries_reviewed, egui::Button::new("Trim confirmed passage"));
            trim_passage = r.clicked();
            elements.push(("performanceMatch.trimPassage".into(), r.rect, "Trim confirmed passage".into()));
        }
        if let Some(plan) = &mut draft.plan {
            ui.separator();
            let limitation = "FilmCraft cannot verify sung words or mouth movements from this score. Repeated phrases and different arrangements can produce a plausible but wrong match.";
            let r = ui.colored_label(egui::Color32::YELLOW, limitation);
            elements.push(("performanceMatch.limitations".into(), r.rect, limitation.into()));
            let r = ui.checkbox(&mut draft.lyrics_differ, "The performed words or sections differ from the studio song");
            elements.push(("performanceMatch.lyricsDiffer".into(), r.rect, "Performed words or sections differ".into()));
            if draft.lyrics_differ {
                let message = "Changing speed cannot make different sung words or missing sections visually match. Use a take with the right words, or cut to another performer or instrument for those lines.";
                let r = ui.colored_label(egui::Color32::YELLOW, message);
                elements.push(("performanceMatch.lyricsLimit".into(), r.rect, message.into()));
            }
            let speed_outliers = plan.points.windows(2).filter(|pair| {
                let song = pair[1].song.0.checked_sub(pair[0].song.0);
                let live = pair[1].live.0.checked_sub(pair[0].live.0);
                matches!((song, live), (Some(song), Some(live)) if song > 0 && live > 0 && !(0.8..=1.25).contains(&(live as f64 / song as f64)))
            }).count();
            if speed_outliers > 0 {
                let message = format!(
                    "{speed_outliers} timing section(s) require speed outside the 80–125% review band. Motion may look unnatural; check these sections before using the draft."
                );
                let r = ui.colored_label(egui::Color32::YELLOW, &message);
                elements.push(("performanceMatch.speedLimit".into(), r.rect, message));
            }
            if let Some(suggested) = plan.suggested_live_end {
                ui.separator();
                ui.label(format!(
                    "Possible musical ending: {:.2} seconds (similarity {:.0}%).",
                    suggested.seconds(),
                    plan.endpoint_confidence.unwrap_or(0.0) * 100.0
                ));
                if ui.button("Use suggested live ending").clicked() {
                    if plan.points.len() >= 2 && suggested > plan.points[plan.points.len() - 2].live {
                        if let Some(last) = plan.points.last_mut() {
                            last.live = suggested;
                        }
                    } else {
                        draft.status = "Suggested ending overlaps a timing point; review the ending manually.".into();
                    }
                }
                ui.weak("Review the final speed before applying. Original media is unchanged.");
            } else {
                ui.weak("No confident ending detected; trim excess footage or edit the final live timing point.");
            }

            ui.weak("Timing points are seconds from each trimmed clip's start. Edit Live to correct a match. The song endpoint stays fixed.");
            let length = plan.points.len().min(1000);
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                egui::Grid::new("performance-match-points").striped(true).show(ui, |ui| {
                    for label in ["Song", "Live", "Next speed", "Review"] {
                        ui.strong(label);
                    }
                    ui.end_row();
                    for index in 0..length {
                        let next = plan.points.get(index + 1).cloned();
                        let Some(point) = plan.points.get_mut(index) else {
                            continue;
                        };
                        ui.label(format!("{:.2} s", point.song.seconds()));
                        let mut seconds = point.live.seconds();
                        let r = ui.add_enabled(
                            index > 0,
                            egui::DragValue::new(&mut seconds)
                                .speed(0.01)
                                .range(0.0..=plan.video.duration.seconds().clamp(0.0, 600.0))
                                .max_decimals(3)
                                .suffix(" s"),
                        );
                        if r.changed() && seconds.is_finite() {
                            point.live = Tick::from_seconds_f64(seconds);
                        }
                        elements.push((format!("performanceMatch.point.{index}.live"), r.rect, format!("{seconds:.3}")));
                        let speed = next.as_ref().and_then(|p| {
                            let duration = p.song.0.checked_sub(point.song.0)?;
                            let live = p.live.0.checked_sub(point.live.0)?;
                            (duration > 0).then_some(live as f64 / duration as f64)
                        });
                        ui.label(speed.map(|v| format!("{:.0}%", v * 100.0)).unwrap_or_else(|| "End".into()));
                        let check = point.similarity < 0.75 || speed.is_some_and(|v| !(0.8..=1.25).contains(&v));
                        if check {
                            ui.colored_label(egui::Color32::YELLOW, "Check");
                        } else {
                            ui.label("Suggested");
                        }
                        ui.end_row();
                    }
                });
            });
        }
        let r = ui.checkbox(&mut draft.mute_live, "Mute the video's linked live audio when applying");
        elements.push(("performanceMatch.muteLive".into(), r.rect, draft.mute_live.to_string()));
        ui.weak("Apply, play the timeline to check singing and drum hits, then Undo if needed. Original media files are untouched.");
        ui.horizontal(|ui| {
            let r = ui.add_enabled(!draft.running, egui::Button::new("Find passages"));
            find_passages = r.clicked();
            elements.push(("performanceMatch.findPassages".into(), r.rect, "Find passages".into()));
            let r = ui.add_enabled(!draft.running && draft.passages.is_none(), egui::Button::new("Analyze"));
            elements.push(("performanceMatch.analyze".into(), r.rect, "Analyze".into()));
            analyze = r.clicked();
            let r = ui.add_enabled(!draft.running && draft.plan.is_some(), egui::Button::new("Apply draft to timeline"));
            elements.push(("performanceMatch.apply".into(), r.rect, "Apply draft to timeline".into()));
            apply = r.clicked();
            let r = ui.button("Close");
            elements.push(("performanceMatch.close".into(), r.rect, "Close".into()));
            close = r.clicked();
        });
    });
    for (id, rect, label) in elements {
        app.auto.add(&id, rect, &label);
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if analyze {
        start(app, ctx, &mut draft, false);
    }
    if find_passages {
        start(app, ctx, &mut draft, true);
    }
    if let Some((item, time)) = preview {
        let result = app.session.execute("source.open", json!({"item":item.0})).and_then(|_| app.session.execute("source.setPlayhead", json!({"time":time.0})));
        if let Err(e) = result {
            draft.status = e.to_string();
        }
    }
    if trim_passage && let Some(report) = &draft.passages {
        match app.session.execute("clip.trimPerformancePassage", json!({"report":report,"start":draft.passage_start.0,"end":draft.passage_end.0,"phrasesConfirmed":draft.confirmed,"boundariesReviewed":draft.boundaries_reviewed})) {
            Ok(_) => {
                draft.passages = None;
                draft.plan = None;
                draft.status = "Confirmed passage trimmed. Analyze timing next; trimming and matching are separate Undo steps.".into();
            }
            Err(e) => draft.status = e.to_string(),
        }
    }
    if apply && let Some(plan) = &draft.plan {
        match app.session.execute("clip.matchPerformance.apply", json!({"plan":plan,"muteLive":draft.mute_live})) {
            Ok(_) => {
                app.ui.status = "Draft performance match applied. Play to review visible sync; Command-Z undoes the complete match.".into();
                close = true;
            }
            Err(e) => draft.status = e.to_string(),
        }
    }
    if close {
        if let Some(job) = ctx.data(|m| m.get_temp::<Completion>(job_id())) {
            job.cancelled.store(true, Ordering::Relaxed);
        }
        ctx.data_mut(|m| m.remove::<Completion>(job_id()));
        app.ui.performance_match = None;
    } else {
        app.ui.performance_match = Some(draft);
    }
}
