// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared environment resolution, docs/model staging, templating, and SHA-256 helpers.

use super::unix_package_fs::{copy_dir_no_symlinks, copy_file_checked, ensure_real_dir_path, reject_symlink, run_status};
use super::{AppError, AppResult};
use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const DOC_FILES: &[&str] = &["README.md", "LICENSE", "LICENSE-MIT", "LICENSE-APACHE", "COPYRIGHT", "NOTICE", "ATTRIBUTION.md"];

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const SHA256_INIT: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];

pub struct EnvCtx {
    pub root: PathBuf,
    pub dist: PathBuf,
    pub target_dir: PathBuf,
    pub version: String,
    pub build_label: Option<String>,
    pub build_sha: String,
    pub build_date: String,
    pub github_actions: bool,
}

pub fn non_empty_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|s| !s.is_empty())
}

pub fn workspace_version(cargo_toml: &str) -> Option<String> {
    let mut in_pkg = false;
    for line in cargo_toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_pkg = trimmed == "[workspace.package]";
            continue;
        }
        if in_pkg
            && let Some((k, v)) = trimmed.split_once('=')
            && k.trim() == "version"
        {
            let cleaned = v.split('#').next().unwrap_or("").replace(['"', ' ', '\t'], "");
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }
    None
}

pub fn validate_build_label(raw: Option<String>) -> AppResult<Option<String>> {
    let Some(label) = raw.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let valid = !label.starts_with('.') && !label.ends_with('.') && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.');
    if !valid {
        return Err(AppError::runtime(format!("error: PDFCRAFT_BUILD_LABEL may hold only letters, digits and inner dots: {label}")));
    }
    Ok(Some(label))
}

pub fn utc_date_from_unix_secs(secs: u64) -> AppResult<String> {
    let days = i64::try_from(secs / 86_400).map_err(|_| AppError::runtime("timestamp overflow"))?;
    let z = days.checked_add(719_468).ok_or_else(|| AppError::runtime("date overflow"))?;
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

pub fn resolve_path_under(base: &Path, raw: PathBuf) -> AppResult<PathBuf> {
    let joined = if raw.is_absolute() { raw } else { base.join(raw) };
    let mut out = PathBuf::new();
    let mut depth: usize = 0;
    for comp in joined.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return Err(AppError::runtime(format!("error: path escapes root: {}", joined.display())));
                }
                let _ = out.pop();
                depth -= 1;
            }
            Component::Normal(seg) => {
                out.push(seg);
                depth += 1;
            }
        }
    }
    Ok(out)
}

impl EnvCtx {
    pub fn load(root_override: Option<PathBuf>) -> AppResult<Self> {
        let cwd = env::current_dir()?;
        let root_candidate = root_override.unwrap_or_else(|| cwd.clone());
        let root = fs::canonicalize(&root_candidate)?;
        let cargo_toml = fs::read_to_string(root.join("Cargo.toml"))?;
        let version = match non_empty_env("PDFCRAFT_VERSION") {
            Some(v) => v,
            None => workspace_version(&cargo_toml).ok_or_else(|| {
                AppError::runtime(format!("error: could not read [workspace.package] version from {}", root.join("Cargo.toml").display()))
            })?,
        };
        let build_label = validate_build_label(non_empty_env("PDFCRAFT_BUILD_LABEL"))?;
        let dist_raw = non_empty_env("DIST").map_or_else(|| root.join("dist/release"), PathBuf::from);
        let dist = resolve_path_under(&cwd, dist_raw)?;
        ensure_real_dir_path(&dist)?;
        let target_raw = non_empty_env("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
        let target_dir = resolve_path_under(&cwd, target_raw)?;
        let build_sha = non_empty_env("PDFCRAFT_BUILD_SHA").unwrap_or_else(|| git_head(&root));
        let build_date = match non_empty_env("PDFCRAFT_BUILD_DATE") {
            Some(d) => d,
            None => utc_date_from_unix_secs(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()))?,
        };
        Ok(Self { root, dist, target_dir, version, build_label, build_sha, build_date, github_actions: non_empty_env("GITHUB_ACTIONS").is_some() })
    }

    pub fn warn(&self, msg: &str) {
        if self.github_actions {
            println!("::warning::{msg}");
        } else {
            eprintln!("warning: {msg}");
        }
    }

    pub fn copy_docs(&self, dest: &Path) -> AppResult {
        ensure_real_dir_path(dest)?;
        for name in DOC_FILES {
            let src = self.root.join(name);
            if src.is_file() {
                copy_file_checked(&src, &dest.join(name))?;
            }
        }
        let licenses = self.root.join("licenses");
        if licenses.is_dir() {
            copy_dir_no_symlinks(&licenses, &dest.join("licenses"))?;
        }
        copy_font_licences(dest)?;
        Ok(())
    }

