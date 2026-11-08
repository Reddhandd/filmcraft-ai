//! Durable API history. In-flight checkpoints are never automatically replayed.
use super::{protocol, turn::Conversation};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
const MAX_STATE: usize = 20 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    provider: String,
    id: String,
    project: String,
    model: String,
    in_flight: bool,
    conversation: Conversation,
}
pub struct Journal {
    path: PathBuf,
    id: String,
    project: String,
    _lock: File,
}
pub(super) fn valid_id(id: &str) -> bool {
    id.starts_with("api-") && id.len() <= 96 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn regular(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err("API session storage must use regular private files".into()),
    }
}
/// Bound JSON expansion as well as file bytes (images legitimately need multi-MiB strings).
fn decode(bytes: &[u8]) -> Result<Snapshot, String> {
    use serde::de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor};
    struct Budget {
        nodes: usize,
        bytes: usize,
    }
    struct Seed<'a> {
        budget: &'a mut Budget,
        depth: usize,
    }
    impl<'de> DeserializeSeed<'de> for Seed<'_> {
        type Value = serde_json::Value;
        fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
            if self.depth > 64 || self.budget.nodes >= 65536 {
                return Err(D::Error::custom("API state tree budget exceeded"));
            }
            self.budget.nodes = self.budget.nodes.saturating_add(1);
            deserializer.deserialize_any(self)
        }
    }
    impl<'de> Visitor<'de> for Seed<'_> {
        type Value = serde_json::Value;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded API state JSON")
        }
        fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
            Ok(value.into())
        }
        fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
            Ok(value.into())
        }
        fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
            Ok(value.into())
        }
        fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
            serde_json::Number::from_f64(value).map(serde_json::Value::Number).ok_or_else(|| E::custom("Invalid API state number"))
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(serde_json::Value::Null)
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
            if value.len() > 4 * 1024 * 1024 || self.budget.bytes.saturating_add(value.len()) > MAX_STATE {
                return Err(E::custom("API state string budget exceeded"));
            }
            self.budget.bytes = self.budget.bytes.saturating_add(value.len());
            Ok(value.into())
        }
        fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
            if value.len() > 4 * 1024 * 1024 || self.budget.bytes.saturating_add(value.len()) > MAX_STATE {
                return Err(E::custom("API state string budget exceeded"));
            }
            self.budget.bytes = self.budget.bytes.saturating_add(value.len());
            Ok(value.into())
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
            let mut values = Vec::new();
            while let Some(value) = sequence.next_element_seed(Seed { budget: self.budget, depth: self.depth.saturating_add(1) })? {
                values.push(value);
            }
            Ok(serde_json::Value::Array(values))
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut values = serde_json::Map::new();
            while let Some(key) = map.next_key::<String>()? {
                if key.len() > 256 || values.contains_key(&key) {
                    return Err(A::Error::custom("Invalid or duplicate API state key"));
                }
                self.budget.bytes = self.budget.bytes.saturating_add(key.len());
                if self.budget.bytes > MAX_STATE {
                    return Err(A::Error::custom("API state key budget exceeded"));
                }
                let value = map.next_value_seed(Seed { budget: self.budget, depth: self.depth.saturating_add(1) })?;
                values.insert(key, value);
            }
            Ok(serde_json::Value::Object(values))
        }
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let mut budget = Budget { nodes: 0, bytes: 0 };
    let value =
        Seed { budget: &mut budget, depth: 0 }.deserialize(&mut deserializer).map_err(|_| "Saved API session has malformed or oversized JSON structure")?;
    deserializer.end().map_err(|_| "Saved API session has trailing data")?;
    serde_json::from_value(value).map_err(|_| "Saved API session has malformed or unsupported history".into())
}

