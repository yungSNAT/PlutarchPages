//! Tests build their own tiny PDFs in code (contributor-original fixtures): no binary files,
//! no Adobe material, exact xref offsets computed by the builder below.

use std::sync::Arc;

use pdfcraft_cos::Document;

use crate::{AuditFinding, FindingKind, audit_redactions};

/// Minimal PDF assembler: objects are numbered 1..=n in order.
struct Builder {
    objects: Vec<Vec<u8>>,
}

impl Builder {
    fn new() -> Self {
        Builder { objects: Vec::new() }
    }

    fn add(&mut self, body: &str) -> usize {
        self.objects.push(body.as_bytes().to_vec());
        self.objects.len()
    }

    fn add_stream(&mut self, data: &[u8]) -> usize {
        self.add_stream_with(data, "")
    }

    fn add_stream_with(&mut self, data: &[u8], dict_extra: &str) -> usize {
        let mut body = format!("<< /Length {} {dict_extra} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.objects.push(body);
        self.objects.len()
    }

    fn build(self) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, obj) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(obj);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref_at = out.len();
        let n = self.objects.len() + 1;
        out.extend_from_slice(format!("xref\n0 {n}\n").as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(b"trailer\n<< ");
        out.extend_from_slice(format!("/Size {n} /Root 1 0 R").as_bytes());
        out.extend_from_slice(b" >>\nstartxref\n");
        out.extend_from_slice(format!("{xref_at}\n").as_bytes());
        out.extend_from_slice(b"%%EOF\n");
        out
    }
}

const FONT: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";

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

/// One page with `content` as its stream; `page_extra` is appended inside the page dict.
fn one_page(content: &[u8], page_extra: &str) -> Vec<u8> {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    let stream_num = 4;
    b.add(&format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {stream_num} 0 R /Resources << /Font << /F1 5 0 R >> >> {page_extra} >>"
    ));
    b.add_stream(content);
    b.add(FONT);
    b.build()
}

/// One page with two content streams (`/Contents [4 0 R 5 0 R]`).
fn two_streams(s1: &[u8], s2: &[u8]) -> Vec<u8> {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents [4 0 R 5 0 R] /Resources << /Font << /F1 6 0 R >> >> >>");
    b.add_stream(s1);
    b.add_stream(s2);
    b.add(FONT);
    b.build()
}

/// One page whose content stream claims `/FlateDecode` but holds garbage: decoding must fail.
fn bad_filter_page() -> Vec<u8> {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>");
    b.add_stream_with(b"this is not deflated data", "/Filter /FlateDecode");
    b.add(FONT);
    b.build()
}

fn open(bytes: Vec<u8>) -> Document {
    Document::open(Arc::new(bytes)).expect("test pdf must open")
}

#[test]
fn faux_redaction_is_found() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0 g\n100 688 220 32 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    let f = &findings[0];
    assert_eq!(f.page, 0);
    assert_eq!(f.kind, FindingKind::CoveredText);
    assert!(f.covered_text.contains("SECRET"), "covered_text was {:?}", f.covered_text);
}

#[test]
fn identity_matrix_between_rectangle_and_fill_still_covers_text() {
    // A no-op `cm` inside the path must not drop the pending rectangle (#1333).
    for cover in [&b"0 g\n100 688 220 32 re 1 0 0 1 0 0 cm f\n"[..], b"0 g\n1 0 0 1 0 0 cm 100 688 220 32 re f\n"] {
        let mut content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n".to_vec();
        content.extend_from_slice(cover);
        let findings = audit_redactions(&open(one_page(&content, ""))).expect("audit must run");
        assert_eq!(findings.len(), 1, "{:?}: {findings:?}", String::from_utf8_lossy(cover));
        assert_eq!(findings[0].kind, FindingKind::CoveredText);
        assert_eq!(findings[0].rect, [100.0, 688.0, 320.0, 720.0]);
        assert_eq!(findings[0].covered_text, "SECRET DATA 123");
    }
}

#[test]
fn clean_document_has_no_findings() {
    let content = b"BT /F1 24 Tf 100 700 Td (nothing hidden here) Tj ET\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn black_box_without_text_is_not_a_finding() {
    // A filled rectangle with no text under it is decoration, not a faux redaction.
    let content = b"BT /F1 24 Tf 100 100 Td (far away) Tj ET\n0 g\n400 600 100 40 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn translucent_cover_is_not_flagged() {
    // A rectangle drawn with a translucent ExtGState does not hide the text.
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
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn unapplied_redact_mark_is_found() {
    let content = b"BT /F1 24 Tf 100 700 Td (DO NOT SHARE) Tj ET\n";
    let page_extra = "/Annots [6 0 R]";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> {page_extra} >>"));
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype /Redact /Rect [95 685 305 725] /C [1 0 0] >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::UnappliedMark);
    assert!(findings[0].covered_text.contains("DO NOT SHARE"));
}

