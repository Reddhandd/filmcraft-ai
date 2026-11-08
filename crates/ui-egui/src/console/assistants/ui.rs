use super::*;
use serde_json::{Value, json};
pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    if let Err(error) = library_ops::ensure(app) {
        ui.label(error);
        memory_button(app, ui);
        return;
    }
    let selected = app.assistants.current.as_ref().and_then(|r| r.snapshot().assistant_name.clone());
    let names = app.assistants.library.as_ref().map(|library| library.assistants.iter().map(|a| a.name.clone()).collect::<Vec<_>>()).unwrap_or_default();
    let status = app
        .assistants
        .current
        .as_ref()
        .and_then(|r| chrono::DateTime::parse_from_rfc3339(&r.snapshot().saved_at).ok())
        .map(|saved| format!("Memory saved {} min ago", chrono::Utc::now().signed_duration_since(saved).num_seconds().max(0) / 60))
        .unwrap_or_else(|| "No checkpoint yet".into());
    let mut autosave_choice = None;
    let mut choice = None;
    let mut open = false;
    ui.small("Project assistants");
    ui.horizontal_wrapped(|ui| {
        let combo = craft_ui::ComboBox::from_id_salt("project-assistants")
            .selected_text(selected.as_deref().unwrap_or("Current project memory"))
            .width(185.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(selected.is_none(), "Current project memory").clicked() {
                    choice = Some(None);
                }
                for name in names {
                    if ui.selectable_label(selected.as_deref() == Some(name.as_str()), &name).clicked() {
                        choice = Some(Some(name));
                    }
                }
            });
        app.auto.add("console.assistant.select", combo.response.rect, "Select a saved project assistant");
        combo.response.on_hover_text(selected.as_deref().unwrap_or("Current project memory"));
        let can_save = app.assistants.current.is_some();
        let save = super::super::layout::icon_button(
            app,
            ui,
            "console.assistant.saveDialog",
            crate::icons::Icon::Save,
            can_save,
            false,
            if selected.is_some() {
                "Update Assistant… · preserve saved teaching and review new learning"
            } else {
                "Save Assistant… · name and save reusable instructions"
            },
        );
        open = save.clicked();
        memory_button(app, ui);
        if selected.is_some() {
            let mut enabled = app.assistants.current.as_ref().is_some_and(|runtime| runtime.snapshot().autosave.is_some());
            let control = ui.checkbox(&mut enabled, "Autosave");
            app.auto.add("console.assistant.autosave", control.rect, "Automatically update named assistant instructions");
            if control.changed() {
                autosave_choice = Some(enabled);
            }
            control.on_hover_text(
                "Preserve saved teaching and save new learning for review every 15 minutes and at milestones. Project history stays in this project.",
            );
        }
        ui.small(status);
    });
    if let Some(enabled) = autosave_choice
        && let Err(error) = crate::menus::invoke(app, ui.ctx(), "console.assistant.autosave", json!({"enabled":enabled}))
    {
        app.ui.status = error;
    }
    if selected.is_some() {
        let versions = ui.button("Assistant versions…");
        app.auto.add("console.assistant.history.open", versions.rect, "Review and restore saved assistant versions");
        if versions.clicked() {
            match crate::menus::invoke(app, ui.ctx(), "console.assistant.history", json!({})) {
                Ok(value) => match serde_json::from_value(value.get("revisions").cloned().unwrap_or(Value::Null)) {
                    Ok(revisions) => {
                        app.assistants.history_revisions = revisions;
                        app.assistants.history_name = selected.clone();
                        app.assistants.history_choice = None;
                        app.assistants.history_dialog = true;
                    }
                    Err(error) => app.ui.status = format!("Cannot read assistant version list: {error}"),
                },
                Err(error) => app.ui.status = error,
            }
        }
    }
    memory_history(app, ui);
    history_dialog(app, ui);
    if let Some(runtime) = &app.assistants.current {
        if let Some(state) = &runtime.autosave_state {
            if state.pending() {
                ui.small("Saving assistant…");
            }
            if let Some(error) = &state.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        }
        if let Some(settings) = &runtime.snapshot().autosave {
            if settings.saved_at.is_empty() {
                ui.small("Assistant autosave enabled; no automatic save yet");
            } else {
                ui.small(format!("Assistant saved {}", settings.saved_at));
            }
        }
    }
    if let Some(choice) = choice {
        let (id, p) = choice.map_or(("console.assistant.detach", json!({})), |name| ("console.assistant.load", json!({"name":name})));
        if let Err(error) = crate::menus::invoke(app, ui.ctx(), id, p) {
            app.ui.status = error;
        }
    }
    if open {
        app.assistants.save_dialog = true;
        app.assistants.name = selected.clone().unwrap_or_default();
    }
    teaching_review(app, ui);
    if app.assistants.current.as_ref().is_some_and(|runtime| runtime.recovered_previous) {
        ui.colored_label(ui.visuals().warn_fg_color, "Recovered a previous assistant checkpoint. Review recent assistant notes before continuing.");
    }
    if let Some(error) = &app.assistants.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if app.assistants.save_dialog {
        let mut visible = true;
        let mut save = false;
        let mut cancel = false;
        egui::Window::new(if selected.is_some() { "Update Assistant" } else { "Save Assistant" })
            .id(egui::Id::unique("save-project-assistant"))
            .open(&mut visible)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label("Assistant name");
                let name = ui.add(egui::TextEdit::singleline(&mut app.assistants.name).char_limit(80));
                app.auto.add("console.assistant.name", name.rect, "Assistant name");
                ui.label(
                    "Preserves saved teaching and adds new learning for review. Missing instructions are not deleted. Project history stays with this project.",
                );
                let exists = app.assistants.library.as_ref().is_some_and(|l| l.assistants.iter().any(|a| a.name == app.assistants.name.trim()));
                let button = ui.button(if exists { "Update Assistant" } else { "Save Assistant" });
                app.auto.add("console.assistant.confirmSave", button.rect, "Save named assistant");
                save = button.clicked();
                cancel = ui.button("Cancel").clicked();
            });
        if save {
            let name = app.assistants.name.trim().to_owned();
            let existing = app.assistants.library.as_ref().and_then(|l| l.assistants.iter().find(|a| a.name == name)).cloned();
            let mut p = json!({"name":name});
            if let Some(existing) = existing {
                p["replace"] = json!(true);
                p["expectedInstructions"] = json!(existing.instructions);
                p["expectedReferenceDocuments"] = json!(existing.reference_documents);
            }
            match crate::menus::invoke(app, ui.ctx(), "console.assistant.save", p) {
                Ok(_) => app.assistants.save_dialog = false,
                Err(error) => app.ui.status = error,
            }
        }
        if !visible || cancel {
            app.assistants.save_dialog = false;
        }
    }
}

