// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// Two pages' annotations share one appearance stream and a mark covers only the first: the second
/// would keep drawing the redacted text, so apply refuses and names its page.
#[test]
fn an_appearance_a_kept_annotation_shares_is_refused() {
    let annot = |rect: &str| format!("<< /Type /Annot /Subtype /Stamp /Rect [{rect}] /AP << /N 9 0 R >> >>").into_bytes();
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [7 0 R] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [8 0 R] >>".to_vec(),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        annot("10 10 110 60"),
        annot("10 10 110 60"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 100 50] /Resources << /Font << /F1 5 0 R >> >>", b"BT /F1 10 Tf 5 5 Td (STAMPSECRET) Tj ET"),
    ]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 70.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::StillUsed(pages)) if pages == vec![1]));
}

/// The same appearance on the marked page only leaves the file with its annotation.
#[test]
fn an_appearance_only_a_removed_annotation_held_leaves_the_file() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [6 0 R] >>".to_vec(),
        FONT.replace("95 0 R", "5 0 R").into_bytes(),
        widths(),
        b"<< /Type /Annot /Subtype /Stamp /Rect [10 10 110 60] /AP << /N 7 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 100 50] /Resources << /Font << /F1 4 0 R >> >>", b"BT /F1 10 Tf 5 5 Td (STAMPSECRET) Tj ET"),
    ]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 70.0]], "");
    apply(&mut doc, None).unwrap();
    assert!(!full_save_holds(&doc, b"STAMPSECRET"), "the appearance is still reachable");
}

/// The same form drawing the image itself: the page still shows it there, so the fork refuses.
#[test]
fn an_image_a_surviving_form_draws_is_refused() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 7 0 R >>".to_vec(),
        stream("", b"q 100 0 0 100 0 0 cm /Im Do Q /Fm Do"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Font << /F1 5 0 R >> /XObject << /Im 8 0 R /Fm 9 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 11 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"IMAGESECRET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources 7 0 R", b"q 50 0 0 50 200 200 cm /Im Do Q"),
    ]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 120.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::StillUsed(p)) if p == [0]));
}

/// The surviving form draws the image through a nested form without /Resources of its own,
/// which draws through the outer form's: refused.
#[test]
fn an_image_a_surviving_forms_nested_form_draws_is_refused() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"q 100 0 0 100 0 0 cm /Im Do Q /Fm Do"),
        b"<< /XObject << /Im 6 0 R /Fm 7 0 R /Inner 8 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 11 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"IMAGESECRET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources 5 0 R", b"/Inner Do"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"q 50 0 0 50 200 200 cm /Im Do Q"),
    ]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 120.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::StillUsed(p)) if p == [0]));
}

/// Two pages share one resource dictionary, and the second draws the image only through a form
/// without /Resources: redacting the image on the first page is refused.
#[test]
fn an_image_another_page_draws_through_a_form_without_resources_is_refused() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 5 0 R /Resources 7 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 6 0 R /Resources 7 0 R >>".to_vec(),
        stream("", b"q 100 0 0 100 0 0 cm /Im Do Q"),
        stream("", b"/Inner Do"),
        b"<< /XObject << /Im 8 0 R /Inner 9 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 11 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"IMAGESECRET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"q 50 0 0 50 200 200 cm /Im Do Q"),
    ]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 120.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::StillUsed(p)) if p == [1]));
}
