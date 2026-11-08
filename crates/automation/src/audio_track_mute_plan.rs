//! Resolve one explicit audio track mute change from inspected sequence state.

use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AudioTrackMute {
    pub track_id: u64,
    pub track_label: String,
    pub track_name: String,
    pub clip_count: usize,
    pub before_muted: bool,
    pub after_muted: bool,
}

pub(crate) fn resolve(sequence: &Value, label: &str, muted: bool) -> Result<AudioTrackMute, String> {
    let label = label.trim();
    let index = label
        .strip_prefix('A')
        .or_else(|| label.strip_prefix('a'))
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .ok_or("name an audio track as A1, A2, and so on")?
        - 1;
    let track = sequence["audio"].as_array().and_then(|tracks| tracks.get(index)).ok_or("audio track was not found in the active sequence")?;
    let id = track["id"].as_u64().ok_or("audio track ID unavailable")?;
    let name = track["name"].as_str().ok_or("audio track name unavailable")?;
    let before = track["muted"].as_bool().ok_or("audio track mute state unavailable")?;
    if track["locked"].as_bool() != Some(false) {
        return Err("audio track is locked; unlock it before changing mute".into());
    }
    if track["muteAutomationKeyframes"].as_u64().ok_or("audio track automation state unavailable")? != 0 {
        return Err("audio track has mute automation; this static mute change is unsupported".into());
    }
    if before == muted {
        return Err(format!("audio track {label} is already {}", if muted { "muted" } else { "unmuted" }));
    }
    let count = track["items"].as_array().ok_or("audio track clips unavailable")?.len();
    Ok(AudioTrackMute {
        track_id: id,
        track_label: format!("A{}", index + 1),
        track_name: name.into(),
        clip_count: count,
        before_muted: before,
        after_muted: muted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_one_audio_track_and_rejects_ambiguous_targets() {
        let mut sequence = json!({"audio":[{"id":7,"name":"Dialogue","locked":false,"muted":false,"muteAutomationKeyframes":0,
            "items":[{"clip":9},{"clip":10}]}]});
        let target = resolve(&sequence, "a1", true).unwrap();
        assert_eq!((target.track_id, target.clip_count, target.after_muted), (7, 2, true));
        assert!(resolve(&sequence, "all", true).is_err());
        assert!(resolve(&sequence, "A2", true).is_err());
        sequence["audio"][0]["muted"] = json!(true);
        assert!(resolve(&sequence, "A1", true).is_err());
        sequence["audio"][0]["locked"] = json!(true);
        assert!(resolve(&sequence, "A1", false).is_err());
        sequence["audio"][0]["locked"] = json!(false);
        sequence["audio"][0]["muteAutomationKeyframes"] = json!(2);
        assert!(resolve(&sequence, "A1", false).is_err());
    }
}
