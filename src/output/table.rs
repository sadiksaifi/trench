/// A reusable table formatter that auto-sizes columns.
///
/// Not coupled to any specific data type — accepts string headers and rows.
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    max_width: Option<usize>,
}

impl Table {
    pub fn new(headers: Vec<&str>) -> Self {
        Self {
            headers: headers.into_iter().map(String::from).collect(),
            rows: Vec::new(),
            max_width: None,
        }
    }

    pub fn row(mut self, cells: Vec<&str>) -> Self {
        self.rows.push(cells.into_iter().map(String::from).collect());
        self
    }

    pub fn max_width(mut self, width: usize) -> Self {
        self.max_width = Some(width);
        self
    }

    pub fn render(&self) -> String {
        if self.rows.is_empty() {
            return String::new();
        }

        let col_count = self.headers.len();
        let gap = 2usize;
        let mut col_widths: Vec<usize> = self.headers.iter().map(|h| h.len()).collect();

        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < col_count {
                    col_widths[i] = col_widths[i].max(cell.len());
                }
            }
        }

        // Shrink columns to fit max_width if set
        if let Some(max) = self.max_width {
            let total_gap = gap * col_count.saturating_sub(1);
            let available = max.saturating_sub(total_gap);
            let total_content: usize = col_widths.iter().sum();

            if total_content > available && available > 0 {
                // Shrink widest columns first until we fit
                while col_widths.iter().sum::<usize>() > available {
                    let max_idx = col_widths
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, w)| *w)
                        .map(|(i, _)| i)
                        .unwrap();
                    if col_widths[max_idx] <= 1 {
                        break;
                    }
                    col_widths[max_idx] -= 1;
                }
            }
        }

        let mut out = String::new();

        // Render a single line, truncating cells to column widths
        let render_line = |out: &mut String, cells: &[String], widths: &[usize]| {
            for (i, cell) in cells.iter().enumerate() {
                if i >= col_count {
                    break;
                }
                if i > 0 {
                    out.push_str(&" ".repeat(gap));
                }
                let w = widths[i];
                let truncated: String = if cell.len() > w {
                    if w > 1 {
                        let mut s: String = cell.chars().take(w - 1).collect();
                        s.push('~');
                        s
                    } else {
                        cell.chars().take(w).collect()
                    }
                } else {
                    cell.clone()
                };
                if i < col_count - 1 {
                    out.push_str(&format!("{:<width$}", truncated, width = w));
                } else {
                    out.push_str(&truncated);
                }
            }
            out.push('\n');
        };

        let headers_as_strings: Vec<String> = self.headers.clone();
        render_line(&mut out, &headers_as_strings, &col_widths);

        for row in &self.rows {
            render_line(&mut out, row, &col_widths);
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_rows_returns_empty_string() {
        let output = Table::new(vec!["Name", "Branch"]).render();
        assert!(output.is_empty(), "no rows should produce empty output");
    }

    #[test]
    fn truncates_columns_to_fit_max_width() {
        let output = Table::new(vec!["Name", "Path"])
            .row(vec!["short", "/very/long/path/that/exceeds/width"])
            .max_width(30)
            .render();

        for line in output.lines() {
            assert!(
                line.len() <= 30,
                "line exceeds max_width: len={}, line={:?}",
                line.len(),
                line
            );
        }
        // Content should still be present, just truncated
        assert!(output.contains("Name"), "header should still appear");
        assert!(output.contains("short"), "short values should not be truncated");
    }

    #[test]
    fn renders_headers_and_rows_with_aligned_columns() {
        let output = Table::new(vec!["Name", "Branch", "Path"])
            .row(vec!["foo", "main", "/tmp/foo"])
            .row(vec!["bar-longer", "dev", "/tmp/bar"])
            .render();

        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3, "expected header + 2 data rows");

        // All lines should have the same length (padded)
        let widths: Vec<usize> = lines.iter().map(|l| l.trim_end().len()).collect();
        // Check that columns are aligned by verifying "Name" and "Branch" appear at same column offsets
        let header = lines[0];
        let row1 = lines[1];
        let branch_offset_header = header.find("Branch").expect("header should contain 'Branch'");
        let branch_offset_row1 = row1.find("main").expect("row should contain 'main'");
        assert_eq!(
            branch_offset_header, branch_offset_row1,
            "Branch column should align between header and row"
        );
    }
}
