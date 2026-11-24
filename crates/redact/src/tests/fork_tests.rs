// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// Upstream keeps an image removed under a mark that another of the page's content streams, with
/// `/DecodeParms` given by reference, still draws. The fork takes an image wholly under a mark out
/// of every draw (`settle_images`), so that stream must be read through its parameters for its
/// draw to go too: read without them, it seemed to draw nothing and the image stayed in the file.
#[test]
fn an_image_drawn_again_in_content_with_parameters_by_reference_goes_from_both_draws() {
    use pdfcraft_cos::{Dict, ObjRef, Object};
    let img = stream("/Type /XObject /Subtype /Image /Width 8 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", *b"IMGBYTES");
    let mut doc = one_page(b"q 10 0 0 10 10 10 cm /Im1 Do Q", "/XObject << /Im1 7 0 R >>", vec![img]);
    let again = predicted(&mut doc, Dict::new(), b"q 10 0 0 10 200 200 cm /Im1 Do Q");
    let again = doc.add(Object::Stream(again));
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"Contents".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(4, 0)), Object::Ref(again)])))
        .unwrap();
    mark(&mut doc, 0, &[[5.0, 5.0, 25.0, 25.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.images_removed, 2, "{r:?}");
    assert!(!content(&reopen(&doc), 0).contains("/Im1 Do"), "{}", content(&reopen(&doc), 0));
    assert!(!saved_streams_hold(&doc, b"IMGBYTES"), "the image stayed in the file");
}

#[test]
fn redacted_image_bytes_are_gone_from_the_saved_file() {
    // Regression for #389: a scanned-page image under a mark must not survive in the saved file.
    // The image carries a canary byte run; after a full-rewrite save it must be absent, because
    // the page no longer references the original image XObject.
    const CANARY: &[u8] = &[0xCA, 0xFE, 0xBA, 0xBE, 0x11, 0x22, 0x33, 0x44];
    // 8×1 DeviceGray/8bpc image whose pixel bytes are the canary; a Flate scan stand-in.
    let img = stream("/Type /XObject /Subtype /Image /Width 8 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", CANARY);
    // Fully cover the single image so its `Do` is dropped and the resource entry removed.
    let src = b"q 80 0 0 10 10 10 cm /Scan Do Q";
    let mut doc = one_page(src, "/XObject << /Scan 7 0 R >>", vec![img]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 40.0]], "GONE");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.images_removed, 1, "{r:?}");
    assert!(doc.full_save_required(), "redaction forces a full save");
    let p = pdfcraft_model::pages(&doc).swap_remove(0);
    let res = doc.resolve(p.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = res.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().cloned()).unwrap_or_default();
    assert!(!xo.contains(b"Scan"), "the original image is still referenced by the page: {:?}", xo);
    // A full rewrite only writes reachable objects: the canary must be gone.
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes.windows(CANARY.len()).any(|w| w == CANARY), "redacted image bytes left in the saved file (#389)");
}

/// Final-review finding F1: no draw of a redacted image, through any resource name, visible or
/// clipped, may keep pixels that were under a mark in the saved file.
#[test]
fn aliases_and_other_draws_of_a_redacted_image_do_not_keep_the_original() {
    // An 8×1 image: the left half under one mark at its first draw, the right half under
    // another at its second.
    const LEFT: &[u8] = &[0xCA, 0xFE, 0xBA, 0xBE];
    const RIGHT: &[u8] = &[0x11, 0x22, 0x33, 0x44];
    let img = || stream("/Type /XObject /Subtype /Image /Width 8 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", [LEFT, RIGHT].concat());
    let leaks = |doc: &Document| saved_streams_hold(doc, LEFT) || saved_streams_hold(doc, RIGHT);
    // Two names for one image; only /Scan is drawn, and it is wholly under the mark.
    let mut doc = one_page(b"q 80 0 0 10 10 10 cm /Scan Do Q", "/XObject << /Scan 7 0 R /Alias 7 0 R >>", vec![img()]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 40.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert!(!leaks(&doc), "an unused alias kept the original image");
    // One image drawn three times, once through an alias. The marks clear the left half of the
    // first draw and the right half of the second; every draw uses one copy with both cleared.
    let mut doc = one_page(
        b"q 80 0 0 10 10 10 cm /Scan Do Q q 80 0 0 10 150 150 cm /Scan Do Q q 80 0 0 10 150 200 cm /Alias Do Q",
        "/XObject << /Scan 7 0 R /Alias 7 0 R >>",
        vec![img()],
    );
    mark(&mut doc, 0, &[[0.0, 0.0, 50.0, 40.0], [190.0, 140.0, 240.0, 170.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_cleared, r.images_removed), (2, 0), "{r:?}");
    let drawn = content(&doc, 0);
    assert!(!drawn.contains("/Scan Do") && !drawn.contains("/Alias Do"), "every draw uses the cleared copy: {drawn}");
    assert_eq!(drawn.matches("/PCRedacted1 Do").count(), 3, "one copy for every draw: {drawn}");
    assert!(!leaks(&doc), "a draw kept pixels another draw's mark cleared");
    // Wholly under a mark at one draw, and drawn again elsewhere clipped to a corner: the image
    // goes from both draws.
    let mut doc = one_page(
        b"q 80 0 0 10 10 10 cm /Scan Do Q q 150 150 5 5 re W n 80 0 0 10 150 150 cm /Alias Do Q",
        "/XObject << /Scan 7 0 R /Alias 7 0 R >>",
        vec![img()],
    );
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 40.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 2);
    let drawn = content(&doc, 0);
    assert!(!drawn.contains(" Do"), "{drawn}");
    assert!(!leaks(&doc), "a clipped draw kept the original image");
}

/// A form redacted on its only draw leaves the page's resources, so its original (unredacted)
/// stream is not kept in the saved file.
#[test]
fn a_redacted_form_does_not_keep_its_original() {
    let form =
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>", b"BT /F1 10 Tf 10 100 Td (SECRET) Tj ET");
    let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", vec![form]);
    mark(&mut doc, 0, &[[0.0, 90.0, 300.0, 120.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().forms_rewritten, 1);
    assert!(!saved_streams_hold(&doc, b"SECRET"), "the original form stayed in the file");
}

#[test]
fn shared_form_xobjects_are_redacted_on_every_page_or_refused() {
    let form =
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>", b"BT /F1 10 Tf 10 100 Td (SECRET) Tj ET");
    let shared = || {
        let mut objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 /MediaBox [0 0 300 300] /Resources << /XObject << /Fm 7 0 R >> >> >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
            stream("", b"/Fm Do"),
            FONT.replace("95 0 R", "6 0 R").into_bytes(),
            widths(),
            form.clone(),
        ];
        objs.push(b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec());
        pdf(objs)
    };
    // Page 2 draws the same form (and content stream) outside the marks: refused, page named.
    let mut doc = shared();
    mark(&mut doc, 0, &[[0.0, 90.0, 300.0, 120.0]], "");
    let err = apply(&mut doc, None).unwrap_err();
    assert!(matches!(&err, RedactError::StillUsed(p) if p == &[1]), "{err:?}");
    assert!(err.to_string().contains("on page 2"), "{err}");
    // Marked on both pages, both are redacted and the original form is gone from the file.
    let mut doc = shared();
    mark(&mut doc, 0, &[[0.0, 90.0, 300.0, 120.0]], "");
    mark(&mut doc, 1, &[[0.0, 90.0, 300.0, 120.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.glyphs, r.forms_rewritten), (12, 2), "{r:?}");
    assert_eq!(under(&mut doc, 1, &[[0.0, 90.0, 300.0, 120.0]]), 0);
    assert_eq!(under(&mut doc, 0, &[[0.0, 90.0, 300.0, 120.0]]), 0);
    assert!(!saved_streams_hold(&doc, b"SECRET"));
}

/// An image shared with another page that draws it outside the marks (clipped or not) can't be
/// redacted on one page alone; an entry nothing draws, inherited from the page tree, is dropped.
#[test]
fn images_used_elsewhere_are_refused_and_unused_entries_dropped() {
    const SECRET: &[u8] = &[0xCA, 0xFE, 0xBA, 0xBE, 0x11, 0x22, 0x33, 0x44];
    let img = || stream("/Type /XObject /Subtype /Image /Width 8 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", SECRET);
    let two_pages = |second: &[u8]| {
        pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 /MediaBox [0 0 300 300] /Resources << /XObject << /Scan 5 0 R >> >> >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
            stream("", b"q 80 0 0 10 10 10 cm /Scan Do Q"),
            img(),
            b"<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>".to_vec(),
            stream("", second),
        ])
    };
    // Page 2 draws it clipped to a corner: refused.
    let mut doc = two_pages(b"q 10 10 2 2 re W n 80 0 0 10 10 10 cm /Scan Do Q");
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 40.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::StillUsed(p)) if p == [1]));
    // Page 2 inherits the entry but draws nothing: the entry is dropped and the image is gone.
    let mut doc = two_pages(b"");
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 40.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert!(!saved_streams_hold(&doc, SECRET), "the inherited entry kept the original image");
}

