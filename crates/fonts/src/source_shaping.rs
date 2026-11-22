//! Shaping an explicitly selected source/document face, with OpenType features and UAX #9.
use harfrust::{BufferClusterLevel, Direction, ShapeOptions, ShaperData, UnicodeBuffer};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, GlyphId};
use unicode_bidi::{Level, ParagraphBidiInfo};

#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    pub id: GlyphId,
    pub position: [f64; 2],
    pub text: String,
}
#[derive(Clone, Debug)]
pub struct ShapedRun {
    pub glyphs: Vec<ShapedGlyph>,
    pub advance: f64,
}

/// Complex scripts/marks require shaping instead of independently encoding characters.
pub fn required(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(u32::from(c), 0x0300..=0x036f | 0x0590..=0x0fff | 0x1780..=0x17ff | 0x200c..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069))
}

/// Whether a Thai source program retains its mark positioning.
/// Character coverage alone does not show that stacked vowels and tones can be laid out.
/// OpenType Thai requires `mark`/`mkmk` (GPOS); `ccmp` substitution (GSUB) is optional.
pub fn has_thai_layout(bytes: &[u8]) -> bool {
    FontRef::new(bytes).is_ok_and(|f| positions_thai_marks(&f))
}

/// Whether the GPOS features the shaper uses for Thai include `mark` or `mkmk`. The selection is
/// HarfRust's, through the same read-fonts calls: the `thai` script, else `DFLT`, `dflt` or
/// `latn` (`ScriptList::select`); then, given no language, a language system tagged `dflt`,
/// else the script's default one. Marks under a script or language it doesn't select don't
/// count, nor does a GPOS with only kerning.
fn positions_thai_marks(face: &FontRef) -> bool {
    use skrifa::Tag;
    let Ok(gpos) = face.gpos() else { return false };
    let (Ok(scripts), Ok(features)) = (gpos.script_list(), gpos.feature_list()) else { return false };
    let records = features.feature_records();
    let marks = |index: u16| records.get(usize::from(index)).is_some_and(|r| r.feature_tag() == *b"mark" || r.feature_tag() == *b"mkmk");
    let Some(selected) = scripts.select(&[Tag::new(b"thai")]) else { return false };
    let Ok(script) = scripts.get(selected.index) else { return false };
    let language = match script.lang_sys_index_for_tag(Tag::new(b"dflt")) {
        Some(index) => script.lang_sys(index).ok().map(|l| l.element),
        None => script.default_lang_sys().transpose().ok().flatten(),
    };
    let Some(language) = language else { return false };
    language.feature_indices().iter().any(|i| marks(i.get()))
        || (language.required_feature_index() != 0xFFFF && marks(language.required_feature_index()))
}

/// Whether `program`, as a page embeds it (not with a cmap the editor rebuilt from ToUnicode),
/// gives the Arabic letters of `text` their joining forms, judged against `reference`, a complete
/// Arabic face. Its own cmap must map every letter, and wherever the reference's shaping draws a
/// letter in a glyph other than its nominal one (a joining form or part of a ligature), the
/// program's shaping must too. A cmap rebuilt from ToUnicode maps a letter to one of its
/// positional forms, which the GSUB doesn't substitute; a GSUB without the joining features, or
/// pruned to the forms a page showed, leaves letters isolated. Either way every glyph may have
/// ink and still be the wrong form. A program whose joining form reuses its nominal glyph is
/// refused too, which only costs the page's font.
pub fn joins_arabic(program: &[u8], reference: &[u8], text: &str) -> bool {
    use skrifa::MetadataProvider as _;
    let (Ok(own), Ok(reference)) = (FontRef::new(program), FontRef::new(reference)) else { return false };
    if text.len() > 262_144 {
        return false;
    }
    let (own_map, reference_map) = (own.charmap(), reference.charmap());
    let joins = |c: char| matches!(u32::from(c), 0x0600..=0x06ff | 0x0750..=0x077f | 0x0870..=0x08ff | 0x200c | 0x200d);
    let letter = |c: char| matches!(u32::from(c), 0x0620..=0x064a | 0x066e..=0x06d5 | 0x06ee..=0x06ef | 0x06fa..=0x06ff | 0x0750..=0x077f | 0x0870..=0x08ff);
    // Any other character ends a run of joining, so each run of Arabic shapes alone as in place.
    text.split(|c: char| !joins(c)).filter(|run| !run.is_empty()).all(|run| {
        let (ours, theirs) = (rtl_clusters(&own, run), rtl_clusters(&reference, run));
        run.char_indices().filter(|(_, c)| letter(*c)).all(|(at, c)| {
            let (Some(nominal), Some(reference_nominal)) = (own_map.map(c), reference_map.map(c)) else { return false };
            let (ours, theirs) = (cluster_at(&ours, at), cluster_at(&theirs, at));
            !ours.is_empty() && !theirs.is_empty() && (theirs.contains(&reference_nominal.to_u32()) || !ours.contains(&nominal.to_u32()))
        })
    })
}

