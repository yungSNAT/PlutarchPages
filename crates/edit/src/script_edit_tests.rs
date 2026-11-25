//! Editing existing text in pages written the way PDF producers write CJK and right-to-left text:
//! a Type 0 font over a subset of a craft-fonts face (Identity-H, CID = glyph id, the outlines of
//! the glyphs the page doesn't show removed, as a subsetter keeping glyph ids leaves them), a
//! ToUnicode map for the glyphs shown, and Arabic and Hebrew shaped and drawn in visual order.
//! Each page is written twice: once with the subset's layout tables (cmap, GSUB, GPOS) kept, and
//! once with outlines only, as many subsetters write it. Each edit must read back as typed after a
//! save, draw what was typed, and add only the glyphs it needs. The faces come from craft-fonts (`CRAFT_FONTS_DIR`); without them these tests say so
//! and pass.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full, write_incremental};

use crate::text::{replace_block, replace_line, text_blocks};

const SIZE: f64 = 20.0;
const BASELINE: f64 = 100.0;
const LEFT: f64 = 40.0;
const RIGHT: f64 = 560.0;
/// How far apart, in pixels, two drawings' ink may be and still match (see [`difference`]).
const NEAR: usize = 2;
/// The largest share of unmatched ink a drawing of the typed text may have.
const DRAWN: f64 = 0.15;
/// The most bytes an edit may add for each character typed.
const GLYPH_BYTES: usize = 16 * 1024;

/// One script's page and its edits: `kept` uses only characters the page shows, `added` some it
/// doesn't. `named` is the subset's font name: the face's own, or one this build has no face for.
struct Case {
    family: &'static str,
    named: &'static str,
    original: &'static str,
    kept: &'static str,
    added: &'static str,
}

const CASES: &[Case] = &[
    Case {
        family: "BIZ UDPGothic",
        named: "BIZUDPGothic",
        original: "見本商会の請求書です",
        kept: "請求書の見本です",
        added: "見本商会の領収書です",
    },
    Case {
        family: "BIZ UDPGothic", named: "MSGothic", original: "見本商会の請求書です", kept: "請求書の見本です", added: "見本商会の領収書です"
    },
    Case {
        family: "LXGW WenKai Lite", named: "LXGWWenKaiLite-Regular", original: "这是我们的发票", kept: "我们的发票", added: "这是你们的收据"
    },
    Case { family: "LXGW WenKai Lite", named: "SimSun", original: "这是我们的发票", kept: "我们的发票", added: "这是你们的收据" },
    Case {
        family: "NanumGothic", named: "NanumGothic", original: "견본 회사의 청구서", kept: "청구서 견본", added: "견본 회사의 영수증"
    },
    Case {
        family: "NanumGothic", named: "MalgunGothic", original: "견본 회사의 청구서", kept: "청구서 견본", added: "견본 회사의 영수증"
    },
    Case {
        family: "Noto Sans Arabic", named: "NotoSansArabic", original: "مرحبا بالعالم", kept: "بالعالم مرحبا", added: "مرحبا يا صديقي"
    },
    Case {
        family: "Noto Sans Arabic", named: "ArialMT", original: "مرحبا بالعالم", kept: "بالعالم مرحبا", added: "مرحبا يا صديقي"
    },
    Case {
        family: "Noto Sans Hebrew", named: "NotoSansHebrew-Regular", original: "שלום עולם", kept: "עולם שלום", added: "שלום חברים"
    },
    Case { family: "Noto Sans Hebrew", named: "ArialMT", original: "שלום עולם", kept: "עולם שלום", added: "שלום חברים" },
];

fn be16(b: &[u8], at: usize) -> usize {
    usize::from(u16::from_be_bytes([b[at], b[at + 1]]))
}

fn be32(b: &[u8], at: usize) -> usize {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as usize
}

fn tables(program: &[u8]) -> BTreeMap<[u8; 4], &[u8]> {
    (0..be16(program, 4))
        .map(|i| {
            let r = 12 + 16 * i;
            let tag: [u8; 4] = program[r..r + 4].try_into().unwrap();
            (tag, &program[be32(program, r + 8)..be32(program, r + 8) + be32(program, r + 12)])
        })
        .collect()
}

/// `program` with the outlines of `keep` (and of the glyphs they are built from) only, every other
/// glyph left empty, as a subsetter keeping glyph ids writes it; the variation tables go, as a
/// subset of the default instance has none. With `layout` the cmap and layout tables stay whole,
/// as some subsetters leave them; without it they go, as others drop them.
fn subset(program: &[u8], keep: &BTreeSet<usize>, layout: bool) -> Vec<u8> {
    let t = tables(program);
    let (head, loca, glyf) = (t[b"head"], t[b"loca"], t[b"glyf"]);
    let glyphs = be16(t[b"maxp"], 4);
    let long = be16(head, 50) == 1;
    let at = |g: usize| if long { be32(loca, 4 * g) } else { 2 * be16(loca, 2 * g) };
    let mut kept: BTreeSet<usize> = keep.iter().copied().chain([0]).collect();
    let mut todo: Vec<usize> = kept.iter().copied().collect();
    while let Some(g) = todo.pop() {
        let data = &glyf[at(g)..at(g + 1)];
        if data.len() < 10 || i16::from_be_bytes([data[0], data[1]]) >= 0 {
            continue;
        }
        let mut p = 10;
        loop {
            let flags = be16(data, p);
            if kept.insert(be16(data, p + 2)) {
                todo.push(be16(data, p + 2));
            }
            let args = if flags & 1 != 0 { 4 } else { 2 };
            let scale = if flags & 8 != 0 {
                2
            } else if flags & 0x40 != 0 {
                4
            } else if flags & 0x80 != 0 {
                8
            } else {
                0
            };
            p += 4 + args + scale;
            if flags & 0x20 == 0 {
                break;
            }
        }
    }
    let (mut new_glyf, mut new_loca) = (Vec::new(), Vec::new());
    for g in 0..glyphs {
        new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
        if kept.contains(&g) {
            new_glyf.extend_from_slice(&glyf[at(g)..at(g + 1)]);
            new_glyf.resize(new_glyf.len().next_multiple_of(4), 0);
        }
    }
    new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
    let mut new_head = head.to_vec();
    new_head[8..12].fill(0);
    new_head[50..52].copy_from_slice(&1u16.to_be_bytes());
    let variation: [&[u8; 4]; 7] = [b"fvar", b"gvar", b"avar", b"HVAR", b"MVAR", b"STAT", b"cvar"];
    let out: Vec<([u8; 4], Vec<u8>)> = t
        .iter()
        .filter(|(tag, _)| !variation.contains(tag) && *tag != b"DSIG" && (layout || ![b"cmap", b"GSUB", b"GPOS", b"GDEF"].contains(tag)))
        .map(|(tag, data)| {
            let data = match tag {
                b"glyf" => new_glyf.clone(),
                b"loca" => new_loca.clone(),
                b"head" => new_head.clone(),
                _ => data.to_vec(),
            };
            (*tag, data)
        })
        .collect();
    assemble(&out)
}

/// The sfnt of `out`, whose tables are in tag order.
fn assemble(out: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut font = Vec::new();
    font.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    let n = out.len() as u16;
    let range = 16 * (1u16 << (15 - n.leading_zeros()));
    for v in [n, range, 15 - n.leading_zeros() as u16, n * 16 - range] {
        font.extend_from_slice(&v.to_be_bytes());
    }
    let mut offset = 12 + 16 * out.len();
    for (tag, data) in out {
        let padded: Vec<u8> = data.iter().copied().chain(std::iter::repeat(0)).take(data.len().next_multiple_of(4)).collect();
        let sum = padded.chunks(4).fold(0u32, |s, c| s.wrapping_add(u32::from_be_bytes(c.try_into().unwrap())));
        font.extend_from_slice(tag);
        font.extend_from_slice(&sum.to_be_bytes());
        font.extend_from_slice(&(offset as u32).to_be_bytes());
        font.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += padded.len();
    }
    for (_, data) in out {
        font.extend_from_slice(data);
        font.resize(font.len().next_multiple_of(4), 0);
    }
    font
}

/// Glyph `g`'s advance in thousandths of an em.
fn advance(program: &[u8], g: usize) -> f64 {
    let t = tables(program);
    let metrics = be16(t[b"hhea"], 34).max(1);
    be16(t[b"hmtx"], 4 * g.min(metrics - 1)) as f64 * 1000.0 / be16(t[b"head"], 18) as f64
}

/// A one-line page drawing `text` in a subset of `program` named `named`, as a producer writes it:
/// shaped, in visual order, each glyph at its shaped position, right-to-left text ending at
/// [`RIGHT`] and other text starting at [`LEFT`]. Saved and opened again, so it is a file like any
/// other.
fn producer_page(program: &[u8], named: &str, text: &str, layout: bool) -> Document {
    producer_page_with(program, named, text, |keep| subset(program, keep, layout))
}

/// [`producer_page`] embedding `embedded(glyphs shown)` as the subset.
fn producer_page_with(program: &[u8], named: &str, text: &str, embedded: impl FnOnce(&BTreeSet<usize>) -> Vec<u8>) -> Document {
    let shaped = pdfcraft_fonts::source_shaping::shape(program, text).unwrap();
    let mut unicode: BTreeMap<usize, String> = BTreeMap::new();
    for g in &shaped.glyphs {
        let entry = unicode.entry(g.id.to_u32() as usize).or_default();
        if entry.is_empty() {
            entry.push_str(&g.text);
        }
    }
    let x = if crate::bidi::rtl_base(text) { RIGHT - shaped.advance * SIZE } else { LEFT };
    let mut content = format!("BT /F1 {SIZE} Tf {x:.3} {BASELINE} Td ");
    // The pen in em from where the line starts, and the rise in force.
    let (mut pen, mut rise) = (0.0, 0.0);
    for g in &shaped.glyphs {
        let [at, up] = g.position;
        if up != rise {
            rise = up;
            let _ = write!(content, "{:.3} Ts ", up * SIZE);
        }
        let _ = write!(content, "[{:.3} <{:04X}>] TJ ", (pen - at) * 1000.0, g.id.to_u32());
        pen = at + advance(program, g.id.to_u32() as usize) / 1000.0;
    }
    content.push_str("ET");
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange\n",
    );
    let _ = writeln!(cmap, "{} beginbfchar", unicode.iter().filter(|(_, s)| !s.is_empty()).count());
    for (g, s) in unicode.iter().filter(|(_, s)| !s.is_empty()) {
        let hex: String = s.encode_utf16().map(|u| format!("{u:04X}")).collect();
        let _ = writeln!(cmap, "<{g:04X}> <{hex}>");
    }
    cmap.push_str("endbfchar endcmap CMapName currentdict /CMap defineresource pop end end");
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 200] /Contents 4 0 R >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut doc = Document::open(Arc::new(out)).unwrap();

    let keep: BTreeSet<usize> = unicode.keys().copied().collect();
    let file = doc.add(Object::Stream(Stream::flate(Dict::new(), &embedded(&keep))));
    let to_unicode = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));
    let widths: Vec<Object> = keep.iter().flat_map(|&g| [Object::Int(g as i64), Object::Array(vec![Object::Real(advance(program, g))])]).collect();
    let base = format!("ABCDEF+{named}");
    let descriptor = Dict::from_iter([
        (b"Type".to_vec(), Object::name("FontDescriptor")),
        (b"FontName".to_vec(), Object::name(&base)),
        (b"Flags".to_vec(), Object::Int(4)),
        (b"FontBBox".to_vec(), Object::Array([0, -250, 1000, 900].map(Object::Int).to_vec())),
        (b"ItalicAngle".to_vec(), Object::Int(0)),
        (b"Ascent".to_vec(), Object::Int(900)),
        (b"Descent".to_vec(), Object::Int(-250)),
        (b"CapHeight".to_vec(), Object::Int(700)),
        (b"StemV".to_vec(), Object::Int(80)),
        (b"FontFile2".to_vec(), Object::Ref(file)),
    ]);
    let system = Dict::from_iter([
        (b"Registry".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"Adobe".to_vec()))),
        (b"Ordering".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"Identity".to_vec()))),
        (b"Supplement".to_vec(), Object::Int(0)),
    ]);
    let descendant = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("CIDFontType2")),
        (b"BaseFont".to_vec(), Object::name(&base)),
        (b"CIDSystemInfo".to_vec(), Object::Dict(system)),
        (b"CIDToGIDMap".to_vec(), Object::name("Identity")),
        (b"DW".to_vec(), Object::Int(1000)),
        (b"W".to_vec(), Object::Array(widths)),
        (b"FontDescriptor".to_vec(), Object::Dict(descriptor)),
    ]);
    let font = Dict::from_iter([
        (b"Type".to_vec(), Object::name("Font")),
        (b"Subtype".to_vec(), Object::name("Type0")),
        (b"BaseFont".to_vec(), Object::name(&base)),
        (b"Encoding".to_vec(), Object::name("Identity-H")),
        (b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(descendant)])),
        (b"ToUnicode".to_vec(), Object::Ref(to_unicode)),
    ]);
    let font = doc.add(Object::Dict(font));
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let fonts = Dict::from_iter([(b"F1".to_vec(), Object::Ref(font))]);
    doc.update_dict(page, |d| d.set(b"Resources".to_vec(), Object::Dict(Dict::from_iter([(b"Font".to_vec(), Object::Dict(fonts))])))).unwrap();
    Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap()
}

