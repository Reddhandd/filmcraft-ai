//! Resolve one scalar parameter or bounded numeric keyframe on one standard video effect.

use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct EffectParamTarget {
    pub clip_id: u64,
    pub item_id: u64,
    pub clip_name: String,
    pub track_id: u64,
    pub track_name: String,
    pub effect_index: usize,
    pub effect_id: String,
    pub param_id: String,
    pub before_value: String,
    pub after_value: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip_time_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_time_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_clip_time_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_media_time_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_keyframe_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove_keyframe: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_interpolation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_interpolation: Option<String>,
}

pub(crate) fn resolve(sequence: &Value, clip_id: u64, effect_id: &str, param_id: &str, value: Value) -> Result<EffectParamTarget, String> {
    resolve_for_mode(sequence, clip_id, effect_id, param_id, value, false)
}

fn resolve_for_mode(sequence: &Value, clip_id: u64, effect_id: &str, param_id: &str, value: Value, keyframe: bool) -> Result<EffectParamTarget, String> {
    if effect_id.is_empty()
        || effect_id.len() > 128
        || !effect_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || param_id.is_empty()
        || param_id.len() > 128
        || !param_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("use the exact effect and parameter IDs from the catalog".into());
    }
    if value.as_f64().is_some_and(|number| !number.is_finite()) || !value.is_number() && !value.is_boolean() && !value.is_string() {
        return Err("parameter value must be a finite number, true/false, or a menu option".into());
    }
    if value.as_str().is_some_and(|label| label.trim().is_empty() || label.chars().count() > 128) {
        return Err("menu option must contain 1–128 characters".into());
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
        let mut matches = effects.iter().enumerate().filter(|(_, effect)| effect["effect"].as_str() == Some(effect_id));
        let (effect_index, effect) = matches.next().ok_or_else(|| format!("effect {effect_id:?} is absent from video clip #{clip_id}"))?;
        if matches.next().is_some() {
            return Err(format!("effect {effect_id:?} appears more than once; choose a clip with one instance"));
        }
        if effect["enabled"].as_bool() != Some(true) || effect["masks"].as_u64() != Some(0) {
            return Err("effect must be enabled and have no masks for static parameter editing".into());
        }
        let param = &effect["params"][param_id];
        let before = param["value"].as_str().ok_or("effect parameter is missing")?;
        if !keyframe && param["keyframes"].as_u64() != Some(0) {
            return Err("animated effect parameters are not supported by this action".into());
        }
        if let Some(current) = before.strip_prefix("Float(").and_then(|s| s.strip_suffix(')')).and_then(|s| s.parse::<f64>().ok()) {
            let requested = value.as_f64().ok_or("numeric slider requires a number")?;
            if !current.is_finite() || (!keyframe && current == requested) {
                return Err("parameter already has that value or is not finite".into());
            }
        } else if let Some(current) = before.strip_prefix("Bool(").and_then(|s| s.strip_suffix(')')).and_then(|s| s.parse::<bool>().ok()) {
            let requested = value.as_bool().ok_or("switch parameter requires true or false")?;
            if current == requested {
                return Err("parameter already has that value".into());
            }
        } else if before.strip_prefix("Choice(").and_then(|s| s.strip_suffix(')')).and_then(|s| s.parse::<u32>().ok()).is_some() {
            if value.as_str().is_none() {
                return Err("menu choice requires an option label".into());
            }
        } else {
            return Err("only static numeric sliders, switches, and menu choices are supported".into());
        }
        return Ok(EffectParamTarget {
            clip_id,
            item_id: clip["item"].as_u64().ok_or("clip source item unavailable")?,
            clip_name: clip["name"].as_str().ok_or("clip name unavailable")?.into(),
            track_id: track["id"].as_u64().ok_or("video track ID unavailable")?,
            track_name: track["name"].as_str().ok_or("video track name unavailable")?.into(),
            effect_index,
            effect_id: effect_id.into(),
            param_id: param_id.into(),
            before_value: before.into(),
            after_value: value,
            clip_time_ticks: None,
            media_time_ticks: None,
            to_clip_time_ticks: None,
            to_media_time_ticks: None,
            before_keyframe_value: None,
            remove_keyframe: None,
            before_interpolation: None,
            after_interpolation: None,
        });
    }
    Err(format!("video clip #{clip_id} was not found in the active sequence"))
}

