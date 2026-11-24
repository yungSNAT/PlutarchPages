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
    FontRef::new(bytes).is_ok_and(|f| positions_marks(&f, b"thai"))
}

/// Whether the GPOS features the shaper uses for `script` include `mark` or `mkmk`
/// ([`mark_lookups`]).
fn positions_marks(face: &FontRef, script: &[u8; 4]) -> bool {
    mark_lookups(face, script).is_some()
}

/// The lookups of the `mark` and `mkmk` features the shaper uses for `script`, if it uses either.
/// The selection is HarfRust's, through the same read-fonts calls: the `script` script, else
/// `DFLT`, `dflt` or `latn` (`ScriptList::select`); then, given no language, a language system
/// tagged `dflt`, else the script's default one. Marks under a script or language it doesn't
/// select don't count, nor does a GPOS with only kerning.
fn mark_lookups(face: &FontRef, script: &[u8; 4]) -> Option<Vec<u16>> {
    use skrifa::Tag;
    let gpos = face.gpos().ok()?;
    let (scripts, features) = (gpos.script_list().ok()?, gpos.feature_list().ok()?);
    let records = features.feature_records();
    let selected = scripts.select(&[Tag::new(script)])?;
    let script = scripts.get(selected.index).ok()?;
    let language = match script.lang_sys_index_for_tag(Tag::new(b"dflt")) {
        Some(index) => script.lang_sys(index).ok().map(|l| l.element),
        None => script.default_lang_sys().transpose().ok().flatten(),
    }?;
    let required = Some(language.required_feature_index()).filter(|&i| i != 0xFFFF);
    let marks: Vec<_> = language
        .feature_indices()
        .iter()
        .map(|i| i.get())
        .chain(required)
        .filter_map(|i| records.get(usize::from(i)))
        .filter(|r| r.feature_tag() == *b"mark" || r.feature_tag() == *b"mkmk")
        .collect();
    (!marks.is_empty()).then(|| {
        marks
            .iter()
            .filter_map(|r| r.feature(features.offset_data()).ok())
            .flat_map(|f| f.lookup_list_indices().iter().map(|i| i.get()).collect::<Vec<_>>())
            .collect()
    })
}

/// Whether one of `lookups` (mark-to-base or mark-to-mark) attaches `mark` to `onto`: both are
/// covered, and `onto` has an anchor for `mark`'s class.
fn attaches(gpos: &skrifa::raw::tables::gpos::Gpos, lookups: &[u16], mark: u32, onto: u32) -> bool {
    use skrifa::raw::tables::gpos::PositionSubtables;
    let anchored = |offsets: &[skrifa::raw::types::BigEndian<skrifa::raw::types::Nullable<skrifa::raw::types::Offset16>>], class: u16| {
        offsets.get(usize::from(class)).is_some_and(|o| !o.get().is_null())
    };
    let Ok(list) = gpos.lookup_list() else { return false };
    let (mark, onto) = (GlyphId::new(mark), GlyphId::new(onto));
    lookups.iter().filter_map(|&i| list.lookups().get(usize::from(i)).ok()).any(|lookup| match lookup.subtables() {
        Ok(PositionSubtables::MarkToBase(subtables)) => subtables.iter().flatten().any(|t| {
            let (Ok(marks), Ok(bases), Ok(classes), Ok(anchors)) = (t.mark_coverage(), t.base_coverage(), t.mark_array(), t.base_array()) else {
                return false;
            };
            let (Some(m), Some(b)) = (marks.get(mark), bases.get(onto)) else { return false };
            let Some(class) = classes.mark_records().get(usize::from(m)).map(|r| r.mark_class()) else { return false };
            anchors.base_records().get(usize::from(b)).is_ok_and(|r| anchored(r.base_anchor_offsets(), class))
        }),
        Ok(PositionSubtables::MarkToMark(subtables)) => subtables.iter().flatten().any(|t| {
            let (Ok(marks), Ok(bases), Ok(classes), Ok(anchors)) = (t.mark1_coverage(), t.mark2_coverage(), t.mark1_array(), t.mark2_array()) else {
                return false;
            };
            let (Some(m), Some(b)) = (marks.get(mark), bases.get(onto)) else { return false };
            let Some(class) = classes.mark_records().get(usize::from(m)).map(|r| r.mark_class()) else { return false };
            anchors.mark2_records().get(usize::from(b)).is_ok_and(|r| anchored(r.mark2_anchor_offsets(), class))
        }),
        _ => false,
    })
}

