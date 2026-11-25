// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Standalone Windows packaging validation helpers and tests (`rustc --edition=2024 -D warnings`).
//!
//! Subcommands:
//!   win_checks font-notices <font-root> [--require-chinese]
//!   win_checks assert-notice-names --expected-count <n> [expected...] [actual...]
//!   win_checks test-font-notices [craft-fonts-root]
//!   win_checks test-install-policy --output-dir <dir> [--arch <x64|x86|arm64>]... [--shell <pwsh>]

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

mod win_checks_json;
mod win_checks_policy;
mod win_checks_tests;

use std::collections::{BTreeMap, HashSet};
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

pub type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

pub fn strip_windows_verbatim(path: PathBuf) -> PathBuf {
    let Some(raw) = path.to_str() else {
        return path;
    };
    if let Some(unc) = raw.strip_prefix("\\\\?\\UNC\\") {
        return PathBuf::from(format!("\\\\{unc}"));
    }
    if let Some(local) = raw.strip_prefix("\\\\?\\")
        && local.as_bytes().get(1) == Some(&b':')
        && local.len() < 260
    {
        return PathBuf::from(local);
    }
    path
}

fn is_valid_family(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn join_rel(root: &Path, rel_slash: &str) -> Result<PathBuf> {
    let mut out = root.to_path_buf();
    for seg in rel_slash.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.contains('\\') || seg.contains(':') {
            return Err(format!("Invalid relative path: {rel_slash}").into());
        }
        out.push(seg);
    }
    Ok(out)
}

fn reject_symlink(path: &Path, label: &str) -> Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path)
        .map_err(|_| format!("Missing {label}: {}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!("Symlink refused for {label}: {}", path.display()).into());
    }
    Ok(meta)
}

fn parse_font_notice_row(root_canon: &Path, manifest: &Path, line: &str) -> Result<(String, PathBuf)> {
    let fields: Vec<&str> = line.split(" | ").map(str::trim).collect();
    if fields.len() < 6 || fields[4] != "OFL-1.1" {
        return Err(format!("Missing OFL metadata in {}: {line}", manifest.display()).into());
    }
    let relative = fields[5].replace('\\', "/");
    let family = relative
        .strip_prefix("fonts/")
        .and_then(|rest| rest.strip_suffix("/OFL.txt"))
        .filter(|fam| is_valid_family(fam))
        .ok_or_else(|| format!("Invalid font notice path: {relative}"))?;
    let font_path = fields[2].replace('\\', "/");
    let prefix = format!("fonts/{family}/");
    if font_path
        .strip_prefix(&prefix)
        .filter(|r| !r.is_empty() && !r.contains('/'))
        .is_none()
    {
        return Err(format!(
            "Font and notice must belong to the same family directory: {font_path} / {relative}"
        )
        .into());
    }
    let _ = reject_symlink(&root_canon.join("fonts"), "fonts directory")?;
    let _ = reject_symlink(&root_canon.join("fonts").join(family), "font family directory")?;
    let notice = join_rel(root_canon, &relative)?;
    let meta = reject_symlink(&notice, "OFL notice").map_err(|_| {
        format!("Missing, symlinked or empty OFL notice for {}: {}", fields[0], notice.display())
    })?;
    if !meta.is_file() || meta.len() == 0 {
        return Err(format!("Missing or empty OFL notice for {}: {}", fields[0], notice.display()).into());
    }
    let notice_canon = fs::canonicalize(&notice)?;
    if !notice_canon.starts_with(root_canon) {
        return Err(format!("OFL notice escapes font root: {}", notice.display()).into());
    }
    Ok((family.to_owned(), strip_windows_verbatim(notice_canon)))
}

