//! Resolve a bounded live-video and studio-audio match before audio analysis.

use serde::Serialize;
use serde_json::Value;

const TICKS_PER_SECOND: i64 = filmcraft_time::TICKS_PER_SECOND;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct MatchTarget {
    pub video_clip_id: u64,
    pub video_item_id: u64,
    pub video_name: String,
    pub video_track_id: u64,
    pub video_track_name: String,
    pub video_start_ticks: i64,
    pub video_duration_ticks: i64,
    pub song_clip_id: u64,
    pub song_item_id: u64,
    pub song_name: String,
    pub song_track_id: u64,
    pub song_track_name: String,
    pub song_start_ticks: i64,
    pub song_duration_ticks: i64,
    pub linked_live_audio_ids: Vec<u64>,
    pub mute_live: bool,
}

pub(crate) fn resolve(sequence: &Value, video_id: u64, song_id: u64) -> Result<MatchTarget, String> {
    if video_id == song_id {
        return Err("name a video clip and a separate studio audio clip".into());
    }
    let video_tracks = sequence["video"].as_array().ok_or("video tracks unavailable")?;
    let audio_tracks = sequence["audio"].as_array().ok_or("audio tracks unavailable")?;
    let (video_track, video) = video_tracks
        .iter()
        .find_map(|track| track["items"].as_array()?.iter().find(|item| item["clip"].as_u64() == Some(video_id)).map(|item| (track, item)))
        .ok_or_else(|| format!("video clip #{video_id} was not found"))?;
    let (song_track, song) = audio_tracks
        .iter()
        .find_map(|track| track["items"].as_array()?.iter().find(|item| item["clip"].as_u64() == Some(song_id)).map(|item| (track, item)))
        .ok_or_else(|| format!("studio audio clip #{song_id} was not found"))?;
    if video_track["locked"].as_bool() != Some(false) || video_track["enabled"].as_bool() != Some(true) {
        return Err("video track is locked or disabled".into());
    }
    if video["enabled"].as_bool() != Some(true) {
        return Err("live video clip is disabled".into());
    }
    if song_track["locked"].as_bool() != Some(false) || song_track["enabled"].as_bool() != Some(true) || song["enabled"].as_bool() != Some(true) {
        return Err("studio audio clip or track is locked or disabled".into());
    }
    let video_item_id = video["item"].as_u64().ok_or("video source item unavailable")?;
    let song_item_id = song["item"].as_u64().ok_or("studio audio source item unavailable")?;
    if video_item_id == song_item_id {
        return Err("choose a separate studio audio source".into());
    }
    let video_start = video["start"].as_i64().ok_or("video start unavailable")?;
    let video_duration = video["duration"].as_i64().ok_or("video duration unavailable")?;
    let song_start = song["start"].as_i64().ok_or("song start unavailable")?;
    let song_duration = song["duration"].as_i64().ok_or("song duration unavailable")?;
    for (clip, duration) in [(video, video_duration), (song, song_duration)] {
        if !(2 * TICKS_PER_SECOND..=30 * TICKS_PER_SECOND).contains(&duration) {
            return Err("controller performance matching supports clips from 2 to 30 seconds".into());
        }
        if clip["speed"].as_f64().is_none_or(|speed| !speed.is_finite() || (speed - 1.0).abs() > 1e-9) || clip["reverse"].as_bool() != Some(false) {
            return Err("match ordinary clips at 100% speed without reverse".into());
        }
    }
    if video_start < 0 || song_start < 0 {
        return Err("clip start cannot be negative".into());
    }
    let song_end = song_start.checked_add(song_duration).ok_or("song end overflow")?;
    for transition in video_track["transitions"].as_array().ok_or("video transitions unavailable")? {
        if transition["from"].as_u64() == Some(video_id) || transition["to"].as_u64() == Some(video_id) {
            return Err("video clip has an attached transition".into());
        }
    }
    for other in video_track["items"].as_array().ok_or("video clips unavailable")? {
        if other["clip"].as_u64() == Some(video_id) {
            continue;
        }
        let start = other["start"].as_i64().ok_or("other video start unavailable")?;
        let end = other["end"].as_i64().ok_or("other video end unavailable")?;
        if start < song_end && end > song_start {
            return Err("matched video would overlap another clip on its track".into());
        }
    }
    let mut linked = Vec::new();
    if let Some(link) = video["link"].as_u64() {
        if song["link"].as_u64() == Some(link) {
            return Err("studio song is linked to the live video; unlink it before matching".into());
        }
        for track in audio_tracks {
            for item in track["items"].as_array().ok_or("audio clips unavailable")? {
                if item["link"].as_u64() == Some(link) {
                    if track["locked"].as_bool() != Some(false) {
                        return Err("linked live audio track is locked".into());
                    }
                    linked.push(item["clip"].as_u64().ok_or("linked live audio ID unavailable")?);
                    if linked.len() > 16 {
                        return Err("too many linked live audio clips".into());
                    }
                }
            }
        }
    }
    Ok(MatchTarget {
        video_clip_id: video_id,
        video_item_id,
        video_name: video["name"].as_str().ok_or("video name unavailable")?.into(),
        video_track_id: video_track["id"].as_u64().ok_or("video track ID unavailable")?,
        video_track_name: video_track["name"].as_str().ok_or("video track name unavailable")?.into(),
        video_start_ticks: video_start,
        video_duration_ticks: video_duration,
        song_clip_id: song_id,
        song_item_id,
        song_name: song["name"].as_str().ok_or("song name unavailable")?.into(),
        song_track_id: song_track["id"].as_u64().ok_or("song track ID unavailable")?,
        song_track_name: song_track["name"].as_str().ok_or("song track name unavailable")?.into(),
        song_start_ticks: song_start,
        song_duration_ticks: song_duration,
        linked_live_audio_ids: linked,
        mute_live: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_explicit_sources_and_rejects_collisions_and_bad_ranges() {
        let four = 4 * TICKS_PER_SECOND;
        let mut sequence = json!({"video":[{"id":1,"name":"V1","locked":false,"enabled":true,
            "items":[{"clip":3,"item":5,"name":"Live","start":0,"duration":four,"end":four,
                "speed":1.0,"reverse":false,"enabled":true,"link":8}],"transitions":[]}],
            "audio":[
                {"id":2,"name":"A1","locked":false,"enabled":true,"items":[{"clip":4,"link":8}]},
                {"id":6,"name":"A2","locked":false,"enabled":true,
                    "items":[{"clip":7,"item":11,"name":"Studio","start":TICKS_PER_SECOND,
                        "duration":four,"speed":1.0,"reverse":false,"enabled":true,"link":null}]}]});
        let target = resolve(&sequence, 3, 7).unwrap();
        assert_eq!(target.linked_live_audio_ids, vec![4]);
        assert_eq!(target.song_start_ticks, TICKS_PER_SECOND);
        sequence["video"][0]["items"].as_array_mut().unwrap().push(json!({"clip":9,"start":TICKS_PER_SECOND,"end":four}));
        assert!(resolve(&sequence, 3, 7).unwrap_err().contains("overlap"));
        sequence["video"][0]["items"].as_array_mut().unwrap().pop();
        sequence["audio"][1]["items"][0]["duration"] = json!(31 * TICKS_PER_SECOND);
        assert!(resolve(&sequence, 3, 7).unwrap_err().contains("2 to 30"));
    }
}
