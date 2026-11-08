//! Headless UI tests of box (marquee) selection in the Timeline: the selection follows the box
//! while it is dragged, Shift adds to the selection the drag began with, and Escape cancels.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_project::ClipId;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    modifiers: egui::Modifiers,
}

impl Driver {
    fn demo() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, modifiers: egui::Modifiers::NONE };
        d.frames(4);
        d
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                assert_eq!(v["ok"], json!(true), "{method} {params} failed: {v}");
                return v["result"].clone();
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }

    fn rect(&mut self, id: &str) -> egui::Rect {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        let e = v.as_array().unwrap().iter().find(|e| e["id"] == id).unwrap_or_else(|| panic!("no element {id}")).clone();
        let r: Vec<f32> = e["rect"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect();
        egui::Rect::from_min_size(egui::pos2(r[0], r[1]), egui::vec2(r[2], r[3]))
    }

    fn send(&mut self, e: egui::Event) {
        let modifiers = self.modifiers;
        self.harness.input_mut().events.extend([egui::Event::ModifiersChanged(modifiers), e]);
        self.frames(1);
    }

    fn button(&mut self, pos: egui::Pos2, pressed: bool) {
        let modifiers = self.modifiers;
        self.send(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers });
    }

    /// Move the pointer to `to` in small steps, a frame each.
    fn move_to(&mut self, from: egui::Pos2, to: egui::Pos2) {
        for k in 1..=8 {
            self.send(egui::Event::PointerMoved(from.lerp(to, k as f32 / 8.0)));
        }
    }

    fn selected(&self, clip: u64) -> bool {
        self.harness.state().session.state.selection.contains(&ClipId(clip))
    }

    /// The last two clips of Video 1, and a point on Video 1 just past the end of the sequence.
    fn setup(&mut self) -> (u64, u64, egui::Rect, egui::Rect, egui::Pos2) {
        self.ok("ui.set", json!({"focused": "Timeline"}));
        let seq = self.exec("sequence.inspect", json!({}));
        let items = seq["video"][0]["items"].as_array().unwrap().clone();
        let id = |i: &Value| i["clip"].as_u64().unwrap();
        let (a, b) = (id(&items[items.len() - 2]), id(&items[items.len() - 1]));
        let (ra, rb) = (self.rect(&format!("timeline.clip.{a}")), self.rect(&format!("timeline.clip.{b}")));
        let start = egui::pos2(rb.max.x + 12.0, rb.center().y);
        assert_eq!(self.ok("ui.timeline.hit", json!({"x": start.x, "y": start.y}))["hit"], "empty", "the drag starts on empty space");
        (a, b, ra, rb, start)
    }
}

#[test]
fn the_selection_follows_the_box_while_it_is_dragged() {
    let mut d = Driver::demo();
    let (a, b, ra, rb, start) = d.setup();
    d.exec("timeline.select", json!({"clips": []}));
    d.send(egui::Event::PointerMoved(start));
    d.button(start, true);
    // into the last clip: selected before the button comes up
    let over_b = egui::pos2(rb.center().x, start.y + 2.0);
    d.move_to(start, over_b);
    assert!(d.selected(b) && !d.selected(a), "the box selects what it touches as it grows");
    // on into the clip before it
    let over_a = egui::pos2(ra.center().x, start.y + 2.0);
    d.move_to(over_b, over_a);
    assert!(d.selected(a) && d.selected(b));
    // and back: the clip that left the box is no longer selected
    d.move_to(over_a, over_b);
    assert!(d.selected(b) && !d.selected(a), "a clip leaving the box is deselected");
    d.button(over_b, false);
    d.frames(2);
    assert!(d.selected(b) && !d.selected(a), "the release keeps what the box showed");
}

#[test]
fn alt_box_selects_only_video_and_delete_keeps_linked_audio() {
    let mut d = Driver::demo();
    let (_, video, _, rv, start) = d.setup();
    let audio = {
        let seq = d.harness.state().session.active_sequence().unwrap();
        let link = seq.find_item(ClipId(video)).unwrap().1.link;
        seq.audio_tracks.iter().flat_map(|tr| &tr.items).find(|clip| clip.link == link).unwrap().id.0
    };
    d.exec("timeline.select", json!({"clips": []}));
    d.modifiers = egui::Modifiers::ALT;
    d.send(egui::Event::PointerMoved(start));
    d.button(start, true);
    let over = egui::pos2(rv.center().x, start.y + 2.0);
    d.move_to(start, over);
    assert!(d.selected(video));
    assert!(!d.selected(audio), "Alt excludes the linked audio during the drag");
    d.button(over, false);
    d.modifiers = egui::Modifiers::NONE;
    d.frames(2);
    assert!(d.selected(video) && !d.selected(audio), "release preserves the individual selection");
    d.exec("edit.clear", json!({}));
    let seq = d.harness.state().session.active_sequence().unwrap();
    assert!(seq.find_item(ClipId(video)).is_none());
    assert!(seq.find_item(ClipId(audio)).is_some(), "Delete leaves the unselected linked audio");
}

