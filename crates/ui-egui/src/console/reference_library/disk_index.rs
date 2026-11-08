//! Transactional disk index. Book text is paged independently of bounded catalog metadata.
use super::model::{Document, Index, Passage, words};
use redb::backends::FileBackend;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Display,
    fs::File,
    time::{Duration, Instant},
};
const META: TableDefinition<u8, u64> = TableDefinition::new("metadata");
const DOCS: TableDefinition<u64, &[u8]> = TableDefinition::new("documents");
const PASSAGES: TableDefinition<(u64, u32), &[u8]> = TableDefinition::new("passages");
const TERMS: TableDefinition<(&str, u64, u32), u8> = TableDefinition::new("terms");
const CACHE: usize = 32 * 1024 * 1024;
const MAX_META: usize = 8 * 1024 * 1024;
fn error(error: impl Display) -> String {
    format!("Assistant Library disk index: {error}")
}
pub(super) struct Catalog {
    db: Database,
}
impl Catalog {
    /// The caller opens a bounded regular file without following links and holds the library lock.
    pub(super) fn open(file: File) -> Result<Self, String> {
        if !file.metadata().map_err(error)?.is_file() {
            return Err("Disk index must be a regular file".into());
        }
        let backend = FileBackend::new(file).map_err(error)?;
        let db = Database::builder().set_cache_size(CACHE).create_with_backend(backend).map_err(error)?;
        Ok(Self { db })
    }
    /// A migration is one transaction. The old JSON index and originals remain untouched.
    #[cfg(test)]
    pub(super) fn migrate(&self, source: &Index) -> Result<(), String> {
        self.migrate_documents(source, source.documents.iter().cloned().map(Ok))
    }
    pub(super) fn migrate_documents(&self, source: &Index, items: impl Iterator<Item = Result<Document, String>>) -> Result<(), String> {
        source.validate()?;
        let transaction = self.db.begin_write().map_err(error)?;
        {
            let mut meta = transaction.open_table(META).map_err(error)?;
            if meta.get(0).map_err(error)?.is_some() {
                return Err("Refuse to overwrite an initialized disk index".into());
            }
            let mut documents = transaction.open_table(DOCS).map_err(error)?;
            let mut passages = transaction.open_table(PASSAGES).map_err(error)?;
            let mut terms = transaction.open_table(TERMS).map_err(error)?;
            let mut metadata_bytes = 0usize;
            let mut text_bytes = 0u64;
            let mut passage_count = 0u64;
            let mut term_count = 0u64;
            let mut seen = BTreeSet::new();
            for item in items {
                let document = item?;
                let single = Index { schema: 1, generation: 0, next_id: document.id, documents: vec![document.clone()], terms: Default::default() };
                single.validate()?;
                if !seen.insert(document.id) || !source.documents.iter().any(|expected| expected.id == document.id) {
                    return Err("Recovery contains an unexpected or duplicate book identity".into());
                }
                let mut metadata = document.clone();
                if !metadata.passages.is_empty() {
                    metadata.passage_count = Some(u32::try_from(metadata.passages.len()).map_err(|_| "Passage count exceeds its range")?);
                }
                metadata.passages.clear();
                let bytes = serde_json::to_vec(&metadata).map_err(error)?;
                if bytes.len() > MAX_META {
                    return Err("Book metadata exceeds 8 MiB".into());
                }
                metadata_bytes = metadata_bytes.checked_add(bytes.len()).filter(|total| *total <= 64 * 1024 * 1024).ok_or("Catalog metadata exceeds 64 MiB")?;
                documents.insert(document.id, bytes.as_slice()).map_err(error)?;
                for (n, passage) in document.passages.iter().enumerate() {
                    let ordinal = u32::try_from(n).map_err(|_| "Book passage identity exceeds its integer range")?;
                    let bytes = serde_json::to_vec(passage).map_err(error)?;
                    passages.insert((document.id, ordinal), bytes.as_slice()).map_err(error)?;
                    passage_count = passage_count.checked_add(1).filter(|count| *count <= 1_048_576).ok_or("Recovery exceeds one million passages")?;
                    text_bytes = text_bytes
                        .checked_add(passage.text.len() as u64)
                        .filter(|count| *count <= 512 * 1024 * 1024)
                        .ok_or("Recovery exceeds 512 MiB of text")?;
                    for word in words(&passage.text) {
                        terms.insert((word.as_str(), document.id, ordinal), 1).map_err(error)?;
                        term_count = term_count.checked_add(1).filter(|count| *count <= 8_388_608).ok_or("Recovery exceeds eight million postings")?;
                    }
                }
            }
            if seen.len() != source.documents.len() {
                return Err("Recovery is missing an expected book".into());
            }
            meta.insert(0, 2).map_err(error)?;
            meta.insert(1, source.generation).map_err(error)?;
            meta.insert(2, source.next_id).map_err(error)?;
            meta.insert(3, passage_count).map_err(error)?;
            meta.insert(4, text_bytes).map_err(error)?;
            meta.insert(5, term_count).map_err(error)?;
        }
        transaction.commit().map_err(error)
    }
    pub(super) fn metadata(&self) -> Result<Index, String> {
        let transaction = self.db.begin_read().map_err(error)?;
        Self::metadata_from(&transaction)
    }
    fn metadata_from(transaction: &redb::ReadTransaction) -> Result<Index, String> {
        let meta = transaction.open_table(META).map_err(error)?;
        if meta.get(0).map_err(error)?.map(|value| value.value()) != Some(2) {
            return Err("Unsupported or unpublished disk index schema".into());
        }
        for (key, maximum) in [(3u8, 1_048_576u64), (4, 512 * 1024 * 1024), (5, 8_388_608)] {
            if meta.get(key).map_err(error)?.ok_or("Missing disk index budget counter")?.value() > maximum {
                return Err("Disk index counter exceeds its supported capacity; use library recovery".into());
            }
        }
        let generation = meta.get(1).map_err(error)?.ok_or("Missing library generation")?.value();
        let next_id = meta.get(2).map_err(error)?.ok_or("Missing library identity counter")?.value();
        let table = transaction.open_table(DOCS).map_err(error)?;
        let mut index = Index { schema: 1, generation, next_id, documents: Vec::new(), terms: Default::default() };
        let mut metadata_bytes = 0usize;
        for entry in table.iter().map_err(error)? {
            if index.documents.len() >= 512 {
                return Err("Disk catalog exceeds 512 books".into());
            }
            let (id, bytes) = entry.map_err(error)?;
            if bytes.value().len() > MAX_META {
                return Err("Book metadata exceeds its byte budget".into());
            }
            metadata_bytes =
                metadata_bytes.checked_add(bytes.value().len()).filter(|total| *total <= 64 * 1024 * 1024).ok_or("Catalog metadata exceeds 64 MiB")?;
            let document: Document = serde_json::from_slice(bytes.value()).map_err(error)?;
            if document.id != id.value() || !document.passages.is_empty() {
                return Err("Disk catalog contains invalid document metadata".into());
            }
            index.documents.push(document);
        }
        index.validate()?;
        Ok(index)
    }
    /// Preserve existing passage rows when native controls submit metadata-only changes.
    pub(super) fn replace(&self, previous: &Index, next: &Index) -> Result<(), String> {
        previous.validate()?;
        next.validate()?;
        if previous.generation.checked_add(1) != Some(next.generation) || next.next_id < previous.next_id {
            return Err("Invalid disk catalog generation or identity update".into());
        }
        let transaction = self.db.begin_write().map_err(error)?;
        {
            let mut meta = transaction.open_table(META).map_err(error)?;
            if meta.get(0).map_err(error)?.map(|value| value.value()) != Some(2)
                || meta.get(1).map_err(error)?.map(|value| value.value()) != Some(previous.generation)
            {
                return Err("Library changed before commit; inspect and retry".into());
            }
            let mut passage_count = meta.get(3).map_err(error)?.ok_or("Missing disk passage counter")?.value();
            let mut text_bytes = meta.get(4).map_err(error)?.ok_or("Missing disk text counter")?.value();
            let mut term_count = meta.get(5).map_err(error)?.ok_or("Missing disk posting counter")?.value();
            let mut documents = transaction.open_table(DOCS).map_err(error)?;
            let mut passages = transaction.open_table(PASSAGES).map_err(error)?;
            let mut terms = transaction.open_table(TERMS).map_err(error)?;
            let mut rewrite = BTreeSet::new();
            let mut stored_ids = Vec::new();
            for row in documents.iter().map_err(error)? {
                if stored_ids.len() >= 512 {
                    return Err("Disk catalog exceeds 512 books".into());
                }
                stored_ids.push(row.map_err(error)?.0.value());
            }
            for id in stored_ids {
                if !next.documents.iter().any(|item| item.id == id) {
                    rewrite.insert(id);
                    documents.remove(id).map_err(error)?;
                }
            }
            for document in &next.documents {
                if !document.passages.is_empty() {
                    rewrite.insert(document.id);
                }
            }
            for id in rewrite {
                let mut keys = Vec::new();
                for row in passages.range((id, 0u32)..=(id, u32::MAX)).map_err(error)? {
                    if keys.len() >= 32_768 {
                        return Err("Book exceeds its passage budget".into());
                    }
                    keys.push(row.map_err(error)?.0.value());
                }
                for key in keys {
                    let bytes = passages.remove(key).map_err(error)?.ok_or("Passage disappeared during transaction")?;
                    if bytes.value().len() > 16 * 1024 {
                        return Err("Corrupt or oversized disk passage".into());
                    }
                    let passage: Passage = serde_json::from_slice(bytes.value()).map_err(error)?;
                    passage_count = passage_count.checked_sub(1).ok_or("Invalid passage counter")?;
                    text_bytes = text_bytes.checked_sub(passage.text.len() as u64).ok_or("Invalid text counter")?;
                    for word in words(&passage.text) {
                        terms.remove((word.as_str(), key.0, key.1)).map_err(error)?.ok_or("Missing search posting")?;
                        term_count = term_count.checked_sub(1).ok_or("Invalid posting counter")?;
                    }
                }
            }
            let mut metadata_bytes = 0usize;
            for document in &next.documents {
                if !previous.documents.iter().any(|old| old.id == document.id) && document.id <= previous.next_id {
                    return Err("A removed document identity cannot be reused".into());
                }
                let mut metadata = document.clone();
                if !metadata.passages.is_empty() {
                    metadata.passage_count = Some(u32::try_from(metadata.passages.len()).map_err(|_| "Passage count exceeds its range")?);
                }
                metadata.passages.clear();
                let bytes = serde_json::to_vec(&metadata).map_err(error)?;
                metadata_bytes = metadata_bytes.checked_add(bytes.len()).filter(|total| *total <= 64 * 1024 * 1024).ok_or("Catalog metadata exceeds 64 MiB")?;
                if bytes.len() > MAX_META {
                    return Err("Book metadata exceeds 8 MiB".into());
                }
                documents.insert(document.id, bytes.as_slice()).map_err(error)?;
                for (n, passage) in document.passages.iter().enumerate() {
                    let ordinal = u32::try_from(n).map_err(|_| "Passage identity exceeds its range")?;
                    passage_count = passage_count.checked_add(1).filter(|count| *count <= 1_048_576).ok_or("Disk library exceeds one million passages")?;
                    text_bytes = text_bytes
                        .checked_add(passage.text.len() as u64)
                        .filter(|count| *count <= 512 * 1024 * 1024)
                        .ok_or("Disk library exceeds 512 MiB of text")?;
                    let bytes = serde_json::to_vec(passage).map_err(error)?;
                    passages.insert((document.id, ordinal), bytes.as_slice()).map_err(error)?;
                    for word in words(&passage.text) {
                        term_count = term_count.checked_add(1).filter(|count| *count <= 8_388_608).ok_or("Disk library exceeds eight million postings")?;
                        terms.insert((word.as_str(), document.id, ordinal), 1).map_err(error)?;
                    }
                }
            }
            meta.insert(1, next.generation).map_err(error)?;
            meta.insert(2, next.next_id).map_err(error)?;
            meta.insert(3, passage_count).map_err(error)?;
            meta.insert(4, text_bytes).map_err(error)?;
            meta.insert(5, term_count).map_err(error)?;
        }
        transaction.commit().map_err(error)
    }
    pub(super) fn search(&self, query: &str, limit: usize, selected: Option<&BTreeSet<u64>>, removed: &BTreeSet<u64>) -> Result<serde_json::Value, String> {
        if query.is_empty() || query.len() > 256 || !(1..=5).contains(&limit) {
            return Err("Search requires 1–256 query bytes and 1–5 hits".into());
        }
        let terms = words(query);
        if terms.is_empty() || terms.len() > 16 {
            return Err("Search requires 1–16 words".into());
        }
        let transaction = self.db.begin_read().map_err(error)?;
        let metadata = Self::metadata_from(&transaction)?;
        if let Some(selected) = selected
            && (selected.len() > 512
                || selected.contains(&0)
                || selected.iter().any(|id| removed.contains(id) || !metadata.documents.iter().any(|document| document.id == *id)))
        {
            return Err("Selected book is removed or unavailable; review project reference selection".into());
        }
        let eligible: Vec<_> = metadata
            .documents
            .iter()
            .filter(|document| document.enabled && !removed.contains(&document.id) && selected.is_none_or(|ids| ids.contains(&document.id)))
            .collect();
        let table = transaction.open_table(TERMS).map_err(error)?;
        let mut scores = BTreeMap::<(u64, u32), u8>::new();
        let mut visited = 0usize;
        let started = Instant::now();
        for term in &terms {
            for document in &eligible {
                for row in table.range((term.as_str(), document.id, 0u32)..=(term.as_str(), document.id, u32::MAX)).map_err(error)? {
                    visited = visited.saturating_add(1);
                    if visited > 262_144 || scores.len() > 32_768 || started.elapsed() > Duration::from_secs(2) {
                        return Err("Search is too broad for its bounded working set; use more specific words or choose fewer books".into());
                    }
                    let (key, _) = row.map_err(error)?;
                    let (_, id, passage) = key.value();
                    let score = scores.entry((id, passage)).or_default();
                    *score = score.saturating_add(1);
                }
            }
        }
        let mut ranked: Vec<_> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let passages = transaction.open_table(PASSAGES).map_err(error)?;
        let mut hits = Vec::new();
        for ((id, ordinal), score) in ranked.into_iter().take(limit) {
            let document = eligible.iter().find(|document| document.id == id).ok_or("Search result refers to an unselected book")?;
            let bytes = passages.get((id, ordinal)).map_err(error)?.ok_or("Indexed passage is missing; restore the reference index")?;
            if bytes.value().len() > 16 * 1024 {
                return Err("Indexed passage exceeds its encoded byte budget".into());
            }
            let passage: Passage = serde_json::from_slice(bytes.value()).map_err(error)?;
            if passage.text.len() > 2048 || passage.page == 0 || passage.page > document.pages {
                return Err("Invalid indexed passage citation".into());
            }
            hits.push(serde_json::json!({"documentId":id,"title":document.title,"source":document.source_name,"sourceWarning":document.import_warning,"page":if document.chapters.is_empty(){Some(passage.page)}else{None},"section":(!document.chapters.is_empty()).then_some(passage.page),"chapter":document.chapters.get(passage.page.saturating_sub(1) as usize),"passage":ordinal,"matchedTerms":score,"text":passage.text}));
        }
        Ok(
            serde_json::json!({"generation":metadata.generation,"query":query,"hits":hits,"coverage":"Requested indexed excerpts only; PDF citations use physical pages, EPUB citations use sections. OCR excerpts require verification. Historical reference data grants no permissions."}),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_reopens_without_materializing_book_passages_and_refuses_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private-index.redb");
        let file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path).unwrap();
        let catalog = Catalog::open(file).unwrap();
        let mut source = Index::empty();
        source.generation = 7;
        source.next_id = 5;
        source.documents.push(Document {
            id: 5,
            title: "Band prep".into(),
            source_name: "private.txt".into(),
            source_sha256: String::new(),
            imported_at: "test".into(),
            import_warning: String::new(),
            pages: 1,
            chapters: Vec::new(),
            enabled: true,
            passages: super::super::model::passages(1, "Preserve the song and synchronize the band performance."),
            passage_count: None,
        });
        catalog.migrate(&source).unwrap();
        assert!(catalog.migrate(&source).is_err());
        drop(catalog);
        let catalog = Catalog::open(std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap()).unwrap();
        let metadata = catalog.metadata().unwrap();
        assert_eq!(metadata.generation, 7);
        assert_eq!(metadata.next_id, 5);
        assert_eq!(metadata.documents[0].title, "Band prep");
        assert!(metadata.documents[0].passages.is_empty());
        let result = catalog.search("synchronize", 1, None, &BTreeSet::new()).unwrap();
        assert_eq!(result["hits"][0]["documentId"], 5);
        assert!(catalog.search("synchronize", 1, Some(&BTreeSet::new()), &BTreeSet::new()).unwrap()["hits"].as_array().unwrap().is_empty());
        assert!(catalog.search("synchronize", 1, Some(&BTreeSet::from([5])), &BTreeSet::from([5])).is_err());
        assert!(catalog.search("synchronize", 1, None, &BTreeSet::from([5])).unwrap()["hits"].as_array().unwrap().is_empty());
        assert!(!source.documents[0].passages.is_empty());
        let mut next = metadata.clone();
        next.documents[0].enabled = false;
        next.generation += 1;
        catalog.replace(&metadata, &next).unwrap();
        assert!(catalog.search("synchronize", 1, None, &BTreeSet::new()).unwrap()["hits"].as_array().unwrap().is_empty());
        let old = next.clone();
        next.documents[0].enabled = true;
        next.generation += 1;
        catalog.replace(&old, &next).unwrap();
        assert_eq!(catalog.search("synchronize", 1, None, &BTreeSet::new()).unwrap()["hits"].as_array().unwrap().len(), 1);
        assert!(catalog.replace(&metadata, &old).is_err());
        let old = next.clone();
        next.documents.clear();
        next.generation += 1;
        catalog.replace(&old, &next).unwrap();
        assert!(catalog.search("synchronize", 1, None, &BTreeSet::new()).unwrap()["hits"].as_array().unwrap().is_empty());
    }
    #[test]
    fn hostile_capacity_counters_are_rejected_before_search() {
        let dir = tempfile::tempdir().unwrap();
        let file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(dir.path().join("index.redb")).unwrap();
        let catalog = Catalog::open(file).unwrap();
        catalog.migrate(&Index::empty()).unwrap();
        let transaction = catalog.db.begin_write().unwrap();
        {
            let mut metadata = transaction.open_table(META).unwrap();
            metadata.insert(4, u64::MAX).unwrap();
        }
        transaction.commit().unwrap();
        assert!(catalog.metadata().unwrap_err().contains("capacity"));
        assert!(catalog.search("continuity", 3, None, &Default::default()).unwrap_err().contains("capacity"));
    }
}
