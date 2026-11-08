//! Bounded private Desktop profile. No account credentials or environment fallback.
use super::*;
use serde::Deserialize;
use std::{fs::File, io::Read, path::Path};
const MAX_PROFILE: u64 = 16 * 1024;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Descriptor {
    version: u32,
    provider: String,
    port: u16,
    control_token: String,
    permit: String,
}
impl BridgeClient {
    /// A stable local descriptor published only while FilmCraft selected Desktop Connect.
    pub fn from_provider_profile(path: &Path) -> Result<Self, AutomationError> {
        let error = |message: &str| AutomationError::BadRequest(message.into());
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| error("Desktop profile unavailable; open FilmCraft, select Desktop ChatGPT, Connect, then restart this MCP server"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(error("Desktop profile must be a regular private file"));
        }
        if metadata.len() > MAX_PROFILE {
            return Err(error("Desktop profile exceeds its byte budget"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(error("Desktop profile must be private to its owner"));
            }
        }
        let file = File::open(path).map_err(|_| error("Cannot read private Desktop profile"))?;
        let mut bytes = Vec::new();
        file.take(MAX_PROFILE.saturating_add(1)).read_to_end(&mut bytes).map_err(|_| error("Cannot read private Desktop profile"))?;
        if bytes.len() as u64 > MAX_PROFILE {
            return Err(error("Desktop profile exceeds its byte budget"));
        }
        let profile: Descriptor = serde_json::from_slice(&bytes).map_err(|_| error("Malformed or unsupported Desktop profile"))?;
        if profile.version != 1
            || profile.provider != "desktopChatGpt"
            || profile.port == 0
            || [&profile.control_token, &profile.permit].iter().any(|value| value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(error("Invalid Desktop profile version, provider, port or private credentials"));
        }
        Ok(Self {
            addr: format!("127.0.0.1:{}", profile.port),
            token: Some(profile.control_token),
            provider_permit: Some(profile.permit),
            external_provider: true,
            provider_profile: Some(path.to_path_buf()),
            idle: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn contents() -> Value {
        json!({"version":1,"provider":"desktopChatGpt","port":9876,"controlToken":"a".repeat(64),"permit":"b".repeat(64)})
    }
    #[test]
    fn explicit_profile_loads_only_loopback_and_has_no_environment_dependency() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&serde_json::to_vec(&contents()).unwrap()).unwrap();
        let bridge = BridgeClient::from_provider_profile(file.path()).unwrap();
        assert_eq!(bridge.addr, "127.0.0.1:9876");
        assert!(bridge.external_provider);
        assert_eq!(bridge.provider_permit.as_deref(), Some("b".repeat(64).as_str()));
    }
    #[test]
    fn profile_parser_mutations_and_hostile_fields_never_panic() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let original = serde_json::to_vec(&contents()).unwrap();
        for cut in 0..original.len() {
            std::fs::write(file.path(), &original[..cut]).unwrap();
            let result = std::panic::catch_unwind(|| BridgeClient::from_provider_profile(file.path()));
            assert!(result.is_ok());
            assert!(result.unwrap().is_err());
        }
        for offset in 0..original.len() {
            let mut bytes = original.clone();
            bytes[offset] ^= 0x80;
            std::fs::write(file.path(), bytes).unwrap();
            assert!(std::panic::catch_unwind(|| BridgeClient::from_provider_profile(file.path())).is_ok());
        }
        for (key, value) in [
            ("port", json!(0)),
            ("port", json!(-1)),
            ("port", json!(u64::MAX)),
            ("port", json!(1.5)),
            ("provider", json!("codex")),
            ("version", json!(2)),
            ("permit", json!("x".repeat(64))),
            ("host", json!("remote.example")),
        ] {
            let mut body = contents();
            body[key] = value;
            std::fs::write(file.path(), serde_json::to_vec(&body).unwrap()).unwrap();
            assert!(BridgeClient::from_provider_profile(file.path()).is_err());
        }
        file.as_file_mut().set_len(MAX_PROFILE.saturating_add(1)).unwrap();
        assert!(BridgeClient::from_provider_profile(file.path()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn public_files_and_symlinks_cannot_supply_private_credentials() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&serde_json::to_vec(&contents()).unwrap()).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let alias = directory.path().join("alias");
        symlink(file.path(), &alias).unwrap();
        assert!(BridgeClient::from_provider_profile(&alias).is_err());
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(BridgeClient::from_provider_profile(file.path()).is_err());
    }
}
