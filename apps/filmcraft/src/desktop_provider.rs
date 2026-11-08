//! External MCP attachment only. Never pretends to embed the ChatGPT composer or voice.
mod profile;
mod setup;
use filmcraft_ui_egui::console::{ChatEvent, CodexConnection, ConnectionStatus, provider::ProviderId};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{Duration, Instant},
};
const LEASE: Duration = Duration::from_secs(10);
pub struct Connection {
    port: Option<u16>,
    control_token: Option<String>,
    storage: Option<PathBuf>,
    profile: Option<profile::Profile>,
    permit: Option<String>,
    setup: Option<PathBuf>,
    deadline: Option<Instant>,
    attached: bool,
    statuses: VecDeque<ConnectionStatus>,
    events: VecDeque<ChatEvent>,
}
impl Connection {
    /// Inject the desktop-owned private credential without changing the process environment.
    pub fn with_control_token(mut self, token: Option<String>) -> Self {
        self.control_token = token;
        self
    }
    pub fn new(port: Option<u16>) -> Self {
        Self {
            port,
            control_token: None,
            storage: None,
            profile: None,
            permit: None,
            setup: None,
            deadline: None,
            attached: false,
            statuses: VecDeque::new(),
            events: VecDeque::new(),
        }
    }
    pub fn with_storage(mut self, storage: Option<PathBuf>) -> Self {
        self.storage = storage;
        self
    }
    fn live(&self) -> bool {
        self.attached && self.deadline.is_some_and(|deadline| Instant::now() < deadline)
    }
    fn remove_setup(&mut self) {
        self.profile = None;
        self.setup = None;
    }
    fn prepare(&mut self, token: &str, cli: PathBuf) -> Result<(), String> {
        if self.setup.is_some() || self.attached {
            return Err("Disconnect the external provider before creating another attachment".into());
        }
        let port = self.port.filter(|port| *port != 0).ok_or("External ChatGPT needs the private local control bridge")?;
        let permit = self.permit.as_deref().ok_or("External provider ownership is missing")?;
        if !cli.is_file() {
            return Err("Build the matching FilmCraft CLI beside this development executable before connecting".into());
        }
        let storage = self.storage.as_deref().ok_or("Desktop connection requires the application's private data directory")?;
        let profile = profile::Profile::publish(storage, &cli, port, token, permit)?;
        self.setup = Some(profile.setup_path().to_path_buf());
        self.profile = Some(profile);
        self.statuses.push_back(ConnectionStatus::Connecting);
        self.events.push_back(ChatEvent::Finished);
        Ok(())
    }
}
impl CodexConnection for Connection {
    fn provider_id(&self) -> ProviderId {
        ProviderId::DesktopChatGpt
    }
    fn provider_capabilities(&self) -> Value {
        json!({"embeddedComposer":false,"nativeTools":self.live(),"voice":false,"externalVoice":"unqualified; use external client controls","images":"external-client-dependent","resume":false,"qualified":false,"attachment":"external MCP client; client identity is not attested","setupFile":self.setup.as_ref().map(|path|path.to_string_lossy())})
    }
    fn bind_provider_permit(&mut self, permit: String) -> Result<(), String> {
        if permit.len() != 64 || !permit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("Invalid external provider ownership".into());
        }
        if self.setup.is_some() || self.attached {
            return Err("Disconnect before replacing external provider ownership".into());
        }
        self.permit = Some(permit);
        Ok(())
    }
    fn connect(&mut self) -> Result<(), String> {
        let cli = std::env::current_exe().map_err(|_| "Cannot locate matching external MCP helper")?.with_file_name(if cfg!(windows) {
            "filmcraft-cli.exe"
        } else {
            "filmcraft-cli"
        });
        let token = self.control_token.clone().or_else(|| std::env::var("FILMCRAFT_CONTROL_TOKEN").ok()).ok_or("Private FilmCraft control token is missing")?;
        self.prepare(&token, cli)
    }
    fn external_provider_event(&mut self, permit: &str, event: &str) -> Result<(), String> {
        // Manager performs constant-work permit authentication before this adapter is called.
        if self.permit.as_deref() != Some(permit) || self.setup.is_none() {
            return Err("External attachment was revoked".into());
        }
        match event {
            "authenticate" if !self.attached || self.live() => Ok(()),
            "attach" if !self.attached => {
                self.attached = true;
                self.deadline = Instant::now().checked_add(LEASE);
                if self.deadline.is_none() {
                    self.disconnect();
                    return Err("External attachment clock exhausted".into());
                }
                self.statuses.push_back(ConnectionStatus::Ready);
                Ok(())
            }
            "heartbeat" if self.live() => {
                self.deadline = Instant::now().checked_add(LEASE);
                if self.deadline.is_none() {
                    self.disconnect();
                    return Err("External attachment clock exhausted".into());
                }
                Ok(())
            }
            "detach" => {
                self.disconnect();
                Ok(())
            }
            _ => Err("External attachment expired or already initialized; disconnect and reconnect with a new permit".into()),
        }
    }
    fn authorizes_provider(&self, permit: &str) -> bool {
        self.live() && self.permit.as_deref() == Some(permit)
    }
    fn poll(&mut self) -> Option<ConnectionStatus> {
        if self.attached && !self.live() {
            self.disconnect();
            return Some(ConnectionStatus::Failed("External MCP heartbeat expired; ownership revoked. Reconnect before using tools".into()));
        }
        self.statuses.pop_front()
    }
    fn poll_owned_status(&mut self) -> Option<(String, ConnectionStatus)> {
        let token = self.permit.clone()?;
        self.poll().map(|status| (token, status))
    }
    fn poll_owned_chat(&mut self) -> Option<(String, ChatEvent)> {
        Some((self.permit.clone()?, self.events.pop_front()?))
    }
    fn disconnect(&mut self) {
        self.attached = false;
        self.deadline = None;
        self.permit = None;
        self.statuses.clear();
        self.events.clear();
        self.remove_setup();
    }
    fn send_prompt(&mut self, _prompt: String) -> Result<(), String> {
        Err("Type or speak in your external ChatGPT conversation; this provider has no embedded composer".into())
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.disconnect();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn prepared() -> (tempfile::TempDir, Connection) {
        let root = tempfile::tempdir().unwrap();
        let mut connection = Connection::new(Some(9876)).with_storage(Some(root.path().join("Desktop")));
        let permit = format!("{:064x}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        connection.bind_provider_permit(permit).unwrap();
        connection.prepare(&"d".repeat(64), std::env::current_exe().unwrap()).unwrap();
        (root, connection)
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    #[test]
    fn external_attachment_needs_init_and_live_heartbeat() {
        let (_root, mut connection) = prepared();
        let token = connection.permit.clone().unwrap();
        assert!(!connection.authorizes_provider(&token));
        connection.external_provider_event(&token, "authenticate").unwrap();
        assert!(!connection.authorizes_provider(&token));
        assert_eq!(connection.poll(), Some(ConnectionStatus::Connecting));
        connection.external_provider_event(&token, "attach").unwrap();
        assert!(connection.authorizes_provider(&token));
        let deadline = connection.deadline;
        connection.external_provider_event(&token, "authenticate").unwrap();
        assert_eq!(connection.deadline, deadline, "Inventory authentication must not renew the active lease");
        assert!(connection.external_provider_event(&token, "attach").is_err());
        connection.external_provider_event(&token, "heartbeat").unwrap();
        connection.deadline = Some(Instant::now());
        assert!(!connection.authorizes_provider(&token));
        assert!(connection.external_provider_event(&token, "authenticate").is_err());
        assert!(connection.external_provider_event(&token, "heartbeat").is_err());
        assert!(matches!(connection.poll(), Some(ConnectionStatus::Failed(_))));
        assert!(connection.setup.is_none());
    }
    #[test]
    fn private_config_is_cleaned_and_contains_only_explicit_setup() {
        let (_root, mut connection) = prepared();
        let path = connection.setup.clone().unwrap();
        let config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(config["mcpServers"]["filmcraft"]["args"][2], "--provider-profile");
        let descriptor = connection.profile.as_ref().unwrap().descriptor_path().to_path_buf();
        let live: Value = serde_json::from_slice(&std::fs::read(&descriptor).unwrap()).unwrap();
        assert_eq!(live["provider"], "desktopChatGpt");
        assert!(!serde_json::to_string(&config).unwrap().contains(connection.permit.as_deref().unwrap()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let public = connection.provider_capabilities().to_string();
        assert!(!public.contains(&"d".repeat(64)));
        assert!(!public.contains(connection.permit.as_deref().unwrap()));
        connection.disconnect();
        assert!(!descriptor.exists());
        assert!(path.exists(), "Stable setup configuration is retained across reconnects");
    }
    #[test]
    fn explicit_reconnect_rejects_old_helper_without_detaching_new_owner() {
        let (_root, mut connection) = prepared();
        let original_setup = connection.setup.clone();
        let old = connection.permit.clone().unwrap();
        connection.external_provider_event(&old, "attach").unwrap();
        connection.disconnect();
        let fresh = format!("{:064x}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        connection.bind_provider_permit(fresh.clone()).unwrap();
        connection.prepare(&"d".repeat(64), std::env::current_exe().unwrap()).unwrap();
        assert_eq!(connection.setup, original_setup, "Saved client setup remains stable");
        connection.external_provider_event(&fresh, "authenticate").unwrap();
        connection.external_provider_event(&fresh, "attach").unwrap();
        for event in ["authenticate", "attach", "heartbeat", "detach"] {
            assert!(connection.external_provider_event(&old, event).is_err());
            assert!(connection.authorizes_provider(&fresh));
            assert!(!connection.authorizes_provider(&old));
        }
    }
}
