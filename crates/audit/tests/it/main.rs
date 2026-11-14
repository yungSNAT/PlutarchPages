//! The audit against content it reads through resources: fonts a graphics state sets, and Type 3
//! fonts and tiling patterns drawn under more than one set of resources. Contributor-original
//! fixtures, built in code.

use std::sync::Arc;

use pdfcraft_audit::{AuditError, FindingKind, audit_redactions};
use pdfcraft_cos::Document;
use pdfcraft_testkit::{Pdf, stream_obj};

/// Object 5 of every page [`page`] builds.
const HELVETICA: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";

/// One page with `resources`, drawing `content`, then `objects` numbered from 6 (5 is [`HELVETICA`]).
fn page(resources: &str, content: &str, objects: &[Vec<u8>]) -> Document {
    let pdf = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources {resources} >>"))
        .stream("", content)
        .obj(HELVETICA)
        .objs(objects);
    Document::open(Arc::new(pdf.build())).expect("test pdf must open")
}

/// A Type 3 font drawing `a` with glyph procedure `glyph` (an object number), its dictionary ending
/// with `extra`.
fn type3(glyph: usize, extra: &str) -> Vec<u8> {
    format!(
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a {glyph} 0 R >> \
         /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] {extra} >>"
    )
    .into_bytes()
}

/// A form XObject over the whole page, with `resources`, drawing `content`.
fn form(resources: &str, content: &str) -> Vec<u8> {
    stream_obj(&format!("/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources {resources}"), content)
}

/// The covered text of each finding, which must all be covered-text findings.
fn covered(doc: &Document) -> Vec<String> {
    let findings = audit_redactions(doc).expect("audit must run");
    assert!(findings.iter().all(|f| f.kind == FindingKind::CoveredText), "{findings:?}");
    findings.into_iter().map(|f| f.covered_text).collect()
}

/// Text that shows in a font an ExtGState's `/Font` sets (ISO 32000-2 §8.4.5), with no `Tf`, is
/// still read, and a cover painted over it is found.
#[test]
fn text_in_a_font_a_graphics_state_sets_is_audited() {
    let doc = page(
        "<< /Font << /F1 5 0 R >> /ExtGState << /GS1 << /Type /ExtGState /Font [5 0 R 24] >> >> >>",
        "/GS1 gs BT 100 700 Td (SECRET) Tj ET 0 g 100 688 220 32 re f",
        &[],
    );
    assert_eq!(covered(&doc), ["SECRET"]);
}

/// A Type 3 font an ExtGState sets is walked as one `Tf` selects is: text its glyph shows fails the
/// audit.
#[test]
fn a_type3_font_a_graphics_state_sets_is_walked() {
    let doc = page(
        "<< /Font << /F1 5 0 R >> /ExtGState << /GS1 << /Font [6 0 R 12] >> >> >>",
        "/GS1 gs BT 100 700 Td (a) Tj ET",
        &[type3(7, ""), stream_obj("", "1000 0 d0 BT /F1 12 Tf (SECRET) Tj ET")],
    );
    let result = audit_redactions(&doc);
    assert!(matches!(result, Err(AuditError::UncheckedText(0, "a Type 3 glyph"))), "{result:?}");
}

/// A Type 3 font without resources of its own reads those where it is selected (PDF 1.1). Selected
/// first by the page, whose resources name no `/X`, and then by a form whose resources do, its
/// glyph still draws form X's text in the form, and the audit fails.
#[test]
fn a_type3_font_without_resources_is_walked_with_each_set_it_reads() {
    let doc = |content: &str| {
        page(
            "<< /Font << /T 6 0 R >> /XObject << /Fm 8 0 R >> >>",
            content,
            &[
                type3(7, ""),
                stream_obj("", "1000 0 d0 /X Do"),
                form("<< /Font << /T 6 0 R >> /XObject << /X 9 0 R >> >>", "BT /T 12 Tf 100 700 Td (a) Tj ET"),
                form("<< /Font << /F1 5 0 R >> >>", "BT /F1 12 Tf (SECRET) Tj ET"),
            ],
        )
    };
    for content in ["/Fm Do", "BT /T 12 Tf 100 700 Td (a) Tj ET /Fm Do"] {
        let result = audit_redactions(&doc(content));
        assert!(matches!(result, Err(AuditError::UncheckedText(0, "a Type 3 glyph"))), "{content}: {result:?}");
    }
}

/// The same for a tiling pattern without resources of its own.
#[test]
fn a_pattern_without_resources_is_walked_with_each_set_it_reads() {
    let doc = |content: &str| {
        let pattern = stream_obj("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10", "/X Do");
        page(
            "<< /Pattern << /P 6 0 R >> /XObject << /Fm 7 0 R >> >>",
            content,
            &[
                pattern,
                form("<< /Pattern << /P 6 0 R >> /XObject << /X 8 0 R >> >>", "/Pattern cs /P scn 0 0 10 10 re f"),
                form("<< /Font << /F1 5 0 R >> >>", "BT /F1 12 Tf (SECRET) Tj ET"),
            ],
        )
    };
    for content in ["/Fm Do", "/Pattern cs /P scn 0 0 10 10 re f /Fm Do"] {
        let result = audit_redactions(&doc(content));
        assert!(matches!(result, Err(AuditError::UncheckedText(0, "a tiling pattern"))), "{content}: {result:?}");
    }
}
