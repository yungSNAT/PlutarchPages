// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// Export Data as CSV writes a name or value that a spreadsheet would run as a formula with a
/// leading `'`, by the same rule as Merge Data Files.
#[test]
fn one_field_exports_never_start_a_formula() {
    for (name, value, expected) in [
        ("Name", "=1+1", "Name\n'=1+1\n"),
        ("Name", "@SUM(A1:A2)", "Name\n'@SUM(A1:A2)\n"),
        ("Name", "+1 555 0100", "Name\n'+1 555 0100\n"),
        ("Name", "-A1", "Name\n'-A1\n"),
        ("Name", " =1+1", "Name\n' =1+1\n"),
        ("Name", "\t=1+1", "Name\n\"'\t'=1+1\"\n"),
        ("Name", "\r=1+1", "Name\n\"'\r'=1+1\"\n"),
        ("Name", "=1,2", "Name\n\"'=1,2\"\n"),
        ("Name", "'=1+1", "Name\n'=1+1\n"),
        ("Name", "-12.5", "Name\n-12.5\n"),
        ("Name", "+1", "Name\n+1\n"),
        ("Name", "'-5", "Name\n'-5\n"),
        ("Name", "'plain", "Name\n'plain\n"),
        ("Name", "a=b", "Name\na=b\n"),
        ("=Name", "Ada", "'=Name\nAda\n"),
    ] {
        let mut src = one_field();
        let field = fields(&src)[0].obj;
        src.update_dict(field, |d| d.set(b"T".to_vec(), PdfString::text(name))).unwrap();
        set_value(&mut src, name, &FieldValue::Text(value.into())).unwrap();
        let source = Arc::new(write_full(&src, &SaveOptions::default()).unwrap());
        let src = Document::open(source).unwrap();
        assert_eq!(export_data(&src, Format::Csv), expected, "{value:?}");
    }
}

#[test]
fn exported_csv_cells_never_start_a_formula() {
    for (text, cell) in FORMULA_CELLS {
        let lead = leading_cell(text, cell);
        // As a value.
        let mut doc = blank();
        set_value(&mut doc, "Name", &FieldValue::Text(text.into())).unwrap();
        assert_eq!(export_data(&doc, Format::Csv), format!("Name,Agree,Pick\n{lead},Off,\n"), "value {text:?}");
        // As a field name.
        let mut doc = blank();
        let field = fields(&doc).into_iter().find(|f| f.name == "Name").unwrap().obj;
        doc.update_dict(field, |d| d.set(b"T".to_vec(), PdfString::text(text))).unwrap();
        assert_eq!(export_data(&doc, Format::Csv), format!("{lead},Agree,Pick\n,Off,\n"), "name {text:?}");
    }
}

/// Where a spreadsheet splits on `;`, `-1,5+A1` was the formula `=-1.5+A1`, and so was the same
/// record after `x;`; a cell holding `;` or a line break could start any formula. A record of one
/// cell can't run on, so its number is left alone.
#[test]
fn records_split_on_semicolons_or_line_breaks_never_start_a_formula() {
    let row = |a: &str| vec![("a".to_string(), vec![a.to_string()]), ("b".to_string(), vec!["5+A1".to_string()])];
    let wrong: Vec<_> = [
        ("-1", " -1,5+A1"),
        ("+3", " +3,5+A1"),
        // A byte order mark leading the file is dropped on import, so the space goes after it.
        ("\u{feff}-1", "\u{feff} -1,5+A1"),
        ("x;-1", "x;'-1,5+A1"),
        ("x;=1+1;", "x;'=1+1;,5+A1"),
        ("x\n=1+1\n", "\"x\n'=1+1\n\",5+A1"),
        ("x\r=1+1", "\"x\r'=1+1\",5+A1"),
    ]
    .into_iter()
    .filter(|(a, line)| merge_csv(&[row(a)]) != format!("a,b\n{line}\n"))
    .collect();
    assert!(wrong.is_empty(), "written otherwise: {wrong:?}");
    assert_eq!(merge_csv(&[vec![("a".to_string(), vec!["-1".to_string()])]]), "a\n-1\n");
}

