// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! macOS app bundle, DMG, codesign & notarization packager (`packaging/macos/package.sh`).

use super::unix_package_env::{EnvCtx, non_empty_env, render_template};
use super::unix_package_fs::{copy_file_checked, create_symlink_unix, ensure_real_dir_path, reset_real_dir, run_staged_binary, run_status};
use super::{AppError, AppResult};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const DEPLOYMENT_TARGET: &str = "11.0";

const HELP: &str = "\
Build, sign and (optionally) notarize the macOS release artifacts:
  $DIST/pdfcraft-<version>-macos-<arch>.dmg
  $DIST/pdfcraft-cli-<version>-macos-<arch>.zip
Usage: packaging/macos/package.sh [--arch universal|aarch64|x86_64] [--skip-build]";

pub fn targets_for_arch(arch: &str) -> AppResult<&'static [&'static str]> {
    match arch {
        "universal" => Ok(&["aarch64-apple-darwin", "x86_64-apple-darwin"]),
        "aarch64" => Ok(&["aarch64-apple-darwin"]),
        "x86_64" => Ok(&["x86_64-apple-darwin"]),
        other => Err(AppError::usage(format!("unknown --arch {other}"))),
    }
}

pub fn parse_macos_args(args: &[String]) -> AppResult<Option<(String, bool)>> {
    let mut arch = String::from("universal");
    let mut skip_build = false;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--arch" => {
                let Some(v) = args.get(idx + 1) else {
                    return Err(AppError::usage("--arch requires a value"));
                };
                arch.clone_from(v);
                idx += 2;
            }
            "--skip-build" => {
                skip_build = true;
                idx += 1;
            }
            "-h" | "--help" => return Ok(None),
            other => return Err(AppError::usage(format!("unknown argument: {other}"))),
        }
    }
    let _ = targets_for_arch(&arch)?;
    Ok(Some((arch, skip_build)))
}

fn read_json_string(s: &str) -> Option<(String, &str)> {
    let inner = s.strip_prefix('"')?;
    let mut out = String::new();
    let mut escaped = false;
    for (idx, ch) in inner.char_indices() {
        if escaped {
            out.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some((out, &inner[idx + 1..]));
        } else {
            out.push(ch);
        }
    }
    None
}

pub fn extract_json_string_field(json: &str, key: &str) -> Option<String> {
    let mut rest = json.trim_start();
    let mut depth: i32 = 0;
    while !rest.is_empty() {
        if let Some(next) = rest.strip_prefix('"') {
            let (s, after) = read_json_string(rest)?;
            let after_ws = after.trim_start();
            if depth == 1
                && s == key
                && let Some(val_start) = after_ws.strip_prefix(':')
            {
                return read_json_string(val_start.trim_start()).map(|(v, _)| v);
            }
            rest = after_ws;
            let _ = next;
        } else {
            let ch = rest.chars().next()?;
            match ch {
                '{' | '[' => depth += 1,
                '}' | ']' => depth = depth.saturating_sub(1),
                _ => {}
            }
            rest = rest[ch.len_utf8()..].trim_start();
        }
    }
    None
}

struct SignCtx {
    identity: String,
    keychain: Option<String>,
    apple_creds: Option<(String, String, String)>,
}

impl SignCtx {
    fn from_env(env: &EnvCtx) -> Self {
        let identity = non_empty_env("MACOS_SIGN_IDENTITY").unwrap_or_else(|| String::from("-"));
        let keychain = non_empty_env("MACOS_KEYCHAIN");
        let creds = match (non_empty_env("APPLE_ID"), non_empty_env("APPLE_PASSWORD"), non_empty_env("APPLE_TEAM_ID")) {
            _ if identity == "-" => {
                env.warn("macOS: ad-hoc signing (no MACOS_SIGN_IDENTITY); artifacts are not notarized");
                None
            }
            (Some(id), Some(pw), Some(team)) => Some((id, pw, team)),
            _ => {
                env.warn("macOS: APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID incomplete; signed but not notarized");
                None
            }
        };
        Self { identity, keychain, apple_creds: creds }
    }

