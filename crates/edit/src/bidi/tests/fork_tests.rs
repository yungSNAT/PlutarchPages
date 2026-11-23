// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

#[test]
fn a_variation_selector_stays_after_its_base_in_rtl_text() {
    assert_eq!(drawn("א❤\u{FE0F}ב"), "ב❤\u{FE0F}א");
    assert_eq!(to_logical("ב❤\u{FE0F}א"), "א❤\u{FE0F}ב");
}
