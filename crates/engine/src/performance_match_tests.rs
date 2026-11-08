use super::performance_match::{Plan, Point, apply, prepare, selected};
use crate::Session;
use filmcraft_project::{ClipId, ItemId};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::json;

fn frame_pixels(s: &Session, sequence: ItemId, time: Tick) -> Vec<f32> {
    let provider = s.media.full_res_provider(s.project.clone(), s.services.clone());
    let options = filmcraft_render::RenderOptions { scale: 0.25, ..Default::default() };
    filmcraft_render::render_sequence(&s.project, sequence, time, options, &provider).px
}

fn fixture() -> (Session, Plan) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name":"Performance match test","video":1,"audio":2})).unwrap();
    let video_item = s.project.items.values().find(|it| it.has_video() && it.has_audio() && it.as_media().is_some()).unwrap().id;
    let song_item = s.project.items.values().find(|it| it.name == "Ambient_Score.wav").unwrap().id;
    let duration = Tick(4 * TICKS_PER_SECOND);
    s.execute("timeline.place", json!({"item":video_item.0,"duration":duration.0})).unwrap();
    let q = s.active_sequence().unwrap();
    let song_track = q.audio_tracks[1].id;
    s.execute("timeline.place", json!({"item":song_item.0,"audioTrack":song_track.0,"duration":duration.0,"time":TICKS_PER_SECOND})).unwrap();
    let q = s.active_sequence().unwrap();
    let video = q.video_tracks[0].items[0].clone();
    let song = q.audio_tracks[1].items[0].clone();
    let song_duration = song.duration;
    let live_duration = video.duration;
    let plan = Plan {
        sequence: s.state.active_sequence.unwrap(),
        video,
        song,
        similarity: 0.9,
        suggested_live_end: None,
        endpoint_confidence: None,
        points: vec![
            Point { song: Tick::ZERO, live: Tick::ZERO, similarity: 0.9 },
            Point { song: Tick(song_duration.0 / 2), live: Tick(live_duration.0 * 3 / 8), similarity: 0.9 },
            Point { song: song_duration, live: live_duration, similarity: 0.9 },
        ],
    };
    (s, plan)
}

#[test]
fn passage_search_rejects_hostile_identifiers_without_editing() {
    let (mut s, _) = fixture();
    let before = s.project.clone();
    let revision = s.revision;
    for p in [json!({}), json!({"video":-1,"song":17}), json!({"video":1.5,"song":17}), json!({"video":u64::MAX,"song":u64::MAX})] {
        assert!(s.execute("clip.findPerformancePassages", p).is_err());
        assert_eq!(*s.project, *before);
        assert_eq!(s.revision, revision);
    }
}

#[test]
fn passage_search_returns_reviewable_hypotheses_without_an_edit_plan() {
    let (mut s, plan) = fixture();
    let before = s.project.clone();
    let revision = s.revision;
    let result = s.execute("clip.findPerformancePassages", json!({"video":plan.video.id.0,"song":plan.song.id.0})).unwrap();
    assert_eq!(result["requiresReview"], true);
    assert_eq!(result["arrangementVerified"], false);
    assert_eq!(result["boundariesVerified"], false);
    assert!(result.get("points").is_none());
    assert_eq!(*s.project, *before);
    assert_eq!(s.revision, revision);
}

