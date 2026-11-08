use super::*;
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, TryRecvError, sync_channel},
    },
    time::{Duration, Instant},
};
static ACTIVE: AtomicBool = AtomicBool::new(false);
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct DeliveryScope {
    epoch: u64,
    project: Option<String>,
    selected: Option<std::collections::BTreeSet<u64>>,
}
fn delivery_scope(app: &FilmcraftApp) -> Result<DeliveryScope, String> {
    Ok(DeliveryScope {
        epoch: app.session.document_epoch,
        project: app.ui.console.sessions.project.clone(),
        selected: super::super::assistants::reference_documents(app)?,
    })
}
#[derive(Debug)]
struct Job {
    scope: Option<DeliveryScope>,
    ticket: u64,
    started: Instant,
    control: super::job::Control,
    cancelable: bool,
    receive: Receiver<Result<Value, String>>,
}
#[derive(Debug, Default)]
pub(super) struct Runtime {
    next: u64,
    job: Option<Job>,
    last: Option<(u64, Option<DeliveryScope>, Result<Value, String>)>,
    catalog: Value,
    display: String,
}
fn root(app: &FilmcraftApp) -> Result<PathBuf, String> {
    app.session
        .persistence
        .as_ref()
        .map(|p| p.data_dir.clone())
        .or_else(|| app.session.prefs_path.as_ref().and_then(|p| p.parent()).map(|p| p.to_path_buf()))
        .map(|p| p.join("Console/ReferenceLibrary"))
        .ok_or("A local data directory is required for the reference library".into())
}
fn poll(r: &mut Runtime, scope: &Result<DeliveryScope, String>) {
    if let Some((_, Some(saved_scope), value)) = &mut r.last
        && scope.as_ref() != Ok(saved_scope)
    {
        *value = Err("Project or reference selection changed; start a fresh library search".into());
    }
    let result = match r.job.as_ref() {
        Some(job) => match job.receive.try_recv() {
            Ok(value) => Some(value),
            Err(TryRecvError::Disconnected) => Some(Err("Reference library worker stopped; previous index is preserved".into())),
            Err(TryRecvError::Empty)
                if job.started.elapsed() > Duration::from_secs(if job.cancelable { 30 * 60 + 30 } else { 180 }) && !job.control.publishing() =>
            {
                job.control.cancel();
                Some(Err("Reference library operation timed out; inspect before retrying. Existing worker may still be stopping.".into()))
            }
            Err(TryRecvError::Empty) => None,
        },
        None => None,
    };
    if let Some(mut value) = result
        && let Some(job) = r.job.take()
    {
        if job.scope.as_ref().is_some_and(|saved| scope.as_ref() != Ok(saved)) {
            value = Err("Project or reference selection changed; start a fresh library search".into());
        }
        match &value {
            Ok(v) => {
                if v.get("documents").is_some() {
                    r.catalog = v.clone();
                }
                if let Some(catalog) = v.get("library") {
                    r.catalog = catalog.clone();
                }
                r.display = if v.get("hits").is_some() {
                    "Search results".into()
                } else if let Some(documents) = v.get("documents").and_then(Value::as_array) {
                    format!("{} books indexed", v.get("total").and_then(Value::as_u64).unwrap_or(documents.len() as u64))
                } else if v.get("duplicate").and_then(Value::as_bool) == Some(true) {
                    "This book is already in your library".into()
                } else if let Some(warning) = v.get("warning").and_then(Value::as_str).filter(|warning| !warning.is_empty()) {
                    format!("Book indexed. {warning}")
                } else {
                    "Assistant Library updated".into()
                };
            }
            Err(e) => r.display = e.clone(),
        }
        r.last = Some((job.ticket, job.scope, value));
    }
}
pub(super) fn invoke(app: &mut FilmcraftApp, id: &str, p: &Map<String, Value>) -> Result<Value, String> {
    let directory = root(app)?;
    if id == "console.library.selection" {
        let value = p.get("documentIds").ok_or("Supply documentIds as an array or null for the shared library")?;
        let selected = if value.is_null() {
            None
        } else {
            let entries = value.as_array().filter(|entries| entries.len() <= 512).ok_or("Select at most 512 document identities")?;
            let mut ids = std::collections::BTreeSet::new();
            for entry in entries {
                let id = entry.as_u64().filter(|id| *id > 0).ok_or("Document identities must be positive integers")?;
                if !ids.insert(id) {
                    return Err("Document selection contains repeated identities".into());
                }
            }
            Some(ids)
        };
        return super::super::assistants::select_reference_documents(app, selected);
    }
    if matches!(id, "console.library.search" | "console.library.page" | "console.library.illustration") && app.hooks.codex_connection.is_some() {
        super::super::assistants::poll(app)?;
    }
    let scope = delivery_scope(app);
    let mut r = app.ui.console.reference_library.runtime.borrow_mut();
    poll(&mut r, &scope);
    if id == "console.library.cancel" {
        let ticket = p.get("ticket").and_then(Value::as_u64).ok_or("Choose the running import ticket")?;
        let job = r.job.as_ref().filter(|job| job.ticket == ticket && job.cancelable).ok_or("No cancellable import matches this ticket")?;
        return Ok(json!({"ticket":ticket,"cancelRequested":job.control.cancel(),"progress":job.control.status()}));
    }
    if id == "console.library.status" {
        let ticket = p.get("ticket").and_then(Value::as_u64).ok_or("Supply the pending reference ticket")?;
        if let Some(job) = r.job.as_ref().filter(|job| job.ticket == ticket) {
            return Ok(json!({"status":"pending","ticket":ticket,"progress":job.control.status()}));
        }
        let pixels = r.last.as_ref().is_some_and(|(last, _, result)| *last == ticket && result.as_ref().is_ok_and(|value| value.get("pngBase64").is_some()));
        if pixels {
            let (_, _, result) = r.last.take().ok_or("Page image was already delivered; request it again")?;
            return result.map(|value| json!({"status":"complete","ticket":ticket,"result":value}));
        }
        if let Some((last, _, result)) = &r.last
            && *last == ticket
        {
            return result.clone().map(|v| json!({"status":"complete","ticket":ticket,"result":v}));
        }
        return Err("Reference ticket is unavailable; inspect the library again".into());
    }
    if r.job.is_some() {
        return Err("A reference library operation is pending; poll its ticket before starting another".into());
    }
    let action = id.strip_prefix("console.library.").ok_or("Unknown reference library command")?.to_owned();
    let ticket = r.next.checked_add(1).ok_or("Reference ticket identities exhausted")?;
    if ACTIVE.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return Err("Reference worker is still active; retry when it has stopped".into());
    }
    let permit = Permit;
    let search_scope = if matches!(action.as_str(), "search" | "page" | "illustration") { Some(scope?) } else { None };
    let mut params = Value::Object(p.clone());
    if let Some(scope) = &search_scope {
        params["selectedDocuments"] = json!(scope.selected);
    }
    let control = super::job::Control::default();
    let worker_control = control.clone();
    let cancelable = action == "import";
    let (send, receive) = sync_channel(1);
    std::thread::Builder::new()
        .name("filmcraft-reference-library".into())
        .spawn(move || {
            let _permit = permit;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut value = super::store::perform_controlled(&directory, &action, &params, &worker_control)?;
                if matches!(action.as_str(), "import" | "remove" | "enable" | "recover") {
                    value["library"] = super::store::perform(&directory, "inspect", &json!({}))?;
                }
                Ok::<Value, String>(value)
            }))
            .unwrap_or_else(|_| Err("Reference library worker failed safely; previous library is preserved".into()));
            drop(_permit);
            let _ = send.send(result);
        })
        .map_err(|e| format!("Cannot start reference library worker: {e}"))?;
    r.next = ticket;
    // Starting another operation invalidates cached deliveries, including pixels.
    r.last = None;
    r.job = Some(Job { scope: search_scope, ticket, started: Instant::now(), control, cancelable, receive });
    Ok(json!({"status":"pending","ticket":ticket,"pollCommand":"console.library.status"}))
}
fn run(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, p: Value) {
    if let Err(e) = crate::menus::invoke(app, ctx, id, p) {
        app.ui.status = e;
    }
}
pub(super) fn button(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let response = super::super::layout::icon_button(
        app,
        ui,
        "console.library.open",
        crate::icons::Icon::Folder,
        true,
        false,
        "Assistant Library · persistent indexed books and notes",
    );
    if response.clicked() {
        app.ui.console.reference_library.open = true;
        run(app, ui.ctx(), "console.library.inspect", json!({}));
    }
}
pub(super) fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    {
        let scope = delivery_scope(app);
        let mut r = app.ui.console.reference_library.runtime.borrow_mut();
        poll(&mut r, &scope);
        if r.job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
    if !app.ui.console.reference_library.open {
        return;
    }
    let mut open = true;
    let mut action = None;
    let mut pick = false;
    let mut pick_package = false;
    let (catalog, display, busy, result) = {
        let r = app.ui.console.reference_library.runtime.borrow();
        (r.catalog.clone(), r.display.clone(), r.job.is_some(), r.last.as_ref().and_then(|(_, _, result)| result.as_ref().ok()).cloned().unwrap_or(Value::Null))
    };
    let import_progress =
        app.ui.console.reference_library.runtime.borrow().job.as_ref().filter(|job| job.cancelable).map(|job| (job.ticket, job.control.status()));
    let project_selection = super::super::assistants::reference_documents(app);
    egui::Window::new("Assistant Library").id(egui::Id::unique("filmcraft-reference-library")).open(&mut open).default_width(600.0).vscroll(true).show(
        ctx,
        |ui| {
            let install = ui.add_enabled(!busy, egui::Button::new("Install Local OCR · 12 MB"));
            app.auto.add("console.library.ocrInstall", install.rect, "Download checksum-verified OCR models; book pages stay on this Mac");
            if install.clicked() {
                action = Some(("console.library.ocrInstall", json!({})));
            }
            ui.label("Books stay local. Only requested excerpts and page images go to the connected AI. Scanned pages require verified local OCR models.");
            match &project_selection {
                Ok(selected) => {
                    let shared = ui.radio(selected.is_none(), "Use all available books");
                    app.auto.add("console.library.selection.shared", shared.rect, "Use the shared reference library for this project");
                    if shared.clicked() {
                        action = Some(("console.library.selection", json!({"documentIds":null})));
                    }
                    let scoped = ui.radio(selected.is_some(), "Choose books for this project");
                    app.auto.add("console.library.selection.project", scoped.rect, "Choose an explicit project reference subset");
                    if scoped.clicked() {
                        action = Some(("console.library.selection", json!({"documentIds":[]})));
                    }
                    if selected.is_some() {
                        ui.small("Save or Update Assistant to reuse this selection with that assistant. Unselected books are excluded from retrieval.");
                    }
                }
                Err(error) => {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            }
            ui.horizontal(|ui| {
                let add = ui.add_enabled(!busy, egui::Button::new("Add Book…"));
                app.auto.add("console.library.pick", add.rect, "Add a local book to the persistent library");
                pick = add.clicked();
                let package = ui.add_enabled(!busy, egui::Button::new("Add EPUB Folder…"));
                app.auto.add("console.library.pickPackage", package.rect, "Import an unpacked, unencrypted EPUB directory");
                pick_package = package.clicked();
                let refresh = ui.add_enabled(!busy, egui::Button::new("Refresh"));
                app.auto.add("console.library.refresh", refresh.rect, "Refresh library catalog");
                if refresh.clicked() {
                    action = Some(("console.library.inspect", json!({})));
                }
            });
            if catalog.get("recoveredPrevious").and_then(Value::as_bool) == Some(true) {
                ui.colored_label(ui.visuals().error_fg_color, "Recovered an older index. Search is disabled until a valid index is restored.");
            }
            {
                let recover = ui.add_enabled(!busy, egui::Button::new("Recover Library Index"));
                app.auto.add(
                    "console.library.recover",
                    recover.rect,
                    "Recover a missing or damaged index while preserving original books and document removals",
                );
                if recover.clicked() {
                    action = Some(("console.library.recover", json!({})));
                }
            }
            if let Some(documents) = catalog.get("documents").and_then(Value::as_array) {
                for d in documents {
                    ui.horizontal_wrapped(|ui| {
                        let id = d.get("id").and_then(Value::as_u64).unwrap_or(0);
                        let mut enabled = d.get("enabled").and_then(Value::as_bool).unwrap_or(false);
                        let check =
                            ui.add_enabled(!busy, egui::Checkbox::new(&mut enabled, d.get("title").and_then(Value::as_str).unwrap_or("Unnamed reference")));
                        app.auto.add(&format!("console.library.enable.{id}"), check.rect, "Enable this book for retrieval");
                        if check.changed() {
                            action = Some(("console.library.enable", json!({"documentId":id,"enabled":enabled})));
                        }
                        if let Ok(Some(selected)) = &project_selection {
                            let mut chosen = selected.contains(&id);
                            let project = ui.checkbox(&mut chosen, "This project");
                            app.auto.add(&format!("console.library.selection.{id}"), project.rect, "Include this book in the project's reference selection");
                            if project.changed() {
                                let mut next = selected.clone();
                                if chosen {
                                    next.insert(id);
                                } else {
                                    next.remove(&id);
                                }
                                action = Some(("console.library.selection", json!({"documentIds":next})));
                            }
                        }
                        let remove = ui.add_enabled(!busy, egui::Button::new("Remove"));
                        app.auto.add(&format!("console.library.remove.{id}"), remove.rect, "Remove book and searchable passages from library");
                        if remove.clicked() {
                            action = Some(("console.library.remove", json!({"documentId":id})));
                        }
                    });
                    if let Some(warning) = d.get("warning").and_then(Value::as_str).filter(|warning| !warning.is_empty()) {
                        ui.colored_label(ui.visuals().warn_fg_color, warning);
                    }
                }
            }
            if let Some(next) = catalog.get("nextOffset").and_then(Value::as_u64) {
                let more = ui.add_enabled(!busy, egui::Button::new("Next Books"));
                app.auto.add("console.library.next", more.rect, "Next page of reference books");
                if more.clicked() {
                    action = Some(("console.library.inspect", json!({"offset":next,"expectedGeneration":catalog.get("generation")})));
                }
            }
            let query = ui.text_edit_singleline(&mut app.ui.console.reference_library.query);
            app.auto.add("console.library.query", query.rect, "Search reference books");
            let search = ui.add_enabled(!busy, egui::Button::new("Search"));
            app.auto.add("console.library.search", search.rect, "Search indexed reference passages");
            if search.clicked() {
                action = Some(("console.library.search", json!({"query":app.ui.console.reference_library.query,"limit":3})));
            }
            if busy {
                ui.spinner();
                if let Some((ticket, progress)) = &import_progress {
                    let publishing = progress["publishing"].as_bool() == Some(true);
                    let cancelled = progress["cancelRequested"].as_bool() == Some(true);
                    ui.label(if publishing {
                        "Finalizing library update…".into()
                    } else {
                        format!("Indexing page {} of {}", progress["page"], progress["totalPages"])
                    });
                    let cancel = ui.add_enabled(!publishing && !cancelled, egui::Button::new("Cancel Import"));
                    app.auto.add("console.library.cancel", cancel.rect, "Cancel before publication; preserve the existing library");
                    if cancel.clicked() {
                        action = Some(("console.library.cancel", json!({"ticket":ticket})));
                    }
                    if cancelled {
                        ui.small("Stopping import; the existing library is preserved.");
                    }
                } else {
                    ui.label("Working in background…");
                }
            }
            ui.label(display);
            if let Some(hits) = result.get("hits").and_then(Value::as_array) {
                if hits.is_empty() {
                    ui.label("No matching passages. Try a different editing term.");
                }
                for hit in hits {
                    ui.group(|ui| {
                        let citation = if let Some(section) = hit.get("section").and_then(Value::as_u64) {
                            format!("section {section} · {}", hit.get("chapter").and_then(Value::as_str).unwrap_or("Untitled section"))
                        } else {
                            format!("page {}", hit.get("page").and_then(Value::as_u64).unwrap_or(0))
                        };
                        ui.strong(format!("{} · {citation}", hit.get("title").and_then(Value::as_str).unwrap_or("Reference")));
                        if let Some(warning) = hit.get("sourceWarning").and_then(Value::as_str).filter(|warning| !warning.is_empty()) {
                            ui.colored_label(ui.visuals().warn_fg_color, warning);
                        }
                        ui.label(hit.get("text").and_then(Value::as_str).unwrap_or(""));
                    });
                }
            }
        },
    );
    app.ui.console.reference_library.open = open;
    if let Some((id, p)) = action {
        run(app, ctx, id, p);
    }
    if pick_package {
        let context = ctx.clone();
        app.pick_ui(crate::native_dialogs::FileDialog::Folder { at: None }, move |app, paths| {
            if let Some(path) = paths.first() {
                run(app, &context, "console.library.import", json!({"path":path}));
            }
        });
    }
    if pick {
        let context = ctx.clone();
        app.pick_ui(crate::native_dialogs::FileDialog::OpenFiles { exts: vec!["pdf".into(), "epub".into(), "txt".into(), "md".into()] }, move |app, paths| {
            if let Some(path) = paths.first() {
                run(app, &context, "console.library.import", json!({"path":path}));
            }
            if paths.len() > 1 {
                app.ui.status = "Index one book at a time; choose Add Book again after it completes".into();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static LIBRARY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn complete(app: &mut FilmcraftApp, value: Value) -> Value {
        let ticket = value["ticket"].as_u64().unwrap();
        let start = Instant::now();
        loop {
            let value = invoke(app, "console.library.status", json!({"ticket":ticket}).as_object().unwrap()).unwrap();
            if value["status"] == "complete" {
                return value["result"].clone();
            }
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn window_menu_opens_shared_library_without_opening_console_or_connecting_ai() {
        let _guard = LIBRARY_TEST_LOCK.lock().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.prefs_path = Some(directory.path().join("prefs.json"));
        let context = egui::Context::default();
        assert!(!app.ui.console.open);
        let revision = app.session.revision;
        assert!(crate::menus::invoke(&mut app, &context, "window.assistantLibrary", json!({"path":"unexpected"})).is_err());
        assert!(!app.ui.console.reference_library.open);
        let ticket = crate::menus::invoke(&mut app, &context, "window.assistantLibrary", json!({})).unwrap();
        complete(&mut app, ticket);
        assert!(app.ui.console.reference_library.open);
        assert!(!app.ui.console.open);
        let mut output = context.run_ui(egui::RawInput::default(), |ui| crate::console::show(&mut app, ui.ctx()));
        output.textures_delta.clear();
        assert!(app.auto.all().iter().any(|element| element.id == "console.library.pick"));
        assert!(!app.ui.console.open);
        assert_eq!(app.session.revision, revision);
    }
    #[test]
    fn native_commands_restore_index_into_fresh_editor_without_changing_project() {
        let _guard = LIBRARY_TEST_LOCK.lock().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("guide.txt");
        std::fs::write(&source, "Exposure latitude protects highlights.").unwrap();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.prefs_path = Some(directory.path().join("prefs.json"));
        let context = egui::Context::default();
        let revision = app.session.revision;
        let admitted = super::super::super::commands::invoke(&mut app, &context, "console.library.import", json!({"path":source})).unwrap();
        assert!(super::super::super::commands::invoke(&mut app, &context, "console.library.search", json!({"query":"latitude"})).is_err());
        complete(&mut app, admitted);
        assert_eq!(app.session.revision, revision);
        drop(app);
        let mut fresh = FilmcraftApp::new(filmcraft_engine::Session::default());
        fresh.session.prefs_path = Some(directory.path().join("prefs.json"));
        let admitted = super::super::super::commands::invoke(&mut fresh, &context, "console.library.search", json!({"query":"latitude"})).unwrap();
        assert_eq!(complete(&mut fresh, admitted)["hits"][0]["page"], 1);
        assert!(super::super::super::commands::invoke(&mut fresh, &context, "console.library.search", json!({"query":"latitude","path":"/private"})).is_err());
    }
    #[test]
    fn pending_and_cached_search_results_are_rejected_after_project_or_selection_change() {
        let original = DeliveryScope { epoch: 1, project: Some("A".into()), selected: Some(std::collections::BTreeSet::from([1])) };
        let changed = DeliveryScope { selected: Some(std::collections::BTreeSet::from([2])), ..original.clone() };
        let another = DeliveryScope { epoch: 2, project: Some("B".into()), ..original.clone() };
        for scope in [Ok(changed), Ok(another), Err("Project memory unavailable".into())] {
            let (send, receive) = sync_channel(1);
            send.send(Ok(json!({"hits":[{"text":"Old selected passage"}]}))).unwrap();
            let mut runtime = Runtime {
                job: Some(Job {
                    scope: Some(original.clone()),
                    ticket: 1,
                    started: Instant::now(),
                    control: super::super::job::Control::default(),
                    cancelable: false,
                    receive,
                }),
                ..Default::default()
            };
            poll(&mut runtime, &scope);
            assert!(runtime.last.as_ref().unwrap().2.is_err());
            assert!(!format!("{:?}", runtime.last).contains("Old selected passage"));
            runtime.last = Some((2, Some(original.clone()), Ok(json!({"hits":[{"text":"Cached old passage"}]}))));
            poll(&mut runtime, &scope);
            assert!(runtime.last.as_ref().unwrap().2.is_err());
            assert!(!format!("{:?}", runtime.last).contains("Cached old passage"));
        }
    }
    #[test]
    fn malformed_native_selections_cannot_mutate_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.prefs_path = Some(dir.path().join("prefs.json"));
        let before = (app.session.revision, app.session.document_epoch, app.session.history.undo.len());
        for params in [
            json!({}),
            json!({"documentIds":true}),
            json!({"documentIds":[0]}),
            json!({"documentIds":[-1]}),
            json!({"documentIds":[1,1]}),
            json!({"documentIds":(1..=129).collect::<Vec<_>>()}),
        ] {
            assert!(super::super::super::commands::invoke(&mut app, &egui::Context::default(), "console.library.selection", params).is_err());
        }
        assert_eq!((app.session.revision, app.session.document_epoch, app.session.history.undo.len()), before);
        assert!(
            super::super::super::commands::invoke(
                &mut app,
                &egui::Context::default(),
                "console.library.search",
                json!({"query":"exposure","selectedDocuments":[1]})
            )
            .is_err()
        );
    }
    #[test]
    fn native_search_uses_project_and_loaded_assistant_selection() {
        let _guard = LIBRARY_TEST_LOCK.lock().unwrap();
        struct Host;
        impl crate::console::CodexConnection for Host {
            fn connect(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn poll(&mut self) -> Option<crate::console::ConnectionStatus> {
                None
            }
            fn disconnect(&mut self) {}
        }
        fn project(dir: &std::path::Path, name: &str) -> FilmcraftApp {
            let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
            app.session.prefs_path = Some(dir.join("prefs.json"));
            app.session.path = Some(dir.join(format!("{name}.fcproj")).to_string_lossy().into_owned());
            app.hooks.codex_connection = Some(Box::new(Host));
            app.ui.console.open = true;
            crate::console::sessions::sync_project(&mut app);
            crate::console::assistants::poll(&mut app).unwrap();
            app
        }
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("shared.txt");
        let second = directory.path().join("selected.txt");
        std::fs::write(&first, "Exposure latitude continuity").unwrap();
        std::fs::write(&second, "Exposure technique").unwrap();
        let mut app = project(directory.path(), "A");
        let context = egui::Context::default();
        let revision = app.session.revision;
        let admitted = invoke(&mut app, "console.library.import", json!({"path":first}).as_object().unwrap()).unwrap();
        complete(&mut app, admitted);
        let admitted = invoke(&mut app, "console.library.import", json!({"path":second}).as_object().unwrap()).unwrap();
        let selected = complete(&mut app, admitted)["documentId"].as_u64().unwrap();
        super::super::super::commands::invoke(&mut app, &context, "console.library.selection", json!({"documentIds":[selected]})).unwrap();
        let admitted = invoke(&mut app, "console.library.search", json!({"query":"exposure latitude","limit":1}).as_object().unwrap()).unwrap();
        assert_eq!(complete(&mut app, admitted)["hits"][0]["documentId"], selected);
        super::super::super::commands::invoke(&mut app, &context, "console.assistant.save", json!({"name":"Selected reader"})).unwrap();
        let mut fresh = project(directory.path(), "B");
        super::super::super::commands::invoke(&mut fresh, &context, "console.assistant.load", json!({"name":"Selected reader"})).unwrap();
        let admitted = invoke(&mut fresh, "console.library.search", json!({"query":"exposure latitude","limit":1}).as_object().unwrap()).unwrap();
        assert_eq!(complete(&mut fresh, admitted)["hits"][0]["documentId"], selected);
        super::super::super::commands::invoke(&mut fresh, &context, "console.library.selection", json!({"documentIds":[]})).unwrap();
        let admitted = invoke(&mut fresh, "console.library.search", json!({"query":"exposure","limit":1}).as_object().unwrap()).unwrap();
        assert!(complete(&mut fresh, admitted)["hits"].as_array().unwrap().is_empty());
        assert_eq!(app.session.revision, revision);
        assert_eq!(fresh.session.revision, revision);
    }
    #[test]
    fn delivered_page_pixels_are_consumed_and_stale_scope_pixels_are_discarded() {
        let _serial = LIBRARY_TEST_LOCK.lock().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.prefs_path = Some(directory.path().join("preferences.json"));
        let scope = delivery_scope(&app).unwrap();
        app.ui.console.reference_library.runtime.borrow_mut().last =
            Some((90, Some(scope.clone()), Ok(json!({"pngBase64":"test pixels","width":1,"height":1,"retainedFiles":0}))));
        let reply = invoke(&mut app, "console.library.status", json!({"ticket":90}).as_object().unwrap()).unwrap();
        assert_eq!(reply["result"]["pngBase64"], "test pixels");
        assert!(app.ui.console.reference_library.runtime.borrow().last.is_none());
        assert!(invoke(&mut app, "console.library.status", json!({"ticket":90}).as_object().unwrap()).is_err());
        app.ui.console.reference_library.runtime.borrow_mut().last = Some((91, Some(scope), Ok(json!({"pngBase64":"stale pixels","width":1,"height":1}))));
        app.session.document_epoch = app.session.document_epoch.checked_add(1).unwrap();
        assert!(invoke(&mut app, "console.library.status", json!({"ticket":91}).as_object().unwrap()).is_err());
        let runtime = app.ui.console.reference_library.runtime.borrow();
        assert!(!serde_json::to_string(&runtime.last.as_ref().unwrap().2).unwrap().contains("stale pixels"));
    }
}
