use super::*;
fn fixture() -> (Session, Plan, ClipId) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name":"Music analysis test","video":1,"audio":2})).unwrap();
    let video_item = s.project.items.values().find(|it| it.has_video() && it.has_audio() && it.as_media().is_some()).unwrap().id;
    let song_item = s.project.items.values().find(|it| it.name == "Ambient_Score.wav").unwrap().id;
    let duration = Tick(8 * TICKS_PER_SECOND);
    s.execute("timeline.place", json!({"item":video_item.0,"duration":duration.0})).unwrap();
    let at = s.active_sequence().unwrap().audio_tracks[1].id;
    s.execute("timeline.place", json!({"item":song_item.0,"audioTrack":at.0,"duration":duration.0,"time":0})).unwrap();
    let q = s.active_sequence().unwrap();
    let song = q.audio_tracks[1].items[0].clone();
    // Placement snaps duration to sequence frames; use the actual endpoint.
    let duration = song.duration;
    let video = q.video_tracks[0].items[0].id;
    let plan = Plan {
        sequence: s.state.active_sequence.unwrap(),
        song,
        bpm: 120.0,
        beats: (0..16).map(|i| Tick(i * TICKS_PER_SECOND / 2)).collect(),
        sections: vec![
            Section { start: Tick::ZERO, end: Tick(4 * TICKS_PER_SECOND), label: "Verse".into() },
            Section { start: Tick(4 * TICKS_PER_SECOND), end: duration, label: "Chorus".into() },
        ],
    };
    (s, plan, video)
}
#[test]
fn markers_and_cuts_preserve_audio_and_undo_redo() {
    let (mut s, p, v) = fixture();
    let sequence = p.sequence;
    let before = s.project.clone();
    let audio = s.active_sequence().unwrap().audio_tracks.clone();
    let outcome = apply(&mut s, p, true, true, 4, vec![v]).unwrap();
    assert_eq!(outcome["cuts"], 3);
    assert_eq!(outcome["markers"], 18);
    let q = s.active_sequence().unwrap();
    q.check().unwrap();
    assert_eq!(q.audio_tracks, audio);
    assert_eq!(q.video_tracks[0].items.len(), 4);
    let bytes = filmcraft_format::encode(&s.project, false);
    let restored = filmcraft_format::decode(&bytes).unwrap().project;
    let reopened = restored.sequence(sequence).unwrap();
    assert_eq!(reopened.audio_tracks, audio);
    assert_eq!(reopened.video_tracks, q.video_tracks);
    assert_eq!(reopened.markers, q.markers);
    reopened.check().unwrap();
    let after = s.project.clone();
    s.undo().unwrap();
    assert_eq!(*s.project, *before);
    s.redo().unwrap();
    assert_eq!(*s.project, *after);
}
#[test]
fn analysis_rejects_missing_selected_audio_stream() {
    let (mut s, plan, _) = fixture();
    s.edit_sequence("Select missing song stream", |q, _, _| {
        let (_, song) = q.find_item_mut(plan.song.id).ok_or_else(|| EngineError::Other("song missing".into()))?;
        song.audio_stream = 99;
        Ok(())
    })
    .unwrap();
    assert!(prepare(&s, plan.song.id).is_err());
}
#[test]
fn repeated_apply_replaces_only_generated_markers() {
    let (mut s, p, _) = fixture();
    s.execute("markers.add", json!({"name":"My marker","time":0})).unwrap();
    apply(&mut s, p.clone(), true, true, 0, vec![]).unwrap();
    apply(&mut s, p, true, true, 0, vec![]).unwrap();
    assert_eq!(s.active_sequence().unwrap().markers.len(), 19);
    assert!(s.active_sequence().unwrap().markers.iter().any(|m| m.name == "My marker"));
}
#[test]
fn hostile_and_stale_plans_do_not_change_project() {
    let (mut s, base, v) = fixture();
    let before = s.project.clone();
    let undo = s.history.undo.len();
    for mode in 0..7 {
        let mut p = base.clone();
        match mode {
            0 => p.beats[1] = Tick(-1),
            1 => p.beats[1] = p.beats[0],
            2 => p.bpm = f64::NAN,
            3 => p.sections[0].end = Tick(-1),
            4 => p.song.name.push_str("stale"),
            5 => p.sections[0].label = String::new(),
            _ => p.sequence = ItemId(u64::MAX),
        }
        assert!(apply(&mut s, p, true, true, 4, vec![v]).is_err());
        assert_eq!(*s.project, *before);
        assert_eq!(s.history.undo.len(), undo);
    }
    assert!(apply(&mut s, base, true, true, usize::MAX, vec![v]).is_err());
}
#[test]
fn locked_video_refuses_atomically() {
    let (mut s, p, v) = fixture();
    s.edit_sequence("Lock", |q, _, _| {
        q.video_tracks[0].locked = true;
        Ok(())
    })
    .unwrap();
    let before = s.project.clone();
    assert!(apply(&mut s, p, true, true, 4, vec![v]).is_err());
    assert_eq!(*s.project, *before);
}
#[test]
fn analysis_disabled_without_a_song() {
    let s = Session::default();
    assert!(!s.is_enabled("clip.analyzeMusic"));
}