fn history_dialog(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    if !app.assistants.history_dialog {
        return;
    }
    let Some(snapshot) = app.assistants.current.as_ref().map(|runtime| runtime.snapshot().clone()) else { return };
    if snapshot.assistant_name != app.assistants.history_name {
        app.assistants.history_dialog = false;
        app.assistants.history_choice = None;
        return;
    }
    let name = snapshot.assistant_name.clone().unwrap_or_default();
    let revisions = app.assistants.history_revisions.clone();
    let choice = app.assistants.history_choice.clone();
    let expected = app.assistants.library.as_ref().and_then(|library| library.instructions(&name).ok());
    let mut open = true;
    let mut preview = None;
    let mut restore = false;
    egui::Window::new("Assistant versions").id(egui::Id::unique("assistant-versions")).open(&mut open).vscroll(true).show(ui.ctx(), |ui| {
        ui.label("The latest 20 previous catalog versions are retained. Restoring changes only this named assistant; project edits are never replayed.");
        if revisions.is_empty() {
            ui.label("No earlier saved versions yet.");
        }
        for revision in revisions.iter().rev() {
            let response = ui.selectable_label(choice.as_ref().is_some_and(|(id, _)| *id == revision.id), format!("{} · {}", revision.id, revision.saved_at));
            app.auto.add(&format!("console.assistant.history.preview.{}", revision.id), response.rect, "Preview earlier assistant teaching");
            if response.clicked() {
                preview = Some(revision.id);
            }
        }
        if let Some((_, instructions)) = &choice {
            ui.separator();
            ui.label(format!("Role: {}", instructions.role));
            ui.label(format!("Goal: {}", instructions.goal));
            for entry in super::preservation::entries(instructions) {
                if matches!(entry.section, super::preservation::Section::Preference | super::preservation::Section::Workflow) {
                    ui.label(format!("{:?}: {}", entry.section, entry.text));
                }
            }
            ui.label("Restore replaces this assistant's current and unsaved teaching with the reviewed version, including its prior deletions and reference selection.");
            let response = ui.add_enabled(expected.is_some(), egui::Button::new("Restore this version"));
            app.auto.add("console.assistant.history.restore", response.rect, "Explicitly restore the reviewed assistant version");
            restore = response.clicked();
        }
    });
    app.assistants.history_dialog = open;
    if let Some(revision) = preview {
        match crate::menus::invoke(app, ui.ctx(), "console.assistant.history", json!({"name":name,"revision":revision})) {
            Ok(value) => match serde_json::from_value(value.get("instructions").cloned().unwrap_or(Value::Null)) {
                Ok(instructions) => app.assistants.history_choice = Some((revision, instructions)),
                Err(error) => app.ui.status = format!("Cannot read assistant version: {error}"),
            },
            Err(error) => app.ui.status = error,
        }
    }
    if restore && let Some((revision, _)) = choice {
        let params = json!({"name":name,"revision":revision,"expectedInstructions":expected,"confirm":true,"scope":snapshot.project_scope,
            "documentEpoch":snapshot.document_epoch,"memoryGeneration":snapshot.context_version});
        match crate::menus::invoke(app, ui.ctx(), "console.assistant.restore", params) {
            Ok(_) => app.assistants.history_dialog = false,
            Err(error) => app.ui.status = error,
        }
    }
}

