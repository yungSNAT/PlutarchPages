// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Runtime font-notice regression suite (`test-font-notices`) and unit tests.

use super::{Result, assert_notice_names, collect_font_notices};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Result<Self> {
        let base = env::temp_dir(); // nosemgrep: rust.lang.security.temp-dir.temp-dir
        for _ in 0..32 {
            let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
            let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = base.join(format!("{prefix}-{}-{stamp}-{seq}", std::process::id()));
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err("Failed to create unique temporary directory".into())
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_rejected<T>(res: Result<T>, message: &str) -> Result {
    if res.is_ok() {
        return Err(format!("Expected rejection: {message}").into());
    }
    Ok(())
}

fn check_manifest_paths(fixture: &Path, core: &str, chinese: &str) -> Result {
    let manifest = fixture.join("fonts").join("manifest.txt");
    let zh = fixture.join("fonts").join("simplified-chinese-manifest.txt");
    fs::write(&manifest, format!("# comment\n{core}\n{core}\n"))?;
    fs::write(&zh, format!("{chinese}\n"))?;
    if collect_font_notices(fixture, true)?.len() != 2 {
        return Err("Expected both unique family notices".into());
    }
    for (bad, msg) in [
        ("fonts/core/OFL.txt", "another family cannot supply this notice"),
        ("../OFL.txt", "notice path escapes font input"),
    ] {
        fs::write(&zh, chinese.replace("fonts/chinese/OFL.txt", bad))?;
        assert_rejected(collect_font_notices(fixture, true), msg)?;
    }
    fs::write(&zh, format!("{chinese}\n"))?;
    Ok(())
}

fn check_manifest_missing(fixture: &Path) -> Result {
    let manifest = fixture.join("fonts").join("manifest.txt");
    let zh = fixture.join("fonts").join("simplified-chinese-manifest.txt");
    let zh_ofl = fixture.join("fonts").join("chinese").join("OFL.txt");
    fs::remove_file(&zh_ofl)?;
    assert_rejected(collect_font_notices(fixture, true), "missing Chinese notice")?;
    fs::write(&zh_ofl, b"")?;
    assert_rejected(collect_font_notices(fixture, true), "empty family notice")?;
    fs::remove_file(&zh)?;
    assert_rejected(collect_font_notices(fixture, true), "required Chinese manifest missing")?;
    if collect_font_notices(fixture, false)?.len() != 1 {
        return Err("Optional Chinese manifest may be absent".into());
    }
    fs::write(&manifest, "malformed\n")?;
    assert_rejected(collect_font_notices(fixture, false), "malformed metadata")
}

pub fn run_font_notice_regressions(craft_fonts_root: Option<&Path>) -> Result {
    let fixture = TempDir::new("pdfcraft-font-notices")?;
    for family in ["core", "chinese"] {
        let dir = fixture.0.join("fonts").join(family);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("OFL.txt"), "Synthetic notice fixture\n")?;
    }
    let core = "Core | Regular | fonts/core/face.ttf | Latn | OFL-1.1 | fonts/core/OFL.txt";
    let chinese = "Chinese | Regular | fonts/chinese/face.ttf | Hans | OFL-1.1 | fonts/chinese/OFL.txt";
    check_manifest_paths(&fixture.0, core, chinese)?;
    check_manifest_missing(&fixture.0)?;
    assert_notice_names(
        &["OFL-core.txt".into(), "OFL-chinese.txt".into()],
        &["SHORT.TXT|OFL-core.txt".into(), "OFL-chinese.txt".into()],
    )?;
    assert_rejected(
        assert_notice_names(&["OFL-core.txt".into(), "OFL-chinese.txt".into()], &["OFL-core.txt".into()]),
        "MSI lost one staged notice",
    )?;
    if let Some(root) = craft_fonts_root {
        let actual = collect_font_notices(root, true)?;
        println!("Verified {} notices from the actual native manifests", actual.len());
    }
    println!("Font notice regression checks passed");
    Ok(())
}

#[cfg(test)]
mod unit_tests {
    use super::super::win_checks_json::parse_install_policy;
    use super::super::win_checks_policy::encode_bmp24;
    use super::super::{assert_notice_names, parse_notice_names_args, strip_windows_verbatim};
    use super::{Result, run_font_notice_regressions};
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn font_notice_regressions_pass() -> Result {
        run_font_notice_regressions(None)
    }

    #[test]
    fn bmp24_matches_header_layout_and_rejects_overflow() -> Result {
        let banner = encode_bmp24(493, 58)?;
        assert_eq!(&banner[0..2], b"BM");
        assert_eq!(banner.len(), 85_894);
        assert_eq!(encode_bmp24(493, 312)?.len(), 54 + 1480 * 312);
        assert!(encode_bmp24(u32::MAX, 1).is_err());
        Ok(())
    }

    #[test]
    fn parses_utf8_and_surrogate_json_and_committed_policy() -> Result {
        let sample = r#"{"extra":true,"num":-0.5e+2,"nil":null,"id":"x","icon":"i","sources":[],"extensions":[],"localization":[],"payloads":["\u2026-\uD83D\uDE00"]}"#;
        let parsed = parse_install_policy(sample)?;
        assert_eq!(parsed.payloads, ["\u{2026}-\u{1F600}"]);
        for bad in [
            r#"{"id":"\uD800","icon":"i","sources":[],"extensions":[],"localization":[],"payloads":[]}"#,
            r#"{"id":"x","icon":"i","sources":[],"extensions":[],"localization":[],"payloads":[]} trailing"#,
            r#"{"id":"x","bad":tru,"icon":"i","sources":[],"extensions":[],"localization":[],"payloads":[]}"#,
            r#"{"id":"x","bad":01,"icon":"i","sources":[],"extensions":[],"localization":[],"payloads":[]}"#,
        ] {
            assert!(parse_install_policy(bad).is_err(), "accepted bad JSON: {bad}");
        }
        let manifest_path = Path::new("packaging/windows/install-policy.json");
        if manifest_path.is_file() {
            let p = parse_install_policy(&fs::read_to_string(manifest_path)?)?;
            assert_eq!(p.id, "pdfcraft");
            assert!(!p.sources.is_empty() && !p.payloads.is_empty());
        }
        Ok(())
    }

    #[test]
    fn notice_names_and_verbatim_helper() -> Result {
        assert_notice_names(&["--dash.txt".into()], &["SHORT|--dash.txt".into()])?;
        assert!(assert_notice_names(&["OFL-missing.txt".into()], &[]).is_err());
        let (exp, act) = parse_notice_names_args(&[
            "--expected-count".into(), "1".into(), "--a.txt".into(), "--b.txt".into(), "--a.txt".into(),
        ])?;
        assert_eq!(exp, ["--a.txt"]);
        assert_eq!(act, ["--b.txt", "--a.txt"]);
        assert_eq!(strip_windows_verbatim(PathBuf::from("\\\\?\\C:\\foo")), PathBuf::from("C:\\foo"));
        assert_eq!(strip_windows_verbatim(PathBuf::from("\\\\?\\UNC\\srv\\sh")), PathBuf::from("\\\\srv\\sh"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_in_font_notices() -> Result {
        use super::super::collect_font_notices;
        use super::TempDir;
        let outside = TempDir::new("pdfcraft-ofl-out")?;
        fs::write(outside.0.join("secret.txt"), "OUT")?;
        let root = TempDir::new("pdfcraft-ofl-root")?;
        let fam = root.0.join("fonts").join("core");
        fs::create_dir_all(&fam)?;
        std::os::unix::fs::symlink(outside.0.join("secret.txt"), fam.join("OFL.txt"))?;
        fs::write(
            root.0.join("fonts").join("manifest.txt"),
            "Core | Regular | fonts/core/face.ttf | Latn | OFL-1.1 | fonts/core/OFL.txt\n",
        )?;
        assert!(collect_font_notices(&root.0, false).is_err());
        Ok(())
    }
}