/// Each glyph of `text` shaped right to left in `face`, with the byte offset its cluster starts at.
fn rtl_clusters(face: &FontRef, text: &str) -> Vec<(usize, u32)> {
    let data = ShaperData::new(face);
    let shaper = data.shaper(face).build();
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.set_cluster_level(BufferClusterLevel::MonotoneGraphemes);
    buffer.set_direction(Direction::RightToLeft);
    buffer.guess_segment_properties();
    shaper.shape(buffer, ShapeOptions::new()).glyph_infos().iter().map(|g| (g.cluster as usize, g.glyph_id)).collect()
}

/// The glyphs of the cluster that holds byte `at`.
fn cluster_at(glyphs: &[(usize, u32)], at: usize) -> Vec<u32> {
    let start = glyphs.iter().map(|(s, _)| *s).filter(|s| *s <= at).max();
    glyphs.iter().filter(|(s, _)| Some(*s) == start).map(|(_, g)| *g).collect()
}

pub fn shape(bytes: &[u8], text: &str) -> Result<ShapedRun, String> {
    shape_inner(bytes, text, false)
}

/// Only used after the PDF adapter has verified legacy Thai PUA glyph mappings.
pub(crate) fn shape_legacy_thai(bytes: &[u8], text: &str) -> Result<ShapedRun, String> {
    shape_inner(bytes, text, true)
}

