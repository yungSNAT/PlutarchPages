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

pub(crate) struct SecretBytes(Vec<u8>);

impl SecretBytes {
    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        scrub_vec(&mut self.0);
    }
}

pub(crate) struct SecretString(String);

impl SecretString {
    pub(crate) fn new(s: String) -> Self {
        Self(s)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        clear_string(&mut self.0);
    }
}

pub(crate) struct SigningGuard {
    pub(crate) temp_dir: PathBuf,
    pub(crate) cert_path: PathBuf,
    pub(crate) keychain_path: PathBuf,
    pub(crate) secret_pass: SecretString,
    pub(crate) keep_keychain_on_success: bool,
}

impl Drop for SigningGuard {
    fn drop(&mut self) {
        scrub_and_remove_file(&self.cert_path);
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

#[cfg(test)]
thread_local! {
    static TEST_SCRUBS: std::cell::RefCell<Vec<(usize, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn clear_test_scrubs() {
    TEST_SCRUBS.with(|s| s.borrow_mut().clear());
}

#[cfg(test)]
pub(crate) fn take_test_scrubs() -> Vec<(usize, bool)> {
    TEST_SCRUBS.with(|s| std::mem::take(&mut *s.borrow_mut()))
}

#[cfg(test)]
pub(crate) fn run_with_signing_guard<F>(mut guard: SigningGuard, work: F) -> AppResult
where
    F: FnOnce(&mut SigningGuard) -> AppResult,
{
    work(&mut guard)
}

fn scrub_vec(bytes: &mut Vec<u8>) {
    bytes.fill(0);
    let _ = std::hint::black_box(bytes.as_slice());
    #[cfg(test)]
    {
        let entry = (bytes.len(), bytes.iter().all(|&b| b == 0));
        TEST_SCRUBS.with(|s| s.borrow_mut().push(entry));
    }
}

fn clear_string(s: &mut String) {
    let mut bytes = std::mem::take(s).into_bytes();
    scrub_vec(&mut bytes);
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

pub(crate) fn decode_base64(input: &str) -> AppResult<SecretBytes> {
    let mut clean = SecretBytes(Vec::with_capacity(input.len()));
    for b in input.bytes() {
        if !b.is_ascii_whitespace() {
            clean.0.push(b);
        }
    }
    if !clean.0.len().is_multiple_of(4) {
        return Err(AppError::runtime("error: invalid base64 length in APPLE_CERTIFICATE"));
    }
    let mut out = SecretBytes(Vec::with_capacity((clean.0.len() / 4) * 3));
    for (idx, chunk) in clean.0.as_chunks::<4>().0.iter().enumerate() {
        decode_base64_quad(chunk, idx + 1 == clean.0.len() / 4, &mut out.0)?;
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

fn random_hex_24() -> AppResult<SecretString> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0u8; 24];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let mut hex = String::with_capacity(48);
    for &b in &bytes {
        hex.push(HEX[usize::from(b >> 4)] as char);
        hex.push(HEX[usize::from(b & 0x0f)] as char);
    }
    bytes.fill(0);
    let _ = std::hint::black_box(&bytes);
    Ok(SecretString::new(hex))
}

fn temp_base() -> AppResult<PathBuf> {
    let raw = non_empty_env("RUNNER_TEMP").or_else(|| non_empty_env("TMPDIR")).map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    Ok(fs::canonicalize(&raw)?)
}

pub fn run(env: &EnvCtx, args: &[String]) -> AppResult {
    if matches!(args, [f] if f == "-h" || f == "--help") {
        println!("{HELP}");
        return Ok(());
    }
    if !args.is_empty() {
        return Err(AppError::usage(format!("unknown argument: {}", args[0])));
    }
    let Some(raw_b64) = non_empty_env("APPLE_CERTIFICATE") else {
        env.warn("APPLE_CERTIFICATE is not set: macOS builds will be ad-hoc signed and not notarized");
        return Ok(());
    };
    let cert_b64 = SecretString::new(raw_b64);
    let cert_bytes = decode_base64(cert_b64.as_str())?;
    drop(cert_b64);

    let base = temp_base()?;
    let temp_dir = create_private_temp_dir(&base, "pdfcraft-signing.")?;
    let cert_path = temp_dir.join("pdfcraft-signing.p12");
    let keychain_path = temp_dir.join("pdfcraft-signing.keychain-db");
    let kc_pass = match non_empty_env("KEYCHAIN_PASSWORD") {
        Some(p) => SecretString::new(p),
        None => random_hex_24()?,
    };
    let mut guard = SigningGuard {
        temp_dir,
        cert_path: cert_path.clone(),
        keychain_path: keychain_path.clone(),
        secret_pass: kc_pass,
        keep_keychain_on_success: false,
    };

    let write_res = write_private_file(&cert_path, cert_bytes.as_slice());
    drop(cert_bytes);
    write_res?;

    import_into_keychain(&cert_path, &keychain_path, guard.secret_pass.as_str())?;
    scrub_and_remove_file(&cert_path);

    let identity = resolve_identity(&keychain_path)?;
    println!("Imported signing identity {identity}");
    emit_outputs(&identity, &keychain_path)?;
    guard.keep_keychain_on_success = true;
    Ok(())
}

pub(crate) fn import_with_runner<F>(cert: &Path, keychain: &Path, kc_pass: &str, cert_pass_raw: String, mut run_cmd: F) -> AppResult
where
    F: FnMut(&mut Command, &str) -> AppResult,
{
    let cert_pass = SecretString::new(cert_pass_raw);
    run_cmd(Command::new("security").arg("create-keychain").arg("-p").arg(kc_pass).arg(keychain), "security create-keychain")?;
    run_cmd(Command::new("security").args(["set-keychain-settings", "-lut", "21600"]).arg(keychain), "security set-keychain-settings")?;
    run_cmd(Command::new("security").arg("unlock-keychain").arg("-p").arg(kc_pass).arg(keychain), "security unlock-keychain")?;
    run_cmd(
        Command::new("security")
            .arg("import")
            .arg(cert)
            .arg("-P")
            .arg(cert_pass.as_str())
            .args(["-T", "/usr/bin/codesign", "-T", "/usr/bin/security", "-t", "cert", "-f", "pkcs12", "-k"])
            .arg(keychain),
        "security import",
    )?;
    drop(cert_pass);
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
    run_cmd(&mut set_list, "security list-keychains -s")
}

fn read_optional_utf8_env(name: &str) -> AppResult<String> {
    match env::var(name) {
        Ok(v) => Ok(v),
        Err(env::VarError::NotPresent) => Ok(String::new()),
        Err(env::VarError::NotUnicode(_)) => Err(AppError::runtime(format!("error: {name} is not valid UTF-8"))),
    }
}

fn import_into_keychain(cert: &Path, keychain: &Path, kc_pass: &str) -> AppResult {
    let cert_pass_raw = read_optional_utf8_env("APPLE_CERTIFICATE_PASSWORD")?;
    import_with_runner(cert, keychain, kc_pass, cert_pass_raw, run_status)
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

pub(crate) fn emit_outputs_to(identity: &str, keychain: &Path, gh_env: Option<&str>) -> AppResult {
    if let Some(path) = gh_env.filter(|s| !s.is_empty()) {
        let mut f = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(f, "MACOS_SIGN_IDENTITY={identity}")?;
        writeln!(f, "MACOS_KEYCHAIN={}", keychain.display())?;
    } else {
        println!("MACOS_SIGN_IDENTITY={identity}");
        println!("MACOS_KEYCHAIN={}", keychain.display());
    }
    Ok(())
}

fn emit_outputs(identity: &str, keychain: &Path) -> AppResult {
    emit_outputs_to(identity, keychain, non_empty_env("GITHUB_ENV").as_deref())
}
