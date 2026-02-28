/// Trait for types that can be rendered as porcelain (colon-separated) output.
///
/// Implement this on any struct that should support `--porcelain` output.
/// Each implementor defines its own field ordering.
pub trait PorcelainRecord {
    /// Return the ordered field values for this record.
    fn porcelain_fields(&self) -> Vec<String>;
}

/// Format a slice of porcelain records as newline-delimited, colon-separated lines.
///
/// This is the canonical way to produce `--porcelain` output across all trench
/// commands.
///
/// # Limitations
///
/// Fields are joined with `:` and records are separated by `\n`. If a field
/// value contains either character the output becomes ambiguous. Consumers
/// should parse left-to-right using the known field count for each record
/// type rather than splitting blindly on `:`.
pub fn format_porcelain(items: &[impl PorcelainRecord]) -> String {
    let mut out = String::new();
    for item in items {
        let line = item.porcelain_fields().join(":");
        out.push_str(&line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRecord {
        name: String,
        branch: String,
        managed: bool,
    }

    impl PorcelainRecord for TestRecord {
        fn porcelain_fields(&self) -> Vec<String> {
            vec![
                self.name.clone(),
                self.branch.clone(),
                self.managed.to_string(),
            ]
        }
    }

    #[test]
    fn format_porcelain_produces_colon_separated_lines() {
        let items = vec![
            TestRecord { name: "alpha".into(), branch: "feature/alpha".into(), managed: true },
            TestRecord { name: "beta".into(), branch: "fix/beta".into(), managed: false },
        ];

        let output = format_porcelain(&items);
        let lines: Vec<&str> = output.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "alpha:feature/alpha:true");
        assert_eq!(lines[1], "beta:fix/beta:false");
    }

    #[test]
    fn format_porcelain_empty_list() {
        let items: Vec<TestRecord> = vec![];
        let output = format_porcelain(&items);
        assert!(output.is_empty());
    }

    #[test]
    fn format_porcelain_single_record() {
        let items = vec![
            TestRecord { name: "solo".into(), branch: "main".into(), managed: true },
        ];

        let output = format_porcelain(&items);
        assert_eq!(output, "solo:main:true\n");
    }

    #[test]
    fn format_porcelain_ends_each_line_with_newline() {
        let items = vec![
            TestRecord { name: "a".into(), branch: "b".into(), managed: true },
        ];

        let output = format_porcelain(&items);
        assert!(output.ends_with('\n'), "each record line must end with newline");
    }

    #[test]
    fn format_porcelain_contains_no_ansi_codes() {
        let items = vec![
            TestRecord { name: "test".into(), branch: "dev".into(), managed: false },
        ];

        let output = format_porcelain(&items);
        assert!(!output.contains('\x1b'), "porcelain output must not contain ANSI escape codes");
    }
}
