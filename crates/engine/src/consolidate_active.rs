//! Consolidate selected video clips, or the entire video timeline when nothing is selected.
//! Audio and video outside the scope remain untouched. Overlaps get separate tracks.

use std::collections::HashSet;

use filmcraft_edit::EditCtx;
use filmcraft_project::{ClipId, Sequence, Track, TrackId, TrackKind};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, has_seq};
use crate::{EngineError, Result, Session};

struct ConsolidationResult {
    removed: Vec<ClipId>,
    kept: usize,
    moved: usize,
}

fn consolidate(q: &mut Sequence, selected: &[ClipId], ctx: &mut EditCtx<'_>) -> Result<ConsolidationResult> {
    q.check_bounds().map_err(EngineError::Other)?;
    let scoped: HashSet<ClipId> = if selected.is_empty() {
        q.video_tracks.iter().flat_map(|track| track.items.iter().map(|item| item.id)).collect()
    } else {
        selected.iter().copied().collect()
    };
    let affected = q.video_tracks.iter().any(|track| track.items.iter().any(|item| scoped.contains(&item.id)));
    if !affected {
        return Ok(ConsolidationResult { removed: Vec::new(), kept: 0, moved: 0 });
    }

    // Moving a transition endpoint to another track would orphan the transition. Refuse the
    // operation before editing; Session::edit_sequence also rolls back any later error.
    for track in &q.video_tracks {
        if track.locked && track.items.iter().any(|item| scoped.contains(&item.id)) {
            return Err(EngineError::Other(format!("Track '{}' is locked", track.name)));
        }
        if track
            .transitions
            .iter()
            .any(|transition| transition.from.is_some_and(|id| scoped.contains(&id)) || transition.to.is_some_and(|id| scoped.contains(&id)))
        {
            return Err(EngineError::Other("Consolidation involving transitions is not yet supported; remove the affected transition first".into()));
        }
    }

    let mut removed = Vec::new();
    let mut items = Vec::new();
    for (track_index, track) in q.video_tracks.iter_mut().enumerate() {
        let mut remaining = Vec::with_capacity(track.items.len());
        for item in track.items.drain(..) {
            if scoped.contains(&item.id) {
                if item.enabled {
                    items.push((track_index, item));
                } else {
                    removed.push(item.id);
                }
            } else {
                remaining.push(item);
            }
        }
        track.items = remaining;
    }
    items.sort_by_key(|(track_index, item)| (item.start, *track_index, item.id.0));
    let kept = items.len();
    let mut moved = 0;
    for (old_index, item) in items {
        let target = q.video_tracks.iter().position(|track| {
            !track.locked
                && track.enabled
                && track.transitions.is_empty()
                && !track.items.iter().any(|other| item.start < other.end() && other.start < item.end())
        });
        let index = if let Some(index) = target {
            index
        } else {
            let next = (*ctx.next_id).checked_add(1).ok_or_else(|| EngineError::Other("No track IDs remain".into()))?;
            let number = q.video_tracks.len().checked_add(1).ok_or_else(|| EngineError::Other("Too many video tracks".into()))?;
            let id = TrackId(*ctx.next_id);
            *ctx.next_id = next;
            q.video_tracks.push(Track::new(id, TrackKind::Video, format!("Video {number}")));
            q.video_tracks.len() - 1
        };
        if index != old_index {
            moved += 1;
        }
        q.video_tracks[index].items.push(item);
    }
    for track in &mut q.video_tracks {
        track.sort();
    }
    Ok(ConsolidationResult { removed, kept, moved })
}