/// The page saved (incrementally, as Save does) and opened again, with the bytes saved.
fn saved(doc: &Document) -> (Document, usize) {
    let bytes = write_incremental(doc, &SaveOptions::default()).unwrap();
    hayro_syntax::Pdf::new(bytes.clone()).expect("parses");
    let len = bytes.len();
    (Document::open(Arc::new(bytes)).unwrap(), len)
}

/// Which of the page's pixels (at 2 pixels a point) are ink, across the line's band, and the
/// band's width in pixels.
fn ink(doc: &Document) -> (usize, Vec<bool>) {
    let bytes = write_incremental(doc, &SaveOptions::default()).unwrap();
    let mut renderer = pdfcraft_render::PageRenderer::new(Arc::new(bytes), pdfcraft_render::RenderConfig::default());
    let page =
        renderer.render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Pixels, scale: 2.0, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    let (w, h) = (page.width as usize, page.height as usize);
    // The band from 0.5 em below the baseline to 1.2 em above it, in pixel rows from the top.
    let top = ((200.0 - BASELINE - 1.2 * SIZE) * 2.0) as usize;
    let bottom = (((200.0 - BASELINE + 0.5 * SIZE) * 2.0) as usize).min(h);
    let ink = (top..bottom)
        .flat_map(|y| (0..w).map(move |x| (y, x)))
        .map(|(y, x)| page.rgba[(y * w + x) * 4 + 1] < 128 && page.rgba[(y * w + x) * 4 + 3] > 128);
    (w, ink.collect())
}

/// The share of ink pixels in both drawings with no ink in the other within [`NEAR`] pixels, so
/// a glyph placed a fraction of a point apart still matches and a wrong or missing glyph doesn't.
fn difference((w, a): &(usize, Vec<bool>), (_, b): &(usize, Vec<bool>)) -> f64 {
    let rows = a.len() / w;
    let unmatched = |a: &[bool], b: &[bool]| {
        let near = |y: usize, x: usize| {
            (y.saturating_sub(NEAR)..(y + NEAR + 1).min(rows)).any(|y| (x.saturating_sub(NEAR)..(x + NEAR + 1).min(*w)).any(|x| b[y * w + x]))
        };
        (0..a.len()).filter(|&i| a[i] && !near(i / w, i % w)).count()
    };
    let total = a.iter().chain(b).filter(|i| **i).count();
    if total == 0 { 0.0 } else { (unmatched(a, b) + unmatched(b, a)) as f64 / total as f64 }
}

/// The page's content, its streams joined.
fn page_content(doc: &Document) -> Vec<u8> {
    let p = pdfcraft_model::pages(doc).swap_remove(0);
    let list = match &*doc.resolve(p.dict.get(b"Contents").unwrap()) {
        Object::Array(a) => a.clone(),
        _ => vec![p.dict.get(b"Contents").unwrap().clone()],
    };
    let mut content = Vec::new();
    for o in &list {
        if let Object::Stream(s) = &*doc.resolve(o) {
            content.extend(s.decoded().unwrap());
        }
    }
    content
}

/// The fonts the page's content selects.
fn fonts_shown(doc: &Document) -> BTreeSet<String> {
    pdfcraft_content::parse(&page_content(doc))
        .ops
        .iter()
        .filter(|o| o.is("Tf"))
        .filter_map(|o| o.name(0).map(|n| String::from_utf8_lossy(n).into_owned()))
        .collect()
}

fn face(family: &str) -> Option<&'static pdfcraft_fonts::CraftFont> {
    pdfcraft_fonts::CRAFT_FONTS.iter().find(|f| f.family == family && f.style == "Regular")
}

/// An Arabic paragraph edited into two rows keeps both right-aligned at the line's right edge.
#[test]
fn arabic_paragraph_rows_stay_right_aligned() {
    let Some(face) = face("Noto Sans Arabic") else {
        eprintln!("skipping: built without the craft-fonts Arabic face (set CRAFT_FONTS_DIR)");
        return;
    };
    let mut doc = producer_page(face.bytes, "ArialMT", "مرحبا بالعالم", true);
    replace_block(&mut doc, 0, 0, "مرحبا يا صديقي\nبالعالم").unwrap();
    let (reopened, _) = saved(&doc);
    let lines = crate::text::text_lines(&reopened, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["مرحبا يا صديقي", "بالعالم"]);
    for l in &lines {
        assert!((l.rect[2] - RIGHT).abs() < 1.0, "{:?} ends at {}, not {RIGHT}", l.text, l.rect[2]);
    }
}