#[test]
fn reviewed_passage_trim_is_atomic_preserves_song_and_undoes_linked_audio() {
    let (mut s, plan) = fixture();
    let report = super::performance_match::find_passages(prepare(&s, plan.video.id, plan.song.id).unwrap()).unwrap();
    let before = s.project.clone();
    let params = json!({"report":report,"start":TICKS_PER_SECOND,"end":3*TICKS_PER_SECOND,"phrasesConfirmed":[true,true,true],"boundariesReviewed":true});
    s.execute("clip.trimPerformancePassage", params.clone()).unwrap();
    let q = s.active_sequence().unwrap();
    let video = q.find_item(plan.video.id).unwrap().1;
    assert_eq!(video.source_in.0, plan.video.source_in.0 + TICKS_PER_SECOND);
    assert_eq!(video.duration.0, 2 * TICKS_PER_SECOND);
    assert_eq!(q.find_item(plan.song.id).unwrap().1, &plan.song);
    for it in q.audio_tracks.iter().flat_map(|t| &t.items).filter(|it| plan.video.link.is_some() && it.link == plan.video.link) {
        assert_eq!(it.source_in, video.source_in);
        assert_eq!(it.duration, video.duration);
    }
    let after = s.project.clone();
    assert!(s.execute("clip.trimPerformancePassage", params).is_err(), "stale trim must fail");
    assert_eq!(*s.project, *after);
    s.undo().unwrap();
    assert_eq!(*s.project, *before);
    s.redo().unwrap();
    assert_eq!(*s.project, *after);
}

#[test]
fn unconfirmed_out_of_range_and_locked_linked_trims_leave_project_unchanged() {
    let (mut s, plan) = fixture();
    let report = super::performance_match::find_passages(prepare(&s, plan.video.id, plan.song.id).unwrap()).unwrap();
    let before = s.project.clone();
    let mut p = json!({"report":report,"start":TICKS_PER_SECOND,"end":3*TICKS_PER_SECOND,"phrasesConfirmed":[true,true,false],"boundariesReviewed":true});
    assert!(s.execute("clip.trimPerformancePassage", p.clone()).is_err());
    assert_eq!(*s.project, *before);
    p["phrasesConfirmed"] = json!([true, true, true]);
    p["end"] = json!(i64::MAX);
    assert!(s.execute("clip.trimPerformancePassage", p.clone()).is_err());
    assert_eq!(*s.project, *before);
    p["end"] = json!(3 * TICKS_PER_SECOND);
    let audio_track = s.active_sequence().unwrap().audio_tracks[0].id;
    s.execute("timeline.setTrack", json!({"track":audio_track.0,"locked":true})).unwrap();
    let locked = s.project.clone();
    assert!(s.execute("clip.trimPerformancePassage", p).is_err());
    assert_eq!(*s.project, *locked);
}

#[test]
fn performance_match_applies_video_only_and_undo_restores_everything() {
    let (mut s, plan) = fixture();
    let before = s.project.clone();
    let song = plan.song.clone();
    apply(&mut s, plan, true).unwrap();
    let q = s.active_sequence().unwrap();
    q.check().unwrap();
    assert_eq!(q.video_tracks[0].items.len(), 2);
    assert_eq!(q.find_item(song.id).unwrap().1, &song);
    assert_eq!(q.video_tracks[0].items[0].start, song.start);
    assert_eq!(q.video_tracks[0].items[0].speed, 0.75);
    assert_eq!(q.video_tracks[0].items[1].speed, 1.25);
    assert!(q.audio_tracks[0].items.iter().all(|it| !it.enabled));
    s.undo().unwrap();
    assert_eq!(*s.project, *before);
}

#[test]
fn performance_match_round_trip_keeps_studio_audio_and_retimed_pieces() {
    let (mut s, plan) = fixture();
    let song = plan.song.clone();
    let sequence = plan.sequence;
    apply(&mut s, plan, true).unwrap();
    let before = s.active_sequence().unwrap().video_tracks[0].items.clone();

    let bytes = filmcraft_format::encode(&s.project, false);
    let loaded = filmcraft_format::decode(&bytes).unwrap().project;
    let restored = loaded.sequence(sequence).unwrap();
    assert_eq!(restored.find_item(song.id).unwrap().1, &song);
    assert_eq!(restored.video_tracks[0].items, before);
    assert_eq!(restored.video_tracks[0].items.len(), 2);
    restored.check().unwrap();
}

