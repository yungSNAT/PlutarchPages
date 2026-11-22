// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// `/Direction /R2L` reaches the view from the file, and holds with the default view too: it says
/// how the book is bound, not how to show it.
#[test]
fn a_document_bound_on_the_right_opens_bound_on_the_right() {
    let right_bound = String::from_utf8_lossy(PAGES)
        .replace("/Pages 2 0 R >>", "/Pages 2 0 R /PageLayout /TwoPageLeft /ViewerPreferences << /Direction /R2L >> >>");
    let open = |file: &[u8], always: &str| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.set_option("always-default-view", always).unwrap();
        app.open_bytes("book.pdf", None, file.to_vec()).expect("opens");
        app.views[0].right_to_left
    };
    assert!(open(right_bound.as_bytes(), "off"));
    assert!(open(right_bound.as_bytes(), "on"));
    assert!(!open(PAGES, "off"));
}
