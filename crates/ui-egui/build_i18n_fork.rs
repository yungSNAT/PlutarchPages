// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' interface rows that craft-i18n doesn't have: `i18n-fork/<locale>.tsv`, in
//! craft-i18n's catalog format. build.rs embeds each locale's overlay rows followed by
//! craft-i18n's `pdfcraft.tsv`, joined in `OUT_DIR`, in place of that file (PATCHES.md). A row
//! craft-i18n gains is then defined twice, which `I18n::check_all` reports and the
//! `the_fork_overlay_is_embedded_and_served` test fails on: delete it from the overlay then.

use std::path::{Path, PathBuf};

fn overlays() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("i18n-fork")
}

/// The catalog to embed for `code`: craft-i18n's `app` with the overlay's rows before it, or
/// `app` itself when the locale has no overlay.
pub fn app_catalog(code: &str, app: PathBuf) -> Result<PathBuf, String> {
    let overlay = overlays().join(format!("{code}.tsv"));
    if !overlay.is_file() {
        return Ok(app);
    }
    let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    // A byte-order mark is read only at the start of a catalog: joined, one would spoil the row
    // after it. Line endings may differ between the two (a Windows checkout); the reader takes both.
    let without_bom = |text: String| text.strip_prefix('\u{feff}').map(str::to_owned).unwrap_or(text);
    let mut text = without_bom(read(&overlay)?);
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&without_bom(read(&app)?));
    let dir = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR is unset")?).join("i18n-fork");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let joined = dir.join(format!("{code}.tsv"));
    std::fs::write(&joined, text).map_err(|e| format!("{}: {e}", joined.display()))?;
    Ok(joined)
}

/// The overlays `ATTRIBUTION.toml` lists, each an `[[asset]]`, as paths from the workspace root.
fn listed(root: &Path) -> Result<Vec<String>, String> {
    let file = root.join("ATTRIBUTION.toml");
    println!("cargo::rerun-if-changed={}", file.display());
    let table: toml::Table = std::fs::read_to_string(&file)
        .map_err(|e| e.to_string())
        .and_then(|t| t.parse::<toml::Table>().map_err(|e| e.to_string()))
        .map_err(|e| format!("{}: {e}", file.display()))?;
    let assets = table.get("asset").and_then(toml::Value::as_array).map_or(&[][..], Vec::as_slice);
    Ok(assets.iter().filter_map(|a| a.get("path")?.as_str()).filter(|p| p.starts_with("crates/ui-egui/i18n-fork/")).map(str::to_owned).collect())
}

/// Report overlays `ATTRIBUTION.toml` lists that are missing, and overlay locales craft-i18n has
/// no catalog for (`embedded`): either way their rows would be lost. An error under
/// `CRAFT_I18N_REQUIRED=1` (`required`), else a warning.
pub fn check_locales(embedded: &[&str], required: bool) {
    let level = if required { "error" } else { "warning" };
    // The directory, so an overlay added or removed reruns the script too.
    println!("cargo::rerun-if-changed={}", overlays().display());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).expect("crates/ui-egui is two levels down");
    match listed(root) {
        Ok(paths) => {
            let missing: Vec<&str> = paths.iter().filter(|p| !root.join(p).is_file()).map(String::as_str).collect();
            if !missing.is_empty() {
                println!("cargo::{level}=ATTRIBUTION.toml lists overlays that are missing: {}", missing.join(", "));
            }
        }
        Err(e) => println!("cargo::{level}={e}"),
    }
    let Ok(list) = std::fs::read_dir(overlays()) else { return };
    let mut lost: Vec<String> = list
        .flatten()
        .filter_map(|e| e.path().file_name()?.to_str()?.strip_suffix(".tsv").map(str::to_owned))
        .filter(|code| !embedded.contains(&code.as_str()))
        .collect();
    if !lost.is_empty() {
        lost.sort();
        println!("cargo::{level}=crates/ui-egui/i18n-fork has rows for {}, which CRAFT_I18N_DIR has no pdfcraft.tsv for", lost.join(", "));
    }
}
