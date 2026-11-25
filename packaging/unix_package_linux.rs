// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Linux packaging (`packaging/linux/package.sh` port).

use super::unix_package_env::{EnvCtx, non_empty_env, render_template, verify_sha256};
use super::unix_package_fs::{
    command_exists, copy_dir_no_symlinks, copy_file_checked, create_symlink_unix, ensure_real_dir_path, install_file_mode, reject_symlink,
    reset_real_dir, run_capture, run_staged_binary, run_status, set_unix_mode, strip_best_effort,
};
use super::{AppError, AppResult};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const APP_ID: &str = "ai.storyteller.pdfcraft";
const APPIMAGETOOL_VERSION: &str = "1.9.1";
const APPIMAGE_RUNTIME_VERSION: &str = "20251108";

const HELP: &str = "\
Build and package PlutarchPages for Linux (<arch> is x86_64 or aarch64, or riscv64 when cross-compiling).
Usage: packaging/linux/package.sh [--skip-build] [--formats \"appimage deb rpm tar cli\"]";

struct LinuxOpts {
    skip_build: bool,
    formats: String,
}

pub struct LinuxArchSpec {
    pub arch: &'static str,
    pub deb_arch: &'static str,
    pub appimagetool_sha256: Option<&'static str>,
    pub runtime_sha256: Option<&'static str>,
}

pub fn resolve_linux_arch(raw: &str) -> AppResult<LinuxArchSpec> {
    match raw {
        "riscv64" => Ok(LinuxArchSpec { arch: "riscv64", deb_arch: "riscv64", appimagetool_sha256: None, runtime_sha256: None }),
        "x86_64" => Ok(LinuxArchSpec {
            arch: "x86_64",
            deb_arch: "amd64",
            appimagetool_sha256: Some("ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0"),
            runtime_sha256: Some("2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d"),
        }),
        "aarch64" | "arm64" => Ok(LinuxArchSpec {
            arch: "aarch64",
            deb_arch: "arm64",
            appimagetool_sha256: Some("f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158"),
            runtime_sha256: Some("00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444"),
        }),
        other => Err(AppError::usage(format!("unsupported architecture {other}"))),
    }
}

pub fn has_format(formats: &str, name: &str) -> bool {
    format!(" {formats} ").contains(&format!(" {name} "))
}

pub fn parse_linux_args(args: &[String]) -> AppResult<Option<(bool, String)>> {
    let mut skip_build = false;
    let mut formats = String::from("appimage deb rpm tar cli");
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--skip-build" => {
                skip_build = true;
                idx += 1;
            }
            "--formats" => {
                let Some(val) = args.get(idx + 1) else {
                    return Err(AppError::usage("--formats requires a value"));
                };
                formats.clone_from(val);
                idx += 2;
            }
            "-h" | "--help" => return Ok(None),
            other => return Err(AppError::usage(format!("unknown argument: {other}"))),
        }
    }
    Ok(Some((skip_build, formats)))
}

