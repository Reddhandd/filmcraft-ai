//! Read-only song analysis followed by one atomic, undoable marker/cut application.
use crate::commands::{CommandSpec, has_seq};
use crate::{EngineError, Result, Session};
use filmcraft_audio_dsp::{remix, song_structure};
use filmcraft_media::SharedSource;
use filmcraft_project::{ClipId, ItemId, Label, Marker, MarkerId, MarkerKind, TrackId, TrackItem, TrackKind};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
const RATE: u32 = 8_000;
fn error(s: impl Into<String>) -> EngineError {
    EngineError::Other(s.into())
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub start: Tick,
    pub end: Tick,
    pub label: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub sequence: ItemId,
    pub song: TrackItem,
    pub bpm: f64,
    pub beats: Vec<Tick>,
    pub sections: Vec<Section>,
}
#[derive(Clone)]
pub struct Input {
    sequence: ItemId,
    song: TrackItem,
    source: SharedSource,
}
pub fn selected(s: &Session) -> Result<ClipId> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let clips: Vec<_> =
        s.state.selection.iter().copied().filter(|c| q.find_item(*c).is_some_and(|(t, _)| q.track(t).is_some_and(|tr| tr.kind == TrackKind::Audio))).collect();
    if clips.len() != 1 {
        return Err(error("Select exactly one studio song on an audio track."));
    }
    clips.first().copied().ok_or_else(|| error("Select a studio song."))
}
fn enabled(s: &Session) -> std::result::Result<(), String> {
    selected(s).map(|_| ()).map_err(|e| e.to_string())
}
pub fn prepare(s: &Session, clip: ClipId) -> Result<Input> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (tid, it) = q.find_item(clip).ok_or_else(|| error("Song clip is missing."))?;
    if q.track(tid).is_none_or(|t| t.kind != TrackKind::Audio) {
        return Err(error("Choose an audio clip."));
    }
    if !it.speed.is_finite() || (it.speed - 1.0).abs() > 1e-9 || it.reverse || it.frame_hold.is_some() || filmcraft_render::remix::is_remixed(it) {
        return Err(error("Analyze an ordinary song clip at 100% speed, without Remix or reverse."));
    }
    if it.start.0 < 0 || it.source_in.0 < 0 || !(6 * TICKS_PER_SECOND..=600 * TICKS_PER_SECOND).contains(&it.duration.0) {
        return Err(error("Trim the studio song to between 6 seconds and 10 minutes."));
    }
    it.start.0.checked_add(it.duration.0).ok_or_else(|| error("Invalid timeline range."))?;
    it.source_in.0.checked_add(it.duration.0).ok_or_else(|| error("Invalid source range."))?;
    let source = s.source(it.item).ok_or_else(|| error("Song media is offline."))?;
    let channels = source.info().audio_streams.get(it.audio_stream).map(|stream| stream.channels).unwrap_or(0);
    if !(1..=8).contains(&channels) {
        return Err(error("Song must have 1–8 audio channels."));
    }
    Ok(Input { sequence: s.state.active_sequence.ok_or(EngineError::NoSequence)?, song: it.clone(), source })
}
pub fn analyze(input: Input) -> Result<Plan> {
    let length = usize::try_from(input.song.duration.to_units_floor(i64::from(RATE))).map_err(|_| error("Invalid duration."))?;
    let buf = input
        .source
        .audio_stream(input.song.audio_stream, input.song.source_in.to_units_floor(i64::from(RATE)), length, RATE)
        .map_err(|e| error(format!("Cannot decode song: {e}")))?;
    if buf.channels.is_empty() || buf.channels.len() > 8 || buf.channels.iter().any(|c| c.len() != length || c.iter().any(|v| !v.is_finite())) {
        return Err(error("Decoder returned invalid audio."));
    }
    let energy = |c: &&Vec<f32>| c.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
    let mono = buf.channels.iter().max_by(|a, b| energy(a).total_cmp(&energy(b))).ok_or_else(|| error("No audio."))?;
    if mono.iter().all(|x| x.abs() < 1e-7) {
        return Err(error("The song range is silent."));
    }
    let a = remix::analyze(&[mono.as_slice()], RATE).map_err(|e| error(format!("Cannot detect beats: {e}")))?;
    let suggestions = song_structure::suggest(&a, mono).map_err(error)?;
    let beats = a
        .beats
        .iter()
        .map(|b| Tick::from_units(b.saturating_add(remix::beat_lead(RATE)), i64::from(RATE)))
        .filter(|b| b.0 >= 0 && *b < input.song.duration)
        .collect();
    let mut sections: Vec<_> = suggestions
        .into_iter()
        .map(|s| Section { start: Tick::from_units(s.start, i64::from(RATE)), end: Tick::from_units(s.end, i64::from(RATE)), label: s.label })
        .collect();
    if let Some(last) = sections.last_mut() {
        last.end = input.song.duration;
    }
    Ok(Plan { sequence: input.sequence, song: input.song, bpm: a.bpm, beats, sections })
}
fn validate(plan: &Plan) -> Result<()> {
    if !plan.bpm.is_finite()
        || !(30.0..=300.0).contains(&plan.bpm)
        || plan.beats.is_empty()
        || plan.beats.len() > 2400
        || plan.sections.is_empty()
        || plan.sections.len() > 64
        || !(0..=Tick::MAX.0).contains(&plan.song.start.0)
        || !(6 * TICKS_PER_SECOND..=600 * TICKS_PER_SECOND).contains(&plan.song.duration.0)
        || plan.song.start.0.checked_add(plan.song.duration.0).is_none()
        || plan.beats.iter().any(|t| t.0 < 0 || *t >= plan.song.duration)
        || plan.beats.windows(2).any(|w| w.first() >= w.get(1))
    {
        return Err(error("Invalid song analysis plan."));
    }
    let mut end = Tick::ZERO;
    for sec in &plan.sections {
        if sec.start != end || sec.end <= sec.start || sec.end > plan.song.duration || sec.label.trim().is_empty() || sec.label.len() > 128 {
            return Err(error("Sections must cover the song in order, with nonempty labels."));
        }
        end = sec.end;
    }
    if end != plan.song.duration {
        return Err(error("Sections must end at the song's end."));
    }
    Ok(())
}
/// Count complete beat intervals from this video's timeline start (beat one).
/// Song-relative detections remain unchanged; audio is never trimmed or moved.
/// A detection within half a local beat of the anchor represents the anchor,
/// not the first cut. Missing opening detections are counted from local spacing.
fn grouped_cut_beats(plan: &Plan, anchor: Tick, every: usize) -> Result<Vec<Tick>> {
    if ![1, 2, 4, 8, 16].contains(&every) {
        return Err(error("Choose a supported positive beat interval."));
    }
    let relative = anchor
        .0
        .checked_sub(plan.song.start.0)
        .filter(|t| *t >= 0 && *t < plan.song.duration.0)
        .ok_or_else(|| error("Each selected video must start within the analyzed song, on its intended beat one."))?;
    let next = plan.beats.partition_point(|beat| beat.0 < relative);
    let Some(first) = plan.beats.get(next) else {
        return Ok(Vec::new());
    };
    // Estimate locally, not from the song's opening tempo. Include preceding
    // intervals for anchors near the end of the song.
    let window_start = next.saturating_sub(8);
    let window_end = next.saturating_add(9).min(plan.beats.len());
    let window = plan.beats.get(window_start..window_end).ok_or_else(|| error("Invalid beat window."))?;
    let mut intervals: Vec<i64> = window.windows(2).filter_map(|pair| pair.get(1)?.0.checked_sub(pair.first()?.0).filter(|d| *d > 0)).collect();
    intervals.sort_unstable();
    let period = intervals.get(intervals.len() / 2).copied().ok_or_else(|| error("At least two distinct beats are needed for video cuts."))?;
    let distance = i128::from(first.0) - i128::from(relative);
    let first_ordinal = (distance + i128::from(period / 2)) / i128::from(period);
    let mut cuts = Vec::new();
    for (index, beat) in plan.beats.iter().skip(next).enumerate() {
        let ordinal = first_ordinal + index as i128;
        if ordinal > 0 && ordinal % every as i128 == 0 {
            let time = plan.song.start.0.checked_add(beat.0).ok_or_else(|| error("Invalid beat timeline position."))?;
            cuts.push(Tick(time));
        }
    }
    Ok(cuts)
}

