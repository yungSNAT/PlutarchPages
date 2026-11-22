// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

#[test]
fn audit_reports_an_earlier_revision_and_metadata_counts() {
    use pdfcraft_cos::{ObjRef, Object, PdfString, SaveOptions, write_incremental};
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R >>");
    b.add("<< /Type /Pages /Kids [] /Count 0 >>");
    b.add_stream_with(b"<synthetic/>", "/Type /Metadata /Subtype /XML");
    b.add("<< /Author (Synthetic Author) >>");
    let mut doc = open(b.build());
    assert!(crate::audit_document(&doc).unwrap().document_findings.iter().all(|f| f.kind != "earlier-revisions"));
    doc.update_dict(ObjRef::new(1, 0), |d| d.set(b"Lang".to_vec(), Object::String(PdfString::literal("en")))).unwrap();
    doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(ObjRef::new(4, 0)));
    let doc = open(write_incremental(&doc, &SaveOptions::default()).unwrap());
    let report = crate::audit_document(&doc).unwrap();
    assert!(report.page_findings.is_empty());
    let kinds: Vec<_> = report.document_findings.iter().map(|f| (f.kind, f.count)).collect();
    assert_eq!(kinds, [("earlier-revisions", 1), ("document-info", 1), ("xmp-metadata", 1)]);
}

#[test]
fn null_metadata_does_not_suppress_page_findings() {
    use pdfcraft_cos::Object;
    let bytes = one_page(b"BT /F1 20 Tf 100 700 Td (SECRET) Tj ET\n0 g 95 690 120 30 re f", "");
    for indirect in [false, true] {
        let mut doc = open(bytes.clone());
        let value = if indirect { Object::Ref(doc.add(Object::Null)) } else { Object::Null };
        doc.trailer_mut().set(b"Info".to_vec(), value.clone());
        let root = doc.root().unwrap();
        doc.update_dict(root, |d| d.set(b"Metadata".to_vec(), value)).unwrap();

        let report = crate::audit_document(&doc).unwrap();
        assert!(report.document_findings.is_empty());
        assert_eq!(report.page_findings.len(), 1);
        assert_eq!(report.page_findings[0].kind, FindingKind::CoveredText);
    }
}

#[test]
fn malformed_metadata_is_reported_and_the_pages_are_still_audited() {
    use pdfcraft_cos::{Object, PdfString};
    let bytes = one_page(b"BT /F1 20 Tf 100 700 Td (SECRET) Tj ET\n0 g 95 690 120 30 re f", "");
    for indirect in [false, true] {
        let mut doc = open(bytes.clone());
        let value = Object::String(PdfString::literal("not a dictionary or stream"));
        let value = if indirect { Object::Ref(doc.add(value)) } else { value };
        doc.trailer_mut().set(b"Info".to_vec(), value.clone());
        let root = doc.root().unwrap();
        doc.update_dict(root, |d| d.set(b"Metadata".to_vec(), value)).unwrap();

        let report = crate::audit_document(&doc).unwrap();
        let kinds: Vec<_> = report.document_findings.iter().map(|f| (f.kind, f.count)).collect();
        assert_eq!(kinds, [("malformed-document-info", 1), ("malformed-xmp-metadata", 1)]);
        assert_eq!(report.page_findings.len(), 1);
        assert_eq!(report.page_findings[0].kind, FindingKind::CoveredText);
    }
}

/// Streams nothing refers to that show text are counted as `unreferenced-text`, a stream or a form
/// alike: a redaction applied and saved without garbage collection leaves the page's old content
/// so. So is one of a type that can be content, or one the audit doesn't know, a tiling pattern or
/// a mistyped stream, and a CMap or an object stream, whose syntax shows no text, so claiming
/// either type must not hide it. One that shows no text, and an image, metadata or a sound,
/// however its data reads, are not; nor is any once the next save must write the document in
/// full, which keeps only what it refers to.
#[test]
fn unreferenced_streams_that_show_text_are_counted() {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>");
    b.add_stream(b"BT /F1 24 Tf 100 500 Td (PUBLIC) Tj ET");
    b.add(FONT);
    b.add_stream(b"BT /F1 24 Tf 100 700 Td (SECRET) Tj ET");
    b.add_stream_with(b"BT /F1 24 Tf 100 700 Td [(SECRET)] TJ ET", "/Type /XObject /Subtype /Form /BBox [0 0 612 792]");
    b.add_stream(b"0 g 100 688 220 32 re f");
    b.add_stream_with(b"BT (SECRET) Tj ET", "/Type /XObject /Subtype /Image /Width 17 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8");
    b.add_stream_with(b"BT (SECRET) Tj ET", "/Type /Metadata /Subtype /XML");
    b.add_stream_with(b"BT /F1 24 Tf (SECRET) Tj ET", "/Type /Pattern /PatternType 1 /BBox [0 0 10 10]");
    b.add_stream_with(b"BT /F1 24 Tf (SECRET) Tj ET", "/Type /Foo /Subtype /Bar");
    b.add_stream_with(b"BT (SECRET) Tj ET", "/Type /CMap /CMapName /Old /CIDSystemInfo << /Registry (A) /Ordering (B) /Supplement 0 >>");
    b.add_stream_with(b"1 0 BT (SECRET) Tj ET", "/Type /ObjStm /N 1 /First 4");
    b.add_stream_with(b"BT (SECRET) Tj ET", "/Type /Sound /R 8000");
    let mut doc = open(b.build());
    let kinds = |doc: &Document| -> Vec<(&'static str, usize)> {
        let report = crate::audit_document(doc).unwrap();
        assert!(report.page_findings.is_empty());
        report.document_findings.iter().map(|f| (f.kind, f.count)).collect()
    };
    assert_eq!(kinds(&doc), [("unreferenced-text", 6)]);
    doc.require_full_save();
    assert!(kinds(&doc).is_empty());
}