/// Final-review findings on ac75a54: an unmarked draw split across a page's content streams, or
/// made by a form with unusual keys, still counts as a draw (redaction refuses instead of pruning
/// the image from under it), and a form whose marks can't be carried between its draws (a
/// singular matrix, or too many draws) makes redaction refuse.
#[test]
fn unusual_draws_make_redaction_refuse_rather_than_break_content() {
    let img = stream("/Type /XObject /Subtype /Image /Width 8 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", [7; 8]);
    let words = b"BT /F1 10 Tf 10 100 Td (LEFTSECRET) Tj 190 0 Td (RIGHTSECRET) Tj ET";
    // Objects: 1 catalog, 2 pages (shared resources), 3 and 6 pages, 4 and 7 contents, 5 the
    // image, 8 a form, 9 the font, 10 its widths, 11 a second content stream for page 2.
    let doc_with = |second_contents: &str, second: &[u8], form_dict: &str, form: &[u8], first: &[u8]| {
        pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 /MediaBox [0 0 300 300] /Resources << /XObject << /Scan 5 0 R /Fm 8 0 R >> /Font << /F1 9 0 R >> >> >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
            stream("", first),
            img.clone(),
            format!("<< /Type /Page /Parent 2 0 R /Contents {second_contents} >>").into_bytes(),
            stream("", second),
            stream(form_dict, form),
            FONT.replace("95 0 R", "10 0 R").into_bytes(),
            widths(),
            stream("", b" Do Q"),
        ])
    };
    let scan = b"q 80 0 0 10 10 10 cm /Scan Do Q";
    let still_used_on_page_2 = |doc: &mut Document| matches!(apply(doc, None), Err(RedactError::StillUsed(p)) if p == [1]);
    // Page 2 draws the image with `/Scan` ending one content stream and `Do` starting the next.
    let mut doc = doc_with("[7 0 R 11 0 R]", b"q 80 0 0 10 10 10 cm /Scan", "/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"", scan);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 40.0]], "");
    assert!(still_used_on_page_2(&mut doc), "a draw split across content streams was missed");
    // Page 2 draws a form that has a stray /N, and the form draws the inherited image.
    let mut doc = doc_with("7 0 R", b"/Fm Do", "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /N 1", scan, scan);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 40.0]], "");
    assert!(still_used_on_page_2(&mut doc), "a form with a stray /N was not read");
    // A form drawn normally on page 1 and through a singular matrix on page 2, both marked.
    let mut doc = doc_with("7 0 R", b"q 1 1 1 1 0 0 cm /Fm Do Q", "/Type /XObject /Subtype /Form /BBox [0 0 300 300]", words, b"/Fm Do");
    mark(&mut doc, 0, &[[0.0, 90.0, 120.0, 120.0]], "");
    mark(&mut doc, 1, &[[0.0, 0.0, 300.0, 300.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::SharedForm(_))), "a singular draw's marks were dropped");
    // One form drawn more often than its marks can be carried between.
    let many: Vec<u8> = (0..300).flat_map(|i| format!("q 1 0 0 1 0 {} cm /Fm Do Q ", i % 300).into_bytes()).collect();
    let mut doc = doc_with("7 0 R", b"", "/Type /XObject /Subtype /Form /BBox [0 0 300 300]", words, &many);
    mark(&mut doc, 0, &[[0.0, 0.0, 300.0, 300.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::SharedForm(p)) if p == [0]));
}

/// The same image shown on another page is not under a mark there: that page keeps it.
#[test]
fn an_image_another_page_still_draws_through_the_same_resources_is_refused() {
    for inherited in [true, false] {
        let mut doc = two_pages_one_resource_dictionary(inherited, true);
        mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
        assert!(matches!(apply(&mut doc, None), Err(RedactError::StillUsed(p)) if p == [1]), "inherited: {inherited}");
    }
}

