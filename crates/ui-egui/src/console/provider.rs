//! Exclusive, revocable ownership for AI transports. No credentials are serialized.
use serde::{Deserialize, Serialize};
/// Owner-deferred: preserve the implementation, but do not expose an API transport in builds.
pub const API_CONNECTIONS_ENABLED: bool = false;
pub const API_UNRESOLVED: &str = "OpenAI API connections are disabled: unresolved multi-frame capture workflow. Use Codex or Desktop ChatGPT.";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderId {
    #[default]
    Codex,
    DesktopChatGpt,
    OpenAiApi,
}
impl ProviderId {
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::DesktopChatGpt => "Desktop ChatGPT",
            Self::OpenAiApi => "OpenAI API",
        }
    }
}
#[derive(Clone)]
pub struct Permit {
    provider: ProviderId,
    generation: u64,
    nonce: [u8; 32],
}
impl std::fmt::Debug for Permit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Permit").field("provider", &self.provider).field("generation", &self.generation).field("nonce", &"[redacted]").finish()
    }
}
impl Permit {
    /// Transport credential only; never serialize into projects, diagnostics or chat.
    pub fn transport_token(&self) -> String {
        self.nonce.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
pub struct Authorized<T> {
    permit: Permit,
    payload: T,
}
impl<T> Authorized<T> {
    pub fn new(permit: Permit, payload: T) -> Self {
        Self { permit, payload }
    }
    pub fn execute<R>(self, owner: &Ownership, operation: impl FnOnce(T) -> R) -> Result<R, String> {
        owner.check_tool(&self.permit)?;
        Ok(operation(self.payload))
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkState {
    #[default]
    Disconnected,
    Connecting,
    AwaitingExternal,
    Ready,
}
#[derive(Default)]
pub struct Ownership {
    selected: ProviderId,
    generation: u64,
    active: Option<Permit>,
    state: LinkState,
}
// The native bridge has no web transport. Never substitute predictable permits on wasm.
fn native_nonce() -> Result<[u8; 32], String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut nonce = [0; 32];
        getrandom::fill(&mut nonce).map_err(|_| "Cannot create a private AI connection permit; no provider was connected")?;
        Ok(nonce)
    }
    #[cfg(target_arch = "wasm32")]
    {
        Err("Native AI provider connections require the desktop application".into())
    }
}
impl Ownership {
    pub fn selected(&self) -> ProviderId {
        self.selected
    }
    pub fn state(&self) -> LinkState {
        self.state
    }
    /// Revocation happens before a replacement can connect; failed auth never restores old access.
    pub fn select(&mut self, provider: ProviderId) -> Result<bool, String> {
        if self.selected == provider {
            return Ok(false);
        }
        self.revoke()?;
        self.selected = provider;
        self.state = if provider == ProviderId::DesktopChatGpt { LinkState::AwaitingExternal } else { LinkState::Disconnected };
        Ok(true)
    }
    pub fn revoke(&mut self) -> Result<(), String> {
        self.active = None;
        self.state = LinkState::Disconnected;
        self.generation = self.generation.checked_add(1).ok_or("AI connection generation exhausted; restart FilmCraft after saving")?;
        Ok(())
    }
    pub fn begin(&mut self, provider: ProviderId) -> Result<Permit, String> {
        if provider != self.selected {
            return Err("This provider is not selected".into());
        }
        self.revoke()?;
        let nonce = native_nonce()?;
        let permit = Permit { provider, generation: self.generation, nonce };
        self.active = Some(permit.clone());
        self.state = LinkState::Connecting;
        Ok(permit)
    }
    pub fn accepts(&self, permit: &Permit) -> bool {
        self.active.as_ref().is_some_and(|active| {
            let different = active.nonce.iter().zip(permit.nonce.iter()).fold(0u8, |difference, (a, b)| difference | (a ^ b));
            active.provider == permit.provider && active.generation == permit.generation && permit.provider == self.selected && different == 0
        })
    }
    pub fn authorizes_token(&self, token: &str) -> bool {
        self.state == LinkState::Ready && self.matches_token(token)
    }
    pub fn matches_token(&self, token: &str) -> bool {
        let Some(active) = self.active.as_ref() else {
            return false;
        };
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
        let mut different = 0u8;
        for (part, expected) in token.as_bytes().as_chunks::<2>().0.iter().zip(active.nonce.iter()) {
            let Ok(text) = std::str::from_utf8(part) else {
                return false;
            };
            let Ok(value) = u8::from_str_radix(text, 16) else {
                return false;
            };
            different |= value ^ expected;
        }
        different == 0
    }
    pub fn ready(&mut self, permit: &Permit) -> Result<(), String> {
        if !self.accepts(permit) {
            return Err("Stale AI connection; its replies and tools are revoked".into());
        }
        self.state = LinkState::Ready;
        Ok(())
    }
    /// Check again immediately before dispatch, not just when a worker queues a request.
    pub fn check_tool(&self, permit: &Permit) -> Result<(), String> {
        if self.state != LinkState::Ready || !self.accepts(permit) {
            return Err("AI provider no longer owns this connection; request was not executed".into());
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switching_revokes_old_replies_and_tools() {
        let mut owner = Ownership::default();
        let old = owner.begin(ProviderId::Codex).unwrap();
        owner.ready(&old).unwrap();
        owner.select(ProviderId::OpenAiApi).unwrap();
        assert!(!owner.accepts(&old));
        assert!(owner.check_tool(&old).is_err());
        let next = owner.begin(ProviderId::OpenAiApi).unwrap();
        assert!(owner.ready(&old).is_err());
        assert_eq!(owner.state(), LinkState::Connecting);
        owner.ready(&next).unwrap();
        assert!(owner.check_tool(&next).is_ok());
        assert!(owner.check_tool(&old).is_err());
    }
    #[test]
    fn reconnect_and_disconnect_cannot_resurrect_old_ownership() {
        let mut owner = Ownership::default();
        let old = owner.begin(ProviderId::Codex).unwrap();
        owner.ready(&old).unwrap();
        let next = owner.begin(ProviderId::Codex).unwrap();
        assert!(!owner.accepts(&old));
        owner.ready(&next).unwrap();
        owner.revoke().unwrap();
        assert!(!owner.accepts(&next));
        assert!(owner.ready(&old).is_err());
        assert_eq!(owner.state(), LinkState::Disconnected);
    }
    #[test]
    fn desktop_selection_does_not_claim_connection() {
        let mut owner = Ownership::default();
        owner.select(ProviderId::DesktopChatGpt).unwrap();
        assert_eq!(owner.state(), LinkState::AwaitingExternal);
        assert!(owner.begin(ProviderId::Codex).is_err());
    }
    #[test]
    fn default_and_redaction_preserve_contract() {
        let mut owner = Ownership::default();
        assert_eq!(owner.selected(), ProviderId::Codex);
        let permit = owner.begin(ProviderId::Codex).unwrap();
        assert!(format!("{permit:?}").contains("[redacted]"));
        assert!(owner.check_tool(&permit).is_err());
    }
    #[test]
    fn queued_mutation_is_rejected_after_switch_before_execution() {
        let mut owner = Ownership::default();
        let permit = owner.begin(ProviderId::Codex).unwrap();
        owner.ready(&permit).unwrap();
        let queued = Authorized::new(permit, 42);
        owner.select(ProviderId::OpenAiApi).unwrap();
        let next = owner.begin(ProviderId::OpenAiApi).unwrap();
        owner.ready(&next).unwrap();
        let mut edits = Vec::new();
        assert!(queued.execute(&owner, |value| edits.push(value)).is_err());
        assert!(edits.is_empty());
    }
    #[test]
    fn tokens_are_revoked_and_hostile_tokens_never_authorize() {
        let mut owner = Ownership::default();
        let permit = owner.begin(ProviderId::Codex).unwrap();
        let token = permit.transport_token();
        assert!(!owner.authorizes_token(&token));
        owner.ready(&permit).unwrap();
        assert!(owner.authorizes_token(&token));
        for bad in ["".into(), "0".repeat(63), "z".repeat(64), "💥".repeat(64), "0".repeat(10000)] {
            assert!(!owner.authorizes_token(&bad));
        }
        owner.revoke().unwrap();
        assert!(!owner.authorizes_token(&token));
    }
}