/// A font program is not content, however its bytes read: an unreferenced stream that a font
/// descriptor names as its `/FontFile` or `/FontFile2` is skipped, the descriptor an object of its
/// own or written out in place in a font; the same key in another dictionary names none. A
/// reference names one object by number and generation, so a stale one to an earlier generation
/// skips nothing. `/Length1` alone, which any stream can carry, does not make one a font program,
/// so a stream with it that shows text and that no descriptor names is counted, and one that can't
/// be decoded fails the audit.
#[test]
fn only_a_font_program_a_descriptor_names_is_skipped_as_unreferenced_text() {
    const DESCRIPTOR: &str =
        "/Type /FontDescriptor /FontName /Old /Flags 4 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80";
    // A page showing PUBLIC, then `objects` from object 6.
    let audit = |objects: &[(&str, &[u8])]| {
        let mut b = Builder::new();
        b.add("<< /Type /Catalog /Pages 2 0 R >>");
        b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
        b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>");
        b.add_stream(b"BT /F1 24 Tf 100 500 Td (PUBLIC) Tj ET");
        b.add(FONT);
        for (dict, data) in objects {
            if data.is_empty() {
                b.add(dict);
            } else {
                b.add_stream_with(data, dict);
            }
        }
        crate::audit_document(&open(b.build()))
    };
    let count = |objects: &[(&str, &[u8])]| {
        let report = audit(objects).unwrap();
        assert!(report.page_findings.is_empty());
        report.document_findings.iter().filter(|f| f.kind == "unreferenced-text").map(|f| f.count).sum::<usize>()
    };
    let text: &[u8] = b"BT /F1 24 Tf 100 700 Td (SECRET) Tj ET";
    let program: &[u8] = b"(SECRET) Tj";
    let named = format!("<< {DESCRIPTOR} /FontFile 8 0 R /FontFile2 9 0 R >>");
    assert_eq!(count(&[("/Length1 38", text), (&named, b""), ("/Length1 11 /Length2 0 /Length3 0", program), ("/Length1 11", program)]), 1);
    let in_place = format!("<< /Type /Font /Subtype /TrueType /BaseFont /Old /FontDescriptor << {DESCRIPTOR} /FontFile2 7 0 R >> >>");
    assert_eq!(count(&[(&in_place, b""), ("/Length1 11", program)]), 0);
    let stale = format!("<< {DESCRIPTOR} /FontFile 7 1 R >>");
    assert_eq!(count(&[(&stale, b""), ("/Length1 38", text)]), 1);
    // A `/FontFile2` key in a dictionary that is no font descriptor, here a form's resource name
    // for the text-bearing form it draws, names no font program.
    let form = "/Type /XObject /Subtype /Form /BBox [0 0 612 792]";
    let caller = format!("{form} /Resources << /XObject << /FontFile2 7 0 R >> >>");
    assert_eq!(count(&[(&caller, b"/FontFile2 Do"), (form, text)]), 1);
    // A font program has neither `/Type` nor `/Subtype`, so a stream with either is read even when
    // a descriptor names it, here a forged one that names a text form, 2 partly typed streams and
    // one whose `/Type` is null, which is still a `/Type` key.
    let forged = format!("<< {DESCRIPTOR} /FontFile 7 0 R /FontFile2 8 0 R >>");
    let forged_more = format!("<< {DESCRIPTOR} /FontFile 10 0 R /FontFile2 11 0 R >>");
    let typed =
        [(&forged[..], b"" as &[u8]), (form, text), ("/Type /XObject", text), (&forged_more, b""), ("/Subtype /Form", text), ("/Type null", text)];
    assert_eq!(count(&typed), 4);
    assert!(audit(&[("/Length1 5 /Filter /FlateDecode", b"xxxxx")]).is_err(), "an undecodable stream must fail the audit");
}

