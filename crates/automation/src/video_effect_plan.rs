//! Resolve a bounded video effect edit against an inspected sequence and catalog.

use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct VideoEffectTarget {
    pub clip_id: u64,
    pub item_id: u64,
    pub clip_name: String,
    pub track_id: u64,
    pub track_name: String,
    pub effect_name: String,
    pub existing_effects: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RemoveEffectTarget {
    pub clip_id: u64,
    pub item_id: u64,
    pub clip_name: String,
    pub track_id: u64,
    pub track_name: String,
    pub effect_index: usize,
    pub effect_id: String,
    pub removed_effect: Value,
}

pub(crate) fn resolve_removal(sequence: &Value, clip_id: u64, effect_id: &str) -> Result<RemoveEffectTarget, String> {
    if effect_id.is_empty() || effect_id.len() > 128 || !effect_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err("use the exact effect ID from the catalog".into());
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
        if effect["enabled"].as_bool().is_none() || effect["masks"].as_u64().is_none() || effect["params"].as_object().is_none() {
            return Err("effect details unavailable".into());
        }
        return Ok(RemoveEffectTarget {
            clip_id,
            item_id: clip["item"].as_u64().ok_or("clip source item unavailable")?,
            clip_name: clip["name"].as_str().ok_or("clip name unavailable")?.into(),
            track_id: track["id"].as_u64().ok_or("video track ID unavailable")?,
            track_name: track["name"].as_str().ok_or("video track name unavailable")?.into(),
            effect_index,
            effect_id: effect_id.into(),
            removed_effect: effect.clone(),
        });
    }
    Err(format!("video clip #{clip_id} was not found in the active sequence"))
}

pub(crate) fn resolve(sequence: &Value, clip_id: u64, name: &str) -> Result<VideoEffectTarget, String> {
    if name.trim().is_empty() || name.chars().count() > 256 {
        return Err("effect name must contain 1–256 characters".into());
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
        return Ok(VideoEffectTarget {
            clip_id,
            item_id: clip["item"].as_u64().ok_or("clip source item unavailable")?,
            clip_name: clip["name"].as_str().ok_or("clip name unavailable")?.into(),
            track_id: track["id"].as_u64().ok_or("video track ID unavailable")?,
            track_name: track["name"].as_str().ok_or("video track name unavailable")?.into(),
            effect_name: name.into(),
            existing_effects,
        });
    }
    Err(format!("video clip #{clip_id} was not found in the active sequence"))
}

pub(crate) fn catalog(catalog: &Value) -> Result<Vec<Value>, String> {
    let entries = catalog.as_array().ok_or("video effect catalog unavailable")?;
    if entries.len() > 512 {
        return Err("video effect catalog too large".into());
    }
    Ok(entries.iter().filter(|effect| effect["controllerVideoEffect"] == true).cloned().collect())
}

pub(crate) fn find(catalog: &[Value], name: &str) -> Result<Value, String> {
    let mut matches = catalog.iter().filter(|effect| {
        effect["id"].as_str().is_some_and(|id| id.eq_ignore_ascii_case(name)) || effect["name"].as_str().is_some_and(|label| label.eq_ignore_ascii_case(name))
    });
    let effect = matches.next().ok_or_else(|| format!("unknown standard video effect {name:?}"))?.clone();
    if matches.next().is_some() {
        return Err(format!("effect name {name:?} is ambiguous; use the effect ID"));
    }
    Ok(effect)
}