/// The page draws an image under the mark and, outside it, a form without /Resources of its own
/// that draws the same image through the page's, whether the form lies wholly outside the mark or
/// is entered and left as it was: the page still shows the image there, so the fork refuses.
#[test]
fn an_image_a_form_draws_again_through_the_pages_resources_is_refused() {
    for bbox in ["[0 0 100 100]", "[0 0 300 300]"] {
        for none in NO_RESOURCES {
            let mut doc = one_page(
                b"q 50 0 0 50 20 230 cm /Im1 Do Q /FmBare Do",
                "/XObject << /FmBare 7 0 R /Im1 8 0 R >>",
                vec![
                    stream(&format!("/Type /XObject /Subtype /Form /BBox {bbox} {none}"), b"q 50 0 0 50 20 20 cm /Im1 Do Q"),
                    stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"PUBIMGSHOWNAGAIN"),
                ],
            );
            mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
            assert!(matches!(apply(&mut doc, None), Err(RedactError::StillUsed(p)) if p == [0]), "{bbox} {none}");
        }
    }
}

/// The same one level down: a form with resources of its own draws the image under the mark and,
/// outside it, a form without any that draws it again through the first form's. Refused.
#[test]
fn an_image_a_nested_form_draws_again_through_its_callers_resources_is_refused() {
    for none in NO_RESOURCES {
        let mut doc = one_page(
            b"/FmOwn Do",
            "/XObject << /FmOwn 7 0 R >>",
            vec![
                stream(
                    "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /XObject << /FmBare 8 0 R /Im1 9 0 R >> >>",
                    b"q 50 0 0 50 20 230 cm /Im1 Do Q /FmBare Do",
                ),
                stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 100 100] {none}"), b"q 50 0 0 50 20 20 cm /Im1 Do Q"),
                stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"PUBIMGSHOWNAGAIN"),
            ],
        );
        mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
        assert!(matches!(apply(&mut doc, None), Err(RedactError::StillUsed(p)) if p == [0]), "{none}");
    }
}

/// A form outside the mark draws the image again through the page's resources, its content behind
/// a predictor whose `/DecodeParms` are given by reference. Both checks of whether the page still
/// draws it read the form through them, the page's (`resource_ops`) and the unlinking one at the
/// end (`names_content_draws`), and so does the fork's check of what the page still shows: the
/// form draws the image outside the mark, so the fork refuses.
#[test]
fn an_image_a_form_with_parameters_by_reference_draws_again_is_refused() {
    let mut doc = one_page(
        b"q 50 0 0 50 20 230 cm /Im1 Do Q /FmBare Do",
        "/XObject << /FmBare 7 0 R /Im1 8 0 R >>",
        vec![
            stream("/Type /XObject /Subtype /Form /BBox [0 0 100 100]", b""),
            stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"PUBIMGSHOWNAGAIN"),
        ],
    );
    let form = behind_parms_by_reference(&mut doc, &form_entries([0, 0, 100, 100]), b"q 50 0 0 50 20 20 cm /Im1 Do Q");
    doc.set(ObjRef::new(7, 0), form);
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::StillUsed(p)) if p == [0]));
}

/// A surviving form outside the mark shares the page's resources and draws the image itself, its
/// content behind a predictor whose `/DecodeParms` are given by reference: unlinking the redacted
/// image from the forms' resources reads it through them, as the fork's check does: the form still
/// draws the image outside the mark, so the fork refuses.
#[test]
fn an_image_a_surviving_form_with_parameters_by_reference_draws_is_refused() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"q 100 0 0 100 0 0 cm /Im Do Q /Fm Do"),
        b"<< /XObject << /Im 6 0 R /Fm 7 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 11 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"IMAGESECRET"),
        stream("/Type /XObject /Subtype /Form /BBox [150 150 300 300]", b""),
    ]);
    let mut entries = form_entries([150, 150, 300, 300]);
    entries.push(("Resources", Object::Ref(ObjRef::new(5, 0))));
    let form = behind_parms_by_reference(&mut doc, &entries, b"q 50 0 0 50 200 200 cm /Im Do Q");
    doc.set(ObjRef::new(7, 0), form);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 120.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::StillUsed(p)) if p == [0]));
}

/// A form without resources of its own that the redaction rewrites gets a new name. It keeps none
/// of its own either when its copy of the page's loses nothing, which takes a form drawing itself
/// (here off the page, so outside the mark), and then goes on drawing through the page's: itself
/// and, through a form it draws unchanged, the image under the mark again outside it. The page's
/// check of what it still draws follows the rewritten form, and so does the fork's: the unchanged
/// form still draws the image outside the mark, so the fork refuses.
#[test]
fn an_image_a_rewritten_form_draws_again_through_the_pages_resources_is_refused() {
    let mut doc = one_page(
        b"q 50 0 0 50 20 230 cm /Im1 Do Q /FmBare Do",
        "/XObject << /FmBare 7 0 R /Inner 8 0 R /Im1 9 0 R >>",
        vec![
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300]",
                b"BT /F1 10 Tf 20 250 Td (SECRET) Tj ET /Inner Do q 1 0 0 1 0 -1000 cm /FmBare Do Q",
            ),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 100 100]", b"q 50 0 0 50 20 20 cm /Im1 Do Q"),
            stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"PUBIMGSHOWNAGAIN"),
        ],
    );
    mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::StillUsed(p)) if p == [0]));
}

#[test]
fn a_reply_chain_goes_with_its_comment() {
    // Replies listed before what they reply to, outside the mark: the whole chain goes.
    let mut doc = annotated_page(
        "",
        "8 0 R 7 0 R 6 0 R 5 0 R",
        vec![
            b"<< /Type /Annot /Subtype /Text /Rect [20 20 40 40] /Contents (comment) >>".to_vec(),
            b"<< /Type /Annot /Subtype /Text /Rect [200 200 220 220] /IRT 5 0 R /Contents (reply 1) >>".to_vec(),
            b"<< /Type /Annot /Subtype /Text /Rect [200 200 220 220] /IRT 6 0 R /Contents (reply 2) >>".to_vec(),
            b"<< /Type /Annot /Subtype /Text /Rect [200 200 220 220] /IRT 7 0 R /Contents (CANARY-CHAIN) >>".to_vec(),
        ],
    );
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 80.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().annotations, 4);
    assert!(!saved_bytes_hold(&doc, b"CANARY-CHAIN"), "a reply chain kept the removed comment");
}