#[test]
fn audit_reports_a_covered_text_advance_only_after_the_displacement() {
    let text = "BT /F1 20 Tf 72 692 Td [-1000] TJ ET";
    let cover = "0 g 71 688 22 21 re f";
    let bytes = format!("{text}\n{cover}");
    let report = crate::audit_document(&open(one_page(bytes.as_bytes(), ""))).unwrap();
    let kinds: Vec<_> = report.page_findings.iter().map(|f| f.kind.clone()).collect();
    assert_eq!(kinds, [FindingKind::RetainedAdvance]);
    assert!(report.page_findings[0].covered_text.is_empty());
    let bytes = format!("{cover}\n{text}");
    let report = crate::audit_document(&open(one_page(bytes.as_bytes(), ""))).unwrap();
    assert!(report.page_findings.is_empty());
}

#[test]
fn translucent_cover_is_flagged() {
    // Text and a black rectangle both drawn at half opacity: the audit doesn't mix translucent
    // text with what lies under it, so the cover is judged by its colour alone.
    let content = b"/GS1 gs\nBT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0 g\n100 688 220 32 re f\n";
    let page_extra = "/Resources << /Font << /F1 5 0 R >> /ExtGState << /GS1 6 0 R >> >>";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R {page_extra} >>"));
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /ExtGState /ca 0.5 >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
}

/// Resources naming the font and the forms X and Y (objects 6 and 7) of [`page_with_forms`].
const FORM_RESOURCES: &str = "/Resources << /Font << /F1 5 0 R >> /XObject << /X 6 0 R /Y 7 0 R >> >>";
const FORM_SECRET: &str = "BT /F1 24 Tf 100 700 Td (SECRET) Tj ET";
const FORM_BOX: &str = "0 g 100 688 220 32 re f";

/// One page drawing `content` with [`FORM_RESOURCES`], and its forms X and Y: each the extra
/// entries of its dictionary and its content.
fn page_with_forms(content: &str, forms: [(&str, &str); 2]) -> Document {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R {FORM_RESOURCES} >>"));
    b.add_stream(content.as_bytes());
    b.add(FONT);
    for (extra, body) in forms {
        b.add_stream_with(body.as_bytes(), &format!("/Type /XObject /Subtype /Form /BBox [0 0 612 792] {extra}"));
    }
    open(b.build())
}

/// The text of each covered-text finding.
fn covered(doc: &Document) -> Vec<String> {
    let findings = audit_redactions(doc).expect("audit must run");
    assert!(findings.iter().all(|f| f.kind == FindingKind::CoveredText), "{findings:?}");
    findings.into_iter().map(|f| f.covered_text).collect()
}

/// R2461: text a form XObject draws is audited like the page's own. A cover hides it when painted
/// after it: by the page after the `Do`, or by the form after the text (a page that is one form).
#[test]
fn text_inside_a_form_is_audited() {
    let only = |content: &str, x: &str| page_with_forms(content, [(FORM_RESOURCES, x), (FORM_RESOURCES, "")]);
    assert_eq!(covered(&only(&format!("/X Do {FORM_BOX}"), FORM_SECRET)), ["SECRET"]);
    assert_eq!(covered(&only("/X Do", &format!("{FORM_SECRET} {FORM_BOX}"))), ["SECRET"]);
    // Painted before the text, the box lies under it.
    assert!(covered(&only(&format!("{FORM_BOX} /X Do"), FORM_SECRET)).is_empty());
    assert!(covered(&only("/X Do", &format!("{FORM_BOX} {FORM_SECRET}"))).is_empty());
}

/// Nested forms: X draws Y, which draws the text, with X's `/Matrix` moving it 100 points down.
#[test]
fn text_inside_nested_forms_is_audited() {
    let nested = |content: &str, x: &str| {
        page_with_forms(content, [(&format!("{FORM_RESOURCES} /Matrix [1 0 0 1 0 -100]"), x), (FORM_RESOURCES, FORM_SECRET)])
    };
    assert_eq!(covered(&nested("/X Do 0 g 100 588 220 32 re f", "/Y Do")), ["SECRET"]);
    // Where the text would be without X's matrix, the box hides nothing.
    assert!(covered(&nested(&format!("/X Do {FORM_BOX}"), "/Y Do")).is_empty());
    // A cover X paints after Y hides Y's text; one painted before it doesn't.
    assert_eq!(covered(&nested("/X Do", &format!("/Y Do {FORM_BOX}"))), ["SECRET"]);
    assert!(covered(&nested("/X Do", &format!("{FORM_BOX} /Y Do"))).is_empty());
}

