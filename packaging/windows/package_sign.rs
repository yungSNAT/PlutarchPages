// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Authenticode signing with signtool, using whichever signing material is configured (was `sign.ps1`).
//!
//! Tries, in order:
//!   1. A code-signing certificate file: `WINDOWS_CERTIFICATE` (base64 .pfx), `WINDOWS_CERTIFICATE_PASSWORD`.
//!   2. Azure Trusted Signing: `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`,
//!      `AZURE_SIGNING_ENDPOINT` (e.g. https://eus.codesigning.azure.net), `AZURE_SIGNING_ACCOUNT`,
//!      `AZURE_CERT_PROFILE`.
//!
//! With neither, it prints a warning and leaves the files unsigned, so test builds still produce
//! installers. This is the one place to adapt when the signing setup changes. Optional overrides:
//! `WINDOWS_TIMESTAMP_URL`, `SIGNTOOL` (path to signtool.exe). Downloading, unzipping and running
//! signtool go to the Windows tools `curl.exe` and `tar.exe` (System32) and signtool itself; the
//! downloaded client is checked against its pinned SHA-256 before it is unpacked.

use super::Result;
use super::package_meta::{non_empty_env, strip_windows_verbatim};
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const AZURE_VARS: [&str; 6] =
    ["AZURE_TENANT_ID", "AZURE_CLIENT_ID", "AZURE_CLIENT_SECRET", "AZURE_SIGNING_ENDPOINT", "AZURE_SIGNING_ACCOUNT", "AZURE_CERT_PROFILE"];
const TRUSTED_SIGNING_VERSION: &str = "1.0.95";
const TRUSTED_SIGNING_SHA256: &str = "3bfcf1e0a3cb42af1692f0a8ed45c15de070c2de86f28a59b2795d904d8a920f";
/// Deepest folder searched under the Windows Kits `bin` folder and the unpacked client.
const MAX_SEARCH_DEPTH: usize = 12;

#[derive(Debug, PartialEq, Eq)]
pub enum Source {
    Certificate,
    Azure,
}

/// The signing material configured, read through `get` (the environment outside tests).
pub fn select_source(get: impl Fn(&str) -> Option<String>) -> Option<Source> {
    if get("WINDOWS_CERTIFICATE").is_some() {
        Some(Source::Certificate)
    } else if AZURE_VARS.iter().all(|v| get(v).is_some()) {
        Some(Source::Azure)
    } else {
        None
    }
}

/// Sign `files` in place and verify the signatures, or warn and leave them unsigned.
pub fn sign_files(files: &[&Path]) -> Result {
    let files = files
        .iter()
        .map(|f| fs::canonicalize(f).map(strip_windows_verbatim).map_err(|e| format!("Cannot find path '{}': {e}", f.display()).into()))
        .collect::<Result<Vec<PathBuf>>>()?;
    let listed = files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>().join(", ");
    let Some(source) = select_source(non_empty_env) else {
        warn(&format!("Windows signing secrets not set (WINDOWS_CERTIFICATE or AZURE_*): leaving unsigned: {listed}"));
        return Ok(());
    };
    let signtool = find_signtool()?;
    let tmp = Scratch::new()?;
    let sign_args = match source {
        Source::Certificate => {
            println!("Signing with WINDOWS_CERTIFICATE: {listed}");
            let pfx = tmp.0.join("cert.pfx");
            let encoded = non_empty_env("WINDOWS_CERTIFICATE").unwrap_or_default();
            write_new(&pfx, &decode_base64(&encoded)?)?;
            let ts = non_empty_env("WINDOWS_TIMESTAMP_URL").unwrap_or_else(|| "http://timestamp.digicert.com".to_owned());
            // The password is used as set, spaces included.
            let password = env::var("WINDOWS_CERTIFICATE_PASSWORD").ok().filter(|p| !p.is_empty());
            certificate_args(&ts, &pfx, password.as_deref())
        }
        Source::Azure => {
            let account = non_empty_env("AZURE_SIGNING_ACCOUNT").unwrap_or_default();
            let profile = non_empty_env("AZURE_CERT_PROFILE").unwrap_or_default();
            println!("Signing with Azure Trusted Signing ({account} / {profile}): {listed}");
            // The dlib authenticates with DefaultAzureCredential, which reads AZURE_TENANT_ID,
            // AZURE_CLIENT_ID and AZURE_CLIENT_SECRET from the environment.
            let dlib = fetch_trusted_signing_dlib(&tmp.0)?;
            let metadata = tmp.0.join("metadata.json");
            let endpoint = non_empty_env("AZURE_SIGNING_ENDPOINT").unwrap_or_default();
            write_new(&metadata, azure_metadata(&endpoint, &account, &profile).as_bytes())?;
            let ts = non_empty_env("WINDOWS_TIMESTAMP_URL").unwrap_or_else(|| "http://timestamp.acs.microsoft.com".to_owned());
            azure_args(&ts, &dlib, &metadata)
        }
    };
    run_signtool(&signtool, sign_args, &files)?;
    run_signtool(&signtool, ["verify", "/pa", "/v"].map(OsString::from).to_vec(), &files)
}