fn read_manifest_lines(root_canon: &Path, manifest: &Path, required: bool) -> Result<Option<String>> {
    match fs::symlink_metadata(manifest) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_file() {
                return Err(format!("Symlink or non-file refused for font manifest: {}", manifest.display()).into());
            }
            let canon = fs::canonicalize(manifest)?;
            if !canon.starts_with(root_canon) {
                return Err(format!("Font manifest escapes root: {}", manifest.display()).into());
            }
            Ok(Some(std::io::read_to_string(fs::File::open(&canon)?)?))
        }
        Err(_) if !required => Ok(None),
        Err(_) => Err(format!("Missing font manifest: {}", manifest.display()).into()),
    }
}

/// Validates declared native font manifests and returns sorted notice file paths.
pub fn collect_font_notices(font_root: &Path, require_chinese: bool) -> Result<Vec<PathBuf>> {
    let root_canon = fs::canonicalize(font_root)
        .map_err(|e| format!("Cannot resolve font root {}: {e}", font_root.display()))?;
    let fonts_dir = root_canon.join("fonts");
    let _ = reject_symlink(&fonts_dir, "fonts directory")?;
    let mut notices: BTreeMap<String, PathBuf> = BTreeMap::new();
    for name in ["manifest.txt", "simplified-chinese-manifest.txt"] {
        let manifest = fonts_dir.join(name);
        let required = name == "manifest.txt" || require_chinese;
        let Some(text) = read_manifest_lines(&root_canon, &manifest, required)? else {
            continue;
        };
        for raw in text.lines() {
            let line = raw.trim();
            if !line.is_empty() && !line.starts_with('#') {
                let (family, notice) = parse_font_notice_row(&root_canon, &manifest, line)?;
                notices.insert(family, notice);
            }
        }
    }
    if notices.is_empty() {
        return Err(format!("No declared font notices in {}", root_canon.display()).into());
    }
    Ok(notices.into_values().collect())
}

pub fn assert_notice_names(expected: &[String], actual: &[String]) -> Result {
    let names: HashSet<&str> = actual
        .iter()
        .map(|s| s.rsplit('|').next().unwrap_or(s.as_str()))
        .collect();
    for exp in expected {
        if !names.contains(exp.as_str()) {
            return Err(format!("The MSI is missing staged font notice: {exp}").into());
        }
    }
    Ok(())
}

pub fn parse_notice_names_args(rest: &[String]) -> Result<(Vec<String>, Vec<String>)> {
    if rest.first().map(String::as_str) == Some("--expected-count") {
        let count_str = rest.get(1).ok_or("Missing count after --expected-count")?;
        let n: usize = count_str.parse().map_err(|_| format!("Invalid count `{count_str}`"))?;
        let items = &rest[2..];
        if items.len() < n {
            return Err(format!("Expected {n} expected notice names, got {}", items.len()).into());
        }
        return Ok((items[..n].to_vec(), items[n..].to_vec()));
    }
    Err("usage: win_checks assert-notice-names --expected-count <n> [expected...] [actual...]".into())
}

fn run_cli(args: &[String]) -> Result {
    let Some((cmd, rest)) = args.split_first() else {
        return Err("usage: win_checks <font-notices|assert-notice-names|test-font-notices|test-install-policy> ...".into());
    };
    match cmd.as_str() {
        "font-notices" => {
            let root = rest.first().ok_or("usage: font-notices <root> [--require-chinese]")?;
            let require_zh = rest.iter().any(|a| a == "--require-chinese");
            for path in collect_font_notices(Path::new(root), require_zh)? {
                println!("{}", path.display());
            }
            Ok(())
        }
        "assert-notice-names" => {
            let (expected, actual) = parse_notice_names_args(rest)?;
            assert_notice_names(&expected, &actual)
        }
        "test-font-notices" => {
            win_checks_tests::run_font_notice_regressions(rest.first().filter(|s| !s.is_empty()).map(Path::new))
        }
        "test-install-policy" => win_checks_policy::run_install_policy_cmd(rest),
        other => Err(format!("Unknown subcommand `{other}`").into()),
    }
}

fn main() {
    // nosemgrep: rust.lang.security.args.args -- CLI subcommand dispatcher for build tooling
    let args: Vec<String> = env::args().skip(1).collect();
    if let Err(err) = run_cli(&args) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