#[test]
fn matched_video_nests_and_reopens_without_moving_studio_audio() {
    let (mut s, plan) = fixture();
    let sequence = plan.sequence;
    let song = plan.song.clone();
    apply(&mut s, plan, true).unwrap();
    let pieces = s.active_sequence().unwrap().video_tracks[0].items.clone();
    let before_nest = s.project.clone();
    let samples = [Tick(song.start.0 + song.duration.0 / 4), Tick(song.start.0 + song.duration.0 * 3 / 4)];
    let before_frames: Vec<_> = samples.iter().map(|time| frame_pixels(&s, sequence, *time)).collect();

    let result = s.execute("clip.nest", json!({"name":"Matched performance"})).unwrap();
    let nested = ItemId(result["sequence"].as_u64().unwrap());
    let outer = s.project.sequence(sequence).unwrap();
    assert_eq!(outer.find_item(song.id).unwrap().1, &song);
    let nest_clip = outer.video_tracks.iter().flat_map(|track| &track.items).find(|item| item.item == nested).unwrap();
    assert_eq!(nest_clip.start, song.start);
    assert_eq!(nest_clip.duration, song.duration);

    let inner = s.project.sequence(nested).unwrap();
    let nested_pieces = &inner.video_tracks[0].items;
    assert_eq!(nested_pieces.len(), pieces.len());
    for (inside, original) in nested_pieces.iter().zip(&pieces) {
        assert_eq!(inside.start, Tick(original.start.0 - song.start.0));
        assert_eq!(inside.duration, original.duration);
        assert_eq!(inside.source_in, original.source_in);
        assert_eq!(inside.speed, original.speed);
    }
    inner.check().unwrap();
    outer.check().unwrap();
    for (time, before) in samples.iter().zip(&before_frames) {
        let after = frame_pixels(&s, sequence, *time);
        assert_eq!(after.len(), before.len());
        assert!(after.iter().zip(before).all(|(a, b)| (a - b).abs() <= 1e-4), "nest changed the rendered matched video at {time:?}");
    }

    let bytes = filmcraft_format::encode(&s.project, false);
    let reopened = filmcraft_format::decode(&bytes).unwrap().project;
    assert_eq!(reopened.sequence(sequence).unwrap().find_item(song.id).unwrap().1, &song);
    assert_eq!(reopened.sequence(nested).unwrap().video_tracks[0].items, *nested_pieces);

    let after_nest = s.project.clone();
    s.undo().unwrap();
    assert_eq!(*s.project, *before_nest);
    s.redo().unwrap();
    assert_eq!(*s.project, *after_nest);
}

#[test]
fn matched_video_consolidates_without_changing_retiming_or_studio_audio() {
    let (mut s, plan) = fixture();
    let sequence = plan.sequence;
    let song = plan.song.clone();
    apply(&mut s, plan, true).unwrap();
    let pieces = s.active_sequence().unwrap().video_tracks[0].items.clone();
    let before = s.project.clone();

    let result = s.execute("clip.consolidateActive", json!({})).unwrap();
    assert_eq!(result["kept"], pieces.len());
    let q = s.active_sequence().unwrap();
    assert_eq!(q.video_tracks[0].items, pieces);
    assert_eq!(q.find_item(song.id).unwrap().1, &song);
    q.check().unwrap();

    let bytes = filmcraft_format::encode(&s.project, false);
    let reopened = filmcraft_format::decode(&bytes).unwrap().project;
    assert_eq!(reopened.sequence(sequence).unwrap().video_tracks[0].items, pieces);
    assert_eq!(reopened.sequence(sequence).unwrap().find_item(song.id).unwrap().1, &song);
    s.undo().unwrap();
    assert_eq!(*s.project, *before);
}