/// Whether `program`, as a page embeds it (not with a cmap the editor rebuilt from ToUnicode),
/// gives the Arabic letters of `text` their joining forms, judged against `reference`, a complete
/// Arabic face, which must draw every letter itself. Its own cmap must map every letter; each
/// letter's isolated, initial, medial and final forms must differ wherever the reference's
/// differ ([`tells_forms_apart`]); and wherever the reference's shaping of `text` draws a letter
/// in glyphs other than those it draws the letter and its marks with alone (a joining form or
/// part of a ligature), the program's must too, each face judged against its own isolated form;
/// and its GPOS must place every glyph that sits on another ([`places_marks`]) and stack marks
/// wherever the reference does ([`stacks_marks`]). A cmap that maps
/// a letter to one of its positional forms (rebuilt from ToUnicode, or written from the glyphs a
/// page showed) leaves the GSUB with nothing to substitute, or substitutes the wrong form; a GSUB without the joining features, pruned to the forms a page
/// showed, or pointing past its lookups leaves letters isolated. Either way every glyph may have
/// ink and still be the wrong form. A program whose joining form reuses another form's glyph is
/// refused too, which only costs the page's font.
pub fn joins_arabic(program: &[u8], reference: &[u8], text: &str) -> bool {
    use skrifa::MetadataProvider as _;
    let (Ok(own), Ok(reference)) = (FontRef::new(program), FontRef::new(reference)) else { return false };
    if text.len() > 262_144 {
        return false;
    }
    let (own_data, reference_data) = (ShaperData::new(&own), ShaperData::new(&reference));
    let (own_shaper, reference_shaper) = (own_data.shaper(&own).build(), reference_data.shaper(&reference).build());
    let own_map = own.charmap();
    let joins = |c: char| matches!(u32::from(c), 0x0600..=0x06ff | 0x0750..=0x077f | 0x0870..=0x08ff | 0x200c | 0x200d);
    let mark = |c: char| matches!(u32::from(c), 0x0610..=0x061a | 0x064b..=0x065f | 0x0670 | 0x06d6..=0x06dc | 0x06df..=0x06e4 | 0x06e7..=0x06e8 | 0x06ea..=0x06ed | 0x08ca..=0x08ff);
    let letter = |c: char| {
        !mark(c) && matches!(u32::from(c), 0x0620..=0x064a | 0x066e..=0x06d5 | 0x06ee..=0x06ef | 0x06fa..=0x06ff | 0x0750..=0x077f | 0x0870..=0x08ff)
    };
    let letters: std::collections::BTreeSet<char> = text.chars().filter(|c| letter(*c)).collect();
    if !letters.iter().all(|&c| own_map.map(c).is_some() && tells_forms_apart(&own_shaper, &reference_shaper, c)) {
        return false;
    }
    let space = |face: &FontRef| face.charmap().map(' ').map(|g| g.to_u32());
    let (own_space, reference_space) = (space(&own), space(&reference));
    // Any other character ends a run of joining, so each run of Arabic shapes alone as in place.
    text.split(|c: char| !joins(c)).filter(|run| !run.is_empty()).all(|run| {
        if !places_marks(&own_shaper, own_space, run) || !stacks_marks((&own_shaper, own_space), (&reference_shaper, reference_space), run) {
            return false;
        }
        let (ours, theirs) = (rtl_clusters(&own_shaper, run), rtl_clusters(&reference_shaper, run));
        run.char_indices().filter(|(_, c)| letter(*c)).all(|(at, c)| {
            // Each face against its own isolated form of the letter with its marks, which share
            // its cluster: a face may draw a letter as a base and its dots even when it stands alone.
            let marks = run.get(at + c.len_utf8()..).unwrap_or_default().chars().take_while(|&m| mark(m));
            let alone: String = std::iter::once(c).chain(marks).collect();
            let isolated = |shaper: &harfrust::Shaper| cluster_at(&rtl_clusters(shaper, &alone), 0);
            let (ours, theirs) = (cluster_at(&ours, at), cluster_at(&theirs, at));
            drawn(&ours) && drawn(&theirs) && (theirs == isolated(&reference_shaper) || ours != isolated(&own_shaper))
        })
    })
}