/// A form whose text the audit can't read in full fails it, quickly, rather than report the page
/// clean: one that draws itself, one that doesn't decode, and a page drawing forms past the
/// reader's visit limit. The cover walk, which goes first, stops each. X has no `/Resources`, so
/// it is read with the page's, which name X itself and Y.
#[test]
fn form_text_left_unread_fails_the_audit() {
    use crate::AuditError::{Cos, TooDeep, TooManyForms};
    let start = std::time::Instant::now();
    let many = format!("{}{FORM_BOX}", "/Y Do ".repeat(pdfcraft_edit::MAX_FORM_VISITS + 1));
    let undecodable = ("/Filter /FlateDecode", "not deflated");
    let result = |content: &str, x, y| audit_redactions(&page_with_forms(content, [x, y]));
    let drawn_inside_itself = result(&format!("/X Do {FORM_BOX}"), ("", "BT /F1 24 Tf 100 700 Td (SECRET) Tj ET /X Do"), ("", ""));
    assert!(matches!(drawn_inside_itself, Err(TooDeep)), "{drawn_inside_itself:?}");
    let undecodable = result(&format!("/X Do {FORM_BOX}"), ("", "BT /F1 24 Tf 100 700 Td (SECRET) Tj ET /Y Do"), undecodable);
    assert!(matches!(undecodable, Err(Cos(pdfcraft_cos::CosError::Filter(_)))), "{undecodable:?}");
    let too_many = result(&many, (FORM_RESOURCES, ""), (FORM_RESOURCES, ""));
    assert!(matches!(too_many, Err(TooManyForms)), "{too_many:?}");
    assert!(start.elapsed() < std::time::Duration::from_secs(10), "took {:?}", start.elapsed());
}

/// The forms a page draws share its decode budget with its content, so text in a form past it
/// fails the audit rather than go unread: after a mebibyte of spaces the page draws Y, another
/// mebibyte, `n` times, then X, the text, and covers it. 255 draws fit a budget of the forms'
/// own, not the page's.
#[test]
fn form_text_past_the_page_decode_budget_fails_the_audit() {
    let mebibyte = " ".repeat(1 << 20);
    let doc = |n: usize| {
        let content = format!("{mebibyte}{}/X Do {FORM_BOX}", "/Y Do ".repeat(n));
        page_with_forms(&content, [("", FORM_SECRET), ("", &mebibyte)])
    };
    assert_eq!(covered(&doc(253)), ["SECRET"], "within the budget, X is read");
    let result = audit_redactions(&doc(255));
    assert!(matches!(result, Err(crate::AuditError::Cos(pdfcraft_cos::CosError::Filter(_)))), "{result:?}");
}

/// The forms a page draws share its 16 MiB text budget too: a `/PCSoft` marker's `/N` is charged
/// to it, 9 MB by the page, then `n` bytes by Y, before X draws the text a cover hides. Text past
/// the budget fails the audit; 8 MB in Y would fit a budget of its own.
#[test]
fn form_text_past_the_page_text_budget_fails_the_audit() {
    let soft = |n: usize| format!("/PCSoft << /N {n} >> BDC EMC ");
    let doc = |n: usize| page_with_forms(&format!("{}/Y Do /X Do {FORM_BOX}", soft(9_000_000)), [("", FORM_SECRET), ("", &soft(n))]);
    assert_eq!(covered(&doc(7_000_000)), ["SECRET"], "within the budget, X is read");
    let result = audit_redactions(&doc(8_000_000));
    assert!(matches!(&result, Err(crate::AuditError::Text(0, e)) if e.contains("too large")), "{result:?}");
}

/// A form without `/Resources` is read with those it was drawn with, by the cover walk as by the
/// text walk: X draws the text, then Y, which only the page's resources name, draws the box.
#[test]
fn cover_in_a_form_reached_through_inherited_resources_is_found() {
    let doc = |x: &str| page_with_forms("/X Do", [("", x), (FORM_RESOURCES, FORM_BOX)]);
    assert_eq!(covered(&doc(&format!("{FORM_SECRET} /Y Do"))), ["SECRET"]);
    assert!(covered(&doc(&format!("/Y Do {FORM_SECRET}"))).is_empty());
}

/// A form whose `/Subtype` is a reference to `/Form` has its text read too (see
/// `indirect_subtype_is_resolved` for its covers). Object 8 is the name.
#[test]
fn text_in_a_form_with_an_indirect_subtype_is_audited() {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R {FORM_RESOURCES} >>"));
    b.add_stream(format!("/X Do {FORM_BOX}").as_bytes());
    b.add(FONT);
    b.add_stream_with(FORM_SECRET.as_bytes(), &format!("/Type /XObject /Subtype 8 0 R /BBox [0 0 612 792] {FORM_RESOURCES}"));
    b.add_stream_with(b"", "/Type /XObject /Subtype /Form /BBox [0 0 612 792]");
    b.add("/Form");
    assert_eq!(covered(&open(b.build())), ["SECRET"]);
}

