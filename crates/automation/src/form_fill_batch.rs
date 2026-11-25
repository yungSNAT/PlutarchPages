// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `form_fill_batch`: one filled copy of an open form for each row of a CSV, tab-delimited or
//! JSON data file. A row is all or nothing: nothing is written for a row the form refuses a value
//! of. Every output name is checked before the first file is written.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use pdfcraft_engine::{Edit, FormField, FormFieldKind, Session, field_flags};
use serde_json::{Value, json};

use crate::batch::{absolute, publish};
use crate::forms::value_for;
use crate::{Args, Automation, Result, failed};

/// Most rows one call fills.
const MAX_ROWS: usize = 10_000;

/// Longest output file name, in bytes: well inside every common file system's 255.
const MAX_NAME: usize = 200;

/// Windows device names, which name no file there whatever the extension.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5",
    "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A value as the data file gave it.
enum Cell {
    /// A CSV or tab-delimited cell.
    Text(String),
    /// A JSON value, as `form_fill` takes it.
    Json(Value),
}

/// One data row: its values by field name, and the value that names its file, if any.
struct Row {
    values: Vec<(String, Cell)>,
    file: Option<String>,
}

type RowResult<T> = std::result::Result<T, String>;

impl Automation {
    pub(crate) fn form_fill_batch(&self, args: &Args) -> Result<Value> {
        let source = self.doc(args)?;
        let (id, form_name, password) = (source.id, source.name.clone(), source.password.clone());
        let name_column = args.opt_str("name_column")?;
        let flatten = args.opt_bool("flatten")?.unwrap_or(false);
        let data = self.resolve(args.str("data")?, false)?;
        let text = String::from_utf8(crate::read_input(&data)?).map_err(|_| failed("the data file is not UTF-8 text"))?;
        let rows = read_rows(&text, name_column)?;
        let out = absolute(&self.resolve(args.str("out_dir")?, true)?)?;
        // The form as it stands, unsaved edits included; the open document is not changed.
        let bytes = self.session.save_bytes(id).map_err(failed)?;
        let javascript = self.session.javascript();
        let stem = Path::new(&form_name).file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| check_name(&format!("{s}.pdf")).is_ok());
        let stem = stem.unwrap_or_else(|| "form".into());
        let names = output_names(&rows, &stem);

        let (mut completed, mut failed_rows) = (0, 0);
        let mut report = Vec::with_capacity(rows.len());
        for (i, (row, name)) in rows.into_iter().zip(names).enumerate() {
            let result: RowResult<_> = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let (row, name) = (row?, name?);
                let pdf = fill(&form_name, &bytes, password.as_deref(), javascript, &row, flatten)?;
                let target = out.join(&name);
                let target = self.resolve(target.to_str().ok_or("the output path is not UTF-8")?, true).map_err(|e| e.to_string())?;
                if !target.starts_with(&out) {
                    return Err("the output path leaves out_dir".to_string());
                }
                let written = publish(&target, &pdf).map_err(|e| e.to_string())?;
                Ok((written, pdf.len()))
            }))
            .unwrap_or_else(|_| Err("filling this row panicked; no file was written for it".into()));
            report.push(match result {
                Ok((written, size)) => {
                    completed += 1;
                    json!({ "row": i + 1, "status": "ok", "output": written.to_string_lossy(), "bytes": size })
                }
                Err(error) => {
                    failed_rows += 1;
                    json!({ "row": i + 1, "status": "error", "error": error })
                }
            });
        }
        Ok(json!({ "completed": completed, "failed": failed_rows, "rows": report }))
    }
}

