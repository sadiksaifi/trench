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
        let mut col_widths: Vec<usize> = self.headers.iter().map(|h| h.len()).collect();

        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < col_count {
                    col_widths[i] = col_widths[i].max(cell.len());
                }
            }
        }

        let gap = 2; // spaces between columns
        let mut out = String::new();

        // Header
        for (i, header) in self.headers.iter().enumerate() {
            if i > 0 {
                out.push_str(&" ".repeat(gap));
            }
            if i < col_count - 1 {
                out.push_str(&format!("{:<width$}", header, width = col_widths[i]));
            } else {
                out.push_str(header);
            }
        }
        out.push('\n');

        // Rows
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i >= col_count {
                    break;
                }
                if i > 0 {
                    out.push_str(&" ".repeat(gap));
                }
                if i < col_count - 1 {
                    out.push_str(&format!("{:<width$}", cell, width = col_widths[i]));
                } else {
                    out.push_str(cell);
                }
            }
            out.push('\n');
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