#[test]
fn opening_cut_counts_from_video_start_when_first_attack_is_missing() {
    for every in [1, 2, 4, 8, 16] {
        for missing in [0, 1, 2] {
            let (_, mut plan, _) = fixture();
            plan.beats = (missing..40).map(|i| Tick(i * TICKS_PER_SECOND / 2)).collect();
            let cuts = grouped_cut_beats(&plan, plan.song.start, every).unwrap();
            let expected: Vec<Tick> = (missing..40).filter(|i| *i > 0 && *i % every as i64 == 0).map(|i| Tick(i * TICKS_PER_SECOND / 2)).collect();
            assert_eq!(cuts, expected, "every={every}, missing={missing}");
        }
    }
}

#[test]
fn first_four_or_eight_beat_video_piece_is_not_one_beat() {
    for every in [4, 8] {
        let (mut session, mut plan, video) = fixture();
        plan.beats.remove(0); // detector omitted the attack at the trimmed beginning
        let rate = session.sequence_rate();
        let expected = rate.snap_nearest(Tick(every as i64 * TICKS_PER_SECOND / 2));
        let before = session.project.clone();
        apply(&mut session, plan, false, false, every, vec![video]).unwrap();
        let sequence = session.active_sequence().unwrap();
        assert_eq!(sequence.video_tracks[0].items[0].duration, expected);
        assert_eq!(sequence.video_tracks[0].items[1].start, expected);
        session.undo().unwrap();
        assert_eq!(*session.project, *before);
    }
}

#[test]
fn beat_grouping_retains_detected_timing_and_ignores_small_opening_jitter() {
    let (_, mut plan, _) = fixture();
    let jitter = Tick(TICKS_PER_SECOND / 100);
    for beat in &mut plan.beats {
        *beat += jitter;
    }
    let cuts = grouped_cut_beats(&plan, plan.song.start, 4).unwrap();
    assert_eq!(cuts[0], Tick(2 * TICKS_PER_SECOND) + jitter);
    assert!(grouped_cut_beats(&plan, plan.song.start, 0).is_err());
    plan.beats.truncate(1);
    assert!(grouped_cut_beats(&plan, plan.song.start, 4).is_err());
}

#[test]
fn clip_anchor_is_independent_of_song_start_and_source_in() {
    let (_, mut plan, _) = fixture();
    plan.song.start = Tick(3 * TICKS_PER_SECOND);
    plan.song.source_in = Tick(11 * TICKS_PER_SECOND);
    for every in [1, 2, 4, 8, 16] {
        for offset in [0, 1, 3, 5] {
            let anchor = Tick(plan.song.start.0 + offset * TICKS_PER_SECOND / 2);
            let cuts = grouped_cut_beats(&plan, anchor, every).unwrap();
            let expected: Vec<_> =
                (offset + 1..16).filter(|i| (i - offset) % every as i64 == 0).map(|i| Tick(plan.song.start.0 + i * TICKS_PER_SECOND / 2)).collect();
            assert_eq!(cuts, expected);
        }
    }
}

#[test]
fn local_tempo_and_small_anchor_jitter_keep_actual_beat_times() {
    let (_, mut plan, _) = fixture();
    // Tempo changes before the selected clip. Use actual later detections.
    plan.song.duration = Tick(16 * TICKS_PER_SECOND);
    plan.beats = (0..10).map(|i| Tick(i * TICKS_PER_SECOND / 2)).chain((0..30).map(|i| Tick(5 * TICKS_PER_SECOND + i * TICKS_PER_SECOND / 4))).collect();
    let anchor = Tick(8 * TICKS_PER_SECOND);
    let expected = Tick(9 * TICKS_PER_SECOND);
    for jitter in [-TICKS_PER_SECOND / 100, 0, TICKS_PER_SECOND / 100] {
        let cuts = grouped_cut_beats(&plan, Tick(anchor.0 + jitter), 4).unwrap();
        assert_eq!(cuts[0], expected);
    }
}