/// The rows of a data file, each a row or why it cannot be one. A problem with the file as a whole
/// (empty, no rows, too many, a repeated column, an unknown name column) is an error.
fn read_rows(text: &str, name_column: Option<&str>) -> Result<Vec<RowResult<Row>>> {
    let text = text.trim_start_matches('\u{feff}');
    let lead = text.trim_start();
    if lead.is_empty() {
        return Err(failed("the data file is empty"));
    }
    let rows = if lead.starts_with(['[', '{']) {
        json_rows(lead, name_column)?
    } else if lead.starts_with('<') || lead.starts_with("%FDF") || lead.starts_with("%PDF") {
        return Err(failed(
            "form_fill_batch reads CSV, tab-delimited text or a JSON array of rows; XML, XFDF and FDF data hold one record (use doc_import_data)",
        ));
    } else {
        table_rows(text, name_column)?
    };
    if rows.is_empty() {
        return Err(failed("the data file has no rows to fill"));
    }
    Ok(rows)
}

fn too_many(rows: usize) -> Result<()> {
    if rows > MAX_ROWS { Err(failed(format!("the data file has {rows} rows; at most {MAX_ROWS} are filled in one call"))) } else { Ok(()) }
}

fn table_rows(text: &str, name_column: Option<&str>) -> Result<Vec<RowResult<Row>>> {
    let mut records = pdfcraft_engine::table_records(text).into_iter();
    let head = records.next().ok_or_else(|| failed("the data file is empty"))?;
    let mut seen = HashSet::new();
    if let Some(twice) = head.iter().find(|h| !seen.insert(h.as_str())) {
        return Err(failed(format!("the column {twice:?} appears twice in the names row")));
    }
    let name_at = match name_column {
        Some(column) => {
            Some(head.iter().position(|h| h == column).ok_or_else(|| failed(format!("name_column: there is no column named {column:?}")))?)
        }
        None => None,
    };
    let records: Vec<Vec<String>> = records.collect();
    too_many(records.len())?;
    Ok(records
        .into_iter()
        .map(|cells| {
            if cells.len() != head.len() {
                return Err(format!("the row has {} values for {} columns", cells.len(), head.len()));
            }
            let mut row = Row { values: Vec::with_capacity(cells.len()), file: None };
            for (i, (name, cell)) in head.iter().zip(cells).enumerate() {
                if Some(i) == name_at {
                    row.file = Some(cell);
                } else {
                    row.values.push((name.clone(), Cell::Text(cell)));
                }
            }
            Ok(row)
        })
        .collect())
}

fn json_rows(text: &str, name_column: Option<&str>) -> Result<Vec<RowResult<Row>>> {
    let value: Value = serde_json::from_str(text).map_err(|e| failed(format!("the data file is not valid JSON: {e}")))?;
    let Value::Array(items) = value else { return Err(failed("JSON data must be an array of objects, one per row")) };
    too_many(items.len())?;
    Ok(items
        .into_iter()
        .map(|item| {
            let Value::Object(mut map) = item else { return Err("the row is not a JSON object of field names and values".to_string()) };
            let file = match name_column {
                None => None,
                Some(column) => Some(match map.remove(column) {
                    Some(Value::String(s)) => s,
                    Some(Value::Number(n)) => n.to_string(),
                    Some(_) => return Err(format!("{column}: a file name is a string or a number")),
                    None => return Err(format!("the row has no {column:?} to name its file")),
                }),
            };
            Ok(Row { values: map.into_iter().map(|(k, v)| (k, Cell::Json(v))).collect(), file })
        })
        .collect())
}

/// Each row's file name, checked; names that two or more rows give fail all of those rows.
fn output_names(rows: &[RowResult<Row>], stem: &str) -> Vec<RowResult<String>> {
    let mut names: Vec<RowResult<String>> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let row = row.as_ref().map_err(Clone::clone)?;
            let name = match &row.file {
                Some(file) if file.to_ascii_lowercase().ends_with(".pdf") => file.clone(),
                Some(file) => format!("{file}.pdf"),
                None => format!("{stem}-{}.pdf", i + 1),
            };
            check_name(&name)?;
            Ok(name)
        })
        .collect();
    // Case-insensitive file systems (Windows, macOS) hold one of `a.pdf` and `A.PDF`.
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, name) in names.iter().enumerate() {
        if let Ok(name) = name {
            by_name.entry(name.to_lowercase()).or_default().push(i);
        }
    }
    for rows in by_name.values().filter(|rows| rows.len() > 1) {
        for &i in rows {
            let others: Vec<String> = rows.iter().filter(|&&j| j != i).map(|j| format!("row {}", j + 1)).collect();
            if let Some(name) = names.get_mut(i) {
                *name = Err(format!("the file name {:?} is also given by {}", name.as_deref().unwrap_or_default(), others.join(", ")));
            }
        }
    }
    names
}

