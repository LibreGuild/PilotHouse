//! A Markdown pipe table, edited cell by cell: read from the block's text,
//! changed, and written back neatly (columns padded to line up, `|` in a
//! cell escaped, each column's alignment kept).

/// How a column's cells line up (from the `:---:` row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// Row 0 is the header; every row has one cell per column.
    pub rows: Vec<Vec<String>>,
    pub align: Vec<Align>,
}

impl Table {
    /// The table in `text`, or `None` when it is not just a pipe table.
    pub fn parse(text: &str) -> Option<Self> {
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        if lines.len() < 2 {
            return None;
        }
        let align: Vec<Align> = cells(lines[1])?
            .iter()
            .map(|cell| parse_align(cell))
            .collect::<Option<_>>()?;
        let columns = align.len();
        let mut rows = Vec::with_capacity(lines.len() - 1);
        for (ix, line) in lines.iter().enumerate() {
            if ix == 1 {
                continue;
            }
            let mut row = cells(line)?;
            // A short row is filled out; cells past the last column are
            // not shown by Markdown either, so they go.
            row.resize(columns, String::new());
            rows.push(row);
        }
        Some(Self { rows, align })
    }

    pub fn columns(&self) -> usize {
        self.align.len()
    }

    /// The cell at `(row, col)` (row 0: the header).
    pub fn cell(&self, row: usize, col: usize) -> Option<&str> {
        self.rows.get(row)?.get(col).map(String::as_str)
    }

    pub fn set_cell(&mut self, row: usize, col: usize, text: &str) {
        if let Some(cell) = self.rows.get_mut(row).and_then(|row| row.get_mut(col)) {
            // A cell is one line.
            *cell = text.replace(['\n', '\r'], " ").trim().to_string();
        }
    }

    /// An empty row at `at` (after the header at the earliest).
    pub fn insert_row(&mut self, at: usize) {
        let at = at.clamp(1, self.rows.len());
        self.rows.insert(at, vec![String::new(); self.columns()]);
    }

    pub fn insert_column(&mut self, at: usize) {
        let at = at.min(self.columns());
        self.align.insert(at, Align::None);
        for row in &mut self.rows {
            row.insert(at, String::new());
        }
    }

    /// Removes row `row` (not the header, nor the last row under it).
    pub fn remove_row(&mut self, row: usize) -> bool {
        if row == 0 || row >= self.rows.len() || self.rows.len() <= 2 {
            return false;
        }
        self.rows.remove(row);
        true
    }

    /// Removes column `col` (not the last one).
    pub fn remove_column(&mut self, col: usize) -> bool {
        if col >= self.columns() || self.columns() <= 1 {
            return false;
        }
        self.align.remove(col);
        for row in &mut self.rows {
            row.remove(col);
        }
        true
    }

    /// The table as Markdown, its columns padded to line up.
    pub fn to_markdown(&self) -> String {
        let escaped: Vec<Vec<String>> = self
            .rows
            .iter()
            .map(|row| row.iter().map(|cell| cell.replace('|', "\\|")).collect())
            .collect();
        let width = |col: usize| {
            escaped
                .iter()
                .map(|row| row[col].chars().count())
                .max()
                .unwrap_or(0)
                .max(3)
        };
        let widths: Vec<usize> = (0..self.columns()).map(width).collect();
        let line = |row: &[String]| {
            let cells: Vec<String> = row
                .iter()
                .zip(&widths)
                .map(|(cell, width)| {
                    let pad = width - cell.chars().count();
                    format!("{cell}{}", " ".repeat(pad))
                })
                .collect();
            format!("| {} |", cells.join(" | "))
        };
        let rule: Vec<String> = self
            .align
            .iter()
            .zip(&widths)
            .map(|(align, width)| match align {
                Align::None => "-".repeat(*width),
                Align::Left => format!(":{}", "-".repeat(width - 1)),
                Align::Center => format!(":{}:", "-".repeat(width - 2)),
                Align::Right => format!("{}:", "-".repeat(width - 1)),
            })
            .collect();
        let mut out = vec![line(&escaped[0]), format!("| {} |", rule.join(" | "))];
        out.extend(escaped[1..].iter().map(|row| line(row)));
        out.join("\n")
    }
}

/// A row's cells: split at `|` not escaped, the outer pipes optional.
fn cells(line: &str) -> Option<Vec<String>> {
    let line = line.trim();
    if !line.contains('|') {
        return None;
    }
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = match line.strip_suffix('|') {
        Some(rest) if !rest.ends_with('\\') => rest,
        _ => line,
    };
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cell.push('|');
                chars.next();
            }
            '|' => cells.push(std::mem::take(&mut cell).trim().to_string()),
            c => cell.push(c),
        }
    }
    cells.push(cell.trim().to_string());
    Some(cells)
}

fn parse_align(cell: &str) -> Option<Align> {
    let cell = cell.trim();
    let (left, right) = (cell.starts_with(':'), cell.ends_with(':'));
    let dashes = cell.trim_matches(':');
    if dashes.is_empty() || !dashes.chars().all(|c| c == '-') {
        return None;
    }
    Some(match (left, right) {
        (true, true) => Align::Center,
        (true, false) => Align::Left,
        (false, true) => Align::Right,
        (false, false) => Align::None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "| Name | Price |\n|:-----|------:|\n| Tea | 3 |\n| Cake \\| pie | 5 |";

    #[test]
    fn reads_cells_alignment_and_escaped_pipes() {
        let table = Table::parse(TABLE).unwrap();
        assert_eq!(table.align, [Align::Left, Align::Right]);
        assert_eq!(table.cell(0, 1), Some("Price"));
        assert_eq!(table.cell(2, 0), Some("Cake | pie"));
        assert!(Table::parse("just text").is_none());
        assert!(Table::parse("| a |\n| not a rule |").is_none());
    }

    #[test]
    fn edits_and_writes_back_lined_up() {
        let mut table = Table::parse(TABLE).unwrap();
        table.set_cell(1, 0, "Green tea");
        table.insert_row(3);
        table.set_cell(3, 0, "Scone");
        table.insert_column(2);
        table.set_cell(0, 2, "Note");
        assert_eq!(
            table.to_markdown(),
            "| Name        | Price | Note |\n\
             | :---------- | ----: | ---- |\n\
             | Green tea   | 3     |      |\n\
             | Cake \\| pie | 5     |      |\n\
             | Scone       |       |      |"
        );
        // What it writes reads back the same.
        assert_eq!(Table::parse(&table.to_markdown()).unwrap(), table);
    }

    #[test]
    fn keeps_a_header_a_row_and_a_column() {
        let mut table = Table::parse("| a |\n|---|\n| 1 |").unwrap();
        assert!(!table.remove_row(0));
        assert!(!table.remove_row(1));
        assert!(!table.remove_column(0));
        table.insert_row(5);
        assert!(table.remove_row(2));
    }
}