#[test]
fn an_inline_comment_under_a_mark_goes() {
    let mut doc = annotated_page("", "<< /Type /Annot /Subtype /Text /Rect [20 20 40 40] /Contents (CANARY-INLINE) >>", vec![]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 80.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().annotations, 1);
    assert!(!saved_bytes_hold(&doc, b"CANARY-INLINE"), "the inline comment stayed");
}

#[test]
fn a_tagged_sibling_widget_does_not_keep_a_redacted_field() {
    // One widget under the mark, one outside it that the structure tree tags.
    let mut doc = annotated_page(
        "/AcroForm << /Fields [5 0 R] >> /StructTreeRoot 8 0 R /MarkInfo << /Marked true >>",
        "6 0 R 7 0 R",
        vec![
            b"<< /FT /Tx /T (name) /V (CANARY-SIBLING) /Kids [6 0 R 7 0 R] >>".to_vec(),
            b"<< /Type /Annot /Subtype /Widget /Parent 5 0 R /Rect [20 20 40 40] >>".to_vec(),
            b"<< /Type /Annot /Subtype /Widget /Parent 5 0 R /Rect [200 200 220 220] /StructParent 0 >>".to_vec(),
            b"<< /Type /StructTreeRoot /K 9 0 R >>".to_vec(),
            b"<< /Type /StructElem /S /Form /P 8 0 R /K [<< /Type /OBJR /Obj 7 0 R >>] >>".to_vec(),
        ],
    );
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 80.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().fields, 1);
    assert!(!saved_bytes_hold(&doc, b"CANARY-SIBLING"), "the tagged sibling widget kept the field");
}

#[test]
fn an_inline_widget_under_a_mark_takes_its_field() {
    // The fork refused this before upstream deleted an inline widget's `/Parent` field with it.
    let mut doc = annotated_page(
        "/AcroForm << /Fields [5 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /Parent 5 0 R /Rect [20 20 40 40] >>",
        vec![b"<< /FT /Tx /T (name) /V (CANARY-INLINE-FIELD) >>".to_vec()],
    );
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 80.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().fields, 1);
    assert!(!saved_bytes_hold(&doc, b"CANARY-INLINE-FIELD"), "the inline widget's field kept its value");
}

#[test]
fn a_pop_up_named_only_by_a_removed_comment_goes() {
    // The pop-up has no /Parent pointing back at the comment.
    let mut doc = annotated_page(
        "",
        "5 0 R 6 0 R",
        vec![
            b"<< /Type /Annot /Subtype /Text /Rect [20 20 40 40] /Contents (comment) /Popup 6 0 R >>".to_vec(),
            b"<< /Type /Annot /Subtype /Popup /Rect [150 150 250 250] /Contents (CANARY-POPUP) >>".to_vec(),
        ],
    );
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 80.0]], "");
    apply(&mut doc, None).unwrap();
    assert!(!saved_bytes_hold(&doc, b"CANARY-POPUP"), "the pop-up stayed");
}

#[test]
fn a_removed_link_sharing_its_action_with_a_kept_one_is_refused() {
    let mut doc = annotated_page(
        "",
        "5 0 R 6 0 R",
        vec![
            b"<< /Type /Annot /Subtype /Link /Rect [20 20 40 40] /A 7 0 R >>".to_vec(),
            b"<< /Type /Annot /Subtype /Link /Rect [200 200 220 220] /A 7 0 R >>".to_vec(),
            b"<< /S /URI /URI (mailto:shared@example.invalid) >>".to_vec(),
        ],
    );
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 80.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::StillUsed(_))));

    // Its own indirect text and action go with it.
    let mut doc = annotated_page(
        "",
        "5 0 R",
        vec![
            b"<< /Type /Annot /Subtype /Link /Rect [20 20 40 40] /A 6 0 R /Contents 7 0 R >>".to_vec(),
            b"<< /S /URI /URI (mailto:own@example.invalid) >>".to_vec(),
            b"(CANARY-OWN-TEXT)".to_vec(),
        ],
    );
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 80.0]], "");
    apply(&mut doc, None).unwrap();
    assert!(!saved_bytes_hold(&doc, b"own@example.invalid") && !saved_bytes_hold(&doc, b"CANARY-OWN-TEXT"));
}

#[test]
fn what_a_removed_annotation_names_elsewhere_fails_closed() {
    // An inline comment sharing its indirect text with a kept comment.
    let mut doc = annotated_page(
        "",
        "<< /Type /Annot /Subtype /Text /Rect [20 20 40 40] /Contents 6 0 R >> 5 0 R",
        vec![b"<< /Type /Annot /Subtype /Text /Rect [200 200 220 220] /Contents 6 0 R >>".to_vec(), b"(shared text)".to_vec()],
    );
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 80.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::StillUsed(_))));

    // A /Popup naming a kept comment rather than a pop-up: the comment stays and apply refuses.
    let mut doc = annotated_page(
        "",
        "5 0 R 6 0 R",
        vec![
            b"<< /Type /Annot /Subtype /Text /Rect [20 20 40 40] /Contents (comment) /Popup 6 0 R >>".to_vec(),
            b"<< /Type /Annot /Subtype /Text /Rect [200 200 220 220] /Contents (kept) >>".to_vec(),
        ],
    );
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 80.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::StillUsed(_))));
}

#[test]
fn unselected_pages_drawing_an_inherited_form_refuse_its_redaction() {
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"BT /F1 10 Tf 10 200 Td (SECRET) Tj ET");
    let mut doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", vec![form]);
    let mut page = doc.get(ObjRef::new(3, 0)).as_dict().unwrap().clone();
    let resources = page.remove(b"Resources").unwrap();
    let second = doc.add(Object::Dict(page));
    doc.update_dict(ObjRef::new(2, 0), |d| {
        d.set(b"Resources".to_vec(), resources);
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(3, 0)), Object::Ref(second)]));
        d.set(b"Count".to_vec(), Object::Int(2));
    })
    .unwrap();
    doc.update_dict(ObjRef::new(3, 0), |d| {
        d.remove(b"Resources");
    })
    .unwrap();
    mark(&mut doc, 0, &[[0.0, 190.0, 300.0, 220.0]], "");
    assert!(matches!(apply(&mut doc, Some(&[0])), Err(RedactError::StillUsed(p)) if p == [1]));
    let mut saved = reopen(&doc);
    assert_eq!(under(&mut saved, 0, &[[0.0, 190.0, 300.0, 220.0]]), 6, "nothing was changed");
    assert_eq!(under(&mut saved, 1, &[[0.0, 190.0, 300.0, 220.0]]), 6);
}

