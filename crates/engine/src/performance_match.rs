//! Live video → studio song matching. Analysis is read-only; applying a reviewed plan is one undo step.
use crate::commands::{CommandSpec, has_seq};
use crate::{EngineError, Result, Session};
use filmcraft_audio_dsp::performance_match as dsp;
use filmcraft_media::SharedSource;
use filmcraft_project::{ClipId, ItemId, TimeInterpolation, TrackItem, TrackKind};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub song: Tick,
    pub live: Tick,
    pub similarity: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Plan {
    pub sequence: ItemId,
    pub video: TrackItem,
    pub song: TrackItem,
    pub points: Vec<Point>,
    pub similarity: f32,
    #[serde(default)]
    pub suggested_live_end: Option<Tick>,
    #[serde(default)]
    pub endpoint_confidence: Option<f32>,
}

#[derive(Clone)]
pub struct Input {
    sequence: ItemId,
    video: TrackItem,
    song: TrackItem,
    video_source: SharedSource,
    song_source: SharedSource,
}

fn error(msg: impl Into<String>) -> EngineError {
    EngineError::Other(msg.into())
}

pub fn selected(s: &Session) -> Result<(ClipId, ClipId)> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let video = s
        .state
        .selection
        .iter()
        .filter_map(|id| q.find_item(*id))
        .find(|(tid, _)| q.track(*tid).is_some_and(|t| t.kind == TrackKind::Video))
        .map(|(_, it)| it)
        .ok_or_else(|| error("Select a live video clip in the timeline."))?;
    let song = s
        .state
        .selection
        .iter()
        .filter_map(|id| q.find_item(*id))
        .find(|(tid, it)| it.item != video.item && q.track(*tid).is_some_and(|t| t.kind == TrackKind::Audio))
        .map(|(_, it)| it)
        .ok_or_else(|| error("Also select the studio song on an audio track (Command-click)."))?;
    Ok((video.id, song.id))
}

fn enabled(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    selected(s).map(|_| ()).map_err(|e| e.to_string())
}

pub fn prepare(s: &Session, video: ClipId, song: ClipId) -> Result<Input> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (vt, v) = q.find_item(video).ok_or_else(|| error("The video clip is missing."))?;
    let (at, a) = q.find_item(song).ok_or_else(|| error("The song clip is missing."))?;
    if q.track(vt).is_none_or(|t| t.kind != TrackKind::Video) || q.track(at).is_none_or(|t| t.kind != TrackKind::Audio) || video == song {
        return Err(error("Choose a video clip and a separate studio audio clip."));
    }
    for it in [v, a] {
        if !it.speed.is_finite() || it.reverse || (it.speed - 1.0).abs() > 1e-9 || it.frame_hold.is_some() || filmcraft_render::remix::is_remixed(it) {
            return Err(error("Start with ordinary clips at 100% speed, without reverse, frame hold or Remix."));
        }
        if it.source_in.0 < 0 || it.duration.0 < 2 * TICKS_PER_SECOND || it.duration.0 > 600 * TICKS_PER_SECOND {
            return Err(error("Trim each clip to the same song, between 2 seconds and 10 minutes."));
        }
        it.source_in.0.checked_add(it.duration.0).ok_or_else(|| error("Clip range is too large."))?;
    }
    let video_source = s.source(v.item).ok_or_else(|| error("The live video is offline."))?;
    let song_source = s.source(a.item).ok_or_else(|| error("The studio song is offline."))?;
    for (src, item) in [(&video_source, v), (&song_source, a)] {
        let channels = src.info().audio_streams.get(item.audio_stream).map(|stream| stream.channels).unwrap_or(0);
        if channels == 0 || channels > 8 {
            return Err(error("Both recordings need audible audio, with at most 8 channels."));
        }
    }
    Ok(Input { sequence: s.state.active_sequence.ok_or(EngineError::NoSequence)?, video: v.clone(), song: a.clone(), video_source, song_source })
}

