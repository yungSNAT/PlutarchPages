// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit tests for `packaging/windows/package.rs`.

#[cfg(test)]
mod unit_tests {
    use super::super::package_fs::{copy_file_checked, ensure_real_dir_path, reset_real_dir};
    use super::super::package_pe::verify_pe_bytes;
    use super::super::{
        PortableSources, Result, arch_spec, clear_existing_zip, copy_notice_line, copy_portable_notices, parse_args, remove_part_entry,
        resolve_dir_path, resolve_version, stage_portable_dir, strip_windows_verbatim, utc_date_from_unix_secs, validate_flags,
    };
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(prefix: &str) -> Result<Self> {
            let base = env::temp_dir(); // nosemgrep: rust.lang.security.temp-dir.temp-dir
            for _ in 0..32 {
                let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
                let seq = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
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

    fn make_pe(machine: u16, opt_magic: u16, subsystem: u16) -> Vec<u8> {
        let mut buf = vec![0u8; 0x120];
        buf[0..2].copy_from_slice(b"MZ");
        buf[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        buf[0x80..0x84].copy_from_slice(b"PE\0\0");
        buf[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        buf[0x98..0x9A].copy_from_slice(&opt_magic.to_le_bytes());
        buf[0xDC..0xDE].copy_from_slice(&subsystem.to_le_bytes());
        buf
    }

    #[test]
    fn flag_combinations_match_package_ps1_rules() -> Result {
        validate_flags(false, false, false)?;
        validate_flags(false, false, true)?;
        validate_flags(true, true, false)?;
        validate_flags(true, false, true)?;
        assert!(validate_flags(false, true, false).is_err());
        assert!(validate_flags(true, true, true).is_err());
        assert!(validate_flags(true, false, false).is_err());
        Ok(())
    }

    #[test]
    fn version_and_msi_version_parsing() -> Result {
        let toml = "[workspace]\n[workspace.package]\nversion_foo = \"9.9.9\"\nversion = \"0.22.0-rc.1\"\nedition = \"2024\"\n";
        assert_eq!(resolve_version(toml, None)?, ("0.22.0-rc.1".into(), "0.22.0".into()));
        assert_eq!(resolve_version(toml, Some("1.4.2"))?, ("1.4.2".into(), "1.4.2".into()));
        assert!(resolve_version("[workspace.package]\nversion_foo = \"9.9.9\"\nedition = \"2024\"\n", None).is_err());
        assert!(resolve_version(toml, Some("-invalid")).is_err());
        Ok(())
    }

    #[test]
    fn arch_specs_cover_x64_x86_arm64() -> Result {
        assert_eq!(arch_spec("x64")?.machine, 0x8664);
        assert_eq!(arch_spec("x86")?.machine, 0x014C);
        assert_eq!(arch_spec("arm64")?.machine, 0xAA64);
        assert!(arch_spec("riscv64").is_err());
        Ok(())
    }

    #[test]
    fn utc_date_conversion_handles_epoch_and_leap_day() -> Result {
        assert_eq!(utc_date_from_unix_secs(0)?, "1970-01-01");
        assert_eq!(utc_date_from_unix_secs(951_782_400)?, "2000-02-29");
        assert_eq!(utc_date_from_unix_secs(1_767_225_600)?, "2026-01-01");
        assert_eq!(utc_date_from_unix_secs(1_795_564_800)?, "2026-11-25");
        assert!(utc_date_from_unix_secs(u64::MAX).is_ok());
        Ok(())
    }

    #[test]
    fn pe_header_validation_accepts_valid_and_rejects_corrupt() -> Result {
        verify_pe_bytes(&make_pe(0x8664, 0x020B, 2), 0x8664, 2, "pdfcraft.exe")?;
        verify_pe_bytes(&make_pe(0x014C, 0x010B, 3), 0x014C, 3, "pdfcraft-cli.exe")?;
        verify_pe_bytes(&make_pe(0xAA64, 0x020B, 2), 0xAA64, 2, "pdfcraft.exe")?;
        let mut bad_mz = make_pe(0x8664, 0x020B, 2);
        bad_mz[0] = b'X';
        assert!(verify_pe_bytes(&bad_mz, 0x8664, 2, "bad_mz").is_err());
        let mut bad_lfa = make_pe(0x8664, 0x020B, 2);
        bad_lfa[0x3C..0x40].copy_from_slice(&0xFFFFu32.to_le_bytes());
        assert!(verify_pe_bytes(&bad_lfa, 0x8664, 2, "bad_lfa").is_err());
        let mut bad_sig = make_pe(0x8664, 0x020B, 2);
        bad_sig[0x80] = b'X';
        assert!(verify_pe_bytes(&bad_sig, 0x8664, 2, "bad_sig").is_err());
        assert!(verify_pe_bytes(&make_pe(0x014C, 0x010B, 2), 0x8664, 2, "wrong_mach").is_err());
        assert!(verify_pe_bytes(&make_pe(0x8664, 0x0107, 2), 0x8664, 2, "bad_magic").is_err());
        assert!(verify_pe_bytes(&make_pe(0x8664, 0x020B, 3), 0x8664, 2, "wrong_sub").is_err());
        assert!(verify_pe_bytes(b"MZ", 0x8664, 2, "short").is_err());
        Ok(())
    }

    #[test]
    fn strip_verbatim_and_resolve_dir_paths() {
        assert_eq!(strip_windows_verbatim(PathBuf::from(r"\\?\C:\repo\pkg")), PathBuf::from(r"C:\repo\pkg"));
        assert_eq!(strip_windows_verbatim(PathBuf::from(r"\\?\UNC\server\share\pkg")), PathBuf::from(r"\\server\share\pkg"));
        let long_path = PathBuf::from(format!(r"\\?\C:\{}", "a".repeat(260)));
        assert_eq!(strip_windows_verbatim(long_path.clone()), long_path);
        assert_eq!(resolve_dir_path(PathBuf::from("dist/out"), Path::new("repo")), PathBuf::from("repo").join("dist/out"));
        #[cfg(windows)]
        assert_eq!(resolve_dir_path(PathBuf::from(r"\\?\D:\abs\dir"), Path::new("repo")), PathBuf::from(r"D:\abs\dir"));
        #[cfg(unix)]
        assert_eq!(resolve_dir_path(PathBuf::from("/abs/dir"), Path::new("repo")), PathBuf::from("/abs/dir"));
    }

    #[test]
    fn parse_args_validates_flags_and_rejects_dangling_values() -> Result {
        let def = parse_args(&["package".into()])?;
        assert_eq!(def.arch, "x64");
        assert!(!def.skip_build);
        assert_eq!(def.shell, "pwsh");
        let full = parse_args(&[
            "package".into(),
            "--arch".into(),
            "arm64".into(),
            "--skip-build".into(),
            "--built-without-craft-fonts".into(),
            "--shell".into(),
            "powershell".into(),
            "--script-dir".into(),
            "scripts".into(),
            "--win-checks".into(),
            "wc.exe".into(),
        ])?;
        assert_eq!(full.arch, "arm64");
        assert!(full.skip_build && full.built_without_craft_fonts);
        assert_eq!(full.script_dir, Some(PathBuf::from("scripts")));
        assert_eq!(full.win_checks, Some(PathBuf::from("wc.exe")));
        assert!(parse_args(&["package".into(), "--unknown".into()]).is_err());
        assert!(parse_args(&["package".into(), "--arch".into()]).is_err());
        Ok(())
    }

    #[test]
    fn fs_helpers_reset_and_copy_checked() -> Result {
        let tmp = TempDir::new("pkg-fs")?;
        let target = tmp.0.join("nested").join("stage");
        reset_real_dir(&target)?;
        fs::write(target.join("old.txt"), b"stale")?;
        reset_real_dir(&target)?;
        assert!(!target.join("old.txt").exists());
        let src = tmp.0.join("src.txt");
        fs::write(&src, b"payload")?;
        let dst = target.join("dst.txt");
        copy_file_checked(&src, &dst)?;
        assert_eq!(fs::read(&dst)?, b"payload");
        assert!(copy_file_checked(&tmp.0.join("missing.txt"), &dst).is_err());
        assert!(ensure_real_dir_path(&dst).is_err());
        assert!(ensure_real_dir_path(&tmp.0.join("..").join("escape")).is_err());
        Ok(())
    }

    #[test]
    fn remove_part_and_clear_zip_handle_files_and_dirs() -> Result {
        let tmp = TempDir::new("pkg-part")?;
        let part_file = tmp.0.join("model.part");
        let part_dir = tmp.0.join("dir.part");
        fs::write(&part_file, b"p")?;
        fs::create_dir(&part_dir)?;
        fs::write(part_dir.join("inner"), b"i")?;
        remove_part_entry(&part_file)?;
        remove_part_entry(&part_dir)?;
        assert!(!part_file.exists() && !part_dir.exists());
        let zip = tmp.0.join("out.zip");
        fs::write(&zip, b"old")?;
        clear_existing_zip(&zip)?;
        clear_existing_zip(&zip)?;
        assert!(!zip.exists());
        assert!(clear_existing_zip(&tmp.0).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unix_symlink_guards_block_traversal_and_copy() -> Result {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new("pkg-sym")?;
        let real_dir = tmp.0.join("real");
        fs::create_dir(&real_dir)?;
        let link_dir = tmp.0.join("link_dir");
        symlink(&real_dir, &link_dir)?;
        assert!(reset_real_dir(&link_dir).is_err());
        assert!(reset_real_dir(&link_dir.join("child")).is_err());
        let file = tmp.0.join("file.txt");
        fs::write(&file, b"x")?;
        let link_file = tmp.0.join("link.txt");
        symlink(&file, &link_file)?;
        assert!(copy_file_checked(&link_file, &tmp.0.join("out.txt")).is_err());
        assert!(copy_file_checked(&file, &link_file).is_err());
        assert!(clear_existing_zip(&link_file).is_err());
        Ok(())
    }

    #[test]
    fn copy_notice_line_validates_and_copies() -> Result {
        let tmp = TempDir::new("pkg-notice")?;
        let (font_dir, docs) = (tmp.0.join("fonts").join("Inter"), tmp.0.join("docs"));
        fs::create_dir_all(&font_dir)?;
        fs::create_dir(&docs)?;
        let ofl = font_dir.join("OFL.txt");
        fs::write(&ofl, b"Inter OFL text\n")?;
        copy_notice_line(&ofl.display().to_string(), &docs)?;
        assert_eq!(fs::read(docs.join("OFL-Inter.txt"))?, b"Inter OFL text\n");
        assert!(copy_notice_line("OFL.txt", &docs).is_err());
        Ok(())
    }

    #[test]
    fn stage_portable_dir_preserves_portable_txt_and_notices() -> Result {
        let tmp = TempDir::new("pkg-portable")?;
        let (script_dir, root, stage, models, docs) =
            (tmp.0.join("win"), tmp.0.join("root"), tmp.0.join("stage"), tmp.0.join("models"), tmp.0.join("docs"));
        for d in [&script_dir, &root, &stage, &models, &docs] {
            fs::create_dir(d)?;
        }
        fs::write(docs.join("OFL-Inter.txt"), b"Inter OFL\n")?;
        fs::write(docs.join("ignored.txt"), b"skip")?;
        for f in ["pdfcraft.exe", "pdfcraft-cli.exe", "NOTICE", "LICENSE-document.txt"] {
            fs::write(stage.join(f), f.as_bytes())?;
        }
        fs::write(models.join("ATTRIBUTION.txt"), b"attrib")?;
        fs::write(root.join("README.md"), b"readme")?;
        let portable_body = b"PlutarchPages portable mode\n";
        fs::write(script_dir.join("portable.txt"), portable_body)?;
        let p_stage = tmp.0.join("portable-out");
        let sources = PortableSources { script_dir: &script_dir, root: &root, stage: &stage, models: &models, docs: &docs };
        stage_portable_dir(&sources, &p_stage, true)?;
        assert_eq!(fs::read(p_stage.join("portable.txt"))?, portable_body);
        assert_eq!(fs::read(p_stage.join("OFL-Inter.txt"))?, b"Inter OFL\n");
        assert!(!p_stage.join("ignored.txt").exists());
        assert!(p_stage.join("models").join("ATTRIBUTION.txt").is_file());
        copy_portable_notices(&docs, &tmp.0)?;
        assert!(tmp.0.join("OFL-Inter.txt").is_file());
        Ok(())
    }
}
