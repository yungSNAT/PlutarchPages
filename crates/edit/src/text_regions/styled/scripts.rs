//! The fork's choice of bundled faces for Arabic and Chinese, Japanese and Korean spans of an
//! edited paragraph (see `PATCHES.md`).

use super::super::{CraftFont, Dict, Document, EditError, Metrics, Object, TextLine, has_arabic_letters, row_break};
use super::{covers_rows, program_candidates, shape_with_program};
use pdfcraft_fonts::source_shaping::{ShapedRun, drawn_like_page, joins_arabic};

/// The largest embedded program read again here, as `embedded_sfnt` reads it.
const MAX_PROGRAM: usize = 16 << 20;

/// The most glyphs [`drawn_words`] reads from a page in one font.
const MAX_DRAWN_GLYPHS: usize = 1 << 16;

/// The most ToUnicode text, in bytes, [`drawn_words`] reads for those glyphs.
const MAX_DRAWN_TEXT: usize = 1 << 20;

/// How an Arabic span of an edited paragraph is drawn ([`arabic_span`]).
pub(super) enum ArabicSpan {
    /// Shaped by this resource's own program: a font of the source's face on the page.
    Program(String),
    /// Shaped with the bundled Arabic face and drawn from its outlines.
    Outlines,
}

/// How `span` (of `source`'s paragraph) is drawn, if it has Arabic letters. The first program of
/// the source's face on the page that gives the span's letters the joining forms the bundled face
/// gives them ([`joins_arabic`], on the program as embedded) and, for every row and every word
/// (the layout measures words alone), shapes it and draws every glyph its shaping chose takes the
/// whole span, a subset's too: its glyphs are the ones the page already draws. The program must
/// also draw the span only in forms the page itself drew with it ([`drawn_like_page`], on the words
/// the page shows in that font, [`drawn_words`]): a GSUB that gives a letter a distinct but wrong
/// form passes the bundled face's checks but not the page's own glyphs. A subset that
/// emptied one of them, or whose cmap or layout tables can't reach one, can't; the span is then
/// shaped with the bundled face and drawn from its outlines, as Add text draws Arabic, and the
/// edit says the font was substituted. Either way one font draws each word; a part these checks
/// didn't cover that still fails to shape is drawn alone from the bundled outlines
/// (`arabic_program_run`). Without the bundled face no program can be checked, and the span falls
/// to the other paths as before, as does one no font can draw (`None`).
pub(super) fn arabic_span(doc: &Document, page: usize, fonts: &Dict, source: &TextLine, span: &str) -> Option<ArabicSpan> {
    if !has_arabic_letters(span) {
        return None;
    }
    let units: Vec<&str> =
        span.split(row_break).flat_map(|row| std::iter::once(row).chain(row.split(char::is_whitespace))).filter(|u| !u.is_empty()).collect();
    let bundled = pdfcraft_fonts::document_arabic_font()?;
    let program = fonts.get(source.font.as_bytes()).and_then(|f| doc.dict(f)).and_then(|font| {
        program_candidates(doc, fonts, &source.font, font, &source.base_font).into_iter().find(|(name, font)| {
            embedded_program(doc, font).is_some_and(|program| {
                joins_arabic(&program, bundled.bytes, span)
                    && codes_are_glyph_ids(doc, font)
                    && drawn_words(doc, page, name, font).is_some_and(|words| drawn_like_page(&program, bundled.bytes, &words, span))
            }) && units.iter().all(|unit| shape_with_program(doc, font, unit).is_some())
        })
    });
    match program {
        Some((name, _)) => Some(ArabicSpan::Program(name)),
        None => covers_rows(bundled.bytes, span).then_some(ArabicSpan::Outlines),
    }
}