/// A forward `TJ` adjustment a form draws under a later cover is a retained advance too.
#[test]
fn retained_advance_inside_a_form_is_reported() {
    let advance = "BT /F1 20 Tf 72 692 Td [-1000] TJ ET";
    let kinds = |content: &str| {
        let doc = page_with_forms(content, [(FORM_RESOURCES, advance), (FORM_RESOURCES, "")]);
        crate::audit_document(&doc).expect("audit must run").page_findings.into_iter().map(|f| f.kind).collect::<Vec<_>>()
    };
    assert_eq!(kinds("/X Do 0 g 71 688 22 21 re f"), [FindingKind::RetainedAdvance]);
    assert!(kinds("0 g 71 688 22 21 re f /X Do").is_empty());
}

/// R2847: a form of more operators than the page's bound fails the audit before the text walk,
/// which holds a stream's operators all at once, reads it. Read first, X would be read whole and
/// then found drawing itself.
#[test]
fn an_oversized_form_fails_before_its_text_is_read() {
    let x = format!("{}/X Do", "n\n".repeat(2_000_001));
    let result = audit_redactions(&page_with_forms("/X Do", [(FORM_RESOURCES, &x), (FORM_RESOURCES, "")]));
    assert!(matches!(result, Err(crate::AuditError::TooManyOps)), "{result:?}");
}

/// R2847: audit errors name pages from 1, as a reader does; their fields keep the 0-based index.
#[test]
fn audit_errors_number_pages_from_1() {
    use crate::AuditError::{BadAppearance, Text, UncheckedText, UnreadForms};
    assert_eq!(Text(0, "x".into()).to_string(), "page 1: x");
    assert_eq!(UnreadForms(1).to_string(), "page 2: the text of a form XObject could not be read");
    assert_eq!(UncheckedText(2, "a tiling pattern").to_string(), "page 3: a tiling pattern shows text, which the audit does not read");
    assert_eq!(BadAppearance(3).to_string(), "page 4: an annotation's appearance has no usable /BBox");
}

/// Resources naming the font, for a page, an appearance or a pattern.
const F1: &str = "/Resources << /Font << /F1 5 0 R >> >>";

/// The body of a stream object for [`page_with`]: `entries` in its dictionary, then `data`.
fn stream(entries: &str, data: &str) -> String {
    format!("<< /Length {} {entries} >>\nstream\n{data}\nendstream", data.len())
}

/// An appearance form of `bbox` with [`F1`], drawing `data`.
fn appearance(bbox: &str, data: &str) -> String {
    stream(&format!("/Type /XObject /Subtype /Form /BBox [{bbox}] {F1}"), data)
}

/// One page drawing `content`, its dictionary ending with `page_extra`, and `objects` from object 6
/// on (5 is [`FONT`]).
fn page_with(page_extra: &str, content: &str, objects: &[String]) -> Document {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R {page_extra} >>"));
    b.add_stream(content.as_bytes());
    b.add(FONT);
    for o in objects {
        b.add(o);
    }
    open(b.build())
}

/// R2846: text an annotation's appearance draws is audited like the page's. The appearance's box,
/// 200 by 50, is mapped onto the annotation's, 400 by 100 at (100, 600), and a cover it paints
/// after the text hides the text.
#[test]
fn text_in_an_annotation_appearance_is_audited() {
    let doc = |data: &str| {
        let annot = "<< /Type /Annot /Subtype /FreeText /Rect [100 600 500 700] /AP << /N 6 0 R >> >>".to_string();
        page_with("/Annots [7 0 R]", "", &[appearance("0 0 200 50", data), annot])
    };
    let text = "BT /F1 24 Tf 5 15 Td (SECRET) Tj ET";
    let findings = audit_redactions(&doc(&format!("{text} 0 g 0 0 200 50 re f"))).unwrap();
    let found: Vec<_> = findings.iter().map(|f| (f.covered_text.as_str(), f.rect)).collect();
    assert_eq!(found, [("SECRET", [100.0, 600.0, 500.0, 700.0])]);
    assert!(covered(&doc(&format!("0 g 0 0 200 50 re f {text}"))).is_empty());
}

/// A cover an annotation's appearance paints hides the page's text, painted before it, unless
/// the annotation is Hidden.
#[test]
fn page_text_under_an_appearance_cover_is_found() {
    let doc = |extra: &str| {
        let ap = stream("/Type /XObject /Subtype /Form /BBox [0 0 220 32]", "0 g 0 0 220 32 re f");
        let annot = format!("<< /Type /Annot /Subtype /Stamp /Rect [100 688 320 720] /AP << /N 6 0 R >> {extra} >>");
        page_with(&format!("{F1} /Annots [7 0 R]"), FORM_SECRET, &[ap, annot])
    };
    assert_eq!(covered(&doc("")), ["SECRET"]);
    assert!(covered(&doc("/F 2")).is_empty());
}

