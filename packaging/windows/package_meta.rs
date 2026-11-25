// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration, version, path normalisation, and build metadata helpers for `package.rs`.

use super::Result;
use std::env;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct ArchSpec {
    pub arch: &'static str,
    pub target: &'static str,
    pub rustflags_env: &'static str,
    pub machine: u16,
}

pub fn validate_flags(skip_build: bool, built_without_craft_fonts: bool, has_fonts_dir: bool) -> Result {
    if built_without_craft_fonts && !skip_build {
        return Err("-BuiltWithoutCraftFonts requires -SkipBuild".into());
    }
    if built_without_craft_fonts && has_fonts_dir {
        return Err("-BuiltWithoutCraftFonts cannot be combined with CRAFT_FONTS_DIR".into());
    }
    if skip_build && !has_fonts_dir && !built_without_craft_fonts {
        return Err("-SkipBuild requires CRAFT_FONTS_DIR or -BuiltWithoutCraftFonts".into());
    }
    Ok(())
}

pub fn arch_spec(arch: &str) -> Result<ArchSpec> {
    match arch {
        "x64" => Ok(ArchSpec {
            arch: "x64",
            target: "x86_64-pc-windows-msvc",
            rustflags_env: "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS",
            machine: 0x8664,
        }),
        "x86" => Ok(ArchSpec {
            arch: "x86",
            target: "i686-pc-windows-msvc",
            rustflags_env: "CARGO_TARGET_I686_PC_WINDOWS_MSVC_RUSTFLAGS",
            machine: 0x014C,
        }),
        "arm64" => Ok(ArchSpec {
            arch: "arm64",
            target: "aarch64-pc-windows-msvc",
            rustflags_env: "CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_RUSTFLAGS",
            machine: 0xAA64,
        }),
        other => Err(format!("Unsupported architecture: {other}").into()),
    }
}

fn is_valid_version_token(raw: &str) -> bool {
    !raw.is_empty()
        && !raw.starts_with(['.', '-', '+'])
        && !raw.ends_with(['.', '-', '+'])
        && !raw.contains("..")
        && raw.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
}

pub fn resolve_version(cargo_toml: &str, env_ver: Option<&str>) -> Result<(String, String)> {
    let raw = match env_ver.map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) => v.to_owned(),
        None => parse_workspace_version(cargo_toml)?,
    };
    if !is_valid_version_token(&raw) {
        return Err(format!("Invalid version string `{raw}`").into());
    }
    let msi = raw.split(['-', '+']).next().unwrap_or(&raw).trim().to_owned();
    if msi.is_empty() || !msi.split('.').all(|seg| !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit())) {
        return Err(format!("Invalid MSI version derived from `{raw}`").into());
    }
    Ok((raw, msi))
}

fn unquote_toml_str(raw: &str) -> Option<&str> {
    let no_comment = raw.split('#').next()?.trim();
    no_comment
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| no_comment.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn normalize_lexical(path: &Path) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    let mut depth: usize = 0;
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return Err(format!("Directory path escapes root with '..': {}", path.display()).into());
                }
                let _ = out.pop();
                depth -= 1;
            }
            Component::Normal(seg) => {
                out.push(seg);
                depth += 1;
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(format!("Empty directory path from {}", path.display()).into());
    }
    Ok(out)
}

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

pub fn resolve_dir_path(raw: PathBuf, cwd: &Path) -> Result<PathBuf> {
    let clean = strip_windows_verbatim(raw);
    let joined = if clean.is_absolute() { clean } else { cwd.join(clean) };
    Ok(strip_windows_verbatim(normalize_lexical(&joined)?))
}

fn parse_workspace_version(cargo_toml: &str) -> Result<String> {
    let mut in_pkg = false;
    for line in cargo_toml.lines().map(str::trim) {
        if line.starts_with('[') {
            in_pkg = line == "[workspace.package]";
            continue;
        }
        if in_pkg
            && let Some((key, val)) = line.split_once('=')
            && key.trim() == "version"
            && let Some(ver) = unquote_toml_str(val)
        {
            return Ok(ver.to_owned());
        }
    }
    Err("Failed to resolve [workspace.package].version from Cargo.toml".into())
}

// Standalone rustc helper has no external crates (`chrono`), so convert Unix epoch seconds to UTC date in pure std.
pub fn utc_date_from_unix_secs(secs: u64) -> Result<String> {
    let days_u = secs / 86_400;
    let days = i64::try_from(days_u).map_err(|_| "Unix timestamp overflow")?;
    let z = days.checked_add(719_468).ok_or("Civil date overflow")?;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + i64::from(m <= 2);
    Ok(format!("{year:04}-{m:02}-{d:02}"))
}

pub fn non_empty_env(name: &str) -> Option<String> {
    env::var(name).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

pub fn resolve_build_meta(root: &Path) -> Result<(String, String)> {
    let sha = match non_empty_env("PDFCRAFT_BUILD_SHA") {
        Some(v) => v,
        None => Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .stdin(Stdio::null())
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| s.trim().to_owned())
            .unwrap_or_default(),
    };
    let date = match non_empty_env("PDFCRAFT_BUILD_DATE") {
        Some(v) => v,
        None => utc_date_from_unix_secs(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?,
    };
    Ok((sha, date))
}
