// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Repackage a PlutarchPages Linux tarball as an Arch Linux package:
//!
//!   $DIST/pdfcraft-<version>-linux-<arch>.pkg.tar.zst
//!
//! Build it with `rustc --edition=2024 -D warnings packaging/linux/arch-package.rs -o arch-package`
//! (with `-C target-feature=+crt-static` to run it where there is no Rust), then from the workspace
//! root run `arch-package <x86_64|aarch64> [TARBALL]`: it reads Cargo.toml and the default DIST in
//! the current directory, where arch-package.sh found them from its own path. It knows the root by
//! packaging/linux/arch-package.rs; elsewhere it needs both `PDFCRAFT_VERSION` and `DIST`, and
//! refuses rather than guess.
//!
//! The version is `PDFCRAFT_VERSION`, or `[workspace.package] version` in Cargo.toml, and the output
//! directory `DIST`, or dist/release, as packaging/env.sh sets them. TARBALL, when not given or empty, is
//! $DIST/pdfcraft-<version>-linux-<arch>.tar.gz from package.sh. Needs makepkg. Nothing in the
//! tarball runs here, so an x86_64 machine can package aarch64. On another distribution run it in an
//! archlinux:base-devel container. makepkg refuses root, so as root it runs itself again as the
//! unprivileged user `builder`, which it adds if needed and makes the owner of DIST.
//!
//! The package is `plutarchpages`, not upstream's `ai.storyteller.pdfcraft`. Both install
//! /usr/bin/pdfcraft, so it provides and conflicts with `pdfcraft`, which upstream's provides too.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn run(command: &mut Command) -> Result {
    let status = command.status()?;
    if !status.success() {
        return Err(format!("{command:?} failed with {status}").into());
    }
    Ok(())
}

fn capture(command: &mut Command) -> Result<Output> {
    let output = command.output()?;
    if !output.status.success() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return Err(format!("{command:?} failed with {}", output.status).into());
    }
    Ok(output)
}

/// `[workspace.package] version` in a Cargo.toml, read as packaging/env.sh reads it: the third
/// field of the section's first line whose first field is `version`, without quotes.
fn workspace_version(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        if line.starts_with('[') {
            in_package = line == "[workspace.package]";
        } else if in_package && line.split_whitespace().next() == Some("version") {
            return Some(line.split_whitespace().nth(2).unwrap_or_default().replace('"', ""));
        }
    }
    None
}

