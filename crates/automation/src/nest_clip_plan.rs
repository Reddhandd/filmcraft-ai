//! Resolve a single unlinked video clip for nesting.

use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct NestTarget {
    pub clip_id: u64,
    pub item_id: u64,
    pub clip_name: String,
    pub track_id: u64,
    pub track_name: String,
    pub start_ticks: i64,
    pub duration_ticks: i64,
    pub nested_name: String,
}

pub(crate) fn resolve(project: &Value, sequence: &Value, clip_id: u64, name: &str) -> Result<NestTarget, String> {
    if name.trim().is_empty() || name.chars().count() > 256 {
        return Err("nested sequence name must contain 1–256 characters".into());
    }
    let mut found = None;
    let mut duplicate_name = false;
    let mut stack = vec![&project["root"]];
    let mut visited = 0usize;
    while let Some(node) = stack.pop() {
        visited += 1;
        if visited > 10_000 {
            return Err("project tree is too large to resolve safely".into());
        }
        if node["item"].as_u64().is_some() && node["name"].as_str() == Some(name) {
            duplicate_name = true;
        }
        if let Some(children) = node["children"].as_array() {
            stack.extend(children.iter());
        }
    }
    if duplicate_name {
        return Err("project already has an item with this name".into());
    }
    for track in sequence["video"].as_array().ok_or("video tracks unavailable")? {
        let items = track["items"].as_array().ok_or("video track clips unavailable")?;
        let Some(clip) = items.iter().find(|clip| clip["clip"].as_u64() == Some(clip_id)) else { continue };
        if track["locked"].as_bool() != Some(false) || track["enabled"].as_bool() != Some(true) {
            return Err("video track is locked or disabled".into());
        }
        if clip["link"].as_u64().is_some() {
            return Err("clip is linked; this nesting action requires one unlinked video clip".into());
        }
        for tr in track["transitions"].as_array().ok_or("video track transitions unavailable")? {
            if tr["from"].as_u64() == Some(clip_id) || tr["to"].as_u64() == Some(clip_id) {
                return Err("clip has an attached transition".into());
            }
        }
        let item_id = clip["item"].as_u64().ok_or("clip source item unavailable")?;
        let mut stack = vec![&project["root"]];
        while let Some(node) = stack.pop() {
            if node["item"].as_u64() == Some(item_id) {
                found = Some(node);
                break;
            }
            if let Some(children) = node["children"].as_array() {
                stack.extend(children.iter());
            }
        }
        let source = found.ok_or("clip source item was not found in the project")?;
        let source_type = source["type"].as_str().ok_or("clip source type unavailable")?;
        if source_type == "Sequence" {
            return Err("nesting an existing sequence is outside this controller action".into());
        }
        let start = clip["start"].as_i64().filter(|t| *t >= 0).ok_or("clip start unavailable")?;
        let duration = clip["duration"].as_i64().filter(|d| *d > 0).ok_or("clip duration unavailable")?;
        start.checked_add(duration).ok_or("clip end overflow")?;
        return Ok(NestTarget {
            clip_id,
            item_id,
            clip_name: clip["name"].as_str().ok_or("clip name unavailable")?.into(),
            track_id: track["id"].as_u64().ok_or("video track ID unavailable")?,
            track_name: track["name"].as_str().ok_or("video track name unavailable")?.into(),
            start_ticks: start,
            duration_ticks: duration,
            nested_name: name.into(),
        });
    }
    Err(format!("video clip #{clip_id} was not found in the active sequence"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_one_unlinked_video_and_rejects_transition() {
        let p = json!({"root":{"children":[{"item":9,"name":"Footage","type":"Movie"}]}});
        let mut q = json!({"video":[{"id":2,"name":"V2","locked":false,"enabled":true,
            "items":[{"clip":46,"item":9,"name":"Footage","start":100,"duration":200,"link":null}],
            "transitions":[]}]});
        let target = resolve(&p, &q, 46, "Nested Overlay").unwrap();
        assert_eq!((target.clip_id, target.item_id, target.start_ticks), (46, 9, 100));
        q["video"][0]["transitions"] = json!([{"from":46,"to":null}]);
        assert!(resolve(&p, &q, 46, "Nested Overlay").unwrap_err().contains("transition"));
        assert!(resolve(&p, &q, 46, "Footage").is_err());
    }
}
