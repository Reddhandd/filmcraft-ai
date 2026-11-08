//! Resolve one explicit, gap-preserving clip deletion from inspected sequence state.

use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DeletedClip {
    pub clip_id: u64,
    pub item_id: u64,
    pub name: String,
    pub track_id: u64,
    pub track_name: String,
    pub kind: String,
    pub start_ticks: i64,
    pub duration_ticks: i64,
}

pub(crate) fn resolve(sequence: &Value, clip_id: u64) -> Result<Vec<DeletedClip>, String> {
    let mut clips = Vec::new();
    for (kind, field) in [("video", "video"), ("audio", "audio")] {
        for track in sequence[field].as_array().ok_or("sequence tracks unavailable")? {
            let track_id = track["id"].as_u64().ok_or("track ID unavailable")?;
            let track_name = track["name"].as_str().ok_or("track name unavailable")?;
            for item in track["items"].as_array().ok_or("track clips unavailable")? {
                clips.push((kind, track_id, track_name, track, item));
            }
        }
    }
    let selected = clips
        .iter()
        .find(|(_, _, _, _, item)| item["clip"].as_u64() == Some(clip_id))
        .ok_or_else(|| format!("clip #{clip_id} was not found in the active sequence"))?;
    let link = selected.4["link"].as_u64();
    let mut affected = Vec::new();
    for (kind, track_id, track_name, track, item) in &clips {
        let id = item["clip"].as_u64().ok_or("clip ID unavailable")?;
        if id != clip_id && !link.is_some_and(|group| item["link"].as_u64() == Some(group)) {
            continue;
        }
        if track["locked"].as_bool() != Some(false) {
            return Err(format!("track {track_name} is locked"));
        }
        affected.push(DeletedClip {
            clip_id: id,
            item_id: item["item"].as_u64().ok_or("clip source ID unavailable")?,
            name: item["name"].as_str().ok_or("clip name unavailable")?.into(),
            track_id: *track_id,
            track_name: (*track_name).into(),
            kind: (*kind).into(),
            start_ticks: item["start"].as_i64().ok_or("clip start unavailable")?,
            duration_ticks: item["duration"].as_i64().filter(|v| *v > 0).ok_or("clip duration unavailable")?,
        });
    }
    if affected.is_empty() || affected.len() > 32 {
        return Err("delete must affect 1–32 clips".into());
    }
    for field in ["video", "audio"] {
        for track in sequence[field].as_array().ok_or("sequence tracks unavailable")? {
            for transition in track["transitions"].as_array().ok_or("track transitions unavailable")? {
                if affected.iter().any(|clip| transition["from"].as_u64() == Some(clip.clip_id) || transition["to"].as_u64() == Some(clip.clip_id)) {
                    return Err("clip has an attached transition; remove that transition first".into());
                }
            }
        }
    }
    Ok(affected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_linked_clips_and_rejects_locked_or_transitioned_targets() {
        let mut sequence = json!({"video":[{"id":1,"name":"V1","locked":false,"transitions":[],"items":[
            {"clip":10,"item":20,"name":"Picture","start":0,"duration":100,"link":7},
            {"clip":11,"item":21,"name":"Next","start":100,"duration":100,"link":null}]}],
            "audio":[{"id":2,"name":"A1","locked":false,"transitions":[],"items":[
            {"clip":12,"item":20,"name":"Sound","start":0,"duration":100,"link":7}]}]});
        assert_eq!(resolve(&sequence, 10).unwrap().iter().map(|clip| clip.clip_id).collect::<Vec<_>>(), vec![10, 12]);
        assert!(resolve(&sequence, 999).is_err());
        sequence["audio"][0]["locked"] = json!(true);
        assert!(resolve(&sequence, 10).unwrap_err().contains("locked"));
        sequence["audio"][0]["locked"] = json!(false);
        sequence["video"][0]["transitions"] = json!([{"from":10,"to":11}]);
        assert!(resolve(&sequence, 10).unwrap_err().contains("transition"));
    }
}
