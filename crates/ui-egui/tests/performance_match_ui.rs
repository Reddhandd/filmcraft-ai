//! Headless review-window test; optional offscreen screenshot for visual QA.
use egui_kittest::Harness;
use filmcraft_engine::{
    Session,
    performance_match::{Plan, Point},
};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use filmcraft_ui_egui::{FilmcraftApp, panels::performance_match};
use serde_json::json;

fn click(h: &mut Harness<'_, FilmcraftApp>, id: &str) {
    let r = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}")).rect;
    let pos = egui::pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    for pressed in [true, false] {
        h.input_mut().events.extend([
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE },
        ]);
        h.step();
    }
    for _ in 0..3 {
        h.step();
    }
}

#[test]
fn trim_warning_precedes_analysis_and_can_be_suppressed() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name":"Warning review","video":1,"audio":2})).unwrap();
    let video_item = s.project.items.values().find(|it| it.has_video() && it.has_audio() && it.as_media().is_some()).unwrap().id;
    let song_item = s.project.items.values().find(|it| it.name == "Ambient_Score.wav").unwrap().id;
    s.execute("timeline.place", json!({"item":video_item.0,"duration":4*TICKS_PER_SECOND})).unwrap();
    let song_track = s.active_sequence().unwrap().audio_tracks[1].id;
    s.execute("timeline.place", json!({"item":song_item.0,"audioTrack":song_track.0,"duration":4*TICKS_PER_SECOND})).unwrap();
    let q = s.active_sequence().unwrap();
    s.state.selection = vec![q.video_tracks[0].items[0].id, q.audio_tracks[1].items[0].id];
    let before = s.project.clone();
    let mut app = FilmcraftApp::new(s);
    performance_match::open(&mut app).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1300.0, 900.0)).with_max_steps(10000).build_eframe(move |_cc| app);
    for _ in 0..5 {
        h.step();
    }
    assert!(h.state().auto.find("performanceMatch.trimWarning.continue").is_some());
    assert!(h.state().auto.find("performanceMatch.analyze").is_none());
    click(&mut h, "performanceMatch.trimWarning.dontShowAgain");
    click(&mut h, "performanceMatch.trimWarning.continue");
    assert!(!h.state().session.prefs.general.show_performance_match_trim_warning);
    assert!(h.state().auto.find("performanceMatch.analyze").is_some());
    assert_eq!(*h.state().session.project, *before);
    click(&mut h, "performanceMatch.close");
    performance_match::open(h.state_mut()).unwrap();
    h.step();
    assert!(h.state().auto.find("performanceMatch.trimWarning.continue").is_none());
    assert!(h.state().auto.find("performanceMatch.analyze").is_some());
}

#[test]
fn passage_review_previews_without_edits_and_requires_confirmation_before_trim() {
    use filmcraft_engine::performance_match::{PassageCandidate, find_passages, prepare};
    let mut s = Session::default();
    s.prefs.general.show_performance_match_trim_warning = false;
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name":"Passage review","video":1,"audio":2})).unwrap();
    let item = s.project.items.values().find(|it| it.has_video() && it.has_audio() && it.as_media().is_some()).unwrap().id;
    let song_item = s.project.items.values().find(|it| it.name == "Ambient_Score.wav").unwrap().id;
    s.execute("timeline.place", json!({"item":item.0,"duration":4*TICKS_PER_SECOND})).unwrap();
    let song_track = s.active_sequence().unwrap().audio_tracks[1].id;
    s.execute("timeline.place", json!({"item":song_item.0,"audioTrack":song_track.0,"duration":4*TICKS_PER_SECOND})).unwrap();
    let q = s.active_sequence().unwrap();
    let video = q.video_tracks[0].items[0].id;
    let song = q.audio_tracks[1].items[0].id;
    s.state.selection = vec![video, song];
    let mut report = find_passages(prepare(&s, video, song).unwrap()).unwrap();
    report.candidates = vec![PassageCandidate { live_start: Tick(TICKS_PER_SECOND), live_end: Tick(3 * TICKS_PER_SECOND), ranking_score: 0.9 }];
    let before = s.project.clone();
    let mut app = FilmcraftApp::new(s);
    performance_match::open(&mut app).unwrap();
    let draft = app.ui.performance_match.as_mut().unwrap();
    draft.passages = Some(report);
    draft.passage_start = Tick(TICKS_PER_SECOND);
    draft.passage_end = Tick(3 * TICKS_PER_SECOND);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1100.0)).with_max_steps(10000).build_eframe(move |_cc| app);
    for _ in 0..5 {
        h.step();
    }
    for id in [
        "performanceMatch.findPassages",
        "performanceMatch.candidate.0",
        "performanceMatch.phrase.0.Studio",
        "performanceMatch.phrase.2.confirmed",
        "performanceMatch.capture.In",
        "performanceMatch.trimPassage",
    ] {
        assert!(h.state().auto.find(id).is_some(), "missing {id}");
    }
    click(&mut h, "performanceMatch.trimPassage");
    assert_eq!(*h.state().session.project, *before);
    click(&mut h, "performanceMatch.phrase.1.Studio");
    assert_eq!(h.state().session.state.source_item, Some(song_item));
    click(&mut h, "performanceMatch.phrase.1.Live");
    assert_eq!(h.state().session.state.source_item, Some(item));
    assert_eq!(*h.state().session.project, *before, "phrase preview must not edit the project");
    {
        let draft = h.state_mut().ui.performance_match.as_mut().unwrap();
        draft.confirmed = [true; 3];
        draft.boundaries_reviewed = true;
    }
    for _ in 0..3 {
        h.step();
    }
    click(&mut h, "performanceMatch.trimPassage");
    let q = h.state().session.active_sequence().unwrap();
    assert_eq!(q.find_item(video).unwrap().1.duration, Tick(2 * TICKS_PER_SECOND));
    assert!(h.state().ui.performance_match.as_ref().unwrap().passages.is_none());
    h.state_mut().session.undo().unwrap();
    assert_eq!(*h.state().session.project, *before);
}