fn mono(source: &SharedSource, it: &TrackItem) -> Result<Vec<f32>> {
    let len = usize::try_from(it.duration.to_units_floor(i64::from(dsp::RATE))).map_err(|_| error("Invalid clip duration."))?;
    let buf = source
        .audio_stream(it.audio_stream, it.source_in.to_units_floor(i64::from(dsp::RATE)), len, dsp::RATE)
        .map_err(|e| error(format!("Cannot read recording audio: {e}")))?;
    if buf.channels.is_empty() || buf.channels.len() > 8 || buf.channels.iter().any(|c| c.len() != len) {
        return Err(error("The decoder did not return the requested song range."));
    }
    let mut out = vec![0.0f32; len];
    // Use the loudest channel rather than averaging potentially opposite-polarity stereo.
    let channel = buf
        .channels
        .iter()
        .max_by(|a, b| {
            let energy = |c: &Vec<f32>| c.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
            energy(a).total_cmp(&energy(b))
        })
        .ok_or_else(|| error("No audio channel."))?;
    out.copy_from_slice(channel);
    Ok(out)
}

pub fn analyze(input: Input) -> Result<Plan> {
    let reference = mono(&input.song_source, &input.song)?;
    let live = mono(&input.video_source, &input.video)?;
    let suggestion = dsp::suggest_live_endpoint(&reference, &live).and_then(|(sample, confidence)| {
        let end = Tick::from_units(sample as i64, i64::from(dsp::RATE));
        (end.0 > 0 && end < input.video.duration).then_some((end, confidence))
    });
    let found = dsp::match_performance(&reference, &live).map_err(error)?;
    let mut points: Vec<Point> = found
        .anchors
        .into_iter()
        .map(|p| Point {
            song: Tick::from_units(p.reference_sample as i64, i64::from(dsp::RATE)),
            live: Tick::from_units(p.live_sample as i64, i64::from(dsp::RATE)),
            similarity: p.similarity,
        })
        .collect();
    if let Some(end) = points.last_mut() {
        end.song = input.song.duration;
        end.live = input.video.duration;
    }
    Ok(Plan {
        sequence: input.sequence,
        video: input.video,
        song: input.song,
        points,
        similarity: found.similarity,
        suggested_live_end: suggestion.map(|x| x.0),
        endpoint_confidence: suggestion.map(|x| x.1),
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassageCandidate {
    pub live_start: Tick,
    pub live_end: Tick,
    pub ranking_score: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassageSearch {
    pub sequence: ItemId,
    pub video: TrackItem,
    pub song: TrackItem,
    pub resolution: Tick,
    pub candidates: Vec<PassageCandidate>,
    pub requires_review: bool,
    pub arrangement_verified: bool,
    pub boundaries_verified: bool,
}

/// Coarse retrieval only: never returns a plan that can be applied to a timeline.
pub fn find_passages(input: Input) -> Result<PassageSearch> {
    find_passages_cancellable(input, || false)
}

pub fn find_passages_cancellable(input: Input, cancelled: impl Fn() -> bool) -> Result<PassageSearch> {
    if cancelled() {
        return Err(error("Passage search cancelled."));
    }
    let reference = mono(&input.song_source, &input.song)?;
    if cancelled() {
        return Err(error("Passage search cancelled."));
    }
    let live = mono(&input.video_source, &input.video)?;
    let found = dsp::passages::find(&reference, &live, cancelled).map_err(error)?;
    let candidates = found
        .candidates
        .iter()
        .map(|c| PassageCandidate {
            live_start: Tick::from_units(c.live_start_sample as i64, i64::from(dsp::RATE)),
            live_end: Tick::from_units(c.live_end_sample as i64, i64::from(dsp::RATE)),
            ranking_score: c.score,
        })
        .collect();
    Ok(PassageSearch {
        sequence: input.sequence,
        video: input.video,
        song: input.song,
        resolution: Tick::from_units(found.resolution_samples as i64, i64::from(dsp::RATE)),
        requires_review: found.requires_review,
        arrangement_verified: false,
        boundaries_verified: false,
        candidates,
    })
}

/// Explicit user-reviewed trim, separate from retrieval and timing analysis.
/// A changed source or a differently edited linked item refuses the whole edit.
pub fn trim_passage(s: &mut Session, report: PassageSearch, start: Tick, end: Tick) -> Result<Value> {
    if s.state.active_sequence != Some(report.sequence) {
        return Err(error("The active sequence changed. Find passages again."));
    }
    prepare(s, report.video.id, report.song.id)?;
    if start.0 < 0 || end > report.video.duration || end.0.checked_sub(start.0).is_none_or(|d| d < 2 * TICKS_PER_SECOND) {
        return Err(error("Choose reviewed boundaries within the live clip, at least two seconds apart."));
    }
    s.edit_sequence("Trim Confirmed Performance Passage", move |q, _, _| {
        let (tid, current) = q.find_item(report.video.id).ok_or_else(|| error("The live clip is missing."))?;
        if current != &report.video || q.find_item(report.song.id).is_none_or(|(_, it)| it != &report.song) {
            return Err(error("A source clip changed. Find passages again."));
        }
        let tr = q.track(tid).ok_or_else(|| error("Missing video track."))?;
        if tr.locked || tr.transitions.iter().any(|t| t.range().overlaps(&current.range())) {
            return Err(error("Unlock the video track and remove touching transitions before trimming."));
        }
        let linked = |it: &TrackItem| report.video.link.is_some() && it.link == report.video.link;
        if q.video_tracks.iter().flat_map(|t| &t.items).any(|it| it.id != report.video.id && linked(it)) {
            return Err(error("The live clip shares a link with another video edit. Review that link before trimming."));
        }
        for tr in &q.audio_tracks {
            for it in tr.items.iter().filter(|it| linked(it)) {
                if tr.locked
                    || it.id == report.song.id
                    || it.item != report.video.item
                    || it.start != report.video.start
                    || it.duration != report.video.duration
                    || it.source_in != report.video.source_in
                    || it.speed != 1.0
                    || it.reverse
                    || it.frame_hold.is_some()
                    || filmcraft_render::remix::is_remixed(it)
                    || tr.transitions.iter().any(|t| t.range().overlaps(&it.range()))
                {
                    return Err(error("Linked audio has a different edit, transition or lock. Review it before trimming."));
                }
            }
        }
        let source_in = Tick(report.video.source_in.0.checked_add(start.0).ok_or_else(|| error("Invalid source offset."))?);
        let duration = Tick(end.0.checked_sub(start.0).ok_or_else(|| error("Invalid passage interval."))?);
        for tr in q.video_tracks.iter_mut().chain(q.audio_tracks.iter_mut()) {
            for it in &mut tr.items {
                if it.id == report.video.id || linked(it) {
                    it.source_in = source_in;
                    it.duration = duration;
                }
            }
        }
        Ok(json!({"clip":report.video.id,"sourceIn":source_in,"duration":duration}))
    })
}

/// Verify the plan against the current document and apply without overwriting other clips.
pub fn apply(s: &mut Session, plan: Plan, mute_live: bool) -> Result<Value> {
    if s.state.active_sequence != Some(plan.sequence) {
        return Err(error("The active sequence changed. Analyze again."));
    }
    prepare(s, plan.video.id, plan.song.id)?;
    if plan.points.len() < 2 || plan.points.len() > 1000 || !plan.similarity.is_finite() || !(0.0..=1.0).contains(&plan.similarity) {
        return Err(error("Invalid match plan."));
    }
    if plan.points.first().is_none_or(|p| p.song != Tick::ZERO || p.live != Tick::ZERO)
        || plan.points.last().is_none_or(|p| p.song != plan.song.duration || p.live <= Tick::ZERO || p.live > plan.video.duration)
    {
        return Err(error(
            "Keep the first timing point at zero and the last song point at its full duration; the live endpoint must be within the source clip.",
        ));
    }
    let ids = s.edit_sequence("Match Performance to Song", move |q, ctx, state| {
        let (tid, current) = q.find_item(plan.video.id).ok_or_else(|| error("The live video is missing."))?;
        if current != &plan.video || q.find_item(plan.song.id).is_none_or(|(_, a)| a != &plan.song) {
            return Err(error("A source clip changed since analysis. Analyze again."));
        }
        if q.track(tid).is_none_or(|t| t.locked) {
            return Err(error("The video track is locked."));
        }
        let end = plan.song.start.0.checked_add(plan.song.duration.0).ok_or_else(|| error("Song end is too large."))?;
        let tr = q.track(tid).ok_or_else(|| error("The video track is missing."))?;
        if tr.transitions.iter().any(|t| t.range().overlaps(&plan.video.range())) {
            return Err(error("Remove transitions touching the live clip before matching."));
        }
        if tr.items.iter().any(|it| it.id != plan.video.id && it.start.0 < end && it.end() > plan.song.start) {
            return Err(error("The matched video would overlap another clip. Move it to an empty video track."));
        }
        let mut pieces = Vec::with_capacity(plan.points.len() - 1);
        for pair in plan.points.windows(2) {
            let (Some(a), Some(b)) = (pair.first(), pair.get(1)) else {
                return Err(error("Invalid timing points."));
            };
            if !a.similarity.is_finite()
                || !b.similarity.is_finite()
                || a.song.0 < 0
                || a.live.0 < 0
                || b.song > plan.song.duration
                || b.live > plan.video.duration
            {
                return Err(error("Invalid timing point."));
            }
            let duration = b.song.0.checked_sub(a.song.0).ok_or_else(|| error("Invalid song interval."))?;
            let source = b.live.0.checked_sub(a.live.0).ok_or_else(|| error("Invalid live interval."))?;
            if duration < ctx.min_duration.0 || source <= 0 {
                return Err(error("Timing points must increase and be at least one video frame apart."));
            }
            let speed = source as f64 / duration as f64;
            if !(0.5..=2.0).contains(&speed) {
                return Err(error("A section needs more than 50–200% speed. Adjust its timing points or trim the song more closely."));
            }
            let id = ClipId(*ctx.next_id);
            *ctx.next_id = ctx.next_id.checked_add(1).ok_or_else(|| error("No more clip identifiers."))?;
            let mut piece = plan.video.clone();
            piece.id = id;
            piece.start = Tick(plan.song.start.0.checked_add(a.song.0).ok_or_else(|| error("Invalid placement."))?);
            piece.duration = Tick(duration);
            piece.source_in = Tick(plan.video.source_in.0.checked_add(a.live.0).ok_or_else(|| error("Invalid source time."))?);
            piece.speed = speed;
            piece.link = None;
            piece.group = None;
            piece.time_interpolation = TimeInterpolation::FrameBlending;
            pieces.push(piece);
        }
        let ids: Vec<ClipId> = pieces.iter().map(|p| p.id).collect();
        let tr = q.track_mut(tid).ok_or_else(|| error("Missing video track."))?;
        tr.items.retain(|p| p.id != plan.video.id);
        tr.items.extend(pieces);
        tr.items.sort_by_key(|p| p.start);
        // Unlink the original live sound; optionally silence only its linked instances.
        for tr in &mut q.audio_tracks {
            for it in &mut tr.items {
                if plan.video.link.is_some() && it.link == plan.video.link {
                    if tr.locked {
                        return Err(error("The linked live-audio track is locked."));
                    }
                    it.link = None;
                    if mute_live {
                        it.enabled = false;
                    }
                }
            }
        }
        state.selection = ids.clone();
        state.playheads.insert(plan.sequence, plan.song.start);
        Ok(ids)
    })?;
    Ok(json!({"clips":ids.iter().map(|c|c.0).collect::<Vec<_>>(),"undo":"Match Performance to Song"}))
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "clip.findPerformancePassages",
            label: "Find Performance Passages",
            menu: &[],
            shortcut: None,
            params: r#"{"video":id,"song":id}"#,
            enabled: has_seq,
            run: |s, p| {
                let video = p.get("video").and_then(Value::as_u64).ok_or_else(|| error("Pass a valid live video clip identifier."))?;
                let song = p.get("song").and_then(Value::as_u64).ok_or_else(|| error("Pass a valid studio audio clip identifier."))?;
                serde_json::to_value(find_passages(prepare(s, ClipId(video), ClipId(song))?)?).map_err(|e| error(e.to_string()))
            },
            journal: false,
        },
        CommandSpec {
            id: "clip.trimPerformancePassage",
            label: "Trim Confirmed Performance Passage",
            menu: &[],
            shortcut: None,
            params: r#"{"report":object,"start":ticks,"end":ticks,"phrasesConfirmed":[true,true,true],"boundariesReviewed":true}"#,
            enabled: has_seq,
            run: |s, p| {
                if p.get("boundariesReviewed").and_then(Value::as_bool) != Some(true)
                    || p.get("phrasesConfirmed").and_then(Value::as_array).is_none_or(|v| v.len() != 3 || v.iter().any(|v| v.as_bool() != Some(true)))
                {
                    return Err(error("Confirm the start, middle and end phrases and review the exact trim boundaries first."));
                }
                let report = serde_json::from_value(p.get("report").cloned().ok_or_else(|| error("Missing passage report."))?)
                    .map_err(|e| error(format!("Invalid passage report: {e}")))?;
                let start = p.get("start").and_then(Value::as_i64).ok_or_else(|| error("Invalid passage start."))?;
                let end = p.get("end").and_then(Value::as_i64).ok_or_else(|| error("Invalid passage end."))?;
                trim_passage(s, report, Tick(start), Tick(end))
            },
            journal: true,
        },
        CommandSpec {
            id: "clip.matchPerformance",
            label: "Match Performance to Song…",
            menu: &["Clip"],
            shortcut: None,
            params: r#"{"clips":[id]? (exactly two: live video, studio audio)}"#,
            enabled,
            run: |s, p| {
                let ids = p.get("clips").and_then(Value::as_array);
                let pair = if let Some(ids) = ids {
                    if ids.len() != 2 {
                        return Err(error("Pass exactly two clip identifiers: video, studio audio."));
                    }
                    (
                        ClipId(ids.first().and_then(Value::as_u64).ok_or_else(|| error("Invalid video identifier."))?),
                        ClipId(ids.get(1).and_then(Value::as_u64).ok_or_else(|| error("Invalid song identifier."))?),
                    )
                } else {
                    selected(s)?
                };
                let result = analyze(prepare(s, pair.0, pair.1)?)?;
                serde_json::to_value(result).map_err(|e| error(e.to_string()))
            },
            journal: false,
        },
        CommandSpec {
            id: "clip.matchPerformance.apply",
            label: "Apply Performance Match",
            menu: &[],
            shortcut: None,
            params: r#"{"plan":object,"muteLive":bool=true,"expectedRevision":u64?,"sequenceId":id?}"#,
            enabled: has_seq,
            run: |s, p| {
                if let Some(expected) = p.get("expectedRevision") {
                    let expected = expected.as_u64().ok_or_else(|| error("Invalid expected project revision."))?;
                    if expected != s.revision {
                        return Err(error(format!("stale project revision: expected {expected}, current {}", s.revision)));
                    }
                }
                if let Some(expected) = p.get("sequenceId") {
                    let expected = expected.as_u64().ok_or_else(|| error("Invalid sequence identifier."))?;
                    if s.state.active_sequence.map(|id| id.0) != Some(expected) {
                        return Err(error("active sequence changed since preview"));
                    }
                }
                let plan: Plan =
                    serde_json::from_value(p.get("plan").cloned().ok_or_else(|| error("Missing plan."))?).map_err(|e| error(format!("Invalid plan: {e}")))?;
                apply(s, plan, p.get("muteLive").and_then(Value::as_bool).unwrap_or(true))
            },
            journal: true,
        },
    ]
}
