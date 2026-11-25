// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! macOS certificate import into a private temporary keychain (`packaging/macos/import-cert.sh`).

use super::unix_package_env::{EnvCtx, non_empty_env};
use super::unix_package_fs::{create_private_temp_dir, run_capture, run_status, write_private_file};
use super::{AppError, AppResult};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const HELP: &str = "\
CI only: import the Developer ID certificate into a temporary keychain so codesign can use it.
Reads APPLE_CERTIFICATE, APPLE_CERTIFICATE_PASSWORD, KEYCHAIN_PASSWORD, RUNNER_TEMP, TMPDIR, GITHUB_ENV.";

struct SigningGuard {
    temp_dir: PathBuf,
    cert_path: PathBuf,
    keychain_path: PathBuf,
    secret_pass: String,
    keep_keychain_on_success: bool,
}

impl Drop for SigningGuard {
    fn drop(&mut self) {
        scrub_and_remove_file(&self.cert_path);
        clear_string(&mut self.secret_pass);
        if !self.keep_keychain_on_success {
            let _ = Command::new("security")
                .arg("delete-keychain")
                .arg(&self.keychain_path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = fs::remove_dir_all(&self.temp_dir);
        }
    }
}

fn clear_string(s: &mut String) {
    let buf = s.len();
    *s = "\0".repeat(buf);
    s.clear();
}

fn scrub_and_remove_file(path: &Path) {
    if let Ok(meta) = fs::symlink_metadata(path)
        && !meta.file_type().is_symlink()
        && meta.is_file()
    {
        if let Ok(mut f) = OpenOptions::new().write(true).open(path) {
            let zeros = vec![0u8; usize::try_from(meta.len()).unwrap_or(0)];
            let _ = f.write_all(&zeros);
            let _ = f.flush();
        }
        let _ = fs::remove_file(path);
    }
}

fn decode_base64_quad(chunk: &[u8; 4], is_last: bool, out: &mut Vec<u8>) -> AppResult {
    let v0 = b64_val(chunk[0])?;
    let v1 = b64_val(chunk[1])?;
    let pad2 = chunk[2] == b'=';
    let pad3 = chunk[3] == b'=';
    if (pad2 && !pad3) || ((pad2 || pad3) && !is_last) {
        return Err(AppError::runtime("error: invalid base64 padding in APPLE_CERTIFICATE"));
    }
    let v2 = if pad2 { 0 } else { b64_val(chunk[2])? };
    let v3 = if pad3 { 0 } else { b64_val(chunk[3])? };
    out.push((v0 << 2) | (v1 >> 4));
    if !pad2 {
        out.push((v1 << 4) | (v2 >> 2));
    }
    if !pad3 {
        out.push((v2 << 6) | v3);
    }
    Ok(())
}

pub fn decode_base64(input: &str) -> AppResult<Vec<u8>> {
    let clean: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !clean.len().is_multiple_of(4) {
        return Err(AppError::runtime("error: invalid base64 length in APPLE_CERTIFICATE"));
    }
    let mut out = Vec::with_capacity((clean.len() / 4) * 3);
    for (idx, chunk) in clean.as_chunks::<4>().0.iter().enumerate() {
        decode_base64_quad(chunk, idx + 1 == clean.len() / 4, &mut out)?;
    }
    Ok(out)
}

fn b64_val(byte: u8) -> AppResult<u8> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(AppError::runtime("error: invalid base64 byte in APPLE_CERTIFICATE")),
    }
}

pub fn parse_dev_id_identity(output: &str) -> Option<String> {
    for line in output.lines() {
        if line.contains("Developer ID Application")
            && let Some(hash) = line.split_whitespace().nth(1)
            && !hash.is_empty()
        {
            return Some(hash.to_owned());
        }
    }
    None
}

fn random_hex_24() -> AppResult<String> {
    let mut bytes = [0u8; 24];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let mut hex = String::with_capacity(48);
    for b in bytes {
        hex.push_str(&format!("{b:02x}"));
    }
    bytes = [0u8; 24];
    let _ = bytes;
    Ok(hex)
}

fn temp_base() -> PathBuf {
    non_empty_env("RUNNER_TEMP").or_else(|| non_empty_env("TMPDIR")).map_or_else(|| PathBuf::from("/tmp"), PathBuf::from)
}

