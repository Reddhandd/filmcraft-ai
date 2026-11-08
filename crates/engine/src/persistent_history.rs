//! Project-specific persistent undo/redo snapshots. Failure never blocks the project file.
use crate::Session;
use filmcraft_project::Project;
use std::{
    fs,
    io::{self, Read},
    path::PathBuf,
    sync::Arc,
};

const MAGIC: &[u8] = b"FCHISTORY1\n";
const MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 200;

fn sidecar(path: &str) -> PathBuf {
    PathBuf::from(format!("{path}.undo"))
}
fn put_blob(w: &mut Vec<u8>, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() > u32::MAX as usize || w.len().checked_add(4).and_then(|n| n.checked_add(bytes.len())).is_none_or(|n| n > MAX_BYTES) {
        return Err(io::Error::other("undo snapshot too large"));
    }
    w.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    w.extend_from_slice(bytes);
    Ok(())
}
fn take_blob<'a>(input: &mut &'a [u8]) -> io::Result<&'a [u8]> {
    if input.len() < 4 {
        return Err(io::Error::other("truncated undo history"));
    }
    let (prefix, rest) = input.split_at(4);
    let len = u32::from_le_bytes(prefix.try_into().map_err(|_| io::Error::other("bad length"))?) as usize;
    if len > MAX_BYTES || rest.len() < len {
        return Err(io::Error::other("invalid undo history length"));
    }
    let (value, remainder) = rest.split_at(len);
    *input = remainder;
    Ok(value)
}
fn encode_entries(out: &mut Vec<u8>, entries: &[(String, Arc<Project>)]) -> io::Result<()> {
    if entries.len() > MAX_ENTRIES {
        return Err(io::Error::other("too many undo entries"));
    }
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (label, project) in entries {
        put_blob(out, label.as_bytes())?;
        put_blob(out, &filmcraft_format::encode(project, false))?;
        if out.len() > MAX_BYTES {
            return Err(io::Error::other("undo journal exceeds 256 MB"));
        }
    }
    Ok(())
}
fn decode_entries(input: &mut &[u8]) -> io::Result<Vec<(String, Arc<Project>)>> {
    let count = take_count(input)?;
    if count > MAX_ENTRIES {
        return Err(io::Error::other("undo history entry count too large"));
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let label = String::from_utf8(take_blob(input)?.to_vec()).map_err(|_| io::Error::other("invalid undo label"))?;
        let project = filmcraft_format::decode(take_blob(input)?).map_err(|e| io::Error::other(e.to_string()))?.project;
        entries.push((label, Arc::new(project)));
    }
    Ok(entries)
}
fn take_count(input: &mut &[u8]) -> io::Result<usize> {
    if input.len() < 4 {
        return Err(io::Error::other("truncated undo count"));
    }
    let (bytes, rest) = input.split_at(4);
    *input = rest;
    Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| io::Error::other("bad count"))?) as usize)
}

/// Persist history only when the on-disk project matches the history's base project.
/// A .undo file is optional and never changes .fcproj format or save behavior.
pub fn save(session: &Session, path: &str) -> io::Result<()> {
    save_snapshot(&session.project, &session.history, path)
}

pub(crate) fn save_snapshot(project: &Project, history: &crate::History, path: &str) -> io::Result<()> {
    let mut bytes = MAGIC.to_vec();
    put_blob(&mut bytes, &filmcraft_format::encode(project, false))?;
    encode_entries(&mut bytes, &history.undo)?;
    encode_entries(&mut bytes, &history.redo)?;
    if bytes.len() > MAX_BYTES {
        return Err(io::Error::other("undo history exceeds 256 MB"));
    }
    filmcraft_format::atomic_write(&sidecar(path), &bytes)
}

pub fn load(session: &mut Session, path: &str) -> io::Result<()> {
    load_snapshot(session, path, Some(path))
}