fn scope_key(value: &str) -> Result<String, String> {
    if value.is_empty() || value.len() > 4096 {
        return Err("Invalid API project scope".into());
    }
    let path = std::path::absolute(value).map_err(|_| "Cannot resolve the API project scope")?;
    let key = path.into_os_string().into_string().map_err(|_| "API project scope is not valid Unicode")?;
    if key.len() > 8192 {
        return Err("Resolved API project scope exceeds its byte budget".into());
    }
    Ok(key)
}
impl Journal {
    pub fn open(
        root: &Path,
        project: &str,
        model: &str,
        resume: Option<&str>,
        tools: &[protocol::Tool],
        aliases: &[String],
    ) -> Result<(Self, Conversation), String> {
        let resolved_root = std::path::absolute(root).map_err(|_| "Cannot resolve the API session storage directory")?;
        let root = resolved_root.as_path();
        if !root.is_absolute()
            || project.is_empty()
            || project.len() > 4096
            || resume.is_some_and(|id| !valid_id(id))
            || aliases.len() > 16
            || aliases.iter().any(|scope| scope.is_empty() || scope.len() > 4096)
        {
            return Err("Invalid API session scope or identifier".into());
        }
        let project = scope_key(project)?;
        let aliases: Vec<String> = aliases.iter().map(|scope| scope_key(scope)).collect::<Result<_, _>>()?;
        protocol::request(model, "", vec![], tools)?;
        let mut directory = std::fs::DirBuilder::new();
        directory.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directory.mode(0o700);
        }
        directory.create(root).map_err(|_| "Cannot create private API session storage")?;
        let (id, path) = match resume {
            Some(id) => (id.to_owned(), root.join(id)),
            None => {
                let file = tempfile::Builder::new().prefix("api-").rand_bytes(16).tempfile_in(root).map_err(|_| "Cannot create private API session")?;
                let id =
                    file.path().file_name().and_then(|name| name.to_str()).filter(|id| valid_id(id)).ok_or("Cannot create API session identity")?.to_owned();
                let (_file, path) = file.keep().map_err(|_| "Cannot retain private API session")?;
                (id, path)
            }
        };
        let lock_path = root.join(format!("{id}.lock"));
        regular(&lock_path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(lock_path).map_err(|_| "Cannot open API session ownership lock")?;
        lock.try_lock().map_err(|_| "This API session is already open or its lock is unavailable")?;
        let journal = Self { path, id, project: project.clone(), _lock: lock };
        if resume.is_some() {
            regular(&journal.path)?;
            let file = File::open(&journal.path).map_err(|_| "Saved API session is missing or unreadable")?;
            if file.metadata().map_err(|_| "Cannot inspect saved API session")?.len() > MAX_STATE as u64 {
                return Err("Saved API session exceeds its byte budget".into());
            }
            let mut bytes = Vec::new();
            file.take((MAX_STATE as u64).saturating_add(1)).read_to_end(&mut bytes).map_err(|_| "Cannot read saved API session")?;
            if bytes.len() > MAX_STATE {
                return Err("Saved API session exceeds its byte budget".into());
            }
            let mut saved = decode(&bytes)?;
            drop(bytes);
            if saved.version != 1
                || saved.provider != "openAiApi"
                || saved.id != journal.id
                || (saved.project != project && !(aliases.contains(&project) && aliases.contains(&saved.project)))
            {
                return Err("Saved API session belongs to a different provider, project or format; start a new conversation".into());
            }
            let discarded = saved.conversation.discard_captures();
            if discarded {
                journal.save(&saved.model, saved.in_flight, &saved.conversation)?;
            }
            if saved.in_flight {
                return Err("Saved API turn was interrupted and may have changed the project. Inspect project/history/jobs and start a new conversation; it will not be replayed".into());
            }
            protocol::request(&saved.model, "", vec![], tools)?;
            saved.conversation.validate(tools)?;
            if saved.project != project {
                journal.save(model, false, &saved.conversation)?;
            }
            return Ok((journal, saved.conversation));
        }
        let conversation = Conversation::default();
        journal.save(model, false, &conversation)?;
        Ok((journal, conversation))
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn save(&self, model: &str, in_flight: bool, conversation: &Conversation) -> Result<(), String> {
        // Keep the independent lock inode open across atomic data-file replacement.
        regular(&self.path)?;
        let data = protocol::bounded_json(
            &serde_json::json!({"version":1,"provider":"openAiApi","id":self.id,"project":self.project,"model":model,"in_flight":in_flight,"conversation":conversation.checkpoint()}),
            MAX_STATE,
        )?;
        let parent = self.path.parent().ok_or("Invalid API session storage directory")?;
        let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(|_| "Cannot stage private API session state")?;
        staged.write_all(&data).and_then(|()| staged.as_file().sync_all()).map_err(|_| "Cannot write API session checkpoint; no further requests were sent")?;
        staged.persist(&self.path).map_err(|_| "Cannot publish API session checkpoint; inspect native state before repeating edits")?;
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sessions_reopen_only_clean_state_and_hold_one_writer() {
        let root = tempfile::tempdir().unwrap();
        let (journal, conversation) = Journal::open(root.path(), "project", "fake-model", None, &[], &[]).unwrap();
        let id = journal.id().to_owned();
        assert!(Journal::open(root.path(), "project", "fake-model", Some(&id), &[], &[]).is_err());
        journal.save("fake-model", true, &conversation).unwrap();
        drop(journal);
        assert!(Journal::open(root.path(), "project", "fake-model", Some(&id), &[], &[]).err().unwrap().contains("interrupted"));
        assert!(Journal::open(root.path(), "other", "fake-model", Some(&id), &[], &[]).is_err());
    }
    #[test]
    fn clean_empty_history_restores_without_credentials_or_dispatch() {
        let root = tempfile::tempdir().unwrap();
        let (journal, _) = Journal::open(root.path(), "project", "fake-model", None, &[], &[]).unwrap();
        let id = journal.id().to_owned();
        let path = journal.path.clone();
        drop(journal);
        let (_restored, _) = Journal::open(root.path(), "project", "fake-model", Some(&id), &[], &[]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("OPENAI_API_KEY"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod resume_tests;

#[cfg(test)]
#[path = "capture_tests.rs"]
mod capture_tests;