/// Whether `own` draws `letter`'s isolated, initial, medial and final forms (set against tatweels,
/// which join on both sides) as distinct glyphs wherever `reference` does, both drawing each form
/// in glyphs of their own.
fn tells_forms_apart(own: &harfrust::Shaper, reference: &harfrust::Shaper, letter: char) -> bool {
    let forms = |shaper: &harfrust::Shaper| {
        let (t, after) = ('\u{0640}', '\u{0640}'.len_utf8());
        [(format!("{letter}"), 0), (format!("{letter}{t}"), 0), (format!("{t}{letter}{t}"), after), (format!("{t}{letter}"), after)]
            .map(|(text, at)| cluster_at(&rtl_clusters(shaper, &text), at))
    };
    let (ours, theirs) = (forms(own), forms(reference));
    ours.iter().chain(&theirs).all(|glyphs| drawn(glyphs)) && (0..4).all(|i| (0..i).all(|j| theirs[i] == theirs[j] || ours[i] != ours[j]))
}

/// Whether a cluster has glyphs and none of them is `.notdef`.
fn drawn(glyphs: &[u32]) -> bool {
    !glyphs.is_empty() && !glyphs.contains(&0)
}

/// Whether the shaper's GPOS places every glyph of `run` that sits on another: in each cluster of
/// several glyphs (the space glyph a joiner gets aside), all but one, the base, must move when
/// `mark` and `mkmk` are turned off. That holds for the marks the text has and for those the GSUB
/// makes, such as a letter's dots drawn as glyphs of their own; feature names alone don't count,
/// so a GPOS that is missing, or whose `mark` lookups are empty or miss these glyphs, fails; a
/// mark stacked on another is [`stacks_marks`]'s to judge.
/// A program that draws its marks in place without GPOS is refused too, which only costs the
/// page's font.
fn places_marks(shaper: &harfrust::Shaper, space: Option<u32>, run: &str) -> bool {
    moved_without(shaper, space, run, &[b"mark", b"mkmk"])
        .is_some_and(|clusters| clusters.values().all(|&(glyphs, unmoved)| glyphs < 2 || unmoved <= 1))
}