fn warn(message: &str) {
    if env::var_os("GITHUB_ACTIONS").is_some_and(|v| !v.is_empty()) {
        println!("::warning::{message}");
    } else {
        eprintln!("WARNING: {message}");
    }
}

fn common_args() -> Vec<OsString> {
    ["sign", "/v", "/fd", "SHA256", "/td", "SHA256", "/d", "PlutarchPages", "/du", "https://github.com/yungSNAT/PlutarchPages"]
        .map(OsString::from)
        .to_vec()
}

pub fn certificate_args(ts: &str, pfx: &Path, password: Option<&str>) -> Vec<OsString> {
    let mut args = common_args();
    args.extend([OsString::from("/tr"), ts.into(), "/f".into(), pfx.into()]);
    if let Some(p) = password {
        args.extend([OsString::from("/p"), p.into()]);
    }
    args
}

pub fn azure_args(ts: &str, dlib: &Path, metadata: &Path) -> Vec<OsString> {
    let mut args = common_args();
    args.extend([OsString::from("/tr"), ts.into(), "/dlib".into(), dlib.into(), "/dmdf".into(), metadata.into()]);
    args
}

fn run_signtool(signtool: &Path, mut args: Vec<OsString>, files: &[PathBuf]) -> Result {
    let verb = args.first().map(|a| a.to_string_lossy().into_owned()).unwrap_or_default();
    args.extend(files.iter().map(OsString::from));
    let status = Command::new(signtool).args(&args).stdin(Stdio::null()).status()?;
    if !status.success() {
        let code = status.code().map_or_else(|| "none".to_owned(), |c| c.to_string());
        return Err(format!("signtool {verb} failed with exit code {code}").into());
    }
    Ok(())
}

fn find_signtool() -> Result<PathBuf> {
    if let Some(p) = non_empty_env("SIGNTOOL") {
        return Ok(PathBuf::from(p));
    }
    if let Some(path) = env::var_os("PATH")
        && let Some(found) = env::split_paths(&path).map(|d| d.join("signtool.exe")).find(|c| c.is_file())
    {
        return Ok(found);
    }
    let kits = non_empty_env("ProgramFiles(x86)").map(|p| Path::new(&p).join("Windows Kits").join("10").join("bin"));
    kits.and_then(|k| newest_x64_signtool(&k)).ok_or_else(|| "signtool.exe not found (install the Windows SDK)".into())
}

/// The x64 signtool.exe with the greatest path under the Windows Kits `bin` folder.
pub fn newest_x64_signtool(kits: &Path) -> Option<PathBuf> {
    let mut found = Vec::new();
    collect_files(kits, "signtool.exe", 0, &mut found);
    found.retain(|f| f.parent().and_then(Path::file_name).is_some_and(|d| d.eq_ignore_ascii_case("x64")));
    found.into_iter().max()
}

/// Files named `name` (any case) under `dir`, skipping links and stopping at `MAX_SEARCH_DEPTH`.
fn collect_files(dir: &Path, name: &str, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() && depth < MAX_SEARCH_DEPTH {
            collect_files(&path, name, depth.saturating_add(1), found);
        } else if kind.is_file() && entry.file_name().eq_ignore_ascii_case(name) {
            found.push(path);
        }
    }
}

fn system_tool(exe: &str) -> PathBuf {
    let root = non_empty_env("SystemRoot").unwrap_or_else(|| "C:\\Windows".to_owned());
    Path::new(&root).join("System32").join(exe)
}

