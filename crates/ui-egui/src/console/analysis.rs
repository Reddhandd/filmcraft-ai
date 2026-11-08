//! Bounded native read-only analysis jobs. Results belong to one project revision.
use super::*;
use filmcraft_engine::{music_analysis, performance_match};
use filmcraft_project::ClipId;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
#[derive(Clone, Debug)]
// Native workers construct these; web requests return an unsupported error.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum Artifact {
    Music(Box<music_analysis::Plan>),
    Performance(Box<performance_match::Plan>),
    Passages(Box<performance_match::PassageSearch>),
}
#[derive(Clone, Debug)]
struct Job {
    created: Instant,
    project: Arc<filmcraft_project::Project>,
    revision: u64,
    epoch: u64,
    sequence: Option<filmcraft_project::ItemId>,
    result: Arc<Mutex<Option<Result<Artifact, String>>>>,
}
#[derive(Clone, Debug, Default)]
pub(super) struct State {
    jobs: std::collections::BTreeMap<String, Job>,
}
impl Job {
    fn check(&self, session: &filmcraft_engine::Session) -> Result<(), String> {
        if self.created.elapsed() > Duration::from_secs(600)
            || self.revision != session.revision
            || self.epoch != session.document_epoch
            || self.sequence != session.state.active_sequence
            || !Arc::ptr_eq(&self.project, &session.project)
        {
            return Err("Analysis is stale; inspect and analyze the current project again".into());
        }
        Ok(())
    }
}
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum Input {
    Music(Box<music_analysis::Input>),
    Performance(Box<performance_match::Input>),
    Passages(Box<performance_match::Input>),
}
fn clip(p: &serde_json::Map<String, Value>, name: &str) -> Result<ClipId, String> {
    p.get(name).and_then(Value::as_u64).filter(|id| *id > 0).map(ClipId).ok_or_else(|| format!("Missing or invalid {name} clip ID"))
}
pub(super) fn start(app: &mut FilmcraftApp, p: &serde_json::Map<String, Value>) -> Result<Value, String> {
    if app.ui.console.connection != ConnectionStatus::Ready || !app.ui.console.chat.busy {
        return Err("A live console turn is required for analysis".into());
    }
    let state = &mut app.ui.console.analysis;
    if state.jobs.values().any(|j| j.result.lock().unwrap_or_else(PoisonError::into_inner).is_none()) {
        return Err("A console analysis is already running; wait for it to finish".into());
    }
    state.jobs.retain(|_, j| j.created.elapsed() < Duration::from_secs(600));
    if state.jobs.len() >= 8 {
        let oldest = state.jobs.iter().min_by_key(|(_, j)| j.created).map(|(id, _)| id.clone());
        if let Some(id) = oldest {
            state.jobs.remove(&id);
        }
    }
    let song = clip(p, "song")?;
    let input = match p.get("kind").and_then(Value::as_str) {
        Some("music") if !p.contains_key("video") => Input::Music(Box::new(music_analysis::prepare(&app.session, song).map_err(|e| e.to_string())?)),
        Some("passages") => Input::Passages(Box::new(performance_match::prepare(&app.session, clip(p, "video")?, song).map_err(|e| e.to_string())?)),
        Some("performance") => Input::Performance(Box::new(performance_match::prepare(&app.session, clip(p, "video")?, song).map_err(|e| e.to_string())?)),
        _ => return Err("Choose music (song only), performance or passages (video and song) analysis".into()),
    };
    #[cfg(target_arch = "wasm32")]
    {
        let _ = input;
        Err("Console analysis requires the desktop app".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let id = super::bridge::ticket_id()?;
        let result = Arc::new(Mutex::new(None));
        let worker = Arc::clone(&result);
        std::thread::Builder::new()
            .name("console-analysis".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match input {
                    Input::Music(input) => music_analysis::analyze(*input).map(|plan| Artifact::Music(Box::new(plan))),
                    Input::Performance(input) => performance_match::analyze(*input).map(|plan| Artifact::Performance(Box::new(plan))),
                    Input::Passages(input) => performance_match::find_passages(*input).map(|report| Artifact::Passages(Box::new(report))),
                }))
                .map_err(|_| "Analysis failed internally; the project was not changed".to_owned())
                .and_then(|r| r.map_err(|e| e.to_string()));
                *worker.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
            })
            .map_err(|e| format!("Cannot start console analysis: {e}"))?;
        state.jobs.insert(
            id.clone(),
            Job {
                created: Instant::now(),
                project: Arc::clone(&app.session.project),
                revision: app.session.revision,
                epoch: app.session.document_epoch,
                sequence: app.session.state.active_sequence,
                result,
            },
        );
        Ok(json!({"analysisId":id,"state":"pending"}))
    }
}
pub(super) fn read(app: &FilmcraftApp, p: &serde_json::Map<String, Value>) -> Result<Value, String> {
    let id = p.get("analysisId").and_then(Value::as_str).filter(|s| s.len() <= 80).ok_or("Invalid analysis ID")?;
    let offset = match p.get("offset") {
        None => 0,
        Some(v) => v.as_u64().filter(|n| *n <= 2400).ok_or("Invalid analysis offset")? as usize,
    };
    let job = app.ui.console.analysis.jobs.get(id).ok_or("Unknown or expired analysis")?;
    job.check(&app.session)?;
    let guard = job.result.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(result) = guard.as_ref() else {
        return Ok(json!({"state":"pending","analysisId":id}));
    };
    let artifact = result.as_ref().map_err(Clone::clone)?;
    Ok(match artifact {
        Artifact::Music(plan) => {
            let beats = plan
                .beats
                .iter()
                .skip(offset)
                .take(128)
                .map(|t| json!({"relative":t.0,"source":plan.song.source_in.0.checked_add(t.0),"timeline":plan.song.start.0.checked_add(t.0)}))
                .collect::<Vec<_>>();
            json!({"state":"done","analysisId":id,"kind":"music","bpm":plan.bpm,"beatCount":plan.beats.len(),"offset":offset,"beats":beats,"nextOffset":(offset.saturating_add(128)<plan.beats.len()).then_some(offset.saturating_add(128)),"sections":plan.sections,"timing":"Exact Tick units; sections and relative beats are relative to the trimmed song. Detected beats do not certify downbeats or first audible sound.","downbeatVerified":false,"firstAudibleSound":null,"confidence":null,"revision":job.revision})
        }
        Artifact::Passages(report) => {
            json!({"state":"done","analysisId":id,"kind":"passages","report":report,"revision":job.revision,"timing":"liveStart/liveEnd are relative Tick offsets from the trimmed live clip source_in; review corresponding start, middle and end phrases and exact boundaries before trimming.","reviewRequired":true})
        }
        Artifact::Performance(plan) => {
            json!({"state":"done","analysisId":id,"kind":"performance","similarity":plan.similarity,"points":plan.points,"timing":{"units":"Tick","songSourceOrigin":plan.song.source_in,"liveSourceOrigin":plan.video.source_in,"songTimelineOrigin":plan.song.start,"liveTimelineOrigin":plan.video.start,"note":"Points and suggestedLiveEnd are relative to trimmed clip source origins."},"suggestedLiveEnd":plan.suggested_live_end,"endpointConfidence":plan.endpoint_confidence,"reviewRequired":true,"revision":job.revision,"note":"Candidate alignment only. Verify corresponding musical passages, start/end and drift before proposing edits."})
        }
    })
}