/// Import keeps a leading `'`, and the space before a leading signed number: it can't tell the ones
/// export added from ones the value had.
#[test]
fn csv_import_keeps_a_leading_apostrophe() {
    for (value, imported) in [("=1+1", "'=1+1"), ("-12.50", " -12.50")] {
        let mut src = blank();
        set_value(&mut src, "Name", &FieldValue::Text(value.into())).unwrap();
        let mut dst = blank();
        import(&mut dst, export_data(&src, Format::Csv).as_bytes()).unwrap();
        assert_eq!(values(&dst)[0].1, [imported]);
    }
}

#[test]
fn a_csv_header_with_a_quoted_tab_is_read_as_csv() {
    // CSV export quotes a field name holding a tab; import must not take that tab as a delimiter.
    let rename = |doc: &mut Document| {
        let field = fields(doc).into_iter().find(|f| f.name == "Name").unwrap().obj;
        doc.update_dict(field, |d| d.set(b"T".to_vec(), PdfString::text("First\tName"))).unwrap();
    };
    let mut src = blank();
    rename(&mut src);
    set_value(&mut src, "First\tName", &FieldValue::Text("Ada".into())).unwrap();
    let csv = export_data(&src, Format::Csv);
    assert!(csv.starts_with("\"First\tName\",Agree,"), "{csv}");
    let mut dst = blank();
    rename(&mut dst);
    let r = import(&mut dst, csv.as_bytes()).unwrap();
    assert!(r.rejected.is_empty(), "{r:?}");
    assert_eq!(values(&dst)[0].1, ["Ada"]);
    // Tab-delimited text stays tab-delimited when a name holds a quote or a comma (text export
    // writes both as they are), an unclosed leading quote included.
    for name in ["Na\"me, x", "\"Name\"", "\"Name"] {
        let mut dst = blank();
        let r = import(&mut dst, format!("{name}\tAgree\r\nv\tYes\r\n").as_bytes()).unwrap();
        assert_eq!(values(&dst)[1].1, ["Yes"], "{name:?}: {r:?}");
    }
    // Names `"Name` and `Agree"` put the header's only tab between quotes; the value row's bare
    // tab shows the file is tab-delimited.
    let mut dst = blank();
    for (from, to) in [("Name", "\"Name"), ("Agree", "Agree\"")] {
        let field = fields(&dst).into_iter().find(|f| f.name == from).unwrap().obj;
        dst.update_dict(field, |d| d.set(b"T".to_vec(), PdfString::text(to))).unwrap();
    }
    let r = import(&mut dst, b"\"Name\tAgree\"\r\nv\tYes\r\n").unwrap();
    assert!(r.rejected.is_empty(), "{r:?}");
    assert_eq!((values(&dst)[0].1.clone(), values(&dst)[1].1.clone()), (vec!["v".to_string()], vec!["Yes".to_string()]));
    // A comma outside quotes in the header keeps it CSV even when a value holds a bare tab.
    let mut dst = blank();
    rename(&mut dst);
    let r = import(&mut dst, b"\"First\tName\",Agree\r\nAda\tMary,Yes\r\n").unwrap();
    assert!(r.rejected.is_empty(), "{r:?}");
    assert_eq!((values(&dst)[0].1.clone(), values(&dst)[1].1.clone()), (vec!["Ada\tMary".to_string()], vec!["Yes".to_string()]));
}

#[test]
fn merged_cells_in_every_column_never_start_a_formula() {
    let row = vec![
        ("=HYPERLINK(\"http://x\")".to_string(), vec!["@SUM(A1)".to_string()]),
        ("Total".to_string(), vec!["-2+3".to_string()]),
        ("Plus".to_string(), vec!["\t+1+cmd".to_string()]),
        ("Balance".to_string(), vec!["-12.5".to_string()]),
        ("\tName".to_string(), vec!["Ada".to_string()]),
    ];
    let csv = merge_csv(&[row]);
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], "\"'=HYPERLINK(\"\"http://x\"\")\",Total,Plus,Balance,\"\tName\"");
    assert_eq!(lines[1], "'@SUM(A1),'-2+3,\"'\t'+1+cmd\",-12.5,Ada");
}
