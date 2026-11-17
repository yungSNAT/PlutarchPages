// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Export to Excel (.xlsx; ECMA-376 SpreadsheetML): one worksheet per page. Detected tables keep
//! their rows and columns (a spanning header cell is merged); other paragraphs take one row each
//! in column A, in reading order, with a blank row around each table. Cells that read as plain
//! numbers (`1234.5`, `-7`, `1,234.50`) of at most 15 significant digits are written as numbers;
//! everything else is text.

use crate::layout::{Item, items};
use crate::{Page, Zip, esc};

/// Excel's limits: 1,048,576 rows and 16,384 columns per sheet, 32,767 characters per cell.
const MAX_ROWS: usize = 1_048_576;
const MAX_COLS: usize = 16_384;
const MAX_CELL: usize = 32_767;

/// Column letters: 0 → A, 25 → Z, 26 → AA.
fn column(mut i: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (i % 26) as u8);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// The cell's value as a number, if it is written as a plain one.
fn number(text: &str) -> Option<f64> {
    let t = text.trim();
    let (sign, digits) = t.strip_prefix('-').map_or(("", t), |d| ("-", d));
    let (int, frac) = digits.split_once('.').map_or((digits, ""), |(i, f)| (i, f));
    let grouped = int.contains(',');
    if grouped {
        // 1,234,567: groups of three after the first.
        let mut parts = int.split(',');
        let first = parts.next()?;
        if first.is_empty() || first.len() > 3 || !parts.all(|p| p.len() == 3 && p.bytes().all(|b| b.is_ascii_digit())) {
            return None;
        }
    }
    let int: String = int.chars().filter(|c| *c != ',').collect();
    if int.is_empty() || int.len() > 15 || !int.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) || frac.len() > 10 {
        return None;
    }
    // A leading zero (an ID or postcode such as 0042) stays text.
    if int.len() > 1 && int.starts_with('0') {
        return None;
    }
    // An f64 (and Excel) keeps 15 significant digits; more would be rounded, so it stays text.
    if format!("{int}{}", frac.trim_end_matches('0')).trim_start_matches('0').len() > 15 {
        return None;
    }
    let v: f64 = format!("{sign}{int}.{}", if frac.is_empty() { "0" } else { frac }).parse().ok()?;
    v.is_finite().then_some(v)
}

struct Sheet {
    rows: Vec<String>,
    merges: Vec<String>,
    next_row: usize,
}

impl Sheet {
    fn new() -> Sheet {
        Sheet { rows: Vec::new(), merges: Vec::new(), next_row: 1 }
    }

    /// Add a row of (column, text, bold, span) cells.
    fn row(&mut self, cells: &[(usize, &str, bool, usize)]) {
        if self.next_row > MAX_ROWS {
            return;
        }
        let r = self.next_row;
        let mut xml = format!("<row r=\"{r}\">");
        for (col, text, bold, span) in cells {
            if *col >= MAX_COLS {
                break;
            }
            let at = format!("{}{r}", column(*col));
            let style = if *bold { " s=\"1\"" } else { "" };
            match number(text) {
                Some(v) => xml.push_str(&format!("<c r=\"{at}\"{style}><v>{v}</v></c>")),
                None => {
                    let t: String = text.chars().take(MAX_CELL).collect();
                    xml.push_str(&format!("<c r=\"{at}\" t=\"inlineStr\"{style}><is><t xml:space=\"preserve\">{}</t></is></c>", esc(&t)));
                }
            }
            if *span > 1 {
                let last = (col + span - 1).min(MAX_COLS - 1);
                self.merges.push(format!("{at}:{}{r}", column(last)));
            }
        }
        xml.push_str("</row>");
        self.rows.push(xml);
        self.next_row += 1;
    }

    fn blank(&mut self) {
        if self.next_row > 1 {
            self.next_row += 1;
        }
    }

    fn xml(&self) -> String {
        let merges = if self.merges.is_empty() {
            String::new()
        } else {
            format!(
                "<mergeCells count=\"{}\">{}</mergeCells>",
                self.merges.len(),
                self.merges.iter().map(|m| format!("<mergeCell ref=\"{m}\"/>")).collect::<String>()
            )
        };
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{}</sheetData>{merges}</worksheet>",
            self.rows.concat()
        )
    }
}

fn sheet_for(page: &Page) -> Sheet {
    let mut sheet = Sheet::new();
    for it in items(std::slice::from_ref(page)) {
        match it {
            Item::Para(runs, _) => {
                let text: String = runs.iter().map(|r| r.text.as_str()).collect();
                sheet.row(&[(0, text.as_str(), runs.first().is_some_and(|r| r.block.bold), 1)]);
            }
            Item::Table(t) => {
                sheet.blank();
                for row in &t.rows {
                    let mut col = 0;
                    let cells: Vec<(usize, &str, bool, usize)> = row
                        .iter()
                        .map(|c| {
                            let at = col;
                            col += c.span.max(1);
                            (at, c.text.as_str(), c.bold, c.span.max(1))
                        })
                        .collect();
                    sheet.row(&cells);
                }
                sheet.blank();
            }
            Item::Img(_) | Item::PageBreak => {}
        }
    }
    sheet
}