/// Annotations are painted in `/Annots` order: a Square after the annotation whose appearance
/// draws the text hides it, one before doesn't, and a Hidden annotation's text isn't drawn.
#[test]
fn appearance_text_and_squares_follow_the_annotation_order() {
    let doc = |annots: &str, extra: &str| {
        let text = format!("<< /Type /Annot /Subtype /FreeText /Rect [0 0 612 792] /AP << /N 6 0 R >> {extra} >>");
        let square = "<< /Type /Annot /Subtype /Square /Rect [95 685 325 725] /IC [0 0 0] >>".to_string();
        page_with(annots, "", &[appearance("0 0 612 792", FORM_SECRET), text, square])
    };
    assert_eq!(covered(&doc("/Annots [7 0 R 8 0 R]", "")), ["SECRET"]);
    assert!(covered(&doc("/Annots [8 0 R 7 0 R]", "")).is_empty());
    assert!(covered(&doc("/Annots [7 0 R 8 0 R]", "/F 2")).is_empty());
}

/// Of an annotation's appearances for each state, the one `/AS` names is painted; without one,
/// none is.
#[test]
fn the_appearance_state_shown_is_audited() {
    let doc = |state: &str| {
        let annot = format!("<< /Type /Annot /Subtype /Widget /Rect [0 0 612 792] /AP << /N << /On 6 0 R /Off 7 0 R >> >> {state} >>");
        page_with("/Annots [8 0 R]", "", &[appearance("0 0 612 792", &format!("{FORM_SECRET} {FORM_BOX}")), appearance("0 0 612 792", ""), annot])
    };
    assert_eq!(covered(&doc("/AS /On")), ["SECRET"]);
    assert!(covered(&doc("/AS /Off")).is_empty());
    assert!(covered(&doc("")).is_empty());
}

/// A Square is checked by the box its appearance paints, one finding whatever its interior colour
/// says, even a white one over a black box. Text no box of its appearance lies over (a cloudy
/// border drawn with curves, beside a small box elsewhere) is checked by its interior colour.
#[test]
fn a_square_and_its_appearance_are_one_finding() {
    let doc = |colour: &str, data: &str| {
        let ap = stream("/Type /XObject /Subtype /Form /BBox [0 0 230 40]", data);
        let square = format!("<< /Type /Annot /Subtype /Square /Rect [95 685 325 725] {colour} /AP << /N 6 0 R >> >>");
        page_with(&format!("{F1} /Annots [7 0 R]"), FORM_SECRET, &[ap, square])
    };
    let boxed = "0 g 0 0 230 40 re f";
    assert_eq!(covered(&doc("/IC [0 0 0]", boxed)), ["SECRET"]);
    assert_eq!(covered(&doc("/IC [1 1 1]", boxed)), ["SECRET"]);
    assert_eq!(covered(&doc("", boxed)), ["SECRET"]);
    let cloudy = "0 g 0 0 m 0 40 230 40 230 0 c f";
    assert_eq!(covered(&doc("/IC [0 0 0]", cloudy)), ["SECRET"]);
    assert_eq!(covered(&doc("/IC [0 0 0]", &format!("{cloudy} 0 0 1 1 re f"))), ["SECRET"]);
    // A white box before the appearance's own text hides nothing: a curve painted over the text is
    // then checked by the interior colour.
    let painted_over = "1 g 0 0 230 40 re f 0 g BT /F1 24 Tf 5 15 Td (SECRET) Tj ET 0 0 m 0 40 230 40 230 0 c f";
    let ap = stream("/Type /XObject /Subtype /Form /BBox [0 0 230 40]", painted_over);
    let square = "<< /Type /Annot /Subtype /Square /Rect [95 685 325 725] /IC [0 0 0] /AP << /N 6 0 R >> >>".to_string();
    assert_eq!(covered(&page_with(&format!("{F1} /Annots [7 0 R]"), "", &[ap, square])), ["SECRET"]);
}

/// An appearance without resources of its own reads the page's, as a form does: a Type 3 glyph it
/// selects from them that shows text fails the audit.
#[test]
fn an_appearance_reads_the_page_resources_when_it_has_none() {
    let font = format!(
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 8 0 R >> \
         /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] {F1} >>"
    );
    let ap = stream("/Type /XObject /Subtype /Form /BBox [0 0 612 792]", "BT /T 12 Tf 100 700 Td (a) Tj ET");
    let annot = "<< /Type /Annot /Subtype /FreeText /Rect [0 0 612 792] /AP << /N 7 0 R >> >>".to_string();
    let glyph = stream("", "1000 0 d0 BT /F1 12 Tf (SECRET) Tj ET");
    let result = audit_redactions(&page_with("/Resources << /Font << /T 6 0 R /F1 5 0 R >> >> /Annots [9 0 R]", "", &[font, ap, glyph, annot]));
    assert!(matches!(result, Err(crate::AuditError::UncheckedText(0, "a Type 3 glyph"))), "{result:?}");
}

