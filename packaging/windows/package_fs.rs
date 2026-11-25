// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Filesystem staging, symlink checks, and subprocess helpers for `package.rs`.
//!
//! Assumes packaging runs under a build-owner-controlled workspace (`target/` and `dist/` are not shared
//! untrusted temporary directories), reducing TOCTOU exposure between symlink checks and file operations.

use super::Result;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

pub fn reject_symlink(path: &Path, label: &str) -> Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path).map_err(|_| format!("Missing {label}: {}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!("Symlink refused for {label}: {}", path.display()).into());
    }
    Ok(meta)
}

pub fn ensure_real_dir_path(path: &Path) -> Result {
    let mut cur = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => cur.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => return Err(format!("Refusing '..' segment in directory path: {}", path.display()).into()),
            Component::Normal(seg) => {
                cur.push(seg);
                match fs::symlink_metadata(&cur) {
                    Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
                        return Err(format!("Symlink or non-directory refused at {}", cur.display()).into());
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&cur)?,
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }
    if cur.as_os_str().is_empty() {
        return Err("Empty directory path".into());
    }
    Ok(())
}

fn reject_parent_or_empty(path: &Path) -> Result {
    if path.as_os_str().is_empty() {
        return Err("Empty directory path".into());
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("Refusing '..' segment in directory path: {}", path.display()).into());
    }
    Ok(())
}

pub fn reset_real_dir(path: &Path) -> Result {
    reject_parent_or_empty(path)?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        ensure_real_dir_path(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(format!("Symlink or non-directory refused before clean reset: {}", path.display()).into());
            }
            fs::remove_dir_all(path)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    ensure_real_dir_path(path)
}

pub fn ensure_real_subdir(base: &Path, child: &str) -> Result<PathBuf> {
    let dir = base.join(child);
    if let Ok(meta) = fs::symlink_metadata(&dir) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(format!("Symlink or non-directory refused at {}", dir.display()).into());
        }
    } else {
        fs::create_dir(&dir)?;
    }
    Ok(dir)
}

pub fn copy_file_checked(src: &Path, dst: &Path) -> Result {
    let meta = reject_symlink(src, "source file")?;
    if !meta.is_file() {
        return Err(format!("Source is not a regular file: {}", src.display()).into());
    }
    if let Ok(dst_meta) = fs::symlink_metadata(dst)
        && dst_meta.file_type().is_symlink()
    {
        return Err(format!("Refusing to overwrite symlink destination: {}", dst.display()).into());
    }
    let _ = fs::copy(src, dst)?;
    Ok(())
}

pub fn copy_dir_no_symlinks(src: &Path, dst: &Path) -> Result {
    let src_meta = reject_symlink(src, "source directory")?;
    if !src_meta.is_dir() {
        return Err(format!("Source is not a directory: {}", src.display()).into());
    }
    ensure_real_dir_path(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            return Err(format!("Symlink refused during copy: {}", entry.path().display()).into());
        }
        let target = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_no_symlinks(&entry.path(), &target)?;
        } else if ft.is_file() {
            copy_file_checked(&entry.path(), &target)?;
        } else {
            return Err(format!("Unsupported filesystem entry refused during copy: {}", entry.path().display()).into());
        }
    }
    Ok(())
}

pub fn allowlisted_ps_bin(shell: &str) -> Result<&'static str> {
    match Path::new(shell).file_stem().and_then(|s| s.to_str()) {
        Some(s) if s.eq_ignore_ascii_case("pwsh") => Ok("pwsh"),
        Some(s) if s.eq_ignore_ascii_case("powershell") => Ok("powershell"),
        _ => Err(format!("Unsupported PowerShell binary: {shell}").into()),
    }
}

pub fn run_ps_script(shell_bin: &str, script: &Path, extra_args: &[&str]) -> Result {
    let status = Command::new(shell_bin)
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(script)
        .args(extra_args)
        .stdin(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(format!("PowerShell script failed ({status}): {}", script.display()).into());
    }
    Ok(())
}

pub fn run_status(cmd: &mut Command, label: &str) -> Result {
    let status = cmd.stdin(Stdio::null()).status()?;
    if !status.success() {
        return Err(format!("{label} failed ({status})").into());
    }
    Ok(())
}
