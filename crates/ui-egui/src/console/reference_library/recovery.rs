//! Per-book recovery records, independent of the search database. Private prose stays local.
use super::model::{Document, Index};
use std::path::{Path, PathBuf};
const MAX_RECORD: usize = 64 * 1024 * 1024;
fn directory(root: &Path) -> Result<PathBuf, String> {
    let path = root.join("BookRecovery");
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.to_string()),
    }
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Book recovery must use a regular private directory".into());
    }
    Ok(path)
}
fn validate(document: &Document) -> Result<(), String> {
    let index = Index { schema: 1, generation: 0, next_id: document.id, documents: vec![document.clone()], terms: Default::default() };
    index.validate()
}
pub(super) fn read(root: &Path, id: u64) -> Result<Document, String> {
    if id == 0 {
        return Err("Choose a positive recovery identity".into());
    }
    let bytes = super::store::read(&directory(root)?.join(format!("book-{id}.json")), MAX_RECORD)?
        .ok_or("Book recovery record is unavailable; original files are preserved")?;
    let document: Document = serde_json::from_slice(&bytes).map_err(|error| format!("Cannot decode book recovery record: {error}"))?;
    if document.id != id {
        return Err("Recovery record has a different book identity".into());
    }
    validate(&document)?;
    Ok(document)
}
pub(super) fn save(root: &Path, document: &Document) -> Result<(), String> {
    let mut full = document.clone();
    if full.passages.is_empty() && full.passage_count.is_some_and(|count| count > 0) {
        full.passages = read(root, full.id)?.passages;
    }
    validate(&full)?;
    let bytes = serde_json::to_vec(&full).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_RECORD {
        return Err("Book recovery record exceeds 64 MiB".into());
    }
    filmcraft_format::atomic::atomic_write(&directory(root)?.join(format!("book-{}.json", full.id)), &bytes)
        .map_err(|error| format!("Cannot retain book recovery record: {error}"))
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Counters {
    generation: u64,
    next_id: u64,
}
pub(super) fn counters(root: &Path, generation: u64, next_id: u64) -> Result<(), String> {
    let path = directory(root)?.join("counters.json");
    let previous: Counters =
        super::store::read(&path, 1024)?.map(|bytes| serde_json::from_slice(&bytes)).transpose().map_err(|error| error.to_string())?.unwrap_or_default();
    let next = Counters { generation: generation.max(previous.generation), next_id: next_id.max(previous.next_id) };
    filmcraft_format::atomic::atomic_write(&path, &serde_json::to_vec(&next).map_err(|error| error.to_string())?).map_err(|error| error.to_string())
}
pub(super) fn metadata(root: &Path, removed: &std::collections::BTreeSet<u64>) -> Result<Index, String> {
    let path = directory(root)?;
    let bytes = super::store::read(&path.join("counters.json"), 1024)?.ok_or("No recovery counter is available; legacy files and originals are preserved")?;
    let counters: Counters = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let mut index = Index {
        schema: 1,
        generation: counters.generation.checked_add(1).ok_or("Recovery generation exhausted")?,
        next_id: counters.next_id,
        documents: Vec::new(),
        terms: Default::default(),
    };
    let mut ids = std::collections::BTreeSet::new();
    for (n, entry) in std::fs::read_dir(&path).map_err(|error| error.to_string())?.enumerate() {
        if n >= 2048 {
            return Err("Book recovery directory exceeds its entry budget".into());
        }
        let name = entry.map_err(|error| error.to_string())?.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_prefix("book-"))
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(|name| name.parse::<u64>().ok())
            .filter(|id| *id > 0)
        else {
            continue;
        };
        index.next_id = index.next_id.max(id);
        if removed.contains(&id) {
            continue;
        }
        if !ids.insert(id) {
            return Err("Duplicate recovery book identity".into());
        }
        if ids.len() > 512 {
            return Err("Recovery exceeds 512 books".into());
        }
        let mut document = read(root, id)?;
        let extension = Path::new(&document.source_name).extension().and_then(|part| part.to_str()).unwrap_or("").to_ascii_lowercase();
        if !["pdf", "epub", "txt", "md"].contains(&extension.as_str()) {
            return Err("Recovery original has an unsupported type".into());
        }
        let original = std::fs::symlink_metadata(root.join(format!("document-{id}.{extension}")))
            .map_err(|_| format!("Original for book {id} is missing; restore it before recovery"))?;
        if !original.is_file() || original.file_type().is_symlink() {
            return Err("Recovery original must be a regular file".into());
        }
        document.passage_count = Some(u32::try_from(document.passages.len()).map_err(|_| "Invalid recovery passage count")?);
        document.passages.clear();
        index.documents.push(document);
    }
    if let Some(last) = removed.last() {
        index.next_id = index.next_id.max(*last);
    }
    index.validate()?;
    Ok(index)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_updates_preserve_recoverable_passages_and_corruption_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let mut document = Document {
            id: 1,
            title: "Continuity".into(),
            source_name: "book.txt".into(),
            source_sha256: String::new(),
            imported_at: "test".into(),
            import_warning: String::new(),
            pages: 1,
            chapters: Vec::new(),
            enabled: true,
            passages: super::super::model::passages(1, "Synchronize the band performance without cutting the song."),
            passage_count: Some(1),
        };
        save(dir.path(), &document).unwrap();
        document.passages.clear();
        document.enabled = false;
        save(dir.path(), &document).unwrap();
        let restored = read(dir.path(), 1).unwrap();
        assert!(!restored.enabled);
        assert!(restored.passages[0].text.contains("Synchronize"));
        std::fs::write(directory(dir.path()).unwrap().join("book-1.json"), b"corrupt").unwrap();
        assert!(read(dir.path(), 1).is_err());
        assert!(read(dir.path(), 0).is_err());
    }
}