/// #2662: a box over one number of a long line hides a few percent of the line, and all of the
/// number. Found as a cover, and as an unapplied mark over the same word.
#[test]
fn a_cover_over_one_word_of_a_long_line_is_found() {
    // Helvetica 11: "The contract value is " ends near x 132, "98765" spans about 132-163.
    let line = b"BT /F1 11 Tf 30 100 Td (The contract value is 98765 and the rest of this long line continues here.) Tj ET
";
    let mut content = line.to_vec();
    content.extend_from_slice(
        b"0 g
131 96 33 14 re f
",
    );
    let findings = audit_redactions(&open(one_page(&content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::CoveredText);
    assert!(findings[0].covered_text.contains("98765"), "{findings:?}");

    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>");
    b.add_stream(line);
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype /Redact /Rect [131 96 164 110] >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::UnappliedMark);
}

/// A box that only grazes the words beside it (less than half of each) is not a cover.
#[test]
fn a_box_grazing_a_word_is_not_flagged() {
    let content = b"BT /F1 11 Tf 30 100 Td (The contract value is 98765 and the rest of this long line continues here.) Tj ET
0 g
131 96 6 14 re f
";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn opaque_filled_square_annotation_over_text_is_found() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>");
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype /Square /Rect [95 685 325 725] /C [0 0 0] /IC [0 0 0] >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::CoveredText);
    assert_eq!(findings[0].rect, [95.0, 685.0, 325.0, 725.0]);
    assert_eq!(findings[0].covered_text, "SECRET DATA 123");
}

/// A Hidden Square (`/F 2`) is never drawn, so the text under it is not a finding.
#[test]
fn hidden_square_annotation_is_not_a_finding() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>");
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype /Square /Rect [95 685 325 725] /C [0 0 0] /IC [0 0 0] /F 2 >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

/// A NoView Square (`/F 32`) is never drawn on screen, so the text under it is not a finding.
#[test]
fn no_view_square_annotation_is_not_a_finding() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>");
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype /Square /Rect [95 685 325 725] /C [0 0 0] /IC [0 0 0] /F 32 >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

/// A Print-only Square (`/F 4`) is still drawn on screen, so the text under it is a finding.
#[test]
fn print_flag_square_annotation_is_still_a_finding() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>");
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype /Square /Rect [95 685 325 725] /C [0 0 0] /IC [0 0 0] /F 4 >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::CoveredText);
}