fn teaching_review(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let Some(snapshot) = app.assistants.current.as_ref().map(|runtime| runtime.snapshot().clone()) else { return };
    let mut review = None;
    if !snapshot.assistant.proposed.is_empty() {
        egui::CollapsingHeader::new(format!("Review new teaching ({})", snapshot.assistant.proposed.len())).show(ui, |ui| {
            ui.label("Earlier teaching remains active. Review new or conflicting instructions before using them.");
            for (index, entry) in snapshot.assistant.proposed.iter().enumerate() {
                ui.group(|ui| {
                    ui.label(format!("{:?}: {}", entry.section, entry.text));
                    ui.horizontal(|ui| {
                        let accept = ui.button("Accept");
                        app.auto.add(&format!("console.assistant.teaching.accept.{index}"), accept.rect, "Accept proposed teaching");
                        if accept.clicked() {
                            review = Some((entry.clone(), "accept"));
                        }
                        let reject = ui.button("Reject");
                        app.auto.add(&format!("console.assistant.teaching.reject.{index}"), reject.rect, "Reject proposed teaching");
                        if reject.clicked() {
                            review = Some((entry.clone(), "reject"));
                        }
                    });
                });
            }
        });
    }
    egui::CollapsingHeader::new("Saved teaching · review or forget").show(ui, |ui| {
        for (index, entry) in super::preservation::entries(&snapshot.assistant).enumerate() {
            ui.group(|ui| {
                ui.label(format!("{:?}: {}", entry.section, entry.text));
                let forget = ui.button("Forget this instruction");
                app.auto.add(&format!("console.assistant.teaching.forget.{index}"), forget.rect, "Explicitly forget this saved teaching");
                if forget.clicked() {
                    review = Some((entry, "forget"));
                }
            });
        }
    });
    if let Some((teaching, action)) = review {
        let params = json!({"scope":snapshot.project_scope,"documentEpoch":snapshot.document_epoch,"memoryGeneration":snapshot.context_version,
            "teaching":teaching,"action":action});
        if let Err(error) = crate::menus::invoke(app, ui.ctx(), "console.assistant.review", params) {
            app.ui.status = error;
        }
    }
}

pub(super) fn preferences_button(app: &mut FilmcraftApp, ui: &mut egui::Ui) -> bool {
    let selected = app.assistants.creative_preferences;
    let response = super::super::layout::icon_button(
        app,
        ui,
        "console.assistant.creativePreferences",
        crate::icons::Icon::Sparkle,
        true,
        selected,
        "Creative preferences",
    );
    if response.clicked() {
        app.assistants.creative_preferences = !app.assistants.creative_preferences;
    }
    app.assistants.creative_preferences
}

fn memory_button(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let label = tl!("View/Edit Memory");
    let enabled = app.ui.console.sessions.project.is_some();
    let response = super::super::layout::icon_button(app, ui, "console.memory.open", crate::icons::Icon::Notebook, enabled, false, label);
    if response.clicked()
        && let Err(error) = crate::menus::invoke(app, ui.ctx(), "console.memory.open", json!({}))
    {
        app.ui.status = error;
    }
}

fn memory_history(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let Some(snapshot) = app.assistants.current.as_ref().map(|runtime| runtime.snapshot().clone()) else { return };
    ui.collapsing("Working-memory originals", |ui| {
        ui.small("When working notes approach capacity, the connected assistant can summarize them. Requested notes go to your selected AI provider; no separate service is used. Standing teaching and unfinished tasks are preserved.");
        ui.small("Keep the latest 10 originals. Restoring changes working notes only; current teaching and unfinished tasks stay intact. No project edits are replayed.");
        let refresh = ui.button("Refresh originals");
        app.auto.add("console.assistant.memoryHistory", refresh.rect, "List retained working-memory originals");
        if refresh.clicked() {
            match crate::menus::invoke(app, ui.ctx(), "console.assistant.memoryHistory", json!({})) {
                Ok(value) => match serde_json::from_value(value.get("originals").cloned().unwrap_or(Value::Null)) {
                    Ok(entries) => app.assistants.memory_originals = entries,
                    Err(error) => app.ui.status = format!("Cannot read retained originals: {error}"),
                },
                Err(error) => app.ui.status = error,
            }
        }
        let mut restore = None;
        for entry in &app.assistants.memory_originals {
            ui.horizontal(|ui| {
                ui.small(format!("{} · {} bytes", entry.saved_at, entry.original_bytes));
                let button = ui.button("Restore working notes");
                app.auto.add(&format!("console.assistant.memoryRestore.{}", entry.id), button.rect, "Restore these original working notes");
                if button.clicked() { restore = Some(entry.id); }
            });
        }
        if let Some(id) = restore
            && let Err(error) = crate::menus::invoke(app, ui.ctx(), "console.assistant.memoryRestore", json!({"original":id,"confirm":true,"scope":snapshot.project_scope,"documentEpoch":snapshot.document_epoch,"memoryGeneration":snapshot.context_version}))
        { app.ui.status = error; }
        if let Some(status) = app.assistants.current.as_ref().and_then(|runtime| runtime.compaction_status.as_ref()) {
            ui.small(status);
        }
    });
}
