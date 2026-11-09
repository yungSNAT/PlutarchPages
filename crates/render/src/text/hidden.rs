// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Which extracted glyphs paint nothing a reader can see, so the tools that hand page text to an
//! agent can say so: text a document hides from its reader is a common route for prompt injection.

use super::device::{Drawn, MAX_CELLS, MAX_REDRAW_WORK, cell, redraw_tolerance};
use super::{GlyphDrawMode, HashMap, Paint, Rect};

/// Why extracted text can be hidden or differ from what a reader sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HiddenReason {
    /// Text rendering mode 3: neither filled nor stroked.
    Invisible,
    /// Rendering mode 3 inside an `/OCR` marked-content sequence, the layer Scan & OCR ▸
    /// Recognize text writes over a scan. Reported apart, not exempted: any file can write the
    /// marker.
    InvisibleOcrLayer,
    /// Every fill and stroke of it has an effective opacity (its colour's, times that of every
    /// enclosing transparency group) that rounds to 0 in an 8-bit raster.
    Transparent,
    /// Under 1 point: the em square of its font, as drawn, is less than 1 point high across the
    /// baseline. Its box is not the measure: a turned glyph's box is as high as it is wide.
    Tiny,
    /// Wholly outside the page's crop box, so never shown. Only the editor's text has it: the
    /// renderer leaves such glyphs out of the text it returns.
    OutsideCropBox,
    /// A character with no width or ink of its own (U+200B to U+200D, U+2060 to U+2064, U+FEFF,
    /// U+180E) or a tag character (U+E0000 to U+E007F), which can spell out text no one sees.
    ZeroWidth,
    /// A character that reorders the text around it as shown (U+061C, U+200E, U+200F, U+202A to
    /// U+202E and U+2066 to U+2069), so what reads differs from what shows.
    BidiControl,
    /// A private-use character (U+E000 to U+F8FF, planes 15 and 16): its meaning is the font's.
    /// Icon and symbol fonts use them for visible glyphs too.
    PrivateUse,
    /// A solid glyph fill close to white over a simple white page backdrop.
    NearBackground,
    /// An ASCII /ActualText span differs from its glyphs' own ASCII text.
    ActualTextDiffers,
    /// A standard font's ASCII encoding and its /ToUnicode ASCII text disagree.
    FontMappingDiffers,
    /// Left-to-right ASCII glyphs on one line occur out of visual order in the content stream.
    SourceOrderDiffers,
}

impl HiddenReason {
    /// The stable machine-readable ID the tools report. Do not rename one; add a variant instead.
    pub fn id(self) -> &'static str {
        match self {
            HiddenReason::Invisible => "invisible",
            HiddenReason::InvisibleOcrLayer => "invisible_ocr_layer",
            HiddenReason::Transparent => "transparent",
            HiddenReason::Tiny => "tiny",
            HiddenReason::OutsideCropBox => "outside_crop_box",
            HiddenReason::ZeroWidth => "zero_width",
            HiddenReason::BidiControl => "bidi_control",
            HiddenReason::PrivateUse => "private_use",
            HiddenReason::NearBackground => "near_background",
            HiddenReason::ActualTextDiffers => "actual_text_differs",
            HiddenReason::FontMappingDiffers => "font_mapping_differs",
            HiddenReason::SourceOrderDiffers => "source_order_differs",
        }
    }
}

/// Whether paint at `opacity` leaves nothing in an 8-bit raster: it rounds to an alpha of 0.
/// A NaN opacity paints, as an unreadable one is not evidence of hiding.
pub fn paints_nothing(opacity: f64) -> bool {
    opacity < 0.5 / 255.0
}

/// Whether text whose em square is `size` points high across the baseline is too small to read:
/// under 1 point. A NaN size is not.
pub fn is_tiny(size: f64) -> bool {
    size < 1.0
}

/// Why `c` hides text or what it does from a reader, if it does: [`HiddenReason::ZeroWidth`],
/// [`HiddenReason::BidiControl`] or [`HiddenReason::PrivateUse`].
pub fn character_reason(c: char) -> Option<HiddenReason> {
    match c {
        '\u{200B}'..='\u{200D}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}' | '\u{180E}' | '\u{E0000}'..='\u{E007F}' => Some(HiddenReason::ZeroWidth),
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => Some(HiddenReason::BidiControl),
        '\u{E000}'..='\u{F8FF}' | '\u{F0000}'..='\u{FFFFD}' | '\u{100000}'..='\u{10FFFD}' => Some(HiddenReason::PrivateUse),
        _ => None,
    }
}

