//! Local maintenance/qualification tool; not an MCP arbitrary-filesystem capability.
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.as_slice() == ["--reference-pdf-worker"] {
        return filmcraft_ui_egui::pdf_reference::serve_stdio();
    }
    let root = args.first().ok_or("Usage: reference_library ROOT ACTION JSON_PARAMS")?;
    let action = args.get(1).ok_or("Supply inspect/import/search/remove/enable")?;
    let params: serde_json::Value = serde_json::from_str(args.get(2).ok_or("Supply JSON parameters")?).map_err(|e| e.to_string())?;
    let value = filmcraft_ui_egui::console::reference_library::local_operation(std::path::Path::new(root), action, &params)?;
    println!("{}", serde_json::to_string(&value).map_err(|e| e.to_string())?);
    Ok(())
}
