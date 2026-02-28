use anyhow::Result;
use serde::Serialize;

/// Serialize a slice of items as a pretty-printed JSON array.
///
/// This is the canonical way to produce `--json` output across all trench
/// commands. The caller is responsible for constructing the concrete
/// `Serialize`-able type; this function handles formatting only.
pub fn format_json<T: Serialize>(items: &[T]) -> Result<String> {
    Ok(serde_json::to_string_pretty(items)?)
}

/// Serialize a single item as a pretty-printed JSON object.
///
/// Used by commands that output a single resource (e.g. `trench create --json`).
pub fn format_json_value<T: Serialize>(item: &T) -> Result<String> {
    Ok(serde_json::to_string_pretty(item)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Dummy {
        name: String,
        count: u32,
    }

    #[test]
    fn format_json_serializes_array() {
        let items = vec![
            Dummy { name: "alpha".into(), count: 1 },
            Dummy { name: "beta".into(), count: 2 },
        ];

        let output = format_json(&items).unwrap();
        let parsed: Vec<Dummy> = serde_json::from_str(&output).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "alpha");
        assert_eq!(parsed[1].count, 2);
    }

    #[test]
    fn format_json_empty_array() {
        let items: Vec<Dummy> = vec![];
        let output = format_json(&items).unwrap();
        assert_eq!(output, "[]");
    }

    #[test]
    fn format_json_value_serializes_single_object() {
        let item = Dummy { name: "solo".into(), count: 42 };
        let output = format_json_value(&item).unwrap();
        let parsed: Dummy = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed, item);
    }

    #[test]
    fn format_json_output_is_pretty_printed() {
        let items = vec![Dummy { name: "x".into(), count: 0 }];
        let output = format_json(&items).unwrap();
        // Pretty-printed JSON has newlines and indentation
        assert!(output.contains('\n'), "output should be pretty-printed");
        assert!(output.contains("  "), "output should have indentation");
    }

    #[test]
    fn format_json_contains_no_ansi_codes() {
        let items = vec![Dummy { name: "test".into(), count: 1 }];
        let output = format_json(&items).unwrap();
        assert!(!output.contains('\x1b'), "JSON output must not contain ANSI escape codes");
    }
}