pub(crate) fn resolve_keyframe(
    sequence: &Value,
    clip_id: u64,
    effect_id: &str,
    param_id: &str,
    value: Value,
    clip_time_ticks: i64,
) -> Result<EffectParamTarget, String> {
    let mut target = resolve_for_mode(sequence, clip_id, effect_id, param_id, value, true)?;
    if !target.after_value.is_number() {
        return Err("keyframe value must be a numeric slider value".into());
    }
    let (media_time_ticks, param) = keyframe_context(sequence, &target, clip_time_ticks)?;
    let count = param["keyframes"].as_u64().ok_or("keyframe count unavailable")?;
    let details = param["keyframeDetails"].as_array().ok_or("keyframe details unavailable")?;
    if count >= 64 || details.len() != count as usize {
        return Err("existing keyframes exceed the bounded edit limit or are unavailable".into());
    }
    let mut previous = None;
    for point in details {
        let time = point["time"].as_i64().ok_or("keyframe time unavailable")?;
        if previous.is_some_and(|prior| time <= prior) {
            return Err("existing keyframes are not strictly ordered".into());
        }
        if time == media_time_ticks {
            return Err("a keyframe already exists at that clip time".into());
        }
        previous = Some(time);
    }
    target.clip_time_ticks = Some(clip_time_ticks);
    target.media_time_ticks = Some(media_time_ticks);
    Ok(target)
}

pub(crate) fn resolve_existing_keyframe(
    sequence: &Value,
    clip_id: u64,
    effect_id: &str,
    param_id: &str,
    value: Value,
    clip_time_ticks: i64,
) -> Result<EffectParamTarget, String> {
    let mut target = resolve_for_mode(sequence, clip_id, effect_id, param_id, value, true)?;
    let requested = target.after_value.as_f64().ok_or("keyframe value must be a number")?;
    let (media_time_ticks, param) = keyframe_context(sequence, &target, clip_time_ticks)?;
    let count = param["keyframes"].as_u64().ok_or("keyframe count unavailable")?;
    let details = param["keyframeDetails"].as_array().ok_or("keyframe details unavailable")?;
    if count == 0 || count > 64 || details.len() != count as usize {
        return Err("existing keyframes are missing or exceed the bounded edit limit".into());
    }
    let mut matches = details.iter().filter(|point| point["time"].as_i64() == Some(media_time_ticks));
    let point = matches.next().ok_or("no keyframe exists at that clip time")?;
    if matches.next().is_some() {
        return Err("more than one keyframe exists at that clip time".into());
    }
    let before = point["value"].as_str().ok_or("keyframe value unavailable")?;
    let current = before
        .strip_prefix("Float(")
        .and_then(|s| s.strip_suffix(')'))
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|n| n.is_finite())
        .ok_or("existing keyframe is not a finite numeric slider value")?;
    if current == requested {
        return Err("keyframe already has that value".into());
    }
    target.clip_time_ticks = Some(clip_time_ticks);
    target.media_time_ticks = Some(media_time_ticks);
    target.before_keyframe_value = Some(before.into());
    Ok(target)
}

pub(crate) fn resolve_remove_keyframe(
    sequence: &Value,
    clip_id: u64,
    effect_id: &str,
    param_id: &str,
    clip_time_ticks: i64,
) -> Result<EffectParamTarget, String> {
    let mut target = resolve_for_mode(sequence, clip_id, effect_id, param_id, Value::from(0.0), true)?;
    let (media_time_ticks, param) = keyframe_context(sequence, &target, clip_time_ticks)?;
    let count = param["keyframes"].as_u64().ok_or("keyframe count unavailable")?;
    let details = param["keyframeDetails"].as_array().ok_or("keyframe details unavailable")?;
    if !(2..=64).contains(&count) || details.len() != count as usize {
        return Err("keyframe removal requires 2–64 existing points so animation remains active".into());
    }
    let mut previous = None;
    let mut found = None;
    for point in details {
        let time = point["time"].as_i64().ok_or("keyframe time unavailable")?;
        if previous.is_some_and(|prior| time <= prior) {
            return Err("existing keyframes are not strictly ordered".into());
        }
        if time == media_time_ticks {
            let value = point["value"].as_str().ok_or("keyframe value unavailable")?;
            value
                .strip_prefix("Float(")
                .and_then(|s| s.strip_suffix(')'))
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|n| n.is_finite())
                .ok_or("existing keyframe is not a finite numeric slider value")?;
            found = Some(value.to_string());
        }
        previous = Some(time);
    }
    target.before_keyframe_value = Some(found.ok_or("no keyframe exists at that clip time")?);
    target.clip_time_ticks = Some(clip_time_ticks);
    target.media_time_ticks = Some(media_time_ticks);
    target.after_value = Value::Null;
    target.remove_keyframe = Some(true);
    Ok(target)
}