/// The text the page and its appearances show is held to one page's worth in all, though each is
/// read with a budget of its own: here one appearance of 6 MiB of text, listed 3 times.
#[test]
fn appearance_text_past_one_page_fails_the_audit() {
    let doc = |listed: usize| {
        let ap = appearance("0 0 612 792", &format!("BT /F1 1 Tf ({}) Tj ET", "X".repeat(6 << 20)));
        let annot = "<< /Type /Annot /Subtype /FreeText /Rect [0 0 612 792] /AP << /N 6 0 R >> >>".to_string();
        page_with(&format!("/Annots [{}]", "7 0 R ".repeat(listed)), "", &[ap, annot])
    };
    assert!(audit_redactions(&doc(2)).unwrap().is_empty());
    let result = audit_redactions(&doc(3));
    assert!(matches!(&result, Err(crate::AuditError::Text(0, e)) if e.contains("too large")), "{result:?}");
}

/// Every glyph procedure entry a Type 3 font lists counts towards the page's operator bound, streams
/// or not: fonts sharing one large table can't be read past it.
#[test]
fn type3_tables_past_the_operator_bound_fail_the_audit() {
    let table: String = (0..1_000).map(|i| format!("/g{i} null ")).collect();
    let fonts = 2_001;
    let font = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs 6 0 R >>";
    let mut objects = vec![format!("<< {table}>>")];
    objects.extend((0..fonts).map(|_| font.to_string()));
    let names: String = (0..fonts).map(|i| format!("/T{i} {} 0 R ", i + 7)).collect();
    let content: String = (0..fonts).map(|i| format!("/T{i} 12 Tf ")).collect();
    let result = audit_redactions(&page_with(&format!("/Resources << /Font << {names}>> >>"), &format!("BT {content}ET"), &objects));
    assert!(matches!(result, Err(crate::AuditError::TooManyOps)), "{result:?}");
}

/// A Redact mark's own appearance, a label here, is not text the mark leaves under it: the mark
/// over the page's text names that text alone, and one over no text is no finding.
#[test]
fn a_redact_mark_does_not_report_its_own_appearance() {
    let marks = |content: &str| {
        let ap = appearance("0 0 230 40", "BT /F1 12 Tf 5 15 Td (REDACTED) Tj ET");
        let mark = "<< /Type /Annot /Subtype /Redact /Rect [95 685 325 725] /AP << /N 6 0 R >> >>".to_string();
        let findings = audit_redactions(&page_with(&format!("{F1} /Annots [7 0 R]"), content, &[ap, mark])).expect("audit must run");
        findings.into_iter().map(|f| (f.kind, f.covered_text)).collect::<Vec<_>>()
    };
    assert_eq!(marks(FORM_SECRET), [(FindingKind::UnappliedMark, "SECRET".to_string())]);
    assert!(marks("").is_empty());
}

/// An appearance without a usable `/BBox` fails the audit: where it draws can't be worked out. One
/// whose box has no area shows nothing.
#[test]
fn an_appearance_without_a_box_fails_the_audit() {
    let doc = |bbox: &str| {
        let ap = stream(&format!("/Type /XObject /Subtype /Form {bbox} {F1}"), &format!("{FORM_SECRET} {FORM_BOX}"));
        let annot = "<< /Type /Annot /Subtype /FreeText /Rect [0 0 612 792] /AP << /N 6 0 R >> >>".to_string();
        page_with("/Annots [7 0 R]", "", &[ap, annot])
    };
    assert_eq!(covered(&doc("/BBox [0 0 612 792]")), ["SECRET"]);
    for bbox in ["", "/BBox [0 0 612]", "/BBox 5"] {
        let result = audit_redactions(&doc(bbox));
        assert!(matches!(result, Err(crate::AuditError::BadAppearance(0))), "{bbox}: {result:?}");
    }
    assert!(covered(&doc("/BBox [0 0 0 792]")).is_empty());
}

/// The appearances of a page's annotations share its form budget: one annotation listed past it
/// fails the audit rather than be read again and again.
#[test]
fn appearances_past_the_page_form_budget_fail_the_audit() {
    let annots = format!("/Annots [{}]", "7 0 R ".repeat(pdfcraft_cos::MAX_PAGE_FORMS + 1));
    let annot = "<< /Type /Annot /Subtype /FreeText /Rect [0 0 612 792] /AP << /N 6 0 R >> >>".to_string();
    let result = audit_redactions(&page_with(&annots, "", &[appearance("0 0 612 792", ""), annot]));
    assert!(matches!(result, Err(crate::AuditError::TooManyForms)), "{result:?}");
}