#[test]
fn performance_match_review_window_renders_and_exposes_automation() {
    let mut s = Session::default();
    s.prefs.general.show_performance_match_trim_warning = false;
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name":"Performance match","video":1,"audio":2})).unwrap();
    let video_item = s.project.items.values().find(|it| it.has_video() && it.has_audio() && it.as_media().is_some()).unwrap().id;
    let song_item = s.project.items.values().find(|it| it.name == "Ambient_Score.wav").unwrap().id;
    s.execute("timeline.place", json!({"item":video_item.0,"duration":4*TICKS_PER_SECOND})).unwrap();
    let song_track = s.active_sequence().unwrap().audio_tracks[1].id;
    s.execute("timeline.place", json!({"item":song_item.0,"audioTrack":song_track.0,"duration":4*TICKS_PER_SECOND})).unwrap();
    let q = s.active_sequence().unwrap();
    let video = q.video_tracks[0].items[0].clone();
    let song = q.audio_tracks[1].items[0].clone();
    s.state.selection = vec![video.id, song.id];
    let song_duration = song.duration;
    let live_duration = video.duration;
    let sequence = s.state.active_sequence.unwrap();
    let before = s.project.clone();
    let mut app = FilmcraftApp::new(s);
    performance_match::open(&mut app).unwrap();
    app.ui.performance_match.as_mut().unwrap().plan = Some(Plan {
        sequence,
        video,
        song,
        similarity: 0.96,
        suggested_live_end: None,
        endpoint_confidence: None,
        points: vec![
            Point { song: Tick::ZERO, live: Tick::ZERO, similarity: 0.9 },
            Point { song: Tick(song_duration.0 / 2), live: Tick(live_duration.0 * 20 / 100), similarity: 0.7 },
            Point { song: song_duration, live: live_duration, similarity: 0.9 },
        ],
    });
    app.ui.performance_match.as_mut().unwrap().status = "Audio-pattern ranking score 0.960. Visible performance sync is not verified.".into();
    let snapshot = std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR");
    let mut builder = Harness::builder().with_size(egui::vec2(1300.0, 900.0)).with_max_steps(10000);
    if snapshot.is_some() {
        builder = builder.wgpu();
    }
    let mut harness = builder.build_eframe(move |_cc| app);
    for _ in 0..5 {
        harness.step();
    }
    assert_eq!(*harness.state().session.project, *before, "Opening a suggested match never edits the document");
    for id in [
        "performanceMatch.analyze",
        "performanceMatch.apply",
        "performanceMatch.point.1.live",
        "performanceMatch.muteLive",
        "performanceMatch.limitations",
        "performanceMatch.lyricsDiffer",
        "performanceMatch.speedLimit",
    ] {
        assert!(harness.state().auto.find(id).is_some(), "missing interactive control {id}");
    }
    click(&mut harness, "performanceMatch.lyricsDiffer");
    assert!(harness.state().ui.performance_match.as_ref().unwrap().lyrics_differ);
    assert!(harness.state().auto.find("performanceMatch.lyricsLimit").is_some());
    assert_eq!(*harness.state().session.project, *before, "acknowledging a lyric difference must not edit the project");
    let state = serde_json::to_value(&harness.state().ui).unwrap();
    assert!(state["performance_match"]["plan"].is_object());
    if let Some(dir) = snapshot {
        std::fs::create_dir_all(&dir).unwrap();
        let image = harness.render().unwrap();
        image.save(std::path::Path::new(&dir).join("performance-match-review.png")).unwrap();
    }
}