fn host_uname_m() -> AppResult<String> {
    let out = run_capture(Command::new("uname").arg("-m"), "uname -m")?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub fn run(env: &EnvCtx, args: &[String]) -> AppResult {
    let Some((skip_build, formats)) = parse_linux_args(args)? else {
        println!("{HELP}");
        return Ok(());
    };
    let opts = LinuxOpts { skip_build, formats };
    let arch_raw = match non_empty_env("CROSS_ARCH") {
        Some(a) => a,
        None => host_uname_m()?,
    };
    let spec = resolve_linux_arch(&arch_raw)?;
    let cross_target = non_empty_env("CROSS_TARGET");
    let label_suffix = env.build_label.as_deref().map_or(String::new(), |l| format!("-{l}"));
    let basename = format!("pdfcraft-{}{label_suffix}-linux-{}", env.version, spec.arch);
    println!("==> PlutarchPages {}{label_suffix} for Linux {} ({})", env.version, spec.arch, opts.formats);

    if !opts.skip_build {
        let targets: Vec<&str> = cross_target.as_deref().into_iter().collect();
        super::unix_package_env::run_cargo_build(env, &targets)?;
    }

    let bin_dir = match cross_target.as_deref() {
        Some(t) => env.target_dir.join(t).join("release"),
        None => env.target_dir.join("release"),
    };
    let work = env.target_dir.join("linux-package");
    let stage = work.join("root");
    reset_real_dir(&work)?;
    stage_fhs_tree(env, &bin_dir, &stage)?;
    emit_packages(env, &opts, &spec, &work, &stage, &basename, &label_suffix)?;
    smoke_check_cli(&stage, cross_target.as_deref())?;
    println!("==> done");
    Ok(())
}

fn stage_fhs_tree(env: &EnvCtx, bin_dir: &Path, stage: &Path) -> AppResult {
    let here = env.root.join("packaging/linux");
    let gui_dst = stage.join("usr/bin/pdfcraft");
    let cli_dst = stage.join("usr/bin/pdfcraft-cli");
    install_file_mode(&bin_dir.join("pdfcraft"), &gui_dst, 0o755)?;
    install_file_mode(&bin_dir.join("pdfcraft-cli"), &cli_dst, 0o755)?;
    let cross_prefix = non_empty_env("CROSS_COMPILE").unwrap_or_default();
    strip_best_effort(&cross_prefix, &[&gui_dst, &cli_dst]);

    install_file_mode(&here.join(format!("{APP_ID}.desktop")), &stage.join(format!("usr/share/applications/{APP_ID}.desktop")), 0o644)?;
    install_file_mode(&here.join(format!("{APP_ID}.mime.xml")), &stage.join(format!("usr/share/mime/packages/{APP_ID}.xml")), 0o644)?;
    let metainfo_dir = stage.join("usr/share/metainfo");
    ensure_real_dir_path(&metainfo_dir)?;
    let metainfo = metainfo_dir.join(format!("{APP_ID}.metainfo.xml"));
    render_template(&here.join(format!("{APP_ID}.metainfo.xml.in")), &metainfo, &[("@VERSION@", &env.version), ("@DATE@", &env.build_date)])?;
    copy_dir_no_symlinks(&env.root.join("assets/app-icon/hicolor"), &stage.join("usr/share/icons/hicolor"))?;
    env.copy_docs(&stage.join("usr/share/doc/pdfcraft"))?;
    env.stage_models(&stage.join("usr/share/pdfcraft/models"), false)?;

    if command_exists("desktop-file-validate") {
        let mut cmd = Command::new("desktop-file-validate");
        cmd.arg(stage.join(format!("usr/share/applications/{APP_ID}.desktop")));
        run_status(&mut cmd, "desktop-file-validate")?;
    }
    if command_exists("appstreamcli") {
        let mut cmd = Command::new("appstreamcli");
        cmd.args(["validate", "--no-net", "--explain"]).arg(&metainfo);
        run_status(&mut cmd, "appstreamcli validate")?;
    }
    Ok(())
}

fn emit_packages(env: &EnvCtx, opts: &LinuxOpts, spec: &LinuxArchSpec, work: &Path, stage: &Path, basename: &str, label_suffix: &str) -> AppResult {
    if has_format(&opts.formats, "tar") {
        let tar_stage = work.join("tar");
        copy_dir_no_symlinks(&stage.join("usr"), &tar_stage.join(basename))?;
        let out = env.dist.join(format!("{basename}.tar.gz"));
        run_status(Command::new("tar").arg("-C").arg(&tar_stage).arg("-czf").arg(&out).arg(basename), "tar")?;
        println!("wrote {}", out.display());
    }
    if has_format(&opts.formats, "cli") {
        let cli_name = format!("pdfcraft-cli-{}{label_suffix}-linux-{}", env.version, spec.arch);
        let cli_stage = work.join("cli");
        let cli_dir = cli_stage.join(&cli_name);
        copy_file_checked(&stage.join("usr/bin/pdfcraft-cli"), &cli_dir.join("pdfcraft-cli"))?;
        set_unix_mode(&cli_dir.join("pdfcraft-cli"), 0o755)?;
        env.copy_docs(&cli_dir)?;
        let out = env.dist.join(format!("{cli_name}.tar.gz"));
        run_status(Command::new("tar").arg("-C").arg(&cli_stage).arg("-czf").arg(&out).arg(&cli_name), "tar cli")?;
        println!("wrote {}", out.display());
    }
    if has_format(&opts.formats, "deb") || has_format(&opts.formats, "rpm") {
        package_nfpm(env, &opts.formats, spec, work, stage, basename)?;
    }
    if has_format(&opts.formats, "appimage") {
        package_appimage(env, spec, work, stage, basename)?;
    }
    Ok(())
}

fn package_nfpm(env: &EnvCtx, formats: &str, spec: &LinuxArchSpec, work: &Path, stage: &Path, basename: &str) -> AppResult {
    if !command_exists("nfpm") {
        return Err(AppError::runtime("error: nfpm not found (https://nfpm.goreleaser.com/docs/install/)"));
    }
    let maintainer =
        non_empty_env("PDFCRAFT_MAINTAINER").unwrap_or_else(|| String::from("PlutarchPages contributors <ryanduguid@users.noreply.github.com>"));
    let release_num = env.build_label.as_deref().map_or_else(|| String::from("1"), |l| format!("1.{l}"));
    let glibc_floor = non_empty_env("GLIBC_FLOOR").unwrap_or_else(|| String::from("2.35"));
    let rpm_glibc = non_empty_env("RPM_GLIBC_FLOOR").unwrap_or_else(|| String::from("libm.so.6(GLIBC_2.35)(64bit)"));
    let nfpm_cfg = work.join("nfpm.yaml");
    let stage_str = stage.to_string_lossy();
    render_template(
        &env.root.join("packaging/linux/nfpm.yaml"),
        &nfpm_cfg,
        &[("${STAGE}", stage_str.as_ref()), ("${GLIBC_FLOOR}", &glibc_floor), ("${RPM_GLIBC_FLOOR}", &rpm_glibc)],
    )?;
    for fmt in ["deb", "rpm"] {
        if has_format(formats, fmt) {
            let mut cmd = Command::new("nfpm");
            cmd.arg("package")
                .arg("-f")
                .arg(&nfpm_cfg)
                .arg("-p")
                .arg(fmt)
                .arg("-t")
                .arg(env.dist.join(format!("{basename}.{fmt}")))
                .current_dir(&env.root)
                .env("VERSION", &env.version)
                .env("RELEASE", &release_num)
                .env("NFPM_ARCH", spec.deb_arch)
                .env("PDFCRAFT_MAINTAINER", &maintainer);
            run_status(&mut cmd, &format!("nfpm {fmt}"))?;
        }
    }
    Ok(())
}

fn download_verified(url: &str, dest: &Path, expected_sha: &str, executable: bool) -> AppResult {
    let download = dest.with_extension("download");
    if download.exists() {
        let _ = fs::remove_file(&download);
    }
    if let Some(parent) = dest.parent() {
        ensure_real_dir_path(parent)?;
    }
    let mut curl = Command::new("curl");
    curl.args(["--proto", "=https", "-fsSL", "-o"]).arg(&download).arg(url);
    run_status(&mut curl, "curl")?;
    verify_sha256(expected_sha, &download)?;
    if executable {
        set_unix_mode(&download, 0o755)?;
    }
    fs::rename(&download, dest)?;
    Ok(())
}

fn ensure_appimage_tool(env: &EnvCtx, spec: &LinuxArchSpec) -> AppResult<PathBuf> {
    if let Some(explicit) = non_empty_env("APPIMAGETOOL") {
        let path = PathBuf::from(explicit);
        let meta = reject_symlink(&path, "APPIMAGETOOL")?;
        if !meta.is_file() {
            return Err(AppError::runtime(format!("error: APPIMAGETOOL is not a regular file: {}", path.display())));
        }
        return Ok(fs::canonicalize(path)?);
    }
    if command_exists("appimagetool") {
        return Ok(PathBuf::from("appimagetool"));
    }
    let Some(sha) = spec.appimagetool_sha256 else {
        return Err(AppError::usage(format!("no pinned appimagetool for {}; set APPIMAGETOOL or put appimagetool on PATH", spec.arch)));
    };
    let tool = env.target_dir.join(format!("appimagetool-{APPIMAGETOOL_VERSION}-{}.AppImage", spec.arch));
    if !tool.is_file() {
        let url = format!("https://github.com/AppImage/appimagetool/releases/download/{APPIMAGETOOL_VERSION}/appimagetool-{}.AppImage", spec.arch);
        download_verified(&url, &tool, sha, true)?;
    }
    verify_sha256(sha, &tool)?;
    Ok(fs::canonicalize(&tool)?)
}

fn ensure_appimage_runtime(env: &EnvCtx, spec: &LinuxArchSpec, rt_sha: &str) -> AppResult<PathBuf> {
    let runtime = env.target_dir.join(format!("appimage-runtime-{APPIMAGE_RUNTIME_VERSION}-{}", spec.arch));
    if !runtime.is_file() {
        let url = format!("https://github.com/AppImage/type2-runtime/releases/download/{APPIMAGE_RUNTIME_VERSION}/runtime-{}", spec.arch);
        download_verified(&url, &runtime, rt_sha, false)?;
    }
    verify_sha256(rt_sha, &runtime)?;
    Ok(fs::canonicalize(&runtime)?)
}

fn prepare_appdir(env: &EnvCtx, stage: &Path, appdir: &Path) -> AppResult {
    let here = env.root.join("packaging/linux");
    copy_dir_no_symlinks(stage, appdir)?;
    create_symlink_unix("usr/bin/pdfcraft", &appdir.join("AppRun"))?;
    copy_file_checked(&here.join(format!("{APP_ID}.desktop")), &appdir.join(format!("{APP_ID}.desktop")))?;
    let icon_name = format!("{APP_ID}.png");
    copy_file_checked(&env.root.join("assets/app-icon/hicolor/256x256/apps").join(&icon_name), &appdir.join(&icon_name))?;
    create_symlink_unix(&icon_name, &appdir.join(".DirIcon"))
}

fn package_appimage(env: &EnvCtx, spec: &LinuxArchSpec, work: &Path, stage: &Path, basename: &str) -> AppResult {
    let Some(rt_sha) = spec.runtime_sha256 else {
        return Err(AppError::usage(format!("no pinned AppImage runtime for {}; build the AppImage on x86_64 or aarch64", spec.arch)));
    };
    let appdir = work.join("PdfCraft.AppDir");
    prepare_appdir(env, stage, &appdir)?;
    let tool = ensure_appimage_tool(env, spec)?;
    let runtime = ensure_appimage_runtime(env, spec, rt_sha)?;

    let abs_dist = fs::canonicalize(&env.dist)?;
    let out = abs_dist.join(format!("{basename}.AppImage"));
    let zsync = abs_dist.join(format!("{basename}.AppImage.zsync"));
    if zsync.exists() {
        reject_symlink(&zsync, "zsync output")?;
        fs::remove_file(&zsync)?;
    }
    let repo = non_empty_env("GITHUB_REPOSITORY").unwrap_or_else(|| String::from("yungSNAT/PlutarchPages"));
    let (owner, name) = repo.split_once('/').unwrap_or((&repo, ""));
    let update_info = format!("gh-releases-zsync|{owner}|{name}|latest|pdfcraft-*-linux-{}.AppImage.zsync", spec.arch);

    let mut cmd = Command::new(&tool);
    cmd.args(["--no-appstream"])
        .arg("--runtime-file")
        .arg(&runtime)
        .args(["-u", &update_info])
        .arg(fs::canonicalize(&appdir)?)
        .arg(&out)
        .current_dir(&abs_dist)
        .env("ARCH", spec.arch)
        .env("APPIMAGE_EXTRACT_AND_RUN", "1");
    run_status(&mut cmd, "appimagetool")?;
    println!("wrote {}", out.display());
    if fs::metadata(&zsync).is_ok_and(|m| m.len() > 0) {
        println!("wrote {}", zsync.display());
    } else {
        env.warn(&format!("zsyncmake not found, so {} was not written; AppImage delta updates need it", zsync.display()));
    }
    Ok(())
}

fn smoke_check_cli(stage: &Path, cross_target: Option<&str>) -> AppResult {
    let cli = stage.join("usr/bin/pdfcraft-cli");
    if cross_target.is_none() {
        return run_staged_binary(&cli, &["--version"], "pdfcraft-cli --version");
    }
    if let Some(emu) = non_empty_env("EMULATOR") {
        return run_status(Command::new(emu).arg(&cli).arg("--version"), "emulator pdfcraft-cli --version");
    }
    Ok(())
}