fn arch_package() -> Result {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let arch = match args.first().map(|arch| arch.as_bytes()) {
        Some(b"x86_64") => "x86_64",
        Some(b"aarch64") => "aarch64",
        None | Some(b"") => return Err("usage: arch-package <x86_64|aarch64> [TARBALL]".into()),
        Some(other) => {
            // As arch-package.sh said it, bytes and all: a usage error, apart from failures.
            io::stderr().write_all(&[b"unsupported architecture ", other, b"\n"].concat())?;
            std::process::exit(2);
        }
    };
    let at_root = Path::new("packaging/linux/arch-package.rs").is_file();
    let outside = "not at the PlutarchPages workspace root: run from it, or set PDFCRAFT_VERSION and DIST";
    let version = match env::var("PDFCRAFT_VERSION") {
        Ok(version) if !version.is_empty() => version,
        Err(env::VarError::NotUnicode(_)) => return Err("PDFCRAFT_VERSION is not UTF-8".into()),
        _ if !at_root => return Err(outside.into()),
        _ => fs::read_to_string("Cargo.toml")
            .ok()
            .and_then(|manifest| workspace_version(&manifest))
            .filter(|version| !version.is_empty())
            .ok_or("could not read [workspace.package] version from Cargo.toml")?,
    };
    let dist = match env::var_os("DIST").filter(|dist| !dist.is_empty()) {
        Some(dist) => PathBuf::from(dist),
        None if at_root => PathBuf::from("dist/release"),
        None => return Err(outside.into()),
    };
    fs::create_dir_all(&dist)?;
    let archive =
        args.get(1).filter(|archive| !archive.is_empty()).map_or_else(|| dist.join(format!("pdfcraft-{version}-linux-{arch}.tar.gz")), PathBuf::from);
    if !archive.is_file() {
        return Err(format!("{} not found (run packaging/linux/package.sh --formats tar first)", archive.display()).into());
    }

    if capture(Command::new("id").arg("-u"))?.stdout.trim_ascii() == b"0" {
        let quiet = |command: &mut Command| command.stdout(Stdio::null()).stderr(Stdio::null()).status();
        if !quiet(Command::new("id").args(["-u", "builder"]))?.success() {
            run(Command::new("useradd").args(["-m", "builder"]))?;
        }
        run(Command::new("chown").args(["--", "builder"]).arg(&dist))?;
        let error = Command::new("runuser")
            .args(["-u", "builder", "--", "env", "HOME=/home/builder"])
            .arg(env::current_exe()?)
            .args(&args)
            .env("PDFCRAFT_VERSION", &version)
            .env("DIST", &dist)
            .exec();
        return Err(format!("couldn't run as builder: {error}").into());
    }

    // A private directory of its own, as arch-package.sh made: std makes none.
    let mut work = capture(Command::new("mktemp").arg("-d"))?.stdout;
    if work.ends_with(b"\n") {
        work.pop();
    }
    let work = PathBuf::from(OsString::from_vec(work));
    let packaged = package(arch, &archive, &work, &dist, &version);
    if let Err(error) = fs::remove_dir_all(&work) {
        eprintln!("warning: couldn't remove {}: {error}", work.display());
    }
    packaged
}

fn package(arch: &str, archive: &Path, work: &Path, dist: &Path, version: &str) -> Result {
    let pkgver = version.replace('-', "");
    if pkgver.is_empty() || !pkgver.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'+')) {
        return Err(format!("can't make a pkgver from {version}").into());
    }

    let stage = work.join("stage");
    fs::create_dir(&stage)?;
    run(Command::new("tar").arg("-xzf").arg(archive).arg("-C").arg(&stage).arg("--strip-components=1"))?;
    let config = work.join("makepkg.conf");
    fs::write(&config, format!("source /etc/makepkg.conf\nCARCH={arch}\nCHOST={arch}-unknown-linux-gnu\nPKGEXT=.pkg.tar.zst\n"))?;
    let recipe = include_str!("arch.PKGBUILD").replace("@VERSION@", &pkgver).replace("@ARCH@", arch);
    fs::write(work.join("PKGBUILD"), recipe)?;
    run(Command::new("makepkg").current_dir(work).arg("--config").arg(&config).args(["--nodeps", "--force"]))?;

    let built = work.join(format!("plutarchpages-{pkgver}-1-{arch}.pkg.tar.zst"));
    let output = dist.join(format!("pdfcraft-{version}-linux-{arch}.pkg.tar.zst"));
    run(Command::new("cp").arg("-f").arg(built).arg(&output))?;
    run(Command::new("pacman").arg("-Qip").arg(&output))?;
    let listing = capture(Command::new("pacman").arg("-Qlp").arg(&output))?;
    let listing = String::from_utf8_lossy(&listing.stdout);
    for path in [
        "/usr/bin/pdfcraft",
        "/usr/bin/pdfcraft-cli",
        "/usr/share/applications/ai.storyteller.pdfcraft.desktop",
        "/usr/share/metainfo/ai.storyteller.pdfcraft.metainfo.xml",
        "/usr/share/pdfcraft/models/ATTRIBUTION.txt",
        "/usr/share/licenses/plutarchpages/LICENSE-MIT",
    ] {
        if !listing.lines().any(|line| line.split_whitespace().nth(1) == Some(path)) {
            return Err(format!("{} has no {path}", output.display()).into());
        }
    }
    println!("wrote {}", output.display());
    Ok(())
}

fn main() -> std::process::ExitCode {
    match arch_package() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