pub(super) fn schema(command: &str) -> Option<&'static str> {
    match command {
        "clip.analyzeMusic.apply" => Some(
            r#"{"analysisId":string,"beatMarkers":bool,"sectionMarkers":bool,"every":0|1|2|4|8|16,"videos":[clipId],"sections"?:[{"start":ticks,"end":ticks,"label":string}]}"#,
        ),
        "clip.matchPerformance.apply" => Some(r#"{"analysisId":string,"muteLive":bool,"points"?:[{"song":ticks,"live":ticks,"similarity":number}]}"#),
        "clip.trimPerformancePassage" => {
            Some(r#"{"analysisId":string,"start":ticks,"end":ticks,"phrasesConfirmed":[true,true,true],"boundariesReviewed":true}"#)
        }
        _ => None,
    }
}
pub(super) fn proposal(app: &FilmcraftApp, id: Value, command: &str, mut params: Value) -> Result<chat::Proposal, String> {
    let p = params.as_object_mut().ok_or("Analysis application parameters must be an object")?;
    let allowed: &[&str] = match command {
        "clip.analyzeMusic.apply" => &["analysisId", "beatMarkers", "sectionMarkers", "every", "videos", "sections"],
        "clip.matchPerformance.apply" => &["analysisId", "muteLive", "points"],
        "clip.trimPerformancePassage" => &["analysisId", "start", "end", "phrasesConfirmed", "boundariesReviewed"],
        _ => return Err("Unsupported analysis application".into()),
    };
    if p.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("Unexpected analysis application parameter; never supply a plan".into());
    }
    let analysis = p.remove("analysisId").and_then(|v| v.as_str().map(str::to_owned)).filter(|s| s.len() <= 80).ok_or("Missing analysisId")?;
    let job = app.ui.console.analysis.jobs.get(&analysis).ok_or("Unknown or expired analysis")?;
    job.check(&app.session)?;
    let guard = job.result.lock().unwrap_or_else(PoisonError::into_inner);
    let result = guard.as_ref().ok_or("Analysis is still running")?.as_ref().map_err(Clone::clone)?;
    let plan = match (command, result) {
        ("clip.analyzeMusic.apply", Artifact::Music(plan)) => {
            for key in ["beatMarkers", "sectionMarkers"] {
                if p.get(key).and_then(Value::as_bool).is_none() {
                    return Err(format!("Explicit {key} boolean required"));
                }
            }
            if !p.get("every").and_then(Value::as_u64).is_some_and(|n| [0, 1, 2, 4, 8, 16].contains(&n))
                || !p.get("videos").and_then(Value::as_array).is_some_and(|v| v.len() <= 64 && v.iter().all(|id| id.as_u64().is_some_and(|n| n > 0)))
            {
                return Err("Choose a supported cut interval and at most 64 explicit video clip IDs".into());
            }
            let mut plan = (**plan).clone();
            if let Some(sections) = p.remove("sections") {
                if !sections.as_array().is_some_and(|v| !v.is_empty() && v.len() <= 64) {
                    return Err("Choose 1–64 sections".into());
                }
                plan.sections = serde_json::from_value(sections).map_err(|e| format!("Invalid sections: {e}"))?;
            }
            serde_json::to_value(plan).map_err(|e| e.to_string())?
        }
        ("clip.matchPerformance.apply", Artifact::Performance(plan)) => {
            if p.get("muteLive").and_then(Value::as_bool).is_none() {
                return Err("Explicit muteLive boolean required".into());
            }
            let mut plan = (**plan).clone();
            if let Some(points) = p.remove("points") {
                if !points.as_array().is_some_and(|v| (2..=1000).contains(&v.len())) {
                    return Err("Choose 2–1000 timing anchors".into());
                }
                plan.points = serde_json::from_value(points).map_err(|e| format!("Invalid timing anchors: {e}"))?;
            }
            serde_json::to_value(plan).map_err(|e| e.to_string())?
        }
        ("clip.trimPerformancePassage", Artifact::Passages(report)) => {
            p.insert("report".into(), serde_json::to_value(report).map_err(|e| e.to_string())?);
            return chat::Proposal::prepare_owned(&app.session, id, command.into(), params);
        }
        _ => return Err("Analysis kind does not match the requested edit".into()),
    };
    p.insert("plan".into(), plan);
    chat::Proposal::prepare_owned(&app.session, id, command.into(), params)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> FilmcraftApp {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.execute("file.newSequence", json!({"width":16,"height":16})).unwrap();
        app
    }
    fn cache(app: &mut FilmcraftApp) {
        let song=serde_json::from_value(json!({"id":99,"item":98,"name":"Synthetic","label":"Violet","start":1000,"duration":10000,"source_in":2000,"speed":1.0,"enabled":true,"effects":[]})).unwrap();
        let plan = music_analysis::Plan {
            sequence: app.session.state.active_sequence.unwrap(),
            song,
            bpm: 120.0,
            beats: (0..260).map(|i| filmcraft_time::Tick(i * 10)).collect(),
            sections: vec![],
        };
        app.ui.console.analysis.jobs.insert(
            "test".into(),
            Job {
                created: Instant::now(),
                project: Arc::clone(&app.session.project),
                revision: app.session.revision,
                epoch: app.session.document_epoch,
                sequence: app.session.state.active_sequence,
                result: Arc::new(Mutex::new(Some(Ok(Artifact::Music(Box::new(plan)))))),
            },
        );
    }
    #[test]
    fn owned_passage_trim_requires_review_and_uses_native_undo() {
        let mut app = app();
        app.session.execute("file.openDemoProject", json!({})).unwrap();
        app.session.execute("file.newSequence", json!({"video":1,"audio":2})).unwrap();
        let video_item = app.session.project.items.values().find(|it| it.has_video() && it.has_audio() && it.as_media().is_some()).unwrap().id;
        let song_item = app.session.project.items.values().find(|it| it.name == "Ambient_Score.wav").unwrap().id;
        let second = filmcraft_time::TICKS_PER_SECOND;
        app.session.execute("timeline.place", json!({"item":video_item.0,"duration":4*second})).unwrap();
        let audio_track = app.session.active_sequence().unwrap().audio_tracks[1].id;
        app.session.execute("timeline.place", json!({"item":song_item.0,"audioTrack":audio_track.0,"duration":4*second})).unwrap();
        let q = app.session.active_sequence().unwrap();
        let video = q.video_tracks[0].items[0].clone();
        let song = q.audio_tracks[1].items[0].clone();
        let report = performance_match::PassageSearch {
            sequence: app.session.state.active_sequence.unwrap(),
            video: video.clone(),
            song,
            resolution: filmcraft_time::Tick(second),
            candidates: vec![],
            requires_review: true,
            arrangement_verified: false,
            boundaries_verified: false,
        };
        app.ui.console.analysis.jobs.insert(
            "passages".into(),
            Job {
                created: Instant::now(),
                project: Arc::clone(&app.session.project),
                revision: app.session.revision,
                epoch: app.session.document_epoch,
                sequence: app.session.state.active_sequence,
                result: Arc::new(Mutex::new(Some(Ok(Artifact::Passages(Box::new(report)))))),
            },
        );
        let before = Arc::clone(&app.session.project);
        let mut p = json!({"analysisId":"passages","start":second,"end":3*second,"phrasesConfirmed":[true,true,false],"boundariesReviewed":true});
        assert!(proposal(&app, Value::Null, "clip.trimPerformancePassage", p.clone()).is_err());
        assert!(Arc::ptr_eq(&before, &app.session.project));
        p["phrasesConfirmed"] = json!([true, true, true]);
        let prepared = proposal(&app, Value::Null, "clip.trimPerformancePassage", p).unwrap();
        prepared.apply(&mut app, &egui::Context::default()).unwrap();
        assert_eq!(app.session.active_sequence().unwrap().find_item(video.id).unwrap().1.duration.0, 2 * second);
        app.session.undo().unwrap();
        assert_eq!(*before, *app.session.project);
        assert!(read(&app, json!({"analysisId":"passages"}).as_object().unwrap()).is_err());
    }
    #[test]
    fn beat_pages_preserve_coordinate_systems_and_reject_stale_results() {
        let mut app = app();
        cache(&mut app);
        let page = read(&app, json!({"analysisId":"test","offset":128}).as_object().unwrap()).unwrap();
        assert_eq!(page["beats"].as_array().unwrap().len(), 128);
        assert_eq!(page["beats"][0], json!({"relative":1280,"source":3280,"timeline":2280}));
        assert_eq!(page["nextOffset"], 256);
        assert_eq!(page["downbeatVerified"], false);
        assert!(page["firstAudibleSound"].is_null());
        assert!(read(&app, json!({"analysisId":"test","offset":-1}).as_object().unwrap()).is_err());
        app.session.revision += 1;
        assert!(read(&app, json!({"analysisId":"test"}).as_object().unwrap()).is_err());
    }
    #[test]
    fn hostile_analysis_and_forged_plans_never_mutate() {
        let mut app = app();
        cache(&mut app);
        app.ui.console.connection = ConnectionStatus::Ready;
        app.ui.console.chat.busy = true;
        let before = Arc::clone(&app.session.project);
        for params in [json!({}), json!({"kind":"music","song":-1}), json!({"kind":"shell","song":1}), json!({"kind":"music","song":1,"video":2})] {
            assert!(start(&mut app, params.as_object().unwrap()).is_err());
        }
        assert!(proposal(&app, json!(1), "clip.analyzeMusic.apply", json!({"analysisId":"test","plan":{}})).is_err());
        assert!(chat::Proposal::prepare(&app.session, json!(1), "clip.analyzeMusic.apply".into(), json!({"plan":{}})).is_err());
        assert!(proposal(&app, json!(1), "clip.matchPerformance.apply", json!({"analysisId":"test","muteLive":true})).is_err());
        assert_eq!(*before, *app.session.project);
    }
}
