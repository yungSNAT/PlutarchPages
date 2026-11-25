// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `pdfcraft-cli form-fill-batch`: the CLI face of the `form_fill_batch` tool. Synthetic data only.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

fn cli(dir: &Path, args: &[&str]) -> Output {
    Command::new(BIN).current_dir(dir).args(args).output().unwrap()
}

/// `form.pdf` in `dir`: a blank page with a text field and a combo box, made through `run --script`.
fn form(dir: &Path) {
    let steps = json!([
        { "tool": "doc_create", "args": { "from": "blank", "width": 300, "height": 300 } },
        { "tool": "form_add_field", "args": { "doc": 1, "page": 1, "type": "text", "rect": [20, 20, 200, 42], "name": "Name" } },
        { "tool": "form_add_field", "args": { "doc": 1, "page": 1, "type": "combo", "rect": [20, 60, 200, 82], "name": "Choice", "options": ["A", "B"] } },
        { "tool": "doc_save", "args": { "doc": 1, "path": "form.pdf" } }
    ]);
    std::fs::write(dir.join("steps.json"), steps.to_string()).unwrap();
    let out = cli(dir, &["run", "--script", "steps.json", "--root", "."]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn cli_form_fill_batch_writes_a_pdf_per_row_and_fails_when_a_row_does() {
    let dir = tempfile::tempdir().unwrap();
    form(dir.path());
    std::fs::write(dir.path().join("rows.csv"), "Name,Choice\nAda,A\nBo,Z\n").unwrap();
    let out = cli(dir.path(), &["form-fill-batch", "form.pdf", "--data", "rows.csv", "--out-dir", "out", "--root", "."]);
    assert!(!out.status.success(), "a refused value fails the run");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!((report["completed"].as_u64(), report["failed"].as_u64()), (Some(1), Some(1)), "{report}");
    assert!(report["rows"][1]["error"].as_str().unwrap().contains("Choice"), "{report}");
    assert!(dir.path().join("out/form-1.pdf").exists() && !dir.path().join("out/form-2.pdf").exists());

    std::fs::write(dir.path().join("named.json"), r#"[{"File": "cy", "Name": "Cy"}]"#).unwrap();
    let out = cli(
        dir.path(),
        &["form-fill-batch", "form.pdf", "--data", "named.json", "--out-dir", "out", "--name-column", "File", "--flatten", "--verbose"],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = cli(dir.path(), &["text", "out/cy.pdf"]);
    assert!(String::from_utf8_lossy(&text.stdout).contains("Cy"), "the flattened value is page text");
}

/// The tool through `run --script`, as an agent's script calls it.
#[test]
fn run_script_fills_a_form_per_row() {
    let dir = tempfile::tempdir().unwrap();
    form(dir.path());
    std::fs::write(dir.path().join("rows.csv"), "File,Name,Choice\nada,Ada,A\nbo,Bo,B\n").unwrap();
    let steps = json!([
        { "tool": "doc_open", "args": { "path": "form.pdf" } },
        { "tool": "form_fill_batch", "args": { "doc": 1, "data": "rows.csv", "out_dir": "out", "name_column": "File" } }
    ]);
    std::fs::write(dir.path().join("fill.json"), steps.to_string()).unwrap();
    let out = cli(dir.path(), &["run", "--script", "fill.json", "--root", "."]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(dir.path().join("out/ada.pdf").exists() && dir.path().join("out/bo.pdf").exists());
}

#[test]
fn cli_form_fill_batch_rejects_bad_flags_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    form(dir.path());
    std::fs::write(dir.path().join("rows.csv"), "Name\nAda\n").unwrap();
    for args in [
        &["form-fill-batch"][..],
        &["form-fill-batch", "form.pdf", "--data"],
        &["form-fill-batch", "form.pdf", "--data", "rows.csv"],
        &["form-fill-batch", "form.pdf", "--data", "--out-dir", "out"],
        &["form-fill-batch", "form.pdf", "form.pdf", "--data", "rows.csv", "--out-dir", "out"],
        &["form-fill-batch", "form.pdf", "--data", "rows.csv", "--data", "rows.csv", "--out-dir", "out"],
        &["form-fill-batch", "form.pdf", "--data", "rows.csv", "--out-dir", "out", "--unknown"],
        &["form-fill-batch", "absent.pdf", "--data", "rows.csv", "--out-dir", "out"],
    ] {
        let out = cli(dir.path(), args);
        assert!(!out.status.success(), "{args:?}");
        assert!(!out.stderr.is_empty(), "{args:?}");
        assert!(!dir.path().join("out").exists(), "{args:?}");
    }
}
