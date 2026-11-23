// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// A Type 3 font the way Chrome (Skia) writes one: a y-flipped font matrix, glyph boxes in `d1`,
/// and an empty glyph (a space) whose box is all zeros; code 1 is a 600 × 700 box, code 15 the
/// space. It is object 4, its glyph procedures objects 5 and 6.
const TYPE3_FONT: &str = "<< /Type /Font /Subtype /Type3 /FontMatrix [0.001 0 0 -0.001 0 0] /FontBBox [0 -700 600 0] /FirstChar 0 /LastChar 15 \
/Widths [0 600 0 0 0 0 0 0 0 0 0 0 0 0 0 250] /Encoding << /Type /Encoding /Differences [1 /g3B 15 /g3] >> /CharProcs << /g3B 5 0 R /g3 6 0 R >> \
/Resources << /ProcSet [/PDF] >> >>";

/// Objects 1–6: catalog, page tree (`kids`, and `resources` for pages without their own),
/// Helvetica, and [`TYPE3_FONT`] with its glyphs. Further objects follow from 7.
fn type3_base(kids: &[usize], resources: &str) -> pdfcraft_testkit::Pdf {
    let kids: Vec<String> = kids.iter().map(|k| format!("{k} 0 R")).collect();
    pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj(format!("<< /Type /Pages /Kids [{}] /Count {} /MediaBox [0 0 200 300] /Resources {resources} >>", kids.join(" "), kids.len()))
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .obj(TYPE3_FONT)
        .stream("", "600 0 0 -700 600 0 d1\n0 -700 600 700 re f")
        .stream("", "250 0 0 0 0 0 d1")
}

fn open(pdf: &pdfcraft_testkit::Pdf) -> Document {
    Document::open(Arc::new(pdf.build())).unwrap()
}