#[test]
fn performance_match_rejects_hostile_plan_atomically() {
    let (mut s, base) = fixture();
    let before = s.project.clone();
    let undo = s.history.undo.len();
    for mode in 0..6 {
        let mut plan = base.clone();
        match mode {
            0 => plan.points[1].live = Tick(-1),
            1 => plan.points[1].song = Tick::ZERO,
            2 => plan.points[1].live = Tick(1),
            3 => plan.video.duration = Tick(i64::MAX),
            4 => plan.similarity = f32::NAN,
            _ => plan.points.clear(),
        }
        assert!(apply(&mut s, plan, true).is_err());
        assert_eq!(*s.project, *before);
        assert_eq!(s.history.undo.len(), undo);
    }
    assert!(prepare(&s, ClipId(u64::MAX), base.song.id).is_err());
}

#[test]
fn performance_match_refuses_stale_or_locked_clips() {
    let (mut s, plan) = fixture();
    s.edit_sequence("Lock", |q, _, _| {
        q.video_tracks[0].locked = true;
        Ok(())
    })
    .unwrap();
    let before = s.project.clone();
    assert!(apply(&mut s, plan.clone(), true).is_err());
    assert_eq!(*s.project, *before);
    s.edit_sequence("Unlock and trim", |q, _, _| {
        q.video_tracks[0].locked = false;
        q.video_tracks[0].items[0].source_in = Tick(1);
        Ok(())
    })
    .unwrap();
    let before = s.project.clone();
    assert!(apply(&mut s, plan, true).is_err());
    assert_eq!(*s.project, *before);
}

#[test]
fn performance_match_rejects_missing_selected_audio_stream() {
    let (mut s, plan) = fixture();
    s.edit_sequence("Select missing audio stream", |q, _, _| {
        let (_, song) = q.find_item_mut(plan.song.id).ok_or_else(|| crate::EngineError::Other("song missing".into()))?;
        song.audio_stream = 99;
        Ok(())
    })
    .unwrap();
    let before = s.project.clone();
    assert!(prepare(&s, plan.video.id, plan.song.id).is_err());
    assert_eq!(*s.project, *before);
}

#[test]
fn performance_match_command_rejects_stale_revision_and_sequence() {
    let (mut s, plan) = fixture();
    let before = s.project.clone();
    let revision = s.revision;
    let sequence = s.state.active_sequence.unwrap().0;
    for params in [
        json!({"plan":plan,"expectedRevision":revision + 1,"sequenceId":sequence}),
        json!({"plan":plan,"expectedRevision":revision,"sequenceId":sequence + 1}),
        json!({"plan":plan,"expectedRevision":"bad","sequenceId":sequence}),
    ] {
        assert!(s.execute("clip.matchPerformance.apply", params).is_err());
        assert_eq!(*s.project, *before);
    }
}

#[test]
fn performance_match_selection_ignores_linked_live_audio() {
    let (mut s, plan) = fixture();
    let q = s.active_sequence().unwrap();
    s.state.selection = vec![plan.video.id, q.audio_tracks[0].items[0].id, plan.song.id];
    assert_eq!(selected(&s).unwrap(), (plan.video.id, plan.song.id));
}

#[test]
fn performance_match_refuses_to_overwrite_the_next_video() {
    let (mut s, plan) = fixture();
    s.execute("timeline.place", json!({"item":plan.video.item.0,"time":4*TICKS_PER_SECOND,"duration":2*TICKS_PER_SECOND})).unwrap();
    let before = s.project.clone();
    assert!(apply(&mut s, plan, true).is_err());
    assert_eq!(*s.project, *before);
}

#[test]
fn performance_match_analysis_uses_exact_trimmed_endpoints() {
    let (mut s, source) = fixture();
    let plan = super::performance_match::analyze(prepare(&s, source.video.id, source.song.id).unwrap()).unwrap();
    assert_eq!(plan.points.last().unwrap().song, source.song.duration);
    assert_eq!(plan.points.last().unwrap().live, source.video.duration);
    let before = s.project.clone();
    apply(&mut s, plan, true).unwrap();
    s.active_sequence().unwrap().check().unwrap();
    s.undo().unwrap();
    assert_eq!(*s.project, *before);
}
