//! Bounded named portable assistant catalog; project checkpoints stay elsewhere.
use crate::console::assistants::{Assistant, Instructions};
use serde::{Deserialize, Serialize};
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_ASSISTANTS: usize = 128;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Library {
    pub schema: u32,
    pub assistants: Vec<Assistant>,
}
impl Default for Library {
    fn default() -> Self {
        Self { schema: 1, assistants: Vec::new() }
    }
}
impl Library {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("Project assistant library exceeds 4 MiB".into());
        }
        let library: Self = serde_json::from_slice(bytes).map_err(|e| format!("Cannot read project assistant library: {e}"))?;
        library.validate()?;
        Ok(library)
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_BYTES {
            return Err("Project assistant library exceeds 4 MiB".into());
        }
        Ok(bytes)
    }
    fn validate(&self) -> Result<(), String> {
        if self.schema != 1 || self.assistants.len() > MAX_ASSISTANTS {
            return Err("Unsupported or oversized project assistant library".into());
        }
        let mut seen = std::collections::HashSet::new();
        for assistant in &self.assistants {
            assistant.validate()?;
            if assistant.name != assistant.name.trim() || !seen.insert(&assistant.name) {
                return Err("Project assistant names must be unique and trimmed".into());
            }
        }
        Ok(())
    }
    /// Caller atomically persists the candidate before replacing the live library.
    pub fn with_saved(&self, name: &str, instructions: Instructions, replace: bool) -> Result<Self, String> {
        self.validate()?;
        let assistant = Assistant { schema: 1, name: name.trim().to_owned(), instructions, reference_documents: None };
        assistant.validate()?;
        let mut next = self.clone();
        if let Some(existing) = next.assistants.iter_mut().find(|entry| entry.name == assistant.name) {
            if !replace {
                return Err("An assistant with that name exists; confirm Update or choose a different name".into());
            }
            existing.instructions = super::preservation::merge(&existing.instructions, &assistant.instructions, true)?;
        } else {
            if next.assistants.len() >= MAX_ASSISTANTS {
                return Err("Project assistant library is full (128 assistants)".into());
            }
            next.assistants.push(assistant);
        }
        next.encode()?;
        Ok(next)
    }
    pub fn reference_selection(&self, name: &str) -> Result<Option<std::collections::BTreeSet<u64>>, String> {
        self.validate()?;
        self.assistants
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.reference_documents.clone())
            .ok_or_else(|| "Choose a saved project assistant".into())
    }
    pub fn with_reference_selection(&self, name: &str, selected: Option<std::collections::BTreeSet<u64>>) -> Result<Self, String> {
        self.validate()?;
        super::validate_reference_documents(&selected)?;
        let mut next = self.clone();
        let entry = next.assistants.iter_mut().find(|entry| entry.name == name).ok_or("Choose a saved project assistant")?;
        entry.reference_documents = selected;
        next.encode()?;
        Ok(next)
    }
    pub fn instructions(&self, name: &str) -> Result<Instructions, String> {
        self.validate()?;
        self.assistants.iter().find(|a| a.name == name).map(|a| a.instructions.clone()).ok_or_else(|| "Choose a saved project assistant".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_required_and_existing_assistant_requires_explicit_update() {
        let original = Library::default();
        assert!(original.with_saved(" ", Instructions::default(), false).is_err());
        let first = original.with_saved(" Concert editor ", Instructions { role: "Keep performance continuity".into(), ..Default::default() }, false).unwrap();
        assert!(original.assistants.is_empty());
        assert_eq!(first.assistants[0].name, "Concert editor");
        assert!(first.with_saved("Concert editor", Instructions::default(), false).is_err());
        let updated = first.with_saved("Concert editor", Instructions { role: "Follow vocalist".into(), ..Default::default() }, true).unwrap();
        assert_eq!(first.instructions("Concert editor").unwrap().role, "Keep performance continuity");
        let instructions = updated.instructions("Concert editor").unwrap();
        assert_eq!(instructions.role, "Keep performance continuity");
        assert_eq!(instructions.proposed.len(), 1);
        assert_eq!(instructions.proposed[0].text, "Follow vocalist");
        let reviewed = super::super::preservation::review(&instructions, &instructions.proposed[0], "accept").unwrap();
        let reviewed = updated.with_saved("Concert editor", reviewed, true).unwrap();
        assert_eq!(reviewed.instructions("Concert editor").unwrap().role, "Follow vocalist");
        assert_eq!(updated.assistants.len(), first.assistants.len());
        assert_eq!(updated.assistants[0].name, first.assistants[0].name);
    }
    #[test]
    fn portable_selection_cannot_transfer_project_history_and_catalog_round_trips() {
        let library = Library::default().with_saved("Editor", Instructions { goal: "Retain complete phrases".into(), ..Default::default() }, false).unwrap();
        assert_eq!(Library::decode(&library.encode().unwrap()).unwrap(), library);
        let selected = serde_json::to_value(library.instructions("Editor").unwrap()).unwrap();
        assert!(selected.get("commands").is_none());
        assert!(selected.get("conversation").is_none());
        assert!(selected.get("projectScope").is_none());
        assert!(library.instructions("missing").is_err());
        let mut bad = library.clone();
        bad.assistants.push(bad.assistants[0].clone());
        assert!(bad.encode().is_err());
    }
}
