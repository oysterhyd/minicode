use anyhow::{Result, bail};
use serde_json::Value;
pub(crate) fn reference_path(reference: &str) -> &str {
    static LOCATION: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let pattern = LOCATION.get_or_init(|| {
        regex::Regex::new(r":[1-9][0-9]*(?::[1-9][0-9]*)?(?:-[1-9][0-9]*)?$").unwrap()
    });
    pattern
        .find(reference)
        .map(|location| &reference[..location.start()])
        .unwrap_or(reference)
}

pub(crate) fn validate(value: &Value) -> Result<()> {
    if !value["summary"].is_string() {
        bail!("summary must be a string");
    }
    for key in ["findings", "evidence_refs", "unresolved"] {
        if !value[key]
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string))
        {
            bail!("{key} must be an array of strings");
        }
    }
    Ok(())
}
pub(crate) fn extract(text: &str) -> Option<Value> {
    let text = text.trim().trim_start_matches('\u{feff}').trim();
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return validate(&value).is_ok().then_some(value);
    }
    let mut cursor = 0;
    for _ in 0..128 {
        let start = cursor + text.get(cursor..)?.find(['{', '['])?;
        let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
        if let Some(Ok(value)) = stream.next() {
            if validate(&value).is_ok() {
                return Some(value);
            }
            // Do not extract an apparent result from inside an unrelated
            // object or array that happens to contain the required keys.
            cursor = start + stream.byte_offset().max(1);
        } else {
            cursor = start + 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn r31b_locations_preserve_drive_letters_and_non_location_colons() {
        for (reference, path) in [
            ("src/a.rs:10", "src/a.rs"),
            ("src/a.rs:10:2", "src/a.rs"),
            ("src/a.rs:1-20", "src/a.rs"),
            (r"C:\repo\a.rs:2", r"C:\repo\a.rs"),
            ("name:word", "name:word"),
            ("../outside.rs:3", "../outside.rs"),
        ] {
            assert_eq!(reference_path(reference), path);
        }
    }
    #[test]
    fn r31_extracts_only_complete_valid_structures() {
        let result = json!({"summary":"中文 { braces }","findings":["ok"],"evidence_refs":[],"unresolved":[]});
        for text in [
            result.to_string(),
            format!("```json\n{result}\n```"),
            format!("prefix {result} suffix"),
            format!("{{broken\n{result}"),
            format!("{{\"noise\":true}}\n{result}"),
        ] {
            assert_eq!(extract(&text), Some(result.clone()), "{text}");
        }
        for text in [
            json!({"example":result}).to_string(),
            json!([result]).to_string(),
            "{\"summary\":\"missing arrays\"}".into(),
        ] {
            assert!(extract(&text).is_none());
        }
        let mut invalid = result;
        invalid["findings"] = json!([42]);
        assert!(validate(&invalid).is_err());
        invalid["findings"] = json!([]);
        invalid["summary"] = json!("x".repeat(256 * 1024));
        assert!(validate(&invalid).is_ok());
        assert_eq!(extract(&format!("```json\n{invalid}\n```")), Some(invalid));
    }
}