#[test]
fn redacting_a_form_every_page_inherits_is_refused_and_leaves_the_pages_untouched() {
    let (mut doc, _) = inheriting_pages(50);
    mark(&mut doc, 0, &[[0.0, 190.0, 300.0, 220.0]], "");
    let before = doc.modified_objects();
    assert!(matches!(apply(&mut doc, Some(&[0])), Err(RedactError::StillUsed(p)) if p == (1..50).collect::<Vec<_>>()));
    assert_eq!(doc.modified_objects(), before);
    let mut saved = reopen(&doc);
    assert_eq!(under(&mut saved, 0, &[[0.0, 190.0, 300.0, 220.0]]), 6);
    assert_eq!(under(&mut saved, 49, &[[0.0, 190.0, 300.0, 220.0]]), 6);
}

/// Add `/Big` to the resources of page-tree node 2: a form past what `drawn_names` may read for a
/// page, which then can't tell what the page draws.
fn add_form_too_large_to_check(doc: &mut Document) {
    let mut dict = Dict::new();
    dict.set(b"Subtype".to_vec(), Object::name("Form"));
    let big = doc.add(Object::Stream(Stream::flate(dict, &vec![b' '; DRAWN_BUDGET + 1])));
    let node = ObjRef::new(2, 0);
    let mut res = doc.get(node).as_dict().and_then(|d| d.get(b"Resources")).and_then(Object::as_dict).cloned().unwrap();
    let mut xo = res.get(b"XObject").and_then(Object::as_dict).cloned().unwrap();
    xo.set(b"Big".to_vec(), Object::Ref(big));
    res.set(b"XObject".to_vec(), Object::Dict(xo));
    doc.update_dict(node, |d| d.set(b"Resources".to_vec(), Object::Dict(res))).unwrap();
}

/// A comment's appearance under a mark goes with it, though the pages' shared resources also list
/// it, as neither page draws it. Once those resources hold more than `drawn_names` may read, the
/// check can't tell that: apply refuses, and says so rather than asking for those pages to be
/// marked, which can't help.
#[test]
fn a_page_too_large_to_check_is_refused_without_asking_for_more_marks() {
    for with_big in [false, true] {
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 /MediaBox [0 0 300 300] /Resources << /XObject << /Icon 5 0 R >> >> >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Annots [7 0 R] >>".to_vec(),
            stream("", b""),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 20 20]", b"0 0 20 20 re f"),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
            b"<< /Type /Annot /Subtype /Text /Rect [20 20 40 40] /Contents (note) /AP << /N 5 0 R >> >>".to_vec(),
        ]);
        if with_big {
            add_form_too_large_to_check(&mut doc);
        }
        mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 80.0]], "");
        let result = apply(&mut doc, None);
        if with_big {
            assert!(matches!(&result, Err(RedactError::Unchecked(p)) if p == &[0, 1]), "{result:?}");
        } else {
            assert_eq!(result.map(|r| r.annotations), Ok(1));
        }
    }
    // A page that draws the original through its contents is still asked to be marked.
    let (mut doc, _) = inheriting_pages(2);
    add_form_too_large_to_check(&mut doc);
    mark(&mut doc, 0, &[[0.0, 190.0, 300.0, 220.0]], "");
    let result = apply(&mut doc, None);
    assert!(matches!(&result, Err(RedactError::StillUsed(p)) if p == &[1]), "{result:?}");
}

/// What `drawn_names` keeps for later pages: a form's names once each, however often it draws
/// them, and no more streams once it holds `DRAWN_BUDGET` bytes of names. What it returns is the
/// same either way.
#[test]
fn drawn_names_keeps_each_name_once_and_stops_keeping_at_the_budget() {
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", b"/A Do /A Do /B Do /A Do");
    let doc = one_page(b"/Fm Do", "/XObject << /Fm 7 0 R >>", vec![form]);
    let page = &pdfcraft_model::pages(&doc)[0];
    let roots: Vec<Object> = page.dict.get(b"Resources").cloned().into_iter().collect();
    let expected: HashSet<Vec<u8>> = [&b"Fm"[..], b"A", b"B"].iter().map(|n| n.to_vec()).collect();
    let mut read = ReadStreams::default();
    assert_eq!(drawn_names(&doc, &page.dict, 0, &roots, &mut read), Some(expected.clone()));
    let kept = read.streams.get(&ObjRef::new(7, 0)).cloned().flatten().map(|(_, names)| names.len());
    assert_eq!(kept, Some(2), "each name once");
    let mut full = ReadStreams { name_bytes: DRAWN_BUDGET + 1, ..ReadStreams::default() };
    assert_eq!(drawn_names(&doc, &page.dict, 0, &roots, &mut full), Some(expected));
    assert!(full.streams.is_empty(), "nothing more kept past the budget");
}

/// A redaction rewrites only the content streams it changes and copies the others as they are. A
/// later stream can hold `EI` inside a token (`FOOEI`, an operator allowed inside `BX`/`EX`, or the
/// name `/BEI`). The renderer reads such an `EI` as a possible image end when no shown text and
/// fewer than 21 operators lie between it and an image's real `EI`. So once paths after an inline
/// image are redacted away, the image could swallow the operators up to that `EI`.
#[test]
fn a_copied_stream_holding_ei_does_not_move_the_end_of_an_inline_image_before_it() {
    // 20 operators after the image, and the copied stream's first, let the renderer take its real
    // `EI` in the source; the ones that fill the redacted rectangles go. Without the `cm` and `rg`,
    // the image would swallow only the `/B` of `/BEI`, leaving `gs` without its operand.
    let image = b"q BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7F> EI";
    let kept = [&image[..], b" 1 0 0 1 5 5 cm 0 0 0 rg", &b" 0 210 10 10 re f".repeat(9)].concat();
    let bare = [&image[..], &b" 0 210 10 10 re f".repeat(10)].concat();
    let ends = |content: &[u8]| {
        let parsed = pdfcraft_content::parse(content);
        let image = parsed.ops.iter().find_map(|o| o.inline.as_ref().map(|(_, data)| data.clone()));
        let states: Vec<_> = parsed.ops.iter().filter(|o| o.is("gs")).map(|o| o.operands.clone()).collect();
        (parsed.ambiguous_images, image, parsed.ops.iter().filter(|o| o.is("cm")).count(), states)
    };
    let bei = || vec![vec![pdfcraft_cos::Object::Name(b"BEI".to_vec())]];
    for (first, tail, cms, states) in [(&kept, &b"BX 1 FOOEI EX Q"[..], 1, vec![]), (&kept, b"/BEI gs Q", 1, bei()), (&bare, b"/BEI gs Q", 0, bei())]
    {
        let label = String::from_utf8_lossy(tail);
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R 5 0 R] /Resources << /Font << /F1 6 0 R >> /ExtGState << /BEI << /ca 0.5 >> >> >> >>".to_vec(),
            stream("", first),
            stream("/PCAdded << /Kind /Shape >>", tail),
            FONT.replace("95 0 R", "7 0 R").into_bytes(),
            widths(),
        ]);
        let expected = (0, Some(b"7F>".to_vec()), cms, states);
        assert_eq!(ends(content(&doc, 0).as_bytes()), expected, "the source, {label}, {cms} cm");
        mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
        apply(&mut doc, None).unwrap();
        let after = content(&doc, 0);
        assert_eq!(ends(after.as_bytes()), expected, "redacted, {label}, {cms} cm: {after}");
        // Re-encoded only, the copied stream draws what it drew, so its added item stays editable.
        assert_eq!(added_items(&doc), 1, "{label}, {cms} cm");
    }
}

