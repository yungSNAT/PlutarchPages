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
    FontRef::new(bytes).is_ok_and(|f| mark_lookups(&f, b"thai").is_some())
}

/// The lookups of the `mark` and `mkmk` features the shaper uses for `script`, if it uses either.
/// The selection is HarfRust's, through the same read-fonts calls: the `script` script, else
/// `DFLT`, `dflt` or `latn` (`ScriptList::select`); then, given no language, a language system
/// tagged `dflt`, else the script's default one. Marks under a script or language it doesn't
/// select don't count, nor does a GPOS with only kerning.
fn visit_mark_features<'a>(
    gpos: &skrifa::raw::tables::gpos::Gpos<'a>,
    script: &[u8; 4],
    mut visit: impl FnMut(Result<skrifa::raw::tables::layout::Feature<'a>, skrifa::raw::ReadError>) -> bool,
) -> bool {
    use skrifa::Tag;
    let (Ok(scripts), Ok(features)) = (gpos.script_list(), gpos.feature_list()) else {
        return false;
    };
    let Some(selected) = scripts.select(&[Tag::new(script)]) else {
        return false;
    };
    let Ok(script) = scripts.get(selected.index) else {
        return false;
    };
    let language = match script.lang_sys_index_for_tag(Tag::new(b"dflt")) {
        Some(index) => script.lang_sys(index).ok().map(|l| l.element),
        None => script.default_lang_sys().transpose().ok().flatten(),
    };
    let Some(language) = language else {
        return false;
    };
    let records = features.feature_records();
    let required = Some(language.required_feature_index()).filter(|&i| i != 0xFFFF);
    for r in language
        .feature_indices()
        .iter()
        .map(|i| i.get())
        .chain(required)
        .filter_map(|i| records.get(usize::from(i)))
        .filter(|r| r.feature_tag() == *b"mark" || r.feature_tag() == *b"mkmk")
    {
        if !visit(r.feature(features.offset_data())) {
            return false;
        }
    }
    true
}

fn mark_lookups(face: &FontRef, script: &[u8; 4]) -> Option<Vec<u16>> {
    let gpos = face.gpos().ok()?;
    let (mut found, mut indices) = (false, Vec::new());
    visit_mark_features(&gpos, script, |feature| {
        found = true;
        if let Ok(f) = feature {
            indices.extend(f.lookup_list_indices().iter().map(|i| i.get()));
        }
        true
    });
    found.then(|| {
        indices.sort_unstable();
        indices.dedup();
        indices
    })
}

const MAX_THAI_GSUB_WORK: usize = 1 << 19;

enum LigRewrite<'a> {
    Single(skrifa::raw::tables::gsub::SingleSubst<'a>),
    Multiple(skrifa::raw::tables::gsub::MultipleSubstFormat1<'a>),
    Alternate(skrifa::raw::tables::gsub::AlternateSubstFormat1<'a>),
}

fn charge_gsub_work(work: &mut usize) -> Option<()> {
    *work = work.saturating_add(1);
    (*work <= MAX_THAI_GSUB_WORK).then_some(())
}

fn push_lig_target(target: u32, work: &mut usize, ligatures: &mut std::collections::HashSet<u32>, queue: &mut Vec<u32>) -> Option<()> {
    charge_gsub_work(work)?;
    if ligatures.insert(target) {
        queue.push(target);
    }
    Some(())
}

fn collect_rewrites<'a, T, E>(
    subs: impl Iterator<Item = Result<T, E>>,
    map: fn(T) -> LigRewrite<'a>,
    work: &mut usize,
    rewrites: &mut Vec<LigRewrite<'a>>,
) -> Option<()> {
    for sub in subs {
        charge_gsub_work(work)?;
        rewrites.push(map(sub.ok()?));
    }
    Some(())
}

fn step_lig_rewrite(
    rw: &LigRewrite<'_>,
    gid: GlyphId,
    work: &mut usize,
    ligatures: &mut std::collections::HashSet<u32>,
    queue: &mut Vec<u32>,
) -> Option<()> {
    use skrifa::raw::tables::gsub::SingleSubst;
    charge_gsub_work(work)?;
    match rw {
        LigRewrite::Single(SingleSubst::Format1(s)) => {
            if s.coverage().ok()?.get(gid).is_some() {
                let target = u32::from((gid.to_u32() as i32 + i32::from(s.delta_glyph_id())) as u16);
                push_lig_target(target, work, ligatures, queue)?;
            }
        }
        LigRewrite::Single(SingleSubst::Format2(s)) => {
            if let Some(idx) = s.coverage().ok()?.get(gid)
                && let Some(sub) = s.substitute_glyph_ids().get(usize::from(idx))
            {
                push_lig_target(sub.get().to_u32(), work, ligatures, queue)?;
            }
        }
        LigRewrite::Multiple(s) => {
            if let Some(idx) = s.coverage().ok()?.get(gid) {
                for sub in s.sequences().get(usize::from(idx)).ok()?.substitute_glyph_ids() {
                    push_lig_target(sub.get().to_u32(), work, ligatures, queue)?;
                }
            }
        }
        LigRewrite::Alternate(s) => {
            if let Some(idx) = s.coverage().ok()?.get(gid) {
                for alt in s.alternate_sets().get(usize::from(idx)).ok()?.alternate_glyph_ids() {
                    push_lig_target(alt.get().to_u32(), work, ligatures, queue)?;
                }
            }
        }
    }
    Some(())
}

/// Bounded GSUB walk collecting Type 4 (`Ligature`, including Type 7 wrappers) output glyphs and
/// their transitive `Single`/`Multiple`/`Alternate` rewrites, which preserve `lig_id`/`lig_comp`
/// inside HarfRust. Returns `None` if a present GSUB table fails to parse or hits
/// [`MAX_THAI_GSUB_WORK`].
fn ligature_glyphs(face: &FontRef) -> Option<std::collections::HashSet<u32>> {
    use skrifa::raw::tables::gsub::SubstitutionSubtables;
    if face.table_data(skrifa::Tag::new(b"GSUB")).is_none() {
        return Some(Default::default());
    }
    let gsub = face.gsub().ok()?;
    let list = gsub.lookup_list().ok()?;
    let mut work = 0usize;
    let mut ligatures = std::collections::HashSet::new();
    let mut rewrites = Vec::new();
    for lookup in list.lookups().iter() {
        charge_gsub_work(&mut work)?;
        match lookup.ok()?.subtables().ok()? {
            SubstitutionSubtables::Ligature(subs) => {
                for sub in subs.iter() {
                    charge_gsub_work(&mut work)?;
                    for set in sub.ok()?.ligature_sets().iter() {
                        charge_gsub_work(&mut work)?;
                        for lig in set.ok()?.ligatures().iter() {
                            charge_gsub_work(&mut work)?;
                            ligatures.insert(lig.ok()?.ligature_glyph().to_u32());
                        }
                    }
                }
            }
            SubstitutionSubtables::Single(subs) => collect_rewrites(subs.iter(), LigRewrite::Single, &mut work, &mut rewrites)?,
            SubstitutionSubtables::Multiple(subs) => collect_rewrites(subs.iter(), LigRewrite::Multiple, &mut work, &mut rewrites)?,
            SubstitutionSubtables::Alternate(subs) => collect_rewrites(subs.iter(), LigRewrite::Alternate, &mut work, &mut rewrites)?,
            _ => {}
        }
    }
    if !ligatures.is_empty() && !rewrites.is_empty() {
        let mut queue: Vec<u32> = ligatures.iter().copied().collect();
        while let Some(gid) = queue.pop() {
            let glyph = GlyphId::new(gid);
            for rw in &rewrites {
                step_lig_rewrite(rw, glyph, &mut work, &mut ligatures, &mut queue)?;
            }
        }
    }
    Some(ligatures)
}

struct MarkFilter<'a> {
    classes: Option<skrifa::raw::tables::gdef::ClassDef<'a>>,
    mark_classes: Option<skrifa::raw::tables::gdef::ClassDef<'a>>,
    mark_sets: Option<skrifa::raw::tables::gdef::MarkGlyphSets<'a>>,
    ligatures: std::collections::HashSet<u32>,
}

impl<'a> MarkFilter<'a> {
    fn new(face: &FontRef<'a>) -> Option<Self> {
        let gdef = face.gdef().ok();
        Some(Self {
            classes: gdef.as_ref().and_then(|g| g.glyph_class_def().transpose().ok().flatten()),
            mark_classes: gdef.as_ref().and_then(|g| g.mark_attach_class_def().transpose().ok().flatten()),
            mark_sets: gdef.as_ref().and_then(|g| g.mark_glyph_sets_def().transpose().ok().flatten()),
            ligatures: ligature_glyphs(face)?,
        })
    }

    fn is_gdef_mark(&self, gid: GlyphId, fallback_mark: bool) -> bool {
        self.classes.as_ref().map_or(fallback_mark, |c| c.get(gid) == 3)
    }

    fn is_ligature(&self, gid: GlyphId) -> bool {
        self.classes.as_ref().is_some_and(|c| c.get(gid) == 2) || self.ligatures.contains(&gid.to_u32())
    }

    fn allows_mark_properties(&self, flag: skrifa::raw::tables::layout::LookupFlag, filtering_set: Option<u16>, gid: GlyphId) -> bool {
        use skrifa::raw::tables::layout::LookupFlag;
        if flag.contains(LookupFlag::USE_MARK_FILTERING_SET) {
            return filtering_set
                .and_then(|set| self.mark_sets.as_ref()?.coverages().get(usize::from(set)).ok())
                .is_some_and(|cov| cov.get(gid).is_some());
        }
        if let Some(class) = flag.mark_attachment_class() {
            return self.classes.is_some() && self.mark_classes.as_ref().is_some_and(|c| c.get(gid) == class);
        }
        true
    }

    fn allows_current(&self, flag: skrifa::raw::tables::layout::LookupFlag, filtering_set: Option<u16>, gid: GlyphId) -> bool {
        use skrifa::raw::tables::layout::LookupFlag;
        if flag.contains(LookupFlag::IGNORE_MARKS) {
            return false;
        }
        let class = self.classes.as_ref().map(|c| c.get(gid));
        if matches!(class, Some(1)) && flag.contains(LookupFlag::IGNORE_BASE_GLYPHS)
            || matches!(class, Some(2)) && flag.contains(LookupFlag::IGNORE_LIGATURES)
        {
            return false;
        }
        if class.is_none_or(|c| c == 3) {
            return self.allows_mark_properties(flag, filtering_set, gid);
        }
        true
    }
}

fn charge_thai_work(work: &mut usize) -> Option<()> {
    *work = work.saturating_add(1);
    (*work <= MAX_THAI_LOOKUP_WORK).then_some(())
}

fn mark_class_with_valid_anchor(
    marks: &skrifa::raw::tables::layout::CoverageTable<'_>,
    classes: &skrifa::raw::tables::gpos::MarkArray<'_>,
    mark: GlyphId,
) -> Option<usize> {
    let m = marks.get(mark)?;
    let record = classes.mark_records().get(usize::from(m))?;
    record.mark_anchor(classes.offset_data()).ok()?;
    Some(usize::from(record.mark_class()))
}

fn lig_attach_ok(
    att: &skrifa::raw::tables::gpos::LigatureAttach<'_>,
    class: usize,
    comp_spec: (usize, Option<usize>),
    work: &mut usize,
) -> Option<bool> {
    let comps = att.component_records();
    if comps.is_empty() {
        return Some(false);
    }
    let anchor_ok = |idx: usize| comps.get(idx).is_ok_and(|r| r.ligature_anchors(att.offset_data()).get(class).is_some_and(|a| a.is_ok()));
    match comp_spec {
        (0, Some(usize::MAX)) => {
            charge_thai_work(work)?;
            Some(anchor_ok(comps.len() - 1))
        }
        (bases_in_text, Some(idx)) if bases_in_text == comps.len() => {
            charge_thai_work(work)?;
            Some(anchor_ok(idx))
        }
        _ => {
            for c in 0..comps.len() {
                charge_thai_work(work)?;
                if !anchor_ok(c) {
                    return Some(false);
                }
            }
            Some(true)
        }
    }
}

