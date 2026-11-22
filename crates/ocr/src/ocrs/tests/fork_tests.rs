// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

#[test]
fn release_model_discovery_excludes_the_source_checkout() {
    let source_models = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models");
    let dirs = Models::search_dirs();
    // An explicit override is still allowed in either profile.
    let has_source = dirs.iter().skip(usize::from(std::env::var_os("PDFCRAFT_MODELS").is_some())).any(|dir| dir == &source_models);
    assert_eq!(has_source, cfg!(debug_assertions), "{dirs:?}");
}
