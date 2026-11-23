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
/// fewer than 21 operators lie between it and an image's real `EI`. So once the text after an
/// inline image is redacted away, the image could swallow the operators up to that `EI`.
#[test]
fn a_copied_stream_holding_ei_does_not_move_the_end_of_an_inline_image_before_it() {
    let first = b"q BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7F> EI 1 0 0 1 5 5 cm BT /F1 10 Tf 20 230 Td (SECRET) Tj ET";
    let ends = |content: &[u8]| {
        let parsed = pdfcraft_content::parse(content);
        let image = parsed.ops.iter().find_map(|o| o.inline.as_ref().map(|(_, data)| data.clone()));
        (parsed.ambiguous_images, image, parsed.ops.iter().filter(|o| o.is("cm") || o.is("Q")).count())
    };
    for tail in [&b"BX 1 FOOEI EX Q"[..], b"/BEI gs Q"] {
        let label = String::from_utf8_lossy(tail);
        let mut doc = pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R 5 0 R] /Resources << /Font << /F1 6 0 R >> >> >>".to_vec(),
            stream("", first),
            stream("", tail),
            FONT.replace("95 0 R", "7 0 R").into_bytes(),
            widths(),
        ]);
        assert_eq!(ends(content(&doc, 0).as_bytes()), (0, Some(b"7F>".to_vec()), 2), "the source, {label}");
        mark(&mut doc, 0, &[[0.0, 200.0, 300.0, 300.0]], "");
        apply(&mut doc, None).unwrap();
        let after = content(&doc, 0);
        assert!(!after.contains("SECRET"), "{label}");
        assert_eq!(ends(after.as_bytes()), (0, Some(b"7F>".to_vec()), 2), "redacted, {label}: {after}");
    }
}
