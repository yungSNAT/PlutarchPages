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
//! archlinux:base-devel container. makepkg refuses root, so as root it does all its work on the
//! package as the unprivileged user `builder`, which it adds if needed (when not root, the caller
//! does that work): builder makes a scratch directory in `TMPDIR` with mktemp, so must be able to
//! write there, writes the recipe root gives it, unpacks the tarball, runs makepkg and checks the
//! package. Root then makes the package a new file in DIST, owned by DIST's owner, which builder's
//! cat fills, and builder removes the directory. DIST itself keeps its owner and mode.
//!
//! It trusts the checkout it runs in, that checkout's owner, the caller's `PATH`, whose directories
//! root searches for `runuser` and so must be root's, and DIST's path: root makes and opens DIST by
//! path, so DIST and every directory above it must be writable only by root, the caller or the
//! checkout's owner, as dist/release in the checkout is. builder is there because makepkg refuses
//! root, not as a sandbox: it runs the repository's own build, and in a local run its uid can be
//! the checkout owner's. What a root run guards is root's own lookups: none can be steered into a
//! write or removal outside DIST. Root reads Cargo.toml and opens DIST before its first program as
//! builder, and resolves no path in `TMPDIR`: it only passes the scratch directory's paths to
//! builder's programs. It makes the package a new name in the opened DIST, never through a link, and
//! gives it DIST's owner through the handle.
//!
//! The package is `plutarchpages`, not upstream's `ai.storyteller.pdfcraft`. Both install
//! /usr/bin/pdfcraft, so it provides and conflicts with `pdfcraft`, which upstream's provides too.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
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
    // The package goes into this directory, opened before anything runs as builder: renaming DIST
    // or putting a link in its place later can't move where it goes.
    let dist_dir = fs::File::open(&dist)?;
    let archive =
        args.get(1).filter(|archive| !archive.is_empty()).map_or_else(|| dist.join(format!("pdfcraft-{version}-linux-{arch}.tar.gz")), PathBuf::from);
    if !archive.is_file() {
        return Err(format!("{} not found (run packaging/linux/package.sh --formats tar first)", archive.display()).into());
    }

    let as_root = capture(Command::new("id").arg("-u"))?.stdout.trim_ascii() == b"0";
    if as_root {
        let quiet = |command: &mut Command| command.stdout(Stdio::null()).stderr(Stdio::null()).status();
        if !quiet(Command::new("id").args(["-u", "builder"]))?.success() {
            run(Command::new("useradd").args(["-m", "builder"]))?;
        }
    }

    // builder (or the caller, when not root) makes, fills and removes its own private directory, and
    // root only passes its path on: a lookup there by root could follow a link, or find another
    // directory under its name, that a builder process put there.
    let mut work = capture(unprivileged("mktemp", as_root).arg("-d"))?.stdout;
    if work.ends_with(b"\n") {
        work.pop();
    }
    let work = PathBuf::from(OsString::from_vec(work));
    let packaged = package(arch, &archive, &work, &dist_dir, &version, as_root);
    if let Err(error) = run(unprivileged("rm", as_root).args(["-rf", "--"]).arg(&work)) {
        eprintln!("warning: couldn't remove {}: {error}", work.display());
    }
    packaged
}

/// `program` run as `builder` when the helper runs as root, and as the caller otherwise.
fn unprivileged(program: &str, as_root: bool) -> Command {
    if !as_root {
        return Command::new(program);
    }
    let mut command = Command::new("runuser");
    command.args(["-u", "builder", "--", "env", "HOME=/home/builder", program]);
    command
}

/// Writes `contents` to `path` through `tee` run by [`unprivileged`], so root opens nothing there.
fn write_unprivileged(path: &Path, contents: &str, as_root: bool) -> Result {
    let mut tee = unprivileged("tee", as_root);
    tee.arg("--").arg(path).stdin(Stdio::piped()).stdout(Stdio::null());
    let mut child = tee.spawn()?;
    let written = child.stdin.take().map_or(Ok(()), |mut stdin| stdin.write_all(contents.as_bytes()));
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("{tee:?} failed with {status}").into());
    }
    written.map_err(|error| format!("writing {}: {error}", path.display()).into())
}

fn package(arch: &str, archive: &Path, work: &Path, dist_dir: &fs::File, version: &str, as_root: bool) -> Result {
    let pkgver = version.replace('-', "");
    if pkgver.is_empty() || !pkgver.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'+')) {
        return Err(format!("can't make a pkgver from {version}").into());
    }

    let stage = work.join("stage");
    run(unprivileged("mkdir", as_root).arg("--").arg(&stage))?;
    let config = work.join("makepkg.conf");
    write_unprivileged(&config, &format!("source /etc/makepkg.conf\nCARCH={arch}\nCHOST={arch}-unknown-linux-gnu\nPKGEXT=.pkg.tar.zst\n"), as_root)?;
    let recipe = include_str!("arch.PKGBUILD").replace("@VERSION@", &pkgver).replace("@ARCH@", arch);
    write_unprivileged(&work.join("PKGBUILD"), &recipe, as_root)?;
    run(unprivileged("tar", as_root).arg("-xzf").arg(archive).arg("-C").arg(&stage).arg("--strip-components=1"))?;
    // builder enters the scratch directory itself: once its tar has run, builder could have put a
    // link where the directory was, and a chdir by root would follow it.
    run(unprivileged("env", as_root).arg("-C").arg(work).arg("makepkg").arg("--config").arg(&config).args(["--nodeps", "--force"]))?;

    let built = work.join(format!("plutarchpages-{pkgver}-1-{arch}.pkg.tar.zst"));
    run(unprivileged("pacman", as_root).arg("-Qip").arg(&built))?;
    let listing = capture(unprivileged("pacman", as_root).arg("-Qlp").arg(&built))?;
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
            return Err(format!("{} has no {path}", built.display()).into());
        }
    }

    // Root writes only a new file in the opened DIST, never through a path in it that someone else
    // could have linked elsewhere, and reads the package only through builder.
    let name = format!("pdfcraft-{version}-linux-{arch}.pkg.tar.zst");
    let target = Path::new("/proc/self/fd").join(dist_dir.as_raw_fd().to_string()).join(&name);
    match fs::remove_file(&target) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    // The package is published, so others must read it; group and others never get write.
    let file = fs::OpenOptions::new().write(true).create_new(true).mode(0o644).open(&target)?; // NOSONAR
    if as_root {
        let owner = dist_dir.metadata()?;
        std::os::unix::fs::fchown(&file, Some(owner.uid()), Some(owner.gid()))?;
    }
    run(unprivileged("cat", as_root).arg("--").arg(&built).stdout(file))?;
    println!("wrote {name} in the output directory");
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
