// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! FreeBSD packaging (`packaging/freebsd/package.sh` port).

use super::unix_package_env::{EnvCtx, non_empty_env, render_template, run_cargo_build};
use super::unix_package_fs::{
    copy_dir_no_symlinks, ensure_real_dir_path, install_file_mode, reset_real_dir, run_capture, run_staged_binary, run_status, strip_best_effort,
};
use super::{AppError, AppResult};
use std::fs;
use std::path::Path;
use std::process::Command;

const APP_ID: &str = "ai.storyteller.pdfcraft";

const HELP: &str = "\
Build PlutarchPages for FreeBSD and package it:
  $DIST/pdfcraft-<version>-freebsd-<arch>.tar.gz   a /usr/local-style tree (bin/, share/)
Usage: packaging/freebsd/package.sh [--skip-build]";

pub fn normalize_freebsd_arch(raw: &str) -> AppResult<&'static str> {
    match raw {
        "amd64" | "x86_64" => Ok("x86_64"),
        "arm64" | "aarch64" => Ok("aarch64"),
        other => Err(AppError::usage(format!("unsupported architecture {other}"))),
    }
}

pub fn resolve_target_and_arch(raw_target: Option<&str>, host_arch_raw: &str, rustc_host: Option<&str>) -> AppResult<(Option<String>, &'static str)> {
    let resolved = match raw_target {
        Some("host-tuple") => Some(rustc_host.ok_or_else(|| AppError::runtime("error: rustc -vV missing host triple"))?.to_owned()),
        Some(t) if !t.is_empty() => Some(t.to_owned()),
        _ => None,
    };
    let arch = match resolved.as_deref() {
        None => normalize_freebsd_arch(host_arch_raw)?,
        Some("aarch64-unknown-freebsd") => "aarch64",
        Some("x86_64-unknown-freebsd") => "x86_64",
        Some(other) => {
            return Err(AppError::usage(format!("CARGO_BUILD_TARGET={other} is not a supported FreeBSD target")));
        }
    };
    Ok((resolved, arch))
}

pub fn is_freebsd_elf_description(desc: &str, arch: &str) -> bool {
    let want = if arch == "x86_64" { "x86-64" } else { arch };
    desc.starts_with("ELF") && desc.contains(want) && desc.contains("FreeBSD") && (desc.contains(" executable, ") || desc.contains("interpreter"))
}

fn rustc_host_triple() -> AppResult<String> {
    let out = run_capture(Command::new("rustc").arg("-vV"), "rustc -vV")?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::trim))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| AppError::runtime("error: rustc -vV missing host line"))
}