#[test]
fn only_selected_middle_clip_is_cut_and_audio_is_unchanged() {
    let (mut s, mut p, _) = fixture();
    let selected = s
        .edit_sequence("Arrange lead-in and tail", |q, ctx, _| {
            q.settings.frame_rate = filmcraft_time::FrameRate::FPS_24;
            // Keep the complete song and its source offset. Move everything together.
            for tr in q.video_tracks.iter_mut().chain(q.audio_tracks.iter_mut()) {
                for it in &mut tr.items {
                    it.start += Tick(3 * TICKS_PER_SECOND);
                }
            }
            let tr = &mut q.video_tracks[0];
            let selected = filmcraft_edit::split_track_at(tr, Tick(7 * TICKS_PER_SECOND / 2), ctx, &mut HashMap::new()).unwrap();
            filmcraft_edit::split_track_at(tr, Tick(9 * TICKS_PER_SECOND), ctx, &mut HashMap::new()).unwrap();
            Ok(selected)
        })
        .unwrap();
    p.song = s.active_sequence().unwrap().audio_tracks[1].items[0].clone();
    let before = s.project.clone();
    let q = s.active_sequence().unwrap();
    let audio = q.audio_tracks.clone();
    let lead = q.video_tracks[0].items[0].clone();
    let tail = q.video_tracks[0].items[2].clone();
    let outcome = apply(&mut s, p, false, false, 4, vec![selected, selected]).unwrap();
    assert_eq!(outcome["cuts"], 2);
    let q = s.active_sequence().unwrap();
    q.check().unwrap();
    assert_eq!(q.audio_tracks, audio);
    assert_eq!(q.video_tracks[0].items.first().unwrap(), &lead);
    assert_eq!(q.video_tracks[0].items.last().unwrap(), &tail);
    assert_eq!(q.video_tracks[0].items[1].duration, Tick(2 * TICKS_PER_SECOND));
    assert_eq!(q.video_tracks[0].items[2].start, Tick(11 * TICKS_PER_SECOND / 2));
    assert_eq!(q.video_tracks[0].items[3].start, Tick(15 * TICKS_PER_SECOND / 2));
    let after = s.project.clone();
    s.undo().unwrap();
    assert_eq!(*s.project, *before);
    s.redo().unwrap();
    assert_eq!(*s.project, *after);
}

#[test]
fn each_selected_clip_has_its_own_anchor() {
    let (mut s, p, first) = fixture();
    let second = s
        .edit_sequence("Split two beat-one clips", |q, ctx, _| {
            q.settings.frame_rate = filmcraft_time::FrameRate::FPS_24;
            Ok(filmcraft_edit::split_track_at(&mut q.video_tracks[0], Tick(5 * TICKS_PER_SECOND / 2), ctx, &mut HashMap::new()).unwrap())
        })
        .unwrap();
    apply(&mut s, p, false, false, 4, vec![first, second]).unwrap();
    let starts: Vec<_> = s.active_sequence().unwrap().video_tracks[0].items.iter().map(|i| i.start).collect();
    assert_eq!(starts, [0, 4, 5, 9, 13].map(|i| Tick(i * TICKS_PER_SECOND / 2)));
}

#[test]
fn anchor_outside_song_refuses_atomically_and_short_clip_is_untouched() {
    let (mut s, mut p, v) = fixture();
    s.edit_sequence("Offset song", |q, _, _| {
        q.audio_tracks[1].items[0].start = Tick(TICKS_PER_SECOND);
        Ok(())
    })
    .unwrap();
    p.song = s.active_sequence().unwrap().audio_tracks[1].items[0].clone();
    let before = s.project.clone();
    assert!(apply(&mut s, p, true, true, 4, vec![v]).is_err());
    assert_eq!(*s.project, *before);
    let (mut s, p, v) = fixture();
    let before = s.project.clone();
    let result = apply(&mut s, p, false, false, 16, vec![v]).unwrap();
    assert_eq!(result["cuts"], 0);
    assert_eq!(*s.project, *before);
}
