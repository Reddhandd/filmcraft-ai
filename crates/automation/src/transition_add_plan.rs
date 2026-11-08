//! Resolve one cross dissolve at an explicit video clip's outgoing cut.

use serde::Serialize;
use serde_json::Value;

const TICKS_PER_SECOND: i128 = filmcraft_time::TICKS_PER_SECOND as i128;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TransitionAdd {
    pub clip_id: u64,
    pub next_clip_id: u64,
    pub track_id: u64,
    pub track_name: String,
    pub effect: &'static str,
    pub frames: i64,
    pub cut_ticks: i64,
    pub start_ticks: i64,
    pub duration_ticks: i64,
}

fn tick_of(frame: i64, num: i128, den: i128) -> Result<i64, String> {
    let ticks = i128::from(frame).checked_mul(TICKS_PER_SECOND).and_then(|n| n.checked_mul(den)).ok_or("transition time overflow")?.div_euclid(num);
    i64::try_from(ticks).map_err(|_| "transition time overflow".into())
}

pub(crate) fn resolve(sequence: &Value, clip_id: u64, frames: i64) -> Result<TransitionAdd, String> {
    if !(2..=60).contains(&frames) || frames % 2 != 0 {
        return Err("cross dissolve duration must be an even number of frames from 2 to 60".into());
    }
    let rate = &sequence["settings"]["frame_rate"];
    let num = rate["num"].as_i64().filter(|n| (1..=1_000_000).contains(n)).ok_or("sequence frame rate unavailable")? as i128;
    let den = rate["den"].as_i64().filter(|n| (1..=1_000_000).contains(n)).ok_or("sequence frame rate unavailable")? as i128;
    let duration = tick_of(frames, num, den)?;
    for track in sequence["video"].as_array().ok_or("video tracks unavailable")? {
        let items = track["items"].as_array().ok_or("video track clips unavailable")?;
        let Some(index) = items.iter().position(|item| item["clip"].as_u64() == Some(clip_id)) else { continue };
        let next = items.get(index + 1).ok_or("selected clip has no following clip at its outgoing cut")?;
        let selected = &items[index];
        let cut = selected["end"].as_i64().ok_or("clip end unavailable")?;
        if next["start"].as_i64() != Some(cut) {
            return Err("selected clip and next clip do not meet at a cut".into());
        }
        let next_id = next["clip"].as_u64().ok_or("following clip ID unavailable")?;
        if track["locked"].as_bool() != Some(false) || track["enabled"].as_bool() != Some(true) {
            return Err("video track is locked or disabled".into());
        }
        let track_id = track["id"].as_u64().ok_or("video track ID unavailable")?;
        let track_name = track["name"].as_str().ok_or("video track name unavailable")?;
        // Mirror sequence.applyVideoTransition: cut - half duration, then snap down.
        let unsnapped = cut.checked_sub(duration.div_euclid(2)).ok_or("transition time overflow")?;
        if unsnapped < 0 {
            return Err("transition would begin before the sequence".into());
        }
        let frame_at = (i128::from(unsnapped) * num).div_euclid(TICKS_PER_SECOND * den);
        let start = tick_of(i64::try_from(frame_at).map_err(|_| "transition time overflow")?, num, den)?;
        let end = start.checked_add(duration).ok_or("transition time overflow")?;
        let selected_start = selected["start"].as_i64().ok_or("clip start unavailable")?;
        let next_end = next["end"].as_i64().ok_or("following clip end unavailable")?;
        if start < selected_start || end > next_end {
            return Err("transition is longer than the neighboring clips".into());
        }
        for existing in track["transitions"].as_array().ok_or("video track transitions unavailable")? {
            let tr_start = existing["start"].as_i64().ok_or("transition start unavailable")?;
            let tr_duration = existing["duration"].as_i64().ok_or("transition duration unavailable")?;
            let tr_end = tr_start.checked_add(tr_duration).ok_or("transition time overflow")?;
            if start < tr_end && tr_start < end {
                return Err("a transition already occupies this cut".into());
            }
        }
        return Ok(TransitionAdd {
            clip_id,
            next_clip_id: next_id,
            track_id,
            track_name: track_name.into(),
            effect: "cross_dissolve",
            frames,
            cut_ticks: cut,
            start_ticks: start,
            duration_ticks: duration,
        });
    }
    Err(format!("video clip #{clip_id} was not found in the active sequence"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_adjacent_cut_and_rejects_overlap() {
        let t = filmcraft_time::TICKS_PER_SECOND;
        let mut q = json!({"settings":{"frame_rate":{"num":24,"den":1}},"video":[
            {"id":2,"name":"Video 1","locked":false,"enabled":true,"items":[
                {"clip":10,"start":0,"end":2*t},{"clip":11,"start":2*t,"end":4*t}],"transitions":[]}]});
        let p = resolve(&q, 10, 12).unwrap();
        assert_eq!((p.start_ticks, p.duration_ticks), (2 * t - t / 4, t / 2));
        q["video"][0]["transitions"] = json!([{"start":p.start_ticks,"duration":p.duration_ticks}]);
        assert!(resolve(&q, 10, 12).unwrap_err().contains("occupies"));
        assert!(resolve(&q, 10, 13).is_err());
        assert!(resolve(&q, 99, 12).is_err());
    }
}