/// Whether `lookup` (mark-to-base, mark-to-ligature, or mark-to-mark) covers `mark` and `onto`
/// with valid parsed anchors for `mark` and its target at `mark`'s class, charging `work` on
/// each visited subtable and component record.
fn lookup_attaches(
    lookup: &skrifa::raw::tables::gpos::PositionLookup<'_>,
    (mark, onto): (GlyphId, GlyphId),
    comp_spec: (usize, Option<usize>),
    work: &mut usize,
) -> Option<bool> {
    use skrifa::raw::tables::gpos::PositionSubtables;
    match lookup.subtables() {
        Ok(PositionSubtables::MarkToBase(subtables)) => {
            for t in subtables.iter() {
                charge_thai_work(work)?;
                let Ok(t) = t else { continue };
                let (Ok(marks), Ok(bases), Ok(classes), Ok(anchors)) = (t.mark_coverage(), t.base_coverage(), t.mark_array(), t.base_array()) else {
                    continue;
                };
                let (Some(class), Some(b)) = (mark_class_with_valid_anchor(&marks, &classes, mark), bases.get(onto)) else {
                    continue;
                };
                if anchors
                    .base_records()
                    .get(usize::from(b))
                    .is_ok_and(|r| r.base_anchors(anchors.offset_data()).get(class).is_some_and(|a| a.is_ok()))
                {
                    return Some(true);
                }
            }
            Some(false)
        }
        Ok(PositionSubtables::MarkToLig(subtables)) => {
            for t in subtables.iter() {
                charge_thai_work(work)?;
                let Ok(t) = t else { continue };
                let (Ok(marks), Ok(ligs), Ok(classes), Ok(ligs_arr)) = (t.mark_coverage(), t.ligature_coverage(), t.mark_array(), t.ligature_array())
                else {
                    continue;
                };
                let (Some(class), Some(b)) = (mark_class_with_valid_anchor(&marks, &classes, mark), ligs.get(onto)) else {
                    continue;
                };
                if let Ok(att) = ligs_arr.ligature_attaches().get(usize::from(b))
                    && lig_attach_ok(&att, class, comp_spec, work)?
                {
                    return Some(true);
                }
            }
            Some(false)
        }
        Ok(PositionSubtables::MarkToMark(subtables)) => {
            for t in subtables.iter() {
                charge_thai_work(work)?;
                let Ok(t) = t else { continue };
                let (Ok(marks), Ok(bases), Ok(classes), Ok(anchors)) = (t.mark1_coverage(), t.mark2_coverage(), t.mark1_array(), t.mark2_array())
                else {
                    continue;
                };
                let (Some(class), Some(b)) = (mark_class_with_valid_anchor(&marks, &classes, mark), bases.get(onto)) else {
                    continue;
                };
                if anchors
                    .mark2_records()
                    .get(usize::from(b))
                    .is_ok_and(|r| r.mark2_anchors(anchors.offset_data()).get(class).is_some_and(|a| a.is_ok()))
                {
                    return Some(true);
                }
            }
            Some(false)
        }
        _ => Some(false),
    }
}