fn shape_inner(bytes: &[u8], text: &str, legacy_thai: bool) -> Result<ShapedRun, String> {
    if text.len() > 262_144 || text.contains(['\n', '\r']) {
        return Err("shape one bounded text row at a time".into());
    }
    if text.is_empty() {
        return Ok(ShapedRun { glyphs: Vec::new(), advance: 0.0 });
    }
    let face = FontRef::new(bytes).map_err(|e| e.to_string())?;
    // Only a vowel or tone mark set on its base (or Sara Am, whose Nikhahit is) needs positioning:
    // digits and bare letters shape the same without it.
    if !legacy_thai && text.chars().any(|c| matches!(u32::from(c), 0x0e31 | 0x0e33..=0x0e3a | 0x0e47..=0x0e4e)) && !positions_thai_marks(&face) {
        return Err("the source subset has no Thai mark positioning (GPOS mark/mkmk); select its full font".into());
    }
    if text.chars().any(|c| matches!(u32::from(c), 0x0600..=0x06ff | 0x0750..=0x077f | 0x08a0..=0x0dff | 0x0e80..=0x0fff | 0x1780..=0x17ff))
        && face.gsub().is_err()
    {
        return Err("the source subset has no contextual substitution tables; select its full font".into());
    }
    let data = ShaperData::new(&face);
    let shaper = data.shaper(&face).build();
    let em = f64::from(shaper.units_per_em().max(1));
    let base = if unicode_bidi::get_base_direction(text) == unicode_bidi::Direction::Rtl { Level::rtl() } else { Level::ltr() };
    let bidi = ParagraphBidiInfo::new(text, Some(base));
    let (levels, runs) = bidi.visual_runs(0..text.len());
    let mut glyphs = Vec::new();
    let mut x = 0.0;
    for run in runs {
        let Some(input) = text.get(run.clone()) else { continue };
        let rtl = levels.get(run.start).is_some_and(Level::is_rtl);
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(input);
        buffer.set_cluster_level(BufferClusterLevel::MonotoneGraphemes);
        buffer.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
        buffer.guess_segment_properties();
        let output = shaper.shape(buffer, ShapeOptions::new());
        let mut starts: Vec<_> = output.glyph_infos().iter().map(|g| g.cluster as usize).collect();
        starts.sort_unstable();
        starts.dedup();
        let mut seen = std::collections::HashSet::new();
        for (g, p) in output.glyph_infos().iter().zip(output.glyph_positions()) {
            if g.glyph_id == 0 {
                let missing = input.get(g.cluster as usize..).and_then(|s| s.chars().next()).unwrap_or('\u{fffd}');
                return Err(format!(
                    "the full font lacks a required contextual glyph for {missing} (U+{:04X}); choose the complete source face",
                    u32::from(missing)
                ));
            }
            let start = g.cluster as usize;
            let end = starts.get(starts.partition_point(|i| *i <= start)).copied().unwrap_or(input.len());
            let logical = if seen.insert(start) { input.get(start..end).unwrap_or_default().to_string() } else { String::new() };
            glyphs.push(ShapedGlyph {
                id: GlyphId::new(g.glyph_id),
                position: [x + f64::from(p.x_offset) / em, f64::from(p.y_offset) / em],
                text: logical,
            });
            x += f64::from(p.x_advance) / em;
        }
    }
    if !x.is_finite() || glyphs.len() > 65_534 {
        return Err("shaped row exceeds its geometry budget".into());
    }
    Ok(ShapedRun { glyphs, advance: x })
}

/// Whether `bytes`, the program that shaped `run`, draws every glyph of it. A subset that keeps
/// glyph ids empties the glyphs it drops (`glyf`/`loca` or CFF charstrings with no contour), and
/// its cmap and layout tables may still lead to them; they would show nothing. `.notdef` never
/// counts. Empty glyphs are real only for white space: the space glyph in a cluster with white
/// space or a default-ignorable character such as a joiner (the shaper gives those the space
/// glyph), and any glyph of a cluster of white space.
pub fn draws_every_glyph(bytes: &[u8], run: &ShapedRun) -> bool {
    use skrifa::MetadataProvider as _;
    use skrifa::instance::{LocationRef, Size};
    use skrifa::outline::{DrawSettings, OutlinePen};
    /// Whether anything but a bare move was drawn.
    struct Ink(bool);
    impl OutlinePen for Ink {
        fn move_to(&mut self, _: f32, _: f32) {}
        fn line_to(&mut self, _: f32, _: f32) {
            self.0 = true;
        }
        fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {
            self.0 = true;
        }
        fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {
            self.0 = true;
        }
        fn close(&mut self) {}
    }
    let Ok(face) = FontRef::new(bytes) else { return false };
    let (outlines, space) = (face.outline_glyphs(), face.charmap().map(' '));
    // A cluster's text is on its first glyph; the others carry none.
    let mut cluster = "";
    run.glyphs.iter().all(|glyph| {
        if !glyph.text.is_empty() {
            cluster = &glyph.text;
        }
        let inked = || {
            let mut ink = Ink(false);
            outlines.get(glyph.id).is_some_and(|o| o.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut ink).is_ok())
                && ink.0
        };
        // The space glyph only where the cluster has something it may stand for: a cmap may map
        // a letter to that glyph id too.
        let blank = |c: char| c.is_whitespace() || default_ignorable(c);
        glyph.id.to_u32() != 0
            && ((Some(glyph.id) == space && cluster.chars().any(blank))
                || (!cluster.is_empty() && cluster.chars().all(char::is_whitespace))
                || inked())
    })
}

