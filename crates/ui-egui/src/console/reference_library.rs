//! Persistent private reference library; no document is executable assistant memory.
#[cfg(not(target_arch = "wasm32"))]
mod disk_index;
#[cfg(not(target_arch = "wasm32"))]
mod epub;
#[cfg(not(target_arch = "wasm32"))]
mod job;
mod model;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
mod ocr_models;
#[cfg(not(target_arch = "wasm32"))]
mod recovery;
#[cfg(not(target_arch = "wasm32"))]
mod store;
use crate::FilmcraftApp;
use serde_json::{Map, Value};
#[derive(Clone, Debug, Default)]
pub struct State {
    #[cfg(not(target_arch = "wasm32"))]
    runtime: std::rc::Rc<std::cell::RefCell<native::Runtime>>,
    open: bool,
    query: String,
}
pub(super) fn invoke(app: &mut FilmcraftApp, id: &str, p: &Map<String, Value>) -> Result<Value, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        native::invoke(app, id, p)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (app, id, p);
        Err("Persistent reference libraries require the desktop build".into())
    }
}
pub(super) fn button(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    #[cfg(not(target_arch = "wasm32"))]
    native::button(app, ui);
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (app, ui);
    }
}

/// Native maintenance entry point. Only explicitly user-selected paths may be imported.
/// Run off the UI thread; models use the narrower Console retrieval commands instead.
#[cfg(not(target_arch = "wasm32"))]
pub fn local_operation(root: &std::path::Path, action: &str, params: &Value) -> Result<Value, String> {
    store::perform(root, action, params)
}

/// Open the same persistent library without opening or connecting Command Console.
pub(crate) fn open(app: &mut FilmcraftApp, ctx: &egui::Context) -> Result<Value, String> {
    app.ui.console.reference_library.open = true;
    crate::console::invoke_session(app, ctx, "console.library.inspect", serde_json::json!({}))
}
pub(super) fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    native::show(app, ctx);
    #[cfg(target_arch = "wasm32")]
    let _ = (app, ctx);
}
