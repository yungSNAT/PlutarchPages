// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

#[test]
fn malformed_metadata_beside_covered_text_still_fails_the_cli_audit() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-audit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("malformed-leak.pdf");
    for (catalog, trailer, kind) in MALFORMED {
        std::fs::write(&path, fixture("BT /F1 24 Tf 100 700 Td (TOP SECRET) Tj ET\n0 g\n90 688 220 32 re f", catalog, trailer)).unwrap();

        let out = Command::new(BIN).arg("audit").arg(&path).output().unwrap();
        assert!(!out.status.success(), "{kind}: covered text must fail the audit");
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["count"], 2, "{report}");
        assert!(report["findings"].as_array().unwrap().iter().all(|f| f["kind"] == "covered-text"), "{report}");
        assert_eq!(report["document_findings"], serde_json::json!([{ "kind": kind, "count": 1 }]), "{report}");
    }
    std::fs::remove_file(path).unwrap();
}

/// A page the audit can't read fails it, and the error names the page as a reader counts: here
/// the first page's text runs past the text reader's budget.
#[test]
fn an_unreadable_page_fails_the_cli_audit_and_is_numbered_from_1() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-audit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("unreadable.pdf");
    std::fs::write(&path, fixture("/PCSoft << /N 20000000 >> BDC EMC", "", "")).unwrap();

    let out = Command::new(BIN).arg("audit").arg(&path).output().unwrap();
    assert!(!out.status.success(), "an audit error must fail the audit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unreadable.pdf: page 1: "), "{stderr}");
    std::fs::remove_file(path).unwrap();
}

/// Text in a stream nothing refers to is still in the file, so it fails the audit as covered text
/// does: a redaction mutool applied and saved without garbage collection leaves the page's old
/// content streams so. Saved with garbage collection, the file audits clean.
#[test]
fn unreferenced_text_fails_the_cli_audit() {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/audit/tests/data/tool-made");
    let out = Command::new(BIN).arg("audit").arg(format!("{data}/mutool-applied-no-garbage.pdf")).output().unwrap();
    assert!(!out.status.success(), "text left in an unreferenced stream must fail the audit");
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["count"], 0, "{report}");
    assert_eq!(report["document_findings"], serde_json::json!([{ "kind": "unreferenced-text", "count": 2 }]), "{report}");

    let out = Command::new(BIN).arg("audit").arg(format!("{data}/mutool-applied.pdf")).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}
