// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// #568 with R2–R4 encryption: the user password can't extract from a copy-restricted file, and
/// the owner password can (the renderer gets the user password owner authentication recovers).
#[test]
fn owner_password_extracts_text_from_copy_restricted_rc4_and_aes128_files() {
    use std::sync::Arc;
    for (i, algorithm) in [pdfcraft_cos::Algorithm::Rc4_128, pdfcraft_cos::Algorithm::Aes128].into_iter().enumerate() {
        let mut doc = pdfcraft_cos::Document::open(Arc::new(fixture(1))).unwrap();
        doc.set_encryption_legacy(&pdfcraft_cos::NewEncryption {
            algorithm,
            user_password: "user",
            owner_password: "owner",
            permissions: 0b0100,
            encrypt_metadata: true,
            seed: [9; 32],
        })
        .unwrap();
        let path = tmp(&format!("restricted-{i}.pdf"));
        std::fs::write(&path, pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()).unwrap();
        let p = path.to_str().unwrap();
        let user = Command::new(BIN).args(["text", p, "--password", "user"]).output().unwrap();
        assert!(!user.status.success(), "{algorithm:?}: the user password must not extract text");
        let owner = Command::new(BIN).args(["text", p, "--password", "owner"]).output().unwrap();
        assert!(owner.status.success(), "{algorithm:?}: {}", String::from_utf8_lossy(&owner.stderr));
        assert!(String::from_utf8_lossy(&owner.stdout).contains("Page 1"), "{algorithm:?}");
    }
}
