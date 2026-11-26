// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `form_fill_batch`: one filled PDF for each row of a CSV, tab-delimited or JSON data file, through
//! the same tool the CLI and MCP use. Every name and value here is synthetic.

use std::path::{Path, PathBuf};

use pdfcraft_automation::{Automation, Content};
use pdfcraft_engine::Session;
use serde_json::{Value, json};

fn call(a: &mut Automation, name: &str, args: Value) -> Value {
    match a.call(name, &args).unwrap_or_else(|e| panic!("{name}: {e}")).remove(0) {
        Content::Json(v) => v,
        other => panic!("expected JSON: {other:?}"),
    }
}

fn fail(a: &mut Automation, args: Value) -> String {
    a.call("form_fill_batch", &args).err().map(|e| e.to_string()).unwrap_or_else(|| panic!("expected an error for {args}"))
}

/// A one-page form saved as `form.pdf`, opened as document 1, with its fields added after the open
/// and never saved: a batch fills the form as it stands in the session, unsaved edits included.
fn form(dir: &Path) -> (Automation, u64) {
    let pdf = Session::new().create_from_text("Form", "Order form").unwrap();
    std::fs::write(dir.join("form.pdf"), &*pdf).unwrap();
    let mut a = Automation::new().with_root(dir).unwrap();
    let doc = call(&mut a, "doc_open", json!({ "path": "form.pdf" }))["doc"].as_u64().unwrap();
    let field = |a: &mut Automation, kind: &str, name: &str, y: i64, extra: Value| {
        let mut args = json!({ "doc": doc, "page": 1, "type": kind, "rect": [20, y, 220, y + 22], "name": name });
        if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
            args.extend(extra.clone());
        }
        call(a, "form_add_field", args);
    };
    field(&mut a, "text", "Name", 20, json!({ "multiline": true }));
    field(&mut a, "text", "Price", 60, json!({}));
    field(&mut a, "text", "Qty", 100, json!({}));
    field(&mut a, "text", "Total", 140, json!({}));
    field(&mut a, "combo", "Choice", 180, json!({ "options": ["A", "B"] }));
    field(&mut a, "checkbox", "Agree", 220, json!({}));
    field(&mut a, "list", "Tags", 260, json!({ "options": ["x", "y", "z"], "multi_select": true }));
    call(&mut a, "form_set_props", json!({ "doc": doc, "field": "Price", "validate": { "min": 0 } }));
    call(
        &mut a,
        "form_set_props",
        json!({ "doc": doc, "field": "Total", "format": { "type": "number", "decimals": 2 }, "calculate": { "notation": "Price * Qty" } }),
    );
    (a, doc)
}