fn fetch_trusted_signing_dlib(tmp: &Path) -> Result<PathBuf> {
    let pkg = tmp.join("trusted-signing.zip");
    let v = TRUSTED_SIGNING_VERSION;
    let uri = format!("https://api.nuget.org/v3-flatcontainer/microsoft.trusted.signing.client/{v}/microsoft.trusted.signing.client.{v}.nupkg");
    let mut curl = Command::new(system_tool("curl.exe"));
    curl.args(["--fail", "--silent", "--show-error", "--location", "--proto", "=https", "--proto-redir", "=https", "--output"]).arg(&pkg).arg(&uri);
    run_tool(&mut curl, "curl.exe download of Microsoft.Trusted.Signing.Client")?;
    if hex(&sha256(&fs::read(&pkg)?)) != TRUSTED_SIGNING_SHA256 {
        return Err(format!("Microsoft.Trusted.Signing.Client {v} checksum mismatch").into());
    }
    let client = tmp.join("client");
    fs::create_dir(&client)?;
    let mut tar = Command::new(system_tool("tar.exe"));
    tar.arg("-xf").arg(&pkg).arg("-C").arg(&client);
    run_tool(&mut tar, "tar.exe unpacking of Microsoft.Trusted.Signing.Client")?;
    find_x64_dlib(&client).ok_or_else(|| "Azure.CodeSigning.Dlib.dll not found in Microsoft.Trusted.Signing.Client".into())
}

/// The first Azure.CodeSigning.Dlib.dll, by path, inside an `x64` folder of the unpacked client.
pub fn find_x64_dlib(client: &Path) -> Option<PathBuf> {
    let mut found = Vec::new();
    collect_files(client, "Azure.CodeSigning.Dlib.dll", 0, &mut found);
    found.retain(|f| f.strip_prefix(client).is_ok_and(|rel| rel.components().any(|c| c.as_os_str().eq_ignore_ascii_case("x64"))));
    found.into_iter().min()
}

fn run_tool(cmd: &mut Command, label: &str) -> Result {
    let status = cmd.stdin(Stdio::null()).status()?;
    if !status.success() {
        return Err(format!("{label} failed ({status})").into());
    }
    Ok(())
}

pub fn azure_metadata(endpoint: &str, account: &str, profile: &str) -> String {
    format!(
        "{{\"Endpoint\": \"{}\", \"CodeSigningAccountName\": \"{}\", \"CertificateProfileName\": \"{}\"}}\n",
        json_escape(endpoint),
        json_escape(account),
        json_escape(profile)
    )
}

pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out
}

/// Standard base64 (RFC 4648 section 4) with whitespace ignored, as .NET's `FromBase64String` reads it.
pub fn decode_base64(text: &str) -> Result<Vec<u8>> {
    let bad = || "WINDOWS_CERTIFICATE is not valid base64".to_owned();
    let (mut out, mut acc, mut bits, mut symbols, mut pad) = (Vec::new(), 0u32, 0u32, 0usize, 0usize);
    for c in text.bytes().filter(|c| !c.is_ascii_whitespace()) {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => {
                pad = pad.saturating_add(1);
                continue;
            }
            _ => return Err(bad().into()),
        };
        if pad > 0 {
            return Err(bad().into());
        }
        symbols = symbols.saturating_add(1);
        acc = ((acc << 6) | u32::from(value)) & 0xffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    if pad > 2 || symbols.saturating_add(pad) % 4 != 0 {
        return Err(bad().into());
    }
    Ok(out)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 (FIPS 180-4), for the pinned checksum of the downloaded signing client.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    // usize is at most 64 bits on every Windows target, so the widening is lossless.
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for block in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (slot, word) in w.iter_mut().zip(block.as_chunks::<4>().0) {
            *slot = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let (w15, w2) = (w[i - 15], w[i - 2]);
            let s0 = w15.rotate_right(7) ^ w15.rotate_right(18) ^ (w15 >> 3);
            let s1 = w2.rotate_right(17) ^ w2.rotate_right(19) ^ (w2 >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for (k, wi) in K.iter().zip(w.iter()) {
            let t1 = hh
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(*k)
                .wrapping_add(*wi);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22)).wrapping_add((a & b) ^ (a & c) ^ (b & c));
            (hh, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(v);
        }
    }
    let mut out = [0u8; 32];
    for (chunk, v) in out.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        *chunk = v.to_be_bytes();
    }
    out
}

fn write_new(path: &Path, bytes: &[u8]) -> Result {
    let mut f = File::create_new(path)?;
    f.write_all(bytes)?;
    Ok(())
}

/// A private scratch folder, removed with everything in it when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self> {
        let base = env::temp_dir(); // nosemgrep: rust.lang.security.temp-dir.temp-dir
        for seq in 0..32u32 {
            let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
            let dir = base.join(format!("pdfcraft-sign-{}-{stamp}-{seq}", std::process::id()));
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        Err("Failed to create a private temporary folder for signing".into())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