/// Whether the GPOS places the marks of `run`, one bidi run shaped in its direction by `face`
/// with `lookups` (its `mark` and `mkmk` lookups for Thai): each cluster must have as many placed
/// glyphs as combining marks, Sara Am counting for its Nikhahit. A glyph is placed if it moves when
/// `mark` and `mkmk` are turned off, or if one of `lookups` attaches it to a glyph before it in the
/// cluster ([`attaches`]): a face may draw a mark where it sits over most bases, so its anchors
/// move it only over some. Feature names alone don't count: a GPOS whose lookups are empty or
/// miss these glyphs would leave each mark at its glyph origin. A cluster whose GSUB merges 2
/// marks into one glyph is refused too, which only costs the page's font.
fn places_thai_marks(face: &FontRef, lookups: &[u16], shaper: &harfrust::Shaper, (run, direction): (&str, Direction)) -> bool {
    let Ok(gpos) = face.gpos() else { return false };
    let off = [b"mark", b"mkmk"].map(|tag| harfrust::Feature::new(harfrust::Tag::new(tag), 0, ..));
    let (with, without) = (shape_in(shaper, run, direction, &[]), shape_in(shaper, run, direction, &off));
    if with.glyph_infos().iter().map(|g| g.glyph_id).ne(without.glyph_infos().iter().map(|g| g.glyph_id)) {
        return false;
    }
    let at = |p: &harfrust::GlyphPosition| (p.x_advance, p.y_advance, p.x_offset, p.y_offset);
    let mut clusters = std::collections::BTreeMap::<usize, (Vec<u32>, usize)>::new();
    for ((info, on), off) in with.glyph_infos().iter().zip(with.glyph_positions()).zip(without.glyph_positions()) {
        let (glyphs, placed) = clusters.entry(info.cluster as usize).or_default();
        if !glyphs.is_empty() && (at(on) != at(off) || glyphs.iter().any(|&onto| attaches(&gpos, lookups, info.glyph_id, onto))) {
            *placed += 1;
        }
        glyphs.push(info.glyph_id);
    }
    let marks = |text: &str| text.chars().filter(|&c| c == '\u{0e33}' || unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::NSM).count();
    let starts: Vec<usize> = clusters.keys().copied().collect();
    clusters.values().zip(&starts).enumerate().all(|(i, ((_, placed), &start))| {
        let end = starts.get(i + 1).copied().unwrap_or(run.len());
        run.get(start..end).is_some_and(|text| *placed >= marks(text))
    })
}

/// Whether `own` stacks as many marks with its GPOS (`mkmk`) as `reference` does, in every cluster
/// of `run`. Without the mark-to-mark data for a pair, the later mark (fatha on shadda, or sukun on
/// that fatha) is attached to the base's anchor, on top of the first, and still moves, so
/// [`places_marks`] can't tell. This shows that `mkmk` acts on each stacked mark, not where it puts
/// it. Where the reference stacks, the cluster must also have as many glyphs as the reference's,
/// so a glyph the reference lacks (a letter's dots drawn apart) can't stand in for a stacked mark.
/// A face that draws such a cluster in other glyphs is refused, which only costs the page's font.
fn stacks_marks(own: (&harfrust::Shaper, Option<u32>), reference: (&harfrust::Shaper, Option<u32>), run: &str) -> bool {
    let stacked = |(shaper, space): (&harfrust::Shaper, Option<u32>)| {
        moved_without(shaper, space, run, &[b"mkmk"]).map(|clusters| {
            clusters.into_iter().map(|(c, (glyphs, unmoved))| (c, (glyphs, glyphs - unmoved))).collect::<std::collections::BTreeMap<_, _>>()
        })
    };
    match (stacked(own), stacked(reference)) {
        (Some(ours), Some(theirs)) => {
            theirs.iter().all(|(c, &(glyphs, moved))| moved == 0 || ours.get(c).is_some_and(|&(g, m)| g == glyphs && m >= moved))
        }
        _ => false,
    }
}

/// Per cluster of `run` (the space glyph a joiner gets aside): its glyphs, and how many of them
/// keep their advance and offset when `features` are turned off. `None` if turning them off
/// changes the glyphs.
fn moved_without(
    shaper: &harfrust::Shaper,
    space: Option<u32>,
    run: &str,
    features: &[&[u8; 4]],
) -> Option<std::collections::BTreeMap<u32, (usize, usize)>> {
    let off: Vec<_> = features.iter().map(|&tag| harfrust::Feature::new(harfrust::Tag::new(tag), 0, ..)).collect();
    let (with, without) = (rtl_shape(shaper, run, &[]), rtl_shape(shaper, run, &off));
    let at = |p: &harfrust::GlyphPosition| (p.x_advance, p.y_advance, p.x_offset, p.y_offset);
    if with.glyph_infos().iter().map(|g| g.glyph_id).ne(without.glyph_infos().iter().map(|g| g.glyph_id)) {
        return None;
    }
    let mut clusters = std::collections::BTreeMap::<u32, (usize, usize)>::new();
    for ((info, on), off) in with.glyph_infos().iter().zip(with.glyph_positions()).zip(without.glyph_positions()) {
        if Some(info.glyph_id) != space {
            let (glyphs, unmoved) = clusters.entry(info.cluster).or_default();
            *glyphs += 1;
            *unmoved += usize::from(at(on) == at(off));
        }
    }
    Some(clusters)
}