#[test]
fn background_before_text_is_not_flagged() {
    // An opaque dark rectangle painted *before* the text is a background, not a cover.
    let content = b"0 g\n100 688 220 32 re f\nBT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn light_shading_after_text_is_not_flagged() {
    // Light table-style shading painted after the text leaves it readable: not a cover.
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0.9 g\n100 688 220 32 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn graphics_state_carries_across_streams() {
    // The first stream sets a white fill after the text; the second stream's rectangle over
    // the text is therefore white, not the default black: no finding. Without the carry fix
    // the rectangle would default to black and flag.
    let s1 = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n1 g\n";
    let s2 = b"100 688 220 32 re f\n";
    let findings = audit_redactions(&open(two_streams(s1, s2))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn audit_fails_closed_on_undecodable_stream() {
    // A content stream that cannot be decoded is an audit error, never an empty finding list.
    let err = audit_redactions(&open(bad_filter_page())).expect_err("audit must fail closed");
    let msg = err.to_string();
    assert!(msg.contains("decoded"), "unexpected error: {msg}");
}

/// A form XObject that draws itself nests without end. The walk can't follow it to the bottom, so
/// the audit fails rather than report the page clean with a possible cover left unseen.
#[test]
fn self_drawing_form_fails_the_audit() {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 6 0 R >> /XObject << /X 5 0 R >> >> >>");
    b.add_stream(b"BT /F1 24 Tf 100 700 Td (SECRET) Tj ET\n/X Do\n");
    b.add_stream_with(
        b"0 g\n100 688 220 32 re f\n/X Do\n",
        "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /XObject << /X 5 0 R >> >>",
    );
    b.add(FONT);
    let result = audit_redactions(&open(b.build()));
    assert!(matches!(result, Err(crate::AuditError::TooDeep)), "{result:?}");
}

/// An applied redaction leaves the words around it and a `TJ` adjustment over the gap where the
/// text was, then the redaction box. At the end of a line, the box covers most of the line's box
/// (which spans the gap) but none of its words: nothing is hidden, so nothing is flagged.
#[test]
fn applied_redaction_at_the_end_of_a_line_is_not_flagged() {
    let content = b"BT /F1 11 Tf 72 742 Td [(Mail the team:) -11354] TJ ET\n0 g\n144.138 739.8 121.814 11 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
    // The same gap between two words, wider than the words beside it.
    let content = b"BT /F1 11 Tf 72 742 Td [(Card:) -14000 (ok)] TJ ET\n0 g\n101 739.8 150 11 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

/// The control: the same box over text still drawn under it is a faux redaction, at the end of a
/// line as in the middle of a short one.
#[test]
fn faux_redaction_at_the_end_of_a_line_is_found() {
    let content = b"BT /F1 11 Tf 72 742 Td (Mail the team: someone@example.com) Tj ET\n0 g\n144.138 739.8 121.814 11 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].covered_text, "Mail the team: someone@example.com");
    let content = b"BT /F1 11 Tf 72 742 Td [(Card:) -300 (4111111111111111) -300 (ok)] TJ ET\n0 g\n101 739.8 100 11 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
}

/// A cover over a short value after a long label hides less than 40% of the line's words, but the
/// whole of the words it covers, and most of the line's box: still a faux redaction. The value
/// follows the label in the same `TJ` after a wide gap, or in a second run moved by `Td`.
#[test]
fn faux_redaction_over_a_short_value_after_a_long_label_is_found() {
    let content = b"BT /F1 11 Tf 72 742 Td [(Social insurance number:) -8000 (123 456 789)] TJ ET\n0 g\n200 739.8 150 11 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].covered_text, "Social insurance number: 123 456 789");
    let content = b"BT /F1 11 Tf 72 742 Td (Account:) Tj 70 0 Td (4111) Tj ET\n0 g\n115 739.8 55 11 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].covered_text.contains("4111"), "{findings:?}");
}

/// Text set with a negative font size in a mirrored text matrix runs its glyphs backwards: a
/// cover over the address it draws is still found.
#[test]
fn faux_redaction_over_mirrored_text_is_found() {
    let content = b"BT /F1 -11 Tf -1 0 0 1 300 742 Tm (Mail the team: someone@example.com) Tj ET\n0 g\n365 733 130 14 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].covered_text, "Mail the team: someone@example.com");
}

/// A cover over less than half a value must retain the historical line-box detection.
#[test]
fn faux_redaction_over_a_partial_value_after_a_long_label_is_found() {
    partial_value_is_found(false);
}

#[test]
fn faux_redaction_over_a_partial_value_after_a_long_label_on_a_rotated_page_is_found() {
    partial_value_is_found(true);
}

fn partial_value_is_found(rotated: bool) {
    let bytes = one_page(
        b"BT /F1 11 Tf 72 742 Td [(Social insurance number:) -8000 (123456789)] TJ ET\n0 g\n193 738 111 16 re f\n",
        if rotated { "/Rotate 90" } else { "" },
    );
    let findings = audit_redactions(&open(bytes)).expect("audit must run");
    assert_eq!(findings.len(), 1, "partial value, rotated={rotated}");
}

/// Keep the historic detections with several covers, lines and text-spacing states.
#[test]
fn faux_redactions_with_multiple_covers_lines_and_spacing_are_found() {
    // The second box of the first case hides most of the number (less than 40% of the line): a
    // finding of its own since covers are also measured per word (#2662).
    let cases: [(&[u8], usize); 4] = [
        (b"BT /F1 11 Tf 72 742 Td (Account: 123456789) Tj ET\n0 g\n72 738 65 16 re f\n140 738 40 16 re f\n", 2),
        (b"BT /F1 11 Tf 72 742 Td (Account: 123456789) Tj 0 -15 Td (Account: 987654321) Tj ET\n0 g\n115 723 100 30 re f\n", 1),
        (b"BT /F1 11 Tf 8 Tc 72 742 Td (Account: 1234) Tj ET\n0 g\n145 738 90 16 re f\n", 1),
        (b"BT /F1 11 Tf 250 Tz 72 742 Td (Account: 1234) Tj ET\n0 g\n100 738 160 16 re f\n", 1),
    ];
    for (content, expected) in cases {
        let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
        assert_eq!(findings.len(), expected, "{}: {findings:?}", String::from_utf8_lossy(content));
    }
}

#[test]
fn rectangle_drawn_as_a_path_covers_text_like_re() {
    // The same cover as `re`, built from `m`/`l` (closed with `h`, a repeated start point, or
    // only by the fill itself), or in a scaled user space: all are faux redactions (#1510).
    // So are `re` with a redundant `h`, and either path painted with `b`/`b*` (#1817).
    let covers: [&[u8]; 10] = [
        b"0 g\n100 688 220 32 re f\n",
        b"0 g\n100 688 220 32 re h f\n",
        b"0 g\n100 688 220 32 re b\n",
        b"0 g\n100 688 220 32 re h b*\n",
        b"0 g\n100 688 m 320 688 l 320 720 l 100 720 l b\n",
        b"0 g\n100 688 m 320 688 l 320 720 l 100 720 l h b*\n",
        b"0 g\n100 688 m 320 688 l 320 720 l 100 720 l h f\n",
        b"0 g\n100 688 m 320 688 l 320 720 l 100 720 l 100 688 l h f\n",
        b"0 g\n100 688 m 100 720 l 320 720 l 320 688 l f\n",
        b"0 g\nq 2 0 0 2 0 0 cm 50 344 m 160 344 l 160 360 l 50 360 l h f Q\n",
    ];
    for cover in covers {
        let mut content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n".to_vec();
        content.extend_from_slice(cover);
        let findings = audit_redactions(&open(one_page(&content, ""))).expect("audit must run");
        assert_eq!(findings.len(), 1, "{:?}: {findings:?}", String::from_utf8_lossy(cover));
        assert_eq!(findings[0].kind, FindingKind::CoveredText);
        assert_eq!(findings[0].rect, [100.0, 688.0, 320.0, 720.0]);
        assert_eq!(findings[0].covered_text, "SECRET DATA 123");
    }
}

#[test]
fn non_rectangular_path_is_not_a_cover() {
    // Each path's bounding box covers the text, but the shape itself does not hide it all.
    let covers: [&[u8]; 7] = [
        // A triangle.
        b"0 g\n100 688 m 320 688 l 320 720 l h f\n",
        // A triangle painted with `b`, and a trapezoid with `b*`.
        b"0 g\n100 688 m 320 688 l 320 720 l b\n",
        b"0 g\n100 688 m 320 688 l 320 720 l 140 720 l b*\n",
        // A triangle, then a line from its start point after `h`: not a fourth corner.
        b"0 g\n100 688 m 320 688 l 320 720 l h 100 720 l f\n",
        // A trapezoid: four corners, one slanted side.
        b"0 g\n100 688 m 320 688 l 320 720 l 140 720 l h f\n",
        // A rectangle rotated 5 degrees about the text by the CTM.
        b"0 g\nq 0.9962 0.0872 -0.0872 0.9962 62.15 -15.63 cm 100 688 m 320 688 l 320 720 l 100 720 l h f Q\n",
        // Two subpaths filled together: even-odd leaves the inner one as a hole.
        b"0 g\n90 680 m 330 680 l 330 730 l 90 730 l h 100 688 m 320 688 l 320 720 l 100 720 l h f*\n",
    ];
    for cover in covers {
        let mut content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n".to_vec();
        content.extend_from_slice(cover);
        let findings = audit_redactions(&open(one_page(&content, ""))).expect("audit must run");
        assert!(findings.is_empty(), "{:?}: {findings:?}", String::from_utf8_lossy(cover));
    }
}

#[test]
fn finding_kind_ids_are_stable() {
    // The string IDs are the crate's public vocabulary (CLI output, future machine-readable
    // formats). They must not change; add a variant instead of renaming.
    assert_eq!(FindingKind::CoveredText.id(), "covered-text");
    assert_eq!(FindingKind::UnappliedMark.id(), "unapplied-mark");
}

#[test]
fn covered_text_finding_is_fully_determined() {
    // Golden test: the whole finding, not just its presence, is pinned.
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0 g\n100 688 220 32 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(
        findings,
        vec![AuditFinding {
            page: 0,
            kind: FindingKind::CoveredText,
            rect: [100.0, 688.0, 320.0, 720.0],
            covered_text: "SECRET DATA 123".to_string(),
        }]
    );
}

#[test]
fn unapplied_mark_finding_is_fully_determined() {
    // Golden test for the second finding class, with its exact mark rectangle.
    let content = b"BT /F1 24 Tf 100 700 Td (DO NOT SHARE) Tj ET\n";
    let page_extra = "/Annots [6 0 R]";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> {page_extra} >>"));
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype /Redact /Rect [95 685 305 725] /C [1 0 0] >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(
        findings,
        vec![AuditFinding { page: 0, kind: FindingKind::UnappliedMark, rect: [95.0, 685.0, 305.0, 725.0], covered_text: "DO NOT SHARE".to_string() }]
    );
}

/// A page whose content draws a chain of `n` nested form XObjects. The innermost form draws
/// nothing, so a successful audit reports no findings.
fn nested_forms(n: usize) -> Vec<u8> {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    // Object 3: page; object 4: content; objects 5..5+n: the form chain.
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /XObject << /F1 5 0 R >> >> >>");
    b.add_stream(b"/F1 Do");
    for i in 1..=n {
        let obj = 4 + i;
        if i < n {
            let next = obj + 1;
            let name = format!("/F{}", i + 1);
            b.add_stream_with(
                format!("/{name} Do", name = &name[1..]).as_bytes(),
                &format!(
                    "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /XObject << /F{i} {next} 0 R >> >>",
                    i = i + 1,
                    next = next
                ),
            );
        } else {
            b.add_stream_with(b"q Q", "/Type /XObject /Subtype /Form /BBox [0 0 612 792]");
        }
        debug_assert_eq!(b.objects.len(), obj);
    }
    b.build()
}

#[test]
fn forms_nested_at_depth_limit_audit_clean() {
    // 32 nested forms is the deepest walk the audit follows: it must succeed and report
    // the page clean rather than fail.
    let findings = audit_redactions(&open(nested_forms(32))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn forms_nested_past_depth_limit_fail_closed() {
    // The 33rd nesting level cannot be followed: the audit fails rather than report the
    // page clean with a possible cover left unseen.
    let result = audit_redactions(&open(nested_forms(33)));
    assert!(matches!(result, Err(crate::AuditError::TooDeep)), "{result:?}");
}

#[test]
fn comment_across_streams_neither_invents_text_nor_a_cover() {
    let doc = open(two_streams(b"BT /F1 24 Tf 72 700 Td (Visible text) Tj ET % note", b"0 g 70 690 200 40 re f"));
    assert!(audit_redactions(&doc).unwrap().is_empty(), "commented cover must not be reported");
    let doc = open(two_streams(b"BT /F1 24 Tf 72 700 Td (Before comment) Tj ET % note", b"BT /F1 24 Tf 72 650 Td (After comment) Tj ET"));
    let text: String = pdfcraft_edit::text_lines(&doc, 0).unwrap().into_iter().map(|line| line.text).collect();
    assert!(!text.contains("After comment"), "editor exposed phantom text: {text}");
}

#[test]
fn reading_blocks_respect_stream_comments_and_split_operands() {
    let doc = open(two_streams(b"BT /F1 24 Tf 72 700 Td (Before comment) Tj ET % note", b"BT /F1 24 Tf 72 650 Td (After comment) Tj ET"));
    let text: String = pdfcraft_edit::reading_blocks(&doc, 0).unwrap().into_iter().map(|block| block.editor_text).collect();
    assert_eq!(text, "Before comment");
    let doc = open(two_streams(b"BT /F1 24 Tf 72 700 Td (Split operand)", b"Tj ET"));
    let text: String = pdfcraft_edit::reading_blocks(&doc, 0).unwrap().into_iter().map(|block| block.editor_text).collect();
    assert_eq!(text, "Split operand");
    // Marked content opened in one stream applies to text in the next: its /ActualText replaces
    // the glyphs, as when the streams are read joined.
    for (s1, s2) in [
        (b"BT /F1 24 Tf 72 700 Td /Span <</ActualText (Replacement text)".as_slice(), b">> BDC (Glyphs) Tj EMC ET".as_slice()),
        (b"BT /F1 24 Tf 72 700 Td /Span <</ActualText (Replacement text)>> BDC", b"(Glyphs) Tj EMC ET"),
    ] {
        let doc = open(two_streams(s1, s2));
        let text: String = pdfcraft_edit::reading_blocks(&doc, 0).unwrap().into_iter().map(|block| block.editor_text).collect();
        assert_eq!(text, "Replacement text", "{}", String::from_utf8_lossy(s1));
    }
}

/// The current path is not graphics state: a rectangle built before `Q` and filled after it
/// paints the box, so it is a cover.
#[test]
fn rectangle_filled_after_restore_is_a_cover() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0 g\nq 100 688 220 32 re Q f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].covered_text.contains("SECRET"), "{findings:?}");
}

/// A `/Subtype` may be an indirect reference to the name, like any other value: a `/Redact`
/// annotation and a covering form XObject named that way are still checked.
#[test]
fn indirect_subtype_is_resolved() {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>");
    b.add_stream(b"BT /F1 24 Tf 100 700 Td (DO NOT SHARE) Tj ET\n");
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype 7 0 R /Rect [95 685 305 725] >>");
    b.add("/Redact");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::UnappliedMark);

    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 6 0 R >> /XObject << /X 5 0 R >> >> >>");
    b.add_stream(b"BT /F1 24 Tf 100 700 Td (SECRET) Tj ET\n/X Do\n");
    b.add_stream_with(b"0 g\n100 688 220 32 re f\n", "/Type /XObject /Subtype 7 0 R /BBox [0 0 612 792]");
    b.add(FONT);
    b.add("/Form");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::CoveredText);
}

/// Text drawn after a cover on the same baseline joins the covered text in one line. The cover
/// still hides the words painted before it.
#[test]
fn later_text_on_the_line_does_not_clear_an_earlier_cover() {
    let content = b"0 g\nBT /F1 24 Tf 100 700 Td (SECRET) Tj ET\n98 688 94 32 re f\nBT /F1 24 Tf 200 700 Td (tail) Tj ET\n";
    let doc = open(one_page(content, ""));
    let lines = pdfcraft_edit::text_lines(&doc, 0).expect("text");
    assert_eq!(lines.len(), 1, "the runs join one line: {:?}", lines.iter().map(|l| &l.text).collect::<Vec<_>>());
    let findings = audit_redactions(&doc).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].covered_text.contains("SECRET"), "{findings:?}");
}

