//! Sequence-local identities used by timeline and caption editing.
use super::*;

fn fixture() -> (Project, ItemId, TrackItem) {
    let mut p = Project::new("Identities");
    let rate = FrameRate::FPS_24;
    let sequence = p.new_sequence("Main", SequenceSettings::default(), 2, 1, None);
    let media = p.add_item("Layer", Label::Iris, ItemKind::AdjustmentLayer { width: 1920, height: 1080, rate, duration: rate.tick_of(100) }, None);
    let clip = p.make_track_item(media, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, rate.tick_of(24)), rate).unwrap();
    p.sequence_mut(sequence).unwrap().video_tracks[0].items.push(clip.clone());
    (p, sequence, clip)
}

fn caption(id: ClipId, start: Tick) -> Caption {
    Caption { id, start, duration: Tick(TICKS_PER_SECOND), text: "Caption".into(), speaker: None, cue_id: None, settings: String::new() }
}

#[test]
fn repeated_clip_ids_are_rejected_on_the_same_or_another_media_track() {
    for destination in 0..3 {
        let (mut p, sequence, mut clip) = fixture();
        clip.start += clip.duration;
        let q = p.sequence_mut(sequence).unwrap();
        match destination {
            0 => q.video_tracks[0].items.push(clip),
            1 => q.video_tracks[1].items.push(clip),
            _ => q.audio_tracks[0].items.push(clip),
        }
        assert!(q.check_bounds().is_ok(), "the defect is identity ambiguity, not invalid times");
        let error = q.check().unwrap_err();
        assert!(error.contains("duplicate clip id"), "{error}");
        let error = p.check_identities().unwrap_err();
        assert!(error.contains("sequence `Main`"), "{error}");
    }
}

#[test]
fn track_ids_are_shared_by_video_audio_submix_and_caption_tracks() {
    for destination in 0..5 {
        let (mut p, sequence, _) = fixture();
        let q = p.sequence_mut(sequence).unwrap();
        let id = q.video_tracks[0].id;
        match destination {
            0 => q.video_tracks[1].id = id,
            1 => q.audio_tracks[0].id = id,
            2 => q.submix_tracks.push(Track::new(id, TrackKind::Audio, "Submix".into())),
            3 => q.caption_tracks.push(CaptionTrack::new(id, "Captions".into(), CaptionFormat::Subtitle)),
            _ => {
                q.caption_tracks.push(CaptionTrack::new(TrackId(900), "Captions 1".into(), CaptionFormat::Subtitle));
                q.caption_tracks.push(CaptionTrack::new(TrackId(900), "Captions 2".into(), CaptionFormat::Subtitle));
            }
        }
        assert!(q.check_identities().unwrap_err().contains("duplicate track id"));
    }
}

#[test]
fn clip_ids_include_captions_and_even_items_on_submix_tracks() {
    for destination in 0..4 {
        let (mut p, sequence, clip) = fixture();
        let id = TrackId(p.alloc_id());
        let other_id = TrackId(p.alloc_id());
        let q = p.sequence_mut(sequence).unwrap();
        if destination == 3 {
            let mut submix = Track::new(id, TrackKind::Audio, "Submix".into());
            submix.items.push(clip);
            q.submix_tracks.push(submix);
        } else {
            let mut captions = CaptionTrack::new(id, "Captions".into(), CaptionFormat::Subtitle);
            let caption_id = if destination == 0 { clip.id } else { ClipId(900) };
            captions.captions.push(caption(caption_id, Tick::ZERO));
            if destination == 1 {
                captions.captions.push(caption(caption_id, Tick(TICKS_PER_SECOND)));
            }
            q.caption_tracks.push(captions);
            if destination == 2 {
                let mut other = CaptionTrack::new(other_id, "Other Captions".into(), CaptionFormat::Subtitle);
                other.captions.push(caption(caption_id, Tick::ZERO));
                q.caption_tracks.push(other);
            }
        }
        assert!(q.check_identities().unwrap_err().contains("duplicate clip id"));
    }
}

#[test]
fn copied_sequences_and_distinct_clip_copies_keep_their_valid_scopes() {
    let (mut p, sequence, mut clip) = fixture();
    clip.id = ClipId(p.alloc_id());
    clip.start += clip.duration;
    p.sequence_mut(sequence).unwrap().video_tracks[0].items.push(clip);
    let source = p.item(sequence).unwrap().clone();
    p.add_item("Main Copy", source.label, source.kind, None);
    assert!(p.check_identities().is_ok(), "sequence duplication retains valid internal ids");
    for item in p.sequences() {
        assert!(item.as_sequence().unwrap().check().is_ok());
    }
}