/// Adds markers and optionally splits selected video clips. Does not choose camera angles.
pub fn apply(s: &mut Session, plan: Plan, beat_markers: bool, section_markers: bool, every: usize, videos: Vec<ClipId>) -> Result<Value> {
    validate(&plan)?;
    if ![0, 1, 2, 4, 8, 16].contains(&every) || videos.len() > 2000 {
        return Err(error("Invalid cut interval or too many video clips."));
    }
    if !beat_markers && !section_markers && every == 0 {
        return Err(error("Choose markers or video cuts."));
    }
    if every > 0 && videos.is_empty() {
        return Err(error("Select video clips to cut, or turn off video cuts."));
    }
    if s.state.active_sequence != Some(plan.sequence) {
        return Err(error("The active sequence changed. Analyze again."));
    }
    let song_id = plan.song.id;
    let tag = format!("FilmCraft music analysis / song {}", song_id.0);
    let result = s.edit_sequence("Apply Music Analysis", move |q, ctx, _| {
        if q.find_item(song_id).is_none_or(|(_, it)| it != &plan.song) {
            return Err(error("The song changed. Analyze again."));
        }
        let mut targets: Vec<(TrackId, TrackItem)> = Vec::new();
        if every > 0 {
            for id in &videos {
                let (tid, it) = q.find_item(*id).ok_or_else(|| error("A selected video is missing."))?;
                let tr = q.track(tid).ok_or_else(|| error("Video track is missing."))?;
                if tr.kind != TrackKind::Video || tr.locked {
                    return Err(error("Choose unlocked video clips."));
                }
                if !it.speed.is_finite()
                    || !(0.05..=20.0).contains(&it.speed)
                    || it.reverse
                    || it.frame_hold.is_some()
                    || it.source_in.0 < 0
                    || it.start.0 < 0
                    || it.duration.0 <= 0
                    || it.start.0.checked_add(it.duration.0).is_none()
                    || i128::from(it.source_in.0) + i128::from(it.duration.0) * 20 > i128::from(i64::MAX)
                {
                    return Err(error("Video has an unsupported speed or range."));
                }
                if tr.transitions.iter().any(|t| t.range().overlaps(&it.range())) {
                    return Err(error("Remove transitions from videos before auto-cutting."));
                }
                if !targets.iter().any(|(_, old)| old.id == it.id) {
                    targets.push((tid, it.clone()));
                }
            }
        }
        let estimate = plan.beats.len().saturating_mul(targets.len()).saturating_mul(2).saturating_add(3000);
        if estimate > 100_000 || ctx.next_id.checked_add(estimate as u64).is_none() {
            return Err(error("Too many edits in one operation."));
        }
        let beat_tag = format!("{tag} / beat");
        let section_tag = format!("{tag} / section");
        q.markers.retain(|m| !((beat_markers && m.comment == beat_tag) || (section_markers && m.comment == section_tag)));
        let mut markers = 0usize;
        if beat_markers {
            for (i, beat) in plan.beats.iter().enumerate() {
                q.markers.push(Marker {
                    id: MarkerId(ctx.alloc()),
                    start: Tick(plan.song.start.0 + beat.0),
                    duration: Tick::ZERO,
                    name: format!("Beat {}", i + 1),
                    comment: beat_tag.clone(),
                    kind: MarkerKind::Comment,
                    color: Label::Teal,
                });
                markers += 1;
            }
        }
        if section_markers {
            for sec in &plan.sections {
                q.markers.push(Marker {
                    id: MarkerId(ctx.alloc()),
                    start: Tick(plan.song.start.0 + sec.start.0),
                    duration: sec.end - sec.start,
                    name: sec.label.clone(),
                    comment: section_tag.clone(),
                    kind: MarkerKind::Segmentation,
                    color: Label::Mango,
                });
                markers += 1;
            }
        }
        q.markers.sort_by_key(|m| m.start);
        let mut cuts = 0usize;
        if every > 0 {
            for (tid, original) in targets {
                let mut times: Vec<_> = grouped_cut_beats(&plan, original.start, every)?
                    .into_iter()
                    .map(|b| q.settings.frame_rate.snap_nearest(b))
                    .filter(|b| *b > original.start && *b < original.end())
                    .collect();
                times.sort_unstable();
                times.dedup();
                // A short selected clip with no interior cuts is a true no-op.
                if times.is_empty() {
                    continue;
                }
                let tr = q.track_mut(tid).ok_or_else(|| error("Missing video track."))?;
                // Video-only cuts deliberately unlink the selected clip from live sound.
                if let Some(it) = tr.items.iter_mut().find(|it| it.id == original.id) {
                    it.link = None;
                    it.group = None;
                }
                let mut previous = original.start;
                for &time in &times {
                    if time.0.saturating_sub(previous.0) < ctx.min_duration.0 || original.end().0.saturating_sub(time.0) < ctx.min_duration.0 {
                        continue;
                    }
                    if filmcraft_edit::split_track_at(tr, time, ctx, &mut HashMap::new()).is_some() {
                        cuts += 1;
                        previous = time;
                    }
                }
            }
        }
        Ok(json!({"markers":markers,"cuts":cuts,"bpm":plan.bpm}))
    })?;
    Ok(result)
}
pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "clip.analyzeMusic",
            label: "Analyze Beats & Song Sections…",
            menu: &["Clip"],
            shortcut: None,
            params: r#"{"clip":id?}"#,
            enabled,
            run: |s, p| {
                let clip = match p.get("clip") {
                    None => selected(s)?,
                    Some(v) => ClipId(v.as_u64().ok_or_else(|| error("Invalid song clip identifier."))?),
                };
                serde_json::to_value(analyze(prepare(s, clip)?)?).map_err(|e| error(e.to_string()))
            },
            journal: false,
        },
        CommandSpec {
            id: "clip.analyzeMusic.apply",
            label: "Apply Music Analysis",
            menu: &[],
            shortcut: None,
            params: r#"{"plan":object,"beatMarkers":bool=true,"sectionMarkers":bool=true,"every":0|1|2|4|8|16,"videos":[id]}"#,
            enabled: has_seq,
            run: |s, p| {
                let plan =
                    serde_json::from_value(p.get("plan").cloned().ok_or_else(|| error("Missing plan."))?).map_err(|e| error(format!("Invalid plan: {e}")))?;
                let every = match p.get("every") {
                    None => 0,
                    Some(v) => v.as_u64().filter(|n| *n <= 16).ok_or_else(|| error("Invalid cut interval."))? as usize,
                };
                let videos: Vec<ClipId> = serde_json::from_value(p.get("videos").cloned().unwrap_or(json!([]))).map_err(|e| error(e.to_string()))?;
                apply(
                    s,
                    plan,
                    p.get("beatMarkers").and_then(Value::as_bool).unwrap_or(true),
                    p.get("sectionMarkers").and_then(Value::as_bool).unwrap_or(true),
                    every,
                    videos,
                )
            },
            journal: true,
        },
    ]
}

#[cfg(test)]
#[path = "music_analysis_tests.rs"]
mod tests;
