//! Small, revision-checked agent reads. Never build a complete timeline to return one page.
use crate::{EngineError, Result, Session};
use filmcraft_project::ItemId;
use serde::Serialize;
use serde_json::{Value, json};
use std::io::Write;

pub const MAX_PAGE_BYTES: usize = 24 * 1024;
const MAX_ROWS: usize = 64;

/// Stop serialization at the budget without allocating another copy of a large result.
pub fn json_fits(value: &impl Serialize, budget: usize) -> bool {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.checked_sub(bytes.len()).ok_or_else(|| std::io::Error::other("JSON budget exceeded"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter(budget), value).is_ok()
}
fn error(message: &str) -> EngineError {
    EngineError::Other(message.into())
}
fn text(value: &str) -> &str {
    let mut end = value.len().min(512);
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value.get(..end).unwrap_or("")
}
fn number(p: &Value, key: &str, default: usize, max: usize) -> Result<usize> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => {
            v.as_u64().and_then(|n| usize::try_from(n).ok()).filter(|n| *n <= max).ok_or_else(|| error(&format!("{key} must be an integer from 0 to {max}")))
        }
    }
}
fn id(p: &Value, key: &str) -> Result<Option<u64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| error(&format!("{key} must be an unsigned integer"))),
    }
}
fn validate(s: &Session, p: &Value, allowed: &[&str]) -> Result<(usize, usize)> {
    let object = p.as_object().ok_or_else(|| error("Inspection parameters must be an object"))?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(error("Unexpected inspection parameter"));
    }
    let offset = number(p, "offset", 0, usize::MAX)?;
    let limit = number(p, "limit", 32, MAX_ROWS)?;
    if limit == 0 {
        return Err(error("limit must be from 1 to 64"));
    }
    let revision = id(p, "expectedRevision")?;
    if revision.is_some_and(|r| r != s.revision) {
        return Err(error("Project changed; restart inspection at offset 0"));
    }
    if offset > 0 && revision.is_none() {
        return Err(error("Further pages require expectedRevision from the first page"));
    }
    Ok((offset, limit))
}
fn page(mut header: Value, rows: impl Iterator<Item = Value>, total: usize, offset: usize, limit: usize) -> Result<Value> {
    let mut entries = Vec::new();
    let mut bytes = 0usize;
    // Reserve space for the bounded header and JSON punctuation. Rows contain only bounded
    // scalar fields; never include complete effects, keyframe arrays or nested sequences.
    for row in rows.skip(offset).take(limit) {
        let size = serde_json::to_vec(&row).map_err(|e| error(&e.to_string()))?.len();
        if bytes.saturating_add(size) > MAX_PAGE_BYTES.saturating_sub(4096) {
            break;
        }
        bytes = bytes.saturating_add(size);
        entries.push(row);
    }
    let next = offset.saturating_add(entries.len());
    header["rows"] = json!(entries);
    header["total"] = json!(total);
    header["offset"] = json!(offset);
    header["nextOffset"] = if next < total { json!(next) } else { Value::Null };
    header["maxBytes"] = json!(MAX_PAGE_BYTES);
    if !json_fits(&header, MAX_PAGE_BYTES) {
        return Err(error("Inspection page exceeded its budget; use a smaller limit"));
    }
    Ok(header)
}
pub fn project(s: &Session, p: &Value) -> Result<Value> {
    let (offset, limit) = validate(s, p, &["offset", "limit", "expectedRevision"])?;
    let rows = s
        .project
        .items
        .values()
        .map(|item| json!({"item":item.id.0,"name":text(&item.name),"type":item.type_label(),"duration":item.duration().0,"label":item.label.name()}));
    page(
        json!({"schema":1,"revision":s.revision,"documentEpoch":s.document_epoch,"name":text(&s.project.name),"path":s.path.as_deref().map(text),"dirty":s.is_dirty(),"activeSequence":s.state.active_sequence.map(|id|id.0),"sourceItem":s.state.source_item.map(|id|id.0),"coverage":"Flat item page; names/paths limited to 512 UTF-8 bytes. No recursive bins or media payloads."}),
        rows,
        s.project.items.len(),
        offset,
        limit,
    )
}
pub fn sequence(s: &Session, p: &Value) -> Result<Value> {
    let (offset, limit) = validate(s, p, &["item", "kind", "track", "clip", "effect", "param", "offset", "limit", "expectedRevision"])?;
    let item = id(p, "item")?.map(ItemId).or(s.state.active_sequence).ok_or(EngineError::NoSequence)?;
    let q = s.project.sequence(item).ok_or(EngineError::NoSequence)?;
    let track = id(p, "track")?;
    let clip = id(p, "clip")?;
    let kind = match p.get("kind") {
        None => "clips",
        Some(v) => v.as_str().ok_or_else(|| error("kind must be a string"))?,
    };
    let tracks = || q.video_tracks.iter().chain(q.audio_tracks.iter()).filter(|t| track.is_none_or(|id| t.id.0 == id));
    if track.is_some() && tracks().next().is_none() {
        return Err(error("Track is missing from this sequence"));
    }
    let clips = || tracks().flat_map(|t| t.items.iter().filter(move |c| clip.is_none_or(|id| c.id.0 == id)).map(move |c| (t, c)));
    if clip.is_some() && clips().next().is_none() {
        return Err(error("Clip is missing from the requested track/sequence"));
    }
    let header = json!({"schema":1,"revision":s.revision,"documentEpoch":s.document_epoch,"id":item.0,"name":s.project.item(item).map(|i|text(&i.name)),"kind":kind,"duration":q.duration().0,"playhead":s.state.playheads.get(&item).map(|t|t.0),"in":q.mark_in.map(|t|t.0),"out":q.mark_out.map(|t|t.0),"trackCount":q.video_tracks.len().saturating_add(q.audio_tracks.len()),"clipCount":clips().count(),"markerCount":q.markers.len(),"selectionCount":s.state.selection.len(),"ticksPerSecond":filmcraft_time::TICKS_PER_SECOND,"coverage":"Paged compact fields; names limited to 512 UTF-8 bytes. Request tracks/markers/selection/effects/parameters/keyframes separately. No sampled media."});
    match kind {
        "clips" => page(header, clips().map(|(t,c)|json!({"clip":c.id.0,"track":t.id.0,"item":c.item.0,"name":text(&c.name),"start":c.start.0,"duration":c.duration.0,"sourceIn":c.source_in.0,"speed":c.speed,"reverse":c.reverse,"enabled":c.enabled,"gainDb":c.gain_db,"label":c.label.name(),"link":c.link,"effectCount":c.effects.len(),"selected":s.state.selection.contains(&c.id)})), clips().count(), offset, limit),
        "tracks" => page(header, tracks().map(|t|json!({"track":t.id.0,"name":text(&t.name),"kind":format!("{:?}",t.kind),"locked":t.locked,"enabled":t.enabled,"muted":t.muted,"solo":t.solo,"syncLock":t.sync_lock,"clipCount":t.items.len(),"transitionCount":t.transitions.len()})),tracks().count(),offset,limit),
        "markers" => page(header,q.markers.iter().map(|m|json!({"id":m.id.0,"start":m.start.0,"duration":m.duration.0,"name":text(&m.name),"comment":text(&m.comment),"color":m.color.name()})),q.markers.len(),offset,limit),
        "selection" => page(header,s.state.selection.iter().map(|id|json!({"clip":id.0})),s.state.selection.len(),offset,limit),
        "effects" | "parameters" | "keyframes" => {
            if clip.is_none() { return Err(error("Effect/parameter/keyframe inspection requires a clip ID")); }
            let (_,c) = clips().next().ok_or_else(||error("Clip is missing"))?;
            if kind == "effects" {
                return page(header,c.effects.iter().enumerate().map(|(index,e)|json!({"index":index,"effect":text(&e.effect),"enabled":e.enabled,"maskCount":e.masks.len(),"parameterCount":e.params.len()})),c.effects.len(),offset,limit);
            }
            let effect = id(p,"effect")?.and_then(|n|usize::try_from(n).ok()).and_then(|n|c.effects.get(n)).ok_or_else(||error("Choose an existing zero-based effect index"))?;
            if kind == "parameters" {
                return page(header,effect.params.iter().map(|(name,param)|json!({"name":text(name),"value":small_value(&param.value),"keyframeCount":param.keyframes.len()})),effect.params.len(),offset,limit);
            }
            let param = p.get("param").and_then(Value::as_str).filter(|v|v.len()<=512).and_then(|name|effect.params.get(name)).ok_or_else(||error("Choose an existing parameter name of at most 512 bytes"))?;
            page(header,param.keyframes.iter().map(|k|json!({"time":k.time.0,"value":small_value(&k.value),"interpolation":small_value(&k.interp)})),param.keyframes.len(),offset,limit)
        }
        _ => Err(error("kind must be clips, tracks, markers, selection, effects, parameters or keyframes")),
    }
}
fn small_value(value: &impl Serialize) -> Value {
    if !json_fits(value, 2048) {
        return json!({"omitted":true,"reason":"Value exceeds 2048-byte inspection budget"});
    }
    serde_json::to_value(value).unwrap_or_else(|_| json!({"omitted":true,"reason":"Value cannot be represented as JSON"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn populated() -> Session {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let id = s.state.active_sequence.unwrap();
        let project = std::sync::Arc::make_mut(&mut s.project);
        let q = project.sequence_mut(id).unwrap();
        let original = q.video_tracks[0].items[0].clone();
        q.video_tracks[0].items = (0..600)
            .map(|n| {
                let mut c = original.clone();
                c.id = filmcraft_project::ClipId(10000 + n);
                c.start = filmcraft_time::Tick(n as i64 * 1000000);
                c
            })
            .collect();
        s
    }
    #[test]
    fn large_timeline_pages_remain_small_and_cover_every_clip_once() {
        let s = populated();
        let before = s.project.clone();
        let full = crate::commands::inspect_sequence(&s, s.state.active_sequence.unwrap(), s.active_sequence().unwrap());
        assert!(serde_json::to_vec_pretty(&full).unwrap().len() > 1024 * 1024, "regression must reproduce the old transport failure");
        let mut offset = 0;
        let mut ids = std::collections::BTreeSet::new();
        loop {
            let result = sequence(&s, &json!({"offset":offset,"expectedRevision":s.revision})).unwrap();
            assert!(serde_json::to_vec(&result).unwrap().len() <= MAX_PAGE_BYTES);
            for row in result["rows"].as_array().unwrap() {
                assert!(ids.insert(row["clip"].as_u64().unwrap()));
            }
            let Some(next) = result["nextOffset"].as_u64() else {
                assert_eq!(ids.len(), result["total"].as_u64().unwrap() as usize);
                break;
            };
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(*before, *s.project);
    }
    #[test]
    fn hostile_pages_and_stale_revisions_are_rejected() {
        let mut s = populated();
        for p in [
            json!({"limit":0}),
            json!({"limit":65}),
            json!({"offset":-1}),
            json!({"offset":1}),
            json!({"track":u64::MAX}),
            json!({"clip":u64::MAX}),
            json!({"kind":[]}),
            json!({"kind":"effects"}),
            json!({"expectedRevision":s.revision+1}),
            json!({"unexpected":1}),
        ] {
            assert!(sequence(&s, &p).is_err(), "{p}");
        }
        let first = sequence(&s, &json!({})).unwrap();
        s.revision += 1;
        assert!(sequence(&s, &json!({"offset":first["nextOffset"],"expectedRevision":first["revision"]})).is_err());
        assert!(sequence(&s, &json!({"offset":u64::MAX,"expectedRevision":s.revision})).unwrap()["rows"].as_array().unwrap().is_empty());
    }
    #[test]
    fn detailed_parameter_and_keyframe_pages_are_readable_without_full_timeline() {
        let mut s = populated();
        let id = s.state.active_sequence.unwrap();
        let q = std::sync::Arc::make_mut(&mut s.project).sequence_mut(id).unwrap();
        let mut parameter = filmcraft_project::Param::new(filmcraft_project::ParamValue::Float(7.0));
        parameter.keyframes =
            (0..200).map(|n| filmcraft_project::Keyframe::new(filmcraft_time::Tick(n), filmcraft_project::ParamValue::Float(n as f64))).collect();
        q.video_tracks[0].items[0].effects[0].params.insert("inspection-test".into(), parameter);
        assert!(!sequence(&s, &json!({"kind":"tracks"})).unwrap()["rows"].as_array().unwrap().is_empty());
        assert_eq!(sequence(&s, &json!({"kind":"markers"})).unwrap()["total"], s.active_sequence().unwrap().markers.len());
        assert!(!sequence(&s, &json!({"kind":"effects","clip":10000})).unwrap()["rows"].as_array().unwrap().is_empty());
        let params = sequence(&s, &json!({"kind":"parameters","clip":10000,"effect":0})).unwrap();
        let row = params["rows"].as_array().unwrap().iter().find(|r| r["name"] == "inspection-test").unwrap();
        assert_eq!(row["value"]["Float"], 7.0);
        assert_eq!(row["keyframeCount"], 200);
        let mut offset = 0;
        let mut times = Vec::new();
        loop {
            let page =
                sequence(&s, &json!({"kind":"keyframes","clip":10000,"effect":0,"param":"inspection-test","offset":offset,"expectedRevision":s.revision}))
                    .unwrap();
            for row in page["rows"].as_array().unwrap() {
                times.push(row["time"].as_i64().unwrap());
            }
            let Some(next) = page["nextOffset"].as_u64() else { break };
            offset = next;
        }
        assert_eq!(times, (0..200).collect::<Vec<_>>());
    }
    #[test]
    fn project_page_byte_budget_shortens_rows_without_skipping_items() {
        let mut s = populated();
        let template = s.project.items.values().find(|i| i.as_media().is_some()).unwrap().clone();
        for n in 50000..50200 {
            let mut item = template.clone();
            item.id = ItemId(n);
            item.name = "\"".repeat(5000);
            std::sync::Arc::make_mut(&mut s.project).items.insert(item.id, item);
        }
        let first = project(&s, &json!({"limit":64})).unwrap();
        assert!(first["rows"].as_array().unwrap().len() < 64);
        let mut seen = std::collections::BTreeSet::new();
        let mut offset = 0;
        loop {
            let page = project(&s, &json!({"offset":offset,"limit":64,"expectedRevision":s.revision})).unwrap();
            assert!(json_fits(&page, MAX_PAGE_BYTES));
            for row in page["rows"].as_array().unwrap() {
                assert!(seen.insert(row["item"].as_u64().unwrap()));
            }
            let Some(next) = page["nextOffset"].as_u64() else { break };
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(seen.len(), s.project.items.len());
    }
    #[test]
    fn hostile_names_and_parameter_values_cannot_expand_page_allocations() {
        let mut s = populated();
        let id = s.state.active_sequence.unwrap();
        let q = std::sync::Arc::make_mut(&mut s.project).sequence_mut(id).unwrap();
        for c in &mut q.video_tracks[0].items {
            c.name = "🙂".repeat(5000);
        }
        let result = sequence(&s, &json!({"limit":64})).unwrap();
        assert!(json_fits(&result, MAX_PAGE_BYTES));
        assert!(result["nextOffset"].as_u64().unwrap() > 0);
        assert_eq!(small_value(&"x".repeat(100000))["omitted"], true);
        assert!(json_fits(&project(&s, &json!({})).unwrap(), MAX_PAGE_BYTES));
    }
}