/// Unicode's Default_Ignorable_Code_Point characters (joiners, bidi controls, variation
/// selectors and the like), which a shaper hides behind the space glyph.
fn default_ignorable(c: char) -> bool {
    matches!(
        u32::from(c),
        0x00ad
            | 0x034f
            | 0x061c
            | 0x115f..=0x1160
            | 0x17b4..=0x17b5
            | 0x180b..=0x180f
            | 0x200b..=0x200f
            | 0x202a..=0x202e
            | 0x2060..=0x206f
            | 0x3164
            | 0xfe00..=0xfe0f
            | 0xfeff
            | 0xffa0
            | 0xfff0..=0xfff8
            | 0x1bca0..=0x1bca3
            | 0x1d173..=0x1d17a
            | 0xe0000..=0xe0fff
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the GPOS table starts in an sfnt.
    fn gpos(bytes: &[u8]) -> usize {
        let count = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        let record = (0..count).map(|i| 12 + i * 16).find(|&r| &bytes[r..r + 4] == b"GPOS").unwrap();
        u32::from_be_bytes(bytes[record + 8..record + 12].try_into().unwrap()) as usize
    }

    fn u16_at(bytes: &[u8], at: usize) -> usize {
        u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize
    }

    /// Where the GPOS ScriptList starts.
    fn script_list(bytes: &[u8]) -> usize {
        gpos(bytes) + u16_at(bytes, gpos(bytes) + 4)
    }

    /// Where script `tag`'s table starts.
    fn script(bytes: &[u8], tag: &[u8; 4]) -> usize {
        let list = script_list(bytes);
        let record = (0..u16_at(bytes, list)).map(|i| list + 2 + i * 6).find(|&r| &bytes[r..r + 4] == tag).unwrap();
        list + u16_at(bytes, record + 4)
    }

    /// Retag script `from` as `to`, keeping the records sorted by tag as the shaper's binary
    /// search needs.
    fn retag_script(bytes: &mut [u8], from: &[u8; 4], to: &[u8; 4]) {
        let list = script_list(bytes);
        let count = u16_at(bytes, list);
        let mut records: Vec<[u8; 6]> = (0..count).map(|i| bytes[list + 2 + i * 6..list + 8 + i * 6].try_into().unwrap()).collect();
        records.iter_mut().find(|r| &r[..4] == from).unwrap()[..4].copy_from_slice(to);
        records.sort_by(|a, b| a[..4].cmp(&b[..4]));
        for (i, record) in records.iter().enumerate() {
            bytes[list + 2 + i * 6..list + 8 + i * 6].copy_from_slice(record);
        }
    }

    /// A shaper uses a font's `thai` script when it has one: marks the font registers only for
    /// Latin don't position Thai.
    #[test]
    fn thai_layout_counts_only_the_script_the_shaper_selects() {
        let mut bytes = crate::script::INTER_REGULAR.to_vec();
        // Inter's DFLT becomes a `thai` script whose default language system has no features;
        // its latn script keeps `mark` and `mkmk`.
        retag_script(&mut bytes, b"DFLT", b"thai");
        let thai = script(&bytes, b"thai");
        let language = thai + u16_at(&bytes, thai);
        assert_ne!(language, thai, "a default language system");
        bytes[language + 2..language + 6].copy_from_slice(&[0xFF, 0xFF, 0, 0]);
        assert!(!has_thai_layout(&bytes), "the latn script's marks are not the thai script's");
    }

    /// Given no language, the shaper takes a language system tagged `dflt` before the script's
    /// default one, in both directions.
    #[test]
    fn thai_layout_follows_a_language_system_tagged_dflt() {
        // Without DFLT the shaper falls back to latn, whose default language system has marks and
        // is shared by its CAT, MOL and ROM records. Cyrillic's default one has only `kern`.
        let mut base = crate::script::INTER_REGULAR.to_vec();
        retag_script(&mut base, b"DFLT", b"zzzz");
        assert!(has_thai_layout(&base));
        let latn = script(&base, b"latn");
        let kern_only = script(&base, b"cyrl") + u16_at(&base, script(&base, b"cyrl"));
        let kern_only = u16::try_from(kern_only - latn).unwrap().to_be_bytes();
        let rom = (0..u16_at(&base, latn + 2)).map(|j| latn + 4 + j * 6).find(|&r| &base[r..r + 4] == b"ROM ").unwrap();
        // ROM becomes `dflt` (still sorted last) and has only kerning: it wins over the default.
        let mut named_without = base.clone();
        named_without[rom..rom + 4].copy_from_slice(b"dflt");
        named_without[rom + 4..rom + 6].copy_from_slice(&kern_only);
        assert!(!has_thai_layout(&named_without), "the dflt language system has no marks");
        // The default has only kerning and the `dflt` one keeps the marks: it wins again.
        let mut default_without = base.clone();
        default_without[rom..rom + 4].copy_from_slice(b"dflt");
        default_without[latn..latn + 2].copy_from_slice(&kern_only);
        assert!(has_thai_layout(&default_without), "the dflt language system has the marks");
    }

    /// A GPOS table alone is not mark positioning: with its `mark` and `mkmk` features renamed,
    /// as in a kerning-only subset, a face can't stack Thai vowels and tones.
    #[test]
    fn thai_layout_needs_mark_positioning_features() {
        let inter = crate::script::INTER_REGULAR;
        assert!(has_thai_layout(inter), "Inter positions marks under its default script");
        let mut bytes = inter.to_vec();
        let table = gpos(&bytes);
        let features = table + u16_at(&bytes, table + 6);
        let mut renamed = 0;
        for i in 0..u16_at(&bytes, features) {
            let tag = features + 2 + i * 6;
            if matches!(&bytes[tag..tag + 4], b"mark" | b"mkmk") {
                bytes[tag..tag + 4].copy_from_slice(b"kern");
                renamed += 1;
            }
        }
        assert!(renamed > 0);
        assert!(FontRef::new(&bytes).unwrap().gpos().is_ok(), "the GPOS table still parses");
        assert!(!has_thai_layout(&bytes));
        let error = shape(&bytes, "กิ่").unwrap_err();
        assert!(error.contains("mark positioning"), "{error}");
    }

    #[test]
    fn thai_positioning_does_not_require_optional_gsub() {
        let Some(face) = crate::document_thai_font() else {
            eprintln!("skipping thai_positioning_does_not_require_optional_gsub: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
            return;
        };
        let mut bytes = face.bytes.to_vec();
        // Keep the actual GPOS anchors and outlines while removing optional substitutions.
        // Table payloads stay at their original offsets; no font fixture is committed.
        let count = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        let tables: Vec<_> = bytes[12..12 + count * 16].as_chunks::<16>().0.iter().filter(|t| &t[..4] != b"GSUB").map(|t| t.to_vec()).collect();
        bytes[4..6].copy_from_slice(&(tables.len() as u16).to_be_bytes());
        for (i, table) in tables.iter().enumerate() {
            bytes[12 + i * 16..28 + i * 16].copy_from_slice(table);
        }
        assert!(FontRef::new(&bytes).unwrap().gsub().is_err());
        assert!(has_thai_layout(&bytes));
        let shaped = shape(&bytes, "กิ่ น้ำ").unwrap();
        assert!(shaped.advance > 0.0);
        assert!(shaped.glyphs.iter().all(|g| g.position.iter().all(|p| p.is_finite())));
    }
}
