//! A chart's data as a small Excel workbook, embedded in the PPTX beside
//! the chart, so PowerPoint's Edit Data opens it. One sheet: categories
//! down column A and a series a column (scatter charts: each series' x
//! and y side by side).

use std::fmt::Write as _;
use std::io::Write as _;

use anyhow::Result;

use super::charts::escape;
use super::model::{Chart, ChartKind};

/// The letters of column `index` (0 is A).
pub fn column(index: usize) -> String {
    let mut out = String::new();
    let mut n = index + 1;
    while n > 0 {
        let rem = (n - 1) % 26;
        out.insert(0, (b'A' + rem as u8) as char);
        n = (n - 1) / 26;
    }
    out
}

enum Cell {
    Text(String),
    Number(f64),
}

/// The chart's cells, row by row.
fn cells(chart: &Chart) -> Vec<Vec<Option<Cell>>> {
    let mut grid: Vec<Vec<Option<Cell>>> = Vec::new();
    let mut set = |row: usize, column: usize, cell: Cell| {
        while grid.len() <= row {
            grid.push(Vec::new());
        }
        let line = &mut grid[row];
        while line.len() <= column {
            line.push(None);
        }
        line[column] = Some(cell);
    };
    if chart.chart == ChartKind::Scatter {
        for (ix, series) in chart.series.iter().enumerate() {
            set(0, ix * 2, Cell::Text("X".into()));
            set(0, ix * 2 + 1, Cell::Text(series.name.clone()));
            for (row, value) in series.values.iter().enumerate() {
                let x = series.x.get(row).copied().unwrap_or((row + 1) as f64);
                set(row + 1, ix * 2, Cell::Number(x));
                set(row + 1, ix * 2 + 1, Cell::Number(*value));
            }
        }
    } else {
        set(0, 0, Cell::Text(String::new()));
        for (row, category) in chart.categories.iter().enumerate() {
            set(row + 1, 0, Cell::Text(category.clone()));
        }
        for (ix, series) in chart.series.iter().enumerate() {
            set(0, ix + 1, Cell::Text(series.name.clone()));
            for (row, value) in series.values.iter().enumerate() {
                set(row + 1, ix + 1, Cell::Number(*value));
            }
        }
    }
    grid
}

/// The workbook's bytes.
pub fn chart_workbook(chart: &Chart) -> Result<Vec<u8>> {
    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
    let mut sheet = format!(
        r#"{XML}<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#
    );
    for (row, line) in cells(chart).iter().enumerate() {
        let _ = write!(sheet, r#"<row r="{}">"#, row + 1);
        for (col, cell) in line.iter().enumerate() {
            let at = format!("{}{}", column(col), row + 1);
            match cell {
                Some(Cell::Text(text)) => {
                    let _ = write!(
                        sheet,
                        r#"<c r="{at}" t="inlineStr"><is><t xml:space="preserve">{}</t></is></c>"#,
                        escape(text)
                    );
                }
                Some(Cell::Number(value)) if value.is_finite() => {
                    let _ = write!(sheet, r#"<c r="{at}"><v>{value}</v></c>"#);
                }
                _ => {}
            }
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");
    let parts: [(&str, String); 6] = [
        (
            "[Content_Types].xml",
            format!(
                r#"{XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#
            ),
        ),
        (
            "_rels/.rels",
            format!(
                r#"{XML}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
            ),
        ),
        (
            "xl/workbook.xml",
            format!(
                r#"{XML}<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#
            ),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            format!(
                r#"{XML}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#
            ),
        ),
        ("xl/worksheets/sheet1.xml", sheet),
        (
            "xl/styles.xml",
            format!(
                r#"{XML}<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs></styleSheet>"#
            ),
        ),
    ];
    let mut bytes = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, text) in parts {
            zip.start_file(name, options)?;
            zip.write_all(text.as_bytes())?;
        }
        zip.finish()?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #[test]
    fn columns_are_lettered() {
        assert_eq!(super::column(0), "A");
        assert_eq!(super::column(25), "Z");
        assert_eq!(super::column(26), "AA");
        assert_eq!(super::column(27), "AB");
    }
}