/// What draws with a Type 3 font is found wherever the print takes it from: the page's fonts,
/// forms inside forms, tiling patterns, soft masks (a shading pattern's too), the page tree's
/// inherited resources, and the appearances of the annotations that print under the chosen
/// Comments & Forms. Pages with ordinary fonts only are left alone (#2684).
#[test]
fn type3_fonts_are_found_through_forms_patterns_soft_masks_and_printed_annotations() {
    let t3 = "<< /Font << /T3 4 0 R >> >>";
    let pages: [(&str, &str); 10] = [
        // 0: Helvetica, and a printing comment drawn with Helvetica.
        (
            "/Resources << /Font << /F1 3 0 R >> >> /Annots [<< /Subtype /Square /F 4 /Rect [10 10 20 20] /AP << /N 11 0 R >> >>]",
            "BT /F1 9 Tf (Hi) Tj ET",
        ),
        // 1: the page's own fonts.
        ("/Resources << /Font << /T3 4 0 R >> >>", "BT /T3 20 Tf <010F01> Tj ET"),
        // 2: a form inside a form.
        ("/Resources << /XObject << /Outer 8 0 R >> >>", "/Outer Do"),
        // 3: a tiling pattern.
        ("/Resources << /Pattern << /P 9 0 R >> >>", "/Pattern cs /P scn 0 0 50 50 re f"),
        // 4: a soft mask's group.
        ("/Resources << /ExtGState << /G << /SMask << /S /Luminosity /G 10 0 R >> >> >> >>", "/G gs 0 0 50 50 re f"),
        // 5: a shading pattern's soft mask.
        (
            "/Resources << /Pattern << /Sh << /PatternType 2 /Shading << /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 1 0] \
/Function << /FunctionType 2 /Domain [0 1] /N 1 >> >> /ExtGState << /SMask << /S /Alpha /G 10 0 R >> >> >> >> >>",
            "/Pattern cs /Sh scn 0 0 50 50 re f",
        ),
        // 6: Helvetica content, and a printing comment drawn with the Type 3 font.
        (
            "/Resources << /Font << /F1 3 0 R >> >> /Annots [<< /Subtype /Square /F 4 /Rect [10 10 20 20] /AP << /N 12 0 R >> >>]",
            "BT /F1 9 Tf (Hi) Tj ET",
        ),
        // 7: the same comment, not printing.
        ("/Resources << >> /Annots [<< /Subtype /Square /F 0 /Rect [10 10 20 20] /AP << /N 12 0 R >> >>]", ""),
        // 8: a form field drawn with the Type 3 font, on Helvetica content.
        (
            "/Resources << /Font << /F1 3 0 R >> >> /Annots [<< /Subtype /Widget /FT /Tx /F 4 /Rect [10 10 20 20] /AP << /N 12 0 R >> >>]",
            "BT /F1 9 Tf (Hi) Tj ET",
        ),
        // 9: no resources of its own: the page tree's.
        ("", "BT /T3 20 Tf <01> Tj ET"),
    ];
    let first = 13;
    let kids: Vec<usize> = (0..pages.len()).map(|i| first + 2 * i).collect();
    let mut pdf = type3_base(&kids, t3)
        // 7: the inner form, 8: the outer one.
        .stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 200 300] /Resources {t3}"), "BT /T3 20 Tf <01> Tj ET")
        .stream("/Type /XObject /Subtype /Form /BBox [0 0 200 300] /Resources << /XObject << /Inner 7 0 R >> >>", "/Inner Do")
        // 9: a tiling pattern, 10: a soft mask's group, 11: a Helvetica appearance, 12: a Type 3 one.
        .stream(&format!("/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources {t3}"), "BT /T3 5 Tf <01> Tj ET")
        .stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 200 300] /Group << /S /Transparency >> /Resources {t3}"), "BT /T3 5 Tf <01> Tj ET")
        .stream("/Subtype /Form /BBox [0 0 10 10] /Resources << /Font << /F1 3 0 R >> >>", "BT /F1 5 Tf (a) Tj ET")
        .stream(&format!("/Subtype /Form /BBox [0 0 10 10] /Resources {t3}"), "BT /T3 5 Tf <01> Tj ET");
    for (i, (entries, content)) in pages.iter().enumerate() {
        assert_eq!(pdf.next_obj(), first + 2 * i);
        pdf = pdf.obj(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R {entries} >>", first + 2 * i + 1)).stream("", content);
    }
    let doc = open(&pdf);
    let all: Vec<usize> = (0..pages.len()).collect();
    assert_eq!(type3_pages(&doc, &all, Content::DocumentAndMarkups), [1, 2, 3, 4, 5, 6, 8, 9]);
    // Only what prints counts: comments are left out of Document, page content out of Form
    // fields only.
    assert_eq!(type3_pages(&doc, &all, Content::Document), [1, 2, 3, 4, 5, 8, 9]);
    assert_eq!(type3_pages(&doc, &all, Content::FormFieldsOnly), [8]);
    // Each page once, in the order asked; pages past the end are left out.
    assert_eq!(type3_pages(&doc, &[9, 1, 1, 0, 42, usize::MAX], Content::DocumentAndMarkups), [9, 1]);
    assert!(type3_pages(&doc, &[], Content::DocumentAndMarkups).is_empty());
}