/// Every case, each failure named, so one run shows them all. Arabic letters take their forms
/// from their neighbours, so a line edit, which redraws one character at a time, refuses them and
/// leaves the page as it was; the paragraph edit shapes them, with the bundled Arabic face when
/// the page's subset cannot draw every glyph its shaping needs.
#[test]
fn existing_cjk_and_right_to_left_text_edits_round_trip_draw_and_add_only_their_glyphs() {
    let mut failures = Vec::new();
    for case in CASES {
        let Some(face) = face(case.family) else {
            eprintln!("skipping {} as {}: built without its craft-fonts face (set CRAFT_FONTS_DIR)", case.family, case.named);
            continue;
        };
        let arabic = case.family == "Noto Sans Arabic";
        for layout in [true, false] {
            let label = format!("{} as {}{}", case.family, case.named, if layout { "" } else { ", outlines only" });
            let original = producer_page(face.bytes, case.named, case.original, layout);
            let (_, size) = saved(&original);
            let read: Vec<String> = text_blocks(&original, 0).unwrap().into_iter().map(|b| b.text).collect();
            if read != [case.original] {
                failures.push(format!("{label}: reads {read:?}, not {:?}", case.original));
            }
            for (what, typed) in [("kept", case.kept), ("added", case.added)] {
                let expected = ink(&producer_page(face.bytes, case.named, typed, true));
                for paragraph in [true, false] {
                    let how = if paragraph { "paragraph" } else { "line" };
                    let mut doc = original.clone();
                    let edit = if paragraph { replace_block(&mut doc, 0, 0, typed) } else { replace_line(&mut doc, 0, 0, typed) };
                    if arabic && !paragraph {
                        let unchanged = write_incremental(&doc, &SaveOptions::default()).unwrap()
                            == write_incremental(&original, &SaveOptions::default()).unwrap();
                        if !matches!(&edit, Err(crate::EditError::Invalid(m)) if m.contains("edit the paragraph")) || !unchanged {
                            failures.push(format!(
                                "{label}, {what} line: {edit:?}, page unchanged: {unchanged}; expected a refusal naming the paragraph edit"
                            ));
                        }
                        continue;
                    }
                    let edit = match edit {
                        Ok(edit) => edit,
                        Err(e) => {
                            failures.push(format!("{label}, {what} {how}: refused: {e}"));
                            continue;
                        }
                    };
                    let (reopened, after) = saved(&doc);
                    let read: Vec<String> = text_blocks(&reopened, 0).unwrap().into_iter().map(|b| b.text).collect();
                    if read != [typed] {
                        failures.push(format!("{label}, {what} {how}: reads back {read:?}, not {typed:?}"));
                    }
                    let drawn = difference(&ink(&reopened), &expected);
                    let grew = after.saturating_sub(size);
                    let shown = fonts_shown(&reopened);
                    eprintln!(
                        "{label}, {what} {how}: substituted {:?}, fonts {shown:?}, unmatched ink {drawn:.3}, file grew {grew} bytes",
                        edit.substituted
                    );
                    if drawn > DRAWN {
                        failures.push(format!("{label}, {what} {how}: {drawn:.3} of the ink differs from the typed text's"));
                    }
                    // Arabic stays in the page's subset where its program can shape it: with its
                    // layout tables.
                    if what == "kept" && (!arabic || layout) && (edit.substituted.is_some() || shown != BTreeSet::from(["F1".to_string()])) {
                        failures.push(format!("{label}, kept {how}: left the page's subset ({:?}, fonts {shown:?})", edit.substituted));
                    }
                    // Without them it can't: the bundled face draws the whole run.
                    let in_subset = glyphs_shown(&reopened).iter().any(|(font, _)| font == "F1");
                    let bundled = pdfcraft_fonts::document_arabic_font().map(|f| f.family);
                    if arabic && !layout && (edit.substituted.as_deref() != bundled || in_subset) {
                        failures.push(format!("{label}, {what} {how}: not all in the bundled face ({:?}, in F1: {in_subset})", edit.substituted));
                    }
                    // Outlines for the glyphs typed at most, never a whole face (megabytes): a
                    // glyph of the calligraphic Chinese face takes about 10 KB.
                    if grew > GLYPH_BYTES * typed.chars().count() {
                        failures.push(format!("{label}, {what} {how}: the file grew {grew} bytes for {} characters", typed.chars().count()));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

/// Arabic with lam-alef ([`lam_alef`]) and marks: [`KEPT`] shows only the glyphs [`ORIGINAL`]
/// does, in other places; [`TYPED_NEW`] adds a letter (noon) it doesn't have.
const ORIGINAL: &str = "السَّلامُ عليكم";
const KEPT: &str = "عليكم السَّلامُ";
const TYPED_NEW: &str = "عليكم السَّلامُ نعم";

/// The glyphs `face` draws a medial lam-alef with, in visual order (alef, then lam): Noto Sans
/// Arabic's required ligature is a pair of forms that lam and alef take nowhere else.
fn lam_alef(face: &pdfcraft_fonts::CraftFont) -> [u32; 2] {
    let ids =
        |text: &str| -> Vec<u32> { pdfcraft_fonts::source_shaping::shape(face.bytes, text).unwrap().glyphs.iter().map(|g| g.id.to_u32()).collect() };
    let (pair, plain) = (ids("سلا"), ids("سلم"));
    assert_ne!(pair[1], plain[1], "lam takes a form of its own before alef");
    [pair[0], pair[1]]
}

/// The glyph `program`'s Unicode BMP (format 4) cmap gives `c`.
pub(crate) fn nominal(program: &[u8], c: char) -> u16 {
    let cmap = tables(program)[b"cmap"];
    let record = (0..be16(cmap, 2)).map(|i| 4 + 8 * i).find(|&r| matches!((be16(cmap, r), be16(cmap, r + 2)), (3, 1) | (0, 3))).unwrap();
    let t = &cmap[be32(cmap, record + 4)..];
    assert_eq!(be16(t, 0), 4);
    let (segments, c) = (be16(t, 6) / 2, c as usize);
    let (ends, starts, deltas, offsets) = (14, 16 + 2 * segments, 16 + 4 * segments, 16 + 6 * segments);
    let i = (0..segments).find(|&i| be16(t, ends + 2 * i) >= c).unwrap();
    let start = be16(t, starts + 2 * i);
    assert!(start <= c, "U+{c:04X} is not in the cmap");
    let offset = be16(t, offsets + 2 * i);
    let glyph = if offset == 0 { c } else { be16(t, offsets + 2 * i + offset + 2 * (c - start)) };
    ((glyph + be16(t, deltas + 2 * i)) & 0xFFFF) as u16
}

/// A cmap mapping only `chars`, each to its glyph in `program`, as a subsetter that prunes the
/// cmap writes it.
fn pruned_cmap(program: &[u8], chars: &BTreeSet<char>) -> Vec<u8> {
    cmap_table(&chars.iter().map(|&c| (c, nominal(program, c))).collect())
}

/// A cmap mapping each of `glyphs`' characters to its glyph.
fn cmap_table(glyphs: &BTreeMap<char, u16>) -> Vec<u8> {
    // One segment a character, then the closing one, which maps U+FFFF to glyph 0.
    let mut segments: Vec<(u16, u16)> = glyphs.iter().map(|(&c, &glyph)| (c as u16, glyph.wrapping_sub(c as u16))).collect();
    segments.push((0xFFFF, 1));
    let n = segments.len() as u16;
    let range = 2 * (1u16 << (15 - n.leading_zeros()));
    let mut table = Vec::new();
    for v in [4, 16 + 8 * n, 0, 2 * n, range, 15 - n.leading_zeros() as u16, 2 * n - range] {
        table.extend_from_slice(&v.to_be_bytes());
    }
    table.extend(segments.iter().flat_map(|(c, _)| c.to_be_bytes()));
    table.extend_from_slice(&[0, 0]);
    table.extend(segments.iter().flat_map(|(c, _)| c.to_be_bytes()));
    table.extend(segments.iter().flat_map(|(_, delta)| delta.to_be_bytes()));
    table.extend(segments.iter().flat_map(|_| [0, 0]));
    let mut cmap: Vec<u8> = [0u16, 1, 3, 1].iter().flat_map(|v| v.to_be_bytes()).collect();
    cmap.extend_from_slice(&12u32.to_be_bytes());
    cmap.extend(table);
    cmap
}

/// `font` with its `tag` table replaced by `data`.
fn with_table(font: &[u8], tag: &[u8; 4], data: Vec<u8>) -> Vec<u8> {
    let out: Vec<([u8; 4], Vec<u8>)> = tables(font).into_iter().map(|(t, d)| (t, if &t == tag { data.clone() } else { d.to_vec() })).collect();
    assemble(&out)
}

/// Each glyph code the page's content shows, in order, with the font resource it is shown in.
fn glyphs_shown(doc: &Document) -> Vec<(String, u16)> {
    let (mut font, mut shown) = (String::new(), Vec::new());
    for op in pdfcraft_content::parse(&page_content(doc)).ops {
        if op.is("Tf") {
            font = String::from_utf8_lossy(op.name(0).unwrap()).into_owned();
        }
        let strings: Vec<&Object> = if op.is("Tj") || op.is("'") {
            op.operands.iter().collect()
        } else if op.is("TJ") {
            op.operands.first().and_then(Object::as_array).map(|a| a.iter().collect()).unwrap_or_default()
        } else {
            Vec::new()
        };
        for s in strings.into_iter().filter_map(Object::as_string) {
            shown.extend(s.bytes.chunks(2).map(|c| (font.clone(), u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))));
        }
    }
    shown
}

/// Edits `page`'s paragraph to `typed` and checks it reads back as typed and draws as the face
/// draws it, all of it in the bundled Arabic outlines: nothing in the page's subset, `F1`.
fn falls_back_for_the_run(face: &pdfcraft_fonts::CraftFont, mut page: Document, typed: &str) {
    let edit = replace_block(&mut page, 0, 0, typed).unwrap();
    let (reopened, _) = saved(&page);
    let read: Vec<String> = text_blocks(&reopened, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(read, [typed]);
    assert_eq!(edit.substituted.as_deref(), pdfcraft_fonts::document_arabic_font().map(|f| f.family));
    let shown = glyphs_shown(&reopened);
    assert!(!shown.iter().any(|(font, _)| font == "F1"), "the subset still draws part of the run: {shown:?}");
    let drawn = difference(&ink(&reopened), &ink(&producer_page(face.bytes, "NotoSansArabic", typed, true)));
    assert!(drawn <= DRAWN, "{drawn:.3} of the ink differs from the typed text's");
}

/// The craft-fonts Arabic face; `None`, saying the test is skipped, in a build without it.
fn arabic_face() -> Option<&'static pdfcraft_fonts::CraftFont> {
    let found = face("Noto Sans Arabic");
    if found.is_none() {
        eprintln!("skipping: built without the craft-fonts Arabic face (set CRAFT_FONTS_DIR)");
    }
    found
}

/// Kept Arabic in a subset whose program keeps its cmap and layout tables stays in the page's
/// font: the glyphs the face's shaping gives (its joining forms, lam-alef, the marks) in visual
/// order, shown in the page's own resource, nothing substituted.
#[test]
fn kept_arabic_stays_in_its_subset_with_the_faces_glyphs() {
    let Some(face) = arabic_face() else { return };
    let shaped = pdfcraft_fonts::source_shaping::shape(face.bytes, KEPT).unwrap();
    let ids: Vec<u32> = shaped.glyphs.iter().map(|g| g.id.to_u32()).collect();
    assert!(ids.windows(2).any(|w| w == lam_alef(face)), "the kept text has lam-alef");
    assert!(shaped.glyphs.iter().any(|g| g.text.is_empty()), "the marks are glyphs of their own");
    let mut doc = producer_page(face.bytes, "NotoSansArabic", ORIGINAL, true);
    let edit = replace_block(&mut doc, 0, 0, KEPT).unwrap();
    assert_eq!(edit.substituted, None);
    let (reopened, _) = saved(&doc);
    let read: Vec<String> = text_blocks(&reopened, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(read, [KEPT]);
    let expected: Vec<(String, u16)> = shaped.glyphs.iter().map(|g| ("F1".to_string(), g.id.to_u32() as u16)).collect();
    assert_eq!(glyphs_shown(&reopened), expected);
    let drawn = difference(&ink(&reopened), &ink(&producer_page(face.bytes, "NotoSansArabic", KEPT, true)));
    assert!(drawn <= DRAWN, "{drawn:.3} of the ink differs from the typed text's");
}

/// A typed letter the subset's cmap doesn't map: the whole run is drawn in the bundled face.
#[test]
fn arabic_letter_the_subset_lacks_falls_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    assert!(TYPED_NEW.chars().any(|c| !ORIGINAL.contains(c)));
    let chars: BTreeSet<char> = ORIGINAL.chars().collect();
    let page = producer_page_with(face.bytes, "NotoSansArabic", ORIGINAL, |keep| {
        with_table(&subset(face.bytes, keep, true), b"cmap", pruned_cmap(face.bytes, &chars))
    });
    falls_back_for_the_run(face, page, TYPED_NEW);
}

/// A typed letter whose glyphs the subset emptied, though its cmap and layout tables still reach
/// them: the whole run is drawn in the bundled face.
#[test]
fn arabic_letter_the_subset_emptied_falls_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    falls_back_for_the_run(face, producer_page(face.bytes, "NotoSansArabic", ORIGINAL, true), TYPED_NEW);
}

/// Kept text one of whose glyphs (lam-alef's alef) the subset emptied, though the page's
/// ToUnicode still names it: the whole run is drawn in the bundled face.
#[test]
fn kept_arabic_with_an_emptied_glyph_falls_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    let alef = lam_alef(face)[0] as usize;
    let page = producer_page_with(face.bytes, "NotoSansArabic", ORIGINAL, |keep| {
        let mut keep = keep.clone();
        assert!(keep.remove(&alef), "the page shows lam-alef");
        subset(face.bytes, &keep, true)
    });
    falls_back_for_the_run(face, page, KEPT);
}

/// A GSUB with no scripts, features or lookups: version 1.0 and 3 empty lists.
const EMPTY_GSUB: [u8; 16] = [0, 1, 0, 0, 0, 10, 0, 12, 0, 14, 0, 0, 0, 0, 0, 0];

/// A subset with its layout tables but no cmap: the editor's stand-in cmap, rebuilt from the
/// page's ToUnicode, maps each letter to a form the page showed (kaf to its medial form, yeh to
/// its dots), which the GSUB doesn't substitute. Typed at a word's start, kaf would be drawn
/// medial in the page's font. The whole run is drawn in the bundled face.
#[test]
fn kept_arabic_in_a_subset_without_its_own_cmap_falls_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    let page = producer_page_with(face.bytes, "NotoSansArabic", ORIGINAL, |keep| {
        let out: Vec<([u8; 4], Vec<u8>)> =
            tables(&subset(face.bytes, keep, true)).into_iter().filter(|(t, _)| t != b"cmap").map(|(t, d)| (t, d.to_vec())).collect();
        assemble(&out)
    });
    falls_back_for_the_run(face, page, "كم عليكم");
}

/// A program whose own cmap maps kaf to its medial form, as a producer that writes the cmap from
/// the glyphs it showed does, with every glyph and the layout tables kept: the joining lookups
/// cover nominal kaf, not the medial form, so kaf at a word's start would be drawn in a wrong
/// form in the page's font (a dotless final kaf, where meem follows), every glyph inked. The
/// whole run is drawn in the bundled face.
#[test]
fn kept_arabic_in_a_subset_whose_cmap_maps_a_letter_to_a_joining_form_falls_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    // The middle cluster's glyph: the final kaf draws 2 (a dotless form and a mark).
    let kafs = pdfcraft_fonts::source_shaping::shape(face.bytes, "ككك").unwrap();
    let medial = kafs.glyphs.iter().filter(|g| !g.text.is_empty()).nth(1).unwrap().id.to_u32() as u16;
    assert_ne!(medial, nominal(face.bytes, 'ك'));
    let shown = pdfcraft_fonts::source_shaping::shape(face.bytes, "عليكم").unwrap();
    assert!(shown.glyphs.iter().any(|g| g.id.to_u32() == u32::from(medial)), "the page shows medial kaf");
    let glyphs: BTreeMap<char, u16> = ORIGINAL.chars().map(|c| (c, if c == 'ك' { medial } else { nominal(face.bytes, c) })).collect();
    let every: BTreeSet<usize> = (0..be16(tables(face.bytes)[b"maxp"], 4)).collect();
    let page =
        producer_page_with(face.bytes, "NotoSansArabic", ORIGINAL, |_| with_table(&subset(face.bytes, &every, true), b"cmap", cmap_table(&glyphs)));
    falls_back_for_the_run(face, page, "كم عليكم");
}

/// Edits kept text on a page whose subset kept the isolated forms, each with ink, and has `gsub`
/// for its GSUB, under which every letter stays isolated: the whole run must be drawn in the
/// bundled face.
fn isolated_only_falls_back_for_the_run(face: &pdfcraft_fonts::CraftFont, gsub: Vec<u8>) {
    let page = producer_page_with(face.bytes, "NotoSansArabic", ORIGINAL, |keep| {
        let keep: BTreeSet<usize> = keep.iter().copied().chain(ORIGINAL.chars().map(|c| usize::from(nominal(face.bytes, c)))).collect();
        with_table(&subset(face.bytes, &keep, true), b"GSUB", gsub)
    });
    falls_back_for_the_run(face, page, KEPT);
}

/// Joining is judged against the bundled face, so a letter it doesn't draw itself never keeps a
/// page's program: here a judge with an empty cmap.
#[test]
fn joining_is_judged_only_for_letters_the_bundled_face_draws() {
    let Some(face) = arabic_face() else { return };
    let blind = with_table(face.bytes, b"cmap", cmap_table(&BTreeMap::new()));
    assert!(pdfcraft_fonts::source_shaping::joins_arabic(face.bytes, face.bytes, KEPT));
    assert!(!pdfcraft_fonts::source_shaping::joins_arabic(face.bytes, &blind, KEPT));
}

/// The space glyph, which has no ink, stands only for white space and default-ignorable
/// characters: a cluster of a letter a cmap sends to it would draw nothing, so its shaping is
/// refused.
#[test]
fn the_space_glyph_never_stands_for_a_letter() {
    use pdfcraft_fonts::source_shaping::{ShapedRun, draws_every_glyph, shape};
    let Some(face) = arabic_face() else { return };
    let space = shape(face.bytes, " ").unwrap().glyphs.remove(0);
    let run =
        |text: &str| ShapedRun { glyphs: vec![pdfcraft_fonts::source_shaping::ShapedGlyph { text: text.into(), ..space.clone() }], advance: 0.0 };
    assert!(draws_every_glyph(face.bytes, &run(" ")));
    assert!(draws_every_glyph(face.bytes, &run("\u{200d}")));
    assert!(!draws_every_glyph(face.bytes, &run("ك")));
    // A joiner shares its letter's cluster: the space glyph may stand for the joiner, not both.
    assert!(!draws_every_glyph(face.bytes, &run("ك\u{200d}")));
    // Kaf and a non-joiner as the shaper draws them pass; their cluster with only space glyphs
    // doesn't.
    let joined = shape(face.bytes, "ك\u{200c}").unwrap();
    assert!(draws_every_glyph(face.bytes, &joined));
    let mut blanked = joined.clone();
    for glyph in &mut blanked.glyphs {
        glyph.id = space.id;
    }
    assert!(!draws_every_glyph(face.bytes, &blanked));
}

/// A subset that kept its cmap, GSUB and every outline but dropped its GPOS: the marks of the
/// kept text would be placed by guesswork. The whole run is drawn in the bundled face.
#[test]
fn kept_arabic_marks_in_a_subset_without_mark_positioning_fall_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    assert!(KEPT.chars().any(|c| matches!(u32::from(c), 0x064b..=0x065f)), "the kept text has marks");
    let page = producer_page_with(face.bytes, "NotoSansArabic", ORIGINAL, |keep| subset_without_gpos(face, keep));
    falls_back_for_the_run(face, page, KEPT);
}

/// The face's subset of the glyphs in `keep`, with its cmap, GSUB and GDEF but no GPOS.
fn subset_without_gpos(face: &pdfcraft_fonts::CraftFont, keep: &BTreeSet<usize>) -> Vec<u8> {
    let out: Vec<([u8; 4], Vec<u8>)> =
        tables(&subset(face.bytes, keep, true)).into_iter().filter(|(t, _)| t != b"GPOS").map(|(t, d)| (t, d.to_vec())).collect();
    assemble(&out)
}

