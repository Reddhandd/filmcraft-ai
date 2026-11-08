//! Resolve one enabled video clip for a built-in Lumetri color preset.

use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ColorPresetTarget {
    pub clip_id: u64,
    pub item_id: u64,
    pub clip_name: String,
    pub track_id: u64,
    pub track_name: String,
    pub preset_name: String,
    pub existing_effects: Vec<String>,
}

pub(crate) fn resolve(sequence: &Value, clip_id: u64, name: &str) -> Result<ColorPresetTarget, String> {
    if name.trim().is_empty() || name.chars().count() > 256 {
        return Err("preset name must contain 1–256 characters".into());
    }
    let tracks = sequence["video"].as_array().ok_or("video tracks unavailable")?;
    for track in tracks {
        let items = track["items"].as_array().ok_or("video clips unavailable")?;
        let Some(clip) = items.iter().find(|item| item["clip"].as_u64() == Some(clip_id)) else { continue };
        if track["locked"].as_bool() != Some(false) || track["enabled"].as_bool() != Some(true) || clip["enabled"].as_bool() != Some(true) {
            return Err("video clip or track is locked or disabled".into());
        }
        let effects = clip["effects"].as_array().ok_or("clip effects unavailable")?;
        if effects.len() >= 64 {
            return Err("clip has too many effects for a bounded controller edit".into());
        }
        let existing_effects = effects
            .iter()
            .map(|effect| effect["effect"].as_str().map(str::to_string).ok_or_else(|| "effect name unavailable".to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(ColorPresetTarget {
            clip_id,
            item_id: clip["item"].as_u64().ok_or("clip source item unavailable")?,
            clip_name: clip["name"].as_str().ok_or("clip name unavailable")?.into(),
            track_id: track["id"].as_u64().ok_or("video track ID unavailable")?,
            track_name: track["name"].as_str().ok_or("video track name unavailable")?.into(),
            preset_name: name.into(),
            existing_effects,
        });
    }
    Err(format!("video clip #{clip_id} was not found in the active sequence"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_one_enabled_video_clip_and_rejects_lock() {
        let mut sequence = json!({"video":[{"id":1,"name":"V1","locked":false,"enabled":true,
            "items":[{"clip":2,"item":5,"name":"Live","enabled":true,
                "effects":[{"effect":"motion"},{"effect":"opacity"}]}]}]});
        let target = resolve(&sequence, 2, "Golden Dusk").unwrap();
        assert_eq!(target.existing_effects, vec!["motion", "opacity"]);
        sequence["video"][0]["locked"] = json!(true);
        assert!(resolve(&sequence, 2, "Golden Dusk").unwrap_err().contains("locked"));
    }
}
