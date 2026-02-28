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
pub fn format_porcelain(items: &[impl PorcelainRecord]) -> String {
    let mut out = String::new();
    for item in items {
        let line = item.porcelain_fields().join(":");
        out.push_str(&line);
        out.push('\n');
    }
    out
}
