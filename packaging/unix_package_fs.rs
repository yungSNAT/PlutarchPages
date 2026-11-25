// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Symlink-refusing filesystem, private temp dir, and subprocess helpers.

use super::{AppError, AppResult};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn reject_symlink(path: &Path, label: &str) -> AppResult<fs::Metadata> {
    let meta = fs::symlink_metadata(path).map_err(|_| AppError::runtime(format!("error: missing {label}: {}", path.display())))?;
    if meta.file_type().is_symlink() {
        return Err(AppError::runtime(format!("error: symlink refused for {label}: {}", path.display())));
    }
    Ok(meta)
}

fn validate_no_parent_segments(path: &Path) -> AppResult {
    if path.as_os_str().is_empty() {
        return Err(AppError::runtime("error: empty directory path"));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(AppError::runtime(format!("error: refusing '..' segment in directory path: {}", path.display())));
    }
    Ok(())
}

fn find_existing_ancestor(path: &Path) -> AppResult<(PathBuf, Vec<OsString>)> {
    let mut cur = path.to_path_buf();
    let mut missing = Vec::new();
    while !cur.as_os_str().is_empty() && fs::symlink_metadata(&cur).is_err() {
        if let Some(name) = cur.file_name() {
            missing.push(name.to_os_string());
        }
        cur.pop();
    }
    let anchor = if cur.as_os_str().is_empty() { PathBuf::from(".") } else { fs::canonicalize(&cur)? };
    Ok((anchor, missing))
}

fn ensure_real_leaf_dir(path: &Path) -> AppResult {
    match fs::symlink_metadata(path) {
        Ok(m) if !m.file_type().is_symlink() && m.is_dir() => Ok(()),
        Ok(_) => Err(AppError::runtime(format!("error: symlink or non-directory refused at {}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

// Resolves the deepest existing ancestor (e.g. macOS `/var -> private/var`) while still refusing an
// explicit symlink at the leaf path itself or symlinks under created segments.
pub fn ensure_real_dir_path(path: &Path) -> AppResult {
    validate_no_parent_segments(path)?;
    match fs::symlink_metadata(path) {
        Ok(m) if !m.file_type().is_symlink() && m.is_dir() => return Ok(()),
        Ok(_) => return Err(AppError::runtime(format!("error: symlink or non-directory refused at {}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let (mut real, missing) = find_existing_ancestor(path)?;
    for seg in missing.into_iter().rev() {
        real.push(seg);
        ensure_real_leaf_dir(&real)?;
    }
    Ok(())
}

pub fn reset_real_dir(path: &Path) -> AppResult {
    validate_no_parent_segments(path)?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        ensure_real_dir_path(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(AppError::runtime(format!("error: symlink or non-directory refused before reset: {}", path.display())));
            }
            fs::remove_dir_all(path)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    ensure_real_dir_path(path)
}

pub fn copy_file_checked(src: &Path, dst: &Path) -> AppResult {
    let meta = reject_symlink(src, "source file")?;
    if !meta.is_file() {
        return Err(AppError::runtime(format!("error: source is not a regular file: {}", src.display())));
    }
    if let Ok(dst_meta) = fs::symlink_metadata(dst)
        && dst_meta.file_type().is_symlink()
    {
        return Err(AppError::runtime(format!("error: symlink refused at destination: {}", dst.display())));
    }
    if let Some(parent) = dst.parent().filter(|p| !p.as_os_str().is_empty()) {
        ensure_real_dir_path(parent)?;
    }
    let _ = fs::copy(src, dst)?;
    Ok(())
}

pub fn install_file_mode(src: &Path, dst: &Path, mode: u32) -> AppResult {
    copy_file_checked(src, dst)?;
    set_unix_mode(dst, mode)?;
    Ok(())
}

#[cfg(unix)]
pub fn set_unix_mode(path: &Path, mode: u32) -> AppResult {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
pub fn set_unix_mode(_path: &Path, _mode: u32) -> AppResult {
    Ok(())
}

pub fn copy_dir_no_symlinks(src: &Path, dst: &Path) -> AppResult {
    let meta = reject_symlink(src, "source directory")?;
    if !meta.is_dir() {
        return Err(AppError::runtime(format!("error: source is not a directory: {}", src.display())));
    }
    ensure_real_dir_path(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            return Err(AppError::runtime(format!("error: symlink refused during copy: {}", entry.path().display())));
        }
        let target = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_no_symlinks(&entry.path(), &target)?;
        } else if ft.is_file() {
            copy_file_checked(&entry.path(), &target)?;
        } else {
            return Err(AppError::runtime(format!("error: unsupported entry refused: {}", entry.path().display())));
        }
    }
    Ok(())
}

pub fn create_private_temp_dir(base: &Path, prefix: &str) -> AppResult<PathBuf> {
    let base_meta = reject_symlink(base, "temporary base directory")?;
    if !base_meta.is_dir() {
        return Err(AppError::runtime(format!("error: temp base is not a directory: {}", base.display())));
    }
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let pid = std::process::id();
    for _ in 0..64 {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let candidate = base.join(format!("{prefix}{pid}-{nanos}-{seq}"));
        match create_dir_0700(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(AppError::runtime("error: failed to create unique temporary directory"))
}

#[cfg(unix)]
fn create_dir_0700(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_dir_0700(path: &Path) -> std::io::Result<()> {
    fs::create_dir(path)
}

pub fn write_private_file(path: &Path, data: &[u8]) -> AppResult {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(data)?;
    file.flush()?;
    Ok(())
}

#[cfg(unix)]
pub fn create_symlink_unix(target: &str, link: &Path) -> AppResult {
    if let Ok(meta) = fs::symlink_metadata(link) {
        if meta.is_dir() && !meta.file_type().is_symlink() {
            return Err(AppError::runtime(format!("error: refusing symlink over directory: {}", link.display())));
        }
        fs::remove_file(link)?;
    }
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}

#[cfg(not(unix))]
pub fn create_symlink_unix(_target: &str, _link: &Path) -> AppResult {
    Err(AppError::runtime("error: Unix symlinks require a Unix host"))
}

pub fn command_exists(program: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| {
        let candidate = dir.join(program);
        is_executable_file(&candidate)
    })
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.is_file() && (m.permissions().mode() & 0o111 != 0))
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

pub fn run_status(cmd: &mut Command, label: &str) -> AppResult {
    let status = cmd.stdin(Stdio::null()).status()?;
    if !status.success() {
        return Err(AppError::runtime(format!("error: {label} failed ({status})")));
    }
    Ok(())
}

pub fn run_capture(cmd: &mut Command, label: &str) -> AppResult<Output> {
    let out = cmd.stdin(Stdio::null()).output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !stderr.trim().is_empty() {
            eprint!("{stderr}");
        }
        return Err(AppError::runtime(format!("error: {label} failed ({})", out.status)));
    }
    Ok(out)
}

pub fn run_quiet(cmd: &mut Command) -> bool {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

pub fn strip_best_effort(prefix: &str, binaries: &[&Path]) {
    let strip_bin = if prefix.is_empty() { "strip".to_owned() } else { format!("{prefix}strip") };
    let _ = Command::new(&strip_bin).args(binaries).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

pub fn run_staged_binary(bin: &Path, args: &[&str], label: &str) -> AppResult {
    let meta = reject_symlink(bin, label)?;
    if !meta.is_file() {
        return Err(AppError::runtime(format!("error: missing executable file {label}: {}", bin.display())));
    }
    let canon = fs::canonicalize(bin)?;
    let mut cmd = Command::new(canon);
    cmd.args(args);
    run_status(&mut cmd, label)
}