/// How many of page 0's content streams carry an added item's `/PCAdded`.
fn added_items(doc: &Document) -> usize {
    use pdfcraft_cos::Object;
    let page = &pdfcraft_model::pages(doc)[0];
    let contents = page.dict.get(b"Contents").map(|c| doc.resolve(c));
    let Some(Object::Array(list)) = contents.as_deref() else { return 0 };
    list.iter().filter(|s| matches!(&*doc.resolve(s), Object::Stream(s) if s.dict.get(b"PCAdded").is_some())).count()
}

/// An added item after an inline image whose shown text holds `EI` (`RECEIPT`) can't end the image:
/// shown text after an image makes the renderer keep its real `EI`. A redaction elsewhere on the
/// page copies the item, which stays editable.
#[test]
fn an_added_item_whose_text_holds_ei_stays_editable_after_a_redaction_elsewhere() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R 5 0 R 6 0 R] /Resources << /Font << /F1 7 0 R >> >> >>".to_vec(),
        stream("", b"q BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7F> EI Q"),
        stream("", b"BT /F1 12 Tf 10 250 Td (SECRET) Tj ET"),
        stream("/PCAdded << /Kind /Text >>", b"BT /F1 12 Tf 10 50 Td (RECEIPT) Tj ET"),
        FONT.replace("95 0 R", "8 0 R").into_bytes(),
        widths(),
    ]);
    mark(&mut doc, 0, &[[0.0, 240.0, 300.0, 270.0]], "");
    apply(&mut doc, None).unwrap();
    let after = content(&doc, 0);
    assert!(!after.contains("SECRET") && after.contains("(RECEIPT) Tj"), "{after}");
    assert_eq!(added_items(&doc), 1);
}

/// The check that refuses a page whose rewritten content would not read back: a piece written as
/// the writer writes it, or copied, reads back; one whose image data was cut short does not.
#[test]
fn content_whose_inline_image_ends_elsewhere_does_not_read_back() {
    let source = b"q BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7F> EI Q".to_vec();
    let ops = pdfcraft_content::parse(&source).ops;
    let processed = [(ops.clone(), true)];
    let streams = [source.clone()];
    assert!(crate::interp::reads_back(&[None], &streams, &processed), "copied");
    assert!(crate::interp::reads_back(&[Some(pdfcraft_content::serialize_ops(&ops))], &streams, &processed), "written");
    let cut = b"q\nBI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7\nEI\nQ\n".to_vec();
    assert!(!crate::interp::reads_back(&[Some(cut)], &streams, &processed), "cut short");
}

/// Removed text that couldn't all be collected stays so when later applies add text that could:
/// the save's search must still count every place holding any text.
#[test]
fn removed_text_cut_short_stays_cut_short_when_more_is_added() {
    let mut held = crate::Removed::new(Vec::new(), false);
    held.extend(&crate::Removed::new(vec!["Bluefin".into()], true));
    let doc = Document::open(Arc::new(pdf_with_outline_title("Annual figures"))).unwrap();
    let left = crate::survivors(&doc, &held);
    assert!(left.unsearched && left.refuses(), "{left:?}");
}

/// A one-page document whose only bookmark is titled `title`.
fn pdf_with_outline_title(title: &str) -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>".to_string(),
        "<< /Type /Outlines /First 5 0 R /Last 5 0 R /Count 1 >>".to_string(),
        format!("<< /Title ({title}) /Parent 4 0 R >>"),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    pdf
}

/// A one-page document with `catalog` and `page` entries added, `extra` objects numbered from 4
/// and `trailer` entries added to its trailer.
fn doc_with(catalog: &str, page: &str, extra: &[&str], trailer: &str) -> Document {
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] {page} >>"),
    ];
    objs.extend(extra.iter().map(ToString::to_string));
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R {trailer} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(pdf)).unwrap()
}

/// Whether a save of `doc` refuses after `removed` was redacted.
fn save_refuses(doc: &Document, removed: &[&str]) -> bool {
    crate::survivors(doc, &crate::Removed::new(removed.iter().map(ToString::to_string).collect(), true)).refuses()
}

/// A stream object holding `data`.
fn stream_of(dict: &str, data: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{data}\nendstream", data.len())
}

/// A save's check matches whole removed runs (review B3): a short run, or one word of a removed
/// sentence, in a field's default appearance, a link's address or a destination's name is not
/// the redacted text.
#[test]
fn a_save_refuses_only_for_a_whole_removed_run() {
    let field = "<< /FT /Tx /T (total) /DA (/Helv 12 Tf 0 g) >>";
    assert!(!save_refuses(&doc_with("/AcroForm << /Fields [4 0 R] >>", "", &[field], ""), &["12"]), "a field's /DA holds '12'");
    let link = "<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A << /S /URI /URI (https://example.com/annual-report) >> >>";
    let doc = doc_with("", "/Annots [4 0 R]", &[link], "");
    assert!(!save_refuses(&doc, &["Codename BLUEFIN report"]), "one word of the sentence is in the address");
    assert!(save_refuses(&doc, &["annual report"]), "the whole run is");
    let dests = "/Names << /Dests << /Names [(page.1) [3 0 R /Fit] (table.1) [3 0 R /Fit]] >> >>";
    assert!(!save_refuses(&doc_with(dests, "", &[], ""), &["See the table on page 4"]));
}