    pub fn stage_models(&self, dest: &Path, clear_cross_target: bool) -> AppResult {
        ensure_real_dir_path(dest)?;
        let abs_dest = fs::canonicalize(dest)?;
        let mut cmd = Command::new("cargo");
        cmd.arg("xtask").arg("models").arg(&abs_dest).current_dir(&self.root);
        if clear_cross_target {
            cmd.env_remove("CARGO_BUILD_TARGET");
        }
        run_status(&mut cmd, "cargo xtask models")?;
        let mut has_licence = false;
        let mut file_names = Vec::new();
        for entry in fs::read_dir(&abs_dest)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".part") {
                let _ = fs::remove_file(&path);
                continue;
            }
            if name.ends_with(".LICENCE.txt") {
                has_licence = true;
            }
            file_names.push(name);
        }
        let attr_non_empty = fs::metadata(abs_dest.join("ATTRIBUTION.txt")).is_ok_and(|m| m.len() > 0);
        if !has_licence || !attr_non_empty {
            return Err(AppError::runtime(format!("error: no OCR models in {} after cargo xtask models", abs_dest.display())));
        }
        file_names.sort();
        println!("OCR models in {}:", abs_dest.display());
        for name in file_names {
            println!("  {name}");
        }
        Ok(())
    }
}

fn copy_font_licences(dest: &Path) -> AppResult {
    let Some(fonts_root) = non_empty_env("CRAFT_FONTS_DIR") else {
        return Ok(());
    };
    let fonts_dir = PathBuf::from(fonts_root).join("fonts");
    if !fonts_dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&fonts_dir)? {
        let entry = entry?;
        let dir = entry.path();
        let ofl = dir.join("OFL.txt");
        if dir.is_dir() && ofl.is_file() {
            let family = entry.file_name().to_string_lossy().into_owned();
            copy_file_checked(&ofl, &dest.join(format!("OFL-{family}.txt")))?;
        }
    }
    Ok(())
}

fn git_head(root: &Path) -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

pub fn render_template(src: &Path, dst: &Path, replacements: &[(&str, &str)]) -> AppResult {
    reject_symlink(src, "template")?;
    let mut text = fs::read_to_string(src)?;
    for (pat, val) in replacements {
        text = text.replace(pat, val);
    }
    if let Ok(m) = fs::symlink_metadata(dst)
        && m.file_type().is_symlink()
    {
        return Err(AppError::runtime(format!("error: symlink refused at template output: {}", dst.display())));
    }
    fs::write(dst, text)?;
    Ok(())
}

pub fn run_cargo_build(env: &EnvCtx, targets: &[&str]) -> AppResult {
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--release", "--locked", "-p", "pdfcraft", "-p", "pdfcraft-cli"]);
    for target in targets {
        cmd.arg("--target").arg(target);
    }
    cmd.current_dir(&env.root).env("CARGO_TARGET_DIR", &env.target_dir);
    run_status(&mut cmd, "cargo build")
}

fn sha256_compress(state: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for (i, word) in w.iter_mut().take(16).enumerate() {
        let j = i * 4;
        *word = u32::from_be_bytes([block[j], block[j + 1], block[j + 2], block[j + 3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(SHA256_K[i]).wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (dst, add) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *dst = dst.wrapping_add(add);
    }
}

struct Sha256State {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

impl Sha256State {
    fn new() -> Self {
        Self { state: SHA256_INIT, buf: [0u8; 64], buf_len: 0, total_len: 0 }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        if self.buf_len > 0 {
            let take = data.len().min(64 - self.buf_len);
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                sha256_compress(&mut self.state, &block);
                self.buf_len = 0;
            }
        }
        let (chunks, rem) = data.as_chunks::<64>();
        for chunk in chunks {
            sha256_compress(&mut self.state, chunk);
        }
        if !rem.is_empty() {
            self.buf[..rem.len()].copy_from_slice(rem);
            self.buf_len = rem.len();
        }
    }

    fn finalize(mut self) -> String {
        let bit_len = self.total_len.wrapping_mul(8);
        self.buf[self.buf_len] = 0x80;
        self.buf_len += 1;
        if self.buf_len > 56 {
            self.buf[self.buf_len..64].fill(0);
            let block = self.buf;
            sha256_compress(&mut self.state, &block);
            self.buf_len = 0;
        }
        self.buf[self.buf_len..56].fill(0);
        self.buf[56..64].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buf;
        sha256_compress(&mut self.state, &block);
        let mut out = String::with_capacity(64);
        for word in self.state {
            for byte in word.to_be_bytes() {
                out.push_str(&format!("{byte:02x}"));
            }
        }
        out
    }
}

#[cfg(test)]
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256State::new();
    h.update(data);
    h.finalize()
}

pub fn sha256_file(path: &Path) -> AppResult<String> {
    reject_symlink(path, "file to hash")?;
    let mut f = File::open(path)?;
    let mut hasher = Sha256State::new();
    let mut chunk = [0u8; 65_536];
    loop {
        let n = f.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        hasher.update(&chunk[..n]);
    }
    Ok(hasher.finalize())
}

pub fn verify_sha256(expected: &str, path: &Path) -> AppResult {
    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(AppError::runtime(format!("error: sha256 mismatch for {}: expected {expected}, got {actual}", path.display())));
    }
    Ok(())
}