/// #4164: a cover over part of a line reports the words under it, not the words of the same
/// line painted beside it, which stay visible.
#[test]
fn covered_text_leaves_out_the_words_beside_the_cover() {
    let covered = |content: &[u8]| {
        let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
        findings.into_iter().map(|f| f.covered_text).collect::<Vec<_>>()
    };
    assert_eq!(covered(b"0 g\nBT /F1 24 Tf 100 700 Td (SECRET) Tj ET\n98 688 94 32 re f\nBT /F1 24 Tf 200 700 Td (tail) Tj ET\n"), ["SECRET"]);
    // A word shown in two runs with nothing between them stays one word (review on #4198).
    assert_eq!(covered(b"0 g\nBT /F1 24 Tf 100 700 Td (SE) Tj (CRET) Tj ET\n98 688 94 32 re f\nBT /F1 24 Tf 200 700 Td (tail) Tj ET\n"), ["SECRET"]);

    // The same for an unapplied redaction mark over the first run only. A mark over both runs
    // reports both, with the space the line has between them (`tail` is set apart here).
    for (rect, expected) in [("[98 688 192 720]", "SECRET"), ("[98 688 310 720]", "SECRET tail")] {
        let mut b = Builder::new();
        b.add("<< /Type /Catalog /Pages 2 0 R >>");
        b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
        b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>");
        b.add_stream(b"BT /F1 24 Tf 100 700 Td (SECRET) Tj ET\nBT /F1 24 Tf 250 700 Td (tail) Tj ET\n");
        b.add(FONT);
        b.add(&format!("<< /Type /Annot /Subtype /Redact /Rect {rect} >>"));
        let findings = audit_redactions(&open(b.build())).expect("audit must run");
        assert_eq!(findings.iter().map(|f| (f.kind.clone(), f.covered_text.as_str())).collect::<Vec<_>>(), [(FindingKind::UnappliedMark, expected)]);
    }
}

