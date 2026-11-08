//! Resolve one audio-only project item into an empty audio track interval.

use serde::Serialize;
use serde_json::Value;

const TICKS_PER_SECOND: i128 = filmcraft_time::TICKS_PER_SECOND as i128;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AudioPlacement {
    pub item_id: u64,
    pub item_name: String,
    pub track_id: u64,
    pub track_name: String,
    pub time_ticks: i64,
    pub duration_ticks: i64,
    pub source_duration_ticks: i64,
}

fn find_item(project: &Value, item_id: u64) -> Result<&Value, String> {
    let mut stack = vec![&project["root"]];
    let mut visited = 0usize;
    while let Some(node) = stack.pop() {
        visited += 1;
        if visited > 10_000 {
            return Err("project tree is too large to resolve safely".into());
        }
        if node["item"].as_u64() == Some(item_id) {
            return Ok(node);
        }
        if let Some(children) = node["children"].as_array() {
            stack.extend(children.iter());
        }
    }
    Err(format!("project item #{item_id} was not found"))
}

fn sequence_rate(sequence: &Value) -> Result<filmcraft_time::FrameRate, String> {
    let rate = &sequence["settings"]["frame_rate"];
    let num = rate["num"].as_i64().filter(|n| (1..=1_000_000).contains(n)).ok_or("sequence frame rate unavailable")?;
    let den = rate["den"].as_i64().filter(|n| (1..=1_000_000).contains(n)).ok_or("sequence frame rate unavailable")?;
    Ok(filmcraft_time::FrameRate::new(num, den))
}

pub(crate) fn resolve(project: &Value, sequence: &Value, item_id: u64, track_label: &str, seconds: f64) -> Result<AudioPlacement, String> {
    if !seconds.is_finite() || seconds < 0.0 || seconds > i64::MAX as f64 / TICKS_PER_SECOND as f64 {
        return Err("timeline time is outside the supported range".into());
    }
    let item = find_item(project, item_id)?;
    if item["type"] != "Audio" || item["audioClipCount"].as_u64() != Some(1) {
        return Err("this placement requires one audio-only media item without split channels".into());
    }
    let name = item["name"].as_str().ok_or("audio item name unavailable")?;
    let source_duration = item["duration"].as_i64().filter(|d| *d > 0).ok_or("audio item duration unavailable")?;
    let rate = sequence_rate(sequence)?;
    let index = track_label
        .trim()
        .strip_prefix('A')
        .or_else(|| track_label.trim().strip_prefix('a'))
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|i| *i > 0)
        .ok_or("name an audio track as A1, A2, and so on")?
        - 1;
    let track = sequence["audio"].as_array().and_then(|tracks| tracks.get(index)).ok_or("audio track was not found in the active sequence")?;
    let track_id = track["id"].as_u64().ok_or("audio track ID unavailable")?;
    let track_name = track["name"].as_str().ok_or("audio track name unavailable")?;
    if track["locked"].as_bool() != Some(false) || track["enabled"].as_bool() != Some(true) {
        return Err(format!("track {track_name} is locked or disabled"));
    }
    let raw = (seconds * TICKS_PER_SECOND as f64).round() as i64;
    let raw_end = raw.checked_add(source_duration).ok_or("placement would exceed the supported timeline")?;
    let time = rate.snap_nearest(filmcraft_time::Tick(raw)).0;
    let end = rate.snap_nearest(filmcraft_time::Tick(raw_end)).0;
    let duration = end.checked_sub(time).filter(|d| *d > 0).ok_or("placement is shorter than one frame")?;
    for clip in track["items"].as_array().ok_or("audio track clips unavailable")? {
        let start = clip["start"].as_i64().ok_or("clip start unavailable")?;
        let clip_end = clip["end"].as_i64().ok_or("clip end unavailable")?;
        if time < clip_end && start < end {
            return Err(format!("placement would overlap clip #{} on {track_name}", clip["clip"]));
        }
    }
    for transition in track["transitions"].as_array().ok_or("audio track transitions unavailable")? {
        let start = transition["start"].as_i64().ok_or("transition start unavailable")?;
        let length = transition["duration"].as_i64().ok_or("transition duration unavailable")?;
        let transition_end = start.checked_add(length).ok_or("transition time overflow")?;
        if time < transition_end && start < end {
            return Err("placement would overlap an audio transition".into());
        }
    }
    Ok(AudioPlacement {
        item_id,
        item_name: name.into(),
        track_id,
        track_name: track_name.into(),
        time_ticks: time,
        duration_ticks: duration,
        source_duration_ticks: source_duration,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn audio_item_placement_resolves_exact_track_and_rejects_overlap() {
        let t = filmcraft_time::TICKS_PER_SECOND;
        let project = json!({"root":{"children":[{"children":[{"item":11,"name":"Score.wav","type":"Audio",
            "duration":t*3,"audioClipCount":1}]}]}});
        let mut sequence = json!({"settings":{"frame_rate":{"num":24,"den":1}},"audio":[
            {"id":20,"name":"Audio 1","locked":false,"enabled":true,"items":[],"transitions":[]}]});
        let placement = resolve(&project, &sequence, 11, "A1", 5.0).unwrap();
        assert_eq!((placement.track_id, placement.time_ticks, placement.duration_ticks), (20, 5 * t, 3 * t));
        sequence["audio"][0]["items"] = json!([{"clip":50,"start":6*t,"end":7*t}]);
        assert!(resolve(&project, &sequence, 11, "A1", 5.0).unwrap_err().contains("overlap"));
        assert!(resolve(&project, &sequence, 11, "A2", 5.0).is_err());
        assert!(resolve(&project, &sequence, 11, "A1", f64::NAN).is_err());
    }
}