/// `text` shaped right to left with `features`.
fn rtl_shape(shaper: &harfrust::Shaper, text: &str, features: &[harfrust::Feature]) -> harfrust::GlyphBuffer {
    shape_in(shaper, text, Direction::RightToLeft, features)
}

/// `text` shaped in `direction` with `features`.
fn shape_in(shaper: &harfrust::Shaper, text: &str, direction: Direction, features: &[harfrust::Feature]) -> harfrust::GlyphBuffer {
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.set_cluster_level(BufferClusterLevel::MonotoneGraphemes);
    buffer.set_direction(direction);
    buffer.guess_segment_properties();
    shaper.shape(buffer, ShapeOptions::new().features(features))
}

/// Each glyph of `text` shaped right to left, with the byte offset its cluster starts at.
fn rtl_clusters(shaper: &harfrust::Shaper, text: &str) -> Vec<(usize, u32)> {
    rtl_shape(shaper, text, &[]).glyph_infos().iter().map(|g| (g.cluster as usize, g.glyph_id)).collect()
}

/// The glyphs, sorted, of the cluster that holds byte `at`.
fn cluster_at(glyphs: &[(usize, u32)], at: usize) -> Vec<u32> {
    let start = glyphs.iter().map(|(s, _)| *s).filter(|s| *s <= at).max();
    let mut cluster: Vec<u32> = glyphs.iter().filter(|(s, _)| Some(*s) == start).map(|(_, g)| *g).collect();
    cluster.sort_unstable();
    cluster
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
    let thai_marks = |text: &str| text.chars().any(|c| matches!(u32::from(c), 0x0e31 | 0x0e33..=0x0e3a | 0x0e47..=0x0e4e));
    let no_thai_positioning = || "the source subset has no Thai mark positioning (GPOS mark/mkmk); select its full font".to_string();
    let thai_lookups = if !legacy_thai && thai_marks(text) { Some(mark_lookups(&face, b"thai").ok_or_else(no_thai_positioning)?) } else { None };
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
        let direction = if levels.get(run.start).is_some_and(Level::is_rtl) { Direction::RightToLeft } else { Direction::LeftToRight };
        if let Some(lookups) = &thai_lookups
            && thai_marks(input)
            && !places_thai_marks(&face, lookups, &shaper, (input, direction))
        {
            return Err(no_thai_positioning());
        }
        let output = shape_in(&shaper, input, direction, &[]);
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
    let blank = |c: char| c.is_whitespace() || default_ignorable(c);
    let inked = |id: GlyphId| {
        let mut ink = Ink(false);
        outlines.get(id).is_some_and(|o| o.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut ink).is_ok()) && ink.0
    };
    // A cluster's text is on its first glyph; the others carry none.
    let mut clusters: Vec<(&str, Vec<GlyphId>)> = Vec::new();
    for glyph in &run.glyphs {
        if !glyph.text.is_empty() || clusters.is_empty() {
            clusters.push((&glyph.text, Vec::new()));
        }
        if let Some((_, ids)) = clusters.last_mut() {
            ids.push(glyph.id);
        }
    }
    // The space glyph may stand for each white-space or default-ignorable character of its cluster
    // once (a joiner shares its letter's cluster), and a cluster with anything else must draw ink:
    // a cmap may map a letter to the space glyph's id too.
    clusters.iter().all(|(text, ids)| {
        let mut blanks = text.chars().filter(|&c| blank(c)).count();
        let (white, mut drew) = (!text.is_empty() && text.chars().all(char::is_whitespace), false);
        let each = ids.iter().all(|&id| {
            let stands_for_blank = Some(id) == space && blanks > 0;
            blanks -= usize::from(stands_for_blank);
            let ink = !stands_for_blank && !white && inked(id);
            drew |= ink;
            id.to_u32() != 0 && (stands_for_blank || white || ink)
        });
        each && (white || drew || text.chars().all(blank))
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

    /// Where `tag` starts in an sfnt.
    fn table(bytes: &[u8], tag: &[u8; 4]) -> usize {
        let count = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        let record = (0..count).map(|i| 12 + i * 16).find(|&r| &bytes[r..r + 4] == tag).unwrap();
        u32::from_be_bytes(bytes[record + 8..record + 12].try_into().unwrap()) as usize
    }

    /// Where the GPOS table starts in an sfnt.
    fn gpos(bytes: &[u8]) -> usize {
        table(bytes, b"GPOS")
    }

    fn u16_at(bytes: &[u8], at: usize) -> usize {
        u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize
    }

    fn u32_at(bytes: &[u8], at: usize) -> usize {
        u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
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

    /// Mark positioning features whose lookups place nothing are no positioning: with the
    /// bundled face's `mark` and `mkmk` lookup lists emptied, the names still select them, but a
    /// vowel or tone would be drawn at its glyph origin, so the text is refused.
    #[test]
    fn thai_mark_features_that_place_nothing_are_refused() {
        let Some(face) = crate::document_thai_font() else {
            eprintln!("skipping thai_mark_features_that_place_nothing_are_refused: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
            return;
        };
        let complete = face.bytes;
        assert!(shape(complete, "กิ่ น้ำ ปู่").is_ok());
        let mut bytes = complete.to_vec();
        let features = gpos(&bytes) + u16_at(&bytes, gpos(&bytes) + 6);
        let mut emptied = 0;
        for i in 0..u16_at(&bytes, features) {
            let record = features + 2 + i * 6;
            if matches!(&bytes[record..record + 4], b"mark" | b"mkmk") {
                let feature = features + u16_at(&bytes, record + 4);
                bytes[feature + 2..feature + 4].copy_from_slice(&[0, 0]);
                emptied += 1;
            }
        }
        assert!(emptied > 0);
        assert!(has_thai_layout(&bytes), "the features are still selected by name");
        for text in ["กิ่", "น้ำ", "ปู่"] {
            let error = shape(&bytes, text).unwrap_err();
            assert!(error.contains("mark positioning"), "{text}: {error}");
        }
        assert!(shape(&bytes, "๑๒๓กข").is_ok(), "nothing to position");
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

    fn coverage_index(bytes: &[u8], cov: usize, glyph: u16) -> Option<usize> {
        let count = u16_at(bytes, cov + 2);
        match u16_at(bytes, cov) {
            1 => (0..count).find(|&i| u16_at(bytes, cov + 4 + 2 * i) == usize::from(glyph)),
            2 => (0..count)
                .map(|i| cov + 4 + 6 * i)
                .find(|&r| (u16_at(bytes, r)..=u16_at(bytes, r + 2)).contains(&usize::from(glyph)))
                .map(|r| u16_at(bytes, r + 4) + usize::from(glyph) - u16_at(bytes, r)),
            _ => None,
        }
    }

    fn write_coverage_one(bytes: &mut [u8], cov: usize, glyph: u16) {
        let room = match u16_at(bytes, cov) {
            1 => 4 + 2 * u16_at(bytes, cov + 2),
            2 => 4 + 6 * u16_at(bytes, cov + 2),
            format => panic!("coverage format {format}"),
        };
        assert!(room >= 6);
        for (i, word) in [1u16, 1, glyph].into_iter().enumerate() {
            bytes[cov + 2 * i..cov + 2 * i + 2].copy_from_slice(&word.to_be_bytes());
        }
    }

    fn thai_mark_lookup_indices(bytes: &[u8]) -> Vec<usize> {
        let table = gpos(bytes);
        let features = table + u16_at(bytes, table + 6);
        let thai = script(bytes, b"thai");
        let lang = thai + u16_at(bytes, thai);
        let mut indices = Vec::new();
        for i in 0..u16_at(bytes, lang + 4) {
            let feature_idx = u16_at(bytes, lang + 6 + 2 * i);
            let record = features + 2 + feature_idx * 6;
            if matches!(&bytes[record..record + 4], b"mark" | b"mkmk") {
                let feature = features + u16_at(bytes, record + 4);
                for j in 0..u16_at(bytes, feature + 2) {
                    indices.push(u16_at(bytes, feature + 4 + 2 * j));
                }
            }
        }
        indices
    }

    /// Lookups flagged `IgnoreMarks` skip every mark, so they attach nothing even when anchors
    /// cover the glyph.
    #[test]
    fn thai_mark_lookups_flagged_ignore_marks_are_refused() {
        let Some(face) = crate::document_thai_font() else { return };
        let mut bytes = face.bytes.to_vec();
        let table = gpos(&bytes);
        let lookups = table + u16_at(&bytes, table + 8);
        for idx in thai_mark_lookup_indices(&bytes) {
            let lookup = lookups + u16_at(&bytes, lookups + 2 + 2 * idx);
            let flag = (u16_at(&bytes, lookup + 2) as u16) | 0x0008;
            bytes[lookup + 2..lookup + 4].copy_from_slice(&flag.to_be_bytes());
        }
        assert!(has_thai_layout(&bytes));
        for text in ["กิ่", "น้ำ", "ปู่", "นี่"] {
            let error = shape(&bytes, text).unwrap_err();
            assert!(error.contains("mark positioning"), "{text}: {error}");
        }
    }

    /// Lookups whose `MarkAttachmentType` or `UseMarkFilteringSet` skips the mark (or the earlier
    /// mark in mark-to-mark) do not position it.
    #[test]
    fn thai_mark_lookups_whose_mark_filtering_skips_the_mark_are_refused() {
        let Some(face) = crate::document_thai_font() else { return };
        let shaped = shape(face.bytes, "นี่").unwrap();
        let [_, vowel, tone] = [0, 1, 2].map(|i| u16::try_from(shaped.glyphs[i].id.to_u32()).unwrap());

        // 1. MarkAttachmentType = 1 (clearing UseMarkFilteringSet): GDEF has no MarkAttachClassDef,
        // so every mark has class 0 and is skipped.
        let mut wrong_class = face.bytes.to_vec();
        let gpos_off = gpos(&wrong_class);
        let lookups = gpos_off + u16_at(&wrong_class, gpos_off + 8);
        for idx in thai_mark_lookup_indices(&wrong_class) {
            let lookup = lookups + u16_at(&wrong_class, lookups + 2 + 2 * idx);
            let flag = ((u16_at(&wrong_class, lookup + 2) as u16) & !0x0010) | 0x0100;
            wrong_class[lookup + 2..lookup + 4].copy_from_slice(&flag.to_be_bytes());
        }
        for text in ["กิ่", "น้ำ", "ปู่", "นี่"] {
            let error = shape(&wrong_class, text).unwrap_err();
            assert!(error.contains("mark positioning"), "{text}: {error}");
        }

        // 2. Restrict the MarkToBase lookup so it covers only sara ii (`vowel`), leaving `tone`
        // (mai ek) to the UseMarkFilteringSet MarkToMark lookup alone.
        let mut base_vowel_only = face.bytes.to_vec();
        let gpos_off = gpos(&base_vowel_only);
        let lookups = gpos_off + u16_at(&base_vowel_only, gpos_off + 8);
        let mut mkmk_lookup = None;
        for idx in thai_mark_lookup_indices(&base_vowel_only) {
            let lookup = lookups + u16_at(&base_vowel_only, lookups + 2 + 2 * idx);
            let kind = u16_at(&base_vowel_only, lookup);
            let flag = u16_at(&base_vowel_only, lookup + 2);
            let mut sub = lookup + u16_at(&base_vowel_only, lookup + 6);
            let actual_kind = if kind == 9 {
                let k = u16_at(&base_vowel_only, sub + 2);
                sub += u32_at(&base_vowel_only, sub + 4);
                k
            } else {
                kind
            };
            if actual_kind == 4 {
                let cov = sub + u16_at(&base_vowel_only, sub + 2);
                if let Some(old_idx) = coverage_index(&base_vowel_only, cov, vowel) {
                    let mark_array = sub + u16_at(&base_vowel_only, sub + 8);
                    let rec = base_vowel_only[mark_array + 2 + 4 * old_idx..mark_array + 6 + 4 * old_idx].to_vec();
                    base_vowel_only[mark_array..mark_array + 2].copy_from_slice(&1u16.to_be_bytes());
                    base_vowel_only[mark_array + 2..mark_array + 6].copy_from_slice(&rec);
                    write_coverage_one(&mut base_vowel_only, cov, vowel);
                }
            } else if actual_kind == 6
                && (flag & 0x0010) != 0
                && coverage_index(&base_vowel_only, sub + u16_at(&base_vowel_only, sub + 2), tone).is_some()
            {
                mkmk_lookup = Some(lookup);
            }
        }
        let mkmk = mkmk_lookup.expect("Noto Sans Thai has a filtered mkmk lookup for above marks");
        assert!(shape(&base_vowel_only, "นี่").is_ok(), "filtered mkmk still attaches mai ek onto sara ii");

        // Pointing that mkmk lookup to set 4 (below marks) skips both marks.
        let mut skip_mark1 = base_vowel_only.clone();
        let count = u16_at(&skip_mark1, mkmk + 4);
        let old_set = u16_at(&skip_mark1, mkmk + 6 + 2 * count);
        let other_set = if old_set == 4 { 5u16 } else { 4u16 };
        skip_mark1[mkmk + 6 + 2 * count..mkmk + 8 + 2 * count].copy_from_slice(&other_set.to_be_bytes());
        let error = shape(&skip_mark1, "นี่").unwrap_err();
        assert!(error.contains("mark positioning"), "skip mark1: {error}");

        // Keeping `tone` in the filtering set while removing `vowel` skips the earlier mark.
        let mut skip_mark2 = base_vowel_only;
        let gdef_off = table(&skip_mark2, b"GDEF");
        let sets = gdef_off + u16_at(&skip_mark2, gdef_off + 12);
        let cov = sets + u32_at(&skip_mark2, sets + 4 + 4 * old_set);
        write_coverage_one(&mut skip_mark2, cov, tone);
        let error = shape(&skip_mark2, "นี่").unwrap_err();
        assert!(error.contains("mark positioning"), "skip mark2: {error}");
    }

    /// Checking a cluster's unmoved marks stays linear in the cluster size: 10,000 combining marks
    /// on one base must be refused promptly, not scanned quadratically.
    #[test]
    fn many_unplaced_marks_in_one_thai_cluster_are_refused_within_a_bound() {
        let Some(face) = crate::document_thai_font() else { return };
        let text = format!("กิ{}", "\u{20d0}".repeat(10_000));
        let started = std::time::Instant::now();
        let error = shape(face.bytes, &text).unwrap_err();
        assert!(error.contains("mark positioning"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    }

    /// A run starting with Latin before Thai marks is shaped under `thai` so Thai contextual rules
    /// and Thai mark lookups agree.
    #[test]
    fn mixed_latin_and_thai_run_shapes_thai_marks_under_thai_script() {
        let Some(face) = crate::document_thai_font() else { return };
        let thai_only: Vec<_> = shape(face.bytes, "กิ่").unwrap().glyphs.into_iter().map(|g| g.id).collect();
        let mixed: Vec<_> = shape(face.bytes, "ok กิ่").unwrap().glyphs.into_iter().skip(3).map(|g| g.id).collect();
        assert_eq!(mixed, thai_only);
    }
}