/// Twenty pages sharing one resource dictionary (Helvetica) and a last page with the Type 3 font
/// of its own. What the first page read in full is not read again for the others, so twenty pages
/// cost little more than one; a budget too small for the pages counts the ones it doesn't reach
/// as drawing with a Type 3 font, the side that prints safely.
#[test]
fn the_type3_check_reads_shared_resources_once_within_a_budget() {
    let kids: Vec<usize> = (0..21).map(|i| 8 + 2 * i).collect();
    let mut pdf = type3_base(&kids, "<< >>").obj("<< /Font << /F1 3 0 R >> >>");
    for i in 0..21 {
        let resources = if i == 20 { "<< /Font << /T3 4 0 R >> >>".to_string() } else { "7 0 R".to_string() };
        assert_eq!(pdf.next_obj(), 8 + 2 * i);
        pdf = pdf
            .obj(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources {resources} >>", 9 + 2 * i))
            .stream("", "BT /F1 9 Tf (Hi) Tj ET");
    }
    let doc = open(&pdf);
    let all: Vec<usize> = (0..21).collect();
    assert_eq!(type3::pages_within(&doc, &all, Content::DocumentAndMarkups, usize::MAX), [20]);
    // Two reads for the first page, one for each page after it: all twenty within 25.
    assert_eq!(type3::pages_within(&doc, &all[..20], Content::DocumentAndMarkups, 25), Vec::<usize>::new());
    // Within 3, the third page and those after it aren't read: they count.
    assert_eq!(type3::pages_within(&doc, &all, Content::DocumentAndMarkups, 3), (2..21).collect::<Vec<_>>());
}

/// Broken, cyclic and hostile resources never panic. What can't be read is not a Type 3 font,
/// but resources nested past the depth limit or too large for the budget count as one: printing
/// such a page as a picture is the safe side.
#[test]
fn type3_detection_survives_malformed_cyclic_deep_and_huge_resources() {
    // Forms from object 7 on, each drawing the next; the last one has `last` as its resources.
    let chain = |n: usize, last: &str| -> Vec<Vec<u8>> {
        (0..n)
            .map(|i| {
                let res = if i + 1 == n { last.to_string() } else { format!("<< /XObject << /X {} 0 R >> >>", 8 + i) };
                pdfcraft_testkit::stream_obj(&format!("/Subtype /Form /BBox [0 0 1 1] /Resources {res}"), "/X Do")
            })
            .collect()
    };
    let huge: String = (0..25_000).map(|i| format!("/F{i} 3 0 R ")).collect();
    let pages: Vec<(String, &str)> = vec![
        ("/Resources 42".into(), ""),
        ("/Resources << /Font /NotADict /XObject [1 2] /Pattern 7 /ExtGState null >>".into(), ""),
        ("/Resources << /Font << /F1 999 0 R /F2 null /F3 << /Subtype 5 >> /F4 [4 0 R] >> >>".into(), ""),
        // Forms that draw themselves and each other.
        ("/Resources << /XObject << /A 47 0 R >> >>".into(), "/A Do"),
        ("/Resources << /ExtGState << /G1 << /SMask /None >> /G2 << /SMask << /G 1 0 R >> >> /G3 << /SMask << /G 999 0 R >> >> >> >>".into(), ""),
        ("/Resources << >> /Annots 5".into(), ""),
        ("/Resources << >> /Annots [7 null << /Subtype /Square /F 4 /AP << /N << /On 4 0 R >> >> >> << /Subtype /Square /F 4 /AP 3 >>]".into(), ""),
        // Forty forms deep, with no Type 3 font at the bottom: past the depth limit.
        ("/Resources << /XObject << /X 7 0 R >> >>".into(), "/X Do"),
        // So many fonts that the budget runs out before the end.
        (format!("/Resources << /Font << {huge} >> >>"), ""),
        // A font whose subtype is an indirect name.
        ("/Resources << /Font << /T 49 0 R >> >>".into(), ""),
    ];
    let first = 51;
    let kids: Vec<usize> = (0..pages.len()).map(|i| first + 2 * i).collect();
    let mut pdf = type3_base(&kids, "<< >>").objs(chain(40, "<< >>"));
    assert_eq!(pdf.next_obj(), 47);
    pdf = pdf
        .stream("/Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /Self 47 0 R /B 48 0 R >> >>", "/Self Do /B Do")
        .stream("/Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /A 47 0 R >> >>", "/A Do")
        .obj("<< /Type /Font /Subtype 50 0 R >>")
        .obj("/Type3");
    assert_eq!(pdf.next_obj(), first);
    for (i, (entries, content)) in pages.iter().enumerate() {
        pdf = pdf.obj(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R {entries} >>", first + 2 * i + 1)).stream("", content);
    }
    let doc = open(&pdf);
    let all: Vec<usize> = (0..pages.len()).collect();
    assert_eq!(type3_pages(&doc, &all, Content::DocumentAndMarkups), [7, 8, 9]);
    // A Type 3 font at the bottom of a chain within the depth limit is found.
    let short = type3_base(&[17], "<< >>")
        .objs(chain(10, "<< /Font << /T3 4 0 R >> >>"))
        .obj("<< /Type /Page /Parent 2 0 R /Contents 18 0 R /Resources << /XObject << /X 7 0 R >> >> >>")
        .stream("", "/X Do");
    assert_eq!(type3_pages(&open(&short), &[0], Content::DocumentAndMarkups), [0]);
}

/// `impose_with_pictures` draws a page's picture in its place, over the whole displayed page
/// (a turned page too), instead of its content and comments; the other pages stay as they are.
#[test]
fn a_picture_replaces_its_page_on_the_sheet_and_the_other_pages_stay_vector() {
    let mut doc = fixture(3);
    // The second page is shown turned a quarter.
    let second = pdfcraft_model::pages(&doc)[1].obj;
    doc.update_dict(second, |d| d.set(b"Rotate".to_vec(), Object::Int(90))).unwrap();
    let picture = |rgb: [u8; 3]| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(1));
        d.set(b"Height".to_vec(), Object::Int(1));
        d.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        Stream::flate(d, &rgb)
    };
    let pictures = std::collections::HashMap::from([(0, picture([255, 0, 0])), (1, picture([0, 0, 255]))]);
    let out = impose_with_pictures(&doc, &settings(vec![0, 1, 2], Layout::Size(SizeMode::Actual)), &pictures).unwrap();
    let printed = Document::open(Arc::new(out)).unwrap();
    let sheets = pdfcraft_model::pages(&printed);
    assert_eq!(sheets.len(), 3);
    let images = printed.scan_objects().filter(|(_, o)| matches!(&**o, Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image"))).count();
    assert_eq!(images, 2, "one picture each");
    // The form a sheet draws, and its content.
    let form = |sheet: &pdfcraft_model::Page| -> (Dict, String) {
        let res = printed.resolve(sheet.dict.get(b"Resources").unwrap());
        let xo = printed.resolve(res.as_dict().unwrap().get(b"XObject").unwrap());
        let f = printed.resolve(xo.as_dict().unwrap().get(b"P0").unwrap());
        let Object::Stream(s) = &*f else { panic!("a form") };
        (s.dict.clone(), String::from_utf8(s.decoded().unwrap()).unwrap())
    };
    let crops = [[0.0, 0.0, 200.0, 300.0], [0.0, 0.0, 200.0, 300.0]];
    for (i, sheet) in sheets.iter().enumerate() {
        let (dict, body) = form(sheet);
        let Some(crop) = crops.get(i) else {
            assert!(body.contains("(Page 3) Tj") && !body.contains("PCPrPicture"), "{body}");
            continue;
        };
        // Only the picture: no text, no comment appearances (page 1 has a printing square).
        assert!(!body.contains("Tj") && !body.contains("PCPrA") && body.contains("/PCPrPicture Do"), "sheet {i}: {body}");
        let res = printed.resolve(dict.get(b"Resources").unwrap());
        let xo = printed.resolve(res.as_dict().unwrap().get(b"XObject").unwrap());
        assert_eq!(xo.as_dict().unwrap().len(), 1, "sheet {i}: the picture alone");
        // The picture's unit square lands on the crop box in user space, for the turned page too
        // (the sheet then turns user space to the display, as for every page).
        let parsed = pdfcraft_content::parse(body.as_bytes());
        let cms: Vec<[f64; 6]> = parsed.ops.iter().filter(|op| op.is("cm")).map(|op| op.nums::<6>().unwrap()).collect();
        assert_eq!(cms.len(), 2);
        let apply = |m: [f64; 6], p: [f64; 2]| [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]];
        let mut corners: Vec<[i64; 2]> =
            [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].iter().map(|&p| apply(cms[0], apply(cms[1], p)).map(|v| v.round() as i64)).collect();
        corners.sort();
        let [x0, y0, x1, y1] = crop.map(|v| v as i64);
        assert_eq!(corners, [[x0, y0], [x0, y1], [x1, y0], [x1, y1]], "sheet {i}");
    }
}
