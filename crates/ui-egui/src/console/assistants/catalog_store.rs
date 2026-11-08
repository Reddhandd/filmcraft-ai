//! Compare-and-save reusable instructions, retaining the last valid catalog first.
use super::{Instructions, atomic, disk, library::Library};
use std::path::Path;

pub(super) const CURRENT: &str = "assistants.json";
pub(super) const PREVIOUS: &str = "assistants.previous.json";
const LIMIT: usize = 4 * 1024 * 1024;

pub(super) fn read(dir: &Path) -> Result<Library, String> {
    disk::read(&dir.join(CURRENT), LIMIT)?.map(|bytes| Library::decode(&bytes)).transpose().map(Option::unwrap_or_default)
}

/// `expected` is the caller's last observed version, never a freshly read substitute.
/// A failed backup or stale comparison must leave the current catalog untouched.
pub(super) fn save(dir: &Path, name: &str, instructions: Instructions, expected: Option<&Instructions>) -> Result<Library, String> {
    save_with_selection(dir, name, instructions, expected, None)
}
pub(super) fn restore(dir: &Path, name: &str, revision: u64, expected: &Instructions) -> Result<Library, String> {
    let _lock = disk::ScopeLock::take(dir)?;
    let current = read(dir)?;
    if current.instructions(name)? != *expected {
        return Err("Saved assistant changed; refresh before restoring a revision".into());
    }
    let previous = super::catalog_history::load(dir, revision)?;
    let restored = previous.assistants.iter().find(|entry| entry.name == name).ok_or("That revision does not contain this assistant")?.clone();
    let mut next = current.clone();
    let entry = next.assistants.iter_mut().find(|entry| entry.name == name).ok_or("Choose an existing assistant to restore")?;
    *entry = restored;
    let bytes = next.encode()?;
    if next == current {
        return Ok(current);
    }
    atomic::atomic_write(&dir.join(PREVIOUS), &current.encode()?).map_err(|error| format!("Cannot retain catalog before restore: {error}"))?;
    super::catalog_history::retain(dir, &current)?;
    atomic::atomic_write(&dir.join(CURRENT), &bytes).map_err(|error| format!("Cannot restore assistant revision: {error}"))?;
    Ok(next)
}
pub(super) struct SelectionUpdate {
    pub expected: Option<std::collections::BTreeSet<u64>>,
    pub selected: Option<std::collections::BTreeSet<u64>>,
}
pub(super) fn save_with_selection(
    dir: &Path,
    name: &str,
    instructions: Instructions,
    expected: Option<&Instructions>,
    selection: Option<SelectionUpdate>,
) -> Result<Library, String> {
    let _lock = disk::ScopeLock::take(dir)?;
    let current = read(dir)?;
    if let Some(expected) = expected {
        expected.validate()?;
        if current.instructions(name.trim())? != *expected {
            return Err("Saved assistant changed; reload the library before updating it".into());
        }
    }
    if let Some(selection) = &selection {
        super::validate_reference_documents(&selection.expected)?;
        super::validate_reference_documents(&selection.selected)?;
        if expected.is_some() && current.reference_selection(name.trim())? != selection.expected {
            return Err("Saved assistant references changed; reload the library before updating it".into());
        }
    }
    let mut next = current.with_saved(name, instructions, expected.is_some())?;
    if let Some(selection) = selection {
        next = next.with_reference_selection(name.trim(), selection.selected)?;
    }
    let bytes = next.encode()?;
    if next == current {
        return Ok(current);
    }
    // Do not rotate away the backup on unchanged saves or after a failed current read.
    if dir.join(CURRENT).try_exists().map_err(|e| format!("Cannot inspect assistant catalog: {e}"))? {
        atomic::atomic_write(&dir.join(PREVIOUS), &current.encode()?).map_err(|e| format!("Cannot retain previous assistant catalog: {e}"))?;
        super::catalog_history::retain(dir, &current)?;
    }
    atomic::atomic_write(&dir.join(CURRENT), &bytes).map_err(|e| format!("Cannot save assistant: {e}"))?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn role(text: &str) -> Instructions {
        Instructions { role: text.into(), ..Default::default() }
    }
    fn reviewed_role(before: &Instructions, text: &str) -> Instructions {
        let proposed = super::super::preservation::merge(before, &role(text), false).unwrap();
        let entry = proposed.proposed.iter().find(|entry| entry.text == text).unwrap();
        super::super::preservation::review(&proposed, entry, "accept").unwrap()
    }
    #[test]
    fn updates_preserve_previous_and_reject_another_projects_stale_writer() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "Editor", role("Original"), None).unwrap();
        let updated = reviewed_role(&role("Original"), "Updated");
        save(dir.path(), "Editor", updated.clone(), Some(&role("Original"))).unwrap();
        let before = std::fs::read(dir.path().join(CURRENT)).unwrap();
        let backup = std::fs::read(dir.path().join(PREVIOUS)).unwrap();
        assert_eq!(Library::decode(&backup).unwrap().instructions("Editor").unwrap(), role("Original"));
        assert!(save(dir.path(), "Editor", role("Stale"), Some(&role("Original"))).is_err());
        assert_eq!(std::fs::read(dir.path().join(CURRENT)).unwrap(), before);
        assert_eq!(std::fs::read(dir.path().join(PREVIOUS)).unwrap(), backup);
        save(dir.path(), "Editor", updated.clone(), Some(&updated)).unwrap();
        assert_eq!(std::fs::read(dir.path().join(PREVIOUS)).unwrap(), backup);
    }
    #[test]
    fn deliberate_restore_changes_only_selected_assistant_and_retains_undo_version() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "Editor", role("Original"), None).unwrap();
        let updated = reviewed_role(&role("Original"), "Updated");
        save(dir.path(), "Editor", updated.clone(), Some(&role("Original"))).unwrap();
        save(dir.path(), "Other", role("Other role"), None).unwrap();
        let revision = super::super::catalog_history::list(dir.path()).unwrap()[0].id;
        let before = std::fs::read(dir.path().join(CURRENT)).unwrap();
        assert!(restore(dir.path(), "Editor", revision, &role("stale")).is_err());
        assert_eq!(std::fs::read(dir.path().join(CURRENT)).unwrap(), before);
        let restored = restore(dir.path(), "Editor", revision, &updated).unwrap();
        assert_eq!(restored.instructions("Editor").unwrap(), role("Original"));
        assert_eq!(restored.instructions("Other").unwrap(), role("Other role"));
        let undo_revision = super::super::catalog_history::list(dir.path()).unwrap().last().unwrap().id;
        let undone = restore(dir.path(), "Editor", undo_revision, &role("Original")).unwrap();
        assert_eq!(undone.instructions("Editor").unwrap(), updated);
    }
    #[test]
    fn backup_failure_and_corrupt_current_never_replace_catalog() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "Editor", role("Original"), None).unwrap();
        let before = std::fs::read(dir.path().join(CURRENT)).unwrap();
        std::fs::create_dir(dir.path().join(PREVIOUS)).unwrap();
        assert!(save(dir.path(), "Editor", role("Updated"), Some(&role("Original"))).is_err());
        assert_eq!(std::fs::read(dir.path().join(CURRENT)).unwrap(), before);
        std::fs::write(dir.path().join(CURRENT), b"broken").unwrap();
        assert!(save(dir.path(), "Other", role("Other"), None).is_err());
        assert_eq!(std::fs::read(dir.path().join(CURRENT)).unwrap(), b"broken");
    }
    #[test]
    fn reference_updates_are_guarded_and_instruction_autosave_preserves_selection() {
        let dir = tempfile::tempdir().unwrap();
        let selected = Some(std::collections::BTreeSet::from([7]));
        save_with_selection(dir.path(), "Editor", role("Original"), None, Some(SelectionUpdate { expected: None, selected: selected.clone() })).unwrap();
        let updated = reviewed_role(&role("Original"), "Updated");
        save(dir.path(), "Editor", updated.clone(), Some(&role("Original"))).unwrap();
        assert_eq!(read(dir.path()).unwrap().reference_selection("Editor").unwrap(), selected);
        let before = std::fs::read(dir.path().join(CURRENT)).unwrap();
        assert!(save_with_selection(dir.path(), "Editor", updated.clone(), Some(&updated), Some(SelectionUpdate { expected: None, selected: None })).is_err());
        assert_eq!(std::fs::read(dir.path().join(CURRENT)).unwrap(), before);
        save_with_selection(
            dir.path(),
            "Editor",
            updated.clone(),
            Some(&updated),
            Some(SelectionUpdate { expected: selected, selected: Some(Default::default()) }),
        )
        .unwrap();
        assert_eq!(read(dir.path()).unwrap().reference_selection("Editor").unwrap(), Some(Default::default()));
    }
}
