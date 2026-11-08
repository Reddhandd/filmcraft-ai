//! A deliberately narrow language layer for reviewable controller plans.
//! It never runs an edit. An MCP client can use the returned typed arguments with the
//! existing marker preview tool after the user has reviewed the plan.

use serde_json::{Value, json};

const TICKS_PER_SECOND: i64 = filmcraft_time::TICKS_PER_SECOND;

fn prefix<'a>(text: &'a str, want: &str) -> Option<&'a str> {
    text.get(..want.len()).filter(|part| part.eq_ignore_ascii_case(want)).and_then(|_| text.get(want.len()..))
}

fn split_at<'a>(text: &'a str, delimiters: &[&str]) -> Option<(&'a str, &'a str)> {
    let lower = text.to_ascii_lowercase();
    delimiters
        .iter()
        .filter_map(|delimiter| lower.find(delimiter).map(|at| (at, *delimiter)))
        .min_by_key(|(at, _)| *at)
        .and_then(|(at, delimiter)| Some((text.get(..at)?.trim(), text.get(at + delimiter.len()..)?.trim())))
}

fn valid_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.chars().count() > 256 {
        return Err("marker name must contain 1–256 characters".into());
    }
    Ok(())
}

fn valid_comment(comment: &str) -> Result<(), String> {
    if comment.chars().count() > 2048 {
        return Err("marker comment must contain at most 2048 characters".into());
    }
    Ok(())
}

fn marker<'a>(sequence: &'a Value, label: &str) -> Result<&'a Value, String> {
    let label = label.trim().trim_matches('"').trim_matches('\'');
    if label.is_empty() {
        return Err("name a marker by its ID or exact name".into());
    }
    let markers = sequence["markers"].as_array().ok_or("active sequence markers unavailable")?;
    let id = label.trim_start_matches('#').parse::<u64>().ok();
    let mut matches = markers.iter().filter(|marker| {
        id.map_or_else(|| marker["name"].as_str().is_some_and(|name| name.eq_ignore_ascii_case(label)), |id| marker["id"].as_u64() == Some(id))
    });
    let found = matches.next().ok_or_else(|| format!("marker {label:?} was not found in the active sequence"))?;
    if matches.next().is_some() {
        return Err(format!("marker name {label:?} is ambiguous; use a marker ID"));
    }
    Ok(found)
}

fn time_at(text: &str, sequence: &Value) -> Result<(i64, f64, &'static str), String> {
    let text = text.trim();
    let text = prefix(text, "the ").unwrap_or(text);
    let (ticks, source) = if text.eq_ignore_ascii_case("playhead") {
        (sequence["playhead"].as_i64().ok_or("active playhead unavailable")?, "playhead")
    } else {
        let lower = text.to_ascii_lowercase();
        let number = lower
            .strip_suffix(" seconds")
            .or_else(|| lower.strip_suffix(" second"))
            .or_else(|| lower.strip_suffix('s'))
            .ok_or("use a time in seconds, such as '3.5 seconds', or 'the playhead'")?;
        let seconds: f64 = number.trim().parse().map_err(|_| "time must be a number of seconds".to_string())?;
        if !seconds.is_finite() || seconds < 0.0 || seconds > i64::MAX as f64 / TICKS_PER_SECOND as f64 {
            return Err("time must be finite, non-negative, and within the supported range".into());
        }
        ((seconds * TICKS_PER_SECOND as f64).round() as i64, "explicit_seconds")
    };
    let duration = sequence["duration"].as_i64().ok_or("active sequence duration unavailable")?;
    if ticks < 0 || ticks > duration {
        return Err("marker time is outside the active sequence".into());
    }
    let seconds = ticks as f64 / TICKS_PER_SECOND as f64;
    if (seconds * TICKS_PER_SECOND as f64).round() as i64 != ticks {
        return Err("marker time cannot be represented exactly through the marker preview API".into());
    }
    Ok((ticks, seconds, source))
}

