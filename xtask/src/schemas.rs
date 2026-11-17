//! The schema gate: every JSON Schema file under `schemas/` must parse and identify itself.
//! Whether a schema matches the implementation is checked by the tests of the owning crate.

/// Problems of one schema file's text; empty when it parses and has `$schema` and `$id`.
pub fn check(file: &str, text: &str) -> Vec<String> {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(e) => return vec![format!("{file}: not valid JSON: {e}")],
    };
    ["$schema", "$id"]
        .into_iter()
        .filter(|key| value.get(key).is_none())
        .map(|key| format!("{file}: missing the `{key}` member"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identified_schema_passes() {
        assert!(
            check(
                "a.json",
                r#"{"$schema": "s", "$id": "i", "type": "object"}"#
            )
            .is_empty()
        );
    }

    #[test]
    fn invalid_json_and_missing_members_fail() {
        assert_eq!(check("a.json", "{").len(), 1);
        assert_eq!(
            check("a.json", r#"{"$id": "i"}"#),
            vec!["a.json: missing the `$schema` member"]
        );
        assert_eq!(check("a.json", "[]").len(), 2);
    }
}
