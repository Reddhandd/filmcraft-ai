//! Resolve a bounded clip move against one inspected sequence.

use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ClipMove {
    pub clip_id: u64,
    pub track_id: u64,
    pub track_name: String,
    pub kind: String,
    pub name: String,
    pub from_ticks: i64,
    pub to_ticks: i64,
    pub duration_ticks: i64,
}

pub(crate) fn seconds_from_text(text: &str) -> Result<f64, String> {
    let lower = text.trim().to_ascii_lowercase();
    let number = lower
        .strip_suffix(" seconds")
        .or_else(|| lower.strip_suffix(" second"))
        .or_else(|| lower.strip_suffix('s'))
        .ok_or("use a destination in seconds, such as '12 seconds'")?;
    let seconds: f64 = number.trim().parse().map_err(|_| "destination time must be a number of seconds".to_string())?;
    if !seconds.is_finite() || seconds < 0.0 || seconds > i64::MAX as f64 / filmcraft_time::TICKS_PER_SECOND as f64 {
        return Err("destination time is outside the supported range".into());
    }
    Ok(seconds)
}

pub(crate) fn resolve(sequence: &Value, clip_id: u64, seconds: f64) -> Result<Vec<ClipMove>, String> {
    if !seconds.is_finite() || seconds < 0.0 || seconds > i64::MAX as f64 / filmcraft_time::TICKS_PER_SECOND as f64 {
        return Err("destination time is outside the supported range".into());
    }
    let to_ticks = (seconds * filmcraft_time::TICKS_PER_SECOND as f64).round() as i64;
    if (to_ticks as f64 / filmcraft_time::TICKS_PER_SECOND as f64 - seconds).abs() > 0.000000001 {
        return Err("destination time cannot be represented exactly".into());
    }
    resolve_ticks(sequence, clip_id, to_ticks)
}

pub(crate) fn resolve_ticks(sequence: &Value, clip_id: u64, to_ticks: i64) -> Result<Vec<ClipMove>, String> {
    if to_ticks < 0 {
        return Err("destination time cannot be negative".into());
    }
    let mut clips = Vec::new();
    for (kind, field) in [("video", "video"), ("audio", "audio")] {
        let tracks = sequence[field].as_array().ok_or("sequence tracks unavailable")?;
        for track in tracks {
            let track_id = track["id"].as_u64().ok_or("track ID unavailable")?;
            let track_name = track["name"].as_str().ok_or("track name unavailable")?;
            let items = track["items"].as_array().ok_or("track clips unavailable")?;
            for item in items {
                clips.push((kind, track_id, track_name, track, item));
            }
        }
    }
    let (_, _, _, _, selected) = clips
        .iter()
        .find(|(_, _, _, _, item)| item["clip"].as_u64() == Some(clip_id))
        .ok_or_else(|| format!("clip #{clip_id} was not found in the active sequence"))?;
    let selected_start = selected["start"].as_i64().ok_or("clip start unavailable")?;
    let link = selected["link"].as_u64();
    let delta = i128::from(to_ticks) - i128::from(selected_start);
    if delta == 0 {
        return Err("clip is already at that time".into());
    }
    let mut moves = Vec::new();
    for (kind, track_id, track_name, track, item) in &clips {
        let id = item["clip"].as_u64().ok_or("clip ID unavailable")?;
        if id != clip_id && !link.is_some_and(|linked| item["link"].as_u64() == Some(linked)) {
            continue;
        }
        if track["locked"].as_bool() != Some(false) || track["enabled"].as_bool() != Some(true) {
            return Err(format!("track {track_name} is locked or disabled"));
        }
        let from = item["start"].as_i64().ok_or("clip start unavailable")?;
        let duration = item["duration"].as_i64().filter(|v| *v > 0).ok_or("clip duration unavailable")?;
        let next = i64::try_from(i128::from(from) + delta).map_err(|_| "move time overflow".to_string())?;
        if next < 0 || next.checked_add(duration).is_none() {
            return Err("move would put a clip outside the supported timeline".into());
        }
        moves.push(ClipMove {
            clip_id: id,
            track_id: *track_id,
            track_name: (*track_name).into(),
            kind: (*kind).into(),
            name: item["name"].as_str().unwrap_or("unnamed").into(),
            from_ticks: from,
            to_ticks: next,
            duration_ticks: duration,
        });
    }
    if moves.is_empty() || moves.len() > 32 {
        return Err("move must affect 1–32 clips".into());
    }
    for (_, track_id, track_name, track, item) in &clips {
        let id = item["clip"].as_u64().ok_or("clip ID unavailable")?;
        if moves.iter().any(|m| m.clip_id == id) {
            continue;
        }
        let start = item["start"].as_i64().ok_or("clip start unavailable")?;
        let end = item["end"].as_i64().ok_or("clip end unavailable")?;
        for moved in moves.iter().filter(|m| m.track_id == *track_id) {
            if moved.to_ticks < end && start < moved.to_ticks + moved.duration_ticks {
                return Err(format!("move would overlap clip #{id} on {track_name}"));
            }
        }
        let _ = track;
    }
    for moved in &moves {
        for other in moves.iter().filter(|m| m.track_id == moved.track_id && m.clip_id != moved.clip_id) {
            if moved.to_ticks < other.to_ticks + other.duration_ticks && other.to_ticks < moved.to_ticks + moved.duration_ticks {
                return Err("linked clips would overlap after the move".into());
            }
        }
    }
    for field in ["video", "audio"] {
        for track in sequence[field].as_array().ok_or("sequence tracks unavailable")? {
            for transition in track["transitions"].as_array().ok_or("track transitions unavailable")? {
                if moves.iter().any(|m| transition["from"].as_u64() == Some(m.clip_id) || transition["to"].as_u64() == Some(m.clip_id)) {
                    return Err("clip has an attached transition; this move is not supported yet".into());
                }
            }
        }
    }
    Ok(moves)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_linked_pair_and_rejects_collisions_and_transitions() {
        let t = filmcraft_time::TICKS_PER_SECOND;
        let mut sequence = json!({
            "video":[{"id":1,"name":"V1","locked":false,"enabled":true,"transitions":[],
                "items":[{"clip":10,"name":"Picture","start":0,"end":t,"duration":t,"link":7},
                         {"clip":11,"name":"Other","start":10*t,"end":11*t,"duration":t,"link":null}]}],
            "audio":[{"id":2,"name":"A1","locked":false,"enabled":true,"transitions":[],
                "items":[{"clip":12,"name":"Sound","start":0,"end":t,"duration":t,"link":7}]}]
        });
        let resolved = resolve(&sequence, 10, 5.0).unwrap();
        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().all(|m| m.to_ticks == 5 * t));
        assert!(resolve(&sequence, 10, 10.0).unwrap_err().contains("overlap"));
        sequence["video"][0]["transitions"] = json!([{"from":10,"to":11}]);
        assert!(resolve(&sequence, 10, 5.0).unwrap_err().contains("transition"));
        assert!(resolve(&sequence, 10, f64::NAN).is_err());
        assert!(resolve(&sequence, 999, 5.0).is_err());
    }
}
