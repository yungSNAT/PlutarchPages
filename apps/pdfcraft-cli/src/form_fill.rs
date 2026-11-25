// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `form-fill-batch`: friendly flags for the `form_fill_batch` tool that `run` and MCP expose.

use serde_json::json;

use crate::{CliError, OptionSpec, automation, flag, print_output, validate_options};

pub(crate) const OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--data", takes_value: true },
    OptionSpec { name: "--out-dir", takes_value: true },
    OptionSpec { name: "--name-column", takes_value: true },
    OptionSpec { name: "--flatten", takes_value: false },
    OptionSpec { name: "--password", takes_value: true },
    OptionSpec { name: "--root", takes_value: true },
];

pub(super) fn run(args: &[String]) -> Result<(), CliError> {
    validate_options("form-fill-batch", args, OPTIONS)?;
    let mut forms = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if let Some(spec) = OPTIONS.iter().find(|s| s.name == arg) {
            if !seen.insert(spec.name) {
                return Err(format!("form-fill-batch: {} may only be supplied once", spec.name).into());
            }
            if spec.takes_value {
                // A password or column name may be anything; a file or folder is never another option.
                let path = matches!(spec.name, "--data" | "--out-dir" | "--root");
                if args.get(i + 1).is_none_or(|v| v.is_empty() || (path && v.starts_with("--"))) {
                    return Err(format!("form-fill-batch: {} needs a value", spec.name).into());
                }
                i += 1;
            }
        } else {
            forms.push(arg.as_str());
        }
        i += 1;
    }
    let [form] = forms.as_slice() else { return Err("form-fill-batch: name one form PDF".into()) };
    let data = flag(args, "--data").ok_or("form-fill-batch: missing --data (a CSV, tab-delimited or JSON file)")?;
    let out = flag(args, "--out-dir").ok_or("form-fill-batch: missing --out-dir")?;
    let mut auto = automation(args)?;
    let mut open = json!({ "path": form });
    if let Some(password) = flag(args, "--password") {
        open["password"] = json!(password);
    }
    let opened = auto.call("doc_open", &open).map_err(|e| format!("form-fill-batch: {form}: {e}"))?;
    let doc = opened
        .iter()
        .find_map(|c| match c {
            pdfcraft_automation::Content::Json(v) => v["doc"].as_u64(),
            _ => None,
        })
        .ok_or("form-fill-batch: the form did not open")?;
    let mut params = json!({ "doc": doc, "data": data, "out_dir": out, "flatten": seen.contains("--flatten") });
    if let Some(column) = flag(args, "--name-column") {
        params["name_column"] = json!(column);
    }
    let content = auto.call("form_fill_batch", &params).map_err(|e| e.to_string())?;
    let failed = content.iter().any(|c| matches!(c, pdfcraft_automation::Content::Json(v) if v["failed"].as_u64().is_some_and(|n| n > 0)));
    print_output(&auto, content, None)?;
    if failed { Err("form-fill-batch: one or more rows failed; see the JSON report".into()) } else { Ok(()) }
}