/// Many covers over the same long line repeat its text in every finding: a small file must not
/// expand into an unbounded report. Past the budget the audit fails rather than truncate.
#[test]
fn covered_text_across_findings_is_bounded() {
    let text = "X".repeat(60_000);
    let mut content = format!("BT /F1 1 Tf 0 700 Td ({text}) Tj ET\n0 g\n").into_bytes();
    for i in 0..1_200 {
        content.extend_from_slice(format!("-{i} 690 50000 20 re f\n").as_bytes());
    }
    let result = audit_redactions(&open(one_page(&content, "")));
    assert!(matches!(result, Err(crate::AuditError::TooMuchOutput)), "{:?}", result.map(|f| f.len()));
}

/// Covers compared against lines are bounded per page, so content that is quadratic in its
/// covers and lines fails the audit instead of running for hours. A line costs one comparison per
/// word: 20 covers against 20 two-word lines are 800.
#[test]
fn cover_and_line_comparisons_are_bounded() {
    let mut content = Vec::new();
    for i in 0..20 {
        content.extend_from_slice(format!("BT /F1 12 Tf 72 {} Td (Line {i}) Tj ET\n", 700 - 14 * i).as_bytes());
    }
    for _ in 0..20 {
        content.extend_from_slice(b"1 g 0 0 10 10 re f\n");
    }
    let doc = open(one_page(&content, ""));
    let limits = crate::Limits { comparisons: 799, covered_text: usize::MAX };
    assert!(matches!(crate::audit(&doc, limits), Err(crate::AuditError::TooManyComparisons)));
    let limits = crate::Limits { comparisons: 800, covered_text: usize::MAX };
    assert!(crate::audit(&doc, limits).expect("within the budget").is_empty());
}

