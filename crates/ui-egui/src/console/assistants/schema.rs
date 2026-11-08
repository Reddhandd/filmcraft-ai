//! Structured assistant memory. Data here never dispatches editing commands.
use serde::{Deserialize, Serialize};

pub const MAX_CHECKPOINT_BYTES: usize = 1024 * 1024;
pub const CHECKPOINT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instructions {
    pub role: String,
    pub goal: String,
    pub preferences: Vec<String>,
    pub workflow: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposed: Vec<super::preservation::Teaching>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approved: Vec<super::preservation::Teaching>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forgotten: Vec<super::preservation::Teaching>,
}
impl Instructions {
    pub fn validate(&self) -> Result<(), String> {
        bounded(&self.role, 4096)?;
        bounded(&self.goal, 8192)?;
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > 256 * 1024 {
            return Err("Assistant instructions exceed 256 KiB; summarize the workflow".into());
        }
        for list in [&self.preferences, &self.workflow] {
            if list.len() > 64 {
                return Err("Assistant instructions exceed 64 entries".into());
            }
            for text in list {
                bounded(text, 4096)?;
            }
        }
        super::preservation::validate_metadata(self)?;
        Ok(())
    }
}

/// None follows shared availability; Some(empty) explicitly permits no book delivery.
pub fn validate_reference_documents(selected: &Option<std::collections::BTreeSet<u64>>) -> Result<(), String> {
    if selected.as_ref().is_some_and(|ids| ids.len() > 512 || ids.contains(&0)) {
        return Err("Reference selection requires at most 512 positive document identities".into());
    }
    Ok(())
}

/// Host-owned opt-in and compare-and-save baseline for one selected named assistant.
/// Project history is deliberately absent from the reusable catalog update.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AssistantAutosave {
    pub expected: Instructions,
    pub saved_at: String,
}

