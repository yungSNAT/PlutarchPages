//! The files of the redaction head-to-head (`docs/redaction-head-to-head.md`), built object for
//! object as that page describes them, and what the audit reports for each.

use std::sync::Arc;

use pdfcraft_audit::{FindingKind, audit_redactions};
use pdfcraft_cos::Document;
use pdfcraft_testkit::{Pdf, stream_obj};

const TEXT: &str = "BT /F1 24 Tf 100 700 Td (SECRET 4417) Tj ET";
const PUBLIC: &str = "BT /F1 24 Tf 100 500 Td (PUBLIC LINE) Tj ET";
const BOX: &str = "100 688 180 32 re f";
const OVER: &str = "180 0 0 32 100 688 cm";
const FONT: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";

/// One page drawing `content` with Helvetica as `/F1` (object 5) plus `fonts`, `resources` and
/// `annots` in its dictionary, then `objects` numbered from 6.
fn file(content: &str, resources: &str, fonts: &str, annots: &str, objects: &[Vec<u8>]) -> Document {
    let pdf = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R {fonts} >> {resources} >> {annots} >>"
        ))
        .stream("", content)
        .obj(FONT)
        .objs(objects);
    Document::open(Arc::new(pdf.build())).expect("test pdf must open")
}

/// An 8 by 8 DeviceGray image, every sample `grey`.
fn image(grey: u8) -> Vec<u8> {
    stream_obj("/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8", [grey; 64])
}

/// Every unsafe file is flagged: covered text, or an unapplied mark for U07. No safe one is.
#[test]
fn the_head_to_head_files() {
    use FindingKind::{CoveredText, UnappliedMark};
    let blocks = [
        // A Type 3 font whose `A` is a filled square, read as U+2588 FULL BLOCK.
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 -200 1000 800] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 8 0 R >> \
          /Encoding << /Type /Encoding /Differences [65 /a] >> /FirstChar 65 /LastChar 65 /Widths [1000] /ToUnicode 7 0 R /Resources << >> >>"
            .to_vec(),
        stream_obj(
            "",
            "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /Blocks def 1 begincodespacerange <00> <FF> \
             endcodespacerange 1 beginbfchar <41> <2588> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end",
        ),
        stream_obj("", "1000 0 0 -200 1000 800 d1 0 -200 1000 1000 re f"),
    ];
    let shading = "/Shading << /Sh << /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 612 0] \
                   /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [0] /N 1 >> >> >>";
    let square = [
        b"<< /Type /Annot /Subtype /Square /Rect [100 688 280 720] /C [0 0 0] /IC [0 0 0] /AP << /N 7 0 R >> >>".to_vec(),
        stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 180 32]", "0 g 0 0 180 32 re f"),
    ];
    let stamp = [
        b"<< /Type /Annot /Subtype /Stamp /Rect [100 688 280 720] /AP << /N 7 0 R >> >>".to_vec(),
        stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /Im 8 0 R >> >>", "/Im Do"),
        image(0),
    ];
    let mark = [b"<< /Type /Annot /Subtype /Redact /Rect [100 688 280 720] /IC [0 0 0] >>".to_vec()];
    let files = [
        ("U01 black box", file(&format!("{TEXT} {PUBLIC} 0 g {BOX}"), "", "", "", &[]), Some(CoveredText)),
        ("U02 black box over red text", file(&format!("1 0 0 rg {TEXT} 0 g {PUBLIC} {BOX}"), "", "", "", &[]), Some(CoveredText)),
        ("U03 whiteout", file(&format!("{TEXT} {PUBLIC} 1 g {BOX}"), "", "", "", &[]), Some(CoveredText)),
        ("U04 black image", file(&format!("{TEXT} {PUBLIC} q {OVER} /Im Do Q"), "/XObject << /Im 6 0 R >>", "", "", &[image(0)]), Some(CoveredText)),
        ("U05 inline image", file(&format!("{TEXT} {PUBLIC} q {OVER} BI /W 1 /H 1 /CS /G /BPC 8 ID \0 EI Q"), "", "", "", &[]), Some(CoveredText)),
        ("U06 Square", file(&format!("{TEXT} {PUBLIC}"), "", "", "/Annots [6 0 R]", &square), Some(CoveredText)),
        ("U07 unapplied mark", file(&format!("{TEXT} {PUBLIC}"), "", "", "/Annots [6 0 R]", &mark), Some(UnappliedMark)),
        (
            "U08 block characters",
            file(&format!("{TEXT} {PUBLIC} BT /B 24 Tf 100 700 Td (AAAAAAA) Tj ET"), "", "/B 6 0 R", "", &blocks),
            Some(CoveredText),
        ),
        ("U09 black shading", file(&format!("{TEXT} {PUBLIC} q 100 688 180 32 re W n /Sh sh Q"), shading, "", "", &[]), Some(CoveredText)),
        ("U10 Stamp drawing an image", file(&format!("{TEXT} {PUBLIC}"), "", "", "/Annots [6 0 R]", &stamp), Some(CoveredText)),
        (
            "S01 applied redaction",
            file(&format!("BT /F1 24 Tf 100 700 Td [(SECRET) -2700] TJ ET {PUBLIC} 0 g 200 688 60 32 re f"), "", "", "", &[]),
            None,
        ),
        ("S02 highlight", file(&format!("{TEXT} {PUBLIC} q /M gs 1 1 0 rg {BOX} Q"), "/ExtGState << /M << /BM /Multiply >> >>", "", "", &[]), None),
        ("S03 background", file(&format!("0.8 g {BOX} 0 g {TEXT} {PUBLIC}"), "", "", "", &[]), None),
        (
            "S04 OCR under its scan",
            file("BT 3 Tr /F1 24 Tf 100 700 Td (SECRET 4417) Tj ET q 612 0 0 792 0 0 cm /Im Do Q", "/XObject << /Im 6 0 R >>", "", "", &[image(255)]),
            None,
        ),
    ];
    for (name, doc, flagged) in files {
        let kinds: Vec<FindingKind> = audit_redactions(&doc).expect("audit must run").into_iter().map(|f| f.kind).collect();
        assert_eq!(kinds, Vec::from_iter(flagged), "{name}");
    }
}