/// A run that draws no word still costs a comparison: the covered text walks every run of a
/// covered line, so a word followed by many `( ) Tj` must not be checked for free (review on
/// #4198).
#[test]
fn runs_without_words_count_against_the_comparison_bound() {
    let mut content = b"BT /F1 24 Tf 100 700 Td (SECRET) Tj".to_vec();
    content.extend_from_slice(&b" ( ) Tj".repeat(50));
    content.extend_from_slice(
        b" ET
0 g
98 688 94 32 re f
",
    );
    let doc = open(one_page(&content, ""));
    let lines = pdfcraft_edit::glyph_text(&doc, &pdfcraft_model::pages(&doc)[0], false).expect("text").lines;
    assert_eq!(lines.iter().map(|l| (l.ink.len(), l.runs().count())).collect::<Vec<_>>(), [(1, 51)]);
    // Plus the hidden line's text, read once to copy it.
    let text = lines[0].text.len() / crate::TEXT_BYTES_PER_COMPARISON;
    let limits = crate::Limits { comparisons: 50 + text, covered_text: usize::MAX };
    assert!(matches!(crate::audit(&doc, limits), Err(crate::AuditError::TooManyComparisons)));
    let limits = crate::Limits { comparisons: 51 + text, covered_text: usize::MAX };
    assert_eq!(crate::audit(&doc, limits).expect("within the budget").len(), 1);
}

