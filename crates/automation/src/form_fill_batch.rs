// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `form_fill_batch`: one filled copy of an open form for each row of a CSV, tab-delimited or
//! JSON data file. A row is all or nothing: nothing is written for a row the form refuses a value
//! of. Every output name is checked before the first file is written. Rows may hold personal data,
//! so errors name the row and field, never a cell's value, unless the caller asks for `verbose`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use pdfcraft_engine::form_scripts::Calculate;
use pdfcraft_engine::{Edit, FieldValue, FormField, FormFieldKind, Session, field_flags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization as _;

use crate::batch::{absolute, publish};
use crate::forms::value_for;
use crate::{Args, Automation, Result, failed};

/// Most rows one call fills.
const MAX_ROWS: usize = 10_000;

/// Largest data file read. 10,000 rows rarely need more, and the cap bounds what parsing holds.
const MAX_DATA: u64 = 32 * 1024 * 1024;

/// Longest output file name, in bytes: well inside every common file system's 255.
const MAX_NAME: usize = 200;

/// Windows device names, which name no file there whatever the extension.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "COM¹", "COM²",
    "COM³", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
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
        let verbose = args.opt_bool("verbose")?.unwrap_or(false);
        let data = self.resolve(args.str("data")?, false)?;
        let rows = read_rows(&read_data(&data)?, name_column)?;
        let out = absolute(&self.resolve(args.str("out_dir")?, true)?)?;
        // The form as it stands, unsaved edits included; the open document is not changed.
        let bytes = self.session.save_bytes(id).map_err(failed)?;
        let javascript = self.session.javascript();
        let stem = Path::new(&form_name).file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| check_name(&format!("{s}.pdf")).is_ok());
        let stem = stem.unwrap_or_else(|| "form".into());
        let names = output_names(&rows, &stem, verbose);

        let (mut completed, mut failed_rows) = (0, 0);
        let mut report = Vec::with_capacity(rows.len());
        for (i, (row, name)) in rows.into_iter().zip(names).enumerate() {
            let result: RowResult<_> = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let (row, name) = (row?, name?);
                let pdf = fill(&form_name, &bytes, password.as_deref(), javascript, &row, flatten, verbose)?;
                let target = out.join(&name);
                let target = self.resolve(target.to_str().ok_or("the output path is not UTF-8")?, true).map_err(|e| e.to_string())?;
                if !target.starts_with(&out) {
                    return Err("the output path leaves out_dir".to_string());
                }
                let written = publish(&target, &pdf).map_err(|e| e.to_string())?;
                // A receipt: the SHA-256 of the bytes written, to check the file against later.
                let sha256: String = Sha256::digest(pdf.as_slice()).iter().map(|b| format!("{b:02x}")).collect();
                Ok((written, pdf.len(), sha256))
            }))
            .unwrap_or_else(|_| Err("filling this row panicked; no file was written for it".into()));
            report.push(match result {
                Ok((written, size, sha256)) => {
                    completed += 1;
                    json!({ "row": i + 1, "status": "ok", "output": written.to_string_lossy(), "bytes": size, "sha256": sha256 })
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

/// The data file as text, refused unread when it is larger than [`MAX_DATA`].
fn read_data(path: &Path) -> Result<String> {
    use std::io::Read as _;
    let too_big = || failed(format!("the data file is larger than {} MiB", MAX_DATA >> 20));
    let cannot = |e: std::io::Error| failed(format!("{}: {e}", path.display()));
    let file = std::fs::File::open(path).map_err(cannot)?;
    if file.metadata().is_ok_and(|m| m.len() > MAX_DATA) {
        return Err(too_big());
    }
    let mut bytes = Vec::new();
    file.take(MAX_DATA + 1).read_to_end(&mut bytes).map_err(cannot)?;
    if bytes.len() as u64 > MAX_DATA {
        return Err(too_big());
    }
    String::from_utf8(bytes).map_err(|_| failed("the data file is not UTF-8 text (save it as UTF-8)"))
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

fn too_many() -> crate::ToolError {
    failed(format!("the data file has more than {MAX_ROWS} rows, the most one call fills"))
}

fn table_rows(text: &str, name_column: Option<&str>) -> Result<Vec<RowResult<Row>>> {
    // The names row and one row past the limit: enough to tell the file is too long.
    let mut records = pdfcraft_engine::table_records(text, MAX_ROWS + 2).into_iter();
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
    if records.len() > MAX_ROWS {
        return Err(too_many());
    }
    Ok(records
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

/// The items of a JSON array, parsed one at a time and stopped one past [`MAX_ROWS`].
struct Items;

impl<'de> serde::de::Visitor<'de> for Items {
    type Value = Vec<Value>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an array of rows")
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Vec<Value>, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element::<Value>()? {
            if items.len() == MAX_ROWS {
                return Err(serde::de::Error::custom("too many rows"));
            }
            items.push(item);
        }
        Ok(items)
    }
}

fn json_rows(text: &str, name_column: Option<&str>) -> Result<Vec<RowResult<Row>>> {
    if text.starts_with('{') {
        return Err(failed("JSON data must be an array of objects, one per row"));
    }
    let mut de = serde_json::Deserializer::from_str(text);
    let items = serde::Deserializer::deserialize_seq(&mut de, Items).and_then(|items| de.end().map(|()| items)).map_err(|e| {
        // The visitor's only error is the row limit; anything else is the JSON itself.
        if e.is_data() { too_many() } else { failed(format!("the data file is not valid JSON: {e}")) }
    })?;
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
fn output_names(rows: &[RowResult<Row>], stem: &str, verbose: bool) -> Vec<RowResult<String>> {
    let mut names: Vec<RowResult<String>> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let row = row.as_ref().map_err(Clone::clone)?;
            let name = match &row.file {
                Some(file) if file.to_lowercase().ends_with(".pdf") => file.clone(),
                Some(file) => format!("{file}.pdf"),
                None => format!("{stem}-{}.pdf", i + 1),
            };
            check_name(&name).map_err(|e| if verbose { format!("{e}: {name:?}") } else { e })?;
            Ok(name)
        })
        .collect();
    // Case-insensitive file systems (Windows, macOS) hold one of `a.pdf` and `A.PDF`, and macOS one
    // of `café.pdf` composed and decomposed: names are compared case-folded and decomposed.
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, name) in names.iter().enumerate() {
        if let Ok(name) = name {
            by_name.entry(name.to_lowercase().nfd().collect()).or_default().push(i);
        }
    }
    for rows in by_name.values().filter(|rows| rows.len() > 1) {
        for &i in rows {
            let others: Vec<String> = rows.iter().filter(|&&j| j != i).map(|j| format!("row {}", j + 1)).collect();
            if let Some(name) = names.get_mut(i) {
                let shown = if verbose { format!(" {:?}", name.as_deref().unwrap_or_default()) } else { String::new() };
                *name = Err(format!("the file name{shown} is also given by {}", others.join(", ")));
            }
        }
    }
    names
}

/// Characters that change how a name displays without showing: bidirectional controls,
/// zero-width characters, Hangul fillers, tag characters and the like, with which one name can
/// display as another.
fn hidden(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{115F}' | '\u{1160}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}' | '\u{3164}' | '\u{FEFF}' | '\u{FFA0}' | '\u{FFF9}'..='\u{FFFB}' | '\u{E0000}'..='\u{E007F}'
    )
}

/// Refuse a file name that could leave the output folder, that some file system cannot hold or
/// that would display as another name. The reason never quotes the name.
fn check_name(name: &str) -> RowResult<()> {
    let base = name.get(..name.len().saturating_sub(4)).unwrap_or_default();
    if base.trim().is_empty() {
        return Err("the file name is empty".into());
    }
    if name.len() > MAX_NAME {
        return Err(format!("the file name is longer than {MAX_NAME} bytes"));
    }
    if base == "." || base == ".." || name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
        return Err("the file name is . or .., or holds a path separator, control character or one of : * ? \" < > |".into());
    }
    if name.chars().any(hidden) {
        return Err("the file name holds an invisible or direction-changing character".into());
    }
    if base.ends_with(['.', ' ']) || name.starts_with(' ') {
        return Err("the file name starts with a space or ends its name with a space or dot".into());
    }
    let device = base.split('.').next().unwrap_or_default().trim_end().to_uppercase();
    if RESERVED.contains(&device.as_str()) {
        return Err("the file name is a reserved device name on Windows".into());
    }
    Ok(())
}