/// Refuse a file name that could leave the output folder or that some file system cannot hold.
fn check_name(name: &str) -> RowResult<()> {
    let base = name.get(..name.len().saturating_sub(4)).unwrap_or_default();
    if base.trim().is_empty() {
        return Err("the file name is empty".into());
    }
    if name.len() > MAX_NAME {
        return Err(format!("the file name is longer than {MAX_NAME} bytes"));
    }
    if base == "." || base == ".." || name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
        return Err(format!("the file name {name:?} is . or .., or holds a path separator, control character or one of : * ? \" < > |"));
    }
    if base.ends_with(['.', ' ']) || name.starts_with(' ') {
        return Err(format!("the file name {name:?} starts with a space or ends its name with a space or dot"));
    }
    let device = base.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    if RESERVED.contains(&device.as_str()) {
        return Err(format!("the file name {name:?} is a reserved device name on Windows"));
    }
    Ok(())
}

/// Fill one row into a fresh copy of the form; the bytes to publish, or why the row failed.
fn fill(
    form_name: &str,
    bytes: &std::sync::Arc<Vec<u8>>,
    password: Option<&str>,
    javascript: bool,
    row: &Row,
    flatten: bool,
) -> RowResult<std::sync::Arc<Vec<u8>>> {
    if row.values.is_empty() {
        return Err("the row fills no field".into());
    }
    // A fresh session per row: no row sees another's values, and the caller's documents are untouched.
    let mut session = Session::new();
    session.set_javascript(javascript);
    let id = session.open(form_name, None, bytes.clone(), password).map_err(|e| format!("the form could not be copied: {e}"))?;
    for (name, cell) in &row.values {
        let value = {
            let doc = session.get(id).ok_or("the form copy closed")?;
            let field =
                doc.form.iter().find(|f| &f.name == name).ok_or_else(|| format!("there is no field named {name:?} in the form (see form_fields)"))?;
            let value = match cell {
                Cell::Text(text) => text_value(field, text)?,
                Cell::Json(value) => value.clone(),
            };
            value_for(field, &value).map_err(|e| e.to_string())?
        };
        session.apply(id, Edit::SetFieldValue { name: name.clone(), value }).map_err(|e| format!("{name}: {e}"))?;
    }
    if flatten {
        session.apply(id, Edit::Flatten { comments: false, fields: true }).map_err(|e| format!("flatten: {e}"))?;
    }
    let pdf = session.save_bytes(id).map_err(|e| e.to_string())?;
    session.check_save(id, &pdf).map_err(|e| e.to_string())?;
    Ok(pdf)
}

/// A CSV or tab-delimited cell as the value `form_fill` takes, read as `doc_export_data` writes it.
fn text_value(field: &FormField, text: &str) -> RowResult<Value> {
    Ok(match field.kind {
        FormFieldKind::CheckBox if text.is_empty() || text == "Off" => Value::Bool(false),
        FormFieldKind::CheckBox if field.state_for(text).is_some() => json!(text),
        FormFieldKind::CheckBox => {
            let states: Vec<&str> = field.widgets.iter().filter_map(|w| w.on_state.as_deref()).collect();
            return Err(format!(
                "{}: a check box takes an empty cell, Off or one of its on states ({}), not {text:?}",
                field.name,
                states.join(", ")
            ));
        }
        FormFieldKind::Radio if text.is_empty() || text == "Off" => Value::Null,
        // Several picks share a cell, joined by ", ", unless the whole cell is one option.
        FormFieldKind::List if field.has(field_flags::MULTI_SELECT) && !field.options.iter().any(|(export, _)| export == text) => {
            json!(text.split(", ").filter(|v| !v.is_empty()).collect::<Vec<_>>())
        }
        _ => json!(text),
    })
}