pub(crate) fn load_snapshot(session: &mut Session, path: &str, media_path: Option<&str>) -> io::Result<()> {
    let path = sidecar(path);
    // Opening a FIFO can block before a handle metadata check can reject it.
    if !fs::metadata(&path)?.is_file() {
        return Err(io::Error::other("undo history must be a regular file"));
    }
    let file = fs::File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::other("undo history must be a regular file"));
    }
    if meta.len() > MAX_BYTES as u64 {
        return Err(io::Error::other("undo file too large"));
    }
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(io::Error::other("undo file too large"));
    }
    let mut input = bytes.as_slice();
    if !input.starts_with(MAGIC) {
        return Err(io::Error::other("unknown history version"));
    }
    input = &input[MAGIC.len()..];
    let expected = take_blob(&mut input)?;
    if expected != filmcraft_format::encode(&session.project, false) {
        return Err(io::Error::other("undo history belongs to another project revision"));
    }
    let undo = decode_entries(&mut input)?;
    let redo = decode_entries(&mut input)?;
    if !input.is_empty() {
        return Err(io::Error::other("trailing history data"));
    }
    for (_, project) in undo.iter().chain(&redo) {
        session.media.add_stored(project, media_path);
    }
    session.history.undo = undo;
    session.history.redo = redo;
    session.history.limit = MAX_ENTRIES;
    session.history.merge_key = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_round_trip() {
        let mut s = Session::default();
        s.history.undo.push(("Change".into(), s.project.clone()));
        let dir = std::env::temp_dir().join(format!("filmcraft-history-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.fcproj");
        let path = path.to_string_lossy();
        save(&s, &path).unwrap();
        let mut t = Session::default();
        load(&mut t, &path).unwrap();
        assert_eq!(t.history.undo.len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn named_pipe_history_is_rejected_without_waiting_for_a_writer() {
        let dir = std::env::temp_dir().join(format!("filmcraft-history-pipe-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.fcproj").to_string_lossy().into_owned();
        let fifo = sidecar(&path);
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(load(&mut Session::default(), &path).map_err(|e| e.to_string()));
        });
        let error = rx.recv_timeout(std::time::Duration::from_secs(1)).expect("history open must not block").unwrap_err();
        assert!(error.contains("regular file"));
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn corrupt_history_does_not_change_session() {
        let dir = std::env::temp_dir().join(format!("filmcraft-corrupt-history-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.fcproj").to_string_lossy().to_string();
        fs::write(sidecar(&path), b"corrupt").unwrap();
        let mut session = Session::default();
        let original = session.project.clone();
        assert!(load(&mut session, &path).is_err());
        assert!(std::sync::Arc::ptr_eq(&original, &session.project));
        assert!(session.history.undo.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn history_rejects_different_project_revision() {
        let dir = std::env::temp_dir().join(format!("filmcraft-mismatched-history-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.fcproj").to_string_lossy().to_string();
        let mut s = Session::default();
        s.history.undo.push(("Change".into(), s.project.clone()));
        save(&s, &path).unwrap();
        let bytes = sidecar(&path);
        let mut raw = fs::read(&bytes).unwrap();
        raw[MAGIC.len() + 4] ^= 0x01;
        fs::write(&bytes, raw).unwrap();
        let mut t = Session::default();
        assert!(load(&mut t, &path).is_err());
        assert!(t.history.undo.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ambiguous_identities_in_an_old_snapshot_do_not_replace_existing_history() {
        let dir = std::env::temp_dir().join(format!("filmcraft-history-identities-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.fcproj").to_string_lossy().into_owned();
        let mut source = Session::default();
        source.execute("file.newSequence", serde_json::json!({"video": 2, "audio": 1})).unwrap();
        let mut damaged = (*source.project).clone();
        let q = damaged.sequence_mut(source.state.active_sequence.unwrap()).unwrap();
        q.audio_tracks[0].id = q.video_tracks[0].id;
        source.history.undo.push(("Damaged old state".into(), Arc::new(damaged)));
        save(&source, &path).unwrap();
        let mut target = Session { project: source.project.clone(), ..Default::default() };
        target.history.undo.push(("Keep existing history".into(), target.project.clone()));
        let before = target.history.clone();
        assert!(load(&mut target, &path).unwrap_err().to_string().contains("duplicate track id"));
        assert_eq!(target.history.undo, before.undo);
        assert_eq!(target.history.redo, before.redo);
        assert_eq!(target.project, source.project);
        fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod hostile_tests {
    use super::*;
    #[test]
    fn truncated_and_mutated_sidecars_never_panic_or_replace_history_on_error() {
        let dir = std::env::temp_dir().join(format!("filmcraft-history-mutation-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.fcproj").to_string_lossy().into_owned();
        let mut source = Session::default();
        source.history.undo.push(("Prior edit".into(), source.project.clone()));
        save(&source, &path).unwrap();
        let valid = fs::read(sidecar(&path)).unwrap();
        let mut samples = Vec::new();
        for length in (0..valid.len()).step_by((valid.len() / 32).max(1)) {
            samples.push(valid[..length].to_vec());
        }
        for offset in [0, MAGIC.len(), MAGIC.len() + 1, valid.len() - 1] {
            let mut bytes = valid.clone();
            bytes[offset] ^= 255;
            samples.push(bytes);
        }
        for bytes in samples {
            fs::write(sidecar(&path), bytes).unwrap();
            let mut session = Session::default();
            session.history.undo.push(("Keep".into(), session.project.clone()));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| load(&mut session, &path)));
            assert!(result.is_ok());
            if result.unwrap().is_err() {
                assert_eq!(session.history.undo[0].0, "Keep");
            }
        }
        let file = fs::File::create(sidecar(&path)).unwrap();
        file.set_len(MAX_BYTES as u64 + 1).unwrap();
        assert!(load(&mut Session::default(), &path).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
