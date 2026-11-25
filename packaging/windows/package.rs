// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Standalone Windows MSI & portable ZIP packager (`rustc --edition=2024 -D warnings`).

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod package_fs;
mod package_pe;
mod package_tests;

use package_fs::{
    allowlisted_ps_bin, copy_dir_no_symlinks, copy_file_checked, ensure_real_dir_path, ensure_real_subdir, reject_symlink, reset_real_dir,
    run_ps_script, run_status,
};
use package_pe::verify_pe_file;
use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

pub struct ArchSpec {
    pub arch: &'static str,
    pub target: &'static str,
    pub rustflags_env: &'static str,
    pub machine: u16,
}

struct Config {
    arch: String,
    skip_build: bool,
    built_without_craft_fonts: bool,
    script_dir: Option<PathBuf>,
    shell: String,
    win_checks: Option<PathBuf>,
}

struct BuildPaths<'a> {
    root: &'a Path,
    script_dir: &'a Path,
    target_dir: &'a Path,
    dist: &'a Path,
}

struct PackageMeta<'a> {
    spec: &'a ArchSpec,
    version: &'a str,
    msi_version: &'a str,
    shell_bin: &'a str,
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

pub fn resolve_version(cargo_toml: &str, env_ver: Option<&str>) -> Result<(String, String)> {
    let raw = match env_ver.map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) => v.to_owned(),
        None => parse_workspace_version(cargo_toml)?,
    };
    let msi = raw.split('-').next().unwrap_or(&raw).trim().to_owned();
    if msi.is_empty() || !msi.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return Err(format!("Invalid MSI version derived from `{raw}`").into());
    }
    Ok((raw, msi))
}

fn parse_workspace_version(cargo_toml: &str) -> Result<String> {
    let mut in_pkg = false;
    for line in cargo_toml.lines().map(str::trim) {
        if line.starts_with('[') {
            in_pkg = line == "[workspace.package]";
            continue;
        }
        if in_pkg
            && let Some(rest) = line.strip_prefix("version")
            && let Some((_, val)) = rest.split_once('=')
        {
            let without_comment = val.split('#').next().unwrap_or(val);
            let trimmed = without_comment.trim().trim_matches('"').trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_owned());
            }
        }
    }
    Err("Failed to resolve [workspace.package].version from Cargo.toml".into())
}

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

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

