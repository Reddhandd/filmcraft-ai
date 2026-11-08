//! Recoverable originals for memory compaction; bounded, project-local and never executable.
use super::{atomic, disk};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
const KEEP: usize = 10;
const MAX_TEXT: usize = 1024 * 1024;
const MAX_FILE: usize = MAX_TEXT * 2 + 16 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Target {
    ProjectNotes,
    AssistantContext,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Original {
    pub scope: String,
    pub target: Target,
    pub document_epoch: u64,
    pub version: u64,
    pub text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Entry {
    pub id: u64,
    pub target: Target,
    pub saved_at: String,
    pub original_bytes: usize,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    next: u64,
    entries: Vec<Entry>,
}
fn root(dir: &Path) -> Result<PathBuf, String> {
    let root = dir.join("MemoryHistory");
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("Cannot create memory history: {error}")),
    }
    let metadata = std::fs::symlink_metadata(&root).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Memory history must use its own regular directory".into());
    }
    Ok(root)
}
fn path(root: &Path, id: u64) -> PathBuf {
    root.join(format!("original-{id}.json"))
}
fn read_index(root: &Path) -> Result<Index, String> {
    let bytes = disk::read(&root.join("originals.json"), 16 * 1024)?;
    let index: Index =
        bytes.map(|bytes| serde_json::from_slice(&bytes)).transpose().map_err(|error| format!("Cannot read memory history: {error}"))?.unwrap_or_default();
    if index.entries.len() > KEEP {
        return Err("Memory history has too many originals".into());
    }
    let mut previous = 0;
    for entry in &index.entries {
        if entry.id <= previous || entry.id > index.next || entry.original_bytes > MAX_TEXT {
            return Err("Memory history metadata is invalid".into());
        }
        chrono::DateTime::parse_from_rfc3339(&entry.saved_at).map_err(|_| "Memory history timestamp is invalid")?;
        previous = entry.id;
    }
    Ok(index)
}
fn load_file(root: &Path, id: u64) -> Result<Original, String> {
    let bytes = disk::read(&path(root, id), MAX_FILE)?.ok_or("Retained original is missing")?;
    let original: Original = serde_json::from_slice(&bytes).map_err(|error| format!("Cannot decode retained original: {error}"))?;
    if original.text.len() > MAX_TEXT || original.scope.is_empty() || original.scope.len() > 4096 {
        return Err("Retained original exceeds memory limits".into());
    }
    Ok(original)
}
fn prune(root: &Path, index: &Index) -> Result<(), String> {
    for (n, entry) in std::fs::read_dir(root).map_err(|error| error.to_string())?.enumerate() {
        if n >= 64 {
            return Err("Memory history contains too many files".into());
        }
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let Some(id) =
            name.to_str().and_then(|name| name.strip_prefix("original-")).and_then(|name| name.strip_suffix(".json")).and_then(|name| name.parse::<u64>().ok())
        else {
            continue;
        };
        if id <= index.next && !index.entries.iter().any(|entry| entry.id == id) {
            if !entry.file_type().map_err(|error| error.to_string())?.is_file() {
                return Err("Memory history contains an unexpected non-file".into());
            }
            std::fs::remove_file(entry.path()).map_err(|error| format!("Cannot retire old memory original: {error}"))?;
        }
    }
    Ok(())
}
pub(super) fn retain(dir: &Path, original: &Original) -> Result<u64, String> {
    if original.text.len() > MAX_TEXT || original.scope.is_empty() || original.scope.len() > 4096 {
        return Err("Memory original exceeds its storage budget".into());
    }
    let root = root(dir)?;
    let _lock = disk::ScopeLock::take(&root)?;
    let mut index = read_index(&root)?;
    prune(&root, &index)?;
    if let Some(last) = index.entries.last() {
        let saved = load_file(&root, last.id)?;
        if saved.scope == original.scope && saved.target == original.target && saved.text == original.text {
            return Ok(last.id);
        }
    }
    let id = index.next.checked_add(1).ok_or("Memory history identity limit reached")?;
    let bytes = serde_json::to_vec(original).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_FILE {
        return Err("Encoded memory original exceeds its storage budget".into());
    }
    atomic::atomic_write(&path(&root, id), &bytes).map_err(|error| format!("Cannot retain original memory: {error}"))?;
    index.next = id;
    index.entries.push(Entry { id, target: original.target, saved_at: chrono::Utc::now().to_rfc3339(), original_bytes: original.text.len() });
    if index.entries.len() > KEEP {
        index.entries.remove(0);
    }
    atomic::atomic_write(&root.join("originals.json"), &serde_json::to_vec(&index).map_err(|error| error.to_string())?)
        .map_err(|error| format!("Cannot publish memory history: {error}"))?;
    prune(&root, &index)?;
    Ok(id)
}
pub(super) fn list(dir: &Path) -> Result<Vec<Entry>, String> {
    Ok(read_index(&root(dir)?)?.entries)
}
pub(super) fn load(dir: &Path, id: u64, scope: &str) -> Result<Original, String> {
    let root = root(dir)?;
    let index = read_index(&root)?;
    if !index.entries.iter().any(|entry| entry.id == id) {
        return Err("Choose a retained memory original".into());
    }
    let original = load_file(&root, id)?;
    if original.scope != scope {
        return Err("Retained memory belongs to a different project".into());
    }
    Ok(original)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn originals_are_recoverable_bounded_deduplicated_and_scope_checked() {
        let dir = tempfile::tempdir().unwrap();
        let mut original =
            Original { scope: "project-A".into(), target: Target::ProjectNotes, document_epoch: 1, version: 2, text: "Original procedure".into() };
        let first = retain(dir.path(), &original).unwrap();
        assert_eq!(retain(dir.path(), &original).unwrap(), first);
        assert!(load(dir.path(), first, "project-B").is_err());
        assert_eq!(load(dir.path(), first, "project-A").unwrap().text, original.text);
        for n in 0..12 {
            original.text = format!("Original {n}");
            retain(dir.path(), &original).unwrap();
        }
        assert_eq!(list(dir.path()).unwrap().len(), KEEP);
        assert!(load(dir.path(), first, "project-A").is_err());
        original.text = "x".repeat(MAX_TEXT + 1);
        assert!(retain(dir.path(), &original).is_err());
    }
}