/// Unmarked text whose letters the face draws as a base and dots of their own (yeh), in a subset
/// that dropped its GPOS: no character is a mark, but the dots the GSUB makes would sit where they
/// fall. The whole run is drawn in the bundled face.
#[test]
fn unmarked_arabic_drawn_with_dot_glyphs_in_a_subset_without_mark_positioning_falls_back_for_the_run() {
    const UNMARKED: &str = "عليكم";
    let Some(face) = arabic_face() else { return };
    assert!(!UNMARKED.chars().any(|c| matches!(u32::from(c), 0x0610..=0x061a | 0x064b..=0x065f | 0x0670)), "no character is a mark");
    let shaped = pdfcraft_fonts::source_shaping::shape(face.bytes, UNMARKED).unwrap();
    assert!(shaped.glyphs.iter().any(|g| g.text.is_empty()), "the face draws a letter as a base and its dots");
    let page = producer_page_with(face.bytes, "NotoSansArabic", ORIGINAL, |keep| subset_without_gpos(face, keep));
    falls_back_for_the_run(face, page, UNMARKED);
}

/// `gpos` with the lookup lists of its `features` emptied, and how many it emptied: the features
/// are still selected but place nothing.
fn without_lookups(gpos: &[u8], features: &[&[u8; 4]]) -> (Vec<u8>, usize) {
    let (mut out, list, mut emptied) = (gpos.to_vec(), be16(gpos, 6), 0);
    for record in (0..be16(gpos, list)).map(|i| list + 2 + 6 * i) {
        if features.iter().any(|f| &gpos[record..record + 4] == *f) {
            let count = list + be16(gpos, record + 4) + 2;
            emptied += usize::from(be16(gpos, count) > 0);
            out[count..count + 2].copy_from_slice(&[0, 0]);
        }
    }
    (out, emptied)
}

/// Edits kept text on a page whose subset has the lookups of its GPOS `features` emptied: the
/// whole run must be drawn in the bundled face.
fn emptied_lookups_fall_back_for_the_run(face: &pdfcraft_fonts::CraftFont, features: &[&[u8; 4]]) {
    let page = producer_page_with(face.bytes, "NotoSansArabic", ORIGINAL, |keep| {
        let program = subset(face.bytes, keep, true);
        let (gpos, emptied) = without_lookups(tables(&program)[b"GPOS"], features);
        assert!(emptied >= features.len(), "the subset's GPOS has lookups for {features:?}");
        with_table(&program, b"GPOS", gpos)
    });
    falls_back_for_the_run(face, page, KEPT);
}

/// A subset whose GPOS keeps its `mark` and `mkmk` features, but with no lookups: their names
/// alone place no mark. The whole run is drawn in the bundled face.
#[test]
fn kept_arabic_marks_in_a_subset_whose_mark_features_have_no_lookups_fall_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    emptied_lookups_fall_back_for_the_run(face, &[b"mark", b"mkmk"]);
}

/// A subset whose `mark` lookups work but whose `mkmk` lookups are empty: the fatha over the
/// shadda in the kept text would be attached to the base's anchor, on top of the shadda, though
/// both marks move. The whole run is drawn in the bundled face.
#[test]
fn kept_arabic_stacked_marks_in_a_subset_without_mark_to_mark_lookups_fall_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    assert!(KEPT.contains("\u{064e}\u{0651}"), "the kept text has fatha and shadda on one letter, which the shaper stacks");
    emptied_lookups_fall_back_for_the_run(face, &[b"mkmk"]);
}

/// The kept text with sukun added over seen's fatha and shadda: the bundled face stacks all 3
/// through `mkmk`, fatha on shadda and sukun on fatha.
const STACKED: &str = "عليكم ال\u{0633}\u{064e}\u{0651}\u{0652}لامُ";

/// The coverage index of `glyph` in the coverage table at `at`, if it is covered.
fn coverage_index(table: &[u8], at: usize, glyph: usize) -> Option<usize> {
    match be16(table, at) {
        1 => (0..be16(table, at + 2)).find(|&i| be16(table, at + 4 + 2 * i) == glyph),
        2 => (0..be16(table, at + 2))
            .map(|i| at + 4 + 6 * i)
            .find(|&r| (be16(table, r)..=be16(table, r + 2)).contains(&glyph))
            .map(|r| be16(table, r + 4) + glyph - be16(table, r)),
        _ => None,
    }
}

/// `gpos` with every anchor that lets a mark attach to `glyph` as the earlier mark (its Mark2Array
/// records in the mark-to-mark subtables of `mkmk`'s lookups) set to none, and how many it cleared:
/// marks still stack on other marks, but nothing stacks on `glyph`, as in a subset that kept only
/// the mark pairs a page showed.
fn without_mark_to_mark_onto(gpos: &[u8], glyph: usize) -> (Vec<u8>, usize) {
    let (mut out, features, lookups, mut cleared) = (gpos.to_vec(), be16(gpos, 6), be16(gpos, 8), 0);
    let mut subtables = Vec::new();
    for record in (0..be16(gpos, features)).map(|i| features + 2 + 6 * i).filter(|&r| &gpos[r..r + 4] == b"mkmk") {
        let feature = features + be16(gpos, record + 4);
        for index in (0..be16(gpos, feature + 2)).map(|i| be16(gpos, feature + 4 + 2 * i)) {
            let lookup = lookups + be16(gpos, lookups + 2 + 2 * index);
            for at in (0..be16(gpos, lookup + 4)).map(|i| lookup + be16(gpos, lookup + 6 + 2 * i)) {
                match be16(gpos, lookup) {
                    6 => subtables.push(at),
                    9 if be16(gpos, at + 2) == 6 => subtables.push(at + be32(gpos, at + 4)),
                    _ => {}
                }
            }
        }
    }
    for at in subtables.into_iter().filter(|&at| be16(gpos, at) == 1) {
        let (classes, array) = (be16(gpos, at + 6), at + be16(gpos, at + 10));
        if let Some(index) = coverage_index(gpos, at + be16(gpos, at + 4), glyph) {
            let anchors = array + 2 + 2 * classes * index;
            cleared += (0..classes).filter(|c| be16(gpos, anchors + 2 * c) != 0).count();
            out[anchors..anchors + 2 * classes].fill(0);
        }
    }
    (out, cleared)
}

/// A subset whose mark-to-mark data stacks fatha on shadda but nothing on fatha: the sukun the edit
/// adds would be attached to seen's own anchor, through the shadda, though both other marks are
/// stacked. The whole run is drawn in the bundled face; the same subset with its GPOS intact keeps
/// the run.
#[test]
fn kept_arabic_with_a_mark_stack_the_subset_cuts_short_falls_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    let (fatha, sukun) = (usize::from(nominal(face.bytes, '\u{064e}')), usize::from(nominal(face.bytes, '\u{0652}')));
    let page = |cut: bool| {
        producer_page_with(face.bytes, "NotoSansArabic", ORIGINAL, |keep| {
            let program = subset(face.bytes, &keep.iter().copied().chain([sukun]).collect(), true);
            if !cut {
                return program;
            }
            let (gpos, cleared) = without_mark_to_mark_onto(tables(&program)[b"GPOS"], fatha);
            assert!(cleared > 0, "the subset's GPOS stacks marks on fatha");
            with_table(&program, b"GPOS", gpos)
        })
    };
    let mut intact = page(false);
    assert_eq!(replace_block(&mut intact, 0, 0, STACKED).unwrap().substituted, None, "the intact subset stacks the 3 marks itself");
    falls_back_for_the_run(face, page(true), STACKED);
}

/// Overwrites the coverage table at `at` with a format 1 list of `glyphs` (sorted), where the
/// table has room for it.
fn write_coverage(table: &mut [u8], at: usize, glyphs: &[u16]) {
    let room = match be16(table, at) {
        1 => 4 + 2 * be16(table, at + 2),
        2 => 4 + 6 * be16(table, at + 2),
        format => panic!("coverage format {format}"),
    };
    assert!(room >= 4 + 2 * glyphs.len(), "room for {} glyphs", glyphs.len());
    let words = [1, u16::try_from(glyphs.len()).unwrap()].into_iter().chain(glyphs.iter().copied());
    for (i, word) in words.enumerate() {
        table[at + 2 * i..at + 2 * i + 2].copy_from_slice(&word.to_be_bytes());
    }
}

/// `program` (the bundled face or a subset keeping its glyph ids) with its last `mkmk` lookup, a
/// mark-to-mark lookup with a mark filtering set, made to put sukun on shadda, its filtering set
/// holding only those 2. It runs after the lookup that stacks fatha on shadda and sukun on fatha,
/// so with shadda, fatha and sukun on a letter the sukun skips the fatha and lands on the shadda,
/// through the fatha, still moved by `mkmk`, as are the other marks.
fn sukun_on_shadda(program: &[u8], shadda: u16, sukun: u16) -> Vec<u8> {
    let mut gpos = tables(program)[b"GPOS"].to_vec();
    let (features, lookups) = (be16(&gpos, 6), be16(&gpos, 8));
    let record = (0..be16(&gpos, features)).map(|i| features + 2 + 6 * i).find(|&r| &gpos[r..r + 4] == b"mkmk").unwrap();
    let feature = features + be16(&gpos, record + 4);
    let last = be16(&gpos, feature + 4 + 2 * (be16(&gpos, feature + 2) - 1));
    let lookup = lookups + be16(&gpos, lookups + 2 + 2 * last);
    let (kind, flag, count) = (be16(&gpos, lookup), be16(&gpos, lookup + 2), be16(&gpos, lookup + 4));
    assert_eq!((flag & 0x10, count), (0x10, 1), "one subtable and a mark filtering set");
    let mut subtable = lookup + be16(&gpos, lookup + 6);
    if kind == 9 {
        subtable += be32(&gpos, subtable + 4);
    }
    assert_eq!((be16(&gpos, subtable), be16(&gpos, subtable + 6)), (1, 1), "mark-to-mark, one class");
    let (mark1, mark2) = (subtable + be16(&gpos, subtable + 2), subtable + be16(&gpos, subtable + 4));
    let (mark1_array, mark2_array) = (subtable + be16(&gpos, subtable + 8), subtable + be16(&gpos, subtable + 10));
    assert_eq!(be16(&gpos, mark1_array + 2), 0, "the first mark's class");
    assert_ne!(be16(&gpos, mark2_array + 2), 0, "the first earlier mark's anchor");
    write_coverage(&mut gpos, mark1, &[sukun]);
    write_coverage(&mut gpos, mark2, &[shadda]);
    let mut gdef = tables(program)[b"GDEF"].to_vec();
    assert!(be32(&gdef, 0) >= 0x0001_0002, "GDEF with mark glyph sets");
    let sets = be16(&gdef, 12);
    let set = be16(&gpos, lookup + 6 + 2 * count);
    let coverage = sets + be32(&gdef, sets + 4 + 4 * set);
    write_coverage(&mut gdef, coverage, &[shadda.min(sukun), shadda.max(sukun)]);
    with_table(&with_table(program, b"GPOS", gpos), b"GDEF", gdef)
}

/// A page's font whose mark-to-mark lookup puts the third of 3 stacked marks on the first, skipping
/// the second (its flags allow that): every mark is still moved by `mkmk`, so the count of stacked
/// marks matches the bundled face's, but the sukun is drawn through the fatha. The run is drawn in
/// the bundled face, which keeps its own marks; the same subset without the change keeps the run.
#[test]
fn a_page_font_that_stacks_a_mark_on_the_wrong_mark_falls_back_for_the_run() {
    use pdfcraft_fonts::source_shaping::joins_arabic;
    let Some(face) = arabic_face() else { return };
    let (shadda, sukun) = (nominal(face.bytes, '\u{0651}'), nominal(face.bytes, '\u{0652}'));
    let skipping = sukun_on_shadda(face.bytes, shadda, sukun);
    assert!(joins_arabic(face.bytes, face.bytes, STACKED));
    assert!(!joins_arabic(&skipping, face.bytes, STACKED));
    let page = |skip: bool| {
        producer_page_with(face.bytes, "NotoSansArabic", ORIGINAL, |keep| {
            let program = subset(face.bytes, &keep.iter().copied().chain([usize::from(sukun)]).collect(), true);
            if skip { sukun_on_shadda(&program, shadda, sukun) } else { program }
        })
    };
    let mut intact = page(false);
    assert_eq!(replace_block(&mut intact, 0, 0, STACKED).unwrap().substituted, None, "the intact subset stacks the 3 marks itself");
    falls_back_for_the_run(face, page(true), STACKED);
}

/// A page's font that splits a letter into more glyphs than the bundled face can meet the count of
/// stacked marks with a glyph the bundled face has no counterpart for. Here the bundled face's
/// stand-in, without `ccmp`, draws sheen whole and stacks fatha on shadda (1 glyph moved by
/// `mkmk`); the page's font draws sheen as seen and 3 dots, stacks shadda on the dots, and can't
/// stack anything on shadda, so its fatha would be drawn through the shadda, though it too has 1
/// glyph moved. It must not keep the text.
#[test]
fn a_page_font_that_splits_a_stacked_letter_differently_is_refused() {
    use pdfcraft_fonts::source_shaping::joins_arabic;
    const SHEEN: &str = "\u{0634}\u{064e}\u{0651}";
    let Some(face) = arabic_face() else { return };
    let (gsub, emptied) = without_lookups(tables(face.bytes)[b"GSUB"], &[b"ccmp"]);
    assert!(emptied > 0, "the bundled face decomposes letters with ccmp");
    let whole = with_table(face.bytes, b"GSUB", gsub);
    let (gpos, cleared) = without_mark_to_mark_onto(tables(face.bytes)[b"GPOS"], usize::from(nominal(face.bytes, '\u{0651}')));
    assert!(cleared > 0, "the bundled face stacks marks on shadda");
    let cut = with_table(face.bytes, b"GPOS", gpos);
    // The split alone is no reason to refuse: with one mark, nothing is stacked by the judge.
    assert!(joins_arabic(face.bytes, &whole, "\u{0634}\u{064e}"));
    assert!(joins_arabic(face.bytes, face.bytes, SHEEN));
    assert!(!joins_arabic(&cut, &whole, SHEEN));
}