fn resolve_build_meta(root: &Path) -> Result<(String, String)> {
    let sha = match non_empty_env("PDFCRAFT_BUILD_SHA") {
        Some(v) => v,
        None => {
            let out = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(root).stdin(Stdio::null()).output()?;
            if !out.status.success() {
                return Err(format!("git rev-parse HEAD failed ({})", out.status).into());
            }
            String::from_utf8(out.stdout)?.trim().to_owned()
        }
    };
    let date = match non_empty_env("PDFCRAFT_BUILD_DATE") {
        Some(v) => v,
        None => utc_date_from_unix_secs(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?,
    };
    Ok((sha, date))
}

fn build_binaries(paths: &BuildPaths<'_>, spec: &ArchSpec) -> Result {
    let (sha, date) = resolve_build_meta(paths.root)?;
    let mut cargo = Command::new("cargo");
    cargo
        .args(["build", "--release", "--locked", "-p", "pdfcraft", "-p", "pdfcraft-cli", "--target", spec.target])
        .current_dir(paths.root)
        .env(spec.rustflags_env, "-C target-feature=+crt-static")
        .env("PDFCRAFT_REQUIRE_WINRES", "1")
        .env("PDFCRAFT_BUILD_SHA", &sha)
        .env("PDFCRAFT_BUILD_DATE", &date);
    run_status(&mut cargo, "cargo build")
}

fn stage_core_files(paths: &BuildPaths<'_>, meta: &PackageMeta<'_>) -> Result<PathBuf> {
    let bin_dir = paths.target_dir.join(meta.spec.target).join("release");
    let gui_exe = bin_dir.join("pdfcraft.exe");
    let cli_exe = bin_dir.join("pdfcraft-cli.exe");
    verify_pe_file(&gui_exe, meta.spec.machine, 2)?;
    verify_pe_file(&cli_exe, meta.spec.machine, 3)?;
    let stage = paths.target_dir.join("windows-package").join(meta.spec.arch);
    reset_real_dir(&stage)?;
    for (src, name) in [
        (gui_exe, "pdfcraft.exe"),
        (cli_exe, "pdfcraft-cli.exe"),
        (paths.root.join("LICENSE-MIT"), "LICENSE-MIT"),
        (paths.root.join("LICENSE-APACHE"), "LICENSE-APACHE"),
        (paths.root.join("NOTICE"), "NOTICE"),
        (paths.root.join("assets").join("app-icon").join("LICENSE-document.txt"), "LICENSE-document.txt"),
    ] {
        copy_file_checked(&src, &stage.join(name))?;
    }
    let sign_script = paths.script_dir.join("sign.ps1");
    for exe in ["pdfcraft.exe", "pdfcraft-cli.exe"] {
        let p = stage.join(exe).display().to_string();
        run_ps_script(meta.shell_bin, &sign_script, &[&p])?;
    }
    Ok(stage)
}

fn stage_models(root: &Path, stage: &Path) -> Result<PathBuf> {
    let models = ensure_real_subdir(stage, "models")?;
    let mut cmd = Command::new("cargo");
    cmd.args(["xtask", "models"]).arg(&models).current_dir(root);
    run_status(&mut cmd, "cargo xtask models")?;
    let mut has_licence = false;
    for entry in fs::read_dir(&models)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".part") {
            fs::remove_file(entry.path())?;
        } else if name.ends_with(".LICENCE.txt") && entry.file_type()?.is_file() {
            has_licence = true;
        }
    }
    if !models.join("ATTRIBUTION.txt").is_file() {
        return Err("models/ATTRIBUTION.txt missing after cargo xtask models".into());
    }
    if !has_licence {
        return Err("Expected at least one *.LICENCE.txt inside staged models/".into());
    }
    Ok(models)
}

fn copy_notice_line(line: &str, docs: &Path) -> Result {
    let ofl = Path::new(line);
    let family = ofl.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str()).ok_or("Missing family name")?;
    if family.is_empty() || family.contains(['/', '\\']) || family == ".." || family == "." {
        return Err(format!("Invalid font family directory `{family}`").into());
    }
    copy_file_checked(ofl, &docs.join(format!("OFL-{family}.txt")))
}

fn stage_font_notices(win_checks: Option<&PathBuf>, docs: &Path) -> Result<bool> {
    let Some(font_root) = non_empty_env("CRAFT_FONTS_DIR") else {
        return Ok(false);
    };
    let bin = win_checks.ok_or("--win-checks executable path is required when CRAFT_FONTS_DIR is set")?;
    let bin_meta = reject_symlink(bin, "win_checks executable")?;
    if !bin_meta.is_file() {
        return Err(format!("--win-checks path is not a regular file: {}", bin.display()).into());
    }
    let mut cmd = Command::new(bin);
    cmd.arg("font-notices").arg(&font_root);
    if env::var_os("CRAFT_FONTS_REQUIRED").is_some() {
        cmd.arg("--require-chinese");
    }
    let out = cmd.stdin(Stdio::null()).output()?;
    if !out.status.success() {
        return Err(format!("win_checks font-notices failed ({}): {}", out.status, String::from_utf8_lossy(&out.stderr)).into());
    }
    for line in String::from_utf8(out.stdout)?.lines().map(str::trim).filter(|s| !s.is_empty()) {
        copy_notice_line(line, docs)?;
    }
    Ok(true)
}