/// The class of a glyph drawn several times (a fill and a stroke, or the glyphs one
/// `/ActualText` replaces): hidden only when no drawing paints. A reason other than the OCR
/// layer wins, so a marker cannot soften what another drawing shows.
pub(super) fn combine(a: Option<HiddenReason>, b: Option<HiddenReason>) -> Option<HiddenReason> {
    match (a, b) {
        (Some(HiddenReason::InvisibleOcrLayer), b @ Some(_)) => b,
        (a @ Some(_), Some(_)) => a,
        _ => None,
    }
}

/// What makes the glyphs a page draws paint nothing, followed while it is interpreted.
#[derive(Default)]
pub(super) struct Visibility {
    /// Whether to classify glyphs at all: plain extraction skips the colour conversion.
    pub(super) on: bool,
    /// The marked-content depth of the outermost open `/OCR` sequence.
    ocr: Option<usize>,
    /// The opacity of each open transparency group, outermost first.
    groups: Vec<f32>,
    /// Occupied boxes on a plain white page. Unknown after complex painting or the work limit.
    white_backdrop: Option<Vec<Rect>>,
}

impl Visibility {
    pub(super) fn enable(&mut self) {
        self.on = true;
        self.white_backdrop = Some(Vec::new());
    }

    pub(super) fn forget_backdrop(&mut self) {
        self.white_backdrop = None;
    }

    pub(super) fn begin_marked_content(&mut self, depth: usize, tag: &[u8]) {
        if self.ocr.is_none() && tag == b"OCR" {
            self.ocr = Some(depth);
        }
    }

    pub(super) fn end_marked_content(&mut self, depth: usize) {
        if self.ocr == Some(depth) {
            self.ocr = None;
        }
    }

    /// hayro pushes and pops groups in pairs, also for an image or a knockout pass, so the stack
    /// is never deeper than the content's nesting.
    pub(super) fn push_group(&mut self, opacity: f32) {
        self.forget_backdrop();
        self.groups.push(opacity);
    }

    pub(super) fn pop_group(&mut self) {
        self.groups.pop();
    }

    /// Why one drawing of a glyph in `mode` with `paint` shows nothing a reader can see, if it
    /// does not: `em` is the height of its em square in points, `None` when unknown (a Type 3
    /// glyph, whose glyph space its font sets). Clipping text (`7 Tr`) is not classed: what it
    /// shows is what is painted through it.
    pub(super) fn class(&mut self, paint: &Paint<'_>, mode: &GlyphDrawMode, em: Option<f64>, bounds: Rect) -> Option<HiddenReason> {
        if !self.on {
            return None;
        }
        match mode {
            GlyphDrawMode::Invisible if self.ocr.is_some() => Some(HiddenReason::InvisibleOcrLayer),
            GlyphDrawMode::Invisible => Some(HiddenReason::Invisible),
            GlyphDrawMode::Clip => None,
            GlyphDrawMode::Fill | GlyphDrawMode::Stroke(_) => {
                let rgba = match paint {
                    // The renderer's own alpha: 0 for a Separation /None, opaque for a colour it
                    // cannot convert (drawn in a fallback colour).
                    Paint::Color(colour) => Some(colour.to_rgba().components()),
                    // shortcut: a pattern's own opacity has no getter in hayro, so it counts as
                    // opaque; reading it needs an accessor patched into the vendored `Pattern`.
                    Paint::Pattern(_) => None,
                };
                let alpha = rgba.map_or(1.0, |channels| channels[3]);
                let alpha = self.groups.iter().fold(alpha, |alpha, group| alpha * group);
                if paints_nothing(f64::from(alpha)) {
                    return Some(HiddenReason::Transparent);
                }
                let near_background = match (mode, rgba, em) {
                    (GlyphDrawMode::Fill, Some([r, g, b, _]), Some(_)) => self.near_white([r, g, b], bounds),
                    _ => {
                        self.forget_backdrop();
                        false
                    }
                };
                if em.is_some_and(is_tiny) { Some(HiddenReason::Tiny) } else { near_background.then_some(HiddenReason::NearBackground) }
            }
        }
    }