/// A combining mark that no Arabic letter carries in its joining run is in no run the checks
/// shape, so its placement goes unjudged: the span never keeps the page's font, even the
/// complete face, whose marks after letters it keeps.
#[test]
fn a_mark_outside_an_arabic_letters_run_is_refused() {
    use pdfcraft_fonts::source_shaping::joins_arabic;
    let Some(face) = arabic_face() else { return };
    assert!(joins_arabic(face.bytes, face.bytes, KEPT));
    assert!(joins_arabic(face.bytes, face.bytes, "عليكم 1 a"));
    // A Quranic mark written after a space opens a run too: refused, though a GPOS-placed mark
    // written straight after the word is kept (the bundled face leaves U+06DB itself unmoved by
    // GPOS, so even straight after a word it falls back).
    for text in
        ["عليكم 1\u{0301}", "عليكم a\u{0301}", "ع\u{0301}ليكم", "1\u{064e} عليكم", "\u{064e}عليكم", "لَا رَيْبَ \u{06db} فِيهِ", "لَا رَيْبَ \u{06e2} فِيهِ"]
    {
        assert!(!joins_arabic(face.bytes, face.bytes, text), "{text:?}");
    }
    assert!(joins_arabic(face.bytes, face.bytes, "لَا رَيْبَ\u{06e2} فِيهِ"));
}

/// A combining mark after ZWJ (`U+200D`) is not positioned by the bundled face's GPOS (`on == off`
/// offsets), so `joins_arabic` refusing `"ب\u{200d}\u{064e}"` is intentional fail-closed behaviour
/// whilst `"ب\u{064e}\u{200d}"` (mark before ZWJ) passes.
#[test]
fn arabic_mark_after_zwj_is_unpositioned_and_refused() {
    use pdfcraft_fonts::source_shaping::{joins_arabic, shape};
    let Some(face) = arabic_face() else { return };
    assert!(joins_arabic(face.bytes, face.bytes, "ب\u{064e}"));
    assert!(joins_arabic(face.bytes, face.bytes, "ب\u{064e}\u{200d}"));
    assert!(!joins_arabic(face.bytes, face.bytes, "ب\u{200d}\u{064e}"));
    let shaped = shape(face.bytes, "ب\u{200d}\u{064e}").unwrap();
    let fatha = nominal(face.bytes, '\u{064e}');
    let placed = shaped.glyphs.iter().find(|g| g.id.to_u32() == u32::from(fatha)).unwrap();
    assert_eq!(placed.position, [0.0, 0.0], "bundled face leaves fatha after ZWJ unpositioned: {placed:?}");
}

/// Appends a `PairPos` (`mark`) lookup adjusting `second`'s `x_offset` when preceded by `first`.
fn add_mark_pair_pos(gpos: &[u8], first: u16, second: u16) -> Vec<u8> {
    let (script_off, feat_off, look_off) = (be16(gpos, 4), be16(gpos, 6), be16(gpos, 8));
    let lcount = be16(gpos, look_off);
    let insert_at = look_off + 2 + 2 * lcount;
    let mut out = Vec::with_capacity(gpos.len() + 128);
    out.extend_from_slice(&gpos[..insert_at]);
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&gpos[insert_at..]);
    let write16 = |buf: &mut [u8], at: usize, val: usize| buf[at..at + 2].copy_from_slice(&u16::try_from(val).unwrap().to_be_bytes());
    write16(&mut out, look_off, lcount + 1);
    if script_off >= insert_at {
        write16(&mut out, 4, script_off + 2);
    }
    if feat_off >= insert_at {
        write16(&mut out, 6, feat_off + 2);
    }
    for i in 0..lcount {
        let off = be16(&out, look_off + 2 + 2 * i);
        if look_off + off >= insert_at {
            write16(&mut out, look_off + 2 + 2 * i, off + 2);
        }
    }
    let new_lookup_abs = out.len();
    write16(&mut out, look_off + 2 + 2 * lcount, new_lookup_abs - look_off);
    for w in [2u16, 0, 1, 8, 1, 12, 0, 1, 1, 18, 1, 1, first, 1, second, 1] {
        out.extend_from_slice(&w.to_be_bytes());
    }
    let feat_base = be16(&out, 6);
    for i in 0..be16(&out, feat_base) {
        let rec = feat_base + 2 + 6 * i;
        if &out[rec..rec + 4] == b"mark" {
            let old_f = feat_base + be16(&out, rec + 4);
            let n = be16(&out, old_f + 2);
            let new_f_abs = out.len();
            out.extend_from_within(old_f..old_f + 2);
            out.extend_from_slice(&u16::try_from(n + 1).unwrap().to_be_bytes());
            out.extend_from_within(old_f + 4..old_f + 4 + 2 * n);
            out.extend_from_slice(&u16::try_from(lcount).unwrap().to_be_bytes());
            write16(&mut out, rec + 4, new_f_abs - feat_base);
        }
    }
    out
}

/// An altered GPOS program that re-attaches `sukun` onto `shadda` instead of `fatha` while
/// spoofing a `(fatha, sukun)` positional dependency through a `PairPos` lookup in `mark` must be
/// refused by `joins_arabic`.
#[test]
fn a_page_font_that_spoofs_stacked_mark_dependency_via_pair_pos_is_refused() {
    use pdfcraft_fonts::source_shaping::joins_arabic;
    let Some(face) = arabic_face() else { return };
    let (fatha, shadda, sukun) = (nominal(face.bytes, '\u{064e}'), nominal(face.bytes, '\u{0651}'), nominal(face.bytes, '\u{0652}'));
    let wrong_parent = sukun_on_shadda(face.bytes, shadda, sukun);
    let spoofed = with_table(&wrong_parent, b"GPOS", add_mark_pair_pos(tables(&wrong_parent)[b"GPOS"], fatha, sukun));
    assert!(!joins_arabic(&spoofed, face.bytes, STACKED));
}

/// Rebuilds `gpos` with `mark_feats` `"arab"` `mark` features aliasing one `Feature` table of 250
/// `mark` lookup references plus an `"mkmk"` feature preserving `mkmk` lookups, keeping the
/// original `LookupList` so HarfRust still places and stacks marks normally.
fn aliased_mark_gpos(gpos: &[u8], mark_feats: usize) -> Vec<u8> {
    let (feat_base, look_off) = (be16(gpos, 6), be16(gpos, 8));
    let mut mark_lookups = Vec::new();
    let mut mkmk_lookups = Vec::new();
    for i in 0..be16(gpos, feat_base) {
        let rec = feat_base + 2 + 6 * i;
        let tag = &gpos[rec..rec + 4];
        let target = if tag == b"mark" {
            &mut mark_lookups
        } else if tag == b"mkmk" {
            &mut mkmk_lookups
        } else {
            continue;
        };
        let f = feat_base + be16(gpos, rec + 4);
        for j in 0..be16(gpos, f + 2) {
            let idx = be16(gpos, f + 4 + 2 * j) as u16;
            if !target.contains(&idx) {
                target.push(idx);
            }
        }
    }
    assert!(!mark_lookups.is_empty() && !mkmk_lookups.is_empty());
    let words = |v: &[u16]| -> Vec<u8> { v.iter().flat_map(|x| x.to_be_bytes()).collect() };
    let (list_len, total_feats) = (250usize, mark_feats + 1);
    let script_len = 12 + 6 + 2 * total_feats;
    let feat_off = 10 + script_len;
    let mark_tbl_rel = 2 + 6 * total_feats;
    let mkmk_tbl_rel = mark_tbl_rel + 4 + 2 * list_len;
    let new_look_off = feat_off + mkmk_tbl_rel + 4 + 2 * mkmk_lookups.len();
    let mut out = words(&[1, 0, 10, u16::try_from(feat_off).unwrap(), u16::try_from(new_look_off).unwrap()]);
    out.extend(words(&[1]));
    out.extend_from_slice(b"arab");
    out.extend(words(&[8, 4, 0, 0, 0xFFFF, u16::try_from(total_feats).unwrap()]));
    for i in 0..total_feats {
        out.extend(words(&[u16::try_from(i).unwrap()]));
    }
    out.extend(words(&[u16::try_from(total_feats).unwrap()]));
    for _ in 0..mark_feats {
        out.extend_from_slice(b"mark");
        out.extend(words(&[u16::try_from(mark_tbl_rel).unwrap()]));
    }
    out.extend_from_slice(b"mkmk");
    out.extend(words(&[u16::try_from(mkmk_tbl_rel).unwrap()]));
    out.extend(words(&[0, u16::try_from(list_len).unwrap()]));
    for i in 0..list_len {
        out.extend(words(&[mark_lookups[i % mark_lookups.len()]]));
    }
    out.extend(words(&[0, u16::try_from(mkmk_lookups.len()).unwrap()]));
    out.extend(words(&mkmk_lookups));
    assert_eq!(out.len(), new_look_off);
    out.extend_from_slice(&gpos[look_off..]);
    out
}

/// An embedded font whose `arab` language system aliases `mark` features into more than `1 << 16`
/// initial lookup references (`300 * 250 = 75,000`) must be refused cleanly inside
/// `mark_lookups_attach_only`, whilst the identical table layout with one `mark` feature (`250`
/// lookup references) passes.
#[test]
fn aliased_mark_features_exceeding_gpos_budget_are_refused() {
    use pdfcraft_fonts::source_shaping::joins_arabic;
    let Some(face) = arabic_face() else { return };
    let gpos = tables(face.bytes)[b"GPOS"];
    let under_budget = with_table(face.bytes, b"GPOS", aliased_mark_gpos(gpos, 1));
    assert!(joins_arabic(&under_budget, face.bytes, STACKED));
    let over_budget = with_table(face.bytes, b"GPOS", aliased_mark_gpos(gpos, 300));
    assert!(!joins_arabic(&over_budget, face.bytes, STACKED));
}

/// Canonical mark reordering (non-CCC input order `"ب\u{0651}\u{064e}\u{0652}"`) still preserves
/// cluster-level dependency checking under `BufferClusterLevel::Characters`, and wrapping a
/// `PairPos` lookup inside a `SequenceContextFormat3` (`seq_index = 0`) lookup in `mark` is still
/// traced and refused.
#[test]
fn arabic_reordered_stacked_marks_and_contextual_pair_pos_are_checked() {
    use pdfcraft_fonts::source_shaping::joins_arabic;
    let Some(face) = arabic_face() else { return };
    let (fatha, shadda, sukun) = (nominal(face.bytes, '\u{064e}'), nominal(face.bytes, '\u{0651}'), nominal(face.bytes, '\u{0652}'));
    let reordered = "ب\u{0651}\u{064e}\u{0652}";
    assert!(joins_arabic(face.bytes, face.bytes, reordered));
    let wrong_parent = sukun_on_shadda(face.bytes, shadda, sukun);
    assert!(!joins_arabic(&wrong_parent, face.bytes, reordered));
    let pair_gpos = add_mark_pair_pos(tables(&wrong_parent)[b"GPOS"], fatha, sukun);
    let (script_off, feat_off, look_off) = (be16(&pair_gpos, 4), be16(&pair_gpos, 6), be16(&pair_gpos, 8));
    let lcount = be16(&pair_gpos, look_off);
    let insert_at = look_off + 2 + 2 * lcount;
    let mut ctx = Vec::with_capacity(pair_gpos.len() + 64);
    ctx.extend_from_slice(&pair_gpos[..insert_at]);
    ctx.extend_from_slice(&[0, 0]);
    ctx.extend_from_slice(&pair_gpos[insert_at..]);
    let write16 = |buf: &mut [u8], at: usize, val: usize| buf[at..at + 2].copy_from_slice(&u16::try_from(val).unwrap().to_be_bytes());
    write16(&mut ctx, look_off, lcount + 1);
    if script_off >= insert_at {
        write16(&mut ctx, 4, script_off + 2);
    }
    if feat_off >= insert_at {
        write16(&mut ctx, 6, feat_off + 2);
    }
    for i in 0..lcount {
        let off = be16(&ctx, look_off + 2 + 2 * i);
        if look_off + off >= insert_at {
            write16(&mut ctx, look_off + 2 + 2 * i, off + 2);
        }
    }
    let ctx_lookup_abs = ctx.len();
    write16(&mut ctx, look_off + 2 + 2 * lcount, ctx_lookup_abs - look_off);
    let pair_idx = u16::try_from(lcount - 1).unwrap();
    // Lookup type 7 (ContextPos), format 3 (1 glyph coverage at +12, 1 SequenceLookupRecord at (0, pair_idx)), Coverage format 1 [fatha].
    for w in [7u16, 0, 1, 8, 3, 1, 1, 12, 0, pair_idx, 1, 1, fatha] {
        ctx.extend_from_slice(&w.to_be_bytes());
    }
    let feat_base = be16(&ctx, 6);
    for i in 0..be16(&ctx, feat_base) {
        let rec = feat_base + 2 + 6 * i;
        if &ctx[rec..rec + 4] == b"mark" {
            let f = feat_base + be16(&ctx, rec + 4);
            let n = be16(&ctx, f + 2);
            write16(&mut ctx, f + 4 + 2 * (n - 1), lcount);
        }
    }
    let wrapped = with_table(&wrong_parent, b"GPOS", ctx);
    assert!(!joins_arabic(&wrapped, face.bytes, STACKED));
}

