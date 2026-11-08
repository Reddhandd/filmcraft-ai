//! Atomic index publication. Original private books are not repository or release assets.
use super::model::*;
use filmcraft_format::atomic;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
pub(super) fn read(path: &Path, max: usize) -> Result<Option<Vec<u8>>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > max as u64 {
        return Err("Reference input must be a bounded regular file, not a link".into());
    }
    #[cfg(unix)]
    let file = std::fs::File::from(
        rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|e| e.to_string())?,
    );
    #[cfg(not(unix))]
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Reference input must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1) as u64).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > max {
        return Err("Reference file grew beyond its size limit".into());
    }
    Ok(Some(bytes))
}
fn catalog(root: &Path) -> Result<Option<super::disk_index::Catalog>, String> {
    let path = root.join("index.redb");
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() == 0 || metadata.len() > 8 * 1024 * 1024 * 1024 => {
            return Err("Disk index must be a bounded regular database; originals and legacy index are preserved".into());
        }
        Ok(_) => {}
    }
    #[cfg(unix)]
    let file = std::fs::File::from(
        rustix::fs::open(
            &path,
            rustix::fs::OFlags::RDWR | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|error| error.to_string())?,
    );
    #[cfg(not(unix))]
    let file = std::fs::OpenOptions::new().read(true).write(true).open(&path).map_err(|error| error.to_string())?;
    super::disk_index::Catalog::open(file).map(Some)
}
fn migrate(root: &Path, source: &Index) -> Result<(), String> {
    for document in &source.documents {
        super::recovery::save(root, document)?;
    }
    write_disk(root, source, source.documents.iter().cloned().map(Ok), false)
}
fn write_disk(root: &Path, source: &Index, items: impl Iterator<Item = Result<Document, String>>, replace_bad: bool) -> Result<(), String> {
    let mut token = [0u8; 8];
    getrandom::fill(&mut token).map_err(|error| error.to_string())?;
    let token = token.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let stage = root.join(format!("index-migration-{token}.redb"));
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&stage).map_err(|error| error.to_string())?;
    let result = (|| {
        let catalog = super::disk_index::Catalog::open(file)?;
        super::recovery::counters(root, source.generation, source.next_id)?;
        catalog.migrate_documents(source, items)?;
        let verified = catalog.metadata()?;
        if verified.generation != source.generation || verified.next_id != source.next_id || verified.documents.len() != source.documents.len() {
            return Err("Disk-index migration verification failed; legacy data is unchanged".into());
        }
        drop(catalog);
        // An exclusive link publishes the completed file without replacing another path.
        if replace_bad {
            let current = root.join("index.redb");
            let metadata = std::fs::symlink_metadata(&current).map_err(|error| error.to_string())?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err("Refuse to replace an unsafe recovery destination".into());
            }
            std::fs::hard_link(&current, root.join(format!("index-damaged-{token}.redb"))).map_err(|error| error.to_string())?;
            std::fs::rename(&stage, &current).map_err(|error| error.to_string())?;
            return Ok(());
        }
        std::fs::hard_link(&stage, root.join("index.redb"))
            .map_err(|error| format!("Cannot publish migrated disk index: {error}. Legacy data is unchanged."))?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&stage);
    result
}
fn load_raw(root: &Path) -> Result<(Index, bool), String> {
    let Some(bytes) = read(&root.join("index.json"), MAX_INDEX)? else {
        return read(&root.join("index.previous.json"), MAX_INDEX)?
            .map(|bytes| Index::decode(&bytes).map(|index| (index, true)))
            .transpose()
            .map(|previous| previous.unwrap_or_else(|| (Index::empty(), false)));
    };
    match Index::decode(&bytes) {
        Ok(i) => Ok((i, false)),
        Err(current) => match read(&root.join("index.previous.json"), MAX_INDEX)?.map(|b| Index::decode(&b)).transpose()? {
            Some(i) => Ok((i, true)),
            None => Err(format!("{current}; no previous reference index is available. Originals are preserved.")),
        },
    }
}
fn removed_ids(root: &Path) -> Result<std::collections::BTreeSet<u64>, String> {
    let ids: std::collections::BTreeSet<u64> = read(&root.join("removed.json"), 512 * 1024)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|e| format!("Cannot read reference removals: {e}")))
        .transpose()?
        .unwrap_or_default();
    if ids.len() > 32_768 || ids.contains(&0) {
        return Err("Invalid or oversized reference removal ledger".into());
    }
    Ok(ids)
}
pub fn load(root: &Path) -> Result<(Index, bool), String> {
    let (mut index, recovered) = match catalog(root)? {
        Some(catalog) => (catalog.metadata()?, false),
        None => load_raw(root)?,
    };
    let removed = removed_ids(root)?;
    index.documents.retain(|d| !removed.contains(&d.id));
    index.terms.retain(|_, positions| {
        positions.retain(|(id, _)| !removed.contains(id));
        !positions.is_empty()
    });
    if let Some(last) = removed.last() {
        index.next_id = index.next_id.max(*last);
    }
    index.validate()?;
    Ok((index, recovered))
}
fn publish(root: &Path, index: &Index, previous: &Index) -> Result<(), String> {
    if let Some(catalog) = catalog(root)? {
        for document in &index.documents {
            let previous = previous.documents.iter().find(|old| old.id == document.id);
            if !document.passages.is_empty() || previous.is_none_or(|old| old.enabled != document.enabled) {
                super::recovery::save(root, document)?;
            }
        }
        super::recovery::counters(root, index.generation, index.next_id)?;
        return catalog.replace(previous, index);
    }
    let bytes = index.encode()?;
    atomic::atomic_write(&root.join("index.previous.json"), &previous.encode()?).map_err(|e| format!("Cannot preserve previous reference index: {e}"))?;
    atomic::atomic_write(&root.join("index.json"), &bytes).map_err(|e| format!("Cannot save reference index; previous index retained: {e}"))
}
fn lock(root: &Path) -> Result<std::fs::File, String> {
    std::fs::create_dir_all(root).map_err(|e| format!("Cannot create reference library: {e}"))?;
    let path = root.join("library.lock");
    #[cfg(unix)]
    let f = std::fs::File::from(
        rustix::fs::open(
            &path,
            rustix::fs::OFlags::RDWR | rustix::fs::OFlags::CREATE | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .map_err(|e| e.to_string())?,
    );
    #[cfg(not(unix))]
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path).map_err(|e| e.to_string())?;
    if !f.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Reference lock must be a regular file".into());
    }
    f.try_lock().map_err(|e| format!("Reference library is busy; retry later: {e}"))?;
    Ok(f)
}
pub fn perform(root: &Path, action: &str, params: &Value) -> Result<Value, String> {
    perform_controlled(root, action, params, &super::job::Control::default())
}
pub(super) fn perform_controlled(root: &Path, action: &str, params: &Value, control: &super::job::Control) -> Result<Value, String> {
    control.check()?;
    let _lock = lock(root)?;
    if action == "ocrInstall" {
        return super::ocr_models::install(root);
    }
    let disk = catalog(root);
    let missing_with_recovery = matches!(&disk, Ok(None)) && read(&root.join("BookRecovery/counters.json"), 1024)?.is_some();
    if missing_with_recovery && action != "recover" {
        return Err("Search database is missing; use library recovery to restore retained book records. Originals and legacy data are preserved".into());
    }
    if action == "recover" && (disk.is_err() || missing_with_recovery) {
        let metadata = super::recovery::metadata(root, &removed_ids(root)?)?;
        let ids: Vec<_> = metadata.documents.iter().map(|document| document.id).collect();
        write_disk(root, &metadata, ids.into_iter().map(|id| super::recovery::read(root, id)), !missing_with_recovery)?;
        return Ok(
            json!({"recovered":true,"generation":metadata.generation,"warning":"Disk index rebuilt from retained book records. Review recent additions and enabled books; removed identities remain excluded. Damaged database retained for diagnosis."}),
        );
    }
    drop(disk);
    let (legacy, recovered) = load(root)?;
    if !recovered && catalog(root)?.is_none() {
        migrate(root, &legacy)?;
    }
    drop(legacy);
    let (index, recovered) = load(root)?;
    match action {
        "inspect" => {
            let number = |key: &str, default: usize, max: usize| -> Result<usize, String> {
                params
                    .get(key)
                    .map(|v| v.as_u64().and_then(|n| usize::try_from(n).ok()).filter(|n| *n <= max).ok_or_else(|| format!("Invalid {key}")))
                    .transpose()
                    .map(|n| n.unwrap_or(default))
            };
            let offset = number("offset", 0, MAX_DOCUMENTS)?;
            let limit = number("limit", 16, 16)?;
            if limit == 0 {
                return Err("Catalog limit must be 1–16".into());
            }
            if let Some(expected) = params.get("expectedGeneration")
                && expected.as_u64() != Some(index.generation)
            {
                return Err("Reference catalog changed; restart inspection at offset zero".into());
            }
            let end = offset.saturating_add(limit).min(index.documents.len());
            Ok(
                json!({"schema":1,"generation":index.generation,"recoveredPrevious":recovered,"total":index.documents.len(),"offset":offset,"nextOffset":(end<index.documents.len()).then_some(end),"documents":index.documents.iter().skip(offset).take(limit).map(|d|json!({"id":d.id,"title":d.title,"source":d.source_name,"pages":if d.chapters.is_empty() {Some(d.pages)} else {None},"sections":(!d.chapters.is_empty()).then_some(d.pages),"passages":d.passage_count.map(|count|count as usize).unwrap_or(d.passages.len()),"enabled":d.enabled,"importedAt":d.imported_at,"warning":d.import_warning})).collect::<Vec<_>>(),"coverage":"Persistent local extracted-text index; only requested search excerpts go to the AI. No model training or retained page images. Scanned pages use local OCR when verified models are installed."}),
            )
        }
        "recover" => {
            if !recovered {
                return Err("Current reference index is valid; recovery is unnecessary".into());
            }
            let mut next = index.clone();
            next.rebuild()?;
            publish(root, &next, &index)?;
            Ok(
                json!({"recovered":true,"generation":next.generation,"warning":"Previous reference index restored; review recent additions. Removed documents remain excluded."}),
            )
        }
        "search" => {
            if recovered {
                return Err("Recovered an older reference index. Review the library before searching; removed content may be present in the backup.".into());
            }
            let query = params.get("query").and_then(Value::as_str).ok_or("Supply a search query")?;
            let limit = params.get("limit").map(|v| v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or("limit must be 1–5")).transpose()?.unwrap_or(3);
            let selected: Option<std::collections::BTreeSet<u64>> = serde_json::from_value(params.get("selectedDocuments").cloned().unwrap_or(Value::Null))
                .map_err(|e| format!("Invalid reference selection: {e}"))?;
            if let Some(catalog) = catalog(root)? {
                catalog.search(query, limit, selected.as_ref(), &removed_ids(root)?)
            } else {
                match selected.as_ref() {
                    Some(ids) => index.search_selected(query, limit, Some(ids)),
                    None => index.search(query, limit),
                }
            }
        }
        "page" | "illustration" => {
            if recovered {
                return Err("Recover the reference catalog before requesting page images".into());
            }
            if params.get("expectedGeneration").and_then(Value::as_u64) != Some(index.generation) {
                return Err("Library changed; inspect or search again before requesting a page image".into());
            }
            let id = params.get("documentId").and_then(Value::as_u64).ok_or("Choose an admitted document identity")?;
            let selected: Option<std::collections::BTreeSet<u64>> =
                serde_json::from_value(params.get("selectedDocuments").cloned().unwrap_or(Value::Null)).map_err(|error| error.to_string())?;
            if selected.as_ref().is_some_and(|ids| !ids.contains(&id)) {
                return Err("This book is not selected for the current project".into());
            }
            let document = index.documents.iter().find(|document| document.id == id && document.enabled).ok_or("Book is removed or disabled")?;
            if action == "illustration" {
                if document.chapters.is_empty() {
                    return Err("Choose an EPUB illustration resource, not a PDF page".into());
                }
                let resource = params.get("resource").and_then(Value::as_str).ok_or("Choose an illustration resource identity from a cited EPUB section")?;
                let bytes = read(&root.join(format!("document-{id}.epub")), 256 * 1024 * 1024)?.ok_or("Original EPUB is missing")?;
                if !document.source_sha256.is_empty()
                    && Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>() != document.source_sha256
                {
                    return Err("Original EPUB changed; reimport before requesting illustrations".into());
                }
                let (admitted, data) = super::epub::illustration(&bytes, resource)?;
                let mut image = super::super::references::library_image(&data, &admitted.media)?;
                image["documentId"] = json!(id);
                image["title"] = json!(document.title);
                image["resource"] = json!(resource);
                image["section"] = json!(admitted.section);
                image["chapter"] = json!(admitted.chapter);
                image["generation"] = json!(index.generation);
                return Ok(image);
            }
            if !document.chapters.is_empty()
                || Path::new(&document.source_name)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_none_or(|extension| !extension.eq_ignore_ascii_case("pdf"))
            {
                return Err("Choose a PDF physical page; EPUB illustration requests use resource identities".into());
            }
            let page = params
                .get("page")
                .and_then(Value::as_u64)
                .and_then(|page| u32::try_from(page).ok())
                .filter(|page| *page > 0 && *page <= document.pages)
                .ok_or("Choose an existing physical page")?;
            let side = params
                .get("maxSide")
                .map(|value| {
                    value
                        .as_u64()
                        .and_then(|side| u32::try_from(side).ok())
                        .filter(|side| (256..=1600).contains(side))
                        .ok_or("Image size must be 256–1600 pixels")
                })
                .transpose()?
                .unwrap_or(1600);
            let bytes = read(&root.join(format!("document-{id}.pdf")), 64 * 1024 * 1024)?.ok_or("Original book is missing")?;
            if !document.source_sha256.is_empty()
                && Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>() != document.source_sha256
            {
                return Err("Original book changed; reimport before requesting images".into());
            }
            let executable = std::env::current_exe().map_err(|error| error.to_string())?;
            let mut process = crate::pdf_reference::process::Process::new(&executable, Arc::new(AtomicBool::new(false)))?;
            process.load_images(Arc::from(bytes), Duration::from_secs(20))?;
            let mut image = process.render(page, side, Duration::from_secs(20))?;
            image["documentId"] = json!(id);
            image["title"] = json!(document.title);
            image["generation"] = json!(index.generation);
            image["sourceWarning"] = json!(document.import_warning);
            Ok(image)
        }
        "import" | "importPrepared" => {
            if recovered {
                return Err("Restore a valid reference index before adding books".into());
            }
            import(root, index, params, control)
        }
        "remove" | "enable" => {
            if recovered {
                return Err("Reference index is damaged; restore a valid index before modifying its catalog".into());
            }
            let id = params.get("documentId").and_then(Value::as_u64).ok_or("Choose a document identity")?;
            let mut next = index.clone();
            let position = next.documents.iter().position(|d| d.id == id).ok_or("Reference document no longer exists")?;
            if action == "remove" {
                let mut removed = removed_ids(root)?;
                if removed.len() >= 32_768 {
                    return Err("Reference removal ledger is full; archive this library before further removals".into());
                }
                removed.insert(id);
                atomic::atomic_write(&root.join("removed.json"), &serde_json::to_vec(&removed).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("Cannot record permanent reference removal: {e}"))?;
                next.documents.remove(position);
            } else {
                next.documents.get_mut(position).ok_or("Reference document missing")?.enabled =
                    params.get("enabled").and_then(Value::as_bool).ok_or("enabled must be boolean")?;
            }
            next.rebuild()?;
            publish(root, &next, &index)?;
            // A deletion must never fall back to an index containing the removed document.
            if catalog(root)?.is_none() {
                atomic::atomic_write(&root.join("index.previous.json"), &next.encode()?)
                    .map_err(|e| format!("Index updated but backup cleanup failed: {e}"))?;
            }
            let mut warning = None;
            if action == "remove" {
                for extension in ["pdf", "txt", "md", "epub"] {
                    if let Err(e) = std::fs::remove_file(root.join(format!("document-{id}.{extension}")))
                        && e.kind() != std::io::ErrorKind::NotFound
                    {
                        warning = Some(format!("Removed from index; original cleanup failed: {e}"));
                    }
                }
            }
            Ok(json!({"updated":true,"generation":next.generation,"warning":warning}))
        }
        _ => Err("Unsupported reference library operation".into()),
    }
}
fn import(root: &Path, index: Index, params: &Value, control: &super::job::Control) -> Result<Value, String> {
    let path = Path::new(params.get("path").and_then(Value::as_str).ok_or("Select a local PDF, EPUB, TXT or MD document")?);
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    if !["pdf", "txt", "md", "epub"].contains(&extension.as_str()) {
        return Err("Reference library supports PDF, unencrypted EPUB, UTF-8 TXT and MD documents".into());
    }
    let source_limit = if extension == "epub" { 256 * 1024 * 1024 } else { 64 * 1024 * 1024 };
    let bytes = if extension == "epub" && std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
        super::epub::directory_snapshot(path)?
    } else {
        read(path, source_limit)?.ok_or("Selected reference file is missing")?
    };
    let source_sha256 = Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let source = path.file_name().and_then(|s| s.to_str()).ok_or("Reference filename must be UTF-8")?;
    if source.len() > 512 {
        return Err("Reference filename exceeds 512 bytes".into());
    }
    // Compare the saved source bytes only during import, avoiding duplicate copies without unstable IDs.
    for d in &index.documents {
        let old_extension = Path::new(&d.source_name).extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
        let original = root.join(format!("document-{}.{}", d.id, old_extension));
        // New catalogs compare stable content identities without loading another full book.
        let duplicate = if !d.source_sha256.is_empty() {
            d.source_sha256 == source_sha256
                && std::fs::symlink_metadata(&original).is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        } else {
            // Migration path for the old 64 MiB format; each legacy read remains bounded.
            read(&original, 64 * 1024 * 1024)?.is_some_and(|old| old == bytes)
        };
        if duplicate {
            return Ok(json!({"imported":false,"duplicate":true,"documentId":d.id,"title":d.title}));
        }
    }
    if index.documents.len() >= MAX_DOCUMENTS {
        return Err("Reference library is full (512 documents)".into());
    }
    // An index backup may predate retained originals. Never reuse or overwrite their identities.
    let mut id = index.next_id;
    let mut allocated = false;
    for _ in 0..=MAX_DOCUMENTS {
        id = id.checked_add(1).ok_or("Reference document identities exhausted")?;
        let mut occupied = false;
        for suffix in ["pdf", "txt", "md", "epub"] {
            match std::fs::symlink_metadata(root.join(format!("document-{id}.{suffix}"))) {
                Ok(_) => occupied = true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        if !occupied {
            allocated = true;
            break;
        }
    }
    if !allocated {
        return Err("Too many retained unindexed originals; recover the reference catalog before adding books".into());
    }
    let mut passages_out = Vec::new();
    let pages;
    let mut chapters = Vec::new();
    let mut import_warning = String::new();
    if let Some(prepared_path) = params.get("preparedPath").and_then(Value::as_str) {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Prepared {
            source_name: String,
            source_bytes: u64,
            pages: Vec<String>,
        }
        let data = read(Path::new(prepared_path), MAX_INDEX)?.ok_or("Prepared extraction is missing")?;
        let prepared: Prepared = serde_json::from_slice(&data).map_err(|e| format!("Invalid prepared extraction: {e}"))?;
        if prepared.source_name != source || prepared.source_bytes != bytes.len() as u64 || prepared.pages.is_empty() || prepared.pages.len() > 4096 {
            return Err("Prepared extraction does not match the selected original".into());
        }
        pages = u32::try_from(prepared.pages.len()).map_err(|_| "Invalid prepared page count")?;
        let mut total = 0usize;
        for (n, text) in prepared.pages.iter().enumerate() {
            total = total.saturating_add(text.len());
            if text.len() > 64 * 1024 || total > MAX_TEXT {
                return Err("Prepared text exceeds page or document budget".into());
            }
            let page = u32::try_from(n.saturating_add(1)).map_err(|_| "Invalid prepared page")?;
            passages_out.extend(passages(page, text));
        }
    } else if extension == "epub" {
        let book = super::epub::parse(&bytes).map_err(|error| format!("Cannot index {source}: {error}. Existing library is unchanged."))?;
        pages = u32::try_from(book.chapters.len()).map_err(|_| "Invalid EPUB section count")?;
        for (n, (title, text)) in book.chapters.into_iter().enumerate() {
            chapters.push(title);
            let section = u32::try_from(n.saturating_add(1)).map_err(|_| "Invalid EPUB section ordinal")?;
            passages_out.extend(passages(section, &text));
            if passages_out.len() > MAX_PASSAGES {
                return Err("EPUB exceeds the reference passage budget".into());
            }
        }
    } else if extension == "pdf" {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut process = crate::pdf_reference::process::Process::new(&executable, control.cancelled.clone())?;
        let loaded = process
            .load(Arc::from(bytes.clone()), Duration::from_secs(20))
            .map_err(|error| format!("Cannot index {source}: {error}. Existing library is unchanged."))?;
        if loaded.get("recoveredStructure").and_then(Value::as_bool) == Some(true) {
            import_warning = "PDF structure was recovered from malformed cross-reference data. Physical page identities and counts were checked; verify important excerpts against the original.".into();
        }
        pages = u32::try_from(loaded.get("pages").or_else(|| loaded.get("pageCount")).and_then(Value::as_u64).ok_or("PDF helper did not return a page count")?)
            .map_err(|_| "Invalid PDF page count")?;
        if pages == 0 || pages > 4096 {
            return Err("Reference PDF requires 1–4096 pages".into());
        }
        let mut ocr_process = None;
        let mut models_checked = false;
        let mut used_ocr = false;
        let mut unrecognized_pages = 0u32;
        let mut total = 0usize;
        for page in 1..=pages {
            let mut offset = 0usize;
            let mut text = String::new();
            control.progress(page, pages);
            control.check()?;
            for _ in 0..17 {
                control.check()?;
                let result = process
                    .read(page, offset, 4096, Duration::from_secs(3))
                    .map_err(|error| format!("Cannot index {source}, page {page}: {error}. Existing library is unchanged."))?;
                text.push_str(result.get("text").and_then(Value::as_str).ok_or("PDF helper returned no text")?);
                match result.get("nextOffset").and_then(Value::as_u64) {
                    Some(n) => {
                        let n = usize::try_from(n).map_err(|_| "Invalid PDF text offset")?;
                        if n <= offset {
                            return Err("PDF text read made no progress".into());
                        }
                        offset = n;
                    }
                    None => break,
                }
            }
            let mut recognized = false;
            if text.trim().is_empty() {
                if !models_checked {
                    models_checked = true;
                    if let Some(models) = super::ocr_models::read(root)? {
                        let mut child = crate::pdf_reference::process::Process::new(&executable, control.cancelled.clone())?;
                        child.load_images(Arc::from(bytes.clone()), Duration::from_secs(20))?;
                        child.load_ocr(&models.detection, &models.recognition, Duration::from_secs(20))?;
                        ocr_process = Some(child);
                    }
                }
                if let Some(child) = &mut ocr_process {
                    let result = child
                        .ocr_page(page, Duration::from_secs(20))
                        .map_err(|error| format!("Cannot recognize {source}, physical page {page}: {error}. Existing library is unchanged."))?;
                    text = result.get("text").and_then(Value::as_str).ok_or("OCR helper returned no text")?.to_owned();
                    recognized = true;
                    used_ocr = true;
                }
            }
            if text.trim().is_empty() && !recognized {
                unrecognized_pages = unrecognized_pages.saturating_add(1);
            }
            control.check()?;
            total = total.saturating_add(text.len());
            if total > MAX_TEXT {
                return Err("Reference text exceeds 8 MiB; split into volumes".into());
            }
            let mut extracted = passages(page, &text);
            if recognized {
                for passage in &mut extracted {
                    passage.text = format!("[Local OCR · physical page {page} · verify recognition against the original]\n{}", passage.text);
                }
            }
            passages_out.extend(extracted);
            if passages_out.len() > MAX_PASSAGES {
                return Err("Reference passage budget exceeded".into());
            }
        }
        if unrecognized_pages > 0 {
            if !import_warning.is_empty() {
                import_warning.push(' ');
            }
            import_warning.push_str(&format!("{unrecognized_pages} physical pages contained no extractable text and were not OCR-processed. They may be blank or scanned; install local OCR models, then remove and add this book again for fuller coverage."));
        }
        if used_ocr {
            if !import_warning.is_empty() {
                import_warning.push(' ');
            }
            import_warning.push_str("Some pages use local OCR; verify important quotations and numbers against the original.");
        }
    } else {
        let text = std::str::from_utf8(&bytes).map_err(|_| "Reference text must be UTF-8")?;
        if text.len() > MAX_TEXT {
            return Err("Reference text exceeds 8 MiB".into());
        }
        pages = 1;
        passages_out = passages(1, text);
    }
    if passages_out.iter().all(|p| p.text.trim().is_empty()) {
        return Err("No searchable text found. For scanned books, install the verified local OCR models and retry; the existing library is unchanged.".into());
    }
    let mut next = index.clone();
    next.next_id = id;
    let passage_count = u32::try_from(passages_out.len()).map_err(|_| "Invalid passage count")?;
    next.documents.push(Document {
        id,
        title: source.to_owned(),
        source_name: source.to_owned(),
        source_sha256,
        imported_at: chrono::Utc::now().to_rfc3339(),
        import_warning: import_warning.clone(),
        pages,
        chapters,
        enabled: true,
        passages: passages_out,
        passage_count: Some(passage_count),
    });
    next.rebuild()?;
    if catalog(root)?.is_none() {
        let _ = next.encode()?;
    }
    control.publish()?;
    let destination = root.join(format!("document-{id}.{extension}"));
    atomic::atomic_write_new(&destination, &bytes).map_err(|e| format!("Cannot retain reference book: {e}"))?;
    if let Err(e) = publish(root, &next, &index) {
        // A storage error at commit can be ambiguous. Retain the original and any
        // recovery record rather than deleting bytes a committed catalog may reference.
        return Err(format!("{e}. The original book was retained for recovery; inspect the library before retrying."));
    }
    Ok(json!({"imported":true,"documentId":id,"pages":pages,"generation":next.generation,"warning":import_warning}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn epub_import_search_and_restart_use_sections_instead_of_fictional_pdf_pages() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Band procedure.epub");
        let root = temp.path().join("library");
        std::fs::write(&source, super::super::epub::tests::fixture(None)).unwrap();
        let result = perform(&root, "import", &json!({"path":source})).unwrap();
        assert_eq!(result["imported"], true);
        assert_eq!(perform(&root, "import", &json!({"path":source})).unwrap()["duplicate"], true);
        let hit = perform(&root, "search", &json!({"query":"performance"})).unwrap();
        assert_eq!(hit["hits"][0]["chapter"], "Band sync");
        assert_eq!(hit["hits"][0]["section"], 1);
        assert!(hit["hits"][0]["page"].is_null());
        let (restored, _) = load(&root).unwrap();
        assert_eq!(restored.documents[0].chapters, ["Band sync"]);
        assert_eq!(restored.documents[0].source_sha256.len(), 64);
        let before = load(&root).unwrap().0.generation;
        let rejected = temp.path().join("Protected.epub");
        std::fs::write(&rejected, super::super::epub::tests::fixture(Some(("META-INF/encryption.xml", "protected")))).unwrap();
        assert!(perform(&root, "import", &json!({"path":rejected})).is_err());
        assert_eq!(load(&root).unwrap().0.generation, before);
    }
    #[test]
    fn import_reopen_disable_remove_and_corruption_do_not_resurrect_text() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let source = temp.path().join("book.txt");
        std::fs::write(&source, "A match cut preserves continuity. Exposure controls highlight detail.").unwrap();
        let imported = perform(&root, "import", &json!({"path":source})).unwrap();
        let id = imported["documentId"].as_u64().unwrap();
        assert_eq!(perform(&root, "import", &json!({"path":source})).unwrap()["duplicate"], true);
        assert_eq!(perform(&root, "search", &json!({"query":"continuity"})).unwrap()["hits"][0]["documentId"], id);
        perform(&root, "enable", &json!({"documentId":id,"enabled":false})).unwrap();
        assert!(perform(&root, "search", &json!({"query":"continuity"})).unwrap()["hits"].as_array().unwrap().is_empty());
        let stale_backup = std::fs::read(root.join("BookRecovery").join(format!("book-{id}.json"))).unwrap();
        perform(&root, "remove", &json!({"documentId":id})).unwrap();
        std::fs::write(root.join("BookRecovery").join(format!("book-{id}.json")), stale_backup).unwrap();
        assert!(!root.join(format!("document-{id}.txt")).exists());
        std::fs::write(root.join("index.redb"), b"{").unwrap();
        assert!(perform(&root, "inspect", &json!({})).is_err());
        assert!(perform(&root, "search", &json!({"query":"continuity"})).is_err());
        assert!(load(&root).is_err());
        perform(&root, "recover", &json!({})).unwrap();
        assert!(perform(&root, "search", &json!({"query":"continuity"})).unwrap()["hits"].as_array().unwrap().is_empty());
    }
    #[test]
    fn occupied_original_is_preserved_and_bad_files_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("document-1.txt"), b"Retain this orphaned original").unwrap();
        let path = temp.path().join("notes.txt");
        std::fs::write(&path, "continuity").unwrap();
        assert_eq!(perform(&root, "import", &json!({"path":path})).unwrap()["documentId"], 2);
        assert_eq!(std::fs::read(root.join("document-1.txt")).unwrap(), b"Retain this orphaned original");
        assert_eq!(load(&root).unwrap().0.documents.len(), 1);
        assert!(perform(&root, "search", &json!({"query":"x","limit":-1})).is_err());
        assert!(perform(&root, "import", &json!({"path":temp.path()})).is_err());
    }
    #[test]
    fn page_delivery_rejects_stale_selection_parameters_and_changed_original_before_launch() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let source = temp.path().join("Reference.pdf");
        let bytes = b"%PDF-private-synthetic-reference";
        std::fs::write(&source, bytes).unwrap();
        let prepared = temp.path().join("prepared.json");
        std::fs::write(
            &prepared,
            serde_json::to_vec(&json!({"source_name":"Reference.pdf","source_bytes":bytes.len(),"pages":["Continuity and performance reference"]})).unwrap(),
        )
        .unwrap();
        let added = perform(&root, "importPrepared", &json!({"path":source,"preparedPath":prepared})).unwrap();
        let id = added["documentId"].as_u64().unwrap();
        let generation = added["generation"].as_u64().unwrap();
        let request = json!({"documentId":id,"page":1,"maxSide":800,"expectedGeneration":generation});
        for (field, value) in
            [("page", json!(0)), ("maxSide", json!(u64::MAX)), ("expectedGeneration", json!(generation + 1)), ("selectedDocuments", json!([]))]
        {
            let mut invalid = request.clone();
            invalid[field] = value;
            assert!(perform(&root, "page", &invalid).is_err());
        }
        let before = load(&root).unwrap().0.generation;
        std::fs::write(root.join(format!("document-{id}.pdf")), b"changed original").unwrap();
        assert!(perform(&root, "page", &request).unwrap_err().contains("Original book changed"));
        assert_eq!(load(&root).unwrap().0.generation, before);
        perform(&root, "enable", &json!({"documentId":id,"enabled":false})).unwrap();
        let mut disabled = request;
        disabled["expectedGeneration"] = perform(&root, "inspect", &json!({})).unwrap()["generation"].clone();
        assert!(perform(&root, "page", &disabled).unwrap_err().contains("removed or disabled"));
    }
    #[test]
    fn damaged_database_recovery_retains_books_counts_enabled_state_and_advances_generation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let first = temp.path().join("first.txt");
        let second = temp.path().join("second.txt");
        std::fs::write(&first, "Continuity synchronizes performance.").unwrap();
        std::fs::write(&second, "Exposure preserves highlights.").unwrap();
        let a = perform(&root, "import", &json!({"path":first})).unwrap()["documentId"].as_u64().unwrap();
        let b = perform(&root, "import", &json!({"path":second})).unwrap()["documentId"].as_u64().unwrap();
        perform(&root, "enable", &json!({"documentId":b,"enabled":false})).unwrap();
        let generation = load(&root).unwrap().0.generation;
        std::fs::write(root.join("index.redb"), b"damaged database").unwrap();
        assert!(perform(&root, "search", &json!({"query":"continuity"})).is_err());
        let recovered = perform(&root, "recover", &json!({})).unwrap();
        assert!(recovered["generation"].as_u64().unwrap() > generation);
        assert_eq!(perform(&root, "search", &json!({"query":"continuity"})).unwrap()["hits"][0]["documentId"], a);
        assert!(perform(&root, "search", &json!({"query":"exposure"})).unwrap()["hits"].as_array().unwrap().is_empty());
        let catalog = perform(&root, "inspect", &json!({})).unwrap();
        assert_eq!(catalog["total"], 2);
        assert_eq!(catalog["documents"][0]["passages"], 1);
        assert!(root.join(format!("document-{a}.txt")).is_file());
        assert!(root.join(format!("document-{b}.txt")).is_file());
    }
    #[test]
    fn missing_database_requires_explicit_recovery_instead_of_replacing_books_with_legacy_data() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let source = temp.path().join("teaching.txt");
        std::fs::write(&source, "Retain this unique band synchronization teaching").unwrap();
        perform(&root, "import", &json!({"path":source})).unwrap();
        std::fs::remove_file(root.join("index.redb")).unwrap();
        assert!(perform(&root, "inspect", &json!({})).unwrap_err().contains("recovery"));
        assert!(!root.join("index.redb").exists());
        assert_eq!(perform(&root, "recover", &json!({})).unwrap()["recovered"], true);
        assert_eq!(perform(&root, "inspect", &json!({})).unwrap()["total"], 1);
    }
    #[test]
    fn failed_import_publication_preserves_original_for_recovery() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("library");
        perform(&root, "inspect", &json!({})).unwrap();
        let counter = root.join("BookRecovery/counters.json");
        std::fs::remove_file(&counter).unwrap();
        std::fs::create_dir(&counter).unwrap();
        let source = temporary.path().join("teaching.txt");
        std::fs::write(&source, "Keep this band synchronization procedure").unwrap();
        let error = perform(&root, "import", &json!({"path":source})).unwrap_err();
        assert!(error.contains("retained for recovery"));
        assert_eq!(std::fs::read(root.join("document-1.txt")).unwrap(), std::fs::read(&source).unwrap());
        assert!(root.join("BookRecovery/book-1.json").is_file());
        assert_eq!(perform(&root, "inspect", &json!({})).unwrap()["total"], 0);
    }
}