    fn sign(&self, extra: &[&str], target: &Path) -> AppResult {
        let mut cmd = Command::new("codesign");
        cmd.args(["--force", "--sign", &self.identity]);
        if let Some(kc) = &self.keychain {
            cmd.arg("--keychain").arg(kc);
        }
        cmd.arg(if self.identity == "-" { "--timestamp=none" } else { "--timestamp" });
        cmd.args(extra).arg(target);
        run_status(&mut cmd, "codesign")
    }

    fn notarize(&self, file: &Path) -> AppResult {
        let Some((apple_id, password, team_id)) = &self.apple_creds else {
            return Ok(());
        };
        let name = file.file_name().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        println!("==> notarizing {name} (this can take a few minutes)");
        let out = Command::new("xcrun")
            .arg("notarytool")
            .arg("submit")
            .arg(file)
            .args(["--apple-id", apple_id, "--password", password, "--team-id", team_id])
            .args(["--wait", "--timeout", "1h", "--output-format", "json"])
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .output()?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        println!("{stdout}");
        let id = extract_json_string_field(&stdout, "id");
        let status = extract_json_string_field(&stdout, "status");
        if status.as_deref() != Some("Accepted") {
            if let Some(sub_id) = id {
                let _ = Command::new("xcrun")
                    .args(["notarytool", "log", &sub_id, "--apple-id", apple_id, "--password", password, "--team-id", team_id])
                    .stdin(Stdio::null())
                    .status();
            }
            let st = status.as_deref().unwrap_or("unknown");
            return Err(AppError::runtime(format!("error: notarization of {name} failed (status: {st})")));
        }
        Ok(())
    }
}

fn stapler_retry(action: &str, target: &Path) -> AppResult {
    let name = target.file_name().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    for try_idx in 1..=4u64 {
        let ok = Command::new("xcrun").args(["stapler", action]).arg(target).stdin(Stdio::null()).status()?.success();
        if ok {
            return Ok(());
        }
        if try_idx < 4 {
            eprintln!("stapler {action} failed (try {try_idx} of 4); retrying in {} s", try_idx * 20);
            thread::sleep(Duration::from_secs(try_idx * 20));
        }
    }
    Err(AppError::runtime(format!("error: stapler {action} {name} failed 4 times")))
}

fn hdiutil_retry(env: &EnvCtx, what: &str, build_cmd: impl Fn() -> Command) -> AppResult {
    for attempt in 1..=5u64 {
        let mut cmd = build_cmd();
        if cmd.stdin(Stdio::null()).status()?.success() {
            return Ok(());
        }
        if attempt == 5 {
            return Err(AppError::runtime(format!("hdiutil {what} failed 5 times")));
        }
        env.warn(&format!("hdiutil {what} failed (attempt {attempt} of 5); retrying in {}s", attempt * 10));
        thread::sleep(Duration::from_secs(attempt * 10));
    }
    Ok(())
}

pub fn run(env: &EnvCtx, args: &[String]) -> AppResult {
    let Some((arch, skip_build)) = parse_macos_args(args)? else {
        println!("{HELP}");
        return Ok(());
    };
    let targets = targets_for_arch(&arch)?;
    let sign_ctx = SignCtx::from_env(env);
    let short_version = env.version.split('-').next().unwrap_or(&env.version);
    let work = env.target_dir.join("macos-package");
    let app = work.join("PlutarchPages.app");
    let dmg = env.dist.join(format!("pdfcraft-{}-macos-{arch}.dmg", env.version));
    let cli_zip = env.dist.join(format!("pdfcraft-cli-{}-macos-{arch}.zip", env.version));

    println!(
        "==> PlutarchPages {} for macOS ({arch}), identity: {}, notarize: {}",
        env.version,
        sign_ctx.identity,
        u8::from(sign_ctx.apple_creds.is_some())
    );
    if !skip_build {
        let mut cmd = Command::new("cargo");
        cmd.args(["build", "--release", "--locked", "-p", "pdfcraft", "-p", "pdfcraft-cli"]);
        for t in targets {
            cmd.arg("--target").arg(t);
        }
        cmd.current_dir(&env.root).env("CARGO_TARGET_DIR", &env.target_dir).env("MACOSX_DEPLOYMENT_TARGET", DEPLOYMENT_TARGET);
        run_status(&mut cmd, "cargo build macos")?;
    }

    reset_real_dir(&work)?;
    lipo_binaries(env, targets, &work.join("bin"))?;
    assemble_and_sign_app(env, &sign_ctx, &work, &app, short_version)?;
    build_and_sign_dmg(env, &sign_ctx, &work, &app, &dmg)?;
    build_cli_zip(env, &sign_ctx, &work, &cli_zip, &arch)?;

    run_staged_binary(&work.join("bin/pdfcraft-cli"), &["--version"], "pdfcraft-cli --version")?;
    println!("==> done");
    Ok(())
}

