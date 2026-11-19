//! Redactions and covers that PyMuPDF, `mutool` and pdf.js wrote over `SECRET 4417`
//! (`tests/data/tool-made`, made as its README says), and what the audit reports for each
//! (`docs/redaction-head-to-head.md`).

use std::sync::Arc;

use pdfcraft_audit::{FindingKind, audit_document, audit_redactions};
use pdfcraft_cos::Document;

/// What the audit reports for a file: each finding's kind and covered text.
type Findings = Vec<(FindingKind, String)>;

/// The document `bytes` hold.
fn open(bytes: &[u8]) -> Document {
    Document::open(Arc::new(bytes.to_vec())).expect("tool-made pdf must open")
}

/// The findings in the file `bytes` hold.
fn findings(bytes: &[u8]) -> Findings {
    audit_redactions(&open(bytes)).expect("audit must run").into_iter().map(|f| (f.kind, f.covered_text)).collect()
}

/// The page findings for each of these 9 files: each cover the tools drew over `SECRET 4417` is
/// flagged and so is each mark left unapplied; the 2 redactions applied and saved with garbage
/// collection, which hold no copy of the text, and the base page are not.
#[test]
fn files_other_tools_wrote() {
    use FindingKind::{CoveredText, UnappliedMark};
    let secret = |kind| vec![(kind, "SECRET 4417".to_owned())];
    let cases: [(&str, &[u8], Findings); 9] = [
        ("base", include_bytes!("../data/tool-made/base.pdf"), vec![]),
        ("pymupdf-applied", include_bytes!("../data/tool-made/pymupdf-applied.pdf"), vec![]),
        ("pymupdf-mark-unapplied", include_bytes!("../data/tool-made/pymupdf-mark-unapplied.pdf"), secret(UnappliedMark)),
        ("pymupdf-drawn-box", include_bytes!("../data/tool-made/pymupdf-drawn-box.pdf"), secret(CoveredText)),
        ("pymupdf-square-annot", include_bytes!("../data/tool-made/pymupdf-square-annot.pdf"), secret(CoveredText)),
        ("mutool-applied", include_bytes!("../data/tool-made/mutool-applied.pdf"), vec![]),
        ("mutool-mark-unapplied", include_bytes!("../data/tool-made/mutool-mark-unapplied.pdf"), secret(UnappliedMark)),
        ("mutool-square-annot", include_bytes!("../data/tool-made/mutool-square-annot.pdf"), secret(CoveredText)),
        ("pdfjs-highlight", include_bytes!("../data/tool-made/pdfjs-highlight.pdf"), secret(CoveredText)),
    ];
    for (name, bytes, want) in cases {
        assert_eq!(findings(bytes), want, "{name}");
    }
}

/// pdf.js's ink stroke hides the text (PDFium renders the box one colour), and the audit reads the
/// band its stroke paints as a cover.
#[test]
fn a_pdfjs_ink_stroke_over_text_is_flagged() {
    let want = vec![(FindingKind::CoveredText, "SECRET 4417".to_owned())];
    assert_eq!(findings(include_bytes!("../data/tool-made/pdfjs-ink.pdf")), want);
}

/// A redaction applied and saved without garbage collection leaves the page's 2 original content
/// streams, one drawing `SECRET 4417`, the other `PUBLIC LINE`, in the file with nothing referring
/// to them. The page shows and extracts no secret, but any reader of the file's objects recovers
/// it: the audit counts both streams in a document finding, and no more when the file keeps its
/// dictionaries in an object stream and its cross-reference table in a stream, which nothing
/// refers to either. Saved with garbage collection, nothing is left to report.
#[test]
fn an_applied_redaction_saved_without_garbage_collection_is_flagged() {
    let document_findings = |bytes: &[u8]| -> Vec<(&'static str, usize)> {
        let report = audit_document(&open(bytes)).expect("audit must run");
        assert!(report.page_findings.is_empty());
        report.document_findings.iter().map(|f| (f.kind, f.count)).collect()
    };
    for (name, bytes, want) in [
        ("pymupdf-applied-no-garbage", &include_bytes!("../data/tool-made/pymupdf-applied-no-garbage.pdf")[..], &[("unreferenced-text", 2)][..]),
        ("mutool-applied-no-garbage", &include_bytes!("../data/tool-made/mutool-applied-no-garbage.pdf")[..], &[("unreferenced-text", 2)]),
        (
            "pymupdf-applied-objstm-no-garbage",
            &include_bytes!("../data/tool-made/pymupdf-applied-objstm-no-garbage.pdf")[..],
            &[("unreferenced-text", 2)],
        ),
        ("pymupdf-applied", &include_bytes!("../data/tool-made/pymupdf-applied.pdf")[..], &[]),
        ("mutool-applied", &include_bytes!("../data/tool-made/mutool-applied.pdf")[..], &[]),
    ] {
        assert_eq!(document_findings(bytes), want, "{name}");
    }
}
