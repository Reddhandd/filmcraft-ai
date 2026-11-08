//! Encode only the client setup; the live profile owns all short-lived credentials.
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};
pub(super) fn encode(cli: &Path, args: &[String], environment: Option<&BTreeMap<String, String>>) -> Result<Value, String> {
    let cli = cli.to_str().ok_or("Desktop helper path is not valid Unicode")?;
    if cli.len() > 8192 || args.len() > 8 || args.iter().any(|argument| argument.len() > 8192) {
        return Err("Desktop setup exceeds its configuration budget".into());
    }
    let mut settings = json!({"command":cli,"args":args});
    if let Some(environment) = environment {
        settings["env"] = json!(environment);
    }
    Ok(json!({"mcpServers":{"filmcraft":settings}}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn non_unicode_cli_path_is_an_error_not_a_json_macro_panic() {
        use std::os::unix::ffi::OsStringExt;
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(b"synthetic-helper-\xff".to_vec()));
        let result = std::panic::catch_unwind(|| encode(&path, &[], None));
        assert!(result.is_ok(), "Invalid Unicode must be an actionable error");
        assert!(result.unwrap().is_err());
    }
}