fn lipo_binaries(env: &EnvCtx, targets: &[&str], out_bin_dir: &Path) -> AppResult {
    ensure_real_dir_path(out_bin_dir)?;
    for bin in ["pdfcraft", "pdfcraft-cli"] {
        let out_bin = out_bin_dir.join(bin);
        let mut cmd = Command::new("lipo");
        cmd.arg("-create").arg("-output").arg(&out_bin);
        for t in targets {
            cmd.arg(env.target_dir.join(t).join("release").join(bin));
        }
        run_status(&mut cmd, "lipo -create")?;
        run_status(Command::new("lipo").arg("-info").arg(&out_bin), "lipo -info")?;
    }
    Ok(())
}

fn stage_app_bundle(env: &EnvCtx, work: &Path, app: &Path, short_version: &str) -> AppResult {
    let here = env.root.join("packaging/macos");
    println!("==> assembling {}", app.display());
    ensure_real_dir_path(&app.join("Contents/MacOS"))?;
    ensure_real_dir_path(&app.join("Contents/Resources"))?;
    copy_file_checked(&work.join("bin/pdfcraft"), &app.join("Contents/MacOS/PlutarchPages"))?;
    copy_file_checked(&env.root.join("assets/app-icon/pdfcraft.icns"), &app.join("Contents/Resources/PlutarchPages.icns"))?;
    copy_file_checked(&env.root.join("NOTICE"), &app.join("Contents/Resources/NOTICE"))?;
    copy_file_checked(&here.join("Credits.rtf"), &app.join("Contents/Resources/Credits.rtf"))?;
    env.copy_docs(&app.join("Contents/Resources"))?;
    env.stage_models(&app.join("Contents/Resources/models"), false)?;
    let sha = if env.build_sha.is_empty() { "unknown" } else { &env.build_sha };
    let plist = app.join("Contents/Info.plist");
    render_template(&here.join("Info.plist.in"), &plist, &[("@VERSION@", &env.version), ("@SHORT_VERSION@", short_version), ("@BUILD_SHA@", sha)])?;
    run_status(Command::new("plutil").arg("-lint").arg(&plist), "plutil -lint")?;
    fs::write(app.join("Contents/PkgInfo"), b"APPL????")?;
    Ok(())
}

fn assemble_and_sign_app(env: &EnvCtx, sign_ctx: &SignCtx, work: &Path, app: &Path, short_version: &str) -> AppResult {
    stage_app_bundle(env, work, app, short_version)?;
    let ent = env.root.join("packaging/macos/entitlements.plist");
    let ent_str = ent.to_string_lossy();
    sign_ctx.sign(&["--options", "runtime", "--entitlements", ent_str.as_ref()], &app.join("Contents/MacOS/PlutarchPages"))?;
    sign_ctx.sign(&["--options", "runtime", "--entitlements", ent_str.as_ref()], app)?;
    run_status(Command::new("codesign").args(["--verify", "--strict", "--deep", "--verbose=2"]).arg(app), "codesign verify app")?;
    if sign_ctx.apple_creds.is_some() {
        let notarize_zip = work.join("PlutarchPages-notarize.zip");
        run_status(Command::new("ditto").args(["-c", "-k", "--keepParent"]).arg(app).arg(&notarize_zip), "ditto app zip")?;
        sign_ctx.notarize(&notarize_zip)?;
        stapler_retry("staple", app)?;
        stapler_retry("validate", app)?;
        run_status(Command::new("spctl").args(["--assess", "--type", "execute", "-vvv"]).arg(app), "spctl app")?;
    }
    Ok(())
}