/// The words page `page`'s own content shows in the font resource `resource` (`font`), for
/// [`drawn_like_page`]: each glyph in drawing order, its code (the program's glyph id, as
/// [`codes_are_glyph_ids`] requires) and the text the font's ToUnicode gives it. A word ends at a
/// glyph whose text has a character outside Arabic's joining runs (a space), and wherever the pen
/// may jump or the font change (`BT`, `ET`, `Tf`, `Td`, `TD`, `Tm`, `T*`, `'`, `"`, `q`, `Q`,
/// `Do`), so a word is never joined to text drawn elsewhere. Forms the page draws aren't read.
/// `None` when the content can't be read or shows more than [`MAX_DRAWN_GLYPHS`] in the font, or
/// their text exceeds [`MAX_DRAWN_TEXT`].
fn drawn_words(doc: &Document, page: usize, resource: &str, font: &Dict) -> Option<Vec<Vec<(u32, String)>>> {
    let joins = |c: char| matches!(u32::from(c), 0x0600..=0x06ff | 0x0750..=0x077f | 0x0870..=0x08ff | 0x200c | 0x200d);
    let p = crate::text::page_dict(doc, page).ok()?;
    let mut budget = pdfcraft_cos::DecodeBudget::page();
    let ops = crate::text::joined(&crate::text::content_streams_within(doc, &p.dict, &mut budget).ok()?).parse().ops;
    let metrics = Metrics::from_dict(doc, font);
    let (mut current, mut saved, mut glyphs, mut bytes) = (Vec::new(), Vec::new(), 0usize, 0usize);
    let (mut words, mut word) = (Vec::new(), Vec::new());
    for op in &ops {
        let name = op.op.as_slice();
        if matches!(name, b"BT" | b"ET" | b"Tf" | b"Td" | b"TD" | b"Tm" | b"T*" | b"'" | b"\"" | b"q" | b"Q" | b"Do") && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        match name {
            b"q" => saved.push(current.clone()),
            b"Q" => current = saved.pop().unwrap_or_default(),
            // `/F1 12 Tf` takes its last two operands, as every reader here does.
            b"Tf" => current = op.operands.len().checked_sub(2).and_then(|i| op.name(i)).unwrap_or_default().to_vec(),
            _ => {}
        }
        if !matches!(name, b"Tj" | b"TJ" | b"'" | b"\"") {
            continue;
        }
        if current != resource.as_bytes() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        let pieces: Vec<Object> = match name {
            b"TJ" => op.operands.last().and_then(Object::as_array).cloned().unwrap_or_default(),
            _ => op.operands.last().cloned().into_iter().collect(),
        };
        for piece in &pieces {
            let Object::String(s) = piece else { continue };
            for (code, _) in metrics.codes(&s.bytes) {
                glyphs += 1;
                if glyphs > MAX_DRAWN_GLYPHS {
                    return None;
                }
                let text = metrics.text_of(code).unwrap_or_default();
                bytes = bytes.saturating_add(text.len());
                if bytes > MAX_DRAWN_TEXT {
                    return None;
                }
                if text.chars().all(joins) {
                    word.push((code, text.into_owned()));
                } else if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    Some(words)
}

/// `font`'s embedded TrueType or OpenType program as the page has it: `embedded_sfnt` may add a
/// cmap rebuilt from the font's ToUnicode, which [`joins_arabic`] must not take for the font's own.
fn embedded_program(doc: &Document, font: &Dict) -> Option<Vec<u8>> {
    let descendant = font.get(b"DescendantFonts").and_then(|d| doc.resolve(d).as_array().and_then(|a| a.first()).cloned()).and_then(|d| doc.dict(&d));
    let descriptor = descendant.as_ref().unwrap_or(font).get(b"FontDescriptor").and_then(|d| doc.dict(d))?;
    [b"FontFile2".as_slice(), b"FontFile3".as_slice()].into_iter().find_map(|key| {
        let object = doc.resolve(descriptor.get(key)?);
        let Object::Stream(stream) = object.as_ref() else { return None };
        pdfcraft_cos::DecodeBudget::new(MAX_PROGRAM, 0).decode_stages_within(doc, stream, MAX_PROGRAM).ok()
    })
}

/// Whether `font`'s ToUnicode reads `run` back as its text when each glyph is shown in its own
/// id: each cluster's glyphs name exactly the cluster's characters. A subset may keep forms its
/// page never showed, which its ToUnicode doesn't name; text shown with them could then be
/// copied only through the run's `ActualText`.
pub(super) fn to_unicode_reads(doc: &Document, font: &Dict, run: &ShapedRun) -> bool {
    let metrics = Metrics::from_dict(doc, font);
    // (the cluster's characters, those its glyphs name), sorted: a right-to-left cluster's
    // glyphs come in visual order.
    let mut clusters: Vec<(Vec<char>, Vec<char>)> = Vec::new();
    for glyph in &run.glyphs {
        if !glyph.text.is_empty() || clusters.is_empty() {
            clusters.push((glyph.text.chars().collect(), Vec::new()));
        }
        if let (Some(cluster), Some(text)) = (clusters.last_mut(), metrics.text_of(glyph.id.to_u32())) {
            cluster.1.extend(text.chars());
        }
    }
    clusters.into_iter().all(|(mut want, mut named)| {
        want.sort_unstable();
        named.sort_unstable();
        want == named
    })
}

/// Whether `font` is a TrueType CID font whose codes are its program's glyph ids (Identity-H and
/// an identity `/CIDToGIDMap`), so text its program shaped is shown in its own codes.
pub(super) fn codes_are_glyph_ids(doc: &Document, font: &Dict) -> bool {
    let name = |dict: &Dict, key: &[u8], value: &[u8]| dict.get(key).is_some_and(|o| doc.resolve(o).as_name() == Some(value));
    let descendant = font.get(b"DescendantFonts").and_then(|d| doc.resolve(d).as_array().and_then(|a| a.first()).cloned()).and_then(|d| doc.dict(&d));
    name(font, b"Subtype", b"Type0")
        && name(font, b"Encoding", b"Identity-H")
        && descendant
            .is_some_and(|d| name(&d, b"Subtype", b"CIDFontType2") && (d.get(b"CIDToGIDMap").is_none() || name(&d, b"CIDToGIDMap", b"Identity")))
}

/// Whether a bundled face is for Chinese, Japanese or Korean.
pub(super) fn is_cjk_face(face: &CraftFont) -> bool {
    ["Jpan", "Hani", "Hans", "Hant", "Kore", "Hang"].iter().any(|script| face.covers(script))
}

/// `text` shaped with the bundled Arabic face, whose outlines draw it ([`ArabicSpan::Outlines`]).
pub(super) fn shape_arabic(text: &str) -> Result<pdfcraft_fonts::source_shaping::ShapedRun, EditError> {
    let face = pdfcraft_fonts::document_arabic_font().ok_or_else(|| EditError::Invalid("the bundled Arabic font is not in this build".into()))?;
    pdfcraft_fonts::source_shaping::shape(face.bytes, text).map_err(EditError::Invalid)
}
