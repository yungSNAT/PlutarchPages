// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Unit and integration tests for `unix_package.rs`.

use crate::unix_package_cert::{
    SecretBytes, SecretString, SigningGuard, TEST_SCRUB_CALLS, decode_base64, emit_outputs_to, import_with_cert_pass, parse_dev_id_identity,
};
use crate::unix_package_env::{EnvCtx, render_template, sha256_file, sha256_hex, utc_date_from_unix_secs, validate_build_label, workspace_version};
use crate::unix_package_flatpak::{normalize_flatpak_arch, parse_flatpak_args};
use crate::unix_package_freebsd::{is_freebsd_elf_description, resolve_target_and_arch};
use crate::unix_package_fs::reset_real_dir;
use crate::unix_package_linux::{has_format, parse_linux_args, resolve_linux_arch};
use crate::unix_package_macos::{extract_json_string_field, parse_macos_args, targets_for_arch};
use crate::unix_package_web::has_root_absolute_url;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_SEQ: AtomicU64 = AtomicU64::new(0);

fn scratch_dir(name: &str) -> PathBuf {
    let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
    let base = env::temp_dir(); // nosemgrep: rust.lang.security.temp-dir.temp-dir
    let dir = base.join(format!("plutarch_unix_pkg_test_{}_{}_{seq}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).ok();
    dir
}

#[test]
fn sha256_matches_nist_and_boundary_vectors() {
    assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(
        sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
    assert_eq!(sha256_hex(&[b'a'; 55]), "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318");
    assert_eq!(sha256_hex(&[b'a'; 56]), "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a");
    assert_eq!(sha256_hex(&[b'a'; 64]), "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb");
    let dir = scratch_dir("sha_stream");
    let path = dir.join("stream.bin");
    let payload = vec![0x5au8; 70_000];
    fs::write(&path, &payload).ok();
    assert_eq!(sha256_file(&path).ok(), Some(sha256_hex(&payload)));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn base64_decodes_and_rejects_malformed() {
    use std::sync::atomic::Ordering;
    assert_eq!(decode_base64("TWFu").ok().as_ref().map(SecretBytes::as_slice), Some(b"Man".as_slice()));
    assert_eq!(decode_base64("T WE=\n").ok().as_ref().map(SecretBytes::as_slice), Some(b"Ma".as_slice()));
    assert_eq!(decode_base64("TQ==").ok().as_ref().map(SecretBytes::as_slice), Some(b"M".as_slice()));
    let before = TEST_SCRUB_CALLS.load(Ordering::Relaxed);
    assert!(decode_base64("abc").is_err());
    assert!(decode_base64("TW=a").is_err());
    assert!(decode_base64("T!Fu").is_err());
    let after = TEST_SCRUB_CALLS.load(Ordering::Relaxed);
    assert!(after >= before + 5, "expected intermediate decode buffers scrubbed on error ({before} -> {after})");
}

#[test]
fn parses_workspace_version_and_build_label() {
    let toml = "[package]\nversion = \"9.9.9\"\n[workspace.package]\nversion = \"0.18.1-rc.2\" # note\n";
    assert_eq!(workspace_version(toml).as_deref(), Some("0.18.1-rc.2"));
    assert_eq!(validate_build_label(Some("local.1".into())).ok().flatten().as_deref(), Some("local.1"));
    assert!(validate_build_label(Some(".bad".into())).is_err());
    assert!(validate_build_label(Some("bad.".into())).is_err());
    assert!(validate_build_label(Some("bad-label".into())).is_err());
    assert_eq!(utc_date_from_unix_secs(1_764_028_800).ok().as_deref(), Some("2025-11-25"));
}

#[test]
fn detects_root_absolute_web_urls() {
    assert!(has_root_absolute_url(r#"<script src="/pkg/app.js"></script>"#));
    assert!(has_root_absolute_url(r#"<link href="/styles.css">"#));
    assert!(!has_root_absolute_url(r#"<script src="./pkg/app.js"></script>"#));
    assert!(!has_root_absolute_url(r#"<link href="//cdn.example.invalid/a.css">"#));
}

#[test]
fn parses_notarytool_json_and_find_identity() {
    let raw = r#"{"message":"\"status\":\"Rejected\"","status":"Accepted","id":"1234-abcd-\"escaped\""}"#;
    assert_eq!(extract_json_string_field(raw, "status").as_deref(), Some("Accepted"));
    assert_eq!(extract_json_string_field(raw, "id").as_deref(), Some("1234-abcd-\"escaped\""));

    let ids = "  1) ABC123DEF456 \"Developer ID Application: Example Corp (TEAM123)\"\n     1 valid identities found\n";
    assert_eq!(parse_dev_id_identity(ids).as_deref(), Some("ABC123DEF456"));
    assert!(parse_dev_id_identity("0 valid identities found\n").is_none());
}

#[test]
fn validates_arch_specs_and_cli_flags() {
    assert_eq!(resolve_linux_arch("arm64").ok().map(|s| (s.arch, s.deb_arch)), Some(("aarch64", "arm64")));
    assert_eq!(resolve_linux_arch("riscv64").ok().map(|s| s.deb_arch), Some("riscv64"));
    assert!(resolve_linux_arch("mips").is_err());
    assert!(has_format("deb rpm tar", "deb"));
    assert!(!has_format("deb-other", "deb"));

    assert!(parse_linux_args(&["--formats".into()]).is_err());
    assert!(parse_linux_args(&["--unknown".into()]).is_err());
    assert!(parse_macos_args(&["--arch".into(), "arm64".into()]).is_err());
    assert_eq!(targets_for_arch("universal").ok().map(|t| t.len()), Some(2));
    assert!(parse_flatpak_args(&["--oci".into(), "dist/app.tar.gz".into()]).is_ok());
    assert_eq!(normalize_flatpak_arch("arm64").ok(), Some("aarch64"));

    assert_eq!(
        resolve_target_and_arch(Some("host-tuple"), "x86_64", Some("aarch64-unknown-freebsd")).ok(),
        Some((Some("aarch64-unknown-freebsd".into()), "aarch64"))
    );
    assert!(resolve_target_and_arch(Some("x86_64-unknown-linux-gnu"), "x86_64", None).is_err());
    assert!(is_freebsd_elf_description("ELF 64-bit LSB executable, x86-64, version 1 (FreeBSD), dynamically linked", "x86_64"));
    assert!(is_freebsd_elf_description("ELF 64-bit LSB shared object, ARM aarch64, FreeBSD, interpreter /libexec/ld-elf.so.1", "aarch64"));
    assert!(!is_freebsd_elf_description("ELF 64-bit LSB relocatable, x86-64, FreeBSD", "x86_64"));
}

#[test]
fn templates_and_docs_staging_reject_traversal() {
    let dir = scratch_dir("docs");
    let root = dir.join("repo");
    fs::create_dir_all(root.join("licenses")).ok();
    fs::write(root.join("Cargo.toml"), "[workspace.package]\nversion = \"1.2.3\"\n").ok();
    fs::write(root.join("LICENSE-MIT"), "MIT").ok();
    fs::write(root.join("licenses/extra.txt"), "EXTRA").ok();
    let tpl = root.join("tpl.in");
    let tpl_out = dir.join("tpl.out");
    fs::write(&tpl, "v=@VERSION@ d=@DATE@").ok();
    assert!(render_template(&tpl, &tpl_out, &[("@VERSION@", "1.2.3"), ("@DATE@", "2026-11-25")]).is_ok());
    assert_eq!(fs::read_to_string(&tpl_out).ok().as_deref(), Some("v=1.2.3 d=2026-11-25"));

    let ctx = EnvCtx {
        root: root.clone(),
        dist: dir.join("dist"),
        target_dir: dir.join("target"),
        version: "1.2.3".into(),
        build_label: None,
        build_sha: "deadbeef".into(),
        build_date: "2026-11-25".into(),
        github_actions: false,
    };
    let dest = dir.join("staged");
    assert!(ctx.copy_docs(&dest).is_ok());
    assert_eq!(fs::read_to_string(dest.join("LICENSE-MIT")).ok().as_deref(), Some("MIT"));
    assert_eq!(fs::read_to_string(dest.join("licenses/extra.txt")).ok().as_deref(), Some("EXTRA"));
    assert!(reset_real_dir(&dir.join("../escape")).is_err());
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn unix_permissions_and_symlink_guards_hold() {
    use crate::unix_package_fs::{copy_dir_no_symlinks, copy_file_checked, create_private_temp_dir, ensure_real_dir_path, write_private_file};
    use std::os::unix::fs::{PermissionsExt, symlink};

    let dir = scratch_dir("unix_guards");
    let priv_dir = create_private_temp_dir(&dir, "p.").ok();
    assert!(priv_dir.is_some());
    if let Some(ref pd) = priv_dir {
        let mode = fs::metadata(pd).map(|m| m.permissions().mode() & 0o777).ok();
        assert_eq!(mode, Some(0o700));
        let priv_file = pd.join("secret.p12");
        assert!(write_private_file(&priv_file, b"secret").is_ok());
        let fmode = fs::metadata(&priv_file).map(|m| m.permissions().mode() & 0o777).ok();
        assert_eq!(fmode, Some(0o600));
    }

    let real_src = dir.join("real.txt");
    let link_src = dir.join("link.txt");
    fs::write(&real_src, "ok").ok();
    let _ = symlink(&real_src, &link_src);
    assert!(copy_file_checked(&link_src, &dir.join("out.txt")).is_err());

    let src_dir = dir.join("tree");
    fs::create_dir_all(&src_dir).ok();
    let _ = symlink(&real_src, src_dir.join("nested_link.txt"));
    assert!(copy_dir_no_symlinks(&src_dir, &dir.join("tree_copy")).is_err());

    let real_parent = dir.join("real_parent");
    let link_parent = dir.join("link_parent");
    fs::create_dir_all(&real_parent).ok();
    let _ = symlink(&real_parent, &link_parent);
    assert!(ensure_real_dir_path(&link_parent.join("nested/sub")).is_ok());
    assert!(ensure_real_dir_path(&link_parent).is_err());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn signing_guard_cleans_up_on_emit_failure() {
    use std::sync::atomic::Ordering;
    let dir = scratch_dir("sign_guard");
    let work = dir.join("work");
    fs::create_dir_all(&work).ok();
    let cert_path = work.join("cert.p12");
    let kc_path = work.join("kc.keychain-db");
    fs::write(&cert_path, b"secret-p12-data").ok();
    let bad_gh_env = work.to_string_lossy().into_owned();
    assert!(emit_outputs_to("ID123", &kc_path, Some(&bad_gh_env)).is_err());
    let before_scrub = TEST_SCRUB_CALLS.load(Ordering::Relaxed);
    {
        let _guard = SigningGuard {
            temp_dir: work.clone(),
            cert_path: cert_path.clone(),
            keychain_path: kc_path.clone(),
            secret_pass: SecretString::new("supersecret".into()),
            keep_keychain_on_success: false,
        };
    }
    let after_guard = TEST_SCRUB_CALLS.load(Ordering::Relaxed);
    assert!(after_guard > before_scrub, "expected SigningGuard to scrub secret_pass on drop");
    assert!(import_with_cert_pass(&cert_path, &kc_path, "kc-pass", "cert-pass".into()).is_err());
    let after_import_err = TEST_SCRUB_CALLS.load(Ordering::Relaxed);
    assert!(after_import_err > after_guard, "expected import_with_cert_pass to scrub cert_pass on early error");
    assert!(!cert_path.exists());
    assert!(!work.exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_dispatches_help_cleanly() {
    let dir = scratch_dir("dispatch");
    fs::create_dir_all(&dir).ok();
    fs::write(dir.join("Cargo.toml"), "[workspace.package]\nversion = \"0.1.0\"\n").ok();
    let root = Some(dir.clone());
    let env = EnvCtx::load(root).ok();
    assert!(env.is_some());
    if let Some(ref e) = env {
        assert!(crate::unix_package_linux::run(e, &["--help".into()]).is_ok());
        assert!(crate::unix_package_freebsd::run(e, &["--help".into()]).is_ok());
        assert!(crate::unix_package_macos::run(e, &["--help".into()]).is_ok());
        assert!(crate::unix_package_flatpak::run(e, &["--help".into()]).is_ok());
        assert!(crate::unix_package_web::run(e, &["--help".into()]).is_ok());
        assert!(crate::unix_package_cert::run(e, &["--help".into()]).is_ok());
    }
    let _ = fs::remove_dir_all(&dir);
}