/// Each stacked mark shapes its word again, so their work over the whole text is bounded: 8 words
/// of beh and 2,047 fathas each are refused at once, not judged for a minute.
#[test]
fn many_stacked_marks_are_refused_within_a_bound() {
    use pdfcraft_fonts::source_shaping::joins_arabic;
    let Some(face) = arabic_face() else { return };
    let word = format!("ب{}", "\u{064e}".repeat(2047));
    let text = vec![word; 8].join(" ");
    let started = std::time::Instant::now();
    assert!(!joins_arabic(face.bytes, face.bytes, &text));
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
}

/// Patching positive `hmtx` advances to `65_000` on a 35,000-comma Arabic run pushes the summed
/// `x` advance past `i32::MAX` inside `followers` (`2.275e9`) without overflowing or panicking.
#[test]
fn wide_advances_on_a_long_arabic_run_do_not_overflow_advance() {
    use pdfcraft_fonts::source_shaping::joins_arabic;
    let Some(face) = arabic_face() else { return };
    let metrics = be16(tables(face.bytes)[b"hhea"], 34);
    let mut hmtx = tables(face.bytes)[b"hmtx"].to_vec();
    for i in 0..metrics {
        if be16(&hmtx, 4 * i) > 0 {
            hmtx[4 * i..4 * i + 2].copy_from_slice(&65_000u16.to_be_bytes());
        }
    }
    let wide = with_table(face.bytes, b"hmtx", hmtx);
    let text = format!("{}ب\u{064e}\u{0651}", "\u{060c}".repeat(35_000));
    assert!(joins_arabic(&wide, face.bytes, &text));
    let (unplaced, emptied) = without_lookups(tables(face.bytes)[b"GPOS"], &[b"mark", b"mkmk"]);
    assert!(emptied > 0);
    assert!(!joins_arabic(&with_table(&wide, b"GPOS", unplaced), face.bytes, &text));
}

/// The glyph ID emitted for character `c` when shaping `text` in `face`.
fn shaped_glyph(face: &[u8], text: &str, c: char) -> usize {
    let target = c.to_string();
    pdfcraft_fonts::source_shaping::shape(face, text).unwrap().glyphs.iter().find(|g| g.text == target).unwrap().id.to_u32() as usize
}

/// The lookup list indices attached to the first feature record tagged `tag` in `gsub`.
fn feature_lookups(gsub: &[u8], tag: &[u8; 4]) -> Vec<usize> {
    let features = be16(gsub, 6);
    for r_idx in 0..be16(gsub, features) {
        let record = features + 2 + 6 * r_idx;
        if &gsub[record..record + 4] == tag {
            let feature = features + be16(gsub, record + 4);
            return (0..be16(gsub, feature + 2)).map(|i| be16(gsub, feature + 4 + 2 * i)).collect();
        }
    }
    Vec::new()
}

/// Rewrites `from` -> `to` inside `MultipleSubst` (`type 2`) sequences for `covered` in `tag`.
fn rewrite_feature_sequence(gsub: &mut [u8], tag: &[u8; 4], covered: usize, from: usize, to: usize) -> usize {
    let lookups = be16(gsub, 8);
    let mut replaced = 0;
    for index in feature_lookups(gsub, tag) {
        let lookup = lookups + be16(gsub, lookups + 2 + 2 * index);
        for i in 0..be16(gsub, lookup + 4) {
            let mut at = lookup + be16(gsub, lookup + 6 + 2 * i);
            let kind = if be16(gsub, lookup) == 7 {
                let kind = be16(gsub, at + 2);
                at += be32(gsub, at + 4);
                kind
            } else {
                be16(gsub, lookup)
            };
            let Some(found) = (kind == 2).then(|| coverage_index(gsub, at + be16(gsub, at + 2), covered)).flatten() else { continue };
            let sequence = at + be16(gsub, at + 6 + 2 * found);
            for k in 0..be16(gsub, sequence) {
                let g = sequence + 2 + 2 * k;
                if be16(gsub, g) == from {
                    gsub[g..g + 2].copy_from_slice(&u16::try_from(to).unwrap().to_be_bytes());
                    replaced += 1;
                }
            }
        }
    }
    replaced
}

/// Copies glyph `g`'s raw `glyf` bytes and 4-byte `hmtx` entry from `program` (long `loca`).
fn glyph_data(program: &[u8], g: usize) -> (Vec<u8>, [u8; 4]) {
    let (glyf, loca, hmtx) = (tables(program)[b"glyf"], tables(program)[b"loca"], tables(program)[b"hmtx"]);
    assert!(g < be16(tables(program)[b"hhea"], 34));
    let (start, end) = (be32(loca, 4 * g), be32(loca, 4 * (g + 1)));
    let entry: [u8; 4] = hmtx[4 * g..4 * g + 4].try_into().unwrap();
    (glyf[start..end].to_vec(), entry)
}

/// Returns `program` with each `(gid, glyf_bytes, hmtx4)` replaced and `loca`/`hmtx` rebuilt.
fn with_glyphs(program: &[u8], edits: &[(usize, Vec<u8>, [u8; 4])]) -> Vec<u8> {
    let (glyf, loca) = (tables(program)[b"glyf"], tables(program)[b"loca"]);
    let mut hmtx = tables(program)[b"hmtx"].to_vec();
    for &(g, _, entry) in edits {
        assert!(g < be16(tables(program)[b"hhea"], 34));
        hmtx[4 * g..4 * g + 4].copy_from_slice(&entry);
    }
    let at = |g: usize| be32(loca, 4 * g);
    let (mut new_glyf, mut new_loca) = (Vec::new(), Vec::new());
    for g in 0..be16(tables(program)[b"maxp"], 4) {
        new_loca.extend_from_slice(&u32::try_from(new_glyf.len()).unwrap().to_be_bytes());
        if let Some((_, data, _)) = edits.iter().find(|(id, _, _)| *id == g) {
            new_glyf.extend_from_slice(data);
        } else {
            new_glyf.extend_from_slice(&glyf[at(g)..at(g + 1)]);
        }
        new_glyf.resize(new_glyf.len().next_multiple_of(4), 0);
    }
    new_loca.extend_from_slice(&u32::try_from(new_glyf.len()).unwrap().to_be_bytes());
    with_table(&with_table(&with_table(program, b"glyf", new_glyf), b"loca", new_loca), b"hmtx", hmtx)
}

/// Every glyph of `face` kept and its variations dropped.
fn flat_font(face: &[u8]) -> Vec<u8> {
    let every: BTreeSet<usize> = (0..be16(tables(face)[b"maxp"], 4)).collect();
    subset(face, &every, true)
}

/// The bundled face drawing initial `tah` in its isolated glyph (`joined` copies `tah`'s initial
/// outline so ink reaches `x <= 0.0`; `!joined` keeps its non-joining isolated outline).
fn tah_initial_as_isolated(face: &[u8], joined: bool) -> Vec<u8> {
    let tah = usize::from(nominal(face, '\u{0637}'));
    let initial = shaped_glyph(face, "\u{0637}\u{0640}", '\u{0637}');
    assert_ne!(initial, tah, "the bundled face has an initial tah of its own");
    let program = flat_font(face);
    let mut gsub = tables(&program)[b"GSUB"].to_vec();
    assert!(rewrite_feature_sequence(&mut gsub, b"init", tah, initial, tah) > 0, "the bundled face makes initial tah by multiple substitution");
    let program = with_table(&program, b"GSUB", gsub);
    if !joined {
        return program;
    }
    let (data, hmtx) = glyph_data(&program, initial);
    with_glyphs(&program, &[(tah, data, hmtx)])
}

/// A two-contour glyph whose contour 0 is a single move-only point at `x = 0` and whose only drawn
/// contour starts at `x = 20`, with `lsb = 0` matching `xmin = 0`.
fn move_only_at_zero_glyph(advance: u16) -> (Vec<u8>, [u8; 4]) {
    let mut g = Vec::new();
    for v in [2i16, 0, 0, 600, 500] {
        g.extend_from_slice(&v.to_be_bytes());
    }
    for v in [0u16, 3, 0] {
        g.extend_from_slice(&v.to_be_bytes());
    }
    g.extend_from_slice(&[1, 1, 1, 1]);
    for dx in [0i16, 20, 580, -300] {
        g.extend_from_slice(&dx.to_be_bytes());
    }
    for dy in [0i16, 0, 0, 500] {
        g.extend_from_slice(&dy.to_be_bytes());
    }
    let mut hmtx = [0u8; 4];
    hmtx[..2].copy_from_slice(&advance.to_be_bytes());
    (g, hmtx)
}

/// The bundled face giving `zah` (`\u{0638}`) its own isolated/initial/medial/final skeleton whilst
/// keeping bare `tah` (`\u{0637}`) completely untouched with distinct positional forms.
fn zah_initial_as_isolated(face: &[u8], joined: bool) -> Vec<u8> {
    let (tah, zah) = (usize::from(nominal(face, '\u{0637}')), usize::from(nominal(face, '\u{0638}')));
    let (tah_init, tah_medi, tah_fina) = (
        shaped_glyph(face, "\u{0637}\u{0640}", '\u{0637}'),
        shaped_glyph(face, "\u{0640}\u{0637}\u{0640}", '\u{0637}'),
        shaped_glyph(face, "\u{0640}\u{0637}", '\u{0637}'),
    );
    let (sad, sad_init, sad_medi, sad_fina) = (
        usize::from(nominal(face, '\u{0635}')),
        shaped_glyph(face, "\u{0635}\u{0640}", '\u{0635}'),
        shaped_glyph(face, "\u{0640}\u{0635}\u{0640}", '\u{0635}'),
        shaped_glyph(face, "\u{0640}\u{0635}", '\u{0635}'),
    );
    let program = flat_font(face);
    let mut gsub = tables(&program)[b"GSUB"].to_vec();
    assert!(rewrite_feature_sequence(&mut gsub, b"ccmp", zah, tah, sad) > 0, "ccmp decomposes zah onto tah");
    assert!(rewrite_feature_sequence(&mut gsub, b"init", sad, sad_init, sad) > 0, "init maps the dedicated skeleton");
    let program = with_table(&program, b"GSUB", gsub);
    let (iso_src, iso_hmtx) = glyph_data(&program, if joined { tah_init } else { tah });
    let (medi_src, medi_hmtx) = glyph_data(&program, tah_medi);
    let (fina_src, fina_hmtx) = glyph_data(&program, tah_fina);
    with_glyphs(&program, &[(sad, iso_src, iso_hmtx), (sad_medi, medi_src, medi_hmtx), (sad_fina, fina_src, fina_hmtx)])
}

/// Appends a contextual `liga` rule mapping `base` -> `target` immediately before `mark`, using `dlig`
/// as the invoked single substitution so bare `base` keeps its joining glyph whilst `base + mark`
/// shapes in `target` across both isolated and initial forms.
fn add_marked_tah_rule(gsub: &mut Vec<u8>, base: usize, mark: usize, target: usize) {
    let ll = be16(gsub, 8);
    let (ctx_li, single_li) = (feature_lookups(gsub, b"liga")[0], feature_lookups(gsub, b"dlig")[0]);
    assert_ne!(ctx_li, single_li);
    let single_off = u16::try_from(gsub.len() - ll).unwrap();
    for v in [1u16, 0, 1, 8, 2, 8, 1, u16::try_from(target).unwrap(), 1, 1, u16::try_from(base).unwrap()] {
        gsub.extend_from_slice(&v.to_be_bytes());
    }
    let ctx_off = u16::try_from(gsub.len() - ll).unwrap();
    for v in [
        6u16,
        0,
        1,
        8,
        3,
        0,
        1,
        18,
        1,
        24,
        1,
        0,
        u16::try_from(single_li).unwrap(),
        1,
        1,
        u16::try_from(base).unwrap(),
        1,
        1,
        u16::try_from(mark).unwrap(),
    ] {
        gsub.extend_from_slice(&v.to_be_bytes());
    }
    gsub[ll + 2 + 2 * single_li..ll + 4 + 2 * single_li].copy_from_slice(&single_off.to_be_bytes());
    gsub[ll + 2 + 2 * ctx_li..ll + 4 + 2 * ctx_li].copy_from_slice(&ctx_off.to_be_bytes());
}

