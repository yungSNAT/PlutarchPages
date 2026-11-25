// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Non-executable MSI policy fixture generation and WiX validation.

use super::win_checks_json::{InstallPolicy, parse_install_policy};
use super::{Result, join_rel, reject_symlink, strip_windows_verbatim};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn ensure_real_dir(root: &Path, rel_slash: &str) -> Result<PathBuf> {
    let mut cur = root.to_path_buf();
    for seg in rel_slash.split('/').filter(|s| !s.is_empty()) {
        if seg == "." || seg == ".." || seg.contains('\\') || seg.contains(':') {
            return Err(format!("Invalid relative directory: {rel_slash}").into());
        }
        cur.push(seg);
        match fs::symlink_metadata(&cur) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    return Err(format!("Symlink or non-directory refused at {}", cur.display()).into());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&cur)?,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(cur)
}

fn write_real_file(path: &Path, bytes: &[u8]) -> Result {
    if let Ok(meta) = fs::symlink_metadata(path)
        && (meta.file_type().is_symlink() || !meta.is_file())
    {
        return Err(format!("Symlink or non-file refused at {}", path.display()).into());
    }
    let mut f = File::create(path)?;
    f.write_all(bytes)?;
    Ok(())
}

/// Encodes a valid uncompressed 24-bit RGB BMP image (`width` x `height`).
pub fn encode_bmp24(width: u32, height: u32) -> Result<Vec<u8>> {
    let row_raw = width.checked_mul(3).ok_or("BMP width overflow")?;
    let row_bytes = row_raw.div_ceil(4).checked_mul(4).ok_or("BMP stride overflow")?;
    let pixel_bytes = row_bytes.checked_mul(height).ok_or("BMP pixel size overflow")?;
    let file_size = 54u32.checked_add(pixel_bytes).ok_or("BMP file size overflow")?;
    let w_i32 = i32::try_from(width).map_err(|_| "BMP width exceeds i32")?;
    let h_i32 = i32::try_from(height).map_err(|_| "BMP height exceeds i32")?;
    let mut buf = vec![0u8; file_size as usize];
    buf[0..2].copy_from_slice(b"BM");
    buf[2..6].copy_from_slice(&file_size.to_le_bytes());
    buf[10..14].copy_from_slice(&54u32.to_le_bytes());
    buf[14..18].copy_from_slice(&40u32.to_le_bytes());
    buf[18..22].copy_from_slice(&w_i32.to_le_bytes());
    buf[22..26].copy_from_slice(&h_i32.to_le_bytes());
    buf[26..28].copy_from_slice(&1u16.to_le_bytes());
    buf[28..30].copy_from_slice(&24u16.to_le_bytes());
    buf[34..38].copy_from_slice(&pixel_bytes.to_le_bytes());
    Ok(buf)
}

fn write_payload_fixtures(stage: &Path, payloads: &[String]) -> Result {
    for name in payloads {
        let rel = name.replace('\\', "/");
        if let Some((dir_rel, _)) = rel.rsplit_once('/') {
            let _ = ensure_real_dir(stage, dir_rel)?;
        }
        let dest = join_rel(stage, &rel)?;
        write_real_file(&dest, b"NON-EXECUTABLE MSI METADATA TEST FIXTURE\r\n")?;
    }
    Ok(())
}

fn write_stage_metadata(stage: &Path) -> Result {
    for folder in ["docs", "models", "notices"] {
        let dir = ensure_real_dir(stage, folder)?;
        write_real_file(
            &dir.join(format!("{folder}-fixture.txt")),
            b"Metadata test fixture, not a user file or model.\r\n",
        )?;
    }
    let models = stage.join("models");
    for model in ["metadata-one.rten", "metadata-two.rten"] {
        write_real_file(&models.join(model), b"NON-EXECUTABLE MODEL METADATA FIXTURE\r\n")?;
    }
    write_real_file(&stage.join("banner.bmp"), &encode_bmp24(493, 58)?)?;
    write_real_file(&stage.join("dialog.bmp"), &encode_bmp24(493, 312)?)?;
    write_real_file(&stage.join("licence.rtf"), b"{\\rtf1 Metadata test fixture.}\r\n")?;
    Ok(())
}

