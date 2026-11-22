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

/// The fonts the page's content selects.
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
fn nominal(program: &[u8], c: char) -> u16 {
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
    // One segment a character, then the closing one, which maps U+FFFF to glyph 0.
    let mut segments: Vec<(u16, u16)> = chars.iter().map(|&c| (c as u16, nominal(program, c).wrapping_sub(c as u16))).collect();
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

/// A subset whose own cmap lacks kaf, which the editor's stand-in cmap then maps to the medial
/// form the page's ToUnicode names: typed at a word's start, kaf would be drawn medial in the
/// page's font. The whole run is drawn in the bundled face.
#[test]
fn kept_arabic_in_a_subset_whose_own_cmap_lacks_a_letter_falls_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    let chars: BTreeSet<char> = ORIGINAL.chars().filter(|&c| c != '\u{0643}').collect();
    let page = producer_page_with(face.bytes, "NotoSansArabic", ORIGINAL, |keep| {
        with_table(&subset(face.bytes, keep, true), b"cmap", pruned_cmap(face.bytes, &chars))
    });
    falls_back_for_the_run(face, page, "كم عليكم");
}

/// A subset whose GSUB has no Arabic joining features, though it kept the isolated forms: every
/// glyph has ink, but every letter would be drawn isolated. The whole run is drawn in the
/// bundled face.
#[test]
fn kept_arabic_in_a_subset_without_joining_features_falls_back_for_the_run() {
    let Some(face) = arabic_face() else { return };
    let page = producer_page_with(face.bytes, "NotoSansArabic", ORIGINAL, |keep| {
        let keep: BTreeSet<usize> = keep.iter().copied().chain(ORIGINAL.chars().map(|c| usize::from(nominal(face.bytes, c)))).collect();
        with_table(&subset(face.bytes, &keep, true), b"GSUB", EMPTY_GSUB.to_vec())
    });
    falls_back_for_the_run(face, page, KEPT);
}

/// A subset that kept a form the page never showed (initial seen), which its ToUnicode doesn't
/// name: the run keeps the subset's program but is embedded with it again, with its own
/// ToUnicode, not shown in the page's codes, which would copy only through `ActualText`.
#[test]
fn arabic_in_a_form_the_to_unicode_lacks_keeps_its_program_with_its_own_to_unicode() {
    let Some(face) = arabic_face() else { return };
    let typed = "سلم عليكم";
    let forms: Vec<usize> = pdfcraft_fonts::source_shaping::shape(face.bytes, typed).unwrap().glyphs.iter().map(|g| g.id.to_u32() as usize).collect();
    let mut page = producer_page_with(face.bytes, "NotoSansArabic", ORIGINAL, |keep| {
        let keep: BTreeSet<usize> = keep.iter().copied().chain(forms.iter().copied()).collect();
        subset(face.bytes, &keep, true)
    });
    let edit = replace_block(&mut page, 0, 0, typed).unwrap();
    assert_eq!(edit.substituted, None);
    let (reopened, _) = saved(&page);
    let read: Vec<String> = text_blocks(&reopened, 0).unwrap().into_iter().map(|b| b.text).collect();
    assert_eq!(read, [typed]);
    let shown = glyphs_shown(&reopened);
    assert!(!shown.is_empty() && !shown.iter().any(|(font, _)| font == "F1"), "shown in the page's codes: {shown:?}");
    let drawn = difference(&ink(&reopened), &ink(&producer_page(face.bytes, "NotoSansArabic", typed, true)));
    assert!(drawn <= DRAWN, "{drawn:.3} of the ink differs from the typed text's");
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