pub fn run(env: &EnvCtx, args: &[String]) -> AppResult {
    if matches!(args, [f] if f == "-h" || f == "--help") {
        println!("{HELP}");
        return Ok(());
    }
    if !args.is_empty() {
        return Err(AppError::usage(format!("unknown argument: {}", args[0])));
    }
    let Some(cert_b64) = non_empty_env("APPLE_CERTIFICATE") else {
        env.warn("APPLE_CERTIFICATE is not set: macOS builds will be ad-hoc signed and not notarized");
        return Ok(());
    };

    let mut cert_bytes = decode_base64(&cert_b64)?;
    let temp_dir = create_private_temp_dir(&temp_base(), "pdfcraft-signing.")?;
    let cert_path = temp_dir.join("pdfcraft-signing.p12");
    let keychain_path = temp_dir.join("pdfcraft-signing.keychain-db");
    let kc_pass = match non_empty_env("KEYCHAIN_PASSWORD") {
        Some(p) => p,
        None => random_hex_24()?,
    };
    let mut guard = SigningGuard {
        temp_dir,
        cert_path: cert_path.clone(),
        keychain_path: keychain_path.clone(),
        secret_pass: kc_pass.clone(),
        keep_keychain_on_success: false,
    };

    let write_res = write_private_file(&cert_path, &cert_bytes);
    cert_bytes.fill(0);
    write_res?;

    import_into_keychain(&cert_path, &keychain_path, &kc_pass)?;
    scrub_and_remove_file(&cert_path);

    let identity = resolve_identity(&keychain_path)?;
    guard.keep_keychain_on_success = true;
    println!("Imported signing identity {identity}");
    emit_outputs(&identity, &keychain_path)?;
    Ok(())
}

fn import_into_keychain(cert: &Path, keychain: &Path, kc_pass: &str) -> AppResult {
    let cert_pass = env::var("APPLE_CERTIFICATE_PASSWORD").unwrap_or_default();
    run_status(Command::new("security").arg("create-keychain").arg("-p").arg(kc_pass).arg(keychain), "security create-keychain")?;
    run_status(Command::new("security").args(["set-keychain-settings", "-lut", "21600"]).arg(keychain), "security set-keychain-settings")?;
    run_status(Command::new("security").arg("unlock-keychain").arg("-p").arg(kc_pass).arg(keychain), "security unlock-keychain")?;
    run_status(
        Command::new("security")
            .arg("import")
            .arg(cert)
            .arg("-P")
            .arg(&cert_pass)
            .args(["-T", "/usr/bin/codesign", "-T", "/usr/bin/security", "-t", "cert", "-f", "pkcs12", "-k"])
            .arg(keychain),
        "security import",
    )?;
    let _ = run_capture(
        Command::new("security").args(["set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k"]).arg(kc_pass).arg(keychain),
        "security set-key-partition-list",
    )?;
    let existing = run_capture(Command::new("security").args(["list-keychains", "-d", "user"]), "security list-keychains")?;
    let existing_list: Vec<String> =
        String::from_utf8_lossy(&existing.stdout).lines().map(|l| l.trim().trim_matches('"').to_owned()).filter(|s| !s.is_empty()).collect();
    let mut set_list = Command::new("security");
    set_list.args(["list-keychains", "-d", "user", "-s"]).arg(keychain);
    for kc in &existing_list {
        set_list.arg(kc);
    }
    run_status(&mut set_list, "security list-keychains -s")
}

fn resolve_identity(keychain: &Path) -> AppResult<String> {
    let out = run_capture(Command::new("security").args(["find-identity", "-v", "-p", "codesigning"]).arg(keychain), "security find-identity")?;
    let text = String::from_utf8_lossy(&out.stdout);
    if let Some(id) = parse_dev_id_identity(&text) {
        return Ok(id);
    }
    eprint!("{text}");
    Err(AppError::runtime("error: no 'Developer ID Application' identity in APPLE_CERTIFICATE"))
}

fn emit_outputs(identity: &str, keychain: &Path) -> AppResult {
    if let Some(gh_env) = non_empty_env("GITHUB_ENV") {
        let mut f = OpenOptions::new().create(true).append(true).open(gh_env)?;
        writeln!(f, "MACOS_SIGN_IDENTITY={identity}")?;
        writeln!(f, "MACOS_KEYCHAIN={}", keychain.display())?;
    } else {
        println!("MACOS_SIGN_IDENTITY={identity}");
        println!("MACOS_KEYCHAIN={}", keychain.display());
    }
    Ok(())
}
