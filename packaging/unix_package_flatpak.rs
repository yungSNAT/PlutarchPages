// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Flatpak single-file bundle packager (`packaging/linux/flatpak-bundle.sh` port).

use super::unix_package_env::{EnvCtx, non_empty_env};
use super::unix_package_fs::{command_exists, copy_file_checked, ensure_real_dir_path, reset_real_dir, run_capture, run_quiet, run_status};
use super::{AppError, AppResult};
use std::path::{Path, PathBuf};
use std::process::Command;

const APP_ID: &str = "io.github.yungSNAT.PlutarchPages";
const FLATHUB_URL: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";

const HELP: &str = "\
Repackage a PlutarchPages Linux tarball as a single-file Flatpak bundle:
  $DIST/pdfcraft-<version>-linux-<arch>.flatpak
Usage: packaging/linux/flatpak-bundle.sh [--no-test] [TARBALL]";

pub struct FlatpakOpts {
    pub test: bool,
    pub oci: bool,
    pub tarball: Option<PathBuf>,
}

pub fn parse_flatpak_args(args: &[String]) -> AppResult<Option<FlatpakOpts>> {
    let mut test = true;
    let mut oci = false;
    let mut tarball = None;
    for arg in args {
        match arg.as_str() {
            "--no-test" => test = false,
            "--oci" => oci = true,
            "-h" | "--help" => return Ok(None),
            other if other.starts_with('-') => {
                return Err(AppError::usage(format!("unknown argument: {other}")));
            }
            path => tarball = Some(PathBuf::from(path)),
        }
    }
    Ok(Some(FlatpakOpts { test, oci, tarball }))
}

pub fn normalize_flatpak_arch(raw: &str) -> AppResult<&'static str> {
    match raw {
        "x86_64" => Ok("x86_64"),
        "aarch64" | "arm64" => Ok("aarch64"),
        other => Err(AppError::usage(format!("unsupported architecture {other}"))),
    }
}

pub fn run(env: &EnvCtx, args: &[String]) -> AppResult {
    let Some(opts) = parse_flatpak_args(args)? else {
        println!("{HELP}");
        return Ok(());
    };
    let uname_out = run_capture(Command::new("uname").arg("-m"), "uname -m")?;
    let arch = normalize_flatpak_arch(String::from_utf8_lossy(&uname_out.stdout).trim())?;
    let basename = format!("pdfcraft-{}-linux-{arch}", env.version);
    let tarball = opts.tarball.as_ref().cloned().unwrap_or_else(|| env.dist.join(format!("{basename}.tar.gz")));
    validate_flatpak_prereqs(&tarball)?;
    build_flatpak_bundle(env, &opts, &basename, arch, &tarball)
}

fn validate_flatpak_prereqs(tarball: &Path) -> AppResult {
    if !tarball.is_file() {
        return Err(AppError::runtime(format!("error: {} not found (run packaging/linux/package.sh --formats tar first)", tarball.display())));
    }
    for tool in ["flatpak", "flatpak-builder"] {
        if !command_exists(tool) {
            return Err(AppError::runtime(format!("error: {tool} not found (apt install flatpak flatpak-builder librsvg2-common)")));
        }
    }
    Ok(())
}

fn build_flatpak_bundle(env: &EnvCtx, opts: &FlatpakOpts, basename: &str, arch: &str, tarball: &Path) -> AppResult {
    let tb_name = tarball.file_name().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    println!("==> PlutarchPages {} Flatpak bundle for {arch} from {tb_name}", env.version);

    let work = env.target_dir.join("flatpak-bundle");
    reset_real_dir(&work)?;
    let stage = work.join("stage");
    ensure_real_dir_path(&stage)?;
    run_status(Command::new("tar").arg("-xzf").arg(tarball).arg("-C").arg(&stage).arg("--strip-components=1"), "tar extract")?;
    check_executable_bin(&stage.join("bin/pdfcraft"), tarball)?;

    let manifest_dst = work.join(format!("{APP_ID}.yml"));
    copy_file_checked(&env.root.join("packaging/linux/flatpak").join(format!("{APP_ID}.bundle.yml")), &manifest_dst)?;
    run_status(Command::new("flatpak").args(["remote-add", "--user", "--if-not-exists", "flathub", FLATHUB_URL]), "flatpak remote-add")?;
    run_status(
        Command::new("flatpak-builder")
            .args(["--user", "--install-deps-from=flathub", "--disable-rofiles-fuse", "--force-clean", "--default-branch=stable"])
            .arg(format!("--repo={}", work.join("repo").display()))
            .arg(format!("--state-dir={}", work.join("state").display()))
            .arg(work.join("build"))
            .arg(&manifest_dst),
        "flatpak-builder",
    )?;
    emit_flatpak_outputs(env, &work, basename, opts)
}

