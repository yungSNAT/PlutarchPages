//! The fork's choice of bundled faces for Arabic and Chinese, Japanese and Korean spans of an
//! edited paragraph (see `PATCHES.md`).

use super::super::{CraftFont, Dict, Document, EditError, Metrics, Object, TextLine, has_arabic_letters, row_break};
use super::{covers_rows, program_candidates, shape_with_program};
use pdfcraft_fonts::source_shaping::{ShapedRun, joins_arabic};

/// The largest embedded program read again here, as `embedded_sfnt` reads it.
const MAX_PROGRAM: usize = 16 << 20;

/// How an Arabic span of an edited paragraph is drawn ([`arabic_span`]).
pub(super) enum ArabicSpan {
    /// Shaped by this resource's own program: a font of the source's face on the page.
    Program(String),
    /// Shaped with the bundled Arabic face and drawn from its outlines.
    Outlines,
}

/// How `span` (of `source`'s paragraph) is drawn, if it has Arabic letters. The first program of
/// the source's face on the page that gives the span's letters their joining forms
/// ([`joins_arabic`], on the program as embedded) and, for every row and every word (the layout
/// measures words alone), shapes it and draws every glyph its shaping chose takes the whole span,
/// a subset's too: its glyphs are the ones the page already draws. A subset that emptied one of
/// them, or whose cmap or layout tables can't reach one, can't; the span is then shaped with the
/// bundled face and drawn from its outlines, as Add text draws Arabic, and the edit says the font
/// was substituted. Either way one font draws the span, never part of a word. `None` (not
/// Arabic, or no font can) leaves the span to the other paths.
pub(super) fn arabic_span(doc: &Document, fonts: &Dict, source: &TextLine, span: &str) -> Option<ArabicSpan> {
    if !has_arabic_letters(span) {
        return None;
    }
    let units: Vec<&str> = span.split(row_break).flat_map(|row| std::iter::once(row).chain(row.split(' '))).filter(|u| !u.is_empty()).collect();
    let program = fonts.get(source.font.as_bytes()).and_then(|f| doc.dict(f)).and_then(|font| {
        program_candidates(doc, fonts, &source.font, font, &source.base_font).into_iter().find(|(_, font)| {
            embedded_program(doc, font).is_some_and(|program| joins_arabic(&program, span))
                && units.iter().all(|unit| shape_with_program(doc, font, unit).is_some())
        })
    });
    match program {
        Some((name, _)) => Some(ArabicSpan::Program(name)),
        None => pdfcraft_fonts::document_arabic_font().filter(|face| covers_rows(face.bytes, span)).map(|_| ArabicSpan::Outlines),
    }
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
