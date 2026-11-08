//! Additive teaching: omitted fields never delete knowledge; revisions are user-reviewed.
use super::Instructions;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Section {
    Role,
    Goal,
    Preference,
    Workflow,
}
/// Section plus exact text is a stable teaching identity across saves and builds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Teaching {
    pub section: Section,
    pub text: String,
}
impl Teaching {
    fn new(section: Section, text: &str) -> Self {
        Self { section, text: text.to_owned() }
    }
}
pub(super) fn validate_metadata(instructions: &Instructions) -> Result<(), String> {
    for (list, limit) in [(&instructions.proposed, 64), (&instructions.approved, 256), (&instructions.forgotten, 256)] {
        if list.len() > limit {
            return Err("Assistant teaching review/history is full; review entries before adding more".into());
        }
        for (index, entry) in list.iter().enumerate() {
            let limit = if entry.section == Section::Goal { 8192 } else { 4096 };
            if entry.text.trim().is_empty() || entry.text.len() > limit || list.iter().take(index).any(|old| old == entry) {
                return Err("Assistant teaching identities must be nonempty, unique and within their text budget".into());
            }
        }
    }
    Ok(())
}
fn contains(instructions: &Instructions, entry: &Teaching) -> bool {
    match entry.section {
        Section::Role => instructions.role == entry.text,
        Section::Goal => instructions.goal == entry.text,
        Section::Preference => instructions.preferences.contains(&entry.text),
        Section::Workflow => instructions.workflow.contains(&entry.text),
    }
}
fn erase(instructions: &mut Instructions, entry: &Teaching) {
    match entry.section {
        Section::Role if instructions.role == entry.text => instructions.role.clear(),
        Section::Goal if instructions.goal == entry.text => instructions.goal.clear(),
        Section::Preference => instructions.preferences.retain(|text| text != &entry.text),
        Section::Workflow => instructions.workflow.retain(|text| text != &entry.text),
        _ => {}
    }
    instructions.proposed.retain(|old| old != entry);
    instructions.approved.retain(|old| old != entry);
}
fn insert(instructions: &mut Instructions, entry: &Teaching) {
    match entry.section {
        Section::Role => instructions.role.clone_from(&entry.text),
        Section::Goal => instructions.goal.clone_from(&entry.text),
        Section::Preference => instructions.preferences.push(entry.text.clone()),
        Section::Workflow => instructions.workflow.push(entry.text.clone()),
    }
}
pub(super) fn entries(instructions: &Instructions) -> impl Iterator<Item = Teaching> + '_ {
    [(Section::Role, instructions.role.as_str()), (Section::Goal, instructions.goal.as_str())]
        .into_iter()
        .chain(instructions.preferences.iter().map(|text| (Section::Preference, text.as_str())))
        .chain(instructions.workflow.iter().map(|text| (Section::Workflow, text.as_str())))
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(section, text)| Teaching::new(section, text))
}
/// Model-supplied snapshots cannot authorize deletions, approve proposals or alter tombstones.
/// Catalog saves may carry metadata already produced by native review of project teaching.
pub(super) fn merge(current: &Instructions, incoming: &Instructions, native_metadata: bool) -> Result<Instructions, String> {
    current.validate()?;
    incoming.validate()?;
    let mut next = current.clone();
    if native_metadata {
        for entry in &incoming.forgotten {
            if !next.forgotten.contains(entry) {
                next.forgotten.push(entry.clone());
            }
            erase(&mut next, entry);
        }
    }
    let initially_empty = current.role.is_empty() && current.goal.is_empty() && current.preferences.is_empty() && current.workflow.is_empty();
    for entry in entries(incoming) {
        if contains(&next, &entry) || next.forgotten.contains(&entry) {
            continue;
        }
        let vacant = match entry.section {
            Section::Role => next.role.is_empty(),
            Section::Goal => next.goal.is_empty(),
            Section::Preference | Section::Workflow => initially_empty,
        };
        let approved_addition = native_metadata && incoming.approved.contains(&entry) && matches!(entry.section, Section::Preference | Section::Workflow);
        if vacant || approved_addition {
            next.proposed.retain(|old| old != &entry);
            insert(&mut next, &entry);
            if !next.approved.contains(&entry) {
                next.approved.push(entry);
            }
        } else if !next.proposed.contains(&entry) {
            // Semantic contradictions cannot be reliably detected with string matching.
            // Retain new teaching for review rather than silently activating a conflict.
            next.proposed.push(entry);
        }
    }
    if native_metadata {
        for entry in &incoming.proposed {
            if !contains(&next, entry) && !next.forgotten.contains(entry) && !next.proposed.contains(entry) {
                next.proposed.push(entry.clone());
            }
        }
    }
    next.validate()?;
    Ok(next)
}
/// Called by explicit native review, never by model-authored structured-memory updates.
pub(super) fn review(current: &Instructions, entry: &Teaching, action: &str) -> Result<Instructions, String> {
    current.validate()?;
    let mut next = current.clone();
    match action {
        "accept" if current.proposed.contains(entry) => {
            let old = match entry.section {
                Section::Role if !current.role.is_empty() => Some(Teaching::new(Section::Role, &current.role)),
                Section::Goal if !current.goal.is_empty() => Some(Teaching::new(Section::Goal, &current.goal)),
                _ => None,
            };
            if let Some(old) = old {
                if !next.forgotten.contains(&old) {
                    next.forgotten.push(old.clone());
                }
                erase(&mut next, &old);
            }
            next.proposed.retain(|old| old != entry);
            if !contains(&next, entry) {
                insert(&mut next, entry);
            }
            if !next.approved.contains(entry) {
                next.approved.push(entry.clone());
            }
        }
        "reject" if current.proposed.contains(entry) => {
            erase(&mut next, entry);
            if !next.forgotten.contains(entry) {
                next.forgotten.push(entry.clone());
            }
        }
        "forget" if contains(current, entry) => {
            erase(&mut next, entry);
            if !next.forgotten.contains(entry) {
                next.forgotten.push(entry.clone());
            }
        }
        _ => return Err("Teaching changed or review action is invalid; reload before reviewing".into()),
    }
    next.validate()?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_updates_keep_band_workflow_and_queue_new_learning_without_duplicates() {
        let first =
            Instructions { role: "Band editor".into(), workflow: vec!["Synchronize performance clips to the master song".into()], ..Default::default() };
        let addition = Instructions { preferences: vec!["Keep wide shots for introductions".into()], ..Default::default() };
        let next = merge(&first, &addition, false).unwrap();
        assert_eq!(next.workflow, first.workflow);
        assert_eq!(next.role, first.role);
        assert_eq!(next.proposed.len(), 1);
        assert_eq!(merge(&next, &addition, false).unwrap(), next);
        assert_eq!(merge(&next, &Instructions::default(), false).unwrap(), next);
        let accepted = review(&next, &next.proposed[0], "accept").unwrap();
        assert_eq!(accepted.preferences, addition.preferences);
        assert_eq!(accepted.workflow, first.workflow);
    }
    #[test]
    fn conflicts_require_review_and_forgotten_teaching_cannot_reappear() {
        let first = Instructions { role: "Band editor".into(), ..Default::default() };
        let incoming = Instructions { role: "Trailer editor".into(), ..Default::default() };
        let next = merge(&first, &incoming, false).unwrap();
        assert_eq!(next.role, first.role);
        let accepted = review(&next, &next.proposed[0], "accept").unwrap();
        assert_eq!(accepted.role, incoming.role);
        assert_eq!(merge(&accepted, &first, false).unwrap().role, incoming.role);
        assert!(merge(&accepted, &first, false).unwrap().proposed.is_empty());
        let forged = Instructions { forgotten: vec![Teaching::new(Section::Role, "Band editor")], ..Default::default() };
        assert_eq!(merge(&first, &forged, false).unwrap(), first);
        let forgotten = review(&first, &Teaching::new(Section::Role, "Band editor"), "forget").unwrap();
        assert!(merge(&forgotten, &first, false).unwrap().role.is_empty());
    }
}
