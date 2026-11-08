//! Exact start alignment of one unlinked audio clip to one video clip.

use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Alignment {
    pub audio_clip_id: u64,
    pub video_clip_id: u64,
    pub anchor_ticks: i64,
    pub affected: Vec<crate::clip_move_plan::ClipMove>,
}

pub(crate) fn resolve(sequence: &Value, audio_clip_id: u64, video_clip_id: u64) -> Result<Alignment, String> {
    let video = sequence["video"]
        .as_array()
        .ok_or("video tracks unavailable")?
        .iter()
        .filter_map(|track| track["items"].as_array())
        .flat_map(|items| items.iter())
        .find(|item| item["clip"].as_u64() == Some(video_clip_id))
        .ok_or_else(|| format!("video clip #{video_clip_id} was not found"))?;
    let anchor = video["start"].as_i64().filter(|t| *t >= 0).ok_or("video clip start unavailable")?;
    let audio = sequence["audio"]
        .as_array()
        .ok_or("audio tracks unavailable")?
        .iter()
        .filter_map(|track| track["items"].as_array())
        .flat_map(|items| items.iter())
        .find(|item| item["clip"].as_u64() == Some(audio_clip_id))
        .ok_or_else(|| format!("audio clip #{audio_clip_id} was not found"))?;
    if audio["link"].as_u64().is_some() {
        return Err("audio clip is linked to another clip; this alignment requires one unlinked audio clip".into());
    }
    let affected = crate::clip_move_plan::resolve_ticks(sequence, audio_clip_id, anchor)?;
    if affected.len() != 1 || affected[0].kind != "audio" {
        return Err("alignment must affect exactly one audio clip".into());
    }
    Ok(Alignment { audio_clip_id, video_clip_id, anchor_ticks: anchor, affected })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn aligns_unlinked_audio_to_video_start_and_rejects_linked_audio() {
        let t = filmcraft_time::TICKS_PER_SECOND;
        let mut q = json!({"video":[{"id":1,"name":"V1","locked":false,"enabled":true,"transitions":[],
            "items":[{"clip":10,"start":3*t,"end":4*t,"duration":t,"link":null}]}],
            "audio":[{"id":2,"name":"A1","locked":false,"enabled":true,"transitions":[],
            "items":[{"clip":11,"start":0,"end":t,"duration":t,"link":null}]}]});
        let plan = resolve(&q, 11, 10).unwrap();
        assert_eq!((plan.anchor_ticks, plan.affected[0].to_ticks), (3 * t, 3 * t));
        q["audio"][0]["items"][0]["link"] = json!(7);
        assert!(resolve(&q, 11, 10).unwrap_err().contains("unlinked"));
        assert!(resolve(&q, 99, 10).is_err());
    }
}