pub(crate) fn interpolation_name(name: &str) -> Result<&'static str, String> {
    match name.trim().to_ascii_lowercase().replace(' ', "").as_str() {
        "linear" => Ok("Linear"),
        "bezier" => Ok("Bezier"),
        "autobezier" => Ok("AutoBezier"),
        "continuousbezier" => Ok("ContinuousBezier"),
        "hold" => Ok("Hold"),
        "easein" => Ok("EaseIn"),
        "easeout" => Ok("EaseOut"),
        _ => Err("interpolation must be Linear, Bezier, Auto Bezier, Continuous Bezier, Hold, Ease In, or Ease Out".into()),
    }
}

pub(crate) fn resolve_interpolation(
    sequence: &Value,
    clip_id: u64,
    effect_id: &str,
    param_id: &str,
    clip_time_ticks: i64,
    interpolation: &str,
) -> Result<EffectParamTarget, String> {
    let desired = interpolation_name(interpolation)?;
    let mut target = resolve_for_mode(sequence, clip_id, effect_id, param_id, Value::from(0.0), true)?;
    let (media_time_ticks, param) = keyframe_context(sequence, &target, clip_time_ticks)?;
    let count = param["keyframes"].as_u64().ok_or("keyframe count unavailable")?;
    let details = param["keyframeDetails"].as_array().ok_or("keyframe details unavailable")?;
    if count == 0 || count > 64 || details.len() != count as usize {
        return Err("existing keyframes are missing or exceed the bounded edit limit".into());
    }
    let mut previous = None;
    let mut found = None;
    for point in details {
        let time = point["time"].as_i64().ok_or("keyframe time unavailable")?;
        if previous.is_some_and(|prior| time <= prior) {
            return Err("existing keyframes are not strictly ordered".into());
        }
        if time == media_time_ticks {
            let value = point["value"].as_str().ok_or("keyframe value unavailable")?;
            value
                .strip_prefix("Float(")
                .and_then(|s| s.strip_suffix(')'))
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|n| n.is_finite())
                .ok_or("existing keyframe is not a finite numeric slider value")?;
            let before = point["interpolation"].as_str().ok_or("keyframe interpolation unavailable")?;
            if before == desired {
                return Err("keyframe already has that interpolation".into());
            }
            found = Some((value.to_string(), before.to_string()));
        }
        previous = Some(time);
    }
    let (before_value, before_interpolation) = found.ok_or("no keyframe exists at that clip time")?;
    target.clip_time_ticks = Some(clip_time_ticks);
    target.media_time_ticks = Some(media_time_ticks);
    target.before_keyframe_value = Some(before_value);
    target.before_interpolation = Some(before_interpolation);
    target.after_interpolation = Some(desired.to_string());
    target.after_value = Value::Null;
    Ok(target)
}

pub(crate) fn resolve_move_keyframe(
    sequence: &Value,
    clip_id: u64,
    effect_id: &str,
    param_id: &str,
    clip_time_ticks: i64,
    to_clip_time_ticks: i64,
) -> Result<EffectParamTarget, String> {
    if clip_time_ticks == to_clip_time_ticks {
        return Err("keyframe is already at the destination time".into());
    }
    let mut target = resolve_for_mode(sequence, clip_id, effect_id, param_id, Value::from(0.0), true)?;
    let (media_time_ticks, param) = keyframe_context(sequence, &target, clip_time_ticks)?;
    let (to_media_time_ticks, _) = keyframe_context(sequence, &target, to_clip_time_ticks)?;
    let count = param["keyframes"].as_u64().ok_or("keyframe count unavailable")?;
    let details = param["keyframeDetails"].as_array().ok_or("keyframe details unavailable")?;
    if count == 0 || count > 64 || details.len() != count as usize {
        return Err("existing keyframes are missing or exceed the bounded edit limit".into());
    }
    let mut previous = None;
    let mut found = None;
    for point in details {
        let time = point["time"].as_i64().ok_or("keyframe time unavailable")?;
        if previous.is_some_and(|prior| time <= prior) {
            return Err("existing keyframes are not strictly ordered".into());
        }
        if time == to_media_time_ticks {
            return Err("a keyframe already exists at the destination time".into());
        }
        if time == media_time_ticks {
            let value = point["value"].as_str().ok_or("keyframe value unavailable")?;
            value
                .strip_prefix("Float(")
                .and_then(|s| s.strip_suffix(')'))
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|n| n.is_finite())
                .ok_or("existing keyframe is not a finite numeric slider value")?;
            let interpolation = point["interpolation"].as_str().ok_or("keyframe interpolation unavailable")?;
            found = Some((value.to_string(), interpolation.to_string()));
        }
        previous = Some(time);
    }
    let (before_value, before_interpolation) = found.ok_or("no keyframe exists at the source time")?;
    target.clip_time_ticks = Some(clip_time_ticks);
    target.media_time_ticks = Some(media_time_ticks);
    target.to_clip_time_ticks = Some(to_clip_time_ticks);
    target.to_media_time_ticks = Some(to_media_time_ticks);
    target.before_keyframe_value = Some(before_value);
    target.before_interpolation = Some(before_interpolation);
    target.after_value = Value::Null;
    Ok(target)
}