/// A redacted year is not in the date a save stamps, or an earlier one (review B1).
#[test]
fn a_redacted_year_is_not_in_a_date() {
    let doc = doc_with("", "", &["<< /ModDate (D:20231114221320Z) /CreationDate (D:20231101) >>"], "/Info 4 0 R");
    assert!(!save_refuses(&doc, &["2023"]));
}

/// A named destination's name is told, not refused (review B2): no category or edit removes one
/// name, so refusing would leave no way to save.
#[test]
fn a_named_destination_holding_the_text_is_told_not_refused() {
    let doc = doc_with("/Names << /Dests << /Names [(Bluefin plans) [3 0 R /Fit]] >> >>", "", &[], "");
    assert!(!save_refuses(&doc, &["BLUEFIN"]));
}

/// Binary attachment data isn't read as text (review B4): squeezed to its letters, any short term
/// is somewhere in a photo. Text data still is, found by its use under `/EF` whatever its `/Type`
/// (review S4).
#[test]
fn attachment_data_is_searched_only_as_text() {
    let names = "/Names << /EmbeddedFiles << /Names [(photo.png) 4 0 R] >> >> /AF [6 0 R]";
    let photo = stream_of("/Type /EmbeddedFile", "\u{1}PNG\r\n\u{1a}\n\u{0}\u{0}xBLUEFINx\u{0}");
    let spec = "<< /Type /Filespec /F (photo.png) /EF << /F 5 0 R >> >>";
    assert!(!save_refuses(
        &doc_with(names, "", &[spec, &photo, "<< /Type /Filespec /F (a.txt) /EF << /F 7 0 R >> >>", &stream_of("", "notes")], ""),
        &["BLUEFIN"]
    ));
    let text = stream_of("", "draft: Codename BLUEFIN report");
    assert!(save_refuses(&doc_with(names, "", &[spec, &photo, "<< /Type /Filespec /F (a.txt) /EF << /F 7 0 R >> >>", &text], ""), &["BLUEFIN"]));
}

/// XFA form data is searched as form fields (review S3).
#[test]
fn xfa_form_data_holding_the_text_refuses() {
    let data = stream_of(
        "",
        "<xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data><f><name>Bluefin</name></f></xfa:data></xfa:datasets>",
    );
    assert!(save_refuses(&doc_with("/AcroForm << /Fields [] /XFA [(datasets) 4 0 R] >>", "", &[&data], ""), &["BLUEFIN"]));
}

/// A signature's name, reason and location are searched; only its data is skipped (review S5).
#[test]
fn a_signers_name_holding_the_text_refuses() {
    let sig = "<< /Type /Sig /Name (Bluefin) /Reason (approval) /ByteRange [0 0 0 0] /Contents <00> >>";
    let field = "<< /FT /Sig /T (sig) /V 5 0 R >>";
    assert!(save_refuses(&doc_with("/AcroForm << /Fields [4 0 R] >>", "", &[field, sig], ""), &["BLUEFIN"]));
}

/// An object that can't be read is written as null by the full save a redaction needs
/// (`write_full` treats it as a dangling reference), so it holds nothing back and doesn't refuse:
/// refusing would leave no way to save (review S6, settled by reading the writer).
#[test]
fn an_object_that_cant_be_read_holds_nothing_back() {
    let doc = doc_with("/Extra 4 0 R", "", &["<< /Title (Report) /Author ) >>"], "");
    assert!(pdfcraft_cos::reachable_objects(&doc).is_ok_and(|r| !r.contains(&4)), "the full save leaves it out");
    assert!(!save_refuses(&doc, &["BLUEFIN"]));
}

/// An object nothing refers to isn't written by the full save a redaction needs, so it holds
/// nothing back.
#[test]
fn an_object_nothing_refers_to_holds_nothing_back() {
    assert!(!save_refuses(&doc_with("", "", &["<< /S /URI /URI (https://example.com/bluefin) >>"], ""), &["BLUEFIN"]));
}

/// Right-to-left text collected in drawing order is found where it is stored in logical order
/// (review S9).
#[test]
fn right_to_left_text_is_found_in_either_order() {
    let outline = "<< /Type /Outlines /First 5 0 R /Last 5 0 R /Count 1 >>";
    let doc = doc_with("/Outlines 4 0 R", "", &[outline, "<< /Title <FEFF05E905DC05D505DD> /Parent 4 0 R >>"], "");
    assert!(save_refuses(&doc, &["\u{5DD}\u{5D5}\u{5DC}\u{5E9}"]), "drawn right to left, stored left to right");
}

/// Strings anywhere in an `/Info` entry are searched, not only a string value (review N3).
#[test]
fn an_info_entry_that_is_an_array_is_searched() {
    let doc = doc_with("", "", &["<< /Keywords [(Bluefin) (plans)] >>"], "/Info 4 0 R");
    assert!(save_refuses(&doc, &["BLUEFIN"]));
}

/// Private data refuses: Remove hidden information's Private data removes it (review N4).
#[test]
fn private_data_holding_the_text_refuses() {
    assert!(save_refuses(&doc_with("", "/PieceInfo << /App << /Private << /Note (Bluefin) >> >> >>", &[], ""), &["BLUEFIN"]));
}

/// A catalog's base address doesn't make the catalog's own strings links (review N5).
#[test]
fn a_base_address_doesnt_make_the_catalog_a_link() {
    let catalog = "/URI << /Base (https://example.com/) >> /PageLabels << /Nums [0 << /S /D /P (Bluefin-) >>] >>";
    assert!(!save_refuses(&doc_with(catalog, "", &[], ""), &["BLUEFIN"]));
}

/// A portfolio's fields for an attachment are searched with it (review N6).
#[test]
fn a_portfolio_field_holding_the_text_refuses() {
    let names = "/Names << /EmbeddedFiles 5 0 R >>";
    assert!(save_refuses(
        &doc_with(names, "", &["<< /Type /Filespec /F (a.txt) /CI << /Owner (Bluefin) >> >>", "<< /Names [(a.txt) 4 0 R] >>"], ""),
        &["BLUEFIN"]
    ));
}

