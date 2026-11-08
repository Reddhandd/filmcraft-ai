//! Stable setup command, ephemeral private credentials, and one live app owner.
use serde_json::json;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
pub(super) struct Profile {
    _lock: File,
    descriptor: PathBuf,
    setup: PathBuf,
    published: bool,
}
fn regular(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err("Desktop profile must use regular private files".into()),
    }
}
fn atomic_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    regular(path)?;
    let parent = path.parent().ok_or("Desktop profile has no storage directory")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(|_| "Cannot stage private Desktop profile")?;
    staged.write_all(bytes).and_then(|()| staged.as_file().sync_all()).map_err(|_| "Cannot save private Desktop profile")?;
    staged.persist(path).map_err(|_| "Cannot publish private Desktop profile")?;
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}
impl Profile {
    pub fn publish(root: &Path, cli: &Path, port: u16, control: &str, permit: &str) -> Result<Self, String> {
        if port == 0 || [control, permit].iter().any(|value| value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())) {
            return Err("Desktop profile needs a valid private FilmCraft bridge".into());
        }
        let root = std::path::absolute(root).map_err(|_| "Cannot resolve Desktop profile storage")?;
        let descriptor = root.join("connection.json");
        let setup = root.join("setup.json");
        let descriptor_text = descriptor.to_str().ok_or("Desktop profile path is not valid Unicode")?;
        if descriptor_text.len() > 8192 {
            return Err("Desktop profile paths exceed their byte budget".into());
        }
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&root).map_err(|_| "Cannot create private Desktop profile storage")?;
        let metadata = std::fs::symlink_metadata(&root).map_err(|_| "Cannot inspect Desktop profile storage")?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("Desktop profile storage must be a private directory".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err("Desktop profile storage must be private to its owner".into());
            }
        }
        let lock_path = root.join("owner.lock");
        regular(&lock_path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(lock_path).map_err(|_| "Cannot open Desktop profile ownership lock")?;
        lock.try_lock().map_err(|_| "Another FilmCraft instance already owns this Desktop profile; disconnect it or use a separate data directory")?;
        let live = serde_json::to_vec(&json!({"version":1,"provider":"desktopChatGpt","port":port,"controlToken":control,"permit":permit}))
            .map_err(|_| "Cannot encode private Desktop connection")?;
        let configuration = serde_json::to_vec(&super::setup::encode(
            cli,
            &["mcp".into(), "--controller-console".into(), "--provider-profile".into(), descriptor_text.into()],
            None,
        )?)
        .map_err(|_| "Cannot encode Desktop setup configuration")?;
        let mut profile = Self { _lock: lock, descriptor, setup, published: false };
        atomic_private(&profile.descriptor, &live)?;
        profile.published = true;
        atomic_private(&profile.setup, &configuration)?;
        Ok(profile)
    }
    pub fn setup_path(&self) -> &Path {
        &self.setup
    }
    #[cfg(test)]
    pub fn descriptor_path(&self) -> &Path {
        &self.descriptor
    }
}
impl Drop for Profile {
    fn drop(&mut self) {
        if !self.published {
            return;
        }
        match std::fs::remove_file(&self.descriptor) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => log::warn!("Could not remove private Desktop connection descriptor; its ownership permit is revoked"),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_uses_identical_setup_but_rotates_private_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Desktop");
        let cli = std::env::current_exe().unwrap();
        let first = Profile::publish(&root, &cli, 9876, &"a".repeat(64), &"b".repeat(64)).unwrap();
        let setup = std::fs::read(first.setup_path()).unwrap();
        let descriptor = first.descriptor_path().to_path_buf();
        let first_state: serde_json::Value = serde_json::from_slice(&std::fs::read(&descriptor).unwrap()).unwrap();
        assert!(!String::from_utf8_lossy(&setup).contains(&"b".repeat(64)));
        assert!(Profile::publish(&root, &cli, 9877, &"c".repeat(64), &"d".repeat(64)).is_err());
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&std::fs::read(&descriptor).unwrap()).unwrap(), first_state);
        drop(first);
        assert!(!descriptor.exists());
        let second = Profile::publish(&root, &cli, 9877, &"c".repeat(64), &"d".repeat(64)).unwrap();
        assert_eq!(std::fs::read(second.setup_path()).unwrap(), setup);
        let second_state: serde_json::Value = serde_json::from_slice(&std::fs::read(second.descriptor_path()).unwrap()).unwrap();
        assert_ne!(second_state["permit"], first_state["permit"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(second.descriptor_path()).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
    #[cfg(unix)]
    #[test]
    fn rejected_profile_symlink_does_not_delete_or_replace_the_original() {
        use std::os::unix::fs::{DirBuilderExt, symlink};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Desktop");
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(&root).unwrap();
        let original = directory.path().join("original");
        std::fs::write(&original, b"preserve me").unwrap();
        let descriptor = root.join("connection.json");
        symlink(&original, &descriptor).unwrap();
        assert!(Profile::publish(&root, &std::env::current_exe().unwrap(), 9876, &"a".repeat(64), &"b".repeat(64)).is_err());
        assert!(std::fs::symlink_metadata(&descriptor).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(original).unwrap(), b"preserve me");
    }
}