/// Covered text is document text: a document whose permissions withhold copying reports where
/// its covers are but not what they hide, unless it was opened with the owner password.
#[test]
fn copy_restricted_documents_withhold_covered_text() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0 g\n100 688 220 32 re f\n";
    let mut doc = open(one_page(content, ""));
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -1 ^ 0b1_0000,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .expect("encrypt");
    let bytes = Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).expect("save"));
    let user = Document::open_with_password(bytes.clone(), None).expect("open");
    let findings = audit_redactions(&user).expect("audit must run");
    assert_eq!(findings.len(), 1, "the cover is still reported: {findings:?}");
    assert_eq!(findings[0].covered_text, "");
    // The output bound counts the text reported and the finding's record, so withheld text
    // costs only the record.
    let record = std::mem::size_of::<AuditFinding>();
    let nothing = crate::Limits { comparisons: usize::MAX, covered_text: record };
    assert_eq!(crate::audit(&user, nothing).expect("withheld text is not output").len(), 1);
    let none = crate::Limits { covered_text: record - 1, ..nothing };
    assert!(matches!(crate::audit(&user, none), Err(crate::AuditError::TooMuchOutput)));
    let owner = Document::open_with_password(bytes, Some("owner")).expect("open");
    let findings = audit_redactions(&owner).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].covered_text, "SECRET DATA 123");
    let exact = crate::Limits { comparisons: usize::MAX, covered_text: "SECRET DATA 123".len() + record };
    assert!(crate::audit(&owner, exact).is_ok());
    let short = crate::Limits { covered_text: exact.covered_text - 1, ..exact };
    assert!(matches!(crate::audit(&owner, short), Err(crate::AuditError::TooMuchOutput)));
}

