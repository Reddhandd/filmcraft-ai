//! An explicit user-level Codex conversation pin, independent of builds and projects.
use serde::{Deserialize, Serialize};
use std::{io::Read, path::PathBuf};

const MAX_BYTES: u64 = 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Stored {
    version: u32,
    thread_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SessionLock {
    path: Option<PathBuf>,
    loaded: bool,
    pub thread_id: Option<String>,
    pub error: Option<String>,
}

fn valid_id(id: &str) -> bool {
    id.len() == 36 && id.bytes().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == b'-' } else { c.is_ascii_hexdigit() })
}
fn read(path: &std::path::Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() && !m.file_type().is_symlink() => {}
        Ok(_) => return Err("Session lock must be a regular file; repair the user-level Console lock file".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Cannot inspect the persistent Codex session lock: {e}")),
    }
    #[cfg(unix)]
    let file = std::fs::File::from(
        rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )
        .map_err(|e| format!("Cannot open the persistent Codex session lock: {e}"))?,
    );
    #[cfg(not(unix))]
    let file = std::fs::File::open(path).map_err(|e| format!("Cannot open the persistent Codex session lock: {e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Session lock is not a regular file".into());
    }
    let mut data = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut data).map_err(|e| format!("Cannot read the persistent Codex session lock: {e}"))?;
    if data.len() as u64 > MAX_BYTES {
        return Err("Persistent Codex session lock exceeds 1 KiB; repair the user-level Console lock file".into());
    }
    Ok(Some(data))
}
impl SessionLock {
    pub fn use_storage(&mut self, path: Option<PathBuf>) {
        self.path = path;
        self.loaded = false;
        self.thread_id = None;
        self.error = None;
    }
    pub fn ensure_loaded(&mut self) {
        if !self.loaded {
            let _ = self.reload();
        }
    }
    /// Refresh at every connection attempt so another build's lock cannot be ignored.
    pub fn reload(&mut self) -> Result<Option<String>, String> {
        self.loaded = true;
        let result: Result<Option<String>, String> = (|| {
            let Some(path) = &self.path else { return Ok(None) };
            let Some(data) = read(path)? else { return Ok(None) };
            let stored: Stored = serde_json::from_slice(&data).map_err(|_| "Invalid persistent Codex session lock; repair the user-level Console lock file")?;
            if stored.version != 1 || stored.thread_id.as_deref().is_some_and(|id| !valid_id(id)) {
                return Err("Unsupported or invalid persistent Codex session lock; repair the user-level Console lock file".into());
            }
            Ok(stored.thread_id)
        })();
        match &result {
            Ok(id) => {
                self.thread_id = id.clone();
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error.clone());
            }
        }
        result
    }
    pub fn store(&mut self, thread_id: Option<String>) -> Result<(), String> {
        if thread_id.as_deref().is_some_and(|id| !valid_id(id)) {
            return Err("Session lock requires a valid Codex conversation UUID".into());
        }
        let path = self.path.as_ref().ok_or("User-level Codex session lock storage is unavailable")?;
        let parent = path.parent().ok_or("Invalid Codex session lock location")?;
        std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create user-level Console storage: {e}"))?;
        // Refuse special files and unreadable/oversized policy data rather than overwrite them.
        let _ = read(path)?;
        let data = serde_json::to_vec(&Stored { version: 1, thread_id: thread_id.clone() }).map_err(|e| e.to_string())?;
        filmcraft_format::atomic::atomic_write(path, &data).map_err(|e| format!("Cannot save Codex session lock: {e}"))?;
        self.thread_id = thread_id;
        self.error = None;
        self.loaded = true;
        Ok(())
    }
    pub fn resolve(&mut self, requested: Option<String>, connect_default: bool) -> Result<Option<String>, String> {
        let Some(locked) = self.reload()? else { return Ok(requested) };
        if connect_default || requested.as_deref() == Some(&locked) {
            Ok(Some(locked))
        } else {
            Err("Codex is locked to one conversation. Unlock Session before starting or choosing another conversation".into())
        }
    }
    #[cfg(test)]
    pub(super) fn at(path: PathBuf) -> Self {
        Self { path: Some(path), ..Self::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "00000000-0000-7000-8000-000000000001";
    #[test]
    fn lock_survives_new_build_state_and_blocks_new_or_other_threads() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("Console/codex-session-lock.json");
        let mut first = SessionLock::at(p.clone());
        first.store(Some(ID.into())).unwrap();
        let mut second = SessionLock::at(p);
        assert_eq!(second.resolve(None, true).unwrap().as_deref(), Some(ID));
        assert!(second.resolve(None, false).is_err());
        assert!(second.resolve(Some("00000000-0000-7000-8000-000000000002".into()), false).is_err());
        assert_eq!(second.resolve(Some(ID.into()), false).unwrap().as_deref(), Some(ID));
        first.store(None).unwrap();
        assert!(second.resolve(None, false).unwrap().is_none());
    }
    #[test]
    fn failed_policy_write_and_special_files_never_clear_an_existing_lock() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("lock.json");
        let mut lock = SessionLock::at(path.clone());
        lock.store(Some(ID.into())).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(lock.store(None).is_err());
        assert_eq!(lock.thread_id.as_deref(), Some(ID));
        assert!(lock.resolve(None, true).is_err());
        #[cfg(unix)]
        {
            std::fs::remove_dir(&path).unwrap();
            let target = d.path().join("target.json");
            std::fs::write(&target, b"protected").unwrap();
            std::os::unix::fs::symlink(&target, &path).unwrap();
            assert!(lock.store(None).is_err());
            assert!(lock.resolve(None, true).is_err());
            assert_eq!(std::fs::read(&target).unwrap(), b"protected");
        }
    }

    #[test]
    fn malformed_or_oversized_policy_fails_closed_and_invalid_write_preserves_lock() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("lock.json");
        let mut lock = SessionLock::at(p.clone());
        lock.store(Some(ID.into())).unwrap();
        let original = std::fs::read(&p).unwrap();
        for id in ["", "../thread", "a", "00000000-0000-7000-8000-00000000000z"] {
            assert!(lock.store(Some(id.into())).is_err());
            assert_eq!(std::fs::read(&p).unwrap(), original);
        }
        for bytes in [b"{".to_vec(), vec![b'x'; 1025], br#"{"version":2,"threadId":null}"#.to_vec(), br#"{"version":1,"threadId":"bad"}"#.to_vec()] {
            std::fs::write(&p, bytes).unwrap();
            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lock.resolve(None, true))).unwrap().is_err());
            assert!(lock.error.is_some());
        }
    }
}