/// Arial, Tahoma and Times New Roman draw initial tah and zah in the isolated glyph, whose ink
/// reaches the next letter. Such a font keeps a span that starts a word with either, where the
/// bundled face's distinct initial form refused it; one whose isolated glyph stops short of the
/// join is still refused, as is the joining font where the bundled face's other forms differ.
#[test]
fn an_initial_tah_in_an_isolated_glyph_that_joins_keeps_the_font() {
    use pdfcraft_fonts::source_shaping::joins_arabic;
    let Some(face) = arabic_face() else { return };
    let (joined, gap) = (tah_initial_as_isolated(face.bytes, true), tah_initial_as_isolated(face.bytes, false));
    for text in ["طلب", "ظلم", "بطل حظ", "ط"] {
        assert!(joins_arabic(&joined, face.bytes, text), "{text}");
    }
    for text in ["طلب", "ظلم"] {
        assert!(!joins_arabic(&gap, face.bytes, text), "{text}");
    }
    assert!(joins_arabic(&gap, face.bytes, "بطل حظ"), "no initial tah or zah");
}

/// A move-only point at `x = 0` cannot qualify an isolated initial tah whose drawn ink leaves a gap.
#[test]
fn move_only_contour_at_zero_does_not_count_as_a_joining_initial_tah() {
    let Some(face) = arabic_face() else { return };
    let gap = tah_initial_as_isolated(face.bytes, false);
    let tah = usize::from(nominal(face.bytes, '\u{0637}'));
    let adv = u16::try_from(be16(tables(&gap)[b"hmtx"], 4 * tah)).unwrap();
    let (glyph, hmtx) = move_only_at_zero_glyph(adv);
    let font = with_glyphs(&gap, &[(tah, glyph, hmtx)]);
    assert!(!pdfcraft_fonts::source_shaping::joins_arabic(&font, face.bytes, "طلب"));
}

/// Shared-initial zah keeps initial and isolated runs when its outline reaches `x <= 0` and refuses
/// initial runs when it leaves a gap, without touching tah.
#[test]
fn an_initial_zah_in_an_isolated_glyph_that_joins_keeps_the_font() {
    use pdfcraft_fonts::source_shaping::joins_arabic;
    let Some(face) = arabic_face() else { return };
    let (joined, gap) = (zah_initial_as_isolated(face.bytes, true), zah_initial_as_isolated(face.bytes, false));
    for text in ["ظلم", "حظوظ", "ظ", "طلب"] {
        assert!(joins_arabic(&joined, face.bytes, text), "{text}");
    }
    assert!(!joins_arabic(&gap, face.bytes, "ظلم"));
    for text in ["حظ", "طلب"] {
        assert!(joins_arabic(&gap, face.bytes, text), "{text}");
    }
}

/// Marked initial tah checks the outline shaped with its marks, not the unmarked glyph alone.
#[test]
fn initial_tah_with_mark_checks_its_marked_outline() {
    use pdfcraft_fonts::source_shaping::joins_arabic;
    let Some(face) = arabic_face() else { return };
    let joined = tah_initial_as_isolated(face.bytes, true);
    assert!(joins_arabic(&joined, face.bytes, "ط\u{064e}ل"));

    let tah = usize::from(nominal(face.bytes, '\u{0637}'));
    let fatha = usize::from(nominal(face.bytes, '\u{064e}'));
    let marked_slot = shaped_glyph(face.bytes, "\u{0637}\u{0640}", '\u{0637}');
    let (gap_data, gap_hmtx) = glyph_data(&flat_font(face.bytes), tah);
    let mut gsub = tables(&joined)[b"GSUB"].to_vec();
    add_marked_tah_rule(&mut gsub, tah, fatha, marked_slot);
    let marked_gap = with_glyphs(&with_table(&joined, b"GSUB", gsub), &[(marked_slot, gap_data, gap_hmtx)]);
    assert!(joins_arabic(&marked_gap, face.bytes, "طلب"));
    assert!(!joins_arabic(&marked_gap, face.bytes, "ط\u{064e}ل"));
}

/// A subset whose GSUB has no Arabic joining features. The whole run is drawn in the bundled face.
#[test]
fn kept_arabic_in_a_subset_without_joining_features_falls_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    isolated_only_falls_back_for_the_run(face, EMPTY_GSUB.to_vec());
}

/// A GSUB whose `arab` script's default language system has `fina`, `init`, `medi` and `rlig`,
/// each with one lookup. With `lookup`, that lookup is a single substitution covering no glyph, as
/// a GSUB pruned to a page's glyphs without layout closure keeps for letters the page showed only
/// isolated; without it, the index points past an empty lookup list.
fn joining_gsub(lookup: bool) -> Vec<u8> {
    let words = |v: &[u16]| -> Vec<u8> { v.iter().flat_map(|x| x.to_be_bytes()).collect() };
    // Version 1.0; script, feature and lookup lists at 10, 36 and 86.
    let mut out = words(&[1, 0, 10, 36, 86]);
    out.extend(words(&[1]));
    out.extend_from_slice(b"arab");
    // The script at 8, its default language system at 4 from it, with features 0 to 3.
    out.extend(words(&[8, 4, 0, 0, 0xFFFF, 4, 0, 1, 2, 3]));
    out.extend(words(&[4]));
    for (i, tag) in [b"fina", b"init", b"medi", b"rlig"].into_iter().enumerate() {
        out.extend_from_slice(tag);
        out.extend(words(&[26 + 6 * i as u16]));
    }
    for _ in 0..4 {
        out.extend(words(&[0, 1, if lookup { 0 } else { 9 }]));
    }
    // One lookup at 4: type 1, one subtable at 8 (format 1, coverage at 6, delta 1); the coverage
    // lists no glyph.
    out.extend(if lookup { words(&[1, 4, 1, 0, 1, 8, 1, 6, 1, 1, 0]) } else { words(&[0]) });
    out
}

/// A subset whose GSUB has the joining features, each with a lookup that covers none of its
/// letters. The whole run is drawn in the bundled face.
#[test]
fn kept_arabic_in_a_subset_whose_joining_lookups_cover_no_letter_falls_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    isolated_only_falls_back_for_the_run(face, joining_gsub(true));
}

/// A subset whose GSUB has the joining features, each pointing past its lookup list. The whole
/// run is drawn in the bundled face.
#[test]
fn kept_arabic_in_a_subset_whose_joining_lookups_are_past_its_lookup_list_falls_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    isolated_only_falls_back_for_the_run(face, joining_gsub(false));
}

/// A subset that kept a form the page never drew (initial seen): nothing on the page shows the
/// program gives it rightly, so the whole run is drawn in the bundled face.
#[test]
fn arabic_in_a_form_the_page_never_drew_falls_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    let typed = "سلم عليكم";
    let forms: Vec<usize> = pdfcraft_fonts::source_shaping::shape(face.bytes, typed).unwrap().glyphs.iter().map(|g| g.id.to_u32() as usize).collect();
    let page = producer_page_with(face.bytes, "NotoSansArabic", ORIGINAL, |keep| {
        let keep: BTreeSet<usize> = keep.iter().copied().chain(forms.iter().copied()).collect();
        subset(face.bytes, &keep, true)
    });
    falls_back_for_the_run(face, page, typed);
}

/// A subset whose GSUB gives lam its medial form where it starts a join and its initial form where
/// it joins on both sides: each of lam's forms is still distinct and drawn, so the bundled face's
/// checks pass, but the page drew them the other way round. The page and the typed text use lam
/// only in those 2 forms, and never before alef. The whole run is drawn in the bundled face.
#[test]
fn a_subset_whose_gsub_swaps_two_forms_the_page_drew_falls_back_for_the_run() {
    const ORIGINAL: &str = "\u{0644}\u{0643}\u{0645} \u{0639}\u{0644}\u{0645}";
    const KEPT: &str = "\u{0639}\u{0644}\u{0645} \u{0644}\u{0643}\u{0645}";
    let Some(face) = arabic_face() else { return };
    let lam = usize::from(nominal(face.bytes, '\u{0644}'));
    let initial = shaped_glyph(face.bytes, "\u{0644}\u{0640}", '\u{0644}');
    let medial = shaped_glyph(face.bytes, "\u{0640}\u{0644}\u{0640}", '\u{0644}');
    assert_ne!(initial, medial);
    let mut gsub = tables(face.bytes)[b"GSUB"].to_vec();
    assert!(rewrite_feature_sequence(&mut gsub, b"init", lam, initial, medial) > 0, "init makes initial lam by multiple substitution");
    assert!(rewrite_feature_sequence(&mut gsub, b"medi", lam, medial, initial) > 0, "medi makes medial lam by multiple substitution");
    // Every glyph kept, so each of lam's forms is drawn.
    let every: BTreeSet<usize> = (0..be16(tables(face.bytes)[b"maxp"], 4)).collect();
    let page = producer_page_with(face.bytes, "NotoSansArabic", ORIGINAL, |_| with_table(&subset(face.bytes, &every, true), b"GSUB", gsub));
    falls_back_for_the_run(face, page, KEPT);
}

/// The page [`kept_arabic_stays_in_its_subset_with_the_faces_glyphs`] keeps, but its font maps
/// codes to glyphs through a `/CIDToGIDMap` stream (one that happens to map each code to itself)
/// rather than the name `Identity`: the codes aren't known to be glyph ids, so the page's words
/// can't be compared with the program's shaping. The whole run is drawn in the bundled face.
#[test]
fn kept_arabic_in_a_subset_mapping_codes_through_a_stream_falls_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    let mut page = producer_page(face.bytes, "NotoSansArabic", ORIGINAL, true);
    let glyphs = be16(tables(face.bytes)[b"maxp"], 4);
    let map: Vec<u8> = (0..glyphs).flat_map(|g| (g as u16).to_be_bytes()).collect();
    let map = page.add(Object::Stream(Stream::flate(Dict::new(), &map)));
    let resources = page.dict(&Object::Ref(pdfcraft_model::pages(&page)[0].obj)).unwrap();
    let fonts = page.dict(resources.get(b"Resources").unwrap()).and_then(|r| page.dict(r.get(b"Font")?)).unwrap();
    let Some(&Object::Ref(font)) = fonts.get(b"F1") else { panic!("F1 is a reference") };
    page.update_dict(font, |d| {
        let mut descendant = d.get(b"DescendantFonts").and_then(Object::as_array).and_then(|a| a.first()).and_then(Object::as_dict).unwrap().clone();
        descendant.set(b"CIDToGIDMap".to_vec(), Object::Ref(map));
        d.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Dict(descendant)]));
    })
    .unwrap();
    falls_back_for_the_run(face, page, KEPT);
}

/// A bracket typed as a word of its own: in the row it is mirrored to the glyph the page shows,
/// but the layout measures it alone, unmirrored, in a glyph the subset emptied. The whole run is
/// drawn in the bundled face, not refused.
#[test]
fn a_bracket_standing_alone_falls_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    let original = "السَّلامُ (عليكم";
    falls_back_for_the_run(face, producer_page(face.bytes, "NotoSansArabic", original, true), "عليكم ( السَّلامُ");
}

/// Builds a minimal `GDEF` classifying `base` as `1` (Base), `marks` as `3` (Mark), and
/// assigning each mark's `MarkAttachClassDef` (`(gid, attach_class)`).
fn arabic_mark_gdef(base: u16, marks: &[(u16, u16)]) -> Vec<u8> {
    let words = |v: &[u16]| -> Vec<u8> { v.iter().flat_map(|x| x.to_be_bytes()).collect() };
    let mut cls = vec![(base, 1u16)];
    cls.extend(marks.iter().map(|&(g, _)| (g, 3u16)));
    cls.sort_by_key(|&(g, _)| g);
    let mut attach: Vec<(u16, u16)> = marks.to_vec();
    attach.sort_by_key(|&(g, _)| g);
    let cls_tbl = |entries: &[(u16, u16)]| -> Vec<u8> {
        let mut t = words(&[2, entries.len() as u16]);
        for &(g, c) in entries {
            t.extend(words(&[g, g, c]));
        }
        t
    };
    let gcls = cls_tbl(&cls);
    let mcls = cls_tbl(&attach);
    let gcls_off = 12usize;
    let mcls_off = gcls_off + gcls.len();
    let mut out = words(&[1, 0, gcls_off as u16, 0, 0, mcls_off as u16]);
    out.extend(gcls);
    out.extend(mcls);
    out
}