fn emit_flatpak_outputs(env: &EnvCtx, work: &Path, basename: &str, opts: &FlatpakOpts) -> AppResult {
    let out = env.dist.join(format!("{basename}.flatpak"));
    run_status(
        Command::new("flatpak")
            .args(["build-bundle", &format!("--runtime-repo={FLATHUB_URL}")])
            .arg(work.join("repo"))
            .arg(&out)
            .args([APP_ID, "stable"]),
        "flatpak build-bundle",
    )?;
    println!("wrote {}", out.display());
    if opts.oci {
        let out_oci = env.dist.join(format!("{basename}-oci"));
        if let Some(parent) = out_oci.parent() {
            ensure_real_dir_path(parent)?;
        }
        if let Ok(meta) = std::fs::symlink_metadata(&out_oci) {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(AppError::runtime(format!("error: symlink or non-directory refused at {}", out_oci.display())));
            }
            std::fs::remove_dir_all(&out_oci)?;
        }
        run_status(
            Command::new("flatpak").args(["build-bundle", "--oci"]).arg(work.join("repo")).arg(&out_oci).args([APP_ID, "stable"]),
            "flatpak build-bundle --oci",
        )?;
        println!("wrote OCI layout to {}", out_oci.display());
    }
    if opts.test {
        verify_sandbox(&out)?;
    }
    println!("==> done");
    Ok(())
}

#[cfg(unix)]
fn check_executable_bin(path: &Path, tarball: &Path) -> AppResult {
    use super::unix_package_fs::reject_symlink;
    use std::os::unix::fs::PermissionsExt;
    let meta = reject_symlink(path, "bin/pdfcraft")?;
    if !meta.is_file() || (meta.permissions().mode() & 0o111 == 0) {
        return Err(AppError::runtime(format!("error: {} has no bin/pdfcraft", tarball.display())));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_executable_bin(path: &Path, tarball: &Path) -> AppResult {
    if !path.is_file() {
        return Err(AppError::runtime(format!("error: {} has no bin/pdfcraft", tarball.display())));
    }
    Ok(())
}

fn verify_sandbox(bundle: &Path) -> AppResult {
    run_status(Command::new("flatpak").args(["install", "--user", "-y", "--noninteractive", "--reinstall"]).arg(bundle), "flatpak install")?;
    run_status(Command::new("flatpak").args(["info", "--user", APP_ID]), "flatpak info")?;
    run_status(Command::new("flatpak").args(["run", "--command=pdfcraft-cli", APP_ID, "--version"]), "flatpak cli --version")?;
    let id_check = format!("test \"$FLATPAK_ID\" = {APP_ID}");
    run_status(Command::new("flatpak").args(["run", "--command=sh", APP_ID, "-c", &id_check]), "flatpak id check")?;

    let home = non_empty_env("HOME").unwrap_or_default();
    let xdg_data = non_empty_env("XDG_DATA_HOME").map_or_else(|| PathBuf::from(home).join(".local/share"), PathBuf::from);
    let exports = xdg_data.join("flatpak/exports/share");
    if !exports.join(format!("applications/{APP_ID}.desktop")).is_file()
        || !exports.join(format!("icons/hicolor/scalable/apps/{APP_ID}.svg")).is_file()
    {
        return Err(AppError::runtime("error: missing exported desktop or SVG icon for Flatpak"));
    }
    let inner_check = format!(
        "test -f /app/share/metainfo/{APP_ID}.metainfo.xml && test -f /app/share/mime/packages/{APP_ID}.xml && test -d /app/share/pdfcraft/models"
    );
    run_status(Command::new("flatpak").args(["run", "--command=sh", APP_ID, "-c", &inner_check]), "flatpak inner files check")?;
    if !run_quiet(Command::new("flatpak").args(["run", "--command=sh", APP_ID, "-c", "command -v lp lpstat"])) {
        println!("note: no lp/lpstat in the runtime, so printing is unavailable in the Flatpak");
    }
    Ok(())
}