/// The pages as an Excel workbook, one worksheet per page ("Page 1", "Page 2"…).
pub fn xlsx(pages: &[Page], title: &str) -> Vec<u8> {
    let sheets: Vec<Sheet> = if pages.is_empty() { vec![Sheet::new()] } else { pages.iter().map(sheet_for).collect() };
    let n = sheets.len();
    let mut types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
<Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\
<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>",
    );
    let mut book = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>",
    );
    let mut book_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );
    for i in 1..=n {
        types.push_str(&format!(
            "<Override PartName=\"/xl/worksheets/sheet{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"
        ));
        book.push_str(&format!("<sheet name=\"Page {i}\" sheetId=\"{i}\" r:id=\"rId{i}\"/>"));
        book_rels.push_str(&format!(
            "<Relationship Id=\"rId{i}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{i}.xml\"/>"
        ));
    }
    types.push_str("</Types>");
    book.push_str("</sheets></workbook>");
    book_rels.push_str(&format!(
        "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>",
        n + 1
    ));
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/></Relationships>";
    // Two cell formats: 0 the default, 1 bold.
    let styles = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
<fonts count=\"2\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font><font><b/><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts>\
<fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill></fills>\
<borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders>\
<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
<cellXfs count=\"2\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/><xf numFmtId=\"0\" fontId=\"1\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyFont=\"1\"/></cellXfs>\
<cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles></styleSheet>";
    let core = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>{}</dc:title></cp:coreProperties>",
        esc(title)
    );
    let mut z = Zip::default();
    z.add("[Content_Types].xml", types.as_bytes(), true);
    z.add("_rels/.rels", rels.as_bytes(), true);
    z.add("docProps/core.xml", core.as_bytes(), true);
    z.add("xl/workbook.xml", book.as_bytes(), true);
    z.add("xl/_rels/workbook.xml.rels", book_rels.as_bytes(), true);
    z.add("xl/styles.xml", styles.as_bytes(), true);
    for (i, s) in sheets.iter().enumerate() {
        z.add(&format!("xl/worksheets/sheet{}.xml", i + 1), s.xml().as_bytes(), true);
    }
    z.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Block;

    #[test]
    fn columns_and_numbers() {
        assert_eq!([column(0), column(25), column(26), column(701), column(702)], ["A", "Z", "AA", "ZZ", "AAA"]);
        assert_eq!(number("1234.5"), Some(1234.5));
        assert_eq!(number(" -7 "), Some(-7.0));
        assert_eq!(number("1,234.50"), Some(1234.5));
        for text in ["0042", "12,34", "1.2.3", "abc", "", "-", "$5", "1e9", "12%", ",123"] {
            assert_eq!(number(text), None, "{text:?}");
        }
    }

    #[test]
    fn numbers_past_fifteen_significant_digits_stay_text() {
        // Up to 15 significant digits survive the f64 round trip exactly.
        for (text, written) in [
            ("99999999999.9999", "99999999999.9999"),
            ("-1,234,567,890.12345", "-1234567890.12345"),
            ("12345.1234567891", "12345.1234567891"),
            ("0.0012345678", "0.0012345678"),
            ("999,999,999,999,999", "999999999999999"),
            ("12.5000000000", "12.5"),
        ] {
            assert_eq!(number(text).map(|v| v.to_string()).as_deref(), Some(written), "{text:?}");
        }
        // More would be rounded (999999999999999.99 becomes 1000000000000000), so they stay text.
        for text in ["999999999999999.99", "-999999999999999.99", "999,999,999,999,999.99", "999999999999.9999", "123456.1234567891"] {
            assert_eq!(number(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_page_with_a_table_becomes_a_sheet() {
        let b = |text: &str, x: f64, y: f64| Block {
            text: text.into(),
            rect: [x, y, x + 60.0, y + 12.0],
            size: 11.0,
            bold: false,
            italic: false,
            color: [0.0; 3],
            rotation: 0.0,
            space_around: [false; 2],
        };
        let mut blocks = vec![b("Quarterly report", 72.0, 720.0)];
        for (i, row) in [["Item", "Qty", "Price"], ["Apples", "12", "3.50"], ["Pears", "7", "1,200.00"]].iter().enumerate() {
            for (j, cell) in row.iter().enumerate() {
                blocks.push(b(cell, 72.0 + 120.0 * j as f64, 680.0 - 18.0 * i as f64));
            }
        }
        let page = Page { width: 612.0, height: 792.0, background: None, blocks, images: vec![], rules: vec![] };
        let xml = sheet_for(&page).xml();
        assert!(xml.contains("Quarterly report"), "{xml}");
        assert!(xml.contains("<v>12</v>") && xml.contains("<v>1200</v>"), "numbers are numbers: {xml}");
        let bytes = xlsx(&[page.clone(), page], "Report");
        assert!(bytes.starts_with(b"PK"));
        assert!(!xlsx(&[], "Empty").is_empty());
    }
}