fn host_uname(flag: &str) -> AppResult<String> {
    let out = run_capture(Command::new("uname").arg(flag), "uname")?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn parse_freebsd_args(args: &[String]) -> AppResult<Option<bool>> {
    match args {
        [] => Ok(Some(false)),
        [flag] if flag == "--skip-build" => Ok(Some(true)),
        [flag] if flag == "-h" || flag == "--help" => Ok(None),
        [first, ..] => Err(AppError::usage(format!("unknown argument: {first}"))),
    }
}

fn resolve_host_descriptor(target: Option<&str>) -> AppResult<String> {
    let host_m = host_uname("-m")?;
    let host_s = host_uname("-s")?;
    let host_arch_norm = normalize_freebsd_arch(&host_m).unwrap_or(&host_m);
    if target.is_some() && host_s != "FreeBSD" { Ok(format!("{host_s} {host_arch_norm}")) } else { Ok(host_arch_norm.to_owned()) }
}

pub fn run(env: &EnvCtx, args: &[String]) -> AppResult {
    let Some(skip_build) = parse_freebsd_args(args)? else {
        println!("{HELP}");
        return Ok(());
    };
    let raw_target = non_empty_env("CARGO_BUILD_TARGET");
    let host_m = host_uname("-m")?;
    let rustc_host = if raw_target.as_deref() == Some("host-tuple") { Some(rustc_host_triple()?) } else { None };
    let (target, arch) = resolve_target_and_arch(raw_target.as_deref(), &host_m, rustc_host.as_deref())?;
    let host_desc = resolve_host_descriptor(target.as_deref())?;
    package_freebsd(env, skip_build, target.as_deref(), arch, &host_desc)
}

fn package_freebsd(env: &EnvCtx, skip_build: bool, target: Option<&str>, arch: &str, host_desc: &str) -> AppResult {
    let basename = format!("pdfcraft-{}-freebsd-{arch}", env.version);
    println!("==> PlutarchPages {} for FreeBSD {arch}", env.version);
    if !skip_build {
        let targets: Vec<&str> = target.into_iter().collect();
        run_cargo_build(env, &targets)?;
    }
    let bin_dir = match target {
        Some(t) => env.target_dir.join(t).join("release"),
        None => env.target_dir.join("release"),
    };
    let work = env.target_dir.join("freebsd-package");
    let stage = work.join(&basename);
    reset_real_dir(&work)?;
    stage_tree(env, &bin_dir, &stage)?;
    verify_binaries(&stage.join("bin"), target, arch, host_desc)?;
    ensure_real_dir_path(&env.dist)?;
    let out = env.dist.join(format!("{basename}.tar.gz"));
    run_status(Command::new("tar").arg("-C").arg(&work).arg("-czf").arg(&out).arg(&basename), "tar")?;
    println!("wrote {}", out.display());
    Ok(())
}

fn stage_tree(env: &EnvCtx, bin_dir: &Path, stage: &Path) -> AppResult {
    let linux_dir = env.root.join("packaging/linux");
    for rel in ["bin", "share/applications", "share/mime/packages", "share/metainfo", "share/icons", "share/doc/pdfcraft"] {
        ensure_real_dir_path(&stage.join(rel))?;
    }
    let gui_dst = stage.join("bin/pdfcraft");
    let cli_dst = stage.join("bin/pdfcraft-cli");
    install_file_mode(&bin_dir.join("pdfcraft"), &gui_dst, 0o755)?;
    install_file_mode(&bin_dir.join("pdfcraft-cli"), &cli_dst, 0o755)?;
    strip_best_effort("", &[&gui_dst, &cli_dst]);

    install_file_mode(&linux_dir.join(format!("{APP_ID}.desktop")), &stage.join(format!("share/applications/{APP_ID}.desktop")), 0o644)?;
    install_file_mode(&linux_dir.join(format!("{APP_ID}.mime.xml")), &stage.join(format!("share/mime/packages/{APP_ID}.xml")), 0o644)?;
    render_template(
        &linux_dir.join(format!("{APP_ID}.metainfo.xml.in")),
        &stage.join(format!("share/metainfo/{APP_ID}.metainfo.xml")),
        &[("@VERSION@", &env.version), ("@DATE@", &env.build_date)],
    )?;
    copy_dir_no_symlinks(&env.root.join("assets/app-icon/hicolor"), &stage.join("share/icons/hicolor"))?;
    env.copy_docs(&stage.join("share/doc/pdfcraft"))?;
    env.stage_models(&stage.join("share/pdfcraft/models"), true)?;
    Ok(())
}

fn verify_binaries(bin_dir: &Path, target: Option<&str>, arch: &str, host_desc: &str) -> AppResult {
    if target.is_none() && arch == host_desc {
        return run_staged_binary(&bin_dir.join("pdfcraft"), &["--version"], "pdfcraft --version");
    }
    for entry in fs::read_dir(bin_dir)? {
        let entry = entry?;
        let path = entry.path();
        let out = run_capture(Command::new("file").arg("-b").arg(&path), "file -b")?;
        let desc = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !is_freebsd_elf_description(&desc, arch) {
            return Err(AppError::runtime(format!("error: {} is not a FreeBSD {arch} executable: {desc}", path.display())));
        }
    }
    let target_label = target.unwrap_or("");
    println!("skipping --version: a CARGO_BUILD_TARGET={target_label} build does not run on this {host_desc} machine");
    Ok(())
}
