// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit tests for `packaging/windows/package.rs`.

#[cfg(test)]
mod unit_tests {
    use super::super::package_fs::{copy_file_checked, ensure_real_dir_path, reset_real_dir};
    use super::super::package_meta::{arch_spec, resolve_dir_path, resolve_version, strip_windows_verbatim, utc_date_from_unix_secs, validate_flags};
    use super::super::package_pe::verify_pe_bytes;
    use super::super::package_sign::{
        Source, azure_args, azure_metadata, certificate_args, decode_base64, find_x64_dlib, hex, json_escape, newest_x64_signtool, select_source,
        sha256,
    };
    use super::super::{
        PortableSources, Result, clear_existing_zip, copy_notice_line, copy_portable_notices, parse_args, remove_part_entry, stage_portable_dir,
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
        buf[0x94..0x96].copy_from_slice(&0x00F0u16.to_le_bytes());
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
        assert!(resolve_version("[workspace.package]\nversion = 0.22.0\n", None).is_err());
        for bad in ["-invalid", "..", "1..2", "0.1.0-../evil", r"0.1.0-..\evil", "1.0."] {
            assert!(resolve_version(toml, Some(bad)).is_err(), "expected rejection for `{bad}`");
        }
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
    fn pe_header_rejects_dos_overlap_and_short_opt_header() {
        let mut dos_overlap = make_pe(0x8664, 0x020B, 2);
        dos_overlap[0x3C..0x40].copy_from_slice(&0x20u32.to_le_bytes());
        dos_overlap[0x20..0x24].copy_from_slice(b"PE\0\0");
        assert!(verify_pe_bytes(&dos_overlap, 0x8664, 2, "overlap").is_err());
        let mut short_opt = make_pe(0x8664, 0x020B, 2);
        short_opt[0x94..0x96].copy_from_slice(&0x0044u16.to_le_bytes());
        assert!(verify_pe_bytes(&short_opt, 0x8664, 2, "short_opt").is_err());
    }

    #[test]
    fn strip_verbatim_and_resolve_dir_paths() -> Result {
        assert_eq!(strip_windows_verbatim(PathBuf::from(r"\\?\C:\repo\pkg")), PathBuf::from(r"C:\repo\pkg"));
        assert_eq!(strip_windows_verbatim(PathBuf::from(r"\\?\UNC\server\share\pkg")), PathBuf::from(r"\\server\share\pkg"));
        let long_path = PathBuf::from(format!(r"\\?\C:\{}", "a".repeat(260)));
        assert_eq!(strip_windows_verbatim(long_path.clone()), long_path);
        assert_eq!(resolve_dir_path(PathBuf::from("dist/out"), Path::new("repo"))?, PathBuf::from("repo").join("dist/out"));
        assert_eq!(resolve_dir_path(PathBuf::from("../dist"), Path::new("work/repo"))?, PathBuf::from("work").join("dist"));
        assert!(resolve_dir_path(PathBuf::from("../../escape"), Path::new("repo")).is_err());
        #[cfg(windows)]
        assert_eq!(resolve_dir_path(PathBuf::from(r"\\?\D:\abs\dir"), Path::new("repo"))?, PathBuf::from(r"D:\abs\dir"));
        #[cfg(unix)]
        assert_eq!(resolve_dir_path(PathBuf::from("/abs/dir"), Path::new("repo"))?, PathBuf::from("/abs/dir"));
        Ok(())
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
            "--host-arch".into(),
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
        assert_eq!(full.host_arch, "arm64");
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
        assert!(reset_real_dir(&tmp.0.join("nested").join("..")).is_err());
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

    #[test]
    fn sha256_matches_the_fips_vectors_and_block_edges() {
        let cases: [(Vec<u8>, &str); 10] = [
            (b"".to_vec(), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            (b"abc".to_vec(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq".to_vec(),
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (vec![b'a'; 55], "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318"),
            (vec![b'a'; 56], "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a"),
            (vec![b'a'; 63], "7d3e74a05d7db15bce4ad9ec0658ea98e3f06eeecf16b4c6fff2da457ddc2f34"),
            (vec![b'a'; 64], "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb"),
            (vec![b'a'; 65], "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0"),
            (vec![b'a'; 120], "2f3d335432c70b580af0e8e1b3674a7c020d683aa5f73aaaedfdc55af904c21c"),
            (vec![b'a'; 1_000_000], "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"),
        ];
        for (input, want) in cases {
            assert_eq!(hex(&sha256(&input)), want, "length {}", input.len());
        }
    }

    #[test]
    fn base64_decodes_like_from_base64_string_and_refuses_bad_input() -> Result {
        assert_eq!(decode_base64("TWFu")?, b"Man");
        assert_eq!(decode_base64("TWE=")?, b"Ma");
        assert_eq!(decode_base64("TQ==")?, b"M");
        assert_eq!(decode_base64(" TW\r\nFu TQ ==\n")?, b"ManM");
        assert_eq!(decode_base64("+/+/")?, [0xfb, 0xff, 0xbf]);
        assert_eq!(decode_base64("")?, b"");
        for bad in ["TWF", "TQ=", "TQ===", "T===", "TQ==TQ==", "TW-u", "TW_u", "TWFu!"] {
            let err = decode_base64(bad).err().map(|e| e.to_string());
            assert_eq!(err.as_deref(), Some("WINDOWS_CERTIFICATE is not valid base64"), "{bad}");
        }
        Ok(())
    }

    #[test]
    fn the_certificate_wins_and_azure_needs_every_variable() {
        let env = |set: &'static [&'static str]| move |name: &str| set.contains(&name).then(|| "x".to_owned());
        let azure =
            ["AZURE_TENANT_ID", "AZURE_CLIENT_ID", "AZURE_CLIENT_SECRET", "AZURE_SIGNING_ENDPOINT", "AZURE_SIGNING_ACCOUNT", "AZURE_CERT_PROFILE"];
        assert_eq!(select_source(env(&[])), None);
        assert_eq!(select_source(env(&["WINDOWS_CERTIFICATE_PASSWORD"])), None);
        assert_eq!(select_source(env(&["WINDOWS_CERTIFICATE"])), Some(Source::Certificate));
        assert_eq!(
            select_source(env(&["AZURE_TENANT_ID", "AZURE_CLIENT_ID", "AZURE_CLIENT_SECRET", "AZURE_SIGNING_ENDPOINT", "AZURE_SIGNING_ACCOUNT"])),
            None
        );
        assert_eq!(
            select_source(env(&[
                "AZURE_TENANT_ID",
                "AZURE_CLIENT_ID",
                "AZURE_CLIENT_SECRET",
                "AZURE_SIGNING_ENDPOINT",
                "AZURE_SIGNING_ACCOUNT",
                "AZURE_CERT_PROFILE"
            ])),
            Some(Source::Azure)
        );
        assert_eq!(
            select_source(env(&[
                "WINDOWS_CERTIFICATE",
                "AZURE_TENANT_ID",
                "AZURE_CLIENT_ID",
                "AZURE_CLIENT_SECRET",
                "AZURE_SIGNING_ENDPOINT",
                "AZURE_SIGNING_ACCOUNT",
                "AZURE_CERT_PROFILE"
            ])),
            Some(Source::Certificate)
        );
        assert_eq!(azure.len(), 6);
    }

    #[test]
    fn signtool_arguments_match_the_former_script() {
        let common = ["sign", "/v", "/fd", "SHA256", "/td", "SHA256", "/d", "PlutarchPages", "/du", "https://github.com/yungSNAT/PlutarchPages"];
        let strings = |v: Vec<std::ffi::OsString>| v.into_iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>();
        let mut want: Vec<String> = common.iter().map(|s| (*s).to_owned()).collect();
        want.extend(["/tr", "http://ts", "/f", "c.pfx"].map(str::to_owned));
        assert_eq!(strings(certificate_args("http://ts", Path::new("c.pfx"), None)), want);
        want.extend(["/p", " pw "].map(str::to_owned));
        assert_eq!(strings(certificate_args("http://ts", Path::new("c.pfx"), Some(" pw "))), want);
        let mut want: Vec<String> = common.iter().map(|s| (*s).to_owned()).collect();
        want.extend(["/tr", "http://acs", "/dlib", "d.dll", "/dmdf", "m.json"].map(str::to_owned));
        assert_eq!(strings(azure_args("http://acs", Path::new("d.dll"), Path::new("m.json"))), want);
    }

    #[test]
    fn azure_metadata_is_escaped_json() {
        assert_eq!(
            azure_metadata("https://eus.codesigning.azure.net", "acc\"t", "p\\f\n"),
            "{\"Endpoint\": \"https://eus.codesigning.azure.net\", \"CodeSigningAccountName\": \"acc\\\"t\", \"CertificateProfileName\": \"p\\\\f\\u000a\"}\n"
        );
        assert_eq!(json_escape("plain é"), "plain é");
    }

    #[test]
    fn the_newest_x64_signtool_and_the_x64_dlib_are_found() -> Result {
        let tmp = TempDir::new("pkg-sign")?;
        for rel in ["10.0.1.0/x64", "10.0.2.0/x64", "10.0.3.0/x86", "10.0.3.0/arm64"] {
            fs::create_dir_all(tmp.0.join(rel))?;
            fs::write(tmp.0.join(rel).join("signtool.exe"), b"")?;
        }
        assert_eq!(newest_x64_signtool(&tmp.0), Some(tmp.0.join("10.0.2.0/x64").join("signtool.exe")));
        assert_eq!(newest_x64_signtool(&tmp.0.join("10.0.3.0")), None);
        assert_eq!(newest_x64_signtool(&tmp.0.join("missing")), None);
        let client = tmp.0.join("client");
        for rel in ["bin/x86", "bin/x64", "bin/arm64"] {
            fs::create_dir_all(client.join(rel))?;
            fs::write(client.join(rel).join("Azure.CodeSigning.Dlib.dll"), b"")?;
        }
        assert_eq!(find_x64_dlib(&client), Some(client.join("bin/x64").join("Azure.CodeSigning.Dlib.dll")));
        fs::remove_file(client.join("bin/x64").join("Azure.CodeSigning.Dlib.dll"))?;
        assert_eq!(find_x64_dlib(&client), None, "an x64 folder above the client does not count");
        Ok(())
    }
}
