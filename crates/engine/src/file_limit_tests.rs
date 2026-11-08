use super::*;
use serde_json::json;

#[test]
fn oversized_sparse_documents_are_rejected_before_reading_or_changing_the_session() {
    let dir = std::env::temp_dir().join(format!("filmcraft-document-limits-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let project = dir.join("large.fcproj");
    std::fs::File::create(&project).unwrap().set_len(filmcraft_format::MAX_PROJECT_BYTES as u64 + 1).unwrap();
    let lut = dir.join("large.cube");
    std::fs::File::create(&lut).unwrap().set_len(filmcraft_project::MAX_LUT_TEXT_BYTES as u64 + 1).unwrap();
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let before = s.project.clone();
    let history = s.history.clone();
    let state = serde_json::to_value(&s.state).unwrap();
    let revisions = (s.revision, s.saved_revision);
    for (command, path) in [("file.open", &project), ("lut.import", &lut)] {
        let error = s.execute(command, json!({"path": path.to_string_lossy()})).unwrap_err().to_string();
        assert!(error.contains("byte limit"), "{error}");
        assert_eq!(s.project, before);
        assert_eq!(s.history.undo, history.undo);
        assert_eq!(s.history.redo, history.redo);
        assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
        assert_eq!((s.revision, s.saved_revision), revisions);
    }
    let recovery = dir.join("snapshot.fcproj");
    std::fs::rename(project, recovery).unwrap();
    let candidate = autosave::RecoveryCandidate { id: "large".into(), dir: dir.clone(), meta: Default::default() };
    assert!(autosave::load_candidate(&candidate).unwrap_err().contains("byte limit"));
    assert!(dir.join("snapshot.fcproj").exists(), "an unreadable recovery candidate is preserved");
    std::fs::remove_dir_all(dir).unwrap();
}