/// Withheld text is never assembled: covers over one long line would otherwise copy the line once
/// per cover, a volume neither the comparison nor the output bound charges.
#[test]
fn withheld_covered_text_is_not_assembled() {
    const PAGE: [f64; 4] = [0.0, 0.0, 612.0, 792.0];
    let doc = open(one_page(b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n", ""));
    let lines: Vec<_> = pdfcraft_edit::text_lines(&doc, 0).expect("text").into_iter().map(|line| crate::Painted { layer: 0, line }).collect();
    let mut budget = crate::Budget { limits: crate::Limits::DEFAULT, comparisons: 0, page_cost: 0, covered_text: 0 };
    budget.start_page(&lines);
    let withheld = budget.hidden_text(&lines, false, &PAGE, |_| true).expect("within the budget");
    assert_eq!(withheld.map(|t| t.capacity()), Some(0), "a finding, with no text built for it");
    assert_eq!(budget.hidden_text(&lines, false, &PAGE, |_| false).expect("within the budget"), None);
    let copied = budget.hidden_text(&lines, true, &PAGE, |_| true).expect("within the budget");
    assert_eq!(copied.as_deref(), Some("SECRET DATA 123"));
}

/// `/ActualText` is metadata the file's author controls: an empty or substitute replacement must
/// not hide the glyphs a cover sits on.
#[test]
fn a_cover_over_text_with_empty_or_substitute_actual_text_is_found() {
    for actual in ["()", "(nothing here)"] {
        let content =
            format!("/Span << /ActualText {actual} >> BDC\nBT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\nEMC\n0 g\n100 688 220 32 re f\n");
        let findings = audit_redactions(&open(one_page(content.as_bytes(), ""))).expect("audit must run");
        assert_eq!(findings.len(), 1, "{actual}: {findings:?}");
        assert!(findings[0].covered_text.contains("SECRET"), "{actual}: covered_text was {:?}", findings[0].covered_text);
    }
}

#[test]
fn repeated_stream_references_share_one_decode_budget() {
    use pdfcraft_cos::{Dict, Document, Object, Stream};
    // One 1 MiB stream of spaces (a kilobyte on disk), named 300 times: by the page's /Contents,
    // and by 300 draws of a form holding it.
    let mut doc = Document::new_empty();
    let spaces = doc.add(Stream::flate(Dict::new(), &vec![b' '; 1 << 20]));
    let contents: Dict = [(b"Contents".to_vec(), Object::Array(vec![Object::Ref(spaces); 300]))].into_iter().collect();
    assert!(crate::cover::filled_rects(&doc, &contents, 0, &[]).is_err(), "repeated content streams");

    let form_dict: Dict = [(b"Subtype".to_vec(), Object::Name(b"Form".to_vec()))].into_iter().collect();
    let form = doc.add(Stream::flate(form_dict, &vec![b' '; 1 << 20]));
    let draws = doc.add(Stream::from_raw(Dict::new(), "/X Do ".repeat(300).into_bytes()));
    let xobjects: Dict = [(b"X".to_vec(), Object::Ref(form))].into_iter().collect();
    let resources: Dict = [(b"XObject".to_vec(), Object::Dict(xobjects))].into_iter().collect();
    let page: Dict = [(b"Contents".to_vec(), Object::Ref(draws)), (b"Resources".to_vec(), Object::Dict(resources))].into_iter().collect();
    assert!(crate::cover::filled_rects(&doc, &page, 0, &[]).is_err(), "repeated form draws");
}

#[test]
fn form_entries_past_the_page_allowance_fail_the_audit() {
    use pdfcraft_cos::{Dict, Document, Object, Stream};
    // An empty form decodes to no bytes, so only the count of entries bounds drawing it again.
    let mut doc = Document::new_empty();
    let form_dict: Dict = [(b"Subtype".to_vec(), Object::Name(b"Form".to_vec()))].into_iter().collect();
    let form = doc.add(Stream::from_raw(form_dict, Vec::new()));
    let draws = doc.add(Stream::from_raw(Dict::new(), "/X Do ".repeat(pdfcraft_cos::MAX_PAGE_FORMS + 1).into_bytes()));
    let xobjects: Dict = [(b"X".to_vec(), Object::Ref(form))].into_iter().collect();
    let resources: Dict = [(b"XObject".to_vec(), Object::Dict(xobjects))].into_iter().collect();
    let page: Dict = [(b"Contents".to_vec(), Object::Ref(draws)), (b"Resources".to_vec(), Object::Dict(resources))].into_iter().collect();
    assert!(matches!(crate::cover::filled_rects(&doc, &page, 0, &[]), Err(crate::AuditError::TooManyForms)));
}

/// Copying a covered line's text walks all of it, padding included, so a long padded line under
/// many covers costs its bytes once per cover, not one run per cover (CodeRabbit).
#[test]
fn covered_text_bytes_count_against_the_comparison_bound() {
    // One run: 20 KiB of spaces around a word, which trims to the word alone.
    let mut content = b"BT /F1 24 Tf 100 700 Td (".to_vec();
    content.extend_from_slice(&[b' '; 10_000]);
    content.extend_from_slice(b"SECRET");
    content.extend_from_slice(&[b' '; 10_000]);
    content.extend_from_slice(b") Tj ET\n0 g\n");
    for _ in 0..10 {
        content.extend_from_slice(b"-1 688 1000000 32 re f\n");
    }
    let doc = open(one_page(&content, ""));
    let lines = pdfcraft_edit::glyph_text(&doc, &pdfcraft_model::pages(&doc)[0], false).expect("text").lines;
    assert_eq!(lines.len(), 1);
    let bytes = lines[0].text.len();
    assert!(bytes > 20_000, "{bytes} bytes of text");
    // Each cover: one comparison for the line, plus its bytes when it is hidden.
    let exact = crate::Limits { comparisons: 10 * (1 + bytes / crate::TEXT_BYTES_PER_COMPARISON), covered_text: usize::MAX };
    let findings = crate::audit(&doc, exact).expect("within the budget");
    assert_eq!(findings.len(), 10, "{findings:?}");
    assert!(findings.iter().all(|f| f.covered_text == "SECRET"), "{findings:?}");
    let short = crate::Limits { comparisons: exact.comparisons - 1, ..exact };
    assert!(matches!(crate::audit(&doc, short), Err(crate::AuditError::TooManyComparisons)));
}

/// One annotation list shared by many pages makes a finding on each: every finding's record
/// counts against the output bound, text or no text (CodeRabbit).
#[test]
fn every_finding_record_counts_against_the_output_bound() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET) Tj ET\n";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R 7 0 R 8 0 R] /Count 3 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots 6 0 R >>");
    b.add_stream(content);
    b.add(FONT);
    b.add("[<< /Type /Annot /Subtype /Redact /Rect [95 685 200 725] >>]");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots 6 0 R >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots 6 0 R >>");
    let doc = open(b.build());
    let one = "SECRET".len() + std::mem::size_of::<AuditFinding>();
    let exact = crate::Limits { comparisons: usize::MAX, covered_text: 3 * one };
    assert_eq!(crate::audit(&doc, exact).expect("within the budget").len(), 3);
    let short = crate::Limits { covered_text: 3 * one - 1, ..exact };
    assert!(matches!(crate::audit(&doc, short), Err(crate::AuditError::TooMuchOutput)));
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
