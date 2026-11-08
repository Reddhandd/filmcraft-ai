//! Console-only presentation helpers; actions still use the existing command dispatcher.
use super::*;
use crate::icons::{self, Icon};

pub(super) fn icon_button(app: &mut FilmcraftApp, ui: &mut egui::Ui, id: &str, icon: Icon, enabled: bool, selected: bool, label: &str) -> egui::Response {
    let response = ui.push_id(id, |ui| ui.add_enabled(enabled, egui::Button::new("").selected(selected).min_size(egui::vec2(34.0, 34.0)))).inner;
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, response.enabled(), label));
    let color = if !response.enabled() {
        ui.visuals().weak_text_color()
    } else if selected {
        app.tokens.icon_active
    } else {
        ui.visuals().text_color()
    };
    icons::paint(ui.painter(), response.rect.shrink(8.0), icon, color);
    app.auto.add(id, response.rect, label);
    if response.has_focus() {
        // Use one tooltip path: hover handling must not close the keyboard-focus label.
        egui::Tooltip::for_widget(&response).show(|ui| {
            ui.label(label);
        });
        response
    } else {
        response.on_hover_text(label).on_disabled_hover_text(label)
    }
}

pub(super) fn undo(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    if icon_button(app, ui, "console.undo", Icon::Undo, true, false, tl!("Undo last FilmCraft edit")).clicked()
        && let Err(error) = execute(app, ui.ctx(), "edit.undo")
    {
        app.ui.status = error;
    }
}

pub(super) fn settings(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let selected = app.ui.console.settings_open;
    if icon_button(app, ui, "console.settings.toggle", Icon::Adjust, true, selected, tl!("Model, speaking voice and microphone settings")).clicked()
        && let Err(error) = commands::invoke(app, ui.ctx(), "console.settings.toggle", json!({}))
    {
        app.ui.status = error;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_actions_keep_accessible_names_and_show_words_on_keyboard_focus() {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let project = app.session.project.clone();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        app.auto.begin_frame();
        let mut first = ctx.run_ui(egui::RawInput::default(), |ui| {
            let response = icon_button(&mut app, ui, "test.assistant.save", Icon::Save, true, false, "Save Assistant");
            response.request_focus();
        });
        first.textures_delta.clear();
        let tree = first.platform_output.accesskit_update.as_ref().unwrap();
        assert!(tree.nodes.iter().any(|(_, node)| node.role() == egui::accesskit::Role::Button && node.label() == Some("Save Assistant")));
        fn contains_label(shape: &egui::Shape) -> bool {
            match shape {
                egui::Shape::Text(text) => text.galley.text().contains("Save Assistant"),
                egui::Shape::Vec(shapes) => shapes.iter().any(contains_label),
                _ => false,
            }
        }
        let mut shown = false;
        for _ in 0..4 {
            let mut focused = ctx.run_ui(egui::RawInput::default(), |ui| {
                let response = icon_button(&mut app, ui, "test.assistant.save", Icon::Save, true, false, "Save Assistant");
                assert!(response.has_focus(), "The icon must retain keyboard focus");
            });
            focused.textures_delta.clear();
            shown |= focused.shapes.iter().any(|shape| contains_label(&shape.shape));
        }
        assert!(shown, "Keyboard focus must paint the function name after layout settles");
        assert!(app.auto.find("test.assistant.save").is_some());
        assert!(std::sync::Arc::ptr_eq(&project, &app.session.project));
    }

    #[test]
    fn settings_toggle_preserves_live_voice_draft_and_project() {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.ui.console.chat.prompt = "Keep my unsent instructions".into();
        app.ui.console.chat.busy = true;
        app.ui.console.chat.voice = VoiceStatus::Active;
        let project = app.session.project.clone();
        let stamp = (app.session.revision, app.session.document_epoch);
        let ctx = egui::Context::default();
        for expected in [true, false] {
            commands::invoke(&mut app, &ctx, "console.settings.toggle", json!({})).unwrap();
            assert_eq!(app.ui.console.settings_open, expected);
            assert_eq!(app.ui.console.chat.prompt, "Keep my unsent instructions");
            assert!(app.ui.console.chat.busy);
            assert_eq!(app.ui.console.chat.voice, VoiceStatus::Active);
            assert!(std::sync::Arc::ptr_eq(&project, &app.session.project));
            assert_eq!((app.session.revision, app.session.document_epoch), stamp);
        }
        assert!(commands::invoke(&mut app, &ctx, "console.settings.toggle", json!({"unexpected":true})).is_err());
        assert!(!app.ui.console.settings_open);
    }
}
