// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Browser/Wasm packaging (`packaging/web/package.sh` port).

use super::unix_package_env::EnvCtx;
use super::unix_package_fs::{
    command_exists, copy_dir_no_symlinks, copy_file_checked, ensure_real_dir_path, reject_symlink, reset_real_dir, run_status,
};
use super::{AppError, AppResult};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const HELP: &str = "\
Build the browser version and zip it:  $DIST/pdfcraft-web-<version>.zip

Usage: packaging/web/package.sh [--skip-build]

Needs: trunk (brew install trunk / cargo install trunk --locked), the wasm32-unknown-unknown
target and network access for the OCR models (cargo xtask models).";

pub fn has_root_absolute_url(html: &str) -> bool {
    let bytes = html.as_bytes();
    for attr in [b"src=\"/".as_slice(), b"href=\"/".as_slice()] {
        let mut idx = 0;
        while let Some(pos) = bytes[idx..].windows(attr.len()).position(|w| w == attr) {
            let after = idx + pos + attr.len();
            if bytes.get(after).is_some_and(|&b| b != b'/') {
                return true;
            }
            idx = idx + pos + 1;
        }
    }
    false
}

fn parse_web_args(args: &[String]) -> AppResult<Option<bool>> {
    match args {
        [] => Ok(Some(false)),
        [flag] if flag == "--skip-build" => Ok(Some(true)),
        [flag] if flag == "-h" || flag == "--help" => Ok(None),
        [first, ..] => Err(AppError::usage(format!("unknown argument: {first}"))),
    }
}

fn validate_web_site(env: &EnvCtx, skip_build: bool) -> AppResult<PathBuf> {
    if !skip_build {
        if !command_exists("trunk") {
            return Err(AppError::runtime("error: trunk not found (cargo install trunk --locked)"));
        }
        let mut cmd = Command::new("trunk");
        cmd.args(["build", "--release"]).current_dir(env.root.join("apps/pdfcraft-web"));
        run_status(&mut cmd, "trunk build")?;
    }
    let site = env.root.join("dist/web");
    let index = site.join("index.html");
    if !index.is_file() {
        return Err(AppError::runtime(format!("error: {} missing; run without --skip-build", index.display())));
    }
    reject_symlink(&index, "index.html")?;
    let html = fs::read_to_string(&index)?;
    if has_root_absolute_url(&html) {
        return Err(AppError::runtime(format!("error: {} has root-absolute URLs; it would break when served from a sub-path", index.display())));
    }
    Ok(site)
}

pub fn run(env: &EnvCtx, args: &[String]) -> AppResult {
    let Some(skip_build) = parse_web_args(args)? else {
        println!("{HELP}");
        return Ok(());
    };
    let site = validate_web_site(env, skip_build)?;
    package_web_zip(env, &site)
}

fn package_web_zip(env: &EnvCtx, site: &Path) -> AppResult {
    let here = env.root.join("packaging/web");
    let name = format!("pdfcraft-web-{}", env.version);
    let work = env.target_dir.join("web-package");
    reset_real_dir(&work)?;
    let stage = work.join(&name);
    copy_dir_no_symlinks(site, &stage)?;
    env.stage_models(&stage.join("models"), false)?;
    for (src_name, dst_name) in [("_headers", "_headers"), (".htaccess", ".htaccess"), ("README.md", "HOSTING.md")] {
        copy_file_checked(&here.join(src_name), &stage.join(dst_name))?;
    }
    env.copy_docs(&stage)?;

    ensure_real_dir_path(&env.dist)?;
    let zip_path = env.dist.join(format!("{name}.zip"));
    if zip_path.exists() {
        fs::remove_file(&zip_path)?;
    }
    let mut zip_cmd = Command::new("zip");
    zip_cmd.arg("-qr9").arg(&zip_path).arg(&name).current_dir(&work);
    run_status(&mut zip_cmd, "zip")?;
    print_written_summary(&zip_path);
    Ok(())
}

fn print_written_summary(zip_path: &Path) {
    let bytes = fs::metadata(zip_path).map_or(0, |m| m.len());
    println!("wrote {} ({bytes} bytes)", zip_path.display());
}
