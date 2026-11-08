//! Bounded runtime voice choices for FilmCraft's v3 realtime transport.
use serde_json::Value;

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b == b'-' || b == b'_')
}

/// Catalog defaults for other protocol versions must never become a v3 override.
pub(crate) fn catalog(result: &Value) -> Result<Vec<String>, String> {
    let entries = result.pointer("/voices/v3").and_then(Value::as_array).ok_or("Codex did not supply a v3 speaking-voice catalog; use Default")?;
    if entries.len() > 64 {
        return Err("Codex speaking-voice catalog exceeds 64 choices; use Default".into());
    }
    let mut voices = Vec::with_capacity(entries.len());
    for entry in entries {
        let id = entry.as_str().filter(|id| valid_id(id)).ok_or("Codex returned an invalid speaking-voice identifier; use Default")?;
        if !voices.iter().any(|voice| voice == id) {
            voices.push(id.to_owned());
        }
    }
    Ok(voices)
}

/// Validate before starting the audio peer, then validate again at request construction.
pub(crate) fn validate_choice(voices: &[String], choice: Option<&str>) -> Result<(), String> {
    if let Some(id) = choice
        && (!valid_id(id) || !voices.iter().any(|voice| voice == id))
    {
        return Err("Selected speaking voice is unavailable; choose an available voice or Use Default before starting voice".into());
    }
    Ok(())
}

/// Default means omission, not an inferred account preference or another version's default.
pub(crate) fn apply_choice(params: &mut Value, voices: &[String], choice: Option<&str>) -> Result<(), String> {
    validate_choice(voices, choice)?;
    let object = params.as_object_mut().ok_or("Invalid realtime start parameters")?;
    match choice {
        Some(id) => {
            object.insert("voice".into(), Value::String(id.to_owned()));
        }
        None => {
            object.remove("voice");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn v3_catalog_is_deduplicated_without_inventing_a_default() {
        let voices = catalog(&json!({"voices":{"v1":["cove"],"v2":["marin"],"v3":["sol","cove","sol"],"defaultV1":"cove","defaultV2":"marin"}})).unwrap();
        assert_eq!(voices, ["sol", "cove"]);
        let mut request = json!({"version":"v3","voice":"old"});
        apply_choice(&mut request, &voices, None).unwrap();
        assert!(request.get("voice").is_none());
        apply_choice(&mut request, &voices, Some("sol")).unwrap();
        assert_eq!(request["voice"], "sol");
        assert_eq!(request["version"], "v3");
    }

    #[test]
    fn hostile_catalogs_and_unavailable_choices_are_rejected_without_mutation() {
        for result in [
            json!(null),
            json!({"voices":{"v2":["sol"]}}),
            json!({"voices":{"v3":null}}),
            json!({"voices":{"v3":[1]}}),
            json!({"voices":{"v3":[""]}}),
            json!({"voices":{"v3":["Sol"]}}),
            json!({"voices":{"v3":["sol\n"]}}),
            json!({"voices":{"v3":["é"]}}),
            json!({"voices":{"v3":["x".repeat(65)]}}),
            json!({"voices":{"v3":vec!["sol";65]}}),
        ] {
            assert!(std::panic::catch_unwind(|| catalog(&result)).unwrap().is_err());
        }
        let mut request = json!({"version":"v3"});
        for id in ["marin", "../sol", "", "sol\n", "Sol"] {
            assert!(apply_choice(&mut request, &["sol".into()], Some(id)).is_err());
            assert_eq!(request, json!({"version":"v3"}));
        }
        assert!(apply_choice(&mut Value::Null, &[], None).is_err());
        assert!(validate_choice(&[], None).is_ok());
        assert!(validate_choice(&[], Some("sol")).is_err());
    }
}