/// A forward `TJ` adjustment an appearance draws under a later cover is a retained advance, where
/// the appearance maps it on the page.
#[test]
fn retained_advance_inside_an_appearance_is_reported() {
    let kinds = |data: &str| {
        let annot = "<< /Type /Annot /Subtype /FreeText /Rect [100 600 200 650] /AP << /N 6 0 R >> >>".to_string();
        let doc = page_with("/Annots [7 0 R]", "", &[appearance("0 0 100 50", data), annot]);
        crate::audit_document(&doc).unwrap().page_findings.into_iter().map(|f| f.kind).collect::<Vec<_>>()
    };
    let advance = "BT /F1 20 Tf 2 4 Td [-1000] TJ ET";
    assert_eq!(kinds(&format!("{advance} 0 g 1 0 22 21 re f")), [FindingKind::RetainedAdvance]);
    assert!(kinds(&format!("0 g 1 0 22 21 re f {advance}")).is_empty());
}

/// A tiling pattern's cell that shows text, itself or through a form, fails the audit, which
/// doesn't place it on the page. A cell that only paints doesn't, nor does a shading pattern.
#[test]
fn text_in_a_tiling_pattern_fails_the_audit() {
    let tiling = "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 100 100] /XStep 100 /YStep 100";
    let shading =
        "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 1 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [1 1 1] /N 1 >> >>";
    let objects = [
        stream(&format!("{tiling} {F1}"), FORM_SECRET),
        stream(&format!("{tiling} /Resources << /XObject << /X 10 0 R >> >>"), "/X Do"),
        stream(tiling, "0 0 50 50 re f"),
        format!("<< /PatternType 2 /Shading {shading} >>"),
        appearance("0 0 100 100", FORM_SECRET),
    ];
    let resources = "/Resources << /Pattern << /P1 6 0 R /P2 7 0 R /P3 8 0 R /P4 9 0 R >> >>";
    let audit = |p: &str| audit_redactions(&page_with(resources, &format!("/Pattern cs /{p} scn 0 0 612 792 re f"), &objects));
    for p in ["P1", "P2"] {
        let result = audit(p);
        assert!(matches!(result, Err(crate::AuditError::UncheckedText(0, "a tiling pattern"))), "{p}: {result:?}");
    }
    for p in ["P3", "P4"] {
        assert!(audit(p).unwrap().is_empty(), "{p}");
    }
}

/// Patterns whose cells paint with the next nest no deeper than forms do.
#[test]
fn nested_patterns_past_the_depth_fail_the_audit() {
    let tiling = "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 100 100] /XStep 100 /YStep 100";
    let objects: Vec<String> =
        (6..46).map(|i| stream(&format!("{tiling} /Resources << /Pattern << /N {} 0 R >> >>", i + 1), "/Pattern cs /N scn 0 0 9 9 re f")).collect();
    let result = audit_redactions(&page_with("/Resources << /Pattern << /N 6 0 R >> >>", "/Pattern cs /N scn", &objects));
    assert!(matches!(result, Err(crate::AuditError::TooDeep)), "{result:?}");
}

/// A Type 3 font whose glyph procedure shows text fails the audit; one whose glyphs only paint
/// doesn't, and selecting it on every line walks its glyphs once: 600,000 times would pass the
/// page's operator bound.
#[test]
fn text_in_a_type3_glyph_fails_the_audit() {
    let font = |glyph: usize| {
        format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a {glyph} 0 R >> \
             /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] {F1} >>"
        )
    };
    let objects = [font(8), font(9), stream("", "1000 0 d0 BT /F1 12 Tf (SECRET) Tj ET"), stream("", "1000 0 0 0 1000 1000 d1 0 0 1000 1000 re f")];
    let audit = |content: &str| audit_redactions(&page_with("/Resources << /Font << /T 6 0 R /P 7 0 R >> >>", content, &objects));
    let result = audit("BT /T 12 Tf 100 700 Td (a) Tj ET");
    assert!(matches!(result, Err(crate::AuditError::UncheckedText(0, "a Type 3 glyph"))), "{result:?}");
    assert!(audit("BT /P 12 Tf 100 700 Td (a) Tj ET").unwrap().is_empty());
    assert!(audit(&format!("BT {}ET", "/P 12 Tf ".repeat(600_000))).unwrap().is_empty());
}

/// A Type 3 font's glyph procedures count against the page's form budget, as forms do.
#[test]
fn type3_glyphs_past_the_page_form_budget_fail_the_audit() {
    let procs: String = (0..=pdfcraft_cos::MAX_PAGE_FORMS).map(|i| format!("/g{i} 7 0 R ")).collect();
    let font = format!("<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << {procs}>> >>");
    let doc = page_with("/Resources << /Font << /T 6 0 R >> >>", "BT /T 12 Tf ET", &[font, stream("", "1000 0 d0")]);
    let result = audit_redactions(&doc);
    assert!(matches!(result, Err(crate::AuditError::TooManyForms)), "{result:?}");
}
