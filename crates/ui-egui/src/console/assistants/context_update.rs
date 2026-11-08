//! Version-guarded semantic memory updates; command history and project identity are host-owned.
use crate::console::assistants::{Checkpoint, Instructions, MAX_CHECKPOINT_BYTES};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    instructions: Instructions,
    decisions: Vec<String>,
    pending: Vec<String>,
    errors: Vec<String>,
}
pub struct Guard<'a> {
    pub scope: &'a str,
    pub document_epoch: u64,
    pub version: u64,
}
pub fn apply(current: &Checkpoint, actual_version: u64, guard: Guard<'_>, bytes: &[u8]) -> Result<Checkpoint, String> {
    if bytes.len() > (256 * 1024).min(MAX_CHECKPOINT_BYTES) {
        return Err("Assistant structured context exceeds 256 KiB; summarize older notes".into());
    }
    if guard.scope != current.project_scope || guard.document_epoch != current.document_epoch || guard.version != actual_version {
        return Err("Assistant context changed; read current structured context before updating".into());
    }
    current.validate(guard.scope)?;
    let update: Update = serde_json::from_slice(bytes).map_err(|e| format!("Cannot read structured assistant update: {e}"))?;
    let mut next = current.clone();
    let mut baseline = current.assistant.clone();
    // The initial host label is a placeholder, not user-taught role instructions.
    if current.assistant_name.is_none()
        && baseline.role == "FilmCraft project assistant"
        && baseline.goal.is_empty()
        && baseline.preferences.is_empty()
        && baseline.workflow.is_empty()
    {
        baseline.role.clear();
    }
    next.assistant = super::preservation::merge(&baseline, &update.instructions, false)?;
    next.decisions = update.decisions;
    next.pending = update.pending;
    next.errors = update.errors;
    next.encode()?;
    Ok(next)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn current() -> Checkpoint {
        Checkpoint {
            schema: 1,
            project_scope: "project-A".into(),
            document_epoch: 7,
            revision: 22,
            generation: 3,
            context_version: 0,
            saved_at: "2026-11-01T12:00:00Z".into(),
            conversation: Some("existing-thread".into()),
            assistant_name: Some("Concert editor".into()),
            autosave: None,
            reference_documents: None,
            assistant: Default::default(),
            decisions: vec![],
            pending: vec![],
            errors: vec![],
            commands: vec![crate::console::assistants::CommandRecord {
                command: "markers.add".into(),
                params: serde_json::json!({"name":"done"}),
                state: crate::console::assistants::CommandState::Executed,
                revision_before: 21,
                revision_after: 22,
                error: None,
            }],
            recent_context: vec![],
        }
    }
    fn update() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"instructions":{"role":"Performance continuity editor","goal":"Preserve the song","preferences":["Keep complete phrases"],"workflow":["Inspect before every edit"]},"decisions":["Keep the wide shot"],"pending":["Review chorus"],"errors":[]})).unwrap()
    }
    #[test]
    fn learned_role_updates_without_replacing_identity_or_executed_command_history() {
        let before = current();
        let next = apply(&before, 9, Guard { scope: "project-A", document_epoch: 7, version: 9 }, &update()).unwrap();
        assert_eq!(next.assistant.role, "Performance continuity editor");
        assert_eq!(next.commands, before.commands);
        assert_eq!(next.conversation, before.conversation);
        assert_eq!(
            (next.project_scope, next.document_epoch, next.revision, next.generation, next.assistant_name),
            (before.project_scope, before.document_epoch, before.revision, before.generation, before.assistant_name)
        );
    }
    #[test]
    fn stale_wrong_scope_or_command_injection_is_rejected_without_mutation() {
        let before = current();
        let original = before.clone();
        for guard in [
            Guard { scope: "project-B", document_epoch: 7, version: 9 },
            Guard { scope: "project-A", document_epoch: 8, version: 9 },
            Guard { scope: "project-A", document_epoch: 7, version: 8 },
        ] {
            assert!(apply(&before, 9, guard, &update()).is_err());
        }
        let mut malicious: serde_json::Value = serde_json::from_slice(&update()).unwrap();
        malicious["commands"] = serde_json::json!([{"command":"edit.clear"}]);
        assert!(apply(&before, 9, Guard { scope: "project-A", document_epoch: 7, version: 9 }, &serde_json::to_vec(&malicious).unwrap()).is_err());
        assert_eq!(before, original);
    }
}
