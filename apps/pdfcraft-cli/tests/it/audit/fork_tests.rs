// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// Text whited out under an opaque white box is still in the file: `hidden-under-white` fails
/// the audit as covered text does.
#[test]
fn text_hidden_under_white_fails_the_cli_audit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("white.pdf");
    std::fs::write(&path, fixture("BT /F1 24 Tf 100 700 Td (TOP SECRET) Tj ET\n1 g\n90 688 220 32 re f", "", "")).unwrap();

    let out = Command::new(BIN).arg("audit").arg(&path).output().unwrap();
    assert!(!out.status.success(), "text under white must fail the audit");
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["count"], 2, "{report}");
    assert!(report["findings"].as_array().unwrap().iter().all(|f| f["kind"] == "hidden-under-white"), "{report}");
}

/// Text under a black box on both pages: 2 covered-text findings.
const COVERED: &str = "BT /F1 24 Tf 100 700 Td (TOP SECRET) Tj ET\n0 g\n90 688 220 32 re f";
/// The same pages with the text gone and the box kept.
const REDACTED: &str = "0 g\n90 688 220 32 re f";

/// `--before` counts the original's leaks too: the JSON line carries both counts, stderr says
/// `leaks: N before -> M after`, and the exit status still follows the file audited.
#[test]
fn audit_before_reports_the_leaks_the_redaction_took_out() {
    let dir = tempfile::tempdir().unwrap();
    let (original, redacted) = (dir.path().join("original.pdf"), dir.path().join("redacted.pdf"));
    std::fs::write(&original, fixture(COVERED, "", "")).unwrap();
    std::fs::write(&redacted, fixture(REDACTED, "", "")).unwrap();

    let out = Command::new(BIN).arg("audit").arg(&redacted).arg("--before").arg(&original).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!((report["leaks"].clone(), report["before"]["leaks"].clone()), (0.into(), 2.into()), "{report}");
    assert_eq!(report["before"]["file"], original.to_str().unwrap(), "{report}");
    assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), "leaks: 2 before -> 0 after");

    // Audited the other way round, the file still leaks: the audit fails and says so.
    let out = Command::new(BIN).arg("audit").arg(&original).arg("--before").arg(&redacted).output().unwrap();
    assert!(!out.status.success());
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!((report["leaks"].clone(), report["before"]["leaks"].clone()), (2.into(), 0.into()), "{report}");
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("leaks: 0 before -> 2 after\n"), "{}", String::from_utf8_lossy(&out.stderr));

    // Without --before the line is as it was.
    let out = Command::new(BIN).arg("audit").arg(&redacted).output().unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(report.get("leaks").is_none() && report.get("before").is_none(), "{report}");
    assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
}

/// An original that can't be read is an error, not a count of 0.
#[test]
fn audit_before_fails_on_an_unreadable_original() {
    let dir = tempfile::tempdir().unwrap();
    let redacted = dir.path().join("redacted.pdf");
    std::fs::write(&redacted, fixture(REDACTED, "", "")).unwrap();
    let missing = dir.path().join("missing.pdf");
    let out = Command::new(BIN).arg("audit").arg(&redacted).arg("--before").arg(&missing).output().unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing.pdf"), "{}", String::from_utf8_lossy(&out.stderr));
}
