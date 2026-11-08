//! `timeline.place` with `items` and `file.newSequence` with `fromItems`: several items back to
//! back, as one undo step.

use filmcraft_project::ItemId;
use serde_json::json;

use crate::Session;

/// A session on an empty sequence, and the ids of three media items with picture and sound.
fn three() -> (Session, Vec<ItemId>) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let items: Vec<ItemId> = s.project.items.values().filter(|i| i.has_video() && i.has_audio() && i.as_media().is_some()).map(|i| i.id).take(3).collect();
    assert!(items.len() >= 2, "the demo project has media to place");
    s.execute("file.newSequence", json!({"name": "Back to back", "video": 2, "audio": 2})).unwrap();
    (s, items)
}

fn ends(s: &Session) -> Vec<(i64, i64)> {
    let q = s.active_sequence().unwrap();
    q.video_tracks[0].items.iter().map(|i| (i.start.0, i.end().0)).collect()
}

/// Dragging several selected items placed them one by one, each one an undo step.
#[test]
fn items_land_back_to_back_in_one_undo_step() {
    let (mut s, items) = three();
    let steps = s.history.undo.len();
    let r = s.execute("timeline.place", json!({"items": items.iter().map(|i| i.0).collect::<Vec<_>>(), "time": 0})).unwrap();
    assert_eq!(r["placed"].as_array().unwrap().len(), items.len());
    let spans = ends(&s);
    assert_eq!(spans.len(), items.len());
    assert_eq!(spans[0].0, 0);
    for w in spans.windows(2) {
        assert_eq!(w[0].1, w[1].0, "each item starts where the one before ends");
    }
    assert_eq!(s.history.undo.len(), steps + 1, "one undo step for all of them");
    assert_eq!(s.history.undo.last().map(|u| u.0.as_str()), Some("Place Clips"));
    s.undo();
    assert!(ends(&s).is_empty(), "one undo takes them all off");
    s.redo();
    assert_eq!(ends(&s), spans);
}

/// One bad item in the list leaves the sequence and the history as they were.
#[test]
fn a_failing_item_places_nothing() {
    let (mut s, items) = three();
    let steps = s.history.undo.len();
    let list = vec![items[0].0, u64::MAX - 1];
    assert!(s.execute("timeline.place", json!({"items": list, "time": 0})).is_err());
    assert!(ends(&s).is_empty());
    assert_eq!(s.history.undo.len(), steps);
}

#[test]
fn rejected_groups_preserve_cleanliness_history_and_published_events() {
    for dirty in [false, true] {
        let (mut s, _) = three();
        if !dirty {
            s.saved_revision = s.revision;
        }
        s.history.merge_key = Some("existing gesture".into());
        s.journal = (0..10_000).map(|i| (format!("retained.{i}"), json!({}))).collect();
        let before = s.project.clone();
        let state = serde_json::to_value(&s.state).unwrap();
        let history = s.history.clone();
        let journal = s.journal.clone();
        let revisions = (s.revision, s.saved_revision);
        s.drain_events();
        let result: crate::Result<()> = s.edit_group("Rejected group", |s| {
            s.execute("file.newBin", json!({"name": "Must roll back"}))?;
            Err(crate::EngineError::Other("injected group error".into()))
        });
        assert!(result.is_err());
        assert_eq!(s.project, before);
        assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
        assert_eq!(s.history.undo, history.undo);
        assert_eq!(s.history.redo, history.redo);
        assert_eq!(s.history.merge_key, history.merge_key);
        assert_eq!(s.journal, journal);
        assert_eq!((s.revision, s.saved_revision), revisions);
        assert_eq!(s.is_dirty(), dirty);
        assert!(s.drain_events().is_empty());
        assert!(s.execute("file.newSequence", json!({"width": 0})).is_err());
        assert_eq!((s.revision, s.saved_revision), revisions);
    }
}

#[test]
fn successful_group_does_not_merge_into_an_earlier_gesture() {
    let (mut s, _) = three();
    s.edit_merged("Gesture", "same key", |p, _| {
        p.name = "Before".into();
        Ok(())
    })
    .unwrap();
    let steps = s.history.undo.len();
    s.edit_group("Group", |s| {
        s.edit_merged("Gesture", "same key", |p, _| {
            p.name = "After".into();
            Ok(())
        })
    })
    .unwrap();
    assert_eq!(s.history.undo.len(), steps + 1);
    s.undo();
    assert_eq!(s.project.name, "Before");
}

