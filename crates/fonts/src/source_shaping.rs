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
        let mut indices: Vec<u16> = marks
            .iter()
            .filter_map(|r| r.feature(features.offset_data()).ok())
            .flat_map(|f| f.lookup_list_indices().iter().map(|i| i.get()).collect::<Vec<_>>())
            .collect();
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
        if let Ok(sub) = sub {
            rewrites.push(map(sub));
        }
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
            if s.coverage().is_ok_and(|c| c.get(gid).is_some()) {
                let target = u32::from((gid.to_u32() as i32 + i32::from(s.delta_glyph_id())) as u16);
                push_lig_target(target, work, ligatures, queue)?;
            }
        }
        LigRewrite::Single(SingleSubst::Format2(s)) => {
            if let Ok(cov) = s.coverage()
                && let Some(idx) = cov.get(gid)
                && let Some(sub) = s.substitute_glyph_ids().get(usize::from(idx))
            {
                push_lig_target(sub.get().to_u32(), work, ligatures, queue)?;
            }
        }
        LigRewrite::Multiple(s) => {
            if let Ok(cov) = s.coverage()
                && let Some(idx) = cov.get(gid)
                && let Ok(seq) = s.sequences().get(usize::from(idx))
            {
                for sub in seq.substitute_glyph_ids() {
                    push_lig_target(sub.get().to_u32(), work, ligatures, queue)?;
                }
            }
        }
        LigRewrite::Alternate(s) => {
            if let Ok(cov) = s.coverage()
                && let Some(idx) = cov.get(gid)
                && let Ok(set) = s.alternate_sets().get(usize::from(idx))
            {
                for alt in set.alternate_glyph_ids() {
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
        let Ok(lookup) = lookup else { continue };
        match lookup.subtables() {
            Ok(SubstitutionSubtables::Ligature(subs)) => {
                for sub in subs.iter() {
                    charge_gsub_work(&mut work)?;
                    let Ok(sub) = sub else { continue };
                    for set in sub.ligature_sets().iter() {
                        charge_gsub_work(&mut work)?;
                        let Ok(set) = set else { continue };
                        for lig in set.ligatures().iter() {
                            charge_gsub_work(&mut work)?;
                            if let Ok(lig) = lig {
                                ligatures.insert(lig.ligature_glyph().to_u32());
                            }
                        }
                    }
                }
            }
            Ok(SubstitutionSubtables::Single(subs)) => collect_rewrites(subs.iter(), LigRewrite::Single, &mut work, &mut rewrites)?,
            Ok(SubstitutionSubtables::Multiple(subs)) => collect_rewrites(subs.iter(), LigRewrite::Multiple, &mut work, &mut rewrites)?,
            Ok(SubstitutionSubtables::Alternate(subs)) => collect_rewrites(subs.iter(), LigRewrite::Alternate, &mut work, &mut rewrites)?,
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
            return self.mark_classes.as_ref().is_some_and(|c| c.get(gid) == class);
        }
        true
    }

    fn allows_current(&self, flag: skrifa::raw::tables::layout::LookupFlag, filtering_set: Option<u16>, gid: GlyphId) -> bool {
        use skrifa::raw::tables::layout::LookupFlag;
        if flag.contains(LookupFlag::IGNORE_MARKS) {
            return false;
        }
        let class = self.classes.as_ref().map(|c| c.get(gid));
        if matches!(class, Some(1)) && flag.contains(LookupFlag::IGNORE_BASE_GLYPHS) {
            return false;
        }
        if matches!(class, Some(2)) && flag.contains(LookupFlag::IGNORE_LIGATURES) {
            return false;
        }
        if class.is_none_or(|c| c == 3) {
            return self.allows_mark_properties(flag, filtering_set, gid);
        }
        true
    }
}

/// Whether `lookup` (mark-to-base or mark-to-mark) covers `mark` and `onto` with valid parsed
/// anchors for both `mark` and `onto` at `mark`'s class.
fn lookup_attaches(lookup: &skrifa::raw::tables::gpos::PositionLookup<'_>, mark: GlyphId, onto: GlyphId) -> bool {
    use skrifa::raw::tables::gpos::PositionSubtables;
    match lookup.subtables() {
        Ok(PositionSubtables::MarkToBase(subtables)) => subtables.iter().flatten().any(|t| {
            let (Ok(marks), Ok(bases), Ok(classes), Ok(anchors)) = (t.mark_coverage(), t.base_coverage(), t.mark_array(), t.base_array()) else {
                return false;
            };
            let (Some(m), Some(b)) = (marks.get(mark), bases.get(onto)) else { return false };
            let Some(record) = classes.mark_records().get(usize::from(m)) else { return false };
            record.mark_anchor(classes.offset_data()).is_ok()
                && anchors
                    .base_records()
                    .get(usize::from(b))
                    .is_ok_and(|r| r.base_anchors(anchors.offset_data()).get(usize::from(record.mark_class())).is_some_and(|a| a.is_ok()))
        }),
        Ok(PositionSubtables::MarkToMark(subtables)) => subtables.iter().flatten().any(|t| {
            let (Ok(marks), Ok(bases), Ok(classes), Ok(anchors)) = (t.mark1_coverage(), t.mark2_coverage(), t.mark1_array(), t.mark2_array()) else {
                return false;
            };
            let (Some(m), Some(b)) = (marks.get(mark), bases.get(onto)) else { return false };
            let Some(record) = classes.mark_records().get(usize::from(m)) else { return false };
            record.mark_anchor(classes.offset_data()).is_ok()
                && anchors
                    .mark2_records()
                    .get(usize::from(b))
                    .is_ok_and(|r| r.mark2_anchors(anchors.offset_data()).get(usize::from(record.mark_class())).is_some_and(|a| a.is_ok()))
        }),
        _ => false,
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

#[derive(Default)]
struct ThaiCluster {
    len: usize,
    placed: usize,
    in_ligature: bool,
    last_base: Option<u32>,
    last_mark: Vec<Option<u32>>,
}

impl ThaiCluster {
    fn reset(&mut self) {
        self.len = 0;
        self.placed = 0;
        self.in_ligature = false;
        self.last_base = None;
        self.last_mark.fill(None);
    }
}

const MAX_THAI_CLUSTER_MARKS: usize = 64;
const MAX_THAI_LOOKUP_WORK: usize = 1 << 22;

struct ThaiValidator<'a> {
    filter: MarkFilter<'a>,
    selected: Vec<(u16, Option<usize>, skrifa::raw::tables::gpos::PositionLookup<'a>)>,
    mkmk_slots: usize,
    cache: std::collections::HashMap<(u16, u32, u32), bool>,
    work: usize,
}

impl<'a> ThaiValidator<'a> {
    fn new(face: &FontRef<'a>, lookups: &[u16]) -> Option<Self> {
        use skrifa::raw::tables::gpos::PositionSubtables;
        use skrifa::raw::tables::layout::LookupFlag;
        let gpos = face.gpos().ok()?;
        let list = gpos.lookup_list().ok()?;
        let mut mkmk_slots = 0usize;
        let mut selected = Vec::new();
        for &i in lookups {
            let Ok(lookup) = list.lookups().get(usize::from(i)) else { continue };
            if lookup.lookup_flag().contains(LookupFlag::IGNORE_MARKS) {
                continue;
            }
            match lookup.subtables() {
                Ok(PositionSubtables::MarkToBase(_)) => selected.push((i, None, lookup)),
                Ok(PositionSubtables::MarkToMark(_)) => {
                    selected.push((i, Some(mkmk_slots), lookup));
                    mkmk_slots += 1;
                }
                _ => {}
            }
        }
        Some(Self { filter: MarkFilter::new(face)?, selected, mkmk_slots, cache: std::collections::HashMap::new(), work: 0 })
    }

    fn charge(&mut self, units: usize) -> bool {
        self.work = self.work.saturating_add(units);
        self.work <= MAX_THAI_LOOKUP_WORK
    }

    fn any_selected_attaches(&mut self, entry: &ThaiCluster, mark: GlyphId) -> Option<bool> {
        if !self.charge(self.selected.len()) {
            return None;
        }
        let attached = self.selected.iter().any(|&(idx, mkmk_slot, ref lookup)| {
            if !self.filter.allows_current(lookup.lookup_flag(), lookup.mark_filtering_set(), mark) {
                return false;
            }
            let onto = match mkmk_slot {
                Some(slot) => entry.last_mark.get(slot).copied().flatten(),
                None => entry.last_base,
            };
            let Some(onto) = onto else { return false };
            *self.cache.entry((idx, mark.to_u32(), onto)).or_insert_with(|| lookup_attaches(lookup, mark, GlyphId::new(onto)))
        });
        Some(attached)
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
        for idx in c_start..c_end {
            let (info, on, off) = (&infos[idx], &on_pos[idx], &off_pos[idx]);
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
        let Some(input) = text.get(run.clone()) else { continue };
        let direction = if levels.get(run.start).is_some_and(Level::is_rtl) { Direction::RightToLeft } else { Direction::LeftToRight };
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
    /// A `MarkToBase` lookup flagged `IgnoreBaseGlyphs` excludes GDEF base glyphs (`class 1`) as
    /// attachment targets; when anchor coordinates leave net offset unchanged, it must be refused.
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
        let bytes = with_table(face.bytes, b"GPOS", gpos);
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
}
