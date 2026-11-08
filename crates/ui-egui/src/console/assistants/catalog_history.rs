//! Bounded restorable catalog revisions. Only the latest twenty archived catalogs are retained.
use super::{atomic, disk, library::Library};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
const KEEP: usize = 20;
const CATALOG_BYTES: usize = 4 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Revision {
    pub id: u64,
    pub saved_at: String,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct History {
    next: u64,
    revisions: Vec<Revision>,
}
fn root(dir: &Path) -> Result<PathBuf, String> {
    let path = dir.join("History");
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("Cannot create assistant history: {error}")),
    }
    let meta = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("Assistant history must use its own regular directory".into());
    }
    Ok(path)
}
fn read(root: &Path) -> Result<History, String> {
    let bytes = disk::read(&root.join("revisions.json"), 16 * 1024)?;
    let history: History =
        bytes.map(|bytes| serde_json::from_slice(&bytes)).transpose().map_err(|error| format!("Cannot read assistant history: {error}"))?.unwrap_or_default();
    if history.revisions.len() > KEEP {
        return Err("Assistant history exceeds its revision limit".into());
    }
    let mut previous = 0;
    for revision in &history.revisions {
        if revision.id <= previous || revision.id > history.next {
            return Err("Assistant revision identities are invalid".into());
        }
        chrono::DateTime::parse_from_rfc3339(&revision.saved_at).map_err(|_| "Assistant revision timestamp is invalid")?;
        previous = revision.id;
    }
    Ok(history)
}
fn path(root: &Path, id: u64) -> PathBuf {
    root.join(format!("revision-{id}.json"))
}
fn prune(root: &Path, history: &History) -> Result<(), String> {
    let entries = std::fs::read_dir(root).map_err(|error| error.to_string())?;
    for (index, entry) in entries.enumerate() {
        if index >= 64 {
            return Err("Assistant history contains too many files; inspect storage before saving".into());
        }
        let entry = entry.map_err(|error| error.to_string())?;
        let filename = entry.file_name();
        let Some(id) = filename
            .to_str()
            .and_then(|name| name.strip_prefix("revision-"))
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        if id <= history.next && !history.revisions.iter().any(|revision| revision.id == id) {
            if !entry.file_type().map_err(|error| error.to_string())?.is_file() {
                return Err("Assistant history contains an unexpected non-file entry".into());
            }
            std::fs::remove_file(entry.path()).map_err(|error| format!("Cannot retire old assistant revision: {error}"))?;
        }
    }
    Ok(())
}
/// Caller holds the catalog lock. Failure leaves the current catalog unchanged.
pub(super) fn retain(dir: &Path, current: &Library) -> Result<(), String> {
    let root = root(dir)?;
    let mut history = read(&root)?;
    prune(&root, &history)?;
    let bytes = current.encode()?;
    if let Some(last) = history.revisions.last()
        && disk::read(&path(&root, last.id), CATALOG_BYTES)?.as_deref() == Some(bytes.as_slice())
    {
        return Ok(());
    }
    let id = history.next.checked_add(1).ok_or("Assistant history identity limit reached")?;
    atomic::atomic_write(&path(&root, id), &bytes).map_err(|error| format!("Cannot retain assistant revision: {error}"))?;
    history.next = id;
    history.revisions.push(Revision { id, saved_at: chrono::Utc::now().to_rfc3339() });
    if history.revisions.len() > KEEP {
        history.revisions.remove(0);
    }
    atomic::atomic_write(&root.join("revisions.json"), &serde_json::to_vec(&history).map_err(|error| error.to_string())?)
        .map_err(|error| format!("Cannot publish assistant revision list: {error}"))?;
    // A failed cleanup prevents this catalog update, keeping the last valid current file.
    prune(&root, &history)
}
pub(super) fn list(dir: &Path) -> Result<Vec<Revision>, String> {
    Ok(read(&root(dir)?)?.revisions)
}
pub(super) fn load(dir: &Path, id: u64) -> Result<Library, String> {
    let root = root(dir)?;
    let history = read(&root)?;
    if !history.revisions.iter().any(|revision| revision.id == id) {
        return Err("Choose a retained assistant revision".into());
    }
    let bytes = disk::read(&path(&root, id), CATALOG_BYTES)?.ok_or("Assistant revision is missing")?;
    Library::decode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_revisions_restore_exact_teaching_and_reject_unknown_ids() {
        let dir = tempfile::tempdir().unwrap();
        let mut library = Library::default();
        for n in 0..24 {
            let instructions = super::super::Instructions { role: format!("Revision {n}"), ..Default::default() };
            library = Library::default().with_saved("Band editor", instructions, false).unwrap();
            retain(dir.path(), &library).unwrap();
        }
        let revisions = list(dir.path()).unwrap();
        assert_eq!(revisions.len(), KEEP);
        assert_eq!(load(dir.path(), revisions[0].id).unwrap().instructions("Band editor").unwrap().role, "Revision 4");
        assert_eq!(load(dir.path(), revisions[KEEP - 1].id).unwrap(), library);
        assert!(load(dir.path(), 0).is_err());
        assert!(load(dir.path(), 1).is_err());
        assert_eq!(std::fs::read_dir(root(dir.path()).unwrap()).unwrap().count(), KEEP + 1);
    }
}
