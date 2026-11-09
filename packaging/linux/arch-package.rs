// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

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

fn package() -> Result {
    let mut args = env::args_os().skip(1);
    let arch = args.next().ok_or("missing package architecture")?;
    let arch = arch.to_str().ok_or("architecture is not UTF-8")?;
    if !matches!(arch, "x86_64" | "aarch64") {
        return Err(format!("unsupported architecture {arch}").into());
    }
    let archive = PathBuf::from(args.next().ok_or("missing Linux tarball")?);
    let work = PathBuf::from(args.next().ok_or("missing scratch directory")?);
    let dist = PathBuf::from(env::var_os("DIST").ok_or("DIST is unset")?);
    let version = env::var("VERSION")?;
    let pkgver = version.replace('-', "");
    if pkgver.is_empty() || !pkgver.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'+')) {
        return Err(format!("can't make a pkgver from {version}").into());
    }

    let stage = work.join("stage");
    fs::create_dir(&stage)?;
    run(Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&stage).arg("--strip-components=1"))?;
    let config = work.join("makepkg.conf");
    fs::write(&config, format!("source /etc/makepkg.conf\nCARCH={arch}\nCHOST={arch}-unknown-linux-gnu\nPKGEXT=.pkg.tar.zst\n"))?;
    let recipe = include_str!("arch.PKGBUILD").replace("@VERSION@", &pkgver).replace("@ARCH@", arch);
    fs::write(work.join("PKGBUILD"), recipe)?;
    run(Command::new("makepkg").current_dir(&work).arg("--config").arg(&config).args(["--nodeps", "--force"]))?;

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
    match package() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