fn add_wix_defines(cmd: &mut Command, root: &Path, stage: &Path, icon_rel: &str) {
    let vars = [
        ("Version", "0.0.1".to_owned()),
        ("DisplayVersion", "0.0.1-metadata-test".to_owned()),
        ("BinDir", stage.display().to_string()),
        ("IconPath", root.join(icon_rel).display().to_string()),
        ("DocsDir", stage.join("docs").display().to_string()),
        ("ModelsDir", stage.join("models").display().to_string()),
        ("AppNoticesDir", stage.join("notices").display().to_string()),
        ("HasCraftFontLicences", "1".to_owned()),
        ("HasCli", "1".to_owned()),
        ("BannerBmp", stage.join("banner.bmp").display().to_string()),
        ("DialogBmp", stage.join("dialog.bmp").display().to_string()),
        ("UiBannerBmp", stage.join("banner.bmp").display().to_string()),
        ("UiDialogBmp", stage.join("dialog.bmp").display().to_string()),
        ("LicenseRtf", stage.join("licence.rtf").display().to_string()),
    ];
    cmd.args(["-culture", "en-US"]);
    for (k, v) in vars {
        cmd.args(["-d", &format!("{k}={v}")]);
    }
}

fn build_and_check_policy_msi(
    root: &Path,
    script_dir: &Path,
    stage: &Path,
    arch: &str,
    policy: &InstallPolicy,
    shell: &str,
) -> Result {
    let msi = stage.join(format!("{}-NOT-INSTALLABLE.msi", policy.id));
    let mut cmd = Command::new("wix");
    cmd.arg("build");
    for src in &policy.sources {
        cmd.arg(script_dir.join(src));
    }
    cmd.args(["-arch", arch]);
    for ext in &policy.extensions {
        cmd.args(["-ext", &format!("WixToolset.{ext}.wixext")]);
    }
    add_wix_defines(&mut cmd, root, stage, &policy.icon);
    cmd.arg("-o").arg(&msi);
    for loc in &policy.localization {
        cmd.args(["-loc", &script_dir.join(loc).display().to_string()]);
    }
    let status = cmd.stdin(Stdio::null()).status()?;
    if !status.success() {
        return Err(format!("WiX validation failed for {arch} ({status}).").into());
    }
    let policy_script = script_dir.join("check-install-policy.ps1").display().to_string();
    let shell_bin = match Path::new(shell).file_stem().and_then(|s| s.to_str()) {
        Some(s) if s.eq_ignore_ascii_case("pwsh") => "pwsh",
        Some(s) if s.eq_ignore_ascii_case("powershell") => "powershell",
        _ => return Err(format!("Unsupported PowerShell binary: {shell}").into()),
    };
    let policy_status = Command::new(shell_bin)
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", &policy_script, "-MsiPath", &msi.display().to_string()])
        .stdin(Stdio::null())
        .status()?;
    if !policy_status.success() {
        return Err(format!("Compiled installer policy failed for {arch} ({policy_status}).").into());
    }
    println!("PASS {arch} metadata-only package. This is not an executable installer.");
    Ok(())
}

fn run_install_policy_fixtures(
    script_dir: &Path,
    output_dir: &Path,
    architectures: &[String],
    shell: &str,
) -> Result {
    for arch in architectures {
        if !matches!(arch.as_str(), "x64" | "x86" | "arm64") {
            return Err(format!("Unsupported architecture: {arch}").into());
        }
    }
    let root = strip_windows_verbatim(fs::canonicalize(script_dir.join("..").join(".."))?);
    let policy = parse_install_policy(&fs::read_to_string(script_dir.join("install-policy.json"))?)?;
    fs::create_dir_all(output_dir)?;
    let _ = reject_symlink(output_dir, "output directory")?;
    let output = strip_windows_verbatim(fs::canonicalize(output_dir)?);
    for arch in architectures {
        let stage = ensure_real_dir(&output, arch)?;
        write_payload_fixtures(&stage, &policy.payloads)?;
        write_stage_metadata(&stage)?;
        build_and_check_policy_msi(&root, script_dir, &stage, arch, &policy, shell)?;
    }
    Ok(())
}

pub fn run_install_policy_cmd(rest: &[String]) -> Result {
    let (mut script_dir, mut output_dir, mut shell, mut archs) = (None, None, String::from("pwsh"), Vec::new());
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--script-dir" if i + 1 < rest.len() => { script_dir = Some(PathBuf::from(&rest[i + 1])); i += 2; }
            "--output-dir" if i + 1 < rest.len() => { output_dir = Some(PathBuf::from(&rest[i + 1])); i += 2; }
            "--shell" if i + 1 < rest.len() => { shell = rest[i + 1].clone(); i += 2; }
            "--arch" if i + 1 < rest.len() => { archs.push(rest[i + 1].clone()); i += 2; }
            other => return Err(format!("Unexpected option `{other}`").into()),
        }
    }
    if archs.is_empty() {
        archs = vec!["x64".into(), "x86".into(), "arm64".into()];
    }
    let s_dir = script_dir.unwrap_or_else(|| PathBuf::from("packaging").join("windows"));
    run_install_policy_fixtures(&s_dir, &output_dir.ok_or("--output-dir is required")?, &archs, &shell)
}