fn workdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// The field values of the PDF at `path` (inside the root), by name.
fn filled(a: &mut Automation, path: &Path) -> serde_json::Map<String, Value> {
    let doc = call(a, "doc_open", json!({ "path": path.to_string_lossy() }))["doc"].clone();
    let fields = call(a, "form_fields", json!({ "doc": doc }));
    call(a, "doc_close", json!({ "doc": doc, "discard_changes": true }));
    fields["fields"].as_array().unwrap().iter().map(|f| (f["name"].as_str().unwrap().to_string(), f["value"].clone())).collect()
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match std::fs::read_dir(dir) {
        Ok(entries) => entries.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

/// The agreed check box's on state, as `form_fields` lists it.
fn on_state(a: &mut Automation, doc: u64) -> String {
    let fields = call(a, "form_fields", json!({ "doc": doc }));
    let agree = fields["fields"].as_array().unwrap().iter().find(|f| f["name"] == "Agree").unwrap().clone();
    agree.get("options").and_then(|o| o[0].as_str()).unwrap_or("Yes").to_string()
}

#[test]
fn csv_rows_each_become_a_filled_pdf_and_the_open_form_is_untouched() {
    let dir = workdir();
    let (mut a, doc) = form(dir.path());
    let before = std::fs::read(dir.path().join("form.pdf")).unwrap();
    let on = on_state(&mut a, doc);
    let csv = format!(
        "\u{feff}Name,Price,Qty,Choice,Agree,Tags\r\n\"Smith, Ada\",19.99,3,A,{on},\"x, z\"\r\n\"Bo \"\"the\"\" Second\nline two\",1,2,B,Off,\r\nCy,0,0,,,y\r\n"
    );
    std::fs::write(dir.path().join("rows.csv"), csv).unwrap();
    let report = call(&mut a, "form_fill_batch", json!({ "doc": doc, "data": "rows.csv", "out_dir": "out" }));
    assert_eq!((report["completed"].as_u64(), report["failed"].as_u64()), (Some(3), Some(0)), "{report}");
    let rows = report["rows"].as_array().unwrap();
    assert_eq!(rows.iter().map(|r| r["row"].as_u64().unwrap()).collect::<Vec<_>>(), vec![1, 2, 3], "{report}");
    assert!(rows.iter().all(|r| r["status"] == "ok"), "{report}");
    // Each row's receipt is the SHA-256 of the file written.
    use sha2::Digest as _;
    let written = std::fs::read(dir.path().join("out/form-1.pdf")).unwrap();
    let hash: String = sha2::Sha256::digest(&written).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!((rows[0]["sha256"].as_str(), rows[0]["bytes"].as_u64()), (Some(hash.as_str()), Some(written.len() as u64)), "{report}");
    assert_eq!(files_in(&dir.path().join("out")), vec!["form-1.pdf", "form-2.pdf", "form-3.pdf"]);

    let first = filled(&mut a, &dir.path().join("out/form-1.pdf"));
    assert_eq!(first["Name"], "Smith, Ada");
    assert_eq!(first["Total"], "59.97", "the form's calculation ran: {first:?}");
    assert_eq!(first["Choice"], "A");
    assert_eq!(first["Agree"], true);
    assert_eq!(first["Tags"], json!(["x", "z"]));
    let second = filled(&mut a, &dir.path().join("out/form-2.pdf"));
    assert_eq!(second["Name"], "Bo \"the\" Second\nline two");
    assert_eq!(second["Agree"], false);
    let third = filled(&mut a, &dir.path().join("out/form-3.pdf"));
    assert_eq!((third["Name"].clone(), third["Tags"].clone()), (json!("Cy"), json!(["y"])));

    // The open document and its file are as they were.
    let open = call(&mut a, "form_fields", json!({ "doc": doc }));
    let open: std::collections::HashMap<&str, &Value> =
        open["fields"].as_array().unwrap().iter().map(|f| (f["name"].as_str().unwrap(), &f["value"])).collect();
    assert_eq!((open["Name"], open["Agree"], open["Tags"]), (&Value::Null, &json!(false), &json!([])), "{open:?}");
    assert_eq!(std::fs::read(dir.path().join("form.pdf")).unwrap(), before);
}

#[test]
fn tab_delimited_rows_fill_too() {
    let dir = workdir();
    let (mut a, doc) = form(dir.path());
    std::fs::write(dir.path().join("rows.txt"), "Name\tChoice\nAda, Q\tB\n\nBo\tA\n").unwrap();
    let report = call(&mut a, "form_fill_batch", json!({ "doc": doc, "data": "rows.txt", "out_dir": "out" }));
    assert_eq!(report["completed"], 2, "{report}");
    let first = filled(&mut a, &dir.path().join("out/form-1.pdf"));
    assert_eq!((first["Name"].clone(), first["Choice"].clone()), (json!("Ada, Q"), json!("B")));
    assert_eq!(filled(&mut a, &dir.path().join("out/form-2.pdf"))["Choice"], "A");
}

#[test]
fn a_refused_value_fails_its_row_and_publishes_nothing_for_it() {
    let dir = workdir();
    let (mut a, doc) = form(dir.path());
    let on = on_state(&mut a, doc);
    let csv = format!("Name,Price,Choice,Agree\nGood,1,A,{on}\nNegative,-5,A,Off\nUnknown option,1,Z,Off\nBad box,1,A,Maybe\nShort row,1\n");
    std::fs::write(dir.path().join("rows.csv"), csv).unwrap();
    let report = call(&mut a, "form_fill_batch", json!({ "doc": doc, "data": "rows.csv", "out_dir": "out" }));
    assert_eq!((report["completed"].as_u64(), report["failed"].as_u64()), (Some(1), Some(4)), "{report}");
    let rows = report["rows"].as_array().unwrap();
    assert_eq!(rows[0]["status"], "ok", "{report}");
    for (i, needle) in [(1, "Price"), (2, "Choice"), (3, "Agree"), (4, "2 values")] {
        assert_eq!(rows[i]["status"], "error", "{report}");
        assert!(rows[i].get("output").is_none(), "{report}");
        let error = rows[i]["error"].as_str().unwrap();
        assert!(error.contains(needle), "row {}: {error}", i + 1);
    }
    // Only the good row's file, and no staging file left behind.
    assert_eq!(files_in(&dir.path().join("out")), vec!["form-1.pdf"]);
}

/// A value the row gives a calculated field must survive the calculation, or the row fails.
#[test]
fn a_calculated_field_the_form_overrides_fails_its_row() {
    let dir = workdir();
    let (mut a, doc) = form(dir.path());
    // An empty cell gives the calculation nothing to disagree with.
    std::fs::write(dir.path().join("rows.csv"), "Total,Price,Qty\n999,2,3\n,4,5\n").unwrap();
    let report = call(&mut a, "form_fill_batch", json!({ "doc": doc, "data": "rows.csv", "out_dir": "out" }));
    assert_eq!((report["completed"].as_u64(), report["failed"].as_u64()), (Some(1), Some(1)), "{report}");
    let error = report["rows"][0]["error"].as_str().unwrap();
    assert!(error.contains("Total") && error.contains("calculates") && !error.contains("999"), "{error}");
    assert_eq!(files_in(&dir.path().join("out")), vec!["form-2.pdf"]);
    let total = filled(&mut a, &dir.path().join("out/form-2.pdf"))["Total"].as_str().unwrap_or_default().to_string();
    assert!(total.starts_with("20"), "{total}");
    // The value the form calculates, given as the form stores it, passes.
    std::fs::write(dir.path().join("agree.csv"), format!("Total,Price,Qty\n{total},4,5\n")).unwrap();
    let report = call(&mut a, "form_fill_batch", json!({ "doc": doc, "data": "agree.csv", "out_dir": "agree" }));
    assert_eq!(report["completed"], 1, "{report}");
    let report = call(&mut a, "form_fill_batch", json!({ "doc": doc, "data": "rows.csv", "out_dir": "verbose", "verbose": true }));
    assert!(report["rows"][0]["error"].as_str().unwrap().contains("999"), "{report}");
}

/// Rows may hold personal data: an error names the row and field, and quotes a value only when
/// the caller asks for verbose.
#[test]
fn errors_quote_no_cell_unless_verbose() {
    let dir = workdir();
    let (mut a, doc) = form(dir.path());
    let csv = "File,Name,Choice,Agree\nSecret-1,Ada,Secret-2,\nok,Bo,A,Secret-3\nSecret/4,Cy,A,\nSecret-5,Dee,A,\nsecret-5,Eve,A,\n";
    std::fs::write(dir.path().join("rows.csv"), csv).unwrap();
    let quiet = call(&mut a, "form_fill_batch", json!({ "doc": doc, "data": "rows.csv", "out_dir": "out", "name_column": "File" }));
    let loud = call(&mut a, "form_fill_batch", json!({ "doc": doc, "data": "rows.csv", "out_dir": "out", "name_column": "File", "verbose": true }));
    for (row, field) in [(0, "Choice"), (1, "Agree"), (2, "file name"), (3, "row 5"), (4, "row 4")] {
        let (quiet, loud) = (quiet["rows"][row]["error"].as_str().unwrap(), loud["rows"][row]["error"].as_str().unwrap());
        assert!(quiet.contains(field) && !quiet.contains("ecret"), "row {}: {quiet}", row + 1);
        assert!(loud.contains(field) && loud.contains("ecret"), "row {}: {loud}", row + 1);
    }
}

#[test]
fn repeated_names_report_one_conflict_and_leave_unique_rows_usable() {
    let dir = workdir();
    let (mut a, doc) = form(dir.path());
    let csv = "File,Name\ncopy,Ada\nCOPY,Bo\ncopy,Cy\none,Dee\n";
    std::fs::write(dir.path().join("rows.csv"), csv).unwrap();
    let report = call(&mut a, "form_fill_batch", json!({ "doc": doc, "data": "rows.csv", "out_dir": "out", "name_column": "File" }));
    assert_eq!(report["completed"], 1, "{report}");
    assert_eq!(report["failed"], 3, "{report}");
    for (i, other) in [2, 1, 1].into_iter().enumerate() {
        assert_eq!(report["rows"][i]["error"], format!("the file name is also given by row {other} (3 rows share it)"));
    }
    assert_eq!(files_in(&dir.path().join("out")), vec!["one.pdf"]);
}

#[test]
fn a_column_that_names_no_field_fails_every_row() {
    let dir = workdir();
    let (mut a, doc) = form(dir.path());
    std::fs::write(dir.path().join("rows.csv"), "Name,Nickname\nAda,A\nBo,B\n").unwrap();
    let report = call(&mut a, "form_fill_batch", json!({ "doc": doc, "data": "rows.csv", "out_dir": "out" }));
    assert_eq!((report["completed"].as_u64(), report["failed"].as_u64()), (Some(0), Some(2)), "{report}");
    assert!(report["rows"][0]["error"].as_str().unwrap().contains("Nickname"), "{report}");
    assert!(files_in(&dir.path().join("out")).is_empty());
}

#[test]
fn output_names_stay_in_the_folder_and_never_overwrite() {
    let dir = workdir();
    let (mut a, doc) = form(dir.path());
    std::fs::create_dir(dir.path().join("out")).unwrap();
    std::fs::write(dir.path().join("out/kept.pdf"), b"precious").unwrap();
    let csv = "File,Name\n../escape,1\nsub/inner,2\nCON,3\n,4\nsame,5\nSAME.pdf,6\nfine,7\nkept,8\n..,9\nback\\slash,10\n";
    std::fs::write(dir.path().join("rows.csv"), csv).unwrap();
    let report = call(&mut a, "form_fill_batch", json!({ "doc": doc, "data": "rows.csv", "out_dir": "out", "name_column": "File" }));
    assert_eq!((report["completed"].as_u64(), report["failed"].as_u64()), (Some(2), Some(8)), "{report}");
    let rows = report["rows"].as_array().unwrap();
    let ok: Vec<u64> = rows.iter().filter(|r| r["status"] == "ok").map(|r| r["row"].as_u64().unwrap()).collect();
    assert_eq!(ok, vec![7, 8], "{report}");
    assert!(rows[4]["error"].as_str().unwrap().contains("row 6"), "a clash names the other row: {report}");
    assert!(rows[5]["error"].as_str().unwrap().contains("row 5"), "{report}");
    assert_eq!(files_in(&dir.path().join("out")), vec!["fine.pdf", "kept (2).pdf", "kept.pdf"]);
    assert_eq!(std::fs::read(dir.path().join("out/kept.pdf")).unwrap(), b"precious");
    assert!(!dir.path().join("escape.pdf").exists() && !dir.path().join("out/sub").exists());
    // The name column fills no field.
    assert_eq!(filled(&mut a, &dir.path().join("out/fine.pdf"))["Name"], "7");
}

/// Each name some file system cannot hold, or that would display as another name, fails its row.
#[test]
fn unsafe_or_misleading_names_fail_their_rows() {
    let dir = workdir();
    let (mut a, doc) = form(dir.path());
    let long = "n".repeat(201);
    let refused = [
        "C:x",
        "a.",
        "a ",
        " lead",
        "tab\there",
        "bell\u{7}",
        long.as_str(),
        "CON.txt",
        "con",
        "COM0",
        "LPT\u{b9}",
        "CONIN$",
        "conout$.pdf",
        "report\u{202e}fdp.exe",
        "zero\u{200b}width",
        "bom\u{feff}",
        "tag\u{e0041}",
        "filler\u{3164}",
        "star*",
        "pipe|",
        "quote\"",
    ];
    let mut rows: Vec<Value> = refused.iter().map(|name| json!({ "Name": "x", "file": name })).collect();
    // The same name composed and decomposed, as macOS holds one of them, and in another case.
    rows.extend([
        json!({ "Name": "x", "file": "caf\u{e9}" }),
        json!({ "Name": "x", "file": "CAFE\u{301}" }),
        json!({ "Name": "x", "file": "COM10" }),
    ]);
    std::fs::write(dir.path().join("rows.json"), Value::Array(rows).to_string()).unwrap();
    let report = call(&mut a, "form_fill_batch", json!({ "doc": doc, "data": "rows.json", "out_dir": "out", "name_column": "file" }));
    let rows = report["rows"].as_array().unwrap();
    for (row, name) in rows.iter().zip(refused) {
        assert_eq!(row["status"], "error", "{name:?}: {row}");
    }
    let n = refused.len();
    assert!(rows[n]["error"].as_str().unwrap().contains(&format!("row {}", n + 2)), "{report}");
    assert!(rows[n + 1]["error"].as_str().unwrap().contains(&format!("row {}", n + 1)), "{report}");
    assert_eq!((rows[n + 2]["status"].as_str(), report["completed"].as_u64()), (Some("ok"), Some(1)), "COM10 is no device: {report}");
    assert_eq!(files_in(&dir.path().join("out")), vec!["COM10.pdf"]);
}

#[test]
fn json_rows_take_typed_values() {
    let dir = workdir();
    let (mut a, doc) = form(dir.path());
    let rows = json!([
        { "Name": "Ada", "Price": "19.99", "Qty": 3, "Agree": true, "Tags": ["x", "z"], "file": "ada" },
        { "Name": "Bo", "Agree": false, "file": "bo" },
        { "Name": { "nested": 1 }, "file": "nested" },
        "not an object",
        { "Name": "Cy", "file": 7 },
        { "Name": "Dee" }
    ]);
    std::fs::write(dir.path().join("rows.json"), rows.to_string()).unwrap();
    let report = call(&mut a, "form_fill_batch", json!({ "doc": doc, "data": "rows.json", "out_dir": "out", "name_column": "file" }));
    assert_eq!((report["completed"].as_u64(), report["failed"].as_u64()), (Some(3), Some(3)), "{report}");
    let statuses: Vec<&str> = report["rows"].as_array().unwrap().iter().map(|r| r["status"].as_str().unwrap()).collect();
    assert_eq!(statuses, vec!["ok", "ok", "error", "error", "ok", "error"], "{report}");
    assert!(report["rows"][3]["error"].as_str().unwrap().contains("object"), "{report}");
    assert!(report["rows"][5]["error"].as_str().unwrap().contains("file"), "a row without its name key fails alone: {report}");
    let ada = filled(&mut a, &dir.path().join("out/ada.pdf"));
    assert_eq!((ada["Qty"].clone(), ada["Total"].clone(), ada["Agree"].clone()), (json!("3"), json!("59.97"), json!(true)));
    assert_eq!(ada["Tags"], json!(["x", "z"]));
    assert_eq!(filled(&mut a, &dir.path().join("out/bo.pdf"))["Agree"], false);
    assert_eq!(filled(&mut a, &dir.path().join("out/7.pdf"))["Name"], "Cy");
}

#[test]
fn empty_or_unusable_data_says_so_before_writing_anything() {
    let dir = workdir();
    let (mut a, doc) = form(dir.path());
    for (file, text, needle) in [
        ("empty.csv", "", "empty"),
        ("blank.csv", "\u{feff}\r\n\n", "empty"),
        ("names.csv", "Name,Qty\n", "no rows"),
        ("none.json", "[]", "no rows"),
        ("object.json", "{\"Name\":\"Ada\"}", "array"),
        ("broken.json", "[{\"Name\":", "JSON"),
        ("data.xml", "<fields><Name>Ada</Name></fields>", "CSV"),
        ("twice.csv", "Name,Name\nAda,Bo\n", "twice"),
    ] {
        std::fs::write(dir.path().join(file), text).unwrap();
        let error = fail(&mut a, json!({ "doc": doc, "data": file, "out_dir": "out" }));
        assert!(error.contains(needle), "{file}: {error}");
    }
    std::fs::write(dir.path().join("rows.csv"), "Name\nAda\n").unwrap();
    let error = fail(&mut a, json!({ "doc": doc, "data": "rows.csv", "out_dir": "out", "name_column": "File" }));
    assert!(error.contains("File"), "{error}");
    // Past the row limit, as a table and as JSON; parsing stops one row past it.
    std::fs::write(dir.path().join("many.csv"), format!("Name\n{}", "Ada\n".repeat(10_001))).unwrap();
    std::fs::write(dir.path().join("many.txt"), format!("Name\tQty\n{}", "Ada\t1\n".repeat(10_001))).unwrap();
    std::fs::write(dir.path().join("many.json"), format!("[{}{{}}]", "{},".repeat(10_000))).unwrap();
    for file in ["many.csv", "many.txt", "many.json"] {
        let error = fail(&mut a, json!({ "doc": doc, "data": file, "out_dir": "out" }));
        assert!(error.contains("10000"), "{file}: {error}");
    }
    // A file over the size cap is refused before it is read.
    let big = std::fs::File::create(dir.path().join("big.csv")).unwrap();
    big.set_len(32 * 1024 * 1024 + 1).unwrap();
    let error = fail(&mut a, json!({ "doc": doc, "data": "big.csv", "out_dir": "out" }));
    assert!(error.contains("32 MiB"), "{error}");
    std::fs::write(dir.path().join("latin1.csv"), b"Name\nRen\xe9\n").unwrap();
    assert!(fail(&mut a, json!({ "doc": doc, "data": "latin1.csv", "out_dir": "out" })).contains("UTF-8"));
    assert!(!dir.path().join("out").exists(), "nothing was written");
}

#[test]
fn paths_outside_the_root_are_refused() {
    let dir = workdir();
    std::fs::create_dir(dir.path().join("root")).unwrap();
    let root: PathBuf = dir.path().join("root");
    let (mut a, doc) = form(&root);
    std::fs::write(root.join("rows.csv"), "Name\nAda\n").unwrap();
    std::fs::write(dir.path().join("outside.csv"), "Name\nAda\n").unwrap();
    fail(&mut a, json!({ "doc": doc, "data": "rows.csv", "out_dir": "../out" }));
    fail(&mut a, json!({ "doc": doc, "data": "../outside.csv", "out_dir": "out" }));
    assert!(!dir.path().join("out").exists() && !root.join("out").exists());
}

/// An output name that resolves through a link out of the root fails its row, and the error does
/// not quote the name unless verbose.
#[cfg(unix)]
#[test]
fn an_output_linked_out_of_the_root_fails_without_quoting_its_name() {
    let dir = workdir();
    let root: PathBuf = dir.path().join("root");
    std::fs::create_dir_all(root.join("out")).unwrap();
    let (mut a, doc) = form(&root);
    std::fs::write(dir.path().join("outside.pdf"), b"outside").unwrap();
    std::os::unix::fs::symlink(dir.path().join("outside.pdf"), root.join("out/Secret-link.pdf")).unwrap();
    std::fs::write(root.join("rows.csv"), "File,Name\nSecret-link,Ada\n").unwrap();
    let args = json!({ "doc": doc, "data": "rows.csv", "out_dir": "out", "name_column": "File" });
    let report = call(&mut a, "form_fill_batch", args);
    assert_eq!(report["failed"], 1, "{report}");
    assert!(!report["rows"][0]["error"].as_str().unwrap().contains("ecret"), "{report}");
    assert_eq!(std::fs::read(dir.path().join("outside.pdf")).unwrap(), b"outside");
}

#[test]
fn flatten_bakes_each_rows_values_into_the_page() {
    let dir = workdir();
    let (mut a, doc) = form(dir.path());
    std::fs::write(dir.path().join("rows.csv"), "Name\nAda Flattened\n").unwrap();
    let report = call(&mut a, "form_fill_batch", json!({ "doc": doc, "data": "rows.csv", "out_dir": "out", "flatten": true }));
    assert_eq!(report["completed"], 1, "{report}");
    let out = call(&mut a, "doc_open", json!({ "path": "out/form-1.pdf" }))["doc"].clone();
    let fields = call(&mut a, "form_fields", json!({ "doc": out }));
    assert!(fields["fields"].as_array().unwrap().is_empty(), "{fields}");
    let text = call(&mut a, "text_extract", json!({ "doc": out }));
    assert!(text.to_string().contains("Ada Flattened"), "{text}");
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_marks_a_batch_with_a_failed_row_as_an_error_and_keeps_the_report() {
    let dir = workdir();
    let (a, doc) = form(dir.path());
    std::fs::write(dir.path().join("rows.csv"), "Name,Choice\nAda,Z\n").unwrap();
    let input = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"form_fill_batch","arguments":{"doc":doc,"data":"rows.csv","out_dir":"out"}}})
        .to_string()
        + "\n";
    let mut out = Vec::new();
    pdfcraft_automation::mcp::McpServer::new(a).serve(input.as_bytes(), &mut out).unwrap();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["result"]["isError"], true, "{report}");
    assert_eq!(report["result"]["structuredContent"]["failed"], 1, "{report}");
}