fn build_and_check_msi(paths: &BuildPaths<'_>, meta: &PackageMeta<'_>, stage: &Path, models: &Path, docs: &Path, has_fonts: bool) -> Result<PathBuf> {
    ensure_real_dir_path(paths.dist)?;
    let stem = format!("pdfcraft-{}-windows-{}", meta.version, meta.spec.arch);
    let msi = paths.dist.join(format!("{stem}.msi"));
    let fonts_flag = if has_fonts { "1" } else { "0" };
    let icon = paths.root.join("assets").join("app-icon").join("pdfcraft.ico");
    let mut wix = Command::new("wix");
    wix.current_dir(paths.script_dir).args(["build", "pdfcraft.wxs", "-arch", meta.spec.arch, "installer-ui.wxs"]);
    for (k, v) in [
        ("Version", meta.msi_version.to_owned()),
        ("BinDir", stage.display().to_string()),
        ("ModelsDir", models.display().to_string()),
        ("IconPath", icon.display().to_string()),
        ("DocsDir", docs.display().to_string()),
        ("HasCraftFontLicences", fonts_flag.to_owned()),
    ] {
        wix.args(["-d", &format!("{k}={v}")]);
    }
    wix.arg("-o").arg(&msi);
    run_status(&mut wix, "wix build")?;
    let (msi_s, docs_s) = (msi.display().to_string(), docs.display().to_string());
    run_ps_script(
        meta.shell_bin,
        &paths.script_dir.join("test-msi.ps1"),
        &[&msi_s, "-CraftFontLicences", fonts_flag, "-ExpectedFontNoticesDirectory", &docs_s],
    )?;
    run_ps_script(meta.shell_bin, &paths.script_dir.join("check-install-policy.ps1"), &["-MsiPath", &msi_s])?;
    let pdb = paths.dist.join(format!("{stem}.wixpdb"));
    if pdb.exists() {
        fs::remove_file(pdb)?;
    }
    run_ps_script(meta.shell_bin, &paths.script_dir.join("sign.ps1"), &[&msi_s])?;
    Ok(msi)
}

fn copy_portable_notices(docs: &Path, p_stage: &Path) -> Result {
    for entry in fs::read_dir(docs)? {
        let entry = entry?;
        let fname = entry.file_name().to_string_lossy().into_owned();
        if fname.starts_with("OFL-") && fname.ends_with(".txt") {
            copy_file_checked(&entry.path(), &p_stage.join(fname))?;
        }
    }
    Ok(())
}

fn build_portable_zip(paths: &BuildPaths<'_>, meta: &PackageMeta<'_>, stage: &Path, models: &Path, docs: &Path, has_fonts: bool) -> Result<PathBuf> {
    let name = format!("pdfcraft-{}-windows-{}-portable", meta.version, meta.spec.arch);
    let p_stage = paths.target_dir.join("windows-package").join(&name);
    reset_real_dir(&p_stage)?;
    for f in ["pdfcraft.exe", "pdfcraft-cli.exe", "NOTICE", "LICENSE-document.txt"] {
        copy_file_checked(&stage.join(f), &p_stage.join(f))?;
    }
    copy_dir_no_symlinks(models, &p_stage.join("models"))?;
    for opt in ["README.md", "LICENSE", "LICENSE-MIT", "LICENSE-APACHE"] {
        let src = paths.root.join(opt);
        if src.is_file() {
            copy_file_checked(&src, &p_stage.join(opt))?;
        }
    }
    if has_fonts {
        copy_portable_notices(docs, &p_stage)?;
    }
    let _ = File::create(p_stage.join("portable.txt"))?;
    let zip = paths.dist.join(format!("{name}.zip"));
    let src_glob = p_stage.join("*");
    let mut cmd = Command::new(meta.shell_bin);
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", "Compress-Archive -Path $env:CRAFT_ZIP_SRC -DestinationPath $env:CRAFT_ZIP_DST -Force"])
        .env("CRAFT_ZIP_SRC", &src_glob)
        .env("CRAFT_ZIP_DST", &zip);
    run_status(&mut cmd, "Compress-Archive")?;
    Ok(zip)
}

