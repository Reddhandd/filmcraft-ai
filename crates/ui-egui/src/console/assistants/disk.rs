//! Native bounded checkpoint storage, called off the UI thread by the integration layer.
use crate::console::assistants::{Checkpoint, MAX_CHECKPOINT_BYTES, atomic, store};
use std::{fs::File, io::Read, path::Path};

pub struct ScopeLock(File);
impl ScopeLock {
    pub fn take(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("Cannot create assistant storage: {e}"))?;
        let path = dir.join("assistant.write.lock");
        #[cfg(unix)]
        let file = File::from(
            rustix::fs::open(
                &path,
                rustix::fs::OFlags::RDWR
                    | rustix::fs::OFlags::CREATE
                    | rustix::fs::OFlags::NONBLOCK
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            )
            .map_err(|e| format!("Cannot open assistant storage lock: {e}"))?,
        );
        #[cfg(not(unix))]
        let file = {
            if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err("Assistant storage lock cannot be a symlink".into());
            }
            std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path).map_err(|e| e.to_string())?
        };
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("Assistant storage lock must be a regular file".into());
        }
        file.try_lock().map_err(|e| format!("Assistant storage is busy or unavailable; retry: {e}"))?;
        Ok(Self(file))
    }
    pub fn held(&self) -> Result<(), String> {
        self.0.metadata().map(|_| ()).map_err(|e| e.to_string())
    }
}
pub fn read(path: &Path, limit: usize) -> Result<Option<Vec<u8>>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {}
        Ok(_) => return Err("Assistant memory must be a regular file, not a link or device".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Cannot inspect assistant memory: {error}")),
    }
    #[cfg(unix)]
    let file = File::from(
        rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|e| format!("Cannot open assistant memory: {e}"))?,
    );
    #[cfg(not(unix))]
    let file = File::open(path).map_err(|e| format!("Cannot open assistant memory: {e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Assistant memory must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1) as u64).read_to_end(&mut bytes).map_err(|e| format!("Cannot read assistant memory: {e}"))?;
    if bytes.len() > limit {
        return Err(format!("Assistant memory exceeds {limit} bytes"));
    }
    Ok(Some(bytes))
}
pub fn load(dir: &Path, scope: &str) -> Result<Option<(Checkpoint, bool)>, String> {
    store::load(scope, |name| read(&dir.join(name), MAX_CHECKPOINT_BYTES))
}
/// Disk generation is rechecked under the OS lock; stale writers cannot replace newer context.
pub fn save(dir: &Path, next: &Checkpoint) -> Result<(), String> {
    let lock = ScopeLock::take(dir)?;
    lock.held()?;
    let previous = load(dir, &next.project_scope)?.map(|(value, _)| value);
    store::save(next, previous.as_ref(), |name, bytes| atomic::atomic_write(&dir.join(name), bytes).map_err(|e| e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn checkpoint(generation: u64) -> Checkpoint {
        Checkpoint {
            schema: 1,
            project_scope: "A".into(),
            document_epoch: 1,
            revision: 2,
            generation,
            context_version: 0,
            saved_at: "2026-11-01T12:00:00Z".into(),
            conversation: None,
            assistant_name: None,
            autosave: None,
            reference_documents: None,
            assistant: Default::default(),
            decisions: vec![],
            pending: vec![],
            errors: vec![],
            commands: vec![],
            recent_context: vec![],
        }
    }
    #[test]
    fn actual_atomic_generations_recover_and_reject_stale_writer() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &checkpoint(1)).unwrap();
        save(dir.path(), &checkpoint(2)).unwrap();
        assert!(save(dir.path(), &checkpoint(2)).is_err());
        assert_eq!(load(dir.path(), "A").unwrap().unwrap().0.generation, 2);
        std::fs::write(dir.path().join(store::CURRENT), b"truncated").unwrap();
        let (previous, recovered) = load(dir.path(), "A").unwrap().unwrap();
        assert!(recovered);
        assert_eq!(previous.generation, 1);
        save(dir.path(), &checkpoint(2)).unwrap();
        assert!(!load(dir.path(), "A").unwrap().unwrap().1);
    }
    #[test]
    fn competing_writer_reports_busy_and_stale_lock_file_is_reusable() {
        let dir = tempfile::tempdir().unwrap();
        let held = ScopeLock::take(dir.path()).unwrap();
        assert!(save(dir.path(), &checkpoint(1)).is_err());
        drop(held);
        save(dir.path(), &checkpoint(1)).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn links_and_oversized_files_are_rejected_without_changing_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("owner.txt");
        std::fs::write(&target, b"owner").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(read(&link, 1024).is_err());
        std::fs::write(dir.path().join("large"), b"12345").unwrap();
        assert!(read(&dir.path().join("large"), 4).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"owner");
    }
}