#[test]
fn generic_and_merged_edits_refuse_ambiguous_identities_before_committing() {
    for merged in [false, true] {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let sequence = s.state.active_sequence.unwrap();
        s.edit_merged("Gesture", "identity-test", |p, _| {
            p.name = "Retained".into();
            Ok(())
        })
        .unwrap();
        let project = s.project.clone();
        let history = s.history.clone();
        let revision = s.revision;
        let corrupt = |p: &mut filmcraft_project::Project, _: &mut crate::EditorState| {
            let q = p.sequence_mut(sequence).unwrap();
            let repeated = q.video_tracks[0].items[0].clone();
            q.audio_tracks[0].items.push(repeated);
            Ok(())
        };
        let result = if merged { s.edit_merged("Gesture", "identity-test", corrupt) } else { s.edit("Bad identities", corrupt) };
        assert!(result.unwrap_err().to_string().contains("duplicate clip id"));
        assert_eq!(s.project, project);
        assert_eq!(s.history.undo, history.undo);
        assert_eq!(s.history.redo, history.redo);
        assert_eq!(s.history.merge_key, history.merge_key);
        assert_eq!(s.revision, revision);
    }
}

/// `items` is checked: not a list, empty, mixed with `item`, or with one item's range.
#[test]
fn items_are_checked() {
    let (mut s, items) = three();
    let one = items[0].0;
    for p in [
        json!({"items": one}),
        json!({"items": []}),
        json!({"items": ["x"]}),
        json!({"items": [one], "item": one}),
        json!({"items": [one], "duration": 5}),
        json!({"items": [one], "sourceIn": 5}),
    ] {
        assert!(s.execute("timeline.place", p.clone()).is_err(), "{p}");
    }
    assert!(ends(&s).is_empty());
}

/// A new sequence from several items is one undo step: the sequence and its clips go together.
#[test]
fn new_sequence_from_items_is_one_undo_step() {
    let (mut s, items) = three();
    let steps = s.history.undo.len();
    let sequences = s.project.sequences().count();
    s.execute("file.newSequence", json!({"fromItems": items.iter().map(|i| i.0).collect::<Vec<_>>()})).unwrap();
    assert_eq!(ends(&s).len(), items.len());
    assert_eq!(s.history.undo.len(), steps + 1);
    s.undo();
    assert_eq!(s.project.sequences().count(), sequences);
    assert!(s.execute("file.newSequence", json!({"fromItems": 3})).is_err());
}

/// Times at the edge of the number range are refused or placed, never a panic, and a refusal
/// leaves the sequence and the history as they were.
#[test]
fn hostile_times_never_panic_and_leave_nothing_half_placed() {
    let (mut s, items) = three();
    let ids: Vec<u64> = items.iter().map(|i| i.0).collect();
    for t in [i64::MAX, i64::MAX - 5, i64::MIN, -1] {
        let steps = s.history.undo.len();
        let r = s.execute("timeline.place", json!({"items": ids, "time": t}));
        if r.is_err() {
            assert_eq!(s.history.undo.len(), steps, "{t}");
            assert!(ends(&s).is_empty(), "{t}");
        } else {
            assert_eq!(s.history.undo.len(), steps + 1, "{t}");
            s.undo();
        }
    }
    // zero-length and many items: capped, not unbounded
    let many = vec![items[0].0; 10_001];
    assert!(s.execute("timeline.place", json!({"items": many, "time": 0})).is_err());
    assert!(ends(&s).is_empty());
}

/// `"track":"new"` with `items` adds one track, and the whole list goes on it, in one undo step.
#[test]
fn items_on_a_new_track_share_that_one_track() {
    let (mut s, items) = three();
    let tracks = s.active_sequence().unwrap().video_tracks.len();
    let steps = s.history.undo.len();
    let list: Vec<u64> = items.iter().map(|i| i.0).collect();
    s.execute("timeline.place", json!({"items": list, "track": "new", "time": 0})).unwrap();
    let q = s.active_sequence().unwrap();
    assert_eq!(q.video_tracks.len(), tracks + 1, "one new video track, not one per item");
    assert_eq!(q.video_tracks[tracks].items.len(), items.len());
    assert_eq!(s.history.undo.len(), steps + 1);
    s.undo();
    assert_eq!(s.active_sequence().unwrap().video_tracks.len(), tracks, "undo removes the track too");
}

/// A panic inside a group leaves the project, the history and its limit as they were.
#[test]
fn a_panic_in_a_group_rolls_back_and_keeps_the_history_limit() {
    let (mut s, items) = three();
    let (steps, limit) = (s.history.undo.len(), s.history.limit);
    let before = s.project.to_json();
    let one = items[0];
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = s.edit_group::<()>("Boom", |s| {
            s.execute("timeline.place", json!({"item": one.0, "time": 0}))?;
            panic!("inside the group");
        });
    }));
    assert!(outcome.is_err(), "the panic goes on");
    assert_eq!(s.history.limit, limit, "the history is not left unlimited");
    assert_eq!(s.history.undo.len(), steps);
    assert_eq!(s.project.to_json(), before);
}