/// Portable instructions deliberately contain no source project, thread or command ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assistant {
    pub schema: u32,
    pub name: String,
    pub instructions: Instructions,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "referenceDocuments")]
    pub reference_documents: Option<std::collections::BTreeSet<u64>>,
}
impl Assistant {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 || self.name.trim().is_empty() || self.name.len() > 80 || self.name.chars().any(char::is_control) {
            return Err("Assistant requires schema 1 and a name of 1–80 UTF-8 bytes".into());
        }
        validate_reference_documents(&self.reference_documents)?;
        self.instructions.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandState {
    Executed,
    Failed,
    Proposed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CommandRecord {
    pub command: String,
    pub params: serde_json::Value,
    pub state: CommandState,
    pub revision_before: u64,
    pub revision_after: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Checkpoint {
    pub schema: u32,
    pub project_scope: String,
    pub document_epoch: u64,
    pub revision: u64,
    pub generation: u64,
    #[serde(default)]
    pub context_version: u64,
    pub saved_at: String,
    pub conversation: Option<String>,
    #[serde(default)]
    pub assistant_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autosave: Option<AssistantAutosave>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_documents: Option<std::collections::BTreeSet<u64>>,
    pub assistant: Instructions,
    pub decisions: Vec<String>,
    pub pending: Vec<String>,
    pub errors: Vec<String>,
    pub commands: Vec<CommandRecord>,
    pub recent_context: Vec<(String, String)>,
}
impl Checkpoint {
    pub fn validate(&self, scope: &str) -> Result<(), String> {
        if self.schema != 1 || self.project_scope != scope || scope.is_empty() || scope.len() > 4096 {
            return Err("Assistant checkpoint belongs to another project or unsupported schema".into());
        }
        bounded(&self.saved_at, 64)?;
        if let Some(id) = &self.conversation
            && (id.is_empty() || id.len() > 256 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
        {
            return Err("Assistant checkpoint conversation ID is invalid".into());
        }
        if let Some(name) = &self.assistant_name
            && (name.trim().is_empty() || name.len() > 80 || name.chars().any(char::is_control))
        {
            return Err("Checkpoint assistant name is invalid".into());
        }
        self.assistant.validate()?;
        validate_reference_documents(&self.reference_documents)?;
        if let Some(autosave) = &self.autosave {
            if self.assistant_name.is_none() {
                return Err("Assistant autosave requires a selected named assistant".into());
            }
            autosave.expected.validate()?;
            bounded(&autosave.saved_at, 64)?;
            if !autosave.saved_at.is_empty() {
                chrono::DateTime::parse_from_rfc3339(&autosave.saved_at).map_err(|_| "Assistant autosave timestamp is invalid")?;
            }
        }
        for list in [&self.decisions, &self.pending, &self.errors] {
            if list.len() > 128 {
                return Err("Assistant checkpoint exceeds 128 notes per section".into());
            }
            for text in list {
                bounded(text, 4096)?;
            }
        }
        if self.commands.len() > 128 || self.recent_context.len() > 32 {
            return Err("Assistant checkpoint history exceeds its entry limit".into());
        }
        for record in &self.commands {
            if record.command.is_empty() || record.command.len() > 256 || record.command.chars().any(char::is_control) {
                return Err("Assistant checkpoint command ID is invalid".into());
            }
            if !record.params.is_object() || serde_json::to_vec(&record.params).map_err(|e| e.to_string())?.len() > 8192 {
                return Err("Assistant checkpoint command parameters exceed their budget".into());
            }
            if let Some(error) = &record.error {
                bounded(error, 4096)?;
            }
        }
        for (role, text) in &self.recent_context {
            bounded(role, 64)?;
            bounded(text, 16 * 1024)?;
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate(&self.project_scope)?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err("Assistant checkpoint exceeds 1 MiB".into());
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8], scope: &str) -> Result<Self, String> {
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err("Assistant checkpoint exceeds 1 MiB".into());
        }
        let checkpoint: Self = serde_json::from_slice(bytes).map_err(|e| format!("Cannot read assistant checkpoint: {e}"))?;
        checkpoint.validate(scope)?;
        Ok(checkpoint)
    }
}
fn bounded(text: &str, limit: usize) -> Result<(), String> {
    if text.len() > limit || text.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err(format!("Assistant text must fit {limit} UTF-8 bytes and contain no control characters"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn checkpoint() -> Checkpoint {
        Checkpoint {
            schema: 1,
            project_scope: "project-A".into(),
            document_epoch: 7,
            revision: 12,
            generation: 2,
            context_version: 0,
            saved_at: "2026-11-01T12:00:00Z".into(),
            conversation: Some("thread-a".into()),
            assistant_name: Some("Concert editor".into()),
            autosave: None,
            reference_documents: None,
            assistant: Instructions::default(),
            decisions: vec!["Keep full phrases".into()],
            pending: vec![],
            errors: vec![],
            commands: vec![CommandRecord {
                command: "markers.add".into(),
                params: serde_json::json!({"name":"already done"}),
                state: CommandState::Executed,
                revision_before: 11,
                revision_after: 12,
                error: None,
            }],
            recent_context: vec![("You".into(), "次のカットを確認".into())],
        }
    }
    #[test]
    fn checkpoint_round_trip_preserves_history_but_rejects_another_project() {
        let original = checkpoint();
        let bytes = original.encode().unwrap();
        assert_eq!(Checkpoint::decode(&bytes, "project-A").unwrap(), original);
        assert!(Checkpoint::decode(&bytes, "project-B").is_err());
        assert_eq!(original.commands[0].state, CommandState::Executed);
    }
    #[test]
    fn portable_assistant_rejects_project_and_command_fields() {
        let good = Assistant { schema: 1, name: "Performance editor".into(), instructions: Instructions::default(), reference_documents: None };
        good.validate().unwrap();
        let mut value = serde_json::to_value(good).unwrap();
        value["commands"] = serde_json::json!([{"command":"edit.clear"}]);
        assert!(serde_json::from_value::<Assistant>(value).is_err());
    }
    #[test]
    fn malformed_checkpoint_mutations_never_panic_or_validate_wrong_scope() {
        let bytes = checkpoint().encode().unwrap();
        for length in 0..bytes.len() {
            let result = std::panic::catch_unwind(|| Checkpoint::decode(&bytes[..length], "project-B"));
            assert!(result.is_ok());
            assert!(result.unwrap().is_err());
        }
        for i in 0..bytes.len() {
            let mut mutated = bytes.clone();
            mutated[i] ^= 0xff;
            assert!(std::panic::catch_unwind(|| Checkpoint::decode(&mutated, "project-A")).is_ok());
        }
        assert!(Checkpoint::decode(&vec![b' '; MAX_CHECKPOINT_BYTES + 1], "project-A").is_err());
    }
    #[test]
    fn invalid_history_and_names_are_rejected_before_save() {
        let mut value = checkpoint();
        value.commands[0].params = serde_json::json!([1, 2, 3]);
        assert!(value.encode().is_err());
        let mut value = checkpoint();
        value.commands[0].params = serde_json::json!({"large":"x".repeat(8193)});
        assert!(value.encode().is_err());
        let mut assistant = Assistant { schema: 1, name: " ".into(), instructions: Instructions::default(), reference_documents: None };
        assert!(assistant.validate().is_err());
        assistant.name = "bad\u{0000}name".into();
        assert!(assistant.validate().is_err());
        assistant.name = "編集".into();
        assert!(assistant.validate().is_ok());
    }
    #[test]
    fn old_checkpoints_default_autosave_off_and_opt_in_round_trips() {
        let old = checkpoint();
        let encoded = old.encode().unwrap();
        assert!(!serde_json::from_slice::<serde_json::Value>(&encoded).unwrap().as_object().unwrap().contains_key("autosave"));
        assert!(Checkpoint::decode(&encoded, &old.project_scope).unwrap().autosave.is_none());
        let mut enabled = old.clone();
        enabled.autosave = Some(AssistantAutosave { expected: old.assistant.clone(), saved_at: "2026-11-05T14:00:00Z".into() });
        assert_eq!(Checkpoint::decode(&enabled.encode().unwrap(), &old.project_scope).unwrap(), enabled);
        enabled.assistant_name = None;
        assert!(enabled.encode().is_err());
    }
    #[test]
    fn autosave_rejects_invalid_timestamp_and_unbounded_instructions() {
        let mut value = checkpoint();
        value.autosave = Some(AssistantAutosave { expected: Instructions::default(), saved_at: "yesterday".into() });
        assert!(value.encode().is_err());
        let settings = value.autosave.as_mut().unwrap();
        settings.saved_at.clear();
        settings.expected.role = "x".repeat(4097);
        assert!(value.encode().is_err());
    }
    #[test]
    fn reference_selection_migrates_off_and_distinguishes_shared_from_explicit_empty() {
        let mut value = checkpoint();
        assert!(value.reference_documents.is_none());
        let old = value.encode().unwrap();
        assert!(Checkpoint::decode(&old, &value.project_scope).unwrap().reference_documents.is_none());
        value.reference_documents = Some(std::collections::BTreeSet::new());
        assert_eq!(Checkpoint::decode(&value.encode().unwrap(), &value.project_scope).unwrap().reference_documents, value.reference_documents);
        value.reference_documents = Some(std::collections::BTreeSet::from([0]));
        assert!(value.encode().is_err());
        value.reference_documents = Some((1..=512).collect());
        assert!(value.encode().is_ok());
        value.reference_documents = Some((1..=513).collect());
        assert!(value.encode().is_err());
    }
}