/// Why the form refused a row's value for `name`: the form's reason, which may quote the value,
/// only when the caller asked for `verbose`.
fn refused(name: &str, reason: impl std::fmt::Display, verbose: bool) -> String {
    if verbose { format!("{name}: {reason}") } else { format!("{name}: the form refuses this value (verbose shows why)") }
}

/// Whether the form calculates the field's value from others.
fn is_calculated(field: &FormField) -> bool {
    !matches!(field.actions.calculate, Calculate::None) || field.actions.scripts.calculate.is_some()
}

/// Fill one row into a fresh copy of the form; the bytes to publish, or why the row failed.
fn fill(
    form_name: &str,
    bytes: &std::sync::Arc<Vec<u8>>,
    password: Option<&str>,
    javascript: bool,
    row: &Row,
    flatten: bool,
    verbose: bool,
) -> RowResult<std::sync::Arc<Vec<u8>>> {
    if row.values.is_empty() {
        return Err("the row fills no field".into());
    }
    // A fresh session per row: no row sees another's values, and the caller's documents are untouched.
    let mut session = Session::new();
    session.set_javascript(javascript);
    let id = session.open(form_name, None, bytes.clone(), password).map_err(|e| format!("the form could not be copied: {e}"))?;
    // Values the row gives calculated fields: a later edit's calculation may replace them.
    let mut calculated = Vec::new();
    for (name, cell) in &row.values {
        let value = {
            let doc = session.get(id).ok_or("the form copy closed")?;
            let field =
                doc.form.iter().find(|f| &f.name == name).ok_or_else(|| format!("there is no field named {name:?} in the form (see form_fields)"))?;
            let value = match cell {
                Cell::Text(text) => text_value(field, text, verbose)?,
                Cell::Json(value) => value.clone(),
            };
            let value = value_for(field, &value).map_err(|e| refused(name, e, verbose))?;
            // An empty value gives the calculation nothing to disagree with.
            if let FieldValue::Text(text) = &value
                && !text.is_empty()
                && is_calculated(field)
            {
                calculated.push((name, text.clone()));
            }
            value
        };
        session.apply(id, Edit::SetFieldValue { name: name.clone(), value }).map_err(|e| refused(name, e, verbose))?;
    }
    // A row that gives a calculated field a value the form then calculates differently is refused,
    // not written with a value it did not give. A value the calculation agrees with passes.
    let doc = session.get(id).ok_or("the form copy closed")?;
    for (name, given) in calculated {
        let got = doc.form.iter().find(|f| &f.name == name).and_then(|f| f.value.first()).map_or("", String::as_str);
        if got != given {
            let shown = if verbose { format!(" ({got:?}, not {given:?})") } else { String::new() };
            return Err(format!("{name}: the form calculates this field, and its result differs from the row's value{shown}"));
        }
    }
    if flatten {
        session.apply(id, Edit::Flatten { comments: false, fields: true }).map_err(|e| format!("flatten: {e}"))?;
    }
    let pdf = session.save_bytes(id).map_err(|e| e.to_string())?;
    session.check_save(id, &pdf).map_err(|e| e.to_string())?;
    Ok(pdf)
}

/// A CSV or tab-delimited cell as the value `form_fill` takes, read as `doc_export_data` writes it.
fn text_value(field: &FormField, text: &str, verbose: bool) -> RowResult<Value> {
    Ok(match field.kind {
        FormFieldKind::CheckBox if text.is_empty() || text == "Off" => Value::Bool(false),
        FormFieldKind::CheckBox if field.state_for(text).is_some() => json!(text),
        FormFieldKind::CheckBox => {
            let states: Vec<&str> = field.widgets.iter().filter_map(|w| w.on_state.as_deref()).collect();
            let shown = if verbose { format!(", not {text:?}") } else { String::new() };
            return Err(format!("{}: a check box takes an empty cell, Off or one of its on states ({}){shown}", field.name, states.join(", ")));
        }
        FormFieldKind::Radio if text.is_empty() || text == "Off" => Value::Null,
        // Several picks share a cell, joined by ", ", unless the whole cell is one option.
        FormFieldKind::List if field.has(field_flags::MULTI_SELECT) && !field.options.iter().any(|(export, _)| export == text) => {
            json!(text.split(", ").filter(|v| !v.is_empty()).collect::<Vec<_>>())
        }
        _ => json!(text),
    })
}