fn run(s: &mut Session, _: &Value) -> Result<Value> {
    let selection = s.state.selection.clone();
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if !q.video_tracks.iter().flat_map(|track| &track.items).any(|item| selection.is_empty() || selection.contains(&item.id)) {
        return Ok(json!({"removed":0,"kept":0,"moved":0}));
    }
    let result = s.edit_sequence("Consolidate Active Clips", |q, ctx, state| {
        let result = consolidate(q, &selection, ctx)?;
        let removed: HashSet<ClipId> = result.removed.iter().copied().collect();
        state.selection.retain(|id| !removed.contains(id));
        Ok(result)
    })?;
    Ok(json!({"removed":result.removed.len(), "kept":result.kept, "moved":result.moved}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "clip.consolidateActive",
        label: "Consolidate Active Clips",
        menu: &["Clip"],
        shortcut: None,
        params: "{}",
        enabled: has_seq,
        run,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_time::{TICKS_PER_SECOND, Tick};

    fn demo() -> Session {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        s
    }

    #[test]
    fn keeps_every_enabled_overlap_and_studio_audio_through_round_trip_and_undo() {
        let mut s = demo();
        s.edit_sequence("Fixture", |q, _, _| {
            for track in &mut q.video_tracks {
                track.transitions.clear();
            }
            q.video_tracks[0].items[2].enabled = false;
            let mut overlapping = q.video_tracks[0].items.remove(3);
            overlapping.start = q.video_tracks[0].items[1].start;
            q.video_tracks[2].items.push(overlapping);
            Ok(())
        })
        .unwrap();
        let sequence = s.state.active_sequence.unwrap();
        let before = s.project.clone();
        let q = s.active_sequence().unwrap();
        let disabled = q.video_tracks[0].items[2].id;
        let audio = q.audio_tracks.clone();
        let mut kept: Vec<_> = q.video_tracks.iter().flat_map(|track| &track.items).filter(|item| item.enabled).cloned().collect();
        kept.sort_by_key(|item| item.id.0);
        let undo_count = s.history.undo.len();

        let result = s.execute("clip.consolidateActive", json!({})).unwrap();
        assert_eq!(result["removed"], 1);
        assert_eq!(result["kept"], kept.len());
        assert_eq!(s.history.undo.len(), undo_count + 1);
        let q = s.active_sequence().unwrap();
        q.check().unwrap();
        assert!(q.find_item(disabled).is_none());
        assert_eq!(q.audio_tracks, audio);
        let mut actual: Vec<_> = q.video_tracks.iter().flat_map(|track| &track.items).cloned().collect();
        actual.sort_by_key(|item| item.id.0);
        assert_eq!(actual, kept, "consolidation changed or discarded an enabled clip");
        let concurrent: Vec<_> = q.video_tracks.iter().filter_map(|track| track.item_at(Tick(7 * TICKS_PER_SECOND))).collect();
        assert_eq!(concurrent.len(), 3, "three overlapping enabled clips need three tracks");

        let bytes = filmcraft_format::encode(&s.project, false);
        let reopened = filmcraft_format::decode(&bytes).unwrap().project;
        let saved = s.project.clone();
        assert_eq!(reopened.sequence(sequence), saved.sequence(sequence));
        s.undo().unwrap();
        assert_eq!(*s.project, *before);
        s.redo().unwrap();
        assert_eq!(*s.project, *saved);
    }

    #[test]
    fn selected_scope_preserves_unselected_video_and_audio() {
        let mut s = demo();
        let (remove, keep, untouched) = {
            let q = s.active_sequence().unwrap();
            (q.video_tracks[0].items[0].id, q.video_tracks[0].items[1].id, q.video_tracks[0].items[2].clone())
        };
        s.edit_sequence("Fixture", |q, _, _| {
            for track in &mut q.video_tracks {
                track.transitions.clear();
            }
            q.video_tracks[0].items[0].enabled = false;
            Ok(())
        })
        .unwrap();
        let audio = s.active_sequence().unwrap().audio_tracks.clone();
        s.state.selection = vec![remove, keep];
        let result = s.execute("clip.consolidateActive", json!({})).unwrap();
        assert_eq!(result["removed"], 1);
        assert_eq!(result["kept"], 1);
        let q = s.active_sequence().unwrap();
        assert!(q.find_item(remove).is_none());
        assert!(q.find_item(keep).is_some());
        assert_eq!(q.find_item(untouched.id).unwrap().1, &untouched);
        assert_eq!(q.audio_tracks, audio);
        assert!(!s.state.selection.contains(&remove));
    }

    #[test]
    fn audio_only_selection_does_not_create_an_undo_step() {
        let mut s = demo();
        let audio_id = s.active_sequence().unwrap().audio_tracks[1].items[0].id;
        s.state.selection = vec![audio_id];
        let before = s.project.clone();
        let undo_count = s.history.undo.len();
        let result = s.execute("clip.consolidateActive", json!({})).unwrap();
        assert_eq!(result, json!({"removed":0,"kept":0,"moved":0}));
        assert_eq!(*s.project, *before);
        assert_eq!(s.history.undo.len(), undo_count);
    }

    #[test]
    fn refuses_transitions_locks_and_exhausted_track_ids_without_editing() {
        let mut s = demo();
        let selected = s.active_sequence().unwrap().video_tracks[0].items[0].id;
        s.state.selection = vec![selected];
        let original = s.project.clone();
        let undo_count = s.history.undo.len();
        assert!(s.execute("clip.consolidateActive", json!({})).unwrap_err().to_string().contains("transition"));
        assert_eq!(*s.project, *original);
        assert_eq!(s.history.undo.len(), undo_count);

        s.edit_sequence("Fixture", |q, _, _| {
            for track in &mut q.video_tracks {
                track.transitions.clear();
            }
            q.video_tracks[0].locked = true;
            Ok(())
        })
        .unwrap();
        let locked = s.project.clone();
        assert!(s.execute("clip.consolidateActive", json!({})).unwrap_err().to_string().contains("locked"));
        assert_eq!(*s.project, *locked);

        let p = std::sync::Arc::make_mut(&mut s.project);
        let q = p.sequence_mut(s.state.active_sequence.unwrap()).unwrap();
        q.video_tracks[0].locked = false;
        for track in &mut q.video_tracks {
            track.enabled = false;
        }
        p.next_id = u64::MAX;
        let exhausted = s.project.clone();
        let undo_count = s.history.undo.len();
        assert!(s.execute("clip.consolidateActive", json!({})).unwrap_err().to_string().contains("IDs"));
        assert_eq!(*s.project, *exhausted);
        assert_eq!(s.history.undo.len(), undo_count);
    }
}
