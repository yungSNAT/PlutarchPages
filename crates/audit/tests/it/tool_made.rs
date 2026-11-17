//! Redactions and covers that PyMuPDF, `mutool` and pdf.js wrote over `SECRET 4417`
//! (`tests/data/tool-made`, made as its README says), and what the audit reports for each
//! (`docs/redaction-head-to-head.md`).

use std::sync::Arc;

use pdfcraft_audit::{FindingKind, audit_redactions};
use pdfcraft_cos::Document;

/// The kind and covered text of each finding in the file `bytes` hold.
fn findings(bytes: &[u8]) -> Vec<(FindingKind, String)> {
    let doc = Document::open(Arc::new(bytes.to_vec())).expect("tool-made pdf must open");
    audit_redactions(&doc).expect("audit must run").into_iter().map(|f| (f.kind, f.covered_text)).collect()
}

/// Every cover over text that still extracts is flagged, and a mark left unapplied too; an applied
/// redaction, which removed the text, is not.
#[test]
fn files_other_tools_wrote() {
    use FindingKind::{CoveredText, UnappliedMark};
    let secret = |kind| vec![(kind, "SECRET 4417".to_owned())];
    let cases: [(&str, &[u8], Vec<(FindingKind, String)>); 9] = [
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

/// pdf.js's ink stroke hides the text (PDFium renders the box one colour), but the audit doesn't
/// read a stroked path as a cover yet.
#[test]
#[ignore = "stroked paths are not read as covers yet (docs/redaction-head-to-head.md)"]
fn a_pdfjs_ink_stroke_over_text_is_flagged() {
    let want = vec![(FindingKind::CoveredText, "SECRET 4417".to_owned())];
    assert_eq!(findings(include_bytes!("../data/tool-made/pdfjs-ink.pdf")), want);
}