fn build_and_sign_dmg(env: &EnvCtx, sign_ctx: &SignCtx, work: &Path, app: &Path, dmg: &Path) -> AppResult {
    let here = env.root.join("packaging/macos");
    println!("==> building {}", dmg.display());
    let stage = work.join("dmg");
    ensure_real_dir_path(&stage)?;
    run_status(Command::new("ditto").arg(app).arg(stage.join("PlutarchPages.app")), "ditto dmg")?;
    create_symlink_unix("/Applications", &stage.join("Applications"))?;
    copy_file_checked(&here.join("dmg/background.tiff"), &stage.join(".background/background.tiff"))?;
    copy_file_checked(&here.join("dmg/dmg-layout.DS_Store"), &stage.join(".DS_Store"))?;
    if dmg.exists() {
        fs::remove_file(dmg)?;
    }

    hdiutil_retry(env, "create", || {
        let mut c = Command::new("hdiutil");
        c.args(["create", "-srcfolder"])
            .arg(&stage)
            .args(["-volname", "PlutarchPages", "-fs", "HFS+", "-format", "UDZO", "-imagekey", "zlib-level=9", "-ov"])
            .arg(dmg);
        c
    })?;
    sign_ctx.sign(&[], dmg)?;
    run_status(Command::new("codesign").args(["--verify", "--strict", "--verbose=2"]).arg(dmg), "codesign verify dmg")?;

    let mount = work.join("dmg-check");
    reset_real_dir(&mount)?;
    hdiutil_retry(env, "attach", || {
        let mut c = Command::new("hdiutil");
        c.args(["attach", "-nobrowse", "-readonly", "-mountpoint"]).arg(&mount).arg(dmg).stdout(Stdio::null());
        c
    })?;
    let verify_res = run_status(
        Command::new("codesign").args(["--verify", "--strict", "--deep", "--verbose=2"]).arg(mount.join("PlutarchPages.app")),
        "codesign verify mounted app",
    );
    let detach_res = hdiutil_retry(env, "detach", || {
        let mut c = Command::new("hdiutil");
        c.arg("detach").arg(&mount).stdout(Stdio::null());
        c
    });
    verify_res?;
    verify_dmg_post_attach(sign_ctx, detach_res, dmg)?;
    Ok(())
}

fn verify_dmg_post_attach(sign_ctx: &SignCtx, detach_res: AppResult, dmg: &Path) -> AppResult {
    detach_res?;
    if sign_ctx.apple_creds.is_some() {
        sign_ctx.notarize(dmg)?;
        stapler_retry("staple", dmg)?;
        stapler_retry("validate", dmg)?;
        run_status(
            Command::new("spctl").args(["--assess", "--type", "open", "--context", "context:primary-signature", "-vvv"]).arg(dmg),
            "spctl dmg",
        )?;
    }
    Ok(())
}

fn build_cli_zip(env: &EnvCtx, sign_ctx: &SignCtx, work: &Path, cli_zip: &Path, arch: &str) -> AppResult {
    println!("==> building {}", cli_zip.display());
    let cli_dir = work.join(format!("pdfcraft-cli-{}-macos-{arch}", env.version));
    ensure_real_dir_path(&cli_dir)?;
    let cli_bin = cli_dir.join("pdfcraft-cli");
    copy_file_checked(&work.join("bin/pdfcraft-cli"), &cli_bin)?;
    env.copy_docs(&cli_dir)?;
    sign_ctx.sign(&["--options", "runtime"], &cli_bin)?;
    run_status(Command::new("codesign").args(["--verify", "--strict", "--verbose=2"]).arg(&cli_bin), "codesign verify cli")?;
    if cli_zip.exists() {
        fs::remove_file(cli_zip)?;
    }
    run_status(Command::new("ditto").args(["-c", "-k", "--keepParent"]).arg(&cli_dir).arg(cli_zip), "ditto cli zip")?;
    sign_ctx.notarize(cli_zip)?;
    Ok(())
}