/// What a save's search costs on a large file (review S11): 10,000 pages, each with a link and a
/// form field, and 20 text attachments of 1 MB. Run with `--ignored --nocapture` to read it.
#[test]
#[ignore = "a timing, not a check"]
fn a_saves_search_of_a_large_file_takes() {
    let pages = 10_000;
    let mut extra = Vec::new();
    let mut kids = Vec::new();
    let mut fields = Vec::new();
    // Objects from 4: per page a page object, a link and a field widget.
    for p in 0..pages {
        let at = 4 + p * 3;
        kids.push(format!("{at} 0 R"));
        fields.push(format!("{} 0 R", at + 2));
        extra.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Annots [{} 0 R {} 0 R] >>", at + 1, at + 2));
        extra.push(format!("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A << /S /URI /URI (https://example.com/page/{p}) >> >>"));
        extra.push(format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (f{p}) /V (value {p}) /Rect [0 20 10 30] >>"));
    }
    let first_file = 4 + pages * 3;
    let names: Vec<String> = (0..20).map(|i| format!("(f{i}.txt) {} 0 R", first_file + i * 2)).collect();
    let text = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(18_000);
    for i in 0..20 {
        extra.push(format!("<< /Type /Filespec /F (f{i}.txt) /EF << /F {} 0 R >> >>", first_file + i * 2 + 1));
        extra.push(stream_of("/Type /EmbeddedFile", &text));
    }
    let catalog = format!("/AcroForm << /Fields [{}] >> /Names << /EmbeddedFiles << /Names [{}] >> >>", fields.join(" "), names.join(" "));
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"),
        format!("<< /Type /Pages /Kids [{}] /Count {pages} >>", kids.join(" ")),
        "<< >>".to_string(),
    ];
    objs.extend(extra);
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let doc = Document::open(Arc::new(pdf)).unwrap();
    let removed: Vec<String> = (0..200).map(|i| format!("Removed sentence number {i} of the redacted page")).collect();
    let started = std::time::Instant::now();
    let left = crate::survivors(&doc, &crate::Removed::new(removed, true));
    eprintln!("S11: {pages} pages, 20 x {} byte attachments, 200 removed runs: {:?}, refuses {}", text.len(), started.elapsed(), left.refuses());
    assert!(!left.refuses(), "nothing in the file holds a removed run: {:?}", left.items.first());
}

/// A large redaction searched for in a file with many strings stays within the search's work
/// limit (delta 2 review NB1): past it, every place would refuse without being searched.
#[test]
fn a_large_redaction_in_a_file_of_many_strings_is_searched_in_full() {
    let links = 2_000;
    let annots: Vec<String> = (0..links).map(|i| format!("{} 0 R", 4 + i)).collect();
    let extra: Vec<String> = (0..links)
        .map(|i| format!("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A << /S /URI /URI (https://example.com/page/{i}/section) >> >>"))
        .collect();
    let extra: Vec<&str> = extra.iter().map(String::as_str).collect();
    let doc = doc_with("", &format!("/Annots [{}]", annots.join(" ")), &extra, "");
    let removed: Vec<String> = (0..2_000).map(|i| format!("Removed sentence number {i} of the redacted page about quarterly figures")).collect();
    let left = crate::survivors(&doc, &crate::Removed::new(removed, true));
    assert!(!left.refuses() && !left.unsearched, "{} places, unsearched {}", left.len(), left.unsearched);
}

/// One word of a redacted line is not a reason to refuse, even when it is a field's whole name
/// or option (delta 2 review S1).
#[test]
fn a_one_word_field_string_inside_a_redacted_line_is_not_refused() {
    let fields = "/AcroForm << /Fields [4 0 R 5 0 R] >>";
    let extra = ["<< /FT /Tx /T (Date) >>", "<< /FT /Ch /T (consent) /Opt [(Yes) (No)] >>"];
    assert!(!save_refuses(&doc_with(fields, "", &extra, ""), &["Date: 12 May 2026", "Yes, I consent"]));
}

/// The parts of a labelled line are runs of their own (delta 2 review S2): the address after
/// "Email:" in a link, the name after "Name:" in a bookmark.
#[test]
fn the_parts_of_a_labelled_line_refuse_on_their_own() {
    let link = "<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A << /S /URI /URI (mailto:jane.citizen@example.com) >> >>";
    assert!(save_refuses(&doc_with("", "/Annots [4 0 R]", &[link], ""), &["Email: jane.citizen@example.com"]));
    let outline = "<< /Type /Outlines /First 5 0 R /Last 5 0 R /Count 1 >>";
    let doc = doc_with("/Outlines 4 0 R", "", &[outline, "<< /Title (Interview with Jane Citizen) /Parent 4 0 R >>"], "");
    assert!(save_refuses(&doc, &["Name: Jane Citizen"]));
}

/// A form's own wording (choice options, tooltip, names) refuses only for a whole removed run, not
/// for words of one (delta 3 review NB2); a filled-in value still does.
#[test]
fn a_forms_own_wording_refuses_only_for_a_whole_run() {
    let fields = "/AcroForm << /Fields [4 0 R] >>";
    let state = ["<< /FT /Ch /T (state) /Opt [(New South Wales) (Victoria)] >>"];
    assert!(!save_refuses(&doc_with(fields, "", &state, ""), &["Address: 1 Smith St, Sydney, New South Wales 2000"]));
    let birth = ["<< /FT /Tx /T (dob) /TU (Date of birth) >>"];
    assert!(!save_refuses(&doc_with(fields, "", &birth, ""), &["Date of birth: 01/02/1980"]));
    let filled = ["<< /FT /Tx /T (dob) /TU (Date of birth) /V (01/02/1980) >>"];
    assert!(save_refuses(&doc_with(fields, "", &filled, ""), &["Date of birth: 01/02/1980"]));
    let named = ["<< /FT /Ch /T (project) /Opt [(Project Bluefin) (Other)] >>"];
    assert!(save_refuses(&doc_with(fields, "", &named, ""), &["Project Bluefin"]));
}

/// A label's one-word value is not a run of its own (delta 3 review S1), unless it has a digit or
/// an '@'.
#[test]
fn a_labels_one_word_value_refuses_only_with_a_digit_or_at() {
    let link = |uri: &str| format!("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A << /S /URI /URI ({uri}) >> >>");
    let doc = |uri: &str| doc_with("", "/Annots [4 0 R]", &[link(uri).as_str()], "");
    assert!(!save_refuses(&doc("https://australia.gov.au/"), &["Country: Australia"]));
    assert!(save_refuses(&doc("https://example.com/tfn/123456789"), &["TFN: 123456789"]));
}