/// Whether `program`, as a page embeds it (not with a cmap the editor rebuilt from ToUnicode),
/// gives the Arabic letters of `text` their joining forms, judged against `reference`, a complete
/// Arabic face, which must draw every letter itself. Its own cmap must map every letter; each
/// letter's isolated, initial, medial and final forms must differ wherever the reference's
/// differ ([`tells_forms_apart`]); and wherever the reference's shaping of `text` draws a letter
/// in glyphs other than those it draws the letter and its marks with alone (a joining form or
/// part of a ligature), the program's must too, each face judged against its own isolated form;
/// and its GPOS must place every glyph that sits on another ([`places_marks`]) and stack marks
/// wherever the reference does ([`stacks_marks`]), on the marks the reference puts them on
/// ([`follows_like`]); every combining mark of `text` must follow an
/// Arabic letter in its joining run, where those checks reach it. A cmap that maps
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
    let joins = |c: char| matches!(u32::from(c), 0x0600..=0x06ff | 0x0750..=0x077f | 0x0870..=0x08ff | 0x200c | 0x200d);
    let mark = |c: char| matches!(u32::from(c), 0x0610..=0x061a | 0x064b..=0x065f | 0x0670 | 0x06d6..=0x06dc | 0x06df..=0x06e4 | 0x06e7..=0x06e8 | 0x06ea..=0x06ed | 0x08ca..=0x08ff);
    let letter = |c: char| {
        !mark(c) && matches!(u32::from(c), 0x0620..=0x064a | 0x066e..=0x06d5 | 0x06ee..=0x06ef | 0x06fa..=0x06ff | 0x0750..=0x077f | 0x0870..=0x08ff)
    };
    // Every combining mark must follow an Arabic letter (or marks on one) in its joining run: the
    // runs below are all that is shaped, so a mark outside them (U+0301 after a digit or a Latin
    // letter, or after an Arabic letter) or opening one would be placed unjudged, at the glyph
    // origin if the GPOS doesn't place it. Refusing such text costs only the page's font.
    let mut on_letter = false;
    for c in text.chars() {
        if unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::NSM {
            if !(joins(c) && on_letter) {
                return false;
            }
        } else {
            on_letter = letter(c);
        }
    }
    let gpos_lookups = own.gpos().ok().and_then(|t| t.lookup_list().ok()).map_or(0, |l| l.lookups().len());
    let gsub_lookups = own.gsub().ok().and_then(|t| t.lookup_list().ok()).map_or(0, |l| l.lookups().len());
    let total_lookups = gpos_lookups.saturating_add(gsub_lookups);
    if total_lookups > 1 << 12 || total_lookups.max(1).saturating_mul(text.len()) > 1 << 27 {
        return false;
    }
    // Any other character ends a run of joining, so each run of Arabic shapes alone as in place.
    let run_marks: Vec<(&str, Vec<(usize, char)>)> =
        text.split(|c: char| !joins(c)).filter(|run| !run.is_empty()).map(|run| (run, stacked_marks(run))).collect();
    let mark_gpos_lookups = if run_marks.iter().any(|(_, marks)| !marks.is_empty()) {
        let Some(n) = mark_lookups_attach_only(&own) else { return false };
        n
    } else {
        0
    };
    let scale = mark_gpos_lookups.max(1);
    // [`follows_like`] shapes a run again for each of its stacked marks, so their work, summed over
    // the text and scaled by reachable mark/mkmk lookups, is bounded before any shaping begins.
    if run_marks
        .iter()
        .map(|(run, marks)| {
            let base = marks.len().saturating_mul(run.len());
            (base, scale.saturating_mul(base))
        })
        .try_fold((0usize, 0usize), |(a, b), (x, y)| Some((a.checked_add(x).filter(|&n| n <= 1 << 20)?, b.checked_add(y).filter(|&n| n <= 1 << 24)?)))
        .is_none()
    {
        return false;
    }
    let (own_data, reference_data) = (ShaperData::new(&own), ShaperData::new(&reference));
    let (own_shaper, reference_shaper) = (own_data.shaper(&own).build(), reference_data.shaper(&reference).build());
    let own_map = own.charmap();
    let letters: std::collections::BTreeSet<char> = text.chars().filter(|c| letter(*c)).collect();
    if !letters.iter().all(|&c| own_map.map(c).is_some() && tells_forms_apart(&own_shaper, &reference_shaper, c)) {
        return false;
    }
    let space = |face: &FontRef| face.charmap().map(' ').map(|g| g.to_u32());
    let (own_space, reference_space) = (space(&own), space(&reference));
    run_marks.into_iter().all(|(run, marks)| {
        if !places_marks(&own_shaper, own_space, run)
            || !stacks_marks((&own_shaper, own_space), (&reference_shaper, reference_space), run)
            || !follows_like(&own_shaper, &reference_shaper, run, &marks)
        {
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

#[derive(Default)]
struct ThaiCluster {
    len: usize,
    placed: usize,
    in_ligature: bool,
    last_base: Option<u32>,
    last_mark: Vec<Option<u32>>,
    bases_in_text: usize,
    mark_comps: Vec<usize>,
    mark_ord: usize,
    hidden_ignorables: usize,
    has_space: bool,
}

impl ThaiCluster {
    fn reset(&mut self) {
        self.len = 0;
        self.placed = 0;
        self.in_ligature = false;
        self.last_base = None;
        self.last_mark.fill(None);
        self.bases_in_text = 0;
        self.mark_comps.clear();
        self.mark_ord = 0;
        self.hidden_ignorables = 0;
        self.has_space = false;
    }

    fn set_text(&mut self, text: &str) {
        self.bases_in_text = 0;
        self.mark_comps.clear();
        self.mark_ord = 0;
        self.hidden_ignorables = 0;
        self.has_space = text.contains(' ');
        for c in text.chars() {
            if default_ignorable(c) {
                self.hidden_ignorables += 1;
                continue;
            }
            if c == '\u{0e33}' || unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::NSM {
                self.mark_comps.push(self.bases_in_text.saturating_sub(1));
            } else {
                self.bases_in_text += 1;
            }
        }
    }
}

const MAX_THAI_CLUSTER_MARKS: usize = 64;
const MAX_THAI_LOOKUP_WORK: usize = 1 << 22;

struct ThaiValidator<'a> {
    filter: MarkFilter<'a>,
    space: Option<u32>,
    selected: Vec<(u16, Option<usize>, skrifa::raw::tables::gpos::PositionLookup<'a>)>,
    mkmk_slots: usize,
    cache: std::collections::HashMap<(u16, u32, u32, usize, Option<usize>), bool>,
    work: usize,
}

impl<'a> ThaiValidator<'a> {
    fn new(face: &FontRef<'a>, lookups: &[u16]) -> Option<Self> {
        use skrifa::MetadataProvider as _;
        use skrifa::raw::tables::gpos::PositionSubtables;
        use skrifa::raw::tables::layout::LookupFlag;
        let gpos = face.gpos().ok()?;
        let list = gpos.lookup_list().ok()?;
        let mut mkmk_slots = 0usize;
        let mut selected = Vec::new();
        let mut work = 0usize;
        for &i in lookups {
            charge_thai_work(&mut work)?;
            let Ok(lookup) = list.lookups().get(usize::from(i)) else { continue };
            if lookup.lookup_flag().contains(LookupFlag::IGNORE_MARKS) {
                continue;
            }
            match lookup.subtables() {
                Ok(PositionSubtables::MarkToBase(_) | PositionSubtables::MarkToLig(_)) => selected.push((i, None, lookup)),
                Ok(PositionSubtables::MarkToMark(_)) => {
                    selected.push((i, Some(mkmk_slots), lookup));
                    mkmk_slots += 1;
                }
                _ => {}
            }
        }
        let space = face.charmap().map(' ').map(GlyphId::to_u32);
        Some(Self { filter: MarkFilter::new(face)?, space, selected, mkmk_slots, cache: std::collections::HashMap::new(), work })
    }

    fn skip_hidden_ignorable(&self, cluster: &mut ThaiCluster, gid: u32, pos: (i32, i32, i32, i32), not_leading_space: bool) -> bool {
        if cluster.hidden_ignorables > 0 && self.space == Some(gid) && pos == (0, 0, 0, 0) && (!cluster.has_space || not_leading_space) {
            cluster.hidden_ignorables -= 1;
            return true;
        }
        false
    }

    fn charge(&mut self, units: usize) -> bool {
        self.work = self.work.saturating_add(units);
        self.work <= MAX_THAI_LOOKUP_WORK
    }

    fn any_selected_attaches(&mut self, entry: &ThaiCluster, mark: GlyphId) -> Option<bool> {
        if !self.charge(self.selected.len()) {
            return None;
        }
        let mark_comp = entry.mark_comps.get(entry.mark_ord).copied();
        for &(idx, mkmk_slot, ref lookup) in &self.selected {
            let flag = lookup.lookup_flag();
            if !self.filter.allows_current(flag, lookup.mark_filtering_set(), mark) {
                continue;
            }
            let onto = match mkmk_slot {
                Some(slot) => entry.last_mark.get(slot).copied().flatten(),
                None => entry.last_base,
            };
            let Some(onto) = onto else { continue };
            let comp_spec =
                if mkmk_slot.is_none() && self.filter.ligatures.contains(&onto) { (entry.bases_in_text, mark_comp) } else { (0, Some(usize::MAX)) };
            let key = (idx, mark.to_u32(), onto, comp_spec.0, comp_spec.1);
            let hit = match self.cache.get(&key) {
                Some(&cached) => cached,
                None => {
                    let ok = lookup_attaches(lookup, (mark, GlyphId::new(onto)), comp_spec, &mut self.work)?;
                    self.cache.insert(key, ok);
                    ok
                }
            };
            if hit {
                return Some(true);
            }
        }
        Some(false)
    }

    fn update_last_marks(&mut self, entry: &mut ThaiCluster, mark: GlyphId) -> bool {
        if entry.in_ligature || self.mkmk_slots == 0 {
            return true;
        }
        if !self.charge(self.mkmk_slots) {
            return false;
        }
        for &(_, mkmk_slot, ref lookup) in &self.selected {
            if let Some(slot) = mkmk_slot
                && self.filter.allows_mark_properties(lookup.lookup_flag(), lookup.mark_filtering_set(), mark)
                && let Some(cell) = entry.last_mark.get_mut(slot)
            {
                *cell = Some(mark.to_u32());
            }
        }
        true
    }

    fn step_glyph(&mut self, cluster: &mut ThaiCluster, gid: GlyphId, moved: bool, zero_advance: bool) -> bool {
        if cluster.len > 0 && !moved {
            match self.any_selected_attaches(cluster, gid) {
                Some(true) => cluster.placed += 1,
                Some(false) => {}
                None => return false,
            }
        } else if cluster.len > 0 && moved {
            cluster.placed += 1;
        }
        let is_mark = self.filter.is_gdef_mark(gid, cluster.len > 0 && zero_advance);
        cluster.len += 1;
        if !is_mark {
            cluster.last_base = Some(gid.to_u32());
            cluster.in_ligature = self.filter.is_ligature(gid);
            cluster.last_mark.fill(None);
            true
        } else {
            cluster.mark_ord += 1;
            self.update_last_marks(cluster, gid)
        }
    }
}

fn has_unbounded_mark_stretch(run: &str) -> bool {
    let mut run_marks = 0usize;
    for c in run.chars() {
        if c == '\u{0e33}' || default_ignorable(c) || unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::NSM {
            run_marks += 1;
            if run_marks > MAX_THAI_CLUSTER_MARKS {
                return true;
            }
        } else {
            run_marks = 0;
        }
    }
    false
}

fn thai_cluster_marks(text: &str) -> usize {
    if !text.chars().any(|c| ('\u{0e00}'..='\u{0e7f}').contains(&c)) {
        return 0;
    }
    text.chars().filter(|&c| !default_ignorable(c) && (c == '\u{0e33}' || unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::NSM)).count()
}

fn next_cluster(infos: &[harfrust::GlyphInfo], pos: &mut usize, rtl: bool) -> Option<(usize, usize)> {
    if rtl {
        let end = *pos;
        if end == 0 {
            return None;
        }
        let cid = infos[end - 1].cluster;
        let mut start = end - 1;
        while start > 0 && infos[start - 1].cluster == cid {
            start -= 1;
        }
        *pos = start;
        Some((start, end))
    } else {
        let start = *pos;
        if start >= infos.len() {
            return None;
        }
        let cid = infos[start].cluster;
        let mut end = start + 1;
        while end < infos.len() && infos[end].cluster == cid {
            end += 1;
        }
        *pos = end;
        Some((start, end))
    }
}

/// Whether the GPOS places the marks of `run`, one bidi run shaped in its direction and script with
/// `validator` (its `mark` and `mkmk` lookups for Thai): each Thai cluster must have as many
/// placed glyphs as non-ignorable combining marks, Sara Am counting for its Nikhahit. Returns the
/// shaped `GlyphBuffer` when valid so callers need not shape again. A glyph is placed if it moves
/// when `mark` and `mkmk` are turned off, or if one of `lookups` attaches it to the base
/// (mark-to-base) or the last unskipped unligated mark before it in the cluster (mark-to-mark),
/// honouring lookup flags (`IgnoreMarks`, `MarkAttachmentType`, `UseMarkFilteringSet`). Feature
/// names alone don't count: a GPOS whose lookups are empty, skipped or miss these glyphs would
/// leave each mark at its glyph origin. A cluster whose GSUB merges 2 marks into one glyph is
/// refused too, which only costs the page's font.
fn places_thai_marks(
    validator: &mut ThaiValidator<'_>,
    shaper: &harfrust::Shaper,
    (run, direction, script): (&str, Direction, Option<harfrust::Script>),
) -> Option<harfrust::GlyphBuffer> {
    if has_unbounded_mark_stretch(run) {
        return None;
    }
    let off = [b"mark", b"mkmk"].map(|tag| harfrust::Feature::new(harfrust::Tag::new(tag), 0, ..));
    let (with, without) = (shape_in(shaper, run, direction, script, &[]), shape_in(shaper, run, direction, script, &off));
    if with.glyph_infos().iter().map(|g| g.glyph_id).ne(without.glyph_infos().iter().map(|g| g.glyph_id)) {
        return None;
    }
    let at = |p: &harfrust::GlyphPosition| (p.x_advance, p.y_advance, p.x_offset, p.y_offset);
    let (infos, on_pos, off_pos) = (with.glyph_infos(), with.glyph_positions(), without.glyph_positions());
    let rtl = direction == Direction::RightToLeft;
    let mut cluster = ThaiCluster { last_mark: vec![None; validator.mkmk_slots], ..Default::default() };
    let mut cur_start: Option<usize> = None;
    let mut validated_end = 0usize;
    let valid_slice = |start: usize, end: usize, placed: usize| run.get(start..end).is_some_and(|text| placed >= thai_cluster_marks(text));
    let mut pos = if rtl { infos.len() } else { 0 };
    while let Some((c_start, c_end)) = next_cluster(infos, &mut pos, rtl) {
        let start = infos[c_start].cluster as usize;
        if let Some(prev) = cur_start {
            if start <= prev || !valid_slice(validated_end, start, cluster.placed) {
                return None;
            }
            validated_end = start;
            cluster.reset();
        }
        cur_start = Some(start);
        let cluster_end = if rtl {
            if c_start > 0 { infos[c_start - 1].cluster as usize } else { run.len() }
        } else {
            infos.get(c_end).map_or(run.len(), |g| g.cluster as usize)
        };
        cluster.set_text(run.get(start..cluster_end).unwrap_or_default());
        for idx in c_start..c_end {
            let (info, on, off) = (&infos[idx], &on_pos[idx], &off_pos[idx]);
            let not_leading_space = if rtl { idx + 1 < c_end } else { idx > c_start };
            if validator.skip_hidden_ignorable(&mut cluster, info.glyph_id, at(on), not_leading_space) {
                continue;
            }
            if !validator.step_glyph(&mut cluster, GlyphId::new(info.glyph_id), at(on) != at(off), on.x_advance == 0) {
                return None;
            }
        }
    }
    let final_ok = match cur_start {
        Some(_) => valid_slice(validated_end, run.len(), cluster.placed),
        None => valid_slice(0, run.len(), 0),
    };
    final_ok.then_some(with)
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

fn charge_arabic_gpos(work: &mut usize) -> bool {
    *work = work.saturating_add(1);
    *work <= 1 << 16
}

fn push_mark_records(
    recs: &[skrifa::raw::tables::layout::SequenceLookupRecord],
    todo: &mut Vec<u16>,
    seen: &mut std::collections::BTreeSet<u16>,
    work: &mut usize,
) -> bool {
    for r in recs {
        if !charge_arabic_gpos(work) || r.sequence_index() != 0 {
            return false;
        }
        let idx = r.lookup_list_index();
        if seen.insert(idx) {
            todo.push(idx);
        }
    }
    true
}

fn push_feature_lookups(
    indices: &[skrifa::raw::types::BigEndian<u16>],
    todo: &mut Vec<u16>,
    seen: &mut std::collections::BTreeSet<u16>,
    work: &mut usize,
) -> bool {
    if !charge_arabic_gpos(work) {
        return false;
    }
    for idx in indices {
        if !charge_arabic_gpos(work) {
            return false;
        }
        let idx = idx.get();
        if seen.insert(idx) {
            todo.push(idx);
        }
    }
    true
}

fn check_rule_sets<T, F>(sets: impl Iterator<Item = Option<Result<T, skrifa::raw::ReadError>>>, work: &mut usize, mut f: F) -> bool
where
    F: FnMut(T, &mut usize) -> bool,
{
    for set in sets {
        if !charge_arabic_gpos(work) {
            return false;
        }
        match set {
            None => {}
            Some(Ok(s)) => {
                if !f(s, work) {
                    return false;
                }
            }
            Some(Err(_)) => return false,
        }
    }
    true
}

fn check_seq_context(
    sub: Result<skrifa::raw::tables::layout::SequenceContext<'_>, skrifa::raw::ReadError>,
    todo: &mut Vec<u16>,
    seen: &mut std::collections::BTreeSet<u16>,
    work: &mut usize,
) -> bool {
    use skrifa::raw::tables::layout::SequenceContext;
    let Ok(sub) = sub else { return false };
    match sub {
        SequenceContext::Format1(s) => check_rule_sets(s.seq_rule_sets().iter(), work, |set, work| {
            set.seq_rules().iter().all(|r| charge_arabic_gpos(work) && r.is_ok_and(|r| push_mark_records(r.seq_lookup_records(), todo, seen, work)))
        }),
        SequenceContext::Format2(s) => check_rule_sets(s.class_seq_rule_sets().iter(), work, |set, work| {
            set.class_seq_rules()
                .iter()
                .all(|r| charge_arabic_gpos(work) && r.is_ok_and(|r| push_mark_records(r.seq_lookup_records(), todo, seen, work)))
        }),
        SequenceContext::Format3(s) => push_mark_records(s.seq_lookup_records(), todo, seen, work),
    }
}

fn check_chain_context(
    sub: Result<skrifa::raw::tables::layout::ChainedSequenceContext<'_>, skrifa::raw::ReadError>,
    todo: &mut Vec<u16>,
    seen: &mut std::collections::BTreeSet<u16>,
    work: &mut usize,
) -> bool {
    use skrifa::raw::tables::layout::ChainedSequenceContext;
    let Ok(sub) = sub else { return false };
    match sub {
        ChainedSequenceContext::Format1(s) => check_rule_sets(s.chained_seq_rule_sets().iter(), work, |set, work| {
            set.chained_seq_rules()
                .iter()
                .all(|r| charge_arabic_gpos(work) && r.is_ok_and(|r| push_mark_records(r.seq_lookup_records(), todo, seen, work)))
        }),
        ChainedSequenceContext::Format2(s) => check_rule_sets(s.chained_class_seq_rule_sets().iter(), work, |set, work| {
            set.chained_class_seq_rules()
                .iter()
                .all(|r| charge_arabic_gpos(work) && r.is_ok_and(|r| push_mark_records(r.seq_lookup_records(), todo, seen, work)))
        }),
        ChainedSequenceContext::Format3(s) => push_mark_records(s.seq_lookup_records(), todo, seen, work),
    }
}

/// Unique reachable `arab` (or fallback) GPOS `mark` and `mkmk` lookup count if they contain
/// only mark attachment lookups (Types 4, 5, 6) and zero-index contextual wrappers over them,
/// so clearing `mark`/`mkmk` on an earlier mark can move a later glyph only through its attachment
/// chain; `None` on non-attachment lookups, non-zero contextual targets or budget exhaustion.
fn mark_lookups_attach_only(face: &FontRef) -> Option<usize> {
    use skrifa::raw::tables::gpos::PositionSubtables;
    let gpos = face.gpos().ok()?;
    let list = gpos.lookup_list().ok()?;
    let (mut seen, mut todo, mut work) = (std::collections::BTreeSet::new(), Vec::new(), 0usize);
    let collected = visit_mark_features(&gpos, b"arab", |feature| {
        let Ok(f) = feature else { return false };
        push_feature_lookups(f.lookup_list_indices(), &mut todo, &mut seen, &mut work)
    });
    if !collected || todo.is_empty() {
        return None;
    }
    while let Some(idx) = todo.pop() {
        if !charge_arabic_gpos(&mut work) {
            return None;
        }
        let lookup = list.lookups().get(usize::from(idx)).ok()?;
        let ok = match lookup.subtables() {
            Ok(PositionSubtables::MarkToBase(_) | PositionSubtables::MarkToLig(_) | PositionSubtables::MarkToMark(_)) => true,
            Ok(PositionSubtables::Contextual(subs)) => {
                subs.iter().all(|s| charge_arabic_gpos(&mut work) && check_seq_context(s, &mut todo, &mut seen, &mut work))
            }
            Ok(PositionSubtables::ChainContextual(subs)) => {
                subs.iter().all(|s| charge_arabic_gpos(&mut work) && check_chain_context(s, &mut todo, &mut seen, &mut work))
            }
            _ => false,
        };
        if !ok {
            return None;
        }
    }
    Some(seen.len())
}

/// Whether each mark of `run` that `reference` attaches to an earlier mark, `own` attaches to that
/// mark too. [`stacks_marks`] shows that `mkmk` moves each stacked mark, not what it puts it on:
/// a lookup whose flags skip the middle of 3 marks puts the third on the first, through the
/// second. A mark follows another if its glyphs move when the other alone loses `mark` and
/// `mkmk`, which moves it; every pair that follows in the reference must follow in `own`. A pair
/// only `own` has (a mark put on one the reference doesn't stack it on) is accepted, as
/// [`stacks_marks`] accepts more stacked marks: a subset keeps its face's attachments. Only a
/// mark beside another can have one follow it (`marks` precomputed once per run); a run with none
/// is not shaped again.
fn follows_like(own: &harfrust::Shaper, reference: &harfrust::Shaper, run: &str, marks: &[(usize, char)]) -> bool {
    if marks.is_empty() {
        return true;
    }
    match (followers(own, run, marks), followers(reference, run, marks)) {
        (Some(ours), Some(theirs)) => theirs.is_subset(&ours),
        _ => false,
    }
}

/// The combining marks of `run` (byte offset and character) beside another one. The shaper
/// reorders a letter's marks, so each of them may carry another.
fn stacked_marks(run: &str) -> Vec<(usize, char)> {
    let is_mark = |c: char| unicode_bidi::bidi_class(c) == unicode_bidi::BidiClass::NSM;
    let chars: Vec<(usize, char)> = run.char_indices().collect();
    let beside = |i: usize| [i.wrapping_sub(1), i + 1].iter().any(|&j| chars.get(j).is_some_and(|&(_, c)| is_mark(c)));
    chars.iter().enumerate().filter(|&(i, &(_, c))| is_mark(c) && beside(i)).map(|(_, &p)| p).collect()
}

/// The pairs (a mark's byte offset in `run`, that of a character whose glyphs move when the mark
/// alone loses `mark` and `mkmk`), shaped right to left a character to a cluster. `None` if that
/// changes the glyphs, changes any `x_advance` or overflows glyph positions.
fn followers(shaper: &harfrust::Shaper, run: &str, marks: &[(usize, char)]) -> Option<std::collections::BTreeSet<(usize, usize)>> {
    // Each glyph's cluster and where it is drawn.
    let placed = |features: &[harfrust::Feature]| {
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(run);
        buffer.set_cluster_level(BufferClusterLevel::Characters);
        buffer.set_direction(Direction::RightToLeft);
        buffer.guess_segment_properties();
        let output = shaper.shape(buffer, ShapeOptions::new().features(features));
        let mut x = 0i64;
        output
            .glyph_infos()
            .iter()
            .zip(output.glyph_positions())
            .map(|(info, p)| {
                let at = [x.checked_add(i64::from(p.x_offset))?, i64::from(p.y_offset)];
                x = x.checked_add(i64::from(p.x_advance))?;
                Some((info.cluster, info.glyph_id, at, p.x_advance))
            })
            .collect::<Option<Vec<(u32, u32, [i64; 2], i32)>>>()
    };
    let unchanged = placed(&[])?;
    let mut pairs = std::collections::BTreeSet::new();
    for &(at, mark) in marks {
        // HarfRust stores the range's end less one and tests `cluster < end`, so this covers the
        // mark's first byte alone; that is its cluster, as every combining mark takes 2 bytes or
        // more. An inclusive range would cover nothing.
        let alone = at..at + mark.len_utf8();
        let off = [b"mark", b"mkmk"].map(|tag| harfrust::Feature::new(harfrust::Tag::new(tag), 0, alone.clone()));
        let moved = placed(&off)?;
        if moved.iter().map(|g| (g.1, g.3)).ne(unchanged.iter().map(|g| (g.1, g.3))) {
            return None;
        }
        // Fail closed if turning off mark/mkmk for the perturbed mark did not displace its own
        // glyph(s); marks merged into an earlier GSUB ligature keep no cluster here and are checked
        // under that ligature's cluster instead.
        let (has_own, self_moved) =
            unchanged.iter().zip(&moved).filter(|(a, _)| a.0 as usize == at).fold((false, false), |(_, m), (a, b)| (true, m || a.2 != b.2));
        if has_own && !self_moved {
            return None;
        }
        pairs.extend(unchanged.iter().zip(&moved).filter(|(a, b)| a.0 as usize != at && a.2 != b.2).map(|(a, _)| (at, a.0 as usize)));
    }
    Some(pairs)
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
    shape_in(shaper, text, Direction::RightToLeft, None, features)
}

/// `text` shaped in `direction` (and optional OpenType `script`) with `features`.
fn shape_in(
    shaper: &harfrust::Shaper,
    text: &str,
    direction: Direction,
    script: Option<harfrust::Script>,
    features: &[harfrust::Feature],
) -> harfrust::GlyphBuffer {
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.set_cluster_level(BufferClusterLevel::MonotoneGraphemes);
    buffer.set_direction(direction);
    buffer.guess_segment_properties();
    if let Some(script) = script {
        buffer.set_script(script);
    }
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

fn split_thai_scripts(input: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut cur_thai) = (0usize, None);
    for (idx, c) in input.char_indices() {
        let kind = if ('\u{0e00}'..='\u{0e7f}').contains(&c) {
            Some(true)
        } else if c.is_alphabetic() {
            Some(false)
        } else {
            None
        };
        if let Some(k) = kind {
            if let Some(prev) = cur_thai
                && prev != k
            {
                out.push(&input[start..idx]);
                start = idx;
            }
            cur_thai = Some(k);
        }
    }
    if start < input.len() {
        out.push(&input[start..]);
    }
    out
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
    let mut thai_validator = if !legacy_thai && thai_marks(text) {
        let lookups = mark_lookups(&face, b"thai").ok_or_else(no_thai_positioning)?;
        Some(ThaiValidator::new(&face, &lookups).ok_or_else(no_thai_positioning)?)
    } else {
        None
    };
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
        let Some(run_text) = text.get(run.clone()) else { continue };
        let direction = if levels.get(run.start).is_some_and(Level::is_rtl) { Direction::RightToLeft } else { Direction::LeftToRight };
        let mut subruns = if !legacy_thai && thai_marks(run_text) { split_thai_scripts(run_text) } else { vec![run_text] };
        if direction == Direction::RightToLeft {
            subruns.reverse();
        }
        for input in subruns {
            let script = (!legacy_thai && thai_marks(input)).then_some(harfrust::script::THAI);
            let output = match (&mut thai_validator, script) {
                (Some(validator), Some(_)) => places_thai_marks(validator, &shaper, (input, direction, script)).ok_or_else(no_thai_positioning)?,
                _ => shape_in(&shaper, input, direction, script, &[]),
            };
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
            | 0x180b..=0x180e
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

    fn with_table(program: &[u8], tag: &[u8; 4], payload: Vec<u8>) -> Vec<u8> {
        let count = u16::from_be_bytes([program[4], program[5]]) as usize;
        let mut tables: Vec<([u8; 4], Vec<u8>)> = program[12..12 + count * 16]
            .as_chunks::<16>()
            .0
            .iter()
            .filter(|r| &r[..4] != tag)
            .map(|r| {
                let off = u32::from_be_bytes(r[8..12].try_into().unwrap()) as usize;
                let len = u32::from_be_bytes(r[12..16].try_into().unwrap()) as usize;
                (r[..4].try_into().unwrap(), program[off..off + len].to_vec())
            })
            .collect();
        tables.push((*tag, payload));
        tables.sort_by_key(|(t, _)| *t);
        let n = tables.len() as u16;
        let entry_selector = (u16::BITS - 1 - n.leading_zeros()) as u16;
        let search_range = (1u16 << entry_selector) * 16;
        let range_shift = n * 16 - search_range;
        let mut out = Vec::new();
        out.extend_from_slice(&program[..4]);
        for w in [n, search_range, entry_selector, range_shift] {
            out.extend_from_slice(&w.to_be_bytes());
        }
        let dir_start = out.len();
        out.resize(dir_start + tables.len() * 16, 0);
        for (i, (t, body)) in tables.into_iter().enumerate() {
            let off = out.len() as u32;
            let len = body.len() as u32;
            out.extend_from_slice(&body);
            while !out.len().is_multiple_of(4) {
                out.push(0);
            }
            let rec = dir_start + i * 16;
            out[rec..rec + 4].copy_from_slice(&t);
            out[rec + 8..rec + 12].copy_from_slice(&off.to_be_bytes());
            out[rec + 12..rec + 16].copy_from_slice(&len.to_be_bytes());
        }
        out
    }

    fn minimal_liga_gsub(first: u16, second: u16, lig: u16) -> Vec<u8> {
        let mut gsub = Vec::new();
        let push = |v: &mut Vec<u8>, w: u16| v.extend_from_slice(&w.to_be_bytes());
        for w in [1u16, 0, 10, 30, 44, 1] {
            push(&mut gsub, w);
        }
        gsub.extend_from_slice(b"thai");
        for w in [8u16, 4, 0, 0, 0xFFFF, 1, 0, 1] {
            push(&mut gsub, w);
        }
        gsub.extend_from_slice(b"liga");
        for w in [8u16, 0, 1, 0, 1, 4, 4, 0x0008, 1, 8, 1, 8, 1, 14, 1, 1, first, 1, 4, lig, 2, second] {
            push(&mut gsub, w);
        }
        gsub
    }

    fn minimal_thai_gpos(base: u16, mark1: u16, mark2: u16, base_x: i16, mkmk_flag: u16, mkmk_count: u16) -> Vec<u8> {
        let mut gpos = Vec::new();
        let push = |v: &mut Vec<u8>, w: u16| v.extend_from_slice(&w.to_be_bytes());
        let push_i = |v: &mut Vec<u8>, w: i16| v.extend_from_slice(&w.to_be_bytes());
        let lookup_count = 1 + mkmk_count;
        let feature_list = 32u16;
        let lookup_list = feature_list + 14 + 6 + 4 + 2 * mkmk_count;
        // Header (10) + ScriptList (22) = 32
        for w in [1u16, 0, 10, feature_list, lookup_list, 1] {
            push(&mut gpos, w);
        }
        gpos.extend_from_slice(b"thai");
        for w in [8u16, 4, 0, 0, 0xFFFF, 2, 0, 1] {
            push(&mut gpos, w);
        }
        // FeatureList: header (2 + 12) + Feature 0 ("mark", 6) + Feature 1 ("mkmk", 4 + 2*mkmk_count)
        push(&mut gpos, 2);
        gpos.extend_from_slice(b"mark");
        push(&mut gpos, 14);
        gpos.extend_from_slice(b"mkmk");
        push(&mut gpos, 20);
        for w in [0u16, 1, 0, 0, mkmk_count] {
            push(&mut gpos, w);
        }
        for i in 1..=mkmk_count {
            push(&mut gpos, i);
        }
        // LookupList header
        let l0_off = 2 + 2 * lookup_count;
        let l0_len = 54u16; // Lookup (8) + Subtable (12) + Covs (12) + MarkArray (12) + BaseArray (10)
        let l1_off = l0_off + l0_len;
        push(&mut gpos, lookup_count);
        push(&mut gpos, l0_off);
        for _ in 0..mkmk_count {
            push(&mut gpos, l1_off);
        }
        // Lookup 0: MarkToBase (46 bytes subtable)
        for w in [4u16, 0, 1, 8, 1, 12, 18, 1, 24, 36, 1, 1, mark1, 1, 1, base, 1, 0, 6, 1, 0, 0, 1, 4] {
            push(&mut gpos, w);
        }
        push(&mut gpos, 1);
        push_i(&mut gpos, base_x);
        push_i(&mut gpos, 0);
        // Lookup 1: MarkToMark (46 bytes subtable)
        for w in [6u16, mkmk_flag, 1, 8, 1, 12, 18, 1, 24, 36, 1, 1, mark2, 1, 1, mark1, 1, 0, 6, 1, 0, 0, 1, 4, 1, 0, 0] {
            push(&mut gpos, w);
        }
        gpos
    }

    /// Covered marks whose mark or target anchor table fails to parse are rejected by HarfRust, so
    /// `lookup_attaches` must parse both anchors rather than checking offset nullness alone.
    #[test]
    fn thai_mark_lookups_with_corrupt_anchors_are_refused() {
        let Some(face) = crate::document_thai_font() else { return };
        assert!(shape(face.bytes, "ดี").is_ok());
        for corrupt_mark in [true, false] {
            let mut bytes = face.bytes.to_vec();
            let gpos_off = gpos(&bytes);
            let lookups = gpos_off + u16_at(&bytes, gpos_off + 8);
            for idx in thai_mark_lookup_indices(&bytes) {
                let lookup = lookups + u16_at(&bytes, lookups + 2 + 2 * idx);
                let kind = u16_at(&bytes, lookup);
                let mut sub = lookup + u16_at(&bytes, lookup + 6);
                let actual_kind = if kind == 9 {
                    let k = u16_at(&bytes, sub + 2);
                    sub += u32_at(&bytes, sub + 4);
                    k
                } else {
                    kind
                };
                if !matches!(actual_kind, 4 | 6) {
                    continue;
                }
                if corrupt_mark {
                    let mark_array = sub + u16_at(&bytes, sub + 8);
                    for r in 0..u16_at(&bytes, mark_array) {
                        let anchor = mark_array + u16_at(&bytes, mark_array + 4 + 4 * r);
                        bytes[anchor..anchor + 2].copy_from_slice(&9u16.to_be_bytes());
                    }
                } else {
                    let class_count = u16_at(&bytes, sub + 6);
                    let base_array = sub + u16_at(&bytes, sub + 10);
                    let rows = u16_at(&bytes, base_array);
                    for j in 0..rows * class_count {
                        let off = u16_at(&bytes, base_array + 2 + 2 * j);
                        if off != 0 {
                            let anchor = base_array + off;
                            bytes[anchor..anchor + 2].copy_from_slice(&9u16.to_be_bytes());
                        }
                    }
                }
            }
            let error = shape(&bytes, "ดี").unwrap_err();
            assert!(error.contains("mark positioning"), "corrupt_mark={corrupt_mark}: {error}");
        }
    }

    /// A GSUB ligature merges two marked bases into one cluster whose marks carry distinct
    /// ligature components; cross-component `mkmk` is rejected by HarfRust and must not pass.
    #[test]
    fn thai_ligated_bases_with_cross_component_mkmk_are_refused() {
        let Some(face) = crate::document_thai_font() else { return };
        let no_ee = shape(face.bytes, "นี่").unwrap();
        let [base_no, vowel] = [0, 1].map(|i| u16::try_from(no_ee.glyphs[i].id.to_u32()).unwrap());
        let ka_ek = shape(face.bytes, "ก\u{0e48}").unwrap();
        let [ka, wide_tone] = [0, 1].map(|i| u16::try_from(ka_ek.glyphs[i].id.to_u32()).unwrap());

        let base_gpos = with_table(face.bytes, b"GPOS", minimal_thai_gpos(base_no, vowel, wide_tone, 10, 0x0000, 1));
        let unligated = with_table(&base_gpos, b"GSUB", minimal_liga_gsub(ka, ka, ka));
        assert!(shape(&unligated, "น\u{0e35}\u{0e48}").is_ok(), "unligated mkmk passes");

        let ligated = with_table(&base_gpos, b"GSUB", minimal_liga_gsub(base_no, ka, base_no));
        assert_eq!(shape_legacy_thai(&ligated, "น\u{0e35}ก\u{0e48}").unwrap().glyphs.len(), 3, "GSUB ligates across vowel");
        let error = shape(&ligated, "น\u{0e35}ก\u{0e48}").unwrap_err();
        assert!(error.contains("mark positioning"), "{error}");
    }

    /// Default-ignorable combiners such as CGJ (`U+034F`) and NFD Latin accents normalized to
    /// precomposed glyphs inside a mixed run do not fail the Thai mark count.
    #[test]
    fn thai_runs_with_cgj_and_precomposed_latin_marks_succeed() {
        let Some(face) = crate::document_thai_font() else { return };
        assert!(shape(face.bytes, "กิ\u{034f}").is_ok(), "CGJ in Thai cluster");
        assert!(shape(face.bytes, "e\u{0301} กิ่").is_ok(), "precomposed Latin mark before Thai");
    }

    /// Thousands of supported `MarkToBase` marks on one base would trigger quadratic offset
    /// propagation inside HarfRust and must be refused promptly before shaping.
    #[test]
    fn many_supported_marks_in_one_thai_cluster_are_refused_without_quadratic_shaping() {
        let Some(face) = crate::document_thai_font() else { return };
        let shaped = shape(face.bytes, "กิ").unwrap();
        let [ka, sara_i] = [0, 1].map(|i| u16::try_from(shaped.glyphs[i].id.to_u32()).unwrap());
        let bytes = with_table(face.bytes, b"GPOS", minimal_thai_gpos(ka, sara_i, sara_i, 10, 0x0008, 0));
        let text = format!("ก{}", "\u{0e34}".repeat(20_000));
        let started = std::time::Instant::now();
        let error = shape(&bytes, &text).unwrap_err();
        assert!(error.contains("mark positioning"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    }

    /// Thousands of skipped `mkmk` lookups over thousands of Thai clusters must neither retain
    /// per-cluster × lookup tables nor spend quadratic time checking every cluster.
    #[test]
    fn many_thai_clusters_with_skipped_mkmk_lookups_fail_promptly() {
        let Some(face) = crate::document_thai_font() else { return };
        let shaped = shape(face.bytes, "กิ").unwrap();
        let [ka, sara_i] = [0, 1].map(|i| u16::try_from(shaped.glyphs[i].id.to_u32()).unwrap());
        let text = "กิ ".repeat(5_000);

        // 1. 8,000 IgnoreMarks mkmk lookups are stripped upfront, so valid MarkToBase runs quickly.
        let ignored = with_table(face.bytes, b"GPOS", minimal_thai_gpos(ka, sara_i, sara_i, 10, 0x0008, 8_000));
        let started = std::time::Instant::now();
        assert!(shape(&ignored, &text).is_ok());
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "ignored mkmk: {:?}", started.elapsed());

        // 2. 8,000 non-IgnoreMarks MarkAttachmentType=1 mkmk lookups over unmoved marks hit the
        // work bound cleanly without allocating clusters * lookups state.
        let skipped = with_table(face.bytes, b"GPOS", minimal_thai_gpos(ka, sara_i, sara_i, 0, 0x0100, 8_000));
        let started = std::time::Instant::now();
        let error = shape(&skipped, &text).unwrap_err();
        assert!(error.contains("mark positioning"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "skipped mkmk: {:?}", started.elapsed());
    }

    fn minimal_transitive_liga_gsub(first: u16, second: u16, mid: u16, final_gid: u16, use_alternate: bool) -> Vec<u8> {
        let mut gsub = Vec::new();
        let push = |v: &mut Vec<u8>, w: u16| v.extend_from_slice(&w.to_be_bytes());
        for w in [1u16, 0, 10, 30, 46, 1] {
            push(&mut gsub, w);
        }
        gsub.extend_from_slice(b"thai");
        for w in [8u16, 4, 0, 0, 0xFFFF, 1, 0, 1] {
            push(&mut gsub, w);
        }
        gsub.extend_from_slice(b"liga");
        for w in [8u16, 0, 2, 0, 1, 2, 6, 38] {
            push(&mut gsub, w);
        }
        // Lookup 0: LigatureSubst (IgnoreMarks 0x0008, 32 bytes): first + second -> mid
        for w in [4u16, 0x0008, 1, 8, 1, 8, 1, 14, 1, 1, first, 1, 4, mid, 2, second] {
            push(&mut gsub, w);
        }
        // Lookup 1: SingleSubstFormat2 or AlternateSubstFormat1: mid -> final_gid
        if use_alternate {
            for w in [3u16, 0, 1, 8, 1, 8, 1, 14, 1, 1, mid, 1, final_gid] {
                push(&mut gsub, w);
            }
        } else {
            for w in [1u16, 0, 1, 8, 2, 8, 1, final_gid, 1, 1, mid] {
                push(&mut gsub, w);
            }
        }
        gsub
    }

    fn aliased_liga_gsub(lookups: u16, sets: u16, ligs: u16, base: u16, lig: u16) -> Vec<u8> {
        let mut gsub = Vec::new();
        let push = |v: &mut Vec<u8>, w: u16| v.extend_from_slice(&w.to_be_bytes());
        for w in [1u16, 0, 10, 12, 14, 0, 0, lookups] {
            push(&mut gsub, w);
        }
        let l0_off = 2 + 2 * lookups;
        for _ in 0..lookups {
            push(&mut gsub, l0_off);
        }
        let cov_off = 6 + 2 * sets;
        let set_off = cov_off + 6;
        for w in [4u16, 0, 1, 8, 1, cov_off, sets] {
            push(&mut gsub, w);
        }
        for _ in 0..sets {
            push(&mut gsub, set_off);
        }
        for w in [1u16, 1, base, ligs] {
            push(&mut gsub, w);
        }
        let lig_off = 2 + 2 * ligs;
        for _ in 0..ligs {
            push(&mut gsub, lig_off);
        }
        push(&mut gsub, lig);
        push(&mut gsub, 1);
        gsub
    }

    /// Valid RTL bidi Thai runs reverse clusters visually whilst preserving forward glyph order
    /// inside each cluster (`[base, mark1, mark2]`).
    #[test]
    fn thai_rtl_runs_keep_intra_cluster_mark_order() {
        let Some(face) = crate::document_thai_font() else { return };
        for text in ["\u{202e}ก\u{0e34}\u{202c}", "\u{202e}ก\u{0e34}\u{e48}\u{202c}", "\u{202e}ก\u{0e34} ข\u{0e34}\u{202c}", "\u{202e}นี่ ดี ปู่\u{202c}"]
        {
            assert!(shape(face.bytes, text).is_ok(), "RTL Thai run failed: {text:?}");
        }
    }

    /// A GSUB ligature rewritten by a subsequent `SingleSubst` or `AlternateSubst` still keeps its
    /// HarfRust `lig_id`/`lig_comp`, so cross-component `mkmk` remains unpositioned and must fail.
    #[test]
    fn thai_ligated_bases_rewritten_by_single_or_alternate_subst_are_refused() {
        let Some(face) = crate::document_thai_font() else { return };
        let no_ee = shape(face.bytes, "นี่").unwrap();
        let [base_no, vowel] = [0, 1].map(|i| u16::try_from(no_ee.glyphs[i].id.to_u32()).unwrap());
        let ka_ek = shape(face.bytes, "ก\u{0e48}").unwrap();
        let [ka, wide_tone] = [0, 1].map(|i| u16::try_from(ka_ek.glyphs[i].id.to_u32()).unwrap());
        let mid_gid = u16::try_from(shape(face.bytes, "ร").unwrap().glyphs[0].id.to_u32()).unwrap();
        let base_gpos = with_table(face.bytes, b"GPOS", minimal_thai_gpos(base_no, vowel, wide_tone, 10, 0x0000, 1));
        for use_alternate in [false, true] {
            let bytes = with_table(&base_gpos, b"GSUB", minimal_transitive_liga_gsub(base_no, ka, mid_gid, base_no, use_alternate));
            let legacy = shape_legacy_thai(&bytes, "น\u{0e35}ก\u{0e48}").unwrap();
            assert_eq!(legacy.glyphs.len(), 3, "use_alternate={use_alternate}");
            assert_eq!(legacy.glyphs[0].id.to_u32(), u32::from(base_no), "Lookup 1 rewrite failed: use_alternate={use_alternate}");
            let error = shape(&bytes, "น\u{0e35}ก\u{0e48}").unwrap_err();
            assert!(error.contains("mark positioning"), "use_alternate={use_alternate}: {error}");
        }
    }

    /// Aliased GSUB ligature offsets must fail closed within budget rather than spending seconds.
    #[test]
    fn pathological_aliased_gsub_ligatures_fail_promptly() {
        let Some(face) = crate::document_thai_font() else { return };
        let ka = u16::try_from(shape(face.bytes, "ก").unwrap().glyphs[0].id.to_u32()).unwrap();
        let bytes = with_table(face.bytes, b"GSUB", aliased_liga_gsub(40, 1_000, 1_000, ka, ka));
        let started = std::time::Instant::now();
        let error = shape(&bytes, "กิ").unwrap_err();
        assert!(error.contains("mark positioning"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
    }

    fn aliased_single_gsub(lookups: u16, subs: u16, ka: u16) -> Vec<u8> {
        let mut gsub = Vec::new();
        let push = |v: &mut Vec<u8>, w: u16| v.extend_from_slice(&w.to_be_bytes());
        for w in [1u16, 0, 10, 12, 14, 0, 0, lookups] {
            push(&mut gsub, w);
        }
        let l0_off = 2 + 2 * lookups;
        for _ in 0..lookups {
            push(&mut gsub, l0_off);
        }
        let sub_off = 6 + 2 * subs;
        for w in [1u16, 0, subs] {
            push(&mut gsub, w);
        }
        for _ in 0..subs {
            push(&mut gsub, sub_off);
        }
        for w in [1u16, 6, 0, 1, 1, ka] {
            push(&mut gsub, w);
        }
        gsub
    }

    /// Aliased `SingleSubst`/`MultipleSubst`/`AlternateSubst` subtables must fail within budget
    /// even when the GSUB defines zero ligatures.
    #[test]
    fn pathological_aliased_gsub_rewrites_fail_promptly() {
        let Some(face) = crate::document_thai_font() else { return };
        let ka = u16::try_from(shape(face.bytes, "ก").unwrap().glyphs[0].id.to_u32()).unwrap();
        let bytes = with_table(face.bytes, b"GSUB", aliased_single_gsub(500, 10_000, ka));
        let started = std::time::Instant::now();
        let error = shape(&bytes, "กิ").unwrap_err();
        assert!(error.contains("mark positioning"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
    }

    /// HarfRust's `MarkBasePosFormat1::apply` overrides the target search with `IGNORE_MARKS`, so
    /// `IgnoreBaseGlyphs` (`0x0002`) only refuses attachment when the current mark glyph itself is
    /// classified as `GDEF class 1` (`Base`), whilst a `class 3` mark still attaches onto `class 1`.
    #[test]
    fn thai_mark_lookup_with_ignore_base_glyphs_is_refused() {
        use skrifa::MetadataProvider as _;
        let Some(face) = crate::document_thai_font() else { return };
        let shaped = shape(face.bytes, "กิ").unwrap();
        let [ka, sara_i] = [0, 1].map(|i| u16::try_from(shaped.glyphs[i].id.to_u32()).unwrap());
        let ka_adv = FontRef::new(face.bytes)
            .unwrap()
            .glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default())
            .advance_width(GlyphId::new(u32::from(ka)))
            .unwrap_or_default()
            .round() as i16;
        let mut gpos = minimal_thai_gpos(ka, sara_i, sara_i, ka_adv, 0, 0);
        let lookups = u16_at(&gpos, 8);
        let l0 = lookups + u16_at(&gpos, lookups + 2);
        gpos[l0 + 2..l0 + 4].copy_from_slice(&0x0002u16.to_be_bytes());
        let with_gpos = with_table(face.bytes, b"GPOS", gpos);
        assert!(shape(&with_gpos, "กิ").is_ok());
        // GDEF Format 2 GlyphClassDef mapping both `ka` and `sara_i` to class 1 (Base).
        let mut sorted = [ka, sara_i];
        sorted.sort_unstable();
        let gdef: Vec<u8> =
            [1u16, 0, 12, 0, 0, 0, 2, 2, sorted[0], sorted[0], 1, sorted[1], sorted[1], 1].into_iter().flat_map(u16::to_be_bytes).collect();
        let bytes = with_table(&with_gpos, b"GDEF", gdef);
        let error = shape(&bytes, "กิ").unwrap_err();
        assert!(error.contains("mark positioning"), "{error}");
    }

    /// In a bilingual font with distinct `latn` and `thai` `mark` GPOS lookups, shaping an un-
    /// normalized NFD Latin accent alongside Thai marks must position both marks using their own
    /// OpenType scripts.
    #[test]
    fn mixed_latin_nfd_accent_and_thai_marks_both_position() {
        let Some(face) = crate::document_thai_font() else { return };
        let latin = shape(face.bytes, "q\u{0301}").unwrap();
        let [q_gid, acute] = [0, 1].map(|i| u16::try_from(latin.glyphs[i].id.to_u32()).unwrap());
        let thai = shape(face.bytes, "กิ").unwrap();
        let [ka, sara_i] = [0, 1].map(|i| u16::try_from(thai.glyphs[i].id.to_u32()).unwrap());

        let mut gpos = Vec::new();
        let push = |v: &mut Vec<u8>, w: u16| v.extend_from_slice(&w.to_be_bytes());
        let push_i = |v: &mut Vec<u8>, w: i16| v.extend_from_slice(&w.to_be_bytes());
        // Header (10) + ScriptList (38) + FeatureList (26) = 74
        for w in [1u16, 0, 10, 48, 74, 2] {
            push(&mut gpos, w);
        }
        gpos.extend_from_slice(b"latn");
        push(&mut gpos, 14);
        gpos.extend_from_slice(b"thai");
        push(&mut gpos, 26);
        for feat in [0u16, 1] {
            for w in [4u16, 0, 0, 0xFFFF, 1, feat] {
                push(&mut gpos, w);
            }
        }
        push(&mut gpos, 2);
        for off in [14u16, 20] {
            gpos.extend_from_slice(b"mark");
            push(&mut gpos, off);
        }
        for idx in [0u16, 1] {
            for w in [0u16, 1, idx] {
                push(&mut gpos, w);
            }
        }
        for w in [2u16, 6, 60] {
            push(&mut gpos, w);
        }
        for (base, mark, base_y) in [(q_gid, acute, 200i16), (ka, sara_i, 150i16)] {
            for w in [4u16, 0, 1, 8, 1, 12, 18, 1, 24, 36, 1, 1, mark, 1, 1, base, 1, 0, 6, 1, 0, 0, 1, 4, 1, 10] {
                push(&mut gpos, w);
            }
            push_i(&mut gpos, base_y);
        }
        let bytes = with_table(face.bytes, b"GPOS", gpos);
        let shaped = shape(&bytes, "q\u{0301} กิ").unwrap();
        let acute_glyph = shaped.glyphs.iter().find(|g| g.id.to_u32() == u32::from(acute)).unwrap();
        let sara_glyph = shaped.glyphs.iter().find(|g| g.id.to_u32() == u32::from(sara_i)).unwrap();
        assert!(acute_glyph.position[1].abs() > 0.0, "Latin NFD accent lost latn GPOS: {acute_glyph:?}");
        assert!(sara_glyph.position[1].abs() > 0.0, "Thai mark lost thai GPOS: {sara_glyph:?}");
    }

    /// Zero-delta `MarkToLigature` (GPOS type 5) lookups with valid component anchors must be
    /// accepted by `places_thai_marks`, whilst corrupt or NULL component anchors must be refused.
    #[test]
    fn thai_mark_to_ligature_zero_delta_accepts_valid_and_refuses_corrupt_anchors() {
        use skrifa::MetadataProvider as _;
        let Some(face) = crate::document_thai_font() else { return };
        let shaped = shape(face.bytes, "กิ").unwrap();
        let [ka, sara_i] = [0, 1].map(|i| u16::try_from(shaped.glyphs[i].id.to_u32()).unwrap());
        let ka_adv = FontRef::new(face.bytes)
            .unwrap()
            .glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default())
            .advance_width(GlyphId::new(u32::from(ka)))
            .unwrap_or_default()
            .round() as i16;
        let mut gpos = minimal_thai_gpos(ka, sara_i, sara_i, ka_adv, 0, 0);
        let l0 = u16_at(&gpos, 8) + u16_at(&gpos, u16_at(&gpos, 8) + 2);
        gpos[l0..l0 + 2].copy_from_slice(&5u16.to_be_bytes());
        gpos.truncate(l0 + 8 + 36);
        let push = |v: &mut Vec<u8>, w: u16| v.extend_from_slice(&w.to_be_bytes());
        let push_i = |v: &mut Vec<u8>, w: i16| v.extend_from_slice(&w.to_be_bytes());
        for w in [1u16, 4, 2, 6, 12] {
            push(&mut gpos, w);
        }
        let anchor1_off = gpos.len();
        for _ in 0..2 {
            push(&mut gpos, 1);
            push_i(&mut gpos, ka_adv);
            push_i(&mut gpos, 0);
        }
        let valid = with_table(face.bytes, b"GPOS", gpos.clone());
        assert!(shape(&valid, "กิ").is_ok());

        let mut corrupt_gpos = gpos.clone();
        corrupt_gpos[anchor1_off + 6..anchor1_off + 8].copy_from_slice(&9u16.to_be_bytes());
        let corrupt = with_table(face.bytes, b"GPOS", corrupt_gpos);
        assert!(shape(&corrupt, "กิ").unwrap_err().contains("mark positioning"));

        let mut null_gpos = gpos;
        null_gpos[anchor1_off - 2..anchor1_off].copy_from_slice(&0u16.to_be_bytes());
        let null_comp = with_table(face.bytes, b"GPOS", null_gpos);
        assert!(shape(&null_comp, "กิ").unwrap_err().contains("mark positioning"));
    }

    /// Component-specific `MarkToLigature` anchors: a 2-component GSUB ligature with an anchor only
    /// on the targeted component is accepted, whilst attaching a mark to a NULL component fails.
    #[test]
    fn thai_mark_to_ligature_component_specific_anchor_follows_mark_component() {
        use skrifa::MetadataProvider as _;
        let Some(face) = crate::document_thai_font() else { return };
        let [no_gid, ka_gid, mid_gid, sara_i] = [
            shape(face.bytes, "น").unwrap().glyphs[0].id.to_u32(),
            shape(face.bytes, "ก").unwrap().glyphs[0].id.to_u32(),
            shape(face.bytes, "ร").unwrap().glyphs[0].id.to_u32(),
            shape(face.bytes, "กิ").unwrap().glyphs[1].id.to_u32(),
        ]
        .map(|g| u16::try_from(g).unwrap());
        let mid_adv = FontRef::new(face.bytes)
            .unwrap()
            .glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default())
            .advance_width(GlyphId::new(u32::from(mid_gid)))
            .unwrap_or_default()
            .round() as i16;
        let gsub = minimal_transitive_liga_gsub(no_gid, ka_gid, mid_gid, mid_gid, false);
        let build_lig_gpos = |comp0_on: bool, comp1_on: bool| {
            let mut gpos = minimal_thai_gpos(mid_gid, sara_i, sara_i, mid_adv, 0, 0);
            let l0 = u16_at(&gpos, 8) + u16_at(&gpos, u16_at(&gpos, 8) + 2);
            gpos[l0..l0 + 2].copy_from_slice(&5u16.to_be_bytes());
            gpos.truncate(l0 + 8 + 36);
            let push = |v: &mut Vec<u8>, w: u16| v.extend_from_slice(&w.to_be_bytes());
            let push_i = |v: &mut Vec<u8>, w: i16| v.extend_from_slice(&w.to_be_bytes());
            for w in [1u16, 4, 2, if comp0_on { 6 } else { 0 }, if comp1_on { 6 } else { 0 }] {
                push(&mut gpos, w);
            }
            push(&mut gpos, 1);
            push_i(&mut gpos, mid_adv);
            push_i(&mut gpos, 0);
            let with_gpos = with_table(face.bytes, b"GPOS", gpos);
            with_table(&with_gpos, b"GSUB", gsub.clone())
        };
        assert!(shape(&build_lig_gpos(true, false), "น\u{0e34}ก").is_ok(), "comp 0 anchor should pass");
        assert!(shape(&build_lig_gpos(false, true), "นก\u{0e34}").is_ok(), "comp 1 anchor should pass");
        assert!(shape(&build_lig_gpos(false, true), "น\u{0e34}ก").unwrap_err().contains("mark positioning"));
        assert!(shape(&build_lig_gpos(true, false), "น\u{0e34}ก\u{0e34}").unwrap_err().contains("mark positioning"));
    }

    /// Corrupt GSUB records in `ligature_glyphs` fail closed rather than silently skipping errors.
    #[test]
    fn corrupt_gsub_ligature_record_fails_closed() {
        let Some(face) = crate::document_thai_font() else { return };
        let ka = u16::try_from(shape(face.bytes, "ก").unwrap().glyphs[0].id.to_u32()).unwrap();
        let mut gsub = aliased_liga_gsub(1, 1, 1, ka, ka);
        gsub.truncate(gsub.len() - 2);
        let bytes = with_table(face.bytes, b"GSUB", gsub);
        let error = shape(&bytes, "กิ").unwrap_err();
        assert!(error.contains("mark positioning"), "{error}");
    }

    /// Aliased GPOS subtable offsets past `MAX_THAI_LOOKUP_WORK` fail closed promptly.
    #[test]
    fn pathological_aliased_gpos_subtables_fail_promptly() {
        use skrifa::MetadataProvider as _;
        let Some(face) = crate::document_thai_font() else { return };
        let font = FontRef::new(face.bytes).unwrap();
        let cmap = font.charmap();
        let metrics = font.glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
        let mut bases: Vec<(char, u16, i16)> = ('\u{0e01}'..='\u{0e2e}')
            .filter_map(|c| {
                let g = cmap.map(c)?;
                let id = u16::try_from(g.to_u32()).ok()?;
                let adv = metrics.advance_width(g).unwrap_or_default().round() as i16;
                Some((c, id, adv))
            })
            .collect();
        bases.sort_by_key(|x| x.1);
        bases.dedup_by_key(|x| x.1);
        let mut marks: Vec<(char, u16)> = ['\u{0e34}', '\u{0e35}', '\u{0e38}', '\u{0e39}', '\u{0e48}']
            .into_iter()
            .filter_map(|c| Some((c, u16::try_from(cmap.map(c)?.to_u32()).ok()?)))
            .collect();
        marks.sort_by_key(|x| x.1);
        let unrelated = bases.pop().unwrap().1;

        let mut gpos = Vec::new();
        let push = |v: &mut Vec<u8>, w: u16| v.extend_from_slice(&w.to_be_bytes());
        let push_i = |v: &mut Vec<u8>, w: i16| v.extend_from_slice(&w.to_be_bytes());
        // Header (10) + ScriptList (20) + FeatureList (14) = 44
        for w in [1u16, 0, 10, 30, 44, 1] {
            push(&mut gpos, w);
        }
        gpos.extend_from_slice(b"thai");
        for w in [8u16, 4, 0, 0, 0xFFFF, 1, 0, 1] {
            push(&mut gpos, w);
        }
        gpos.extend_from_slice(b"mark");
        for w in [8u16, 0, 1, 0, 1, 4] {
            push(&mut gpos, w);
        }
        let l0_abs = gpos.len();
        let subs: u16 = 25_000;
        let miss_off = 6 + 2 * subs;
        let hit_off = miss_off + 46;
        for w in [4u16, 0, subs] {
            push(&mut gpos, w);
        }
        for _ in 0..subs - 1 {
            push(&mut gpos, miss_off);
        }
        push(&mut gpos, hit_off);
        assert_eq!(gpos.len() - l0_abs, usize::from(miss_off));
        for w in [1u16, 12, 18, 1, 24, 36, 1, 1, unrelated, 1, 1, unrelated, 1, 0, 6, 1, 0, 0, 1, 4, 1, 0, 0] {
            push(&mut gpos, w);
        }
        assert_eq!(gpos.len() - l0_abs, usize::from(hit_off));
        let mark_cov_off = 12u16;
        let mark_cov_len = 4 + 2 * marks.len() as u16;
        let base_cov_off = mark_cov_off + mark_cov_len;
        let base_cov_len = 4 + 2 * bases.len() as u16;
        let mark_arr_off = base_cov_off + base_cov_len;
        let mark_arr_len = 2 + 4 * marks.len() as u16 + 6;
        let base_arr_off = mark_arr_off + mark_arr_len;
        for w in [1u16, mark_cov_off, base_cov_off, 1, mark_arr_off, base_arr_off, 1, marks.len() as u16] {
            push(&mut gpos, w);
        }
        for &(_, m) in &marks {
            push(&mut gpos, m);
        }
        for w in [1u16, bases.len() as u16] {
            push(&mut gpos, w);
        }
        for &(_, b, _) in &bases {
            push(&mut gpos, b);
        }
        let anchor0 = 2 + 4 * marks.len() as u16;
        push(&mut gpos, marks.len() as u16);
        for _ in &marks {
            push(&mut gpos, 0);
            push(&mut gpos, anchor0);
        }
        for w in [1u16, 0, 0] {
            push(&mut gpos, w);
        }
        let first_base_anchor = 2 + 2 * bases.len() as u16;
        push(&mut gpos, bases.len() as u16);
        for idx in 0..bases.len() {
            push(&mut gpos, first_base_anchor + 6 * idx as u16);
        }
        for &(_, _, adv) in &bases {
            push(&mut gpos, 1);
            push_i(&mut gpos, adv);
            push_i(&mut gpos, 0);
        }
        let gpos_bytes = with_table(face.bytes, b"GPOS", gpos);
        let bytes = with_table(&gpos_bytes, b"GSUB", minimal_liga_gsub(unrelated, unrelated, unrelated));
        let text: String = marks.iter().flat_map(|&(m, _)| bases.iter().map(move |&(b, _, _)| format!("{b}{m} "))).collect();
        let started = std::time::Instant::now();
        let error = shape(&bytes, &text).unwrap_err();
        assert!(error.contains("mark positioning"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
    }

    /// Dropping `GDEF` with CGJ (`U+034F`) between ligature base and mark must not skip `mark_ord`
    /// onto a later component and fail open on an unpositioned NULL-anchor `MarkToLigature` mark.
    #[test]
    fn thai_cgj_does_not_shift_ligature_component_or_fail_open_without_gdef() {
        use skrifa::MetadataProvider as _;
        let Some(face) = crate::document_thai_font() else { return };
        let [no_gid, ka_gid, mid_gid, sara_i, mai_ek] = [
            shape(face.bytes, "น").unwrap().glyphs[0].id.to_u32(),
            shape(face.bytes, "ก").unwrap().glyphs[0].id.to_u32(),
            shape(face.bytes, "ร").unwrap().glyphs[0].id.to_u32(),
            shape(face.bytes, "กิ").unwrap().glyphs[1].id.to_u32(),
            shape(face.bytes, "ก\u{0e48}").unwrap().glyphs[1].id.to_u32(),
        ]
        .map(|g| u16::try_from(g).unwrap());
        let mid_adv = FontRef::new(face.bytes)
            .unwrap()
            .glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default())
            .advance_width(GlyphId::new(u32::from(mid_gid)))
            .unwrap_or_default()
            .round() as i16;
        let gsub = minimal_transitive_liga_gsub(no_gid, ka_gid, mid_gid, mid_gid, false);
        let mut gpos = minimal_thai_gpos(mid_gid, sara_i, sara_i, mid_adv, 0, 0);
        let l0 = u16_at(&gpos, 8) + u16_at(&gpos, u16_at(&gpos, 8) + 2);
        gpos[l0..l0 + 2].copy_from_slice(&5u16.to_be_bytes());
        gpos.truncate(l0 + 8);
        let push = |v: &mut Vec<u8>, w: u16| v.extend_from_slice(&w.to_be_bytes());
        let push_i = |v: &mut Vec<u8>, w: i16| v.extend_from_slice(&w.to_be_bytes());
        let marks = if sara_i < mai_ek { [(sara_i, 0i16), (mai_ek, -250i16)] } else { [(mai_ek, -250i16), (sara_i, 0i16)] };
        for w in [1u16, 12, 20, 1, 26, 48, 1, 2, marks[0].0, marks[1].0, 1, 1, mid_gid, 2, 0, 10, 0, 16] {
            push(&mut gpos, w);
        }
        for (_, y) in marks {
            push(&mut gpos, 1);
            push_i(&mut gpos, 0);
            push_i(&mut gpos, y);
        }
        for w in [1u16, 4, 2, 0, 6, 1] {
            push(&mut gpos, w);
        }
        push_i(&mut gpos, mid_adv);
        push_i(&mut gpos, 0);
        let with_gpos = with_table(face.bytes, b"GPOS", gpos);
        let with_gsub = with_table(&with_gpos, b"GSUB", gsub);
        let no_gdef = with_table(&with_gsub, b"GDEF", Vec::new());
        assert!(shape(&no_gdef, "น\u{0e34}ก\u{0e48}").unwrap_err().contains("mark positioning"));
        let error = shape(&no_gdef, "น\u{034f}\u{0e34}ก\u{0e48}").unwrap_err();
        assert!(error.contains("mark positioning"), "{error}");
    }

    /// Valid Thai clusters with CGJ (`U+034F`) between base and mark (or between marks) still pass
    /// zero-delta GPOS when `GDEF` maps `' '` as a non-mark base glyph.
    #[test]
    fn thai_cgj_between_base_and_zero_delta_mark_is_accepted() {
        use skrifa::MetadataProvider as _;
        let Some(face) = crate::document_thai_font() else { return };
        let shaped = shape(face.bytes, "กิ่").unwrap();
        let [ka, sara_i, mai_ek] = [0, 1, 2].map(|i| u16::try_from(shaped.glyphs[i].id.to_u32()).unwrap());
        let ka_adv = FontRef::new(face.bytes)
            .unwrap()
            .glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default())
            .advance_width(GlyphId::new(u32::from(ka)))
            .unwrap_or_default()
            .round() as i16;
        let bytes = with_table(face.bytes, b"GPOS", minimal_thai_gpos(ka, sara_i, mai_ek, ka_adv, 0, 1));
        assert!(shape(&bytes, "ก\u{0e34}").is_ok(), "baseline zero-delta passes");
        assert!(shape(&bytes, "ก\u{034f}\u{0e34}").is_ok(), "CGJ before zero-delta mark should pass");
        assert!(shape(&bytes, "ก\u{0e34}\u{034f}\u{0e48}").is_ok(), "CGJ before mkmk mark should pass");
    }

    /// Generated zero-delta `MarkToBase`, `MarkToLigature`, and `MarkToMark` cases (including `GDEF`
    /// presence/classes, lookup flags, filtering sets, CGJ, and missing/corrupt anchors) match
    /// HarfRust's actual shaping decisions with zero false accepts or false refusals (`G1`).
    #[test]
    fn thai_validator_matches_harfrust_oracle_across_targets_filters_and_anchors() {
        use skrifa::MetadataProvider as _;
        let Some(face) = crate::document_thai_font() else { return };
        let font = FontRef::new(face.bytes).unwrap();
        let metrics = font.glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
        let gid_adv = |g: u16| metrics.advance_width(GlyphId::new(u32::from(g))).unwrap_or_default().round() as i16;
        let [no_gid, ka_gid, mid_gid, sara_i, sara_ii, mai_ek, unrelated] = [
            shape(face.bytes, "น").unwrap().glyphs[0].id.to_u32(),
            shape(face.bytes, "ก").unwrap().glyphs[0].id.to_u32(),
            shape(face.bytes, "ร").unwrap().glyphs[0].id.to_u32(),
            shape(face.bytes, "ก\u{0e34}").unwrap().glyphs[1].id.to_u32(),
            shape(face.bytes, "น\u{0e35}").unwrap().glyphs[1].id.to_u32(),
            shape(face.bytes, "ก\u{0e48}").unwrap().glyphs[1].id.to_u32(),
            shape(face.bytes, "ข").unwrap().glyphs[0].id.to_u32(),
        ]
        .map(|g| u16::try_from(g).unwrap());
        let [ka_adv, mid_adv] = [gid_adv(ka_gid), gid_adv(mid_gid)];
        let push = |v: &mut Vec<u8>, w: u16| v.extend_from_slice(&w.to_be_bytes());
        let push_i = |v: &mut Vec<u8>, w: i16| v.extend_from_slice(&w.to_be_bytes());

        let build_gdef = |cls: &[(u16, u16)], mcls: &[(u16, u16)], fset_gids: &[u16]| -> Vec<u8> {
            let mut out = Vec::new();
            for _ in 0..7 {
                push(&mut out, 0);
            }
            out[0..4].copy_from_slice(&[0, 1, 0, 2]);
            let write_cls = |buf: &mut Vec<u8>, items: &[(u16, u16)]| -> u16 {
                if items.is_empty() {
                    return 0;
                }
                let mut sorted = items.to_vec();
                sorted.sort_by_key(|x| x.0);
                sorted.dedup_by_key(|x| x.0);
                let off = buf.len() as u16;
                push(buf, 2);
                push(buf, sorted.len() as u16);
                for (g, c) in sorted {
                    for w in [g, g, c] {
                        push(buf, w);
                    }
                }
                off
            };
            let cls_off = write_cls(&mut out, cls);
            let mcls_off = write_cls(&mut out, mcls);
            let msets_off = if fset_gids.is_empty() {
                0
            } else {
                let mut sorted = fset_gids.to_vec();
                sorted.sort_unstable();
                sorted.dedup();
                let off = out.len() as u16;
                for w in [1u16, 1, 0, 8, 1, sorted.len() as u16] {
                    push(&mut out, w);
                }
                for g in sorted {
                    push(&mut out, g);
                }
                off
            };
            out[4..6].copy_from_slice(&cls_off.to_be_bytes());
            out[10..12].copy_from_slice(&mcls_off.to_be_bytes());
            out[12..14].copy_from_slice(&msets_off.to_be_bytes());
            out
        };

        let apply_gdef = |bytes: &[u8], gdef: Option<&Vec<u8>>| match gdef {
            None => bytes.to_vec(),
            Some(tbl) => with_table(bytes, b"GDEF", tbl.clone()),
        };

        let harfrust_attaches = |bytes: &[u8], text: &str, expected_marks: usize| -> bool {
            let f = FontRef::new(bytes).unwrap();
            let sd = harfrust::ShaperData::new(&f);
            let sh = sd.shaper(&f).build();
            let off = [b"mark", b"mkmk"].map(|t| harfrust::Feature::new(harfrust::Tag::new(t), 0, ..));
            let (on_b, off_b) = (
                shape_in(&sh, text, Direction::LeftToRight, Some(harfrust::script::THAI), &[]),
                shape_in(&sh, text, Direction::LeftToRight, Some(harfrust::script::THAI), &off),
            );
            let at = |p: &harfrust::GlyphPosition| (p.x_advance, p.y_advance, p.x_offset, p.y_offset);
            on_b.glyph_positions().iter().zip(off_b.glyph_positions()).filter(|(a, b)| at(a) != at(b)).count() >= expected_marks
        };

        let assert_oracle = |zero_bytes: &[u8], shifted_bytes: &[u8], text: &str, expected_marks: usize, label: &str| {
            let oracle = harfrust_attaches(shifted_bytes, text, expected_marks);
            assert_eq!(shape(zero_bytes, text).is_ok(), oracle, "oracle={oracle} on {label}");
        };

        // 1. MarkToBase across flags, GDEF class variations, and missing/corrupt anchors.
        #[derive(Clone, Copy)]
        enum AnchorKind {
            Valid,
            NullTarget,
            CorruptTarget,
            CorruptMark,
        }
        let build_mtb = |flag: u16, anchor: AnchorKind, base_y: i16| {
            let mut gpos = minimal_thai_gpos(ka_gid, sara_i, sara_i, ka_adv, 0, 0);
            let lookups = u16_at(&gpos, 8);
            let l0 = lookups + u16_at(&gpos, lookups + 2);
            gpos[l0 + 2..l0 + 4].copy_from_slice(&flag.to_be_bytes());
            gpos[l0 + 52..l0 + 54].copy_from_slice(&base_y.to_be_bytes());
            match anchor {
                AnchorKind::Valid => {}
                AnchorKind::NullTarget => gpos[l0 + 46..l0 + 48].copy_from_slice(&0u16.to_be_bytes()),
                AnchorKind::CorruptTarget => gpos[l0 + 48..l0 + 50].copy_from_slice(&9u16.to_be_bytes()),
                AnchorKind::CorruptMark => gpos[l0 + 38..l0 + 40].copy_from_slice(&9u16.to_be_bytes()),
            }
            if flag & 0x0010 != 0 {
                gpos[l0 + 6..l0 + 8].copy_from_slice(&10u16.to_be_bytes());
                gpos.splice(l0 + 8..l0 + 8, 0u16.to_be_bytes());
            }
            with_table(face.bytes, b"GPOS", gpos)
        };
        let empty_gdef = Vec::new();
        let gdef_match = build_gdef(&[(ka_gid, 1), (sara_i, 3)], &[(sara_i, 1)], &[sara_i]);
        let gdef_nocls_match = build_gdef(&[], &[(sara_i, 1)], &[sara_i]);
        let gdef_mismatch = build_gdef(&[(ka_gid, 0), (sara_i, 3)], &[(sara_i, 2)], &[unrelated]);
        let gdefs_mtb: [(&str, Option<&Vec<u8>>); 5] = [
            ("keep", None),
            ("drop", Some(&empty_gdef)),
            ("match", Some(&gdef_match)),
            ("nocls_match", Some(&gdef_nocls_match)),
            ("mismatch", Some(&gdef_mismatch)),
        ];
        for flag in [0x0000u16, 0x0002, 0x0004, 0x0008, 0x0100, 0x0010] {
            for (gname, gdef) in gdefs_mtb {
                let z = apply_gdef(&build_mtb(flag, AnchorKind::Valid, 0), gdef);
                let s = apply_gdef(&build_mtb(flag, AnchorKind::Valid, 200), gdef);
                assert_oracle(&z, &s, "ก\u{0e34}", 1, &format!("mtb flag={flag:#06x} gdef={gname}"));
            }
        }
        for (aname, anchor) in
            [("null_target", AnchorKind::NullTarget), ("corrupt_target", AnchorKind::CorruptTarget), ("corrupt_mark", AnchorKind::CorruptMark)]
        {
            let z = build_mtb(0, anchor, 0);
            let s = build_mtb(0, anchor, 200);
            assert_oracle(&z, &s, "ก\u{0e34}", 1, &format!("mtb anchor={aname}"));
        }

        // 2. MarkToLigature across component masks, CGJ, non-ligated target, GDEF classes, and flags.
        let liga_gsub = minimal_transitive_liga_gsub(no_gid, ka_gid, mid_gid, mid_gid, false);
        let build_mtl = |lig: u16, adv: i16, flag: u16, c0_on: bool, c1_on: bool, anc_fmt: u16, base_y: i16| {
            let mut gpos = minimal_thai_gpos(lig, sara_i, sara_i, adv, 0, 0);
            let l0 = u16_at(&gpos, 8) + u16_at(&gpos, u16_at(&gpos, 8) + 2);
            gpos[l0..l0 + 2].copy_from_slice(&5u16.to_be_bytes());
            gpos[l0 + 2..l0 + 4].copy_from_slice(&flag.to_be_bytes());
            gpos.truncate(l0 + 8);
            for w in [1u16, 12, 18, 1, 24, 36, 1, 1, sara_i, 1, 1, lig, 1, 0, 6, 1, 0, 0] {
                push(&mut gpos, w);
            }
            for w in [1u16, 4, 2, if c0_on { 6 } else { 0 }, if c1_on { 6 } else { 0 }] {
                push(&mut gpos, w);
            }
            push(&mut gpos, anc_fmt);
            push_i(&mut gpos, adv);
            push_i(&mut gpos, base_y);
            let gp = with_table(face.bytes, b"GPOS", gpos);
            with_table(&gp, b"GSUB", liga_gsub.clone())
        };
        let gdef_lig1 = build_gdef(&[(mid_gid, 1), (sara_i, 3)], &[], &[]);
        let gdef_lig2 = build_gdef(&[(mid_gid, 2), (sara_i, 3)], &[], &[]);
        let gdef_lig0 = build_gdef(&[(mid_gid, 0), (sara_i, 3)], &[], &[]);
        for (c0, c1, fmt) in [(true, true, 1), (true, false, 1), (false, true, 1), (false, false, 1), (true, true, 9)] {
            for text in ["น\u{0e34}ก", "นก\u{0e34}", "น\u{034f}\u{0e34}ก"] {
                let z = build_mtl(mid_gid, mid_adv, 0, c0, c1, fmt, 0);
                let s = build_mtl(mid_gid, mid_adv, 0, c0, c1, fmt, 200);
                assert_oracle(&z, &s, text, 1, &format!("mtl c=({c0},{c1},fmt={fmt}) text={text:?}"));
            }
        }
        for flag in [0x0000u16, 0x0002, 0x0004, 0x0008] {
            for (gname, gdef) in
                [("keep", None), ("drop", Some(&empty_gdef)), ("c1", Some(&gdef_lig1)), ("c2", Some(&gdef_lig2)), ("c0", Some(&gdef_lig0))]
            {
                let z = apply_gdef(&build_mtl(mid_gid, mid_adv, flag, true, true, 1, 0), gdef);
                let s = apply_gdef(&build_mtl(mid_gid, mid_adv, flag, true, true, 1, 200), gdef);
                assert_oracle(&z, &s, "น\u{0e34}ก", 1, &format!("mtl flag={flag:#06x} gdef={gname}"));
            }
        }
        for (c0, c1) in [(true, false), (false, true)] {
            let z = build_mtl(ka_gid, ka_adv, 0, c0, c1, 1, 0);
            let s = build_mtl(ka_gid, ka_adv, 0, c0, c1, 1, 200);
            assert_oracle(&z, &s, "ก\u{0e34}", 1, &format!("mtl nonlig c=({c0},{c1})"));
        }

        // 3. MarkToMark across unligated/CGJ/ligated-cross-comp, flags, GDEF sets, and anchors.
        let build_mkmk = |base: u16, adv: i16, m1: u16, flag: u16, corrupt: Option<bool>, m2_y: i16| {
            let mut gpos = minimal_thai_gpos(base, m1, mai_ek, adv, flag, 1);
            let l0 = u16_at(&gpos, 8) + u16_at(&gpos, u16_at(&gpos, 8) + 2);
            gpos[l0 + 52..l0 + 54].copy_from_slice(&200i16.to_be_bytes());
            let l1 = l0 + 54;
            gpos[l1 + 52..l1 + 54].copy_from_slice(&m2_y.to_be_bytes());
            if let Some(is_corrupt) = corrupt {
                if is_corrupt {
                    gpos[l1 + 48..l1 + 50].copy_from_slice(&9u16.to_be_bytes());
                } else {
                    gpos[l1 + 46..l1 + 48].copy_from_slice(&0u16.to_be_bytes());
                }
            }
            if flag & 0x0010 != 0 {
                gpos[l1 + 6..l1 + 8].copy_from_slice(&10u16.to_be_bytes());
                gpos.splice(l1 + 8..l1 + 8, 0u16.to_be_bytes());
            }
            let gp = with_table(face.bytes, b"GPOS", gpos);
            with_table(&gp, b"GSUB", liga_gsub.clone())
        };
        let gdef_mk_match = build_gdef(&[(ka_gid, 1), (sara_i, 3), (mai_ek, 3)], &[(sara_i, 1), (mai_ek, 1)], &[sara_i, mai_ek]);
        let gdef_mk_nocls_match = build_gdef(&[], &[(sara_i, 1), (mai_ek, 1)], &[sara_i, mai_ek]);
        let gdef_mk_miss = build_gdef(&[(ka_gid, 1), (sara_i, 3), (mai_ek, 3)], &[(sara_i, 1), (mai_ek, 2)], &[unrelated]);
        let gdef_mk_tmiss = build_gdef(&[(ka_gid, 1), (sara_i, 3), (mai_ek, 3)], &[(sara_i, 2), (mai_ek, 1)], &[mai_ek]);
        let gdef_mk_nocls_tmiss = build_gdef(&[], &[(sara_i, 2), (mai_ek, 1)], &[mai_ek]);
        for flag in [0x0000u16, 0x0008, 0x0100, 0x0010] {
            for (gname, gdef) in [
                ("keep", None),
                ("drop", Some(&empty_gdef)),
                ("match", Some(&gdef_mk_match)),
                ("nocls_match", Some(&gdef_mk_nocls_match)),
                ("miss", Some(&gdef_mk_miss)),
                ("target_miss", Some(&gdef_mk_tmiss)),
                ("nocls_tmiss", Some(&gdef_mk_nocls_tmiss)),
            ] {
                let z = apply_gdef(&build_mkmk(ka_gid, ka_adv, sara_i, flag, None, 0), gdef);
                let s = apply_gdef(&build_mkmk(ka_gid, ka_adv, sara_i, flag, None, 200), gdef);
                assert_oracle(&z, &s, "ก\u{0e34}\u{0e48}", 2, &format!("mkmk flag={flag:#06x} gdef={gname}"));
            }
        }
        for (cname, cval) in [("null", Some(false)), ("corrupt", Some(true))] {
            let z = build_mkmk(ka_gid, ka_adv, sara_i, 0, cval, 0);
            let s = build_mkmk(ka_gid, ka_adv, sara_i, 0, cval, 200);
            assert_oracle(&z, &s, "ก\u{0e34}\u{0e48}", 2, &format!("mkmk anchor={cname}"));
        }
        let z_cgj = build_mkmk(ka_gid, ka_adv, sara_i, 0, None, 0);
        let s_cgj = build_mkmk(ka_gid, ka_adv, sara_i, 0, None, 200);
        assert_oracle(&z_cgj, &s_cgj, "ก\u{0e34}\u{034f}\u{0e48}", 2, "mkmk cgj");
        let z_lig = build_mkmk(mid_gid, mid_adv, sara_ii, 0, None, 0);
        let s_lig = build_mkmk(mid_gid, mid_adv, sara_ii, 0, None, 200);
        assert_oracle(&z_lig, &s_lig, "น\u{0e35}ก\u{0e48}", 2, "mkmk cross-component lig");
    }

    /// Thai runs followed by non-Thai combining marks keep those combining marks attached to the
    /// preceding Thai segment rather than splitting mid-cluster (`I2`).
    #[test]
    fn thai_followed_by_non_thai_combining_mark_stays_in_thai_segment() {
        assert_eq!(split_thai_scripts("ก\u{0301} ab\u{0301}"), vec!["ก\u{0301} ", "ab\u{0301}"]);
        assert_eq!(split_thai_scripts("ab\u{0301} ก\u{0301}"), vec!["ab\u{0301} ", "ก\u{0301}"]);
    }
}