    fn near_white(&mut self, rgb: [f32; 3], bounds: Rect) -> bool {
        const MAX_INK_BOXES: usize = 512;
        const NEAR_WHITE: f32 = 8.0 / 255.0;
        let Some(ink) = self.white_backdrop.as_mut() else { return false };
        // shortcut: test only a plain white backdrop with bounded previous glyph boxes. Paths,
        // images and complex compositing need a separate comparison with rendered pixels.
        // Dark translucent glyphs can build up to visible ink when drawn repeatedly.
        let near = rgb.iter().all(|c| (1.0 - c).abs() <= NEAR_WHITE);
        let overlaps = |r: &Rect| r.x0 < bounds.x1 && bounds.x0 < r.x1 && r.y0 < bounds.y1 && bounds.y0 < r.y1;
        if near && !ink.iter().any(overlaps) {
            return true;
        }
        if ink.len() == MAX_INK_BOXES {
            self.forget_backdrop();
        } else {
            ink.push(bounds);
        }
        false
    }
}

/// A hidden glyph with a painting glyph of the same text at its place shows after all: `2 Tr`
/// draws each glyph once to fill and once to stroke, and a file may draw a hidden copy before a
/// visible one. Layout keeps the first drawing of a glyph drawn twice, so the classes are joined
/// before it, as `TextDevice::finish` matches redraws. Past the work budget the rest stay
/// hidden.
pub(super) fn cover_redrawn(glyphs: &mut [Drawn]) {
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, g) in glyphs.iter().enumerate() {
        if g.3.is_none() {
            grid.entry((cell(g.2.rect[0]), cell(g.2.rect[1]))).or_default().push(i);
        }
    }
    if grid.is_empty() {
        return;
    }
    let mut shown = Vec::new();
    let mut budget = MAX_REDRAW_WORK;
    'glyphs: for (i, g) in glyphs.iter().enumerate() {
        if g.3.is_none() {
            continue;
        }
        let rect = &g.2.rect;
        let tol = redraw_tolerance(rect);
        let (x0, x1, y0, y1) = (cell(rect[0] - tol), cell(rect[0] + tol), cell(rect[1] - tol), cell(rect[1] + tol));
        if x1.saturating_sub(x0).saturating_add(1).saturating_mul(y1.saturating_sub(y0).saturating_add(1)) > MAX_CELLS {
            continue;
        }
        for key in (x0..=x1).flat_map(|x| (y0..=y1).map(move |y| (x, y))) {
            for &j in grid.get(&key).map_or(&[][..], Vec::as_slice) {
                let Some(rest) = budget.checked_sub(1) else { break 'glyphs };
                budget = rest;
                let Some(other) = glyphs.get(j) else { continue };
                let o = &other.2.rect;
                if (o[0] - rect[0]).abs() < tol && (o[1] - rect[1]).abs() < tol {
                    // ActualText can replace a glyph with a long string. Count its bytes,
                    // not just the candidate, before comparing a possibly shared prefix.
                    let Some(rest) = budget.checked_sub(g.2.text.len()) else { break 'glyphs };
                    budget = rest;
                    if other.2.text == g.2.text {
                        shown.push(i);
                        continue 'glyphs;
                    }
                }
            }
            let Some(rest) = budget.checked_sub(1) else { break 'glyphs };
            budget = rest;
        }
    }
    for i in shown {
        if let Some(g) = glyphs.get_mut(i) {
            g.3 = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HiddenReason::{self, Invisible, InvisibleOcrLayer, Tiny, Transparent};
    use crate::text::{InterpreterSettings, Pdf, extract_page, extract_page_hidden};

    /// A 400 × 400 page drawing `content`, with Helvetica as /F1, /GS0 (fill opacity 0), /GS3
    /// (fill and stroke opacity 0), a grey 1 × 1 image /Im0 and a transparency group /Fm0 that
    /// shows "Inside the group".
    fn page(content: &str) -> Pdf {
        let group = "BT /F1 20 Tf 1 0 0 1 50 200 Tm (Inside the group) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> \
             /ExtGState << /GS0 << /ca 0 >> /GS3 << /ca 0 /CA 0 >> >> /XObject << /Im0 6 0 R /Fm0 7 0 R >> >> >>"
                .to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 \
             /Filter /ASCIIHexDecode /Length 3 >>\nstream\n80>\nendstream"
                .to_string(),
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 400 400] /Group << /S /Transparency >> \
                 /Resources << /Font << /F1 3 0 R >> >> /Length {} >>\nstream\n{group}\nendstream",
                group.len()
            ),
        ];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        Pdf::new(std::sync::Arc::new(bytes)).unwrap()
    }

    /// Each returned glyph's text with its class. The classes never change the text.
    fn classified(content: &str) -> Vec<(String, Option<HiddenReason>)> {
        let pdf = page(content);
        let (text, hidden) = extract_page_hidden(&pdf, 0, &InterpreterSettings::default()).unwrap();
        assert_eq!(hidden.len(), text.glyphs.len(), "one class per returned glyph");
        assert_eq!(Some(&text), extract_page(&pdf, 0, &InterpreterSettings::default()).as_ref(), "the same text as plain extraction");
        text.glyphs.iter().map(|g| g.text.clone()).zip(hidden).collect()
    }

    /// The text of the glyphs classed `reason`, in order, without spaces.
    fn flagged(glyphs: &[(String, Option<HiddenReason>)], reason: HiddenReason) -> String {
        glyphs.iter().filter(|(_, r)| *r == Some(reason)).flat_map(|(t, _)| t.chars()).filter(|c| !c.is_whitespace()).collect()
    }

    /// `text` at height `y` in rendering mode `mode`, which stays set after the text object.
    fn line(y: u32, mode: u8, text: &str) -> String {
        format!("BT /F1 20 Tf {mode} Tr 1 0 0 1 50 {y} Tm ({text}) Tj ET\n")
    }

    #[test]
    fn ordinary_text_is_not_hidden() {
        let glyphs = classified(&line(300, 0, "Ordinary text"));
        assert!(glyphs.iter().all(|(_, r)| r.is_none()), "{glyphs:?}");
    }

    #[test]
    fn rendering_mode_3_and_zero_fill_opacity_are_hidden() {
        let content = format!("{}{}q /GS0 gs {}Q", line(300, 0, "Shown"), line(250, 3, "Unseen"), line(200, 0, "Clear"));
        let glyphs = classified(&content);
        assert_eq!(flagged(&glyphs, Invisible), "Unseen");
        assert_eq!(flagged(&glyphs, Transparent), "Clear");
        assert_eq!(flagged(&glyphs, InvisibleOcrLayer), "");
    }

    #[test]
    fn a_stroke_that_shows_outlines_a_transparent_fill() {
        let content = format!("q /GS0 gs 0 0 1 RG 1 w {}Q q /GS3 gs 1 w {}Q", line(300, 2, "Outlined"), line(200, 2, "Neither"));
        let glyphs = classified(&content);
        assert_eq!(flagged(&glyphs, Transparent), "Neither", "the fill and stroke of 2 Tr are one glyph: hidden only when both paint nothing");
    }

    #[test]
    fn a_hidden_copy_drawn_again_visibly_shows() {
        for content in
            [format!("{}{}", line(300, 3, "Secret"), line(300, 0, "Secret")), format!("{}{}", line(300, 0, "Secret"), line(300, 3, "Secret"))]
        {
            let glyphs = classified(&content);
            assert_eq!(glyphs.iter().map(|(t, _)| t.as_str()).collect::<String>(), "Secret", "layout keeps one drawing");
            assert!(glyphs.iter().all(|(_, r)| r.is_none()), "{content}: {glyphs:?}");
        }
    }

    #[test]
    fn redraw_comparisons_charge_long_replacement_text_to_the_budget() {
        let prefix = "a".repeat(32 * 1024);
        let unmatched = format!("{prefix}a");
        let matching = format!("{prefix}b");
        let glyph = |text: &str, reason| {
            let rect = [100.0, 100.0, 110.0, 110.0];
            let quad = [[100.0, 110.0], [110.0, 110.0], [110.0, 100.0], [100.0, 100.0]];
            (0, [1.0, 0.0], crate::text::TextGlyph { text: text.into(), rect, quad, direction: [1.0, 0.0] }, reason)
        };
        // All candidates share a long prefix at the same spot. The genuine match comes
        // after more text than the work budget permits comparing, so it stays conservative.
        let count = super::MAX_REDRAW_WORK / matching.len() + 1;
        let mut glyphs: Vec<_> = (0..count).map(|_| glyph(&unmatched, None)).collect();
        glyphs.push(glyph(&matching, None));
        glyphs.push(glyph(&matching, Some(Invisible)));
        super::cover_redrawn(&mut glyphs);
        assert_eq!(glyphs.last().unwrap().3, Some(Invisible), "the comparison budget must include replacement bytes");
    }

    #[test]
    fn classes_stay_with_their_glyphs_through_redraws_and_actual_text() {
        let content = [
            line(350, 0, "Dup"),
            // Drawn again at the same place: layout drops this copy.
            line(350, 0, "Dup"),
            format!("/Span <</ActualText (Word)>> BDC {}EMC\n", line(300, 0, "xyz")),
            line(250, 3, "Hidden"),
            format!("/Span <</ActualText (Gone)>> BDC {}EMC\n", line(200, 3, "abc")),
            line(150, 0, "End"),
        ]
        .concat();
        let glyphs = classified(&content);
        let text: Vec<&str> = glyphs.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, ["D", "u", "p", "Word", "H", "i", "d", "d", "e", "n", "Gone", "E", "n", "d"]);
        assert_eq!(flagged(&glyphs, Invisible), "HiddenGone", "{glyphs:?}");
    }

    #[test]
    fn the_ocr_layer_is_reported_with_its_own_reason() {
        let content = format!(
            "/OCR BMC q {}/Span BMC {}EMC {}Q EMC {}",
            line(300, 3, "Scanned"),
            line(250, 3, "Nested"),
            line(200, 3, "Words"),
            line(150, 3, "Outside")
        );
        let glyphs = classified(&content);
        assert_eq!(flagged(&glyphs, InvisibleOcrLayer), "ScannedNestedWords", "an inner sequence does not end the layer");
        assert_eq!(flagged(&glyphs, Invisible), "Outside");
    }

    #[test]
    fn invisible_text_over_a_picture_is_not_exempt() {
        let glyphs = classified(&format!("q 400 0 0 400 0 0 cm /Im0 Do Q {}", line(200, 3, "Over")));
        assert_eq!(flagged(&glyphs, Invisible), "Over");
    }

    #[test]
    fn text_under_1_point_is_tiny_whatever_its_box() {
        let content = [
            line(350, 0, "Readable"),
            "BT /F1 0.8 Tf 1 0 0 1 50 300 Tm (Small) Tj ET\n".to_owned(),
            // 20 pt, drawn at 0.8 pt by the text matrix.
            "BT /F1 20 Tf 0.04 0 0 0.04 50 250 Tm (Shrunk) Tj ET\n".to_owned(),
            // Turned a quarter: each narrow 3 pt glyph's box is under 1 pt high, but the text is 3 pt.
            "BT /F1 3 Tf 0 1 -1 0 300 50 Tm (iiii) Tj ET\n".to_owned(),
            // Turned a quarter and widened 5 times: 0.8 pt text whose glyph boxes are over 3 pt high.
            "BT /F1 0.8 Tf 500 Tz 0 1 -1 0 350 50 Tm (WWW) Tj ET\n".to_owned(),
        ]
        .concat();
        let glyphs = classified(&content);
        let mut tiny: Vec<char> = flagged(&glyphs, Tiny).chars().collect();
        tiny.sort_unstable();
        let mut expected: Vec<char> = "SmallShrunkWWW".chars().collect();
        expected.sort_unstable();
        assert_eq!(tiny, expected, "{glyphs:?}");
        assert!(glyphs.iter().filter(|(t, _)| t == "i").all(|(_, r)| r.is_none()), "{glyphs:?}");
    }

    #[test]
    fn characters_that_hide_or_reorder_text_are_classed() {
        use super::{HiddenReason::*, character_reason};
        for (c, reason) in [
            ('\u{200B}', Some(ZeroWidth)),
            ('\u{200D}', Some(ZeroWidth)),
            ('\u{2064}', Some(ZeroWidth)),
            ('\u{FEFF}', Some(ZeroWidth)),
            ('\u{180E}', Some(ZeroWidth)),
            ('\u{E0000}', Some(ZeroWidth)),
            ('\u{E0049}', Some(ZeroWidth)),
            ('\u{E007F}', Some(ZeroWidth)),
            ('\u{061C}', Some(BidiControl)),
            ('\u{200E}', Some(BidiControl)),
            ('\u{202E}', Some(BidiControl)),
            ('\u{2066}', Some(BidiControl)),
            ('\u{2069}', Some(BidiControl)),
            ('\u{E000}', Some(PrivateUse)),
            ('\u{F8FF}', Some(PrivateUse)),
            ('\u{F0000}', Some(PrivateUse)),
            ('\u{10FFFD}', Some(PrivateUse)),
            ('a', None),
            ('é', None),
            ('\u{00AD}', None),
            ('\u{2065}', None),
            ('\u{E0080}', None),
            ('\u{F900}', None),
            ('\u{202F}', None),
        ] {
            assert_eq!(character_reason(c), reason, "U+{:04X}", u32::from(c));
        }
    }

    #[test]
    fn a_transparency_group_hides_its_text_until_it_ends() {
        let glyphs = classified(&format!("q /GS0 gs /Fm0 Do Q {}", line(300, 0, "After")));
        // "After" reads first and is not flagged: the group's opacity ended with it.
        assert_eq!(flagged(&glyphs, Transparent), "Insidethegroup", "the group's opacity is 0 and its text is opaque inside it");
    }
}
