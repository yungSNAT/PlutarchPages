//! The fork's choice of bundled faces for Arabic and Chinese, Japanese and Korean spans of an
//! edited paragraph (see `PATCHES.md`).

use super::super::{CraftFont, Dict, Document, EditError, Metrics, TextLine, has_arabic_letters, row_break};
use super::{covers_rows, program_candidates, shape_with_program};
use crate::text::fallback::is_subset;

/// Whether `span` (of `source`'s paragraph) is Arabic that the bundled Arabic face draws, and no
/// complete program of the source's face on the page can shape it. A subset can't: shaping picks
/// the letters' joining forms, which it may not have kept, and its character map may still name
/// glyphs whose outlines it dropped. Its letters are then shaped with the bundled face and drawn
/// from its outlines, as Add text draws Arabic, and the edit says the font was substituted.
pub(super) fn arabic_outlines(doc: &Document, fonts: &Dict, source: &TextLine, span: &str) -> bool {
    let Some(face) = pdfcraft_fonts::document_arabic_font() else { return false };
    if !has_arabic_letters(span) || !covers_rows(face.bytes, span) {
        return false;
    }
    let Some(font) = fonts.get(source.font.as_bytes()).and_then(|f| doc.dict(f)) else { return true };
    !program_candidates(doc, fonts, &source.font, font, &source.base_font).into_iter().any(|(_, font)| {
        !is_subset(&Metrics::from_dict(doc, &font).base_font) && span.split(row_break).all(|row| shape_with_program(doc, &font, row).is_some())
    })
}

/// Whether a bundled face is for Chinese, Japanese or Korean.
pub(super) fn is_cjk_face(face: &CraftFont) -> bool {
    ["Jpan", "Hani", "Hans", "Hant", "Kore", "Hang"].iter().any(|script| face.covers(script))
}

/// `text` shaped with the bundled Arabic face, whose outlines draw it ([`arabic_outlines`]).
pub(super) fn shape_arabic(text: &str) -> Result<pdfcraft_fonts::source_shaping::ShapedRun, EditError> {
    let face = pdfcraft_fonts::document_arabic_font().ok_or_else(|| EditError::Invalid("the bundled Arabic font is not in this build".into()))?;
    pdfcraft_fonts::source_shaping::shape(face.bytes, text).map_err(EditError::Invalid)
}
