//! The audit against content it reads through resources: fonts a graphics state sets, Type 3 fonts
//! and tiling patterns drawn under more than one set of resources, and covers painted in colour
//! spaces, patterns and shadings. Contributor-original fixtures, built in code.

use std::sync::Arc;

use pdfcraft_audit::{AuditError, FindingKind, audit_redactions};
use pdfcraft_cos::Document;
use pdfcraft_testkit::{Pdf, stream_obj};

mod covers;

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
/// still read, and a cover painted over it is found: a font given by reference or written out in
/// place, on the page or in a form it draws.
#[test]
fn text_in_a_font_a_graphics_state_sets_is_audited() {
    let shown = "/GS1 gs BT 100 700 Td (SECRET) Tj ET 0 g 100 688 220 32 re f";
    for font in ["5 0 R", HELVETICA] {
        let doc = page(&format!("<< /ExtGState << /GS1 << /Type /ExtGState /Font [{font} 24] >> >> >>"), shown, &[]);
        assert_eq!(covered(&doc), ["SECRET"], "{font}");
    }
    let doc = page("<< /XObject << /Fm 6 0 R >> >>", "/Fm Do", &[form("<< /ExtGState << /GS1 << /Font [5 0 R 24] >> >> >>", shown)]);
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

/// Resources written out in place have no object number to tell them apart by. Type 3 fonts A and B
/// are written out in place, each with resources of its own naming font T, which has none: T's
/// glyph draws form X, which only B's resources name, so it shows text when B's glyph selects T,
/// though A's selected it first.
#[test]
fn resources_written_in_place_are_told_apart() {
    let font = |glyph: usize, resources: &str| String::from_utf8(type3(glyph, &format!("/Resources {resources}"))).expect("ascii");
    let a = font(7, "<< /Font << /T 6 0 R >> >>");
    let b = font(7, "<< /Font << /T 6 0 R >> /XObject << /X 9 0 R >> >>");
    let doc = page(
        &format!("<< /Font << /A {a} /B {b} >> >>"),
        "BT /A 12 Tf /B 12 Tf ET",
        &[
            type3(8, ""),
            stream_obj("", "1000 0 d0 /T 1 Tf"),
            stream_obj("", "1000 0 d0 /X Do"),
            form("<< /Font << /F1 5 0 R >> >>", "BT /F1 12 Tf (SECRET) Tj ET"),
        ],
    );
    let result = audit_redactions(&doc);
    assert!(matches!(result, Err(AuditError::UncheckedText(0, "a Type 3 glyph"))), "{result:?}");
}

/// The text every cover test draws: SECRET along y 700 and PUBLIC along y 500.
const TWO_LINES: &str = "BT /F1 24 Tf 100 700 Td (SECRET) Tj 0 -200 Td (PUBLIC) Tj ET";

/// A shading over the whole page, solid black.
const BLACK_SHADING: &str = "<< /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 612 0] \
                             /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [0] /N 1 >> >>";

/// A cover painted in a colour set with `cs` and `sc` or `scn` is judged by that colour, not by
/// the last `g`, `rg` or `k`: black over the black text hides it whatever `g` said before, and
/// white doesn't. DeviceCMYK starts black, by name or through a resource, and an ICC-based CMYK space
/// starts at 0 0 0 0, white; the other spaces start black.
#[test]
fn a_colour_a_colour_space_sets_is_the_covers() {
    let resources = "<< /Font << /F1 5 0 R >> /ColorSpace << /Icc [/ICCBased 6 0 R] /Rgb /DeviceRGB /Cmyk /DeviceCMYK /Icc4 [/ICCBased 7 0 R] >> >>";
    for (fill, hidden) in [
        ("1 g /DeviceGray cs 0 sc", true),
        ("1 g /DeviceRGB cs 0 0 0 sc", true),
        ("1 g /DeviceCMYK cs 0 0 0 1 scn", true),
        ("1 g /Icc cs 0 scn", true),
        ("1 g /Rgb cs 0 0 0 sc", true),
        ("1 g /DeviceGray cs", true),
        ("1 g /Cmyk cs", true),
        ("0 g /Icc4 cs", false),
        ("0 g /DeviceGray cs 1 sc", false),
        ("0 g /DeviceRGB cs 1 1 1 sc", false),
        ("0 g /Icc cs 1 sc", false),
    ] {
        let doc = page(resources, &format!("{TWO_LINES} {fill} 100 688 220 32 re f"), &[stream_obj("/N 1", ""), stream_obj("/N 4", "")]);
        let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
        assert_eq!(covered(&doc), want, "{fill}");
    }
}

/// A cover in a colour the audit doesn't read, a pattern's or a spot colour's, counts as hiding the
/// text under it: it may be as dark as the text, whatever `g` said before.
#[test]
fn a_cover_in_a_colour_the_audit_cant_read_hides_text() {
    let resources = "<< /Font << /F1 5 0 R >> /Pattern << /P 6 0 R >> /ColorSpace << /Spot [/Separation /Ink /DeviceGray 7 0 R] >> >>";
    let objects =
        [format!("<< /PatternType 2 /Shading {BLACK_SHADING} >>").into_bytes(), b"<< /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >>".to_vec()];
    for fill in ["1 g /Pattern cs /P scn", "1 g /Spot cs 1 scn"] {
        let doc = page(resources, &format!("{TWO_LINES} {fill} 100 688 220 32 re f"), &objects);
        assert_eq!(covered(&doc), ["SECRET"], "{fill}");
    }
}

/// A shading painted with `sh` over text hides what the clip it is painted in holds: a clip set by
/// a rectangle, by lines or by a curve, the box of the form it is painted in, or the page, and
/// within its own `/BBox`. A half-transparent one leaves the text legible, as a half-transparent
/// rectangle does.
#[test]
fn a_shading_painted_over_text_hides_it() {
    let resources =
        "<< /Font << /F1 5 0 R >> /Shading << /Sh 6 0 R /Boxed 8 0 R /Odd 9 0 R >> /XObject << /Fm 7 0 R >> /ExtGState << /Half << /ca 0.5 >> >> >>";
    let objects = [
        BLACK_SHADING.as_bytes().to_vec(),
        stream_obj("/Type /XObject /Subtype /Form /BBox [100 688 320 720] /Resources << /Shading << /Sh 6 0 R >> >>", "/Sh sh"),
        BLACK_SHADING.replacen("<< ", "<< /BBox [100 688 320 720] ", 1).into_bytes(),
        BLACK_SHADING.replacen("<< ", "<< /BBox [100 688 320 720 /X] ", 1).into_bytes(),
    ];
    for (paint, hidden) in [
        ("q 100 688 220 32 re W n /Sh sh Q", true),
        ("q 100 688 m 320 688 l 320 720 l 100 720 l h W n /Sh sh Q", true),
        ("q 100 688 m 320 688 l 320 720 l 210 740 100 720 v h W n /Sh sh Q", true),
        ("q 0 600 612 192 re W n 100 0 0 100 0 0 cm /Sh sh Q", true),
        ("/Fm Do", true),
        ("/Boxed sh", true),
        ("0.01 0 0 0.01 0 0 cm /Boxed sh", false),
        ("q 100 400 220 32 re W n /Sh sh Q", false),
        ("q 100 688 220 32 re W n /Half gs /Sh sh Q", false),
    ] {
        let doc = page(resources, &format!("{TWO_LINES} {paint}"), &objects);
        let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
        assert_eq!(covered(&doc), want, "{paint}");
    }
    // A /BBox that is not 4 numbers is no box: the shading fills its clip, here the page.
    for paint in ["/Sh sh", "/Odd sh"] {
        let doc = page(resources, &format!("{TWO_LINES} {paint}"), &objects);
        let all = covered(&doc).concat();
        assert!(all.contains("SECRET") && all.contains("PUBLIC"), "{paint}: {all}");
    }
    let doc = page(resources, &format!("/Sh sh {TWO_LINES}"), &objects);
    assert!(covered(&doc).is_empty(), "text painted after the shading is on it");
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