/// Builds a minimal self-contained `GPOS` over `base`, `fatha` (`ccc=30`), `shadda` (`ccc=33`) and
/// `sukun` (`ccc=27`) where `fatha` has `(0, fatha_y)` base anchor (`mark_anchor = (0, 0)`),
/// `sukun` (`+100` on `base`, `+200` via `mkmk`) onto either `shadda` (`sukun_to_shadda = true`,
/// using `MarkAttachmentType = 1` to skip `fatha`) or `fatha` (Arabic shaper orders `[base,
/// shadda, fatha, sukun]`).
fn stacked_pair_gpos(base: u16, fatha: u16, shadda: u16, sukun: u16, fatha_y: i16, sukun_to_shadda: bool) -> Vec<u8> {
    let words = |v: &[u16]| -> Vec<u8> { v.iter().flat_map(|x| x.to_be_bytes()).collect() };
    let mut marks = [(fatha, 0u16), (shadda, 1u16), (sukun, 2u16)];
    marks.sort_by_key(|&(g, _)| g);
    let m_cov = words(&[1, 3, marks[0].0, marks[1].0, marks[2].0]);
    let mut m_arr = words(&[3]);
    let mut anc_off = 2 + 3 * 4;
    for (_, cls) in &marks {
        m_arr.extend(words(&[*cls, anc_off as u16]));
        anc_off += 6;
    }
    for _ in 0..3 {
        m_arr.extend(words(&[1, 0, 0]));
    }
    let b_cov = words(&[1, 1, base]);
    let b_arr = words(&[1, 8, 14, 20, 1, 0, fatha_y as u16, 1, 0, 300, 1, 0, 100]);
    let mtb_hdr = 12usize;
    let m_cov_off = mtb_hdr;
    let b_cov_off = m_cov_off + m_cov.len();
    let m_arr_off = b_cov_off + b_cov.len();
    let b_arr_off = m_arr_off + m_arr.len();
    let mut l0 = words(&[4, 0, 1, 8, 1, m_cov_off as u16, b_cov_off as u16, 3, m_arr_off as u16, b_arr_off as u16]);
    l0.extend(m_cov);
    l0.extend(b_cov);
    l0.extend(m_arr);
    l0.extend(b_arr);
    let (target, l1_flag) = if sukun_to_shadda { (shadda, 0x0100u16) } else { (fatha, 0u16) };
    let l1 = words(&[
        6, l1_flag, 1, 8, 1, 12, 18, 1, 24, 36, // MarkMarkPosFormat1(mark1Cov=+12, mark2Cov=+18, cls=1, m1Arr=+24, m2Arr=+36)
        1, 1, sukun, 1, 1, target, 1, 0, 6, 1, 0, 0, 1, 4, 1, 0, 200,
    ]);
    // Header(10) + ScriptList(22) + FeatureList(26) = 58
    let mut out = words(&[1, 0, 10, 32, 58]);
    out.extend(words(&[1]));
    out.extend_from_slice(b"arab");
    out.extend(words(&[8, 4, 0, 0, 0xFFFF, 2, 0, 1]));
    out.extend(words(&[2]));
    out.extend_from_slice(b"mark");
    out.extend(words(&[14]));
    out.extend_from_slice(b"mkmk");
    out.extend(words(&[20]));
    out.extend(words(&[0, 1, 0, 0, 1, 1]));
    assert_eq!(out.len(), 58);
    let l0_off = 6usize;
    let l1_off = l0_off + l0.len();
    out.extend(words(&[2, l0_off as u16, l1_off as u16]));
    out.extend(l0);
    out.extend(l1);
    out
}

/// If a reference mark (`fatha`) has zero displacement (`fatha_y = 0`), `followers` cannot tell
/// whether `fatha` moved on its own (`!self_moved`) so it must fail closed (`None`) rather than
/// accepting a `page` font (`nonzero_ref` or `wrong_parent`) whose dependency chain cannot be
/// verified, whilst real stacked pairs on the bundled face remain accepted.
#[test]
fn zero_displacement_reference_mark_cannot_hide_wrong_parent() {
    use pdfcraft_fonts::source_shaping::joins_arabic;
    let Some(face) = arabic_face() else { return };
    for sample in
        [STACKED, "س\u{0651}\u{064e}", "س\u{0651}\u{064f}", "س\u{0651}\u{0650}", "س\u{0651}\u{064b}", "س\u{0651}\u{064c}", "س\u{0651}\u{064d}"]
    {
        assert!(joins_arabic(face.bytes, face.bytes, sample), "{sample:?}");
    }
    let (seen, fatha, shadda, sukun) =
        (nominal(face.bytes, 'س'), nominal(face.bytes, '\u{064e}'), nominal(face.bytes, '\u{0651}'), nominal(face.bytes, '\u{0652}'));
    let run = "س\u{064e}\u{0651}\u{0652}";
    let (gsub, _) = without_lookups(tables(face.bytes)[b"GSUB"], &[b"ccmp"]);
    let gdef = arabic_mark_gdef(seen, &[(sukun, 1), (shadda, 1), (fatha, 2)]);
    let base_face = with_table(&with_table(face.bytes, b"GSUB", gsub), b"GDEF", gdef);
    let nonzero_ref = with_table(&base_face, b"GPOS", stacked_pair_gpos(seen, fatha, shadda, sukun, 150, false));
    let zero_fatha_ref = with_table(&base_face, b"GPOS", stacked_pair_gpos(seen, fatha, shadda, sukun, 0, false));
    let wrong_parent = with_table(&base_face, b"GPOS", stacked_pair_gpos(seen, fatha, shadda, sukun, 150, true));
    assert!(joins_arabic(&nonzero_ref, &nonzero_ref, run));
    assert!(!joins_arabic(&wrong_parent, &nonzero_ref, run));
    assert!(!joins_arabic(&wrong_parent, &zero_fatha_ref, run));
    assert!(!joins_arabic(&nonzero_ref, &zero_fatha_ref, run));
}

/// Rebuilds `face`'s `GPOS` with `extra_lookups` matching `MarkToBase` lookups added to `arab`'s
/// `mark` feature, each exposing `subtables_per_lookup` aliased subtable offsets pointing to one
/// shared `(beh, fatha)` `MarkToBaseFormat1` subtable.
fn matching_mark_gpos(face: &[u8], extra_lookups: usize, subtables_per_lookup: usize) -> Vec<u8> {
    let (beh, fatha) = (nominal(face, 'ب'), nominal(face, '\u{064e}'));
    let orig_gpos = tables(face)[b"GPOS"];
    let (orig_feat_base, orig_look_off) = (be16(orig_gpos, 6), be16(orig_gpos, 8));
    let orig_lcount = be16(orig_gpos, orig_look_off);
    let orig_bodies = &orig_gpos[orig_look_off + 2 + 2 * orig_lcount..];
    let words = |v: &[u16]| -> Vec<u8> { v.iter().flat_map(|x| x.to_be_bytes()).collect() };
    let mut mark_ls = Vec::new();
    let mut mkmk_ls = Vec::new();
    for i in 0..be16(orig_gpos, orig_feat_base) {
        let rec = orig_feat_base + 2 + 6 * i;
        let f = orig_feat_base + be16(orig_gpos, rec + 4);
        let dst = if &orig_gpos[rec..rec + 4] == b"mark" {
            &mut mark_ls
        } else if &orig_gpos[rec..rec + 4] == b"mkmk" {
            &mut mkmk_ls
        } else {
            continue;
        };
        for j in 0..be16(orig_gpos, f + 2) {
            let idx = be16(orig_gpos, f + 4 + 2 * j) as u16;
            if !dst.contains(&idx) {
                dst.push(idx);
            }
        }
    }
    let new_lcount = orig_lcount + extra_lookups;
    let feat_off = 10 + 12 + 6 + 4;
    let mark_rel = 2 + 12;
    let mkmk_rel = mark_rel + 4 + 2 * (mark_ls.len() + extra_lookups);
    let look_off = feat_off + mkmk_rel + 4 + 2 * mkmk_ls.len();
    let mut out = words(&[1, 0, 10, u16::try_from(feat_off).unwrap(), u16::try_from(look_off).unwrap()]);
    out.extend(words(&[1]));
    out.extend_from_slice(b"arab");
    out.extend(words(&[8, 4, 0, 0, 0xFFFF, 2, 0, 1]));
    out.extend(words(&[2]));
    out.extend_from_slice(b"mark");
    out.extend(words(&[u16::try_from(mark_rel).unwrap()]));
    out.extend_from_slice(b"mkmk");
    out.extend(words(&[u16::try_from(mkmk_rel).unwrap()]));
    out.extend(words(&[0, u16::try_from(mark_ls.len() + extra_lookups).unwrap()]));
    for i in 0..extra_lookups {
        out.extend(words(&[u16::try_from(orig_lcount + i).unwrap()]));
    }
    out.extend(words(&mark_ls));
    out.extend(words(&[0, u16::try_from(mkmk_ls.len()).unwrap()]));
    out.extend(words(&mkmk_ls));
    let orig_hdr = 2 + 2 * orig_lcount;
    let new_hdr = 2 + 2 * new_lcount;
    let fan_hdr = 6 + 2 * subtables_per_lookup;
    let mut fan_lookup = words(&[4, 0, u16::try_from(subtables_per_lookup).unwrap()]);
    for _ in 0..subtables_per_lookup {
        fan_lookup.extend(words(&[u16::try_from(fan_hdr).unwrap()]));
    }
    fan_lookup.extend(words(&[1, 12, 18, 1, 24, 36, 1, 1, fatha, 1, 1, beh, 1, 0, 6, 1, 0, 200, 1, 4, 1, 0, 500]));
    let fan_off = new_hdr + orig_bodies.len();
    let mut llist = words(&[u16::try_from(new_lcount).unwrap()]);
    for i in 0..orig_lcount {
        let old_off = be16(orig_gpos, orig_look_off + 2 + 2 * i);
        llist.extend(words(&[u16::try_from(old_off - orig_hdr + new_hdr).unwrap()]));
    }
    for _ in 0..extra_lookups {
        llist.extend(words(&[u16::try_from(fan_off).unwrap()]));
    }
    out.extend_from_slice(&llist);
    out.extend_from_slice(orig_bodies);
    out.extend_from_slice(&fan_lookup);
    out
}

/// Matching `MarkToBase` lookups (`extra`) combined with a stacked Arabic run stay bounded in time:
/// `1,000` lookups with `720` stacked marks fails closed promptly rather than multiplying `1 << 16`
/// GPOS lookups by `1 << 20` perturbation steps, while a near-budget `(200, 120)` pair finishes in
/// under 2 s.
#[test]
fn near_budget_gpos_and_stacked_run_stay_within_time_bound() {
    use pdfcraft_fonts::source_shaping::joins_arabic;
    let Some(face) = arabic_face() else { return };
    let in_budget = with_table(face.bytes, b"GPOS", matching_mark_gpos(face.bytes, 200, 1));
    let in_run = format!("ب{}", "\u{064e}".repeat(120));
    let t_in = std::time::Instant::now();
    assert!(joins_arabic(&in_budget, face.bytes, &in_run));
    assert!(t_in.elapsed() < std::time::Duration::from_secs(2), "in-budget {:?}", t_in.elapsed());

    let over_budget = with_table(face.bytes, b"GPOS", matching_mark_gpos(face.bytes, 1_000, 1));
    let over_run = format!("ب{}", "\u{064e}".repeat(720));
    let t_over = std::time::Instant::now();
    assert!(!joins_arabic(&over_budget, face.bytes, &over_run));
    assert!(t_over.elapsed() < std::time::Duration::from_secs(2), "over-budget {:?}", t_over.elapsed());
}

/// A single `MarkToBase` lookup containing thousands of aliased subtable references must fail
/// closed promptly before shaper construction whilst a small multi-subtable lookup stays accepted.
#[test]
fn aliased_mark_subtable_fanout_fails_closed_within_time_bound() {
    use pdfcraft_fonts::source_shaping::joins_arabic;
    let Some(face) = arabic_face() else { return };
    let small_subtables = with_table(face.bytes, b"GPOS", matching_mark_gpos(face.bytes, 1, 8));
    let mod_run = format!("ب{}", "\u{064e}".repeat(60));
    let t_small = std::time::Instant::now();
    assert!(joins_arabic(&small_subtables, face.bytes, &mod_run));
    assert!(t_small.elapsed() < std::time::Duration::from_secs(2), "small-subtables {:?}", t_small.elapsed());

    let fanout_5k = with_table(face.bytes, b"GPOS", matching_mark_gpos(face.bytes, 1, 5_000));
    let t_5k = std::time::Instant::now();
    assert!(!joins_arabic(&fanout_5k, face.bytes, &mod_run));
    assert!(t_5k.elapsed() < std::time::Duration::from_secs(2), "fanout-5k {:?}", t_5k.elapsed());

    let fanout_2k = with_table(face.bytes, b"GPOS", matching_mark_gpos(face.bytes, 1, 2_000));
    let long_run = format!("ب{}", "\u{064e}".repeat(120));
    let t_2k = std::time::Instant::now();
    assert!(!joins_arabic(&fanout_2k, face.bytes, &long_run));
    assert!(t_2k.elapsed() < std::time::Duration::from_secs(2), "fanout-2k {:?}", t_2k.elapsed());

    let short_stacked = "ب\u{064e}\u{0651}";
    let t_short = std::time::Instant::now();
    assert!(!joins_arabic(&fanout_5k, face.bytes, short_stacked));
    assert!(t_short.elapsed() < std::time::Duration::from_secs(2), "fanout-short {:?}", t_short.elapsed());
}