#[test]
fn shift_adds_to_the_selection_the_drag_began_with() {
    let mut d = Driver::demo();
    let (a, b, _, rb, start) = d.setup();
    d.exec("timeline.select", json!({"clips": [a]}));
    d.modifiers = egui::Modifiers::SHIFT;
    d.send(egui::Event::PointerMoved(start));
    d.button(start, true);
    let over_b = egui::pos2(rb.center().x, start.y + 2.0);
    d.move_to(start, over_b);
    assert!(d.selected(a) && d.selected(b), "Shift keeps the earlier selection and adds the box");
    // shrinking the box to nothing leaves the selection the drag began with
    d.move_to(over_b, start + egui::vec2(-2.0, 2.0));
    assert!(d.selected(a) && !d.selected(b));
    d.move_to(start + egui::vec2(-2.0, 2.0), over_b);
    d.button(over_b, false);
    d.modifiers = egui::Modifiers::NONE;
    d.frames(2);
    assert!(d.selected(a) && d.selected(b));
}

#[test]
fn escape_cancels_the_box_and_restores_the_selection() {
    let mut d = Driver::demo();
    let (a, b, ra, _, start) = d.setup();
    d.exec("timeline.select", json!({"clips": [a]}));
    d.send(egui::Event::PointerMoved(start));
    d.button(start, true);
    let over_a = egui::pos2(ra.center().x, start.y + 2.0);
    d.move_to(start, over_a);
    assert!(d.selected(b));
    d.send(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    d.button(over_a, false);
    d.frames(2);
    assert!(d.selected(a) && !d.selected(b), "Escape put back the selection the drag began with");
}

#[test]
fn marquee_tool_starts_over_clip_bodies_and_edges_without_editing() {
    for edge in [false, true] {
        for modifiers in [egui::Modifiers::NONE, egui::Modifiers::SHIFT, egui::Modifiers::COMMAND, egui::Modifiers::ALT] {
            let mut d = Driver::demo();
            let (a, b, ra, rb, _) = d.setup();
            d.exec("tool.marquee", json!({}));
            d.exec("timeline.select", json!({"clips": [a]}));
            let before = serde_json::to_value(&d.harness.state().session.project).unwrap();
            let start = if edge { egui::pos2(rb.max.x - 2.0, rb.center().y) } else { rb.center() };
            let end = egui::pos2(ra.center().x, start.y + 4.0);
            d.modifiers = modifiers;
            d.send(egui::Event::PointerMoved(start));
            d.button(start, true);
            d.move_to(start, end);
            assert!(d.selected(a) && d.selected(b));
            d.button(end, false);
            d.frames(2);
            assert_eq!(serde_json::to_value(&d.harness.state().session.project).unwrap(), before, "selection never moves, trims, or edits clips");
            assert!(d.selected(a) && d.selected(b));
        }
    }
}

#[test]
fn m_adds_a_marker_in_timeline_and_modified_chord_selects_marquee() {
    let mut d = Driver::demo();
    d.setup();
    let export = d.harness.state().session.shortcuts.resolve(
        &filmcraft_engine::shortcuts::Chord::parse("Cmd+M").unwrap(),
        Some("Timeline"),
        filmcraft_engine::shortcuts::Platform::current(),
    );
    assert_eq!(export.map(|binding| binding.command.as_str()), Some("mode.export"));
    let markers_before = d.harness.state().session.active_sequence().unwrap().markers.len();
    for pressed in [true, false] {
        d.send(egui::Event::Key { key: egui::Key::M, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
    }
    assert_eq!(d.harness.state().ui.tool, filmcraft_ui_egui::state::Tool::Selection);
    assert_eq!(d.harness.state().session.active_sequence().unwrap().markers.len(), markers_before + 1);

    let marquee_chord = if cfg!(target_os = "macos") { "Cmd+Ctrl+M" } else { "Shift+V" };
    d.ok("ui.key", json!({"key": marquee_chord}));
    d.frames(2);
    assert_eq!(d.harness.state().ui.tool, filmcraft_ui_egui::state::Tool::Marquee);
    assert_eq!(d.harness.state().session.active_sequence().unwrap().markers.len(), markers_before + 1, "Marquee must not add a marker");
}

#[test]
fn recorder_dialog_exposes_capture_controls_and_rejects_hostile_windows() {
    let mut d = Driver::demo();
    let dir = tempfile::tempdir().unwrap();
    d.harness.state_mut().session.prefs_path = Some(dir.path().join("preferences.json"));
    let before = d.harness.state().session.project.clone();
    assert_eq!(d.exec("diagnostics.toggle", json!({}))["enabled"], true);
    d.exec("diagnostics.capture", json!({}));
    d.frames(3);
    d.rect("diagnostics.capture.30");
    let report = d.exec("diagnostics.capture", json!({"seconds":30}));
    assert!(std::path::Path::new(report["path"].as_str().unwrap()).exists());
    let ctx = d.harness.ctx.clone();
    for seconds in [json!(-1), json!(0), json!(601), json!("60"), json!(null)] {
        assert!(filmcraft_ui_egui::menus::invoke(d.harness.state_mut(), &ctx, "diagnostics.capture", json!({"seconds":seconds})).is_err());
    }
    // Capture changes the notice text and therefore the window's measured size.
    // Let the registry settle before resolving the synthetic click coordinates.
    d.frames(3);
    d.ok("ui.click", json!({"id":"diagnostics.close"}));
    d.frames(2);
    assert!(!d.harness.state().flight_recorder.window_open);
    assert_eq!(d.exec("diagnostics.toggle", json!({}))["enabled"], false);
    assert_eq!(*d.harness.state().session.project, *before);
}
