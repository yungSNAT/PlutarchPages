// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

#[test]
fn integrity_signals_distinguish_each_split_view_family() {
    let plain = "BT /F1 20 Tf 1 0 0 1 20 150 Tm (Approved) Tj ET";
    let near_white = "0.98 g BT /F1 20 Tf 1 0 0 1 20 150 Tm (Approved) Tj ET";
    let pages = marked_pages(&[
        plain,
        near_white,
        "0 g 0 0 300 300 re f 1 g BT /F1 20 Tf 1 0 0 1 20 150 Tm (Approved) Tj ET",
        "/Span <</ActualText (Refused)>> BDC BT /F1 20 Tf 1 0 0 1 20 150 Tm (Approved) Tj ET EMC",
        "/Span <</ActualText (Approved)>> BDC BT /F1 20 Tf 1 0 0 1 20 150 Tm (Approved) Tj ET EMC",
        "BT /F1 20 Tf 1 0 0 1 60 150 Tm (right) Tj 1 0 0 1 20 150 Tm (left) Tj ET",
        "BT /F1 20 Tf 1 0 0 1 20 150 Tm (left) Tj 1 0 0 1 60 150 Tm (right) Tj ET",
    ]);
    let settings = InterpreterSettings::default();
    for (page, expected) in
        [None, Some(HiddenReason::NearBackground), None, Some(HiddenReason::ActualTextDiffers), None, Some(HiddenReason::SourceOrderDiffers), None]
            .into_iter()
            .enumerate()
    {
        let (text, reasons) = extract_page_hidden(&pages, page, &settings).unwrap();
        if page >= 5 {
            assert_eq!(text.plain_text(), "left right");
            assert!(text.line_of.iter().all(|&line| line == text.line_of[0]), "the words must form a single line");
        }
        assert_eq!(reasons.iter().copied().flatten().next(), expected, "page {page}: {reasons:?}");
        assert_eq!(text.plain_text(), extract_page(&pages, page, &settings).unwrap().plain_text());
    }

    for (mapping, expected) in [("<41> <005A>", Some(HiddenReason::FontMappingDiffers)), ("<41> <0041>", None)] {
        let pdf = ligature_pages(&format!("1 beginbfchar {mapping} endbfchar"), "A", &["1 0 0 1 20 150"]);
        let (text, reasons) = extract_page_hidden(&pdf, 0, &settings).unwrap();
        assert_eq!(reasons, vec![expected]);
        assert_eq!(text.plain_text(), if expected.is_some() { "Z" } else { "A" });
    }
}

#[test]
fn integrity_signals_skip_translucent_dark_ink_and_separate_lines() {
    let layered = "BT /F1 20 Tf 1 0 0 1 20 150 Tm (Approved) Tj ET ";
    let layered = format!("/Faint gs {}", layered.repeat(20));
    let separate_lines = "BT /F1 20 Tf 1 0 0 1 150 100 Tm (second) Tj 1 0 0 1 20 150 Tm (first) Tj ET";
    let pdf = marked_pages(&[&layered, separate_lines]);
    let settings = InterpreterSettings::default();
    for page in 0..2 {
        let (text, reasons) = extract_page_hidden(&pdf, page, &settings).unwrap();
        assert!(!text.glyphs.is_empty(), "page {page}");
        assert!(reasons.iter().all(Option::is_none), "page {page}: {reasons:?}");
    }
}
