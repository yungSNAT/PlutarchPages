// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// Text whited out under an opaque white box is still in the file: `hidden-under-white` fails
/// the audit as covered text does.
#[test]
fn text_hidden_under_white_fails_the_cli_audit() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-audit-white-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("white.pdf");
    std::fs::write(&path, fixture("BT /F1 24 Tf 100 700 Td (TOP SECRET) Tj ET\n1 g\n90 688 220 32 re f", "", "")).unwrap();

    let out = Command::new(BIN).arg("audit").arg(&path).output().unwrap();
    assert!(!out.status.success(), "text under white must fail the audit");
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["count"], 2, "{report}");
    assert!(report["findings"].as_array().unwrap().iter().all(|f| f["kind"] == "hidden-under-white"), "{report}");
    std::fs::remove_file(path).unwrap();
}
