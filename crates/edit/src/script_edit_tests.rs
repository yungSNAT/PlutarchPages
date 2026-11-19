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
    let mut font = Vec::new();
    font.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    let n = out.len() as u16;
    let range = 16 * (1u16 << (15 - n.leading_zeros()));
    for v in [n, range, 15 - n.leading_zeros() as u16, n * 16 - range] {
        font.extend_from_slice(&v.to_be_bytes());
    }
    let mut offset = 12 + 16 * out.len();
    for (tag, data) in &out {
        let padded: Vec<u8> = data.iter().copied().chain(std::iter::repeat(0)).take(data.len().next_multiple_of(4)).collect();
        let sum = padded.chunks(4).fold(0u32, |s, c| s.wrapping_add(u32::from_be_bytes(c.try_into().unwrap())));
        font.extend_from_slice(tag);
        font.extend_from_slice(&sum.to_be_bytes());
        font.extend_from_slice(&(offset as u32).to_be_bytes());
        font.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += padded.len();
    }
    for (_, data) in &out {
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
    let file = doc.add(Object::Stream(Stream::flate(Dict::new(), &subset(program, &keep, layout))));
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
fn fonts_shown(doc: &Document) -> BTreeSet<String> {
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
    pdfcraft_content::parse(&content)
        .ops
        .iter()
        .filter(|o| o.is("Tf"))
        .filter_map(|o| o.name(0).map(|n| String::from_utf8_lossy(n).into_owned()))
        .collect()
}

fn face(family: &str) -> Option<&'static pdfcraft_fonts::CraftFont> {
    pdfcraft_fonts::CRAFT_FONTS.iter().find(|f| f.family == family && f.style == "Regular")
}

/// Every case, each failure named, so one run shows them all. Arabic letters take their forms
/// from their neighbours, so a line edit, which redraws one character at a time, refuses them and
/// leaves the page as it was; the paragraph edit shapes them, with the bundled Arabic face when
/// the page's subset cannot.
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
                    // Arabic kept in the page's subset only where its program can shape it.
                    if what == "kept" && !arabic && (edit.substituted.is_some() || shown != BTreeSet::from(["F1".to_string()])) {
                        failures.push(format!("{label}, kept {how}: left the page's subset ({:?}, fonts {shown:?})", edit.substituted));
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
