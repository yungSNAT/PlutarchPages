// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// A Document Open password with an empty owner password (#603): `check-one` renders without
/// asking for a password, so it must report the file as locked rather than read it through the
/// empty owner password.
#[test]
fn check_one_does_not_open_a_file_through_an_empty_owner_password() {
    for algorithm in ALGORITHMS {
        let mut doc = pdfcraft_cos::Document::open(Arc::new(fixture(PAGE))).unwrap();
        doc.set_encryption_legacy(&NewEncryption {
            algorithm,
            user_password: "secret",
            owner_password: "",
            permissions: 0,
            encrypt_metadata: true,
            seed: [3; 32],
        })
        .unwrap();
        let path = tmp(&format!("check-one-{algorithm:?}.pdf"));
        std::fs::write(&path, pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()).unwrap();
        let out = Command::new(BIN).args(["check-one", path.to_str().unwrap()]).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("open-error"), "{algorithm:?}: {stdout}");
    }
}