struct Prepared {
    spec: ArchSpec,
    script_dir: PathBuf,
    root: PathBuf,
    target_dir: PathBuf,
    dist: PathBuf,
    version: String,
    msi_version: String,
    shell_bin: &'static str,
}

fn prepare_package(cfg: &Config) -> Result<Prepared> {
    validate_flags(cfg.skip_build, cfg.built_without_craft_fonts, non_empty_env("CRAFT_FONTS_DIR").is_some())?;
    let spec = arch_spec(&cfg.arch)?;
    let raw_script_dir = cfg.script_dir.clone().unwrap_or_else(|| PathBuf::from("packaging").join("windows"));
    let script_dir = fs::canonicalize(raw_script_dir)?;
    let root = fs::canonicalize(script_dir.join("..").join(".."))?;
    let cargo_toml = fs::read_to_string(root.join("Cargo.toml"))?;
    let (version, msi_version) = resolve_version(&cargo_toml, non_empty_env("PDFCRAFT_VERSION").as_deref())?;
    let dist = non_empty_env("DIST").map(PathBuf::from).unwrap_or_else(|| root.join("dist").join("release"));
    let target_dir = non_empty_env("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"));
    let shell_bin = allowlisted_ps_bin(&cfg.shell)?;
    Ok(Prepared { spec, script_dir, root, target_dir, dist, version, msi_version, shell_bin })
}

fn run_package(cfg: &Config) -> Result {
    let p = prepare_package(cfg)?;
    let paths = BuildPaths { root: &p.root, script_dir: &p.script_dir, target_dir: &p.target_dir, dist: &p.dist };
    let meta = PackageMeta { spec: &p.spec, version: &p.version, msi_version: &p.msi_version, shell_bin: p.shell_bin };
    if !cfg.skip_build {
        build_binaries(&paths, &p.spec)?;
    }
    let stage = stage_core_files(&paths, &meta)?;
    let models = stage_models(paths.root, &stage)?;
    let docs = ensure_real_subdir(&stage, "font-licences")?;
    let has_fonts = stage_font_notices(cfg.win_checks.as_ref(), &docs)?;
    let msi = build_and_check_msi(&paths, &meta, &stage, &models, &docs, has_fonts)?;
    let zip = build_portable_zip(&paths, &meta, &stage, &models, &docs, has_fonts)?;
    if p.spec.arch != "arm64" || env::consts::ARCH == "aarch64" {
        run_status(Command::new(stage.join("pdfcraft-cli.exe")).arg("--version"), "pdfcraft-cli.exe --version")?;
    }
    println!("Packaged {} ({} bytes) and {} ({} bytes)", msi.display(), fs::metadata(&msi)?.len(), zip.display(), fs::metadata(&zip)?.len());
    Ok(())
}

fn parse_args(args: &[String]) -> Result<Config> {
    let mut cfg = Config {
        arch: String::from("x64"),
        skip_build: false,
        built_without_craft_fonts: false,
        script_dir: None,
        shell: String::from("pwsh"),
        win_checks: None,
    };
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--arch" if i + 1 < args.len() => {
                cfg.arch = args[i + 1].clone();
                i += 2;
            }
            "--skip-build" => {
                cfg.skip_build = true;
                i += 1;
            }
            "--built-without-craft-fonts" => {
                cfg.built_without_craft_fonts = true;
                i += 1;
            }
            "--script-dir" if i + 1 < args.len() => {
                cfg.script_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--shell" if i + 1 < args.len() => {
                cfg.shell = args[i + 1].clone();
                i += 2;
            }
            "--win-checks" if i + 1 < args.len() => {
                cfg.win_checks = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            other => return Err(format!("Unexpected argument `{other}`").into()),
        }
    }
    Ok(cfg)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect(); // nosemgrep: rust.lang.security.args.args
    if let Err(err) = parse_args(&args).and_then(|c| run_package(&c)) {
        eprintln!("{err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