fn keyframe_context<'a>(sequence: &'a Value, target: &EffectParamTarget, clip_time_ticks: i64) -> Result<(i64, &'a Value), String> {
    let track = sequence["video"]
        .as_array()
        .and_then(|tracks| tracks.iter().find(|track| track["id"].as_u64() == Some(target.track_id)))
        .ok_or("video track unavailable")?;
    let clip =
        track["items"].as_array().and_then(|clips| clips.iter().find(|clip| clip["clip"].as_u64() == Some(target.clip_id))).ok_or("video clip unavailable")?;
    let duration = clip["duration"].as_i64().ok_or("clip duration unavailable")?;
    if clip_time_ticks < 0
        || clip_time_ticks >= duration
        || clip["speed"].as_f64() != Some(1.0)
        || clip["reverse"].as_bool() != Some(false)
        || !clip["frameHold"].is_null()
    {
        return Err("keyframe requires a forward, normal-speed clip and a time inside it".into());
    }
    let media_time_ticks =
        clip["sourceIn"].as_i64().ok_or("clip source time unavailable")?.checked_add(clip_time_ticks).ok_or("keyframe media time overflows")?;
    let param = &clip["effects"][target.effect_index]["params"][target.param_id.as_str()];
    if param.is_null() {
        return Err("effect parameter is missing".into());
    }
    Ok((media_time_ticks, param))
}

pub(crate) fn parameter(catalog: &[Value], target: &EffectParamTarget) -> Result<(Value, Value), String> {
    let effect = catalog
        .iter()
        .find(|effect| effect["id"].as_str() == Some(target.effect_id.as_str()))
        .ok_or_else(|| format!("effect {:?} is not a standard video effect", target.effect_id))?;
    let param = effect["paramInfo"]
        .as_array()
        .and_then(|params| params.iter().find(|param| param["id"].as_str() == Some(target.param_id.as_str())))
        .ok_or_else(|| format!("unknown parameter {:?} on effect {:?}", target.param_id, target.effect_id))?;
    if target.remove_keyframe == Some(true) || target.after_interpolation.is_some() || target.to_clip_time_ticks.is_some() {
        if param["type"].as_str() != Some("float") || param["animatable"].as_bool() != Some(true) {
            return Err("keyframe action requires an animatable numeric slider".into());
        }
        return Ok((param.clone(), Value::Null));
    }
    let resolved = match param["type"].as_str() {
        Some("float") => {
            let requested = target.after_value.as_f64().ok_or("numeric slider requires a number")?;
            let min = param["min"].as_f64().ok_or("parameter minimum unavailable")?;
            let max = param["max"].as_f64().ok_or("parameter maximum unavailable")?;
            if !requested.is_finite() || requested < min || requested > max {
                return Err(format!("parameter value must be between {min} and {max}"));
            }
            target.after_value.clone()
        }
        Some("bool") => {
            target.after_value.as_bool().ok_or("switch parameter requires true or false")?;
            target.after_value.clone()
        }
        Some("choice") => {
            let requested = target.after_value.as_str().ok_or("menu choice requires an option label")?;
            let options = param["options"].as_array().ok_or("menu options unavailable")?;
            if options.len() > 128 {
                return Err("menu has too many options for a bounded controller edit".into());
            }
            let mut matches = options.iter().enumerate().filter(|(_, option)| option.as_str().is_some_and(|name| name.eq_ignore_ascii_case(requested)));
            let (index, _) = matches.next().ok_or_else(|| format!("unknown menu option {requested:?}"))?;
            if matches.next().is_some() {
                return Err("menu option is ambiguous".into());
            }
            let current = target
                .before_value
                .strip_prefix("Choice(")
                .and_then(|s| s.strip_suffix(')'))
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or("current menu choice unavailable")?;
            if current == index {
                return Err("parameter already has that value".into());
            }
            Value::from(index as u64)
        }
        _ => return Err("only numeric sliders, switches, and menu choices are supported".into()),
    };
    Ok((param.clone(), resolved))
}
