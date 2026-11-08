//! Persistence protocol; the host supplies bounded reads and atomic writes under its scope lock.
use crate::console::assistants::Checkpoint;
pub const CURRENT: &str = "assistant-checkpoint.json";
pub const PREVIOUS: &str = "assistant-checkpoint.previous.json";

pub fn load(scope: &str, mut read: impl FnMut(&str) -> Result<Option<Vec<u8>>, String>) -> Result<Option<(Checkpoint, bool)>, String> {
    let current = read(CURRENT).and_then(|bytes| bytes.map(|b| Checkpoint::decode(&b, scope)).transpose());
    match current {
        Ok(Some(value)) => Ok(Some((value, false))),
        Ok(None) => read(PREVIOUS)?.map(|b| Checkpoint::decode(&b, scope).map(|v| (v, true))).transpose(),
        Err(current_error) => match read(PREVIOUS).and_then(|bytes| bytes.map(|b| Checkpoint::decode(&b, scope)).transpose()) {
            Ok(Some(value)) => Ok(Some((value, true))),
            Ok(None) => Err(format!("Assistant checkpoint unavailable: {current_error}; no previous checkpoint")),
            Err(previous_error) => Err(format!("Assistant checkpoint unavailable: {current_error}; previous: {previous_error}")),
        },
    }
}

/// Preserve the last valid generation before publishing new context. Caller owns the scope lock.
/// A failed write never returns a new generation or clears the caller's dirty state.
pub fn save(next: &Checkpoint, previous: Option<&Checkpoint>, mut atomic_write: impl FnMut(&str, &[u8]) -> Result<(), String>) -> Result<(), String> {
    let bytes = next.encode()?;
    if let Some(previous) = previous {
        previous.validate(&next.project_scope)?;
        if next.generation != previous.generation.checked_add(1).ok_or("Assistant checkpoint generation exhausted")? {
            return Err("Assistant checkpoint generation changed; reload before saving".into());
        }
        atomic_write(PREVIOUS, &previous.encode()?).map_err(|e| format!("Cannot retain previous assistant checkpoint: {e}"))?;
    } else if next.generation != 1 {
        return Err("First assistant checkpoint must have generation 1".into());
    }
    atomic_write(CURRENT, &bytes).map_err(|e| format!("Cannot save assistant checkpoint: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
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
    fn full_disk_during_publish_retains_current_and_recoverable_previous() {
        let old = checkpoint(1);
        let mut files = HashMap::from([(CURRENT.to_owned(), old.encode().unwrap())]);
        assert!(
            save(&checkpoint(2), Some(&old), |name, bytes| {
                if name == CURRENT {
                    return Err("no space left on device".into());
                }
                files.insert(name.to_owned(), bytes.to_vec());
                Ok(())
            })
            .is_err()
        );
        let (loaded, recovered) = load("A", |n| Ok(files.get(n).cloned())).unwrap().unwrap();
        assert_eq!(loaded, old);
        assert!(!recovered);
        files.insert(CURRENT.into(), b"corrupt".to_vec());
        let (loaded, recovered) = load("A", |n| Ok(files.get(n).cloned())).unwrap().unwrap();
        assert_eq!(loaded, old);
        assert!(recovered);
    }
    #[test]
    fn wrong_project_or_stale_generation_never_writes() {
        let mut calls = 0;
        assert!(
            save(&checkpoint(1), Some(&checkpoint(1)), |_, _| {
                calls += 1;
                Ok(())
            })
            .is_err()
        );
        let mut wrong = checkpoint(1);
        wrong.project_scope = "B".into();
        assert!(
            save(&checkpoint(2), Some(&wrong), |_, _| {
                calls += 1;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(calls, 0);
        assert!(load("B", |_| Ok(Some(checkpoint(1).encode().unwrap()))).is_err());
    }
    #[test]
    fn valid_second_generation_retains_first_and_loads_second() {
        let mut files = HashMap::new();
        save(&checkpoint(1), None, |n, b| {
            files.insert(n.to_owned(), b.to_vec());
            Ok(())
        })
        .unwrap();
        save(&checkpoint(2), Some(&checkpoint(1)), |n, b| {
            files.insert(n.to_owned(), b.to_vec());
            Ok(())
        })
        .unwrap();
        assert_eq!(Checkpoint::decode(files.get(PREVIOUS).unwrap(), "A").unwrap().generation, 1);
        assert_eq!(load("A", |n| Ok(files.get(n).cloned())).unwrap().unwrap().0.generation, 2);
    }
}