pub(crate) fn plan(request: &str, project: &Value, sequence: &Value) -> Result<Value, String> {
    let request = request.trim();
    if request.is_empty() || request.chars().count() > 2048 {
        return Err("request must contain 1–2048 characters".into());
    }
    let revision = project["revision"].as_u64().ok_or("project revision unavailable")?;
    let sequence_id = sequence["id"].as_u64().ok_or("active sequence ID unavailable")?;
    if project["activeSequence"].as_u64() != Some(sequence_id) {
        return Err("active sequence changed while planning; inspect again".into());
    }
    let base = json!({
        "request": request,
        "preview_only": true,
        "project": {"name": project["name"], "path": project["path"], "revision": revision},
        "sequence": {"id": sequence_id, "name": sequence["name"]},
        "risk": "reversible",
        "undo": "edit.undo",
        "next_step": "Review this plan, then call the named preview tool with the exact arguments. Commit only its returned token after approval."
    });

    for lead in ["add a marker at ", "add marker at ", "place a marker at "] {
        if let Some(rest) = prefix(request, lead) {
            let (at, named) = split_at(rest, &[" called ", " named "])
                .ok_or("say where and what to call the marker, for example: Add a marker at the playhead called Opening")?;
            let (name, comment) = split_at(named, &[" with comment "]).map_or((named, None), |(name, comment)| (name, Some(comment)));
            valid_name(name)?;
            if let Some(comment) = comment {
                valid_comment(comment)?;
            }
            let (ticks, seconds, source) = time_at(at, sequence)?;
            let mut result = base;
            result["action"] = json!("add_marker");
            result["target"] = json!({"sequence_id": sequence_id, "time_ticks": ticks, "seconds": seconds, "time_source": source});
            result["after"] = json!({"name": name, "comment": comment.unwrap_or("")});
            result["execution"] = json!({"tool": "marker_add_preview", "arguments": {"sequence_id": sequence_id, "revision": revision, "seconds": seconds, "name": name, "comment": comment.unwrap_or("")}});
            return Ok(result);
        }
    }

    if let Some(rest) = prefix(request, "move clip ") {
        let (label, destination) = split_at(rest, &[" to "]).ok_or("say which clip ID to move and a destination in seconds")?;
        let clip_id = label
            .trim()
            .trim_start_matches('#')
            .parse::<u64>()
            .map_err(|_| "name the clip by its numeric ID, such as 'Move clip #47 to 30 seconds'".to_string())?;
        let seconds = crate::clip_move_plan::seconds_from_text(destination)?;
        let affected = crate::clip_move_plan::resolve(sequence, clip_id, seconds)?;
        let mut result = base;
        result["action"] = json!("move_clip");
        result["target"] = json!({"clip_id":clip_id,"affected":affected});
        result["execution"] =
            json!({"tool":"clip_move_preview","arguments":{"sequence_id":sequence_id,"revision":revision,"clip_id":clip_id,"seconds":seconds}});
        return Ok(result);
    }

    for lead in ["delete clip ", "remove clip "] {
        if let Some(label) = prefix(request, lead) {
            let clip_id =
                label.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the clip by its numeric ID, such as 'Delete clip #47'".to_string())?;
            let affected = crate::clip_delete_plan::resolve(sequence, clip_id)?;
            let mut result = base;
            result["action"] = json!("delete_clip");
            result["target"] = json!({"clip_id":clip_id,"affected":affected,"gap_preserved":true});
            result["execution"] = json!({"tool":"clip_delete_preview","arguments":{
                "sequence_id":sequence_id,"revision":revision,"clip_id":clip_id}});
            return Ok(result);
        }
    }

    if let Some(rest) = prefix(request, "place audio item ") {
        let (label, destination) = split_at(rest, &[" on "]).ok_or("say which audio item ID and track, such as 'Place audio item #11 on A3 at 30 seconds'")?;
        let (track, time) = split_at(destination, &[" at "]).ok_or("say which audio track and timeline time")?;
        let item_id = label.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the audio item by its numeric project ID".to_string())?;
        let seconds = crate::clip_move_plan::seconds_from_text(time)?;
        let placement = crate::audio_place_plan::resolve(project, sequence, item_id, track, seconds)?;
        let mut result = base;
        result["action"] = json!("place_audio");
        result["target"] = json!(placement);
        result["execution"] = json!({"tool":"audio_place_preview","arguments":{"sequence_id":sequence_id,"revision":revision,
            "item_id":item_id,"track":track,"seconds":seconds}});
        return Ok(result);
    }

    for (lead, muted) in [("mute audio track ", true), ("unmute audio track ", false)] {
        if let Some(label) = prefix(request, lead) {
            let target = crate::audio_track_mute_plan::resolve(sequence, label, muted)?;
            let mut result = base;
            result["action"] = json!("set_audio_track_mute");
            result["target"] = json!(target);
            result["execution"] = json!({"tool":"audio_track_mute_preview","arguments":{
                "sequence_id":sequence_id,"revision":revision,"track":target.track_label,"muted":muted}});
            return Ok(result);
        }
    }

    if let Some(rest) = prefix(request, "align audio clip ") {
        let (audio, video) = split_at(rest, &[" to video clip "]).ok_or("name both clip IDs, such as 'Align audio clip #47 to video clip #35'")?;
        let audio_clip_id = audio.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the audio clip by its numeric ID".to_string())?;
        let video_clip_id = video.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the video clip by its numeric ID".to_string())?;
        let alignment = crate::audio_video_align_plan::resolve(sequence, audio_clip_id, video_clip_id)?;
        let mut result = base;
        result["action"] = json!("align_audio_to_video");
        result["target"] = json!(alignment);
        result["execution"] = json!({"tool":"audio_align_preview","arguments":{
            "sequence_id":sequence_id,"revision":revision,"audio_clip_id":audio_clip_id,"video_clip_id":video_clip_id}});
        return Ok(result);
    }

    if let Some(rest) = prefix(request, "nest clip ") {
        let (label, name) = split_at(rest, &[" as "]).ok_or("name the clip and new sequence, such as 'Nest clip #46 as Overlay Nest'")?;
        let clip_id = label.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the video clip by its numeric ID".to_string())?;
        let target = crate::nest_clip_plan::resolve(project, sequence, clip_id, name)?;
        let mut result = base;
        result["action"] = json!("nest_clip");
        result["target"] = json!(target);
        result["execution"] = json!({"tool":"nest_clip_preview","arguments":{
            "sequence_id":sequence_id,"revision":revision,"clip_id":clip_id,"name":name}});
        return Ok(result);
    }

    if let Some(rest) = prefix(request, "match performance of video clip ") {
        let (video, song) = split_at(rest, &[" to song clip "]).ok_or("name both clip IDs, such as 'Match performance of video clip #51 to song clip #53'")?;
        let video_id = video.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the video clip by its numeric ID".to_string())?;
        let song_id = song.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the song clip by its numeric ID".to_string())?;
        let target = crate::performance_match_plan::resolve(sequence, video_id, song_id)?;
        let mut result = base;
        result["action"] = json!("match_performance");
        result["target"] = json!(target);
        result["execution"] = json!({"tool":"performance_match_preview","arguments":{
            "sequence_id":sequence_id,"revision":revision,"video_clip_id":video_id,"song_clip_id":song_id}});
        return Ok(result);
    }

    if let Some(rest) = prefix(request, "apply color preset ") {
        let (name, label) = split_at(rest, &[" to clip "]).ok_or("name the preset and video clip ID, such as 'Apply color preset Golden Dusk to clip #32'")?;
        let clip_id = label.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the video clip by its numeric ID".to_string())?;
        let target = crate::color_preset_plan::resolve(sequence, clip_id, name)?;
        let mut result = base;
        result["action"] = json!("apply_color_preset");
        result["target"] = json!(target);
        result["execution"] = json!({"tool":"color_preset_preview","arguments":{
            "sequence_id":sequence_id,"revision":revision,"clip_id":clip_id,"name":name}});
        return Ok(result);
    }

    if let Some(rest) = prefix(request, "apply video effect ") {
        let (name, label) =
            split_at(rest, &[" to clip "]).ok_or("name the effect and video clip ID, such as 'Apply video effect Gaussian Blur to clip #32'")?;
        let clip_id = label.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the video clip by its numeric ID".to_string())?;
        let target = crate::video_effect_plan::resolve(sequence, clip_id, name)?;
        let mut result = base;
        result["action"] = json!("apply_video_effect");
        result["target"] = json!(target);
        result["execution"] = json!({"tool":"video_effect_preview","arguments":{
            "sequence_id":sequence_id,"revision":revision,"clip_id":clip_id,"name":name}});
        return Ok(result);
    }

    if let Some(rest) = prefix(request, "move keyframe for parameter ") {
        let (param_id, rest) = split_at(rest, &[" of effect "])
            .ok_or("use 'Move keyframe for parameter blurriness of effect gaussian_blur on clip #32 from 0.001 seconds to 0.002 seconds'")?;
        let (effect_id, rest) = split_at(rest, &[" on clip "]).ok_or("name the effect ID and video clip ID")?;
        let (clip_text, rest) = split_at(rest, &[" from "]).ok_or("give the keyframe's clip-relative source time")?;
        let (from_text, to_text) = split_at(rest, &[" to "]).ok_or("give the keyframe's clip-relative destination time")?;
        let clip_id = clip_text.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the video clip by its numeric ID".to_string())?;
        let parse_time = |text: &str| -> Result<i64, String> {
            let lower = text.trim().to_ascii_lowercase();
            let number = lower
                .strip_suffix(" seconds")
                .or_else(|| lower.strip_suffix(" second"))
                .or_else(|| lower.strip_suffix('s'))
                .ok_or("give clip-relative time in seconds, such as '0.001 seconds'")?;
            let seconds = number.trim().parse::<f64>().map_err(|_| "keyframe time must be a number of seconds".to_string())?;
            if !seconds.is_finite() || seconds < 0.0 || seconds >= i64::MAX as f64 / TICKS_PER_SECOND as f64 {
                return Err("keyframe time must be finite, non-negative, and within the supported range".into());
            }
            Ok((seconds * TICKS_PER_SECOND as f64).round() as i64)
        };
        let clip_time_ticks = parse_time(from_text)?;
        let to_clip_time_ticks = parse_time(to_text)?;
        let target = crate::effect_param_plan::resolve_move_keyframe(sequence, clip_id, effect_id, param_id, clip_time_ticks, to_clip_time_ticks)?;
        let mut result = base;
        result["action"] = json!("move_effect_keyframe");
        result["target"] = json!(target);
        result["execution"] = json!({"tool":"effect_param_preview","arguments":{
            "sequence_id":sequence_id,"revision":revision,"clip_id":clip_id,"effect_id":effect_id,
            "param_id":param_id,"value":null,"clip_time_ticks":clip_time_ticks,
            "to_clip_time_ticks":to_clip_time_ticks}});
        return Ok(result);
    }

    if let Some(rest) = prefix(request, "set interpolation for parameter ") {
        let (param_id, rest) = split_at(rest, &[" of effect "])
            .ok_or("use 'Set interpolation for parameter blurriness of effect gaussian_blur on clip #32 at 0 seconds to Hold'")?;
        let (effect_id, rest) = split_at(rest, &[" on clip "]).ok_or("name the effect ID and video clip ID")?;
        let (clip_text, rest) = split_at(rest, &[" at "]).ok_or("give the clip-relative time of the keyframe")?;
        let (time_text, interpolation) = split_at(rest, &[" to "]).ok_or("give the new interpolation mode")?;
        let clip_id = clip_text.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the video clip by its numeric ID".to_string())?;
        let seconds_text = time_text.trim().to_ascii_lowercase();
        let seconds_text = seconds_text
            .strip_suffix(" seconds")
            .or_else(|| seconds_text.strip_suffix(" second"))
            .or_else(|| seconds_text.strip_suffix('s'))
            .ok_or("give clip-relative time in seconds, such as '0.001 seconds'")?;
        let seconds = seconds_text.trim().parse::<f64>().map_err(|_| "keyframe time must be a number of seconds".to_string())?;
        if !seconds.is_finite() || seconds < 0.0 || seconds >= i64::MAX as f64 / TICKS_PER_SECOND as f64 {
            return Err("keyframe time must be finite, non-negative, and within the supported range".into());
        }
        let clip_time_ticks = (seconds * TICKS_PER_SECOND as f64).round() as i64;
        let target = crate::effect_param_plan::resolve_interpolation(sequence, clip_id, effect_id, param_id, clip_time_ticks, interpolation)?;
        let mut result = base;
        result["action"] = json!("set_effect_keyframe_interpolation");
        result["target"] = json!(target);
        result["execution"] = json!({"tool":"effect_param_preview","arguments":{
            "sequence_id":sequence_id,"revision":revision,"clip_id":clip_id,"effect_id":effect_id,
            "param_id":param_id,"value":null,"clip_time_ticks":clip_time_ticks,
            "interpolation":interpolation}});
        return Ok(result);
    }

    if let Some(rest) = prefix(request, "remove keyframe for parameter ") {
        let (param_id, rest) =
            split_at(rest, &[" of effect "]).ok_or("use 'Remove keyframe for parameter blurriness of effect gaussian_blur on clip #32 at 0.001 seconds'")?;
        let (effect_id, rest) = split_at(rest, &[" on clip "]).ok_or("name the effect ID and video clip ID")?;
        let (clip_text, time_text) = split_at(rest, &[" at "]).ok_or("give the clip-relative time of the keyframe to remove")?;
        let clip_id = clip_text.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the video clip by its numeric ID".to_string())?;
        let seconds_text = time_text.trim().to_ascii_lowercase();
        let seconds_text = seconds_text
            .strip_suffix(" seconds")
            .or_else(|| seconds_text.strip_suffix(" second"))
            .or_else(|| seconds_text.strip_suffix('s'))
            .ok_or("give clip-relative time in seconds, such as '0.001 seconds'")?;
        let seconds = seconds_text.trim().parse::<f64>().map_err(|_| "keyframe time must be a number of seconds".to_string())?;
        if !seconds.is_finite() || seconds < 0.0 || seconds >= i64::MAX as f64 / TICKS_PER_SECOND as f64 {
            return Err("keyframe time must be finite, non-negative, and within the supported range".into());
        }
        let clip_time_ticks = (seconds * TICKS_PER_SECOND as f64).round() as i64;
        let target = crate::effect_param_plan::resolve_remove_keyframe(sequence, clip_id, effect_id, param_id, clip_time_ticks)?;
        let mut result = base;
        result["action"] = json!("remove_effect_keyframe");
        result["target"] = json!(target);
        result["execution"] = json!({"tool":"effect_param_preview","arguments":{
            "sequence_id":sequence_id,"revision":revision,"clip_id":clip_id,"effect_id":effect_id,
            "param_id":param_id,"value":null,"clip_time_ticks":clip_time_ticks,
            "remove_existing_keyframe":true}});
        return Ok(result);
    }

    for (lead, edit_existing) in [("add keyframe for parameter ", false), ("change keyframe for parameter ", true)] {
        if let Some(rest) = prefix(request, lead) {
            let (param_id, rest) = split_at(rest, &[" of effect "])
                .ok_or("use 'Change keyframe for parameter blurriness of effect gaussian_blur on clip #32 at 0 seconds to 75'")?;
            let (effect_id, rest) = split_at(rest, &[" on clip "]).ok_or("name the effect ID and video clip ID")?;
            let (clip_text, rest) = split_at(rest, &[" at "]).ok_or("give a time inside the clip")?;
            let (time_text, value_text) = split_at(rest, &[" to "]).ok_or("give the keyframe's numeric value")?;
            let clip_id = clip_text.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the video clip by its numeric ID".to_string())?;
            let seconds_text = time_text.trim().to_ascii_lowercase();
            let seconds_text = seconds_text
                .strip_suffix(" seconds")
                .or_else(|| seconds_text.strip_suffix(" second"))
                .or_else(|| seconds_text.strip_suffix('s'))
                .ok_or("give clip-relative time in seconds, such as '1 second'")?;
            let seconds = seconds_text.trim().parse::<f64>().map_err(|_| "keyframe time must be a number of seconds".to_string())?;
            if !seconds.is_finite() || seconds < 0.0 || seconds >= i64::MAX as f64 / TICKS_PER_SECOND as f64 {
                return Err("keyframe time must be finite, non-negative, and within the supported range".into());
            }
            let clip_time_ticks = (seconds * TICKS_PER_SECOND as f64).round() as i64;
            let value = value_text.parse::<f64>().map_err(|_| "keyframe value must be a number".to_string())?;
            if !value.is_finite() {
                return Err("keyframe value must be finite".into());
            }
            let target = if edit_existing {
                crate::effect_param_plan::resolve_existing_keyframe(sequence, clip_id, effect_id, param_id, json!(value), clip_time_ticks)?
            } else {
                crate::effect_param_plan::resolve_keyframe(sequence, clip_id, effect_id, param_id, json!(value), clip_time_ticks)?
            };
            let mut result = base;
            result["action"] = json!(if edit_existing { "change_effect_keyframe" } else { "add_effect_keyframe" });
            result["target"] = json!(target);
            result["execution"] = json!({"tool":"effect_param_preview","arguments":{
            "sequence_id":sequence_id,"revision":revision,"clip_id":clip_id,"effect_id":effect_id,
            "param_id":param_id,"value":value,"clip_time_ticks":clip_time_ticks,
            "edit_existing_keyframe":edit_existing}});
            return Ok(result);
        }
    }

    if let Some(rest) = prefix(request, "set parameter ") {
        let (param_id, rest) = split_at(rest, &[" of effect "]).ok_or("use 'Set parameter blurriness of effect gaussian_blur on clip #32 to 25'")?;
        let (effect_id, rest) = split_at(rest, &[" on clip "]).ok_or("name the effect ID and video clip ID")?;
        let (label, value_text) = split_at(rest, &[" to "]).ok_or("give a numeric value after 'to'")?;
        let clip_id = label.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the video clip by its numeric ID".to_string())?;
        let value = if value_text.eq_ignore_ascii_case("true") {
            json!(true)
        } else if value_text.eq_ignore_ascii_case("false") {
            json!(false)
        } else if let Ok(number) = value_text.parse::<f64>() {
            if !number.is_finite() {
                return Err("parameter value must be finite".into());
            }
            json!(number)
        } else {
            json!(value_text)
        };
        let target = crate::effect_param_plan::resolve(sequence, clip_id, effect_id, param_id, value.clone())?;
        let mut result = base;
        result["action"] = json!("set_effect_parameter");
        result["target"] = json!(target);
        result["execution"] = json!({"tool":"effect_param_preview","arguments":{
            "sequence_id":sequence_id,"revision":revision,"clip_id":clip_id,"effect_id":effect_id,
            "param_id":param_id,"value":value}});
        return Ok(result);
    }

    if let Some(rest) = prefix(request, "remove video effect ") {
        let (effect_id, label) = split_at(rest, &[" from clip "]).ok_or("use 'Remove video effect gaussian_blur from clip #32'")?;
        let clip_id = label.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the video clip by its numeric ID".to_string())?;
        let target = crate::video_effect_plan::resolve_removal(sequence, clip_id, effect_id)?;
        let mut result = base;
        result["action"] = json!("remove_video_effect");
        result["target"] = json!(target);
        result["execution"] = json!({"tool":"video_effect_remove_preview","arguments":{
            "sequence_id":sequence_id,"revision":revision,"clip_id":clip_id,"effect_id":effect_id}});
        return Ok(result);
    }

    if let Some(rest) = prefix(request, "add cross dissolve after clip ") {
        let (label, duration) =
            split_at(rest, &[" for "]).ok_or("say which video clip and an even frame count, such as 'Add cross dissolve after clip #32 for 12 frames'")?;
        let clip_id = label.trim().trim_start_matches('#').parse::<u64>().map_err(|_| "name the video clip by its numeric ID".to_string())?;
        let frame_text = duration.to_ascii_lowercase();
        let frame_text = frame_text.strip_suffix(" frames").or_else(|| frame_text.strip_suffix(" frame")).ok_or("use a duration in frames")?;
        let frames = frame_text.trim().parse::<i64>().map_err(|_| "frame count must be an integer".to_string())?;
        let transition = crate::transition_add_plan::resolve(sequence, clip_id, frames)?;
        let mut result = base;
        result["action"] = json!("add_cross_dissolve");
        result["target"] = json!(transition);
        result["execution"] = json!({"tool":"transition_add_preview","arguments":{
            "sequence_id":sequence_id,"revision":revision,"clip_id":clip_id,"frames":frames}});
        return Ok(result);
    }

    if let Some(rest) = prefix(request, "rename marker ") {
        let (label, name) = split_at(rest, &[" to "]).ok_or("say which marker to rename and its new name")?;
        valid_name(name)?;
        let found = marker(sequence, label)?;
        let marker_id = found["id"].as_u64().ok_or("marker ID unavailable")?;
        let mut result = base;
        result["action"] = json!("edit_marker");
        result["target"] = json!({"sequence_id": sequence_id, "marker_id": marker_id, "time_ticks": found["start"]});
        result["before"] = json!({"name": found["name"], "comment": found["comment"]});
        result["after"] = json!({"name": name, "comment": found["comment"]});
        result["execution"] =
            json!({"tool": "marker_edit_preview", "arguments": {"sequence_id": sequence_id, "revision": revision, "marker_id": marker_id, "name": name}});
        return Ok(result);
    }

    if let Some(rest) = prefix(request, "set comment on marker ") {
        let (label, comment) = split_at(rest, &[" to "]).ok_or("say which marker to comment on and the comment text")?;
        valid_comment(comment)?;
        let found = marker(sequence, label)?;
        let marker_id = found["id"].as_u64().ok_or("marker ID unavailable")?;
        let mut result = base;
        result["action"] = json!("edit_marker");
        result["target"] = json!({"sequence_id": sequence_id, "marker_id": marker_id, "time_ticks": found["start"]});
        result["before"] = json!({"name": found["name"], "comment": found["comment"]});
        result["after"] = json!({"name": found["name"], "comment": comment});
        result["execution"] =
            json!({"tool": "marker_edit_preview", "arguments": {"sequence_id": sequence_id, "revision": revision, "marker_id": marker_id, "comment": comment}});
        return Ok(result);
    }

    Err("unsupported request; try 'Add a marker at the playhead called Opening', 'Move clip #47 to 30 seconds', 'Delete clip #46', 'Place audio item #11 on A3 at 30 seconds', 'Add cross dissolve after clip #32 for 12 frames', 'Align audio clip #47 to video clip #35', 'Nest clip #46 as Overlay Nest', 'Match performance of video clip #51 to song clip #53', 'Apply color preset Golden Dusk to clip #32', 'Apply video effect Gaussian Blur to clip #32', 'Set parameter blurriness of effect gaussian_blur on clip #32 to 25', or 'Remove video effect gaussian_blur from clip #32'"
        .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> (Value, Value) {
        (
            json!({"name":"Test","path":null,"revision":4,"activeSequence":21}),
            json!({"id":21,"name":"Edit","duration":TICKS_PER_SECOND * 10,"playhead":TICKS_PER_SECOND * 2,
                "markers":[{"id":8,"name":"Opening beat","comment":"old","start":0},{"id":9,"name":"Opening beat","comment":"other","start":TICKS_PER_SECOND}]}),
        )
    }

    #[test]
    fn resolves_marker_requests_without_guessing_a_target() {
        let (p, q) = context();
        let add = plan("Add a marker at the playhead called Chorus with comment Cut here", &p, &q).unwrap();
        assert_eq!(add["target"]["time_ticks"], TICKS_PER_SECOND * 2);
        assert_eq!(add["execution"]["arguments"]["revision"], 4);
        assert_eq!(add["after"]["comment"], "Cut here");
        assert_eq!(plan("Add marker at 12 seconds named Too late", &p, &q).unwrap_err(), "marker time is outside the active sequence");
        assert!(plan("Rename marker Opening beat to Intro", &p, &q).unwrap_err().contains("ambiguous"));
        let rename = plan("Rename marker #8 to Intro", &p, &q).unwrap();
        assert_eq!(rename["target"]["marker_id"], 8);
        assert_eq!(rename["before"]["name"], "Opening beat");
        assert_eq!(rename["after"]["name"], "Intro");
        let comment = plan("Set comment on marker 9 to Downbeat", &p, &q).unwrap();
        assert_eq!(comment["execution"]["arguments"]["comment"], "Downbeat");
        assert!(plan("Delete everything", &p, &q).is_err());
        assert!(plan("Add marker at NaN seconds named Bad", &p, &q).is_err());
        assert!(plan("Add marker at -1 seconds named Bad", &p, &q).is_err());
        assert!(plan(&format!("Add marker at 1 second named {}", "A".repeat(257)), &p, &q).is_err());
        assert!(plan("Rename marker #999 to Lost", &p, &q).is_err());
        let mut different_project = p.clone();
        different_project["activeSequence"] = json!(22);
        assert!(plan("Add marker at 1 second named Bad", &different_project, &q).is_err());
    }

    #[test]
    fn plans_a_clip_move_with_exact_partner_targets() {
        let (p, mut q) = context();
        let t = TICKS_PER_SECOND;
        q["video"] = json!([{"id":1,"name":"V1","locked":false,"enabled":true,"transitions":[],
            "items":[{"clip":10,"name":"Shot","start":0,"end":t,"duration":t,"link":7}]}]);
        q["audio"] = json!([{"id":2,"name":"A1","locked":false,"enabled":true,"transitions":[],
            "items":[{"clip":11,"name":"Sound","start":0,"end":t,"duration":t,"link":7}]}]);
        let move_plan = plan("Move clip #10 to 3 seconds", &p, &q).unwrap();
        assert_eq!(move_plan["execution"]["tool"], "clip_move_preview");
        assert_eq!(move_plan["target"]["affected"].as_array().unwrap().len(), 2);
        assert_eq!(move_plan["target"]["affected"][1]["to_ticks"], 3 * t);
        assert!(plan("Move clip #10 to -1 seconds", &p, &q).is_err());
        assert!(plan("Move clip #999 to 3 seconds", &p, &q).is_err());
    }
}
