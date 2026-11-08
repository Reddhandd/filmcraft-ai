//! The menu opens a reviewable plan; controls are available to UI automation.
use egui_kittest::Harness;
use filmcraft_engine::{
    Session,
    music_analysis::{Plan, Section},
};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use filmcraft_ui_egui::{FilmcraftApp, menus};
use serde_json::json;

#[test]
fn beat_analysis_menu_opens_review_without_editing_the_song() {
    let mut session = Session::default();
    session.execute("file.openDemoProject", json!({})).unwrap();
    session.execute("file.newSequence", json!({"name":"Beat analysis","video":1,"audio":2})).unwrap();
    let video_item = session.project.items.values().find(|item| item.has_video() && item.has_audio() && item.as_media().is_some()).unwrap().id;
    let song_item = session.project.items.values().find(|item| item.name == "Ambient_Score.wav").unwrap().id;
    session.execute("timeline.place", json!({"item":video_item.0,"duration":8*TICKS_PER_SECOND})).unwrap();
    let song_track = session.active_sequence().unwrap().audio_tracks[1].id;
    session.execute("timeline.place", json!({"item":song_item.0,"audioTrack":song_track.0,"duration":8*TICKS_PER_SECOND})).unwrap();
    let sequence = session.state.active_sequence.unwrap();
    let timeline = session.active_sequence().unwrap();
    let video = timeline.video_tracks[0].items[0].id;
    let song = timeline.audio_tracks[1].items[0].clone();
    let half = Tick(song.duration.0 / 2);
    let end = song.duration;
    session.state.selection = vec![video, song.id];
    let before = session.project.clone();

    let mut app = FilmcraftApp::new(session);
    let result = menus::invoke(&mut app, &egui::Context::default(), "clip.analyzeMusic", json!({})).unwrap();
    assert_eq!(result["dialog"], "musicAnalysis");
    let draft = app.ui.music_analysis.as_mut().unwrap();
    assert_eq!(draft.videos, vec![video]);
    draft.plan = Some(Plan {
        sequence,
        song,
        bpm: 120.0,
        beats: (0..16).map(|n| Tick(n * TICKS_PER_SECOND / 2)).collect(),
        sections: vec![Section { start: Tick::ZERO, end: half, label: "Verse".into() }, Section { start: half, end, label: "Chorus".into() }],
    });

    let mut harness = Harness::builder().with_size(egui::vec2(1300.0, 900.0)).with_max_steps(10000).build_eframe(move |_cc| app);
    for _ in 0..5 {
        harness.step();
    }
    assert_eq!(*harness.state().session.project, *before, "review must not edit the project");
    for id in ["musicAnalysis.analyze", "musicAnalysis.apply", "musicAnalysis.beats", "musicAnalysis.sections", "musicAnalysis.every.4"] {
        assert!(harness.state().auto.find(id).is_some(), "missing control {id}");
    }
}
