use std::sync::Arc;

use super::fallback::{same_font, same_paint};
use super::interp::{
    CachedFont, Carry, Clip, FontKey, LayerScope, Origin, Pen, Shown, TextState, Ts, clip_shows, clip_within, direction, interpret, region, to_frame,
    union,
};
use super::{DecodeBudget, Dict, Document, EditError, HashMap, Matrix, ObjRef, Object, Op, Pieces, Stream, TextLine, parse, serialize_ops};

impl TextLine {
    /// The baseline's height in user space.
    pub fn origin_baseline(&self) -> f64 {
        self.origin.baseline
    }

    /// Paint position of the line's first operator: the index of each `Do` that drew the form it
    /// is in, outermost first, then the operator's index in the page's joined stream or in the
    /// form's. Positions compare in paint order lexicographically (`Iterator::cmp`). A line joins
    /// runs painted at different times, so anything painted after this position may lie on top
    /// of part of it.
    pub fn paint_order(&self) -> impl Iterator<Item = usize> + '_ {
        self.drawn_by.iter().copied().chain(std::iter::once(self.ops.iter().min().copied().unwrap_or(0)))
    }

    /// The runs of text the line joins, in content order: each run's text as its glyphs show it,
    /// the boxes of its words (a part of `ink`), and whether joining put a space between it and
    /// the run before (`text` has one there; runs closer together join with none, so one word
    /// can span runs). None when the line's text was reordered for reading (right-to-left or
    /// visual-order Indic text): then only the line's text as a whole says what it reads.
    pub fn runs(&self) -> impl Iterator<Item = (&str, &[[f64; 4]], bool)> {
        let mut start = 0;
        self.runs.iter().map(move |(text, end, spaced)| {
            let ink = self.ink.get(start..*end).unwrap_or_default();
            start = *end;
            (text.as_str(), ink, *spaced)
        })
    }

    /// Each run's text rendering mode (`Tr`) and whether it is drawn in an `/OCR` marked-content
    /// sequence, in content order: one per run of [`TextLine::runs`], also when the line was
    /// reordered and that lists none.
    pub fn run_modes(&self) -> impl Iterator<Item = (i64, bool)> + '_ {
        self.modes.iter().map(|(mode, ocr, _, _)| (*mode, *ocr))
    }

    /// Each run's paint position, as [`TextLine::paint_order`] gives the line's, in content order:
    /// one per run of [`TextLine::run_modes`]. A run joined to a line can be painted long after the
    /// line starts.
    pub fn run_paint_orders(&self) -> impl Iterator<Item = Vec<usize>> + '_ {
        self.modes.iter().map(|&(_, _, _, op)| self.drawn_by.iter().copied().chain(std::iter::once(op)).collect())
    }

    /// For each line, whether optional content hides each run of [`TextLine::run_modes`].
    /// Evaluate each content stream's shared scopes once for the caller's layer choices.
    pub fn hidden_layer_runs(lines: &[Self], mut hides: impl FnMut(ObjRef) -> bool) -> Vec<Vec<bool>> {
        let mut evaluated = HashMap::new();
        lines
            .iter()
            .map(|line| {
                let scopes = evaluated.entry(line.layer_scopes.as_ptr()).or_insert_with(|| {
                    let mut hidden = Vec::with_capacity(line.layer_scopes.len() + 1);
                    hidden.push(false);
                    for scope in line.layer_scopes.iter() {
                        hidden.push(match *scope {
                            LayerScope::Nested { parent, layer } => hidden[parent] || hides(layer),
                            LayerScope::Replaced { first, other } => hidden[first] && hidden[other],
                        });
                    }
                    hidden
                });
                line.modes.iter().map(|(_, _, index, _)| scopes[*index]).collect()
            })
            .collect()
    }

    /// The fill and stroke opacity (`ca` and `CA`) the line is drawn at: one for all its runs,
    /// which join only when they agree.
    pub fn opacity(&self) -> (f64, f64) {
        self.paint.opacity()
    }

    pub(crate) fn rotation(&self) -> f64 {
        let m = Matrix(self.origin.tm).then(&Matrix(self.origin.ctm)).0;
        m[1].atan2(m[0]).to_degrees()
    }

    /// The unit direction its baseline runs in, in user space.
    pub(super) fn direction(&self) -> (f64, f64) {
        direction(&Matrix(self.origin.tm).then(&Matrix(self.origin.ctm)))
    }

    /// Whether it reads left to right along the page's x axis (most text; a page drawn
    /// turned a quarter, as landscape pages often are, isn't).
    pub(crate) fn upright(&self) -> bool {
        let (x, y) = self.direction();
        (x - 1.0).abs() < 1e-3 && y.abs() < 1e-3
    }

    /// Where it starts as `(along, across)` its baseline: user space turned to the text's direction.
    pub(crate) fn frame_origin(&self) -> (f64, f64) {
        to_frame(self.direction(), (self.origin.x, self.origin.baseline))
    }

    /// Its box in the same frame, `[along0, across0, along1, across1]` (for upright text, `rect`).
    pub(crate) fn frame_rect(&self) -> [f64; 4] {
        self.frame
    }
}

/// One entry of a page's `/Contents`, and what it decoded to.
pub(crate) struct ContentStream {
    /// The entry as listed.
    pub(crate) entry: Object,
    /// Its decoded data, empty when it doesn't decode.
    pub(crate) data: Vec<u8>,
    /// How much of it decoded.
    pub(crate) read: Read,
}

/// How much of a content stream decoded ([`rewritten_contents`] says what each allows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Read {
    Whole,
    /// Damaged partway: the data is what decoded before the damage. What follows it is unknown,
    /// the graphics state and syntax the later streams start in too.
    Damaged,
    /// Not at all (a filter this reader lacks, say): the data is empty.
    Unread,
}

/// The page's content streams as listed, each with its decoded data, within one page's
/// [`DecodeBudget`]. An error when the content runs past it: reading or rewriting a page from
/// part of its content would lose the rest.
pub(crate) fn content_streams(doc: &Document, page: &Dict) -> Result<Vec<ContentStream>, EditError> {
    content_streams_within(doc, page, &mut DecodeBudget::page())
}

/// [`content_streams`], decoded against `budget`, which a walk of the page's forms then goes on
/// spending.
pub(crate) fn content_streams_within(doc: &Document, page: &Dict, budget: &mut DecodeBudget) -> Result<Vec<ContentStream>, EditError> {
    let list: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    let mut out = Vec::with_capacity(list.len());
    for entry in list {
        let (data, read) = match &*doc.resolve(&entry) {
            Object::Stream(s) => match budget.decode_with_damage(doc, s, usize::MAX) {
                Ok((data, damaged)) => (data, if damaged { Read::Damaged } else { Read::Whole }),
                Err(e) if budget.remaining() == 0 => return Err(EditError::Invalid(format!("the page content is too large to read: {e}"))),
                Err(_) => (Vec::new(), Read::Unread),
            },
            // Nothing to lose: a null or missing entry draws nothing.
            _ => (Vec::new(), Read::Whole),
        };
        out.push(ContentStream { entry, data, read });
    }
    Ok(out)
}

/// What a text-showing operator does besides showing text (ISO 32000-2 §9.4.3, Table 107): `'`
/// moves to the next line (`T*`), and `"` first sets the word and character spacing (`aw Tw`,
/// `ac Tc`). Text after it relies on both, so a dropped `'` or `"` leaves these in its place.
pub(crate) fn show_side_effects(op: &Op) -> Vec<Op> {
    let mut out = Vec::new();
    if op.is("\"") {
        out.extend(op.num(0).map(|aw| Op::new("Tw", vec![pdfcraft_content::num(aw)])));
        out.extend(op.num(1).map(|ac| Op::new("Tc", vec![pdfcraft_content::num(ac)])));
    }
    if op.is("'") || op.is("\"") {
        out.push(Op::new("T*", vec![]));
    }
    out
}

/// For a paragraph or region rewrite: what dropped operator `i` must leave behind
/// ([`show_side_effects`]), so the text after it keeps its line and spacing. `split_at` is the
/// operator the text object was split at: the rewrite's `Tm` and state there already include
/// that operator's effects.
pub(crate) fn dropped_side_effects(ops: &[Op], i: usize, keep: bool, split_at: Option<usize>) -> Vec<Op> {
    if keep || split_at == Some(i) {
        return Vec::new();
    }
    ops.get(i).map(show_side_effects).unwrap_or_default()
}

/// For a paragraph or region rewrite that splits a text object: what reopens it after the
/// rewritten paragraph, so the text after it starts where it did. `tlm` and `tm` are the line and
/// text matrices where the paragraph began, and `size` and `scale` the font size (`Tf`) and
/// horizontal scaling (`Tz` / 100) in force there. A `Tm` restores the line matrix, which a later
/// `Td` or `T*` moves from; a kept run before the paragraph on the same baseline had already
/// advanced the pen beyond it (ISO 32000-2 §9.4.4), so a numeric `TJ` then moves the text matrix
/// alone by that advance. Only an advance along a horizontal baseline is expressible that way;
/// one that isn't (a vertical run's, or one at a zero size) leaves the pen at the line matrix.
pub(crate) fn reopen_text_object(tlm: [f64; 6], tm: [f64; 6], size: Option<f64>, scale: f64) -> Vec<Op> {
    let n = pdfcraft_content::num;
    let mut out = vec![Op::new("BT", vec![]), Op::new("Tm", tlm.iter().map(|v| n(*v)).collect())];
    // tm = [1 0 0 1 tx ty] x tlm: solve for the advance (tx, ty) in text space.
    let (dx, dy) = (tm[4] - tlm[4], tm[5] - tlm[5]);
    let det = tlm[0] * tlm[3] - tlm[1] * tlm[2];
    let tx = (dx * tlm[3] - dy * tlm[2]) / det;
    let ty = (dy * tlm[0] - dx * tlm[1]) / det;
    let denominator = size.unwrap_or(0.0) * scale;
    let adjustment = -1000.0 * tx / denominator;
    if tx.is_finite() && ty.is_finite() && tx.abs() > 1e-9 && ty.abs() <= 1e-9 && denominator != 0.0 && adjustment.is_finite() {
        out.push(Op::new("TJ", vec![Object::Array(vec![n(adjustment)])]));
    }
    out
}

/// For a paragraph or region rewrite: a numeric `TJ` per dropped show operator in `drop`, moving
/// the text matrix as far as that operator's text did without drawing anything (as
/// [`super::clear_block`] does). Text shown later in the same text object without its own `Td`
/// or `Tm`, such as a run in another style or column on the same baseline, then stays where it
/// was. Only an advance such a run relies on is kept: none when the text object ends, or its next
/// kept run starts a line of its own (`Td`, `TD`, `Tm`, `T*`, `'`, `"`), first. An advance a `TJ`
/// can't express (spacing at a zero font size) is left out. `resources` is the page's resource
/// dictionary. This reads the glyphs without `/ActualText`, which can come to more text than the
/// reading the edit was laid out from; a page whose text that way exceeds the page's text budget
/// keeps no advances rather than refusing an edit the reading allowed.
pub(crate) fn dropped_advances(doc: &Document, ops: &[Op], resources: &Dict, drop: &std::collections::HashSet<usize>) -> HashMap<usize, Op> {
    dropped_advances_within(doc, ops, resources, drop, None)
}

/// [`dropped_advances`] with `text_left` in place of the page's text budget (tests make it small).
pub(crate) fn dropped_advances_within(
    doc: &Document,
    ops: &[Op],
    resources: &Dict,
    drop: &std::collections::HashSet<usize>,
    text_left: Option<usize>,
) -> HashMap<usize, Op> {
    let mut out = HashMap::new();
    let mut carry = Carry::new();
    if let Some(left) = text_left {
        carry.text_left = left;
    }
    let follows = kept_run_follows(ops, drop);
    let Ok(shown) = interpret(doc, ops, resources, &mut HashMap::new(), &mut carry, None, None) else {
        return out;
    };
    for s in shown {
        if !drop.contains(&s.op) || s.advance == 0.0 || !follows.get(s.op).copied().unwrap_or(false) {
            continue;
        }
        let denominator = s.state.size * s.state.scale;
        let adjustment = -1000.0 * s.advance / denominator;
        if denominator != 0.0 && adjustment.is_finite() {
            out.entry(s.op).or_insert_with(|| Op::new("TJ", vec![Object::Array(vec![pdfcraft_content::num(adjustment)])]));
        }
    }
    out
}

/// For each operator: whether a run shown after it in its text object, and not in `drop`, is
/// placed by the pen it left rather than by a line move of its own (see [`dropped_advances`]).
/// A `Q` puts back the text matrices its `q` saved (as [`interpret`] reads it), so a line move
/// between them doesn't count for the text after the `Q`. Leaving an advance in is harmless where
/// nothing turns out to use it. One pass from the end, so a page of many dropped runs stays linear.
fn kept_run_follows(ops: &[Op], drop: &std::collections::HashSet<usize>) -> Vec<bool> {
    // The `q` and `Q` that pair up: [`interpret`] ignores a `Q` with nothing saved, so only a
    // `Q` that restores something is a boundary here.
    let mut paired = vec![false; ops.len()];
    let mut open: Vec<usize> = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        if op.is("q") {
            open.push(i);
        } else if op.is("Q")
            && let Some(q) = open.pop()
        {
            for at in [q, i] {
                if let Some(slot) = paired.get_mut(at) {
                    *slot = true;
                }
            }
        }
    }
    let mut out = vec![false; ops.len()];
    let mut follows = false;
    // What followed each `Q` still open (read backwards), restored at its `q`.
    let mut after_restore: Vec<bool> = Vec::new();
    for (i, op) in ops.iter().enumerate().rev() {
        if let Some(slot) = out.get_mut(i) {
            *slot = follows;
        }
        let pairs = paired.get(i).copied().unwrap_or(false);
        match op.op.as_slice() {
            b"Q" if pairs => {
                after_restore.push(follows);
                follows = false;
            }
            b"q" if pairs => follows = follows || after_restore.pop().unwrap_or(false),
            b"ET" | b"BT" | b"Td" | b"TD" | b"Tm" | b"T*" | b"'" | b"\"" => follows = false,
            b"Tj" | b"TJ" if !drop.contains(&i) => follows = true,
            _ => {}
        }
    }
    out
}

/// The page's streams joined into the one stream they are: an operator may span two of them.
pub(crate) fn joined(streams: &[ContentStream]) -> Pieces {
    Pieces::join(&streams.iter().map(|s| s.data.as_slice()).collect::<Vec<_>>())
}

/// Rewrite a page's content (its streams joined, see [`Pieces`]): `edit(i)` gives the operators
/// to insert before operator `i` of `ops` (parsed from the joined pieces) and whether to keep it.
/// Everything else is copied byte for byte. A page's streams are one stream in pieces, split
/// between any two tokens (ISO 32000-2 §7.8.2), so an operator's operands can end one piece and
/// its keyword start the next; removing or replacing it rewrites every piece it spans. Returns
/// each piece's new bytes.
pub(crate) fn splice(pieces: &Pieces, ops: &[Op], mut edit: impl FnMut(usize) -> (Vec<Op>, bool)) -> Vec<Vec<u8>> {
    let data = pieces.data();
    let mut edits = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let (insert, keep) = edit(i);
        if insert.is_empty() && keep {
            continue;
        }
        let mut bytes = Vec::new();
        if !insert.is_empty() {
            if op.span.start.checked_sub(1).and_then(|p| data.get(p)).is_some_and(|b| !b.is_ascii_whitespace()) {
                bytes.push(b'\n');
            }
            bytes.extend_from_slice(&serialize_ops(&insert));
        }
        let end = if keep { op.span.start } else { op.span.end };
        edits.push((op.span.start..end, bytes));
    }
    pieces.splice(edits)
}

/// An error when content at `/Contents` index `at` (a stream there, or one inserted there) is drawn
/// after a stream not read whole, damaged partway or not decoding at all: what wasn't read can
/// change how it reads, the graphics state it draws in among it.
pub(crate) fn drawn_before_damage(streams: &[ContentStream], at: usize) -> Result<(), EditError> {
    if at > first_damaged(streams) {
        return Err(EditError::Invalid(
            "this change comes after a content stream that is damaged or can't be decoded, and what wasn't read can change how the rest of the page reads"
                .into(),
        ));
    }
    Ok(())
}

/// The index of the first stream not read whole, or the number of streams.
fn first_damaged(streams: &[ContentStream]) -> usize {
    streams.iter().position(|s| s.read != Read::Whole).unwrap_or(streams.len())
}

/// The index of the first of `ops`, parsed from `pieces` (the [`joined`] `streams`), drawn after a
/// stream not read whole, or `ops.len()`. A `q … Q` still open there may hold what wasn't read,
/// painting in the state it sets. An operator counts by its keyword: one whose operands start
/// before such a stream takes them from around what wasn't read.
pub(crate) fn first_op_after_damage(streams: &[ContentStream], pieces: &Pieces, ops: &[Op]) -> usize {
    let damaged = first_damaged(streams);
    ops.iter().position(|op| pieces.pieces_of(op).1 > damaged).unwrap_or(ops.len())
}

/// Whether `new` changes a text object still open at the first stream not read whole. The text
/// object goes on in it, and the text it shows there starts where this text ends: a change of this
/// text moves it. A change before the text object doesn't.
fn changes_text_open_into_damage(streams: &[ContentStream], new: &[Vec<u8>]) -> bool {
    let upto = first_damaged(streams).saturating_add(1);
    if upto > streams.len() {
        return false;
    }
    let before = Pieces::join(&streams.iter().take(upto).map(|s| s.data.as_slice()).collect::<Vec<_>>());
    let mut open = None;
    for op in before.parse().ops {
        match op.op.as_slice() {
            b"BT" => open = Some(op.span.start),
            b"ET" => open = None,
            _ => {}
        }
    }
    let Some(at) = open else { return false };
    let after = Pieces::join(&new.iter().take(upto).map(Vec::as_slice).collect::<Vec<_>>());
    // Unchanged from the text object's start to the stream not read whole: it ends the same.
    !before.data().get(at..).is_some_and(|text| after.data().ends_with(text))
}

/// The page's `/Contents` after a [`splice`]: pieces whose bytes changed become new streams
/// (keeping their dictionaries), the others stay as they were. An error, before anything is
/// added, when a piece that changed is a stream that didn't decode whole, or comes after one.
/// Written from what decoded, a damaged stream would lose everything after the damage, and one
/// that doesn't decode all of it. What follows the damage is unknown, so a later stream may not
/// read as it seems: a string or comment the damage left open runs on into it, and operators the
/// damage hides can change the graphics state it draws in. A stream that doesn't decode hides its
/// operators all the same.
pub(crate) fn rewritten_contents(doc: &mut Document, streams: &[ContentStream], new: Vec<Vec<u8>>) -> Result<Object, EditError> {
    // The last stream that changed: when it is drawn before the damage, so are the others.
    if let Some(i) = streams.iter().zip(&new).rposition(|(s, data)| s.data != *data) {
        drawn_before_damage(streams, i)?;
    }
    if changes_text_open_into_damage(streams, &new) {
        return Err(EditError::Invalid(
            "this change is in a text object that goes on in a content stream that is damaged or can't be decoded, whose text starts where this text ends"
                .into(),
        ));
    }
    for (s, data) in streams.iter().zip(&new) {
        if s.data == *data {
            continue;
        }
        match s.read {
            Read::Whole => {}
            Read::Damaged => {
                return Err(EditError::Invalid(
                    "this change is in a content stream that is damaged partway, and rewriting it would lose what follows the damage".into(),
                ));
            }
            Read::Unread => {
                return Err(EditError::Invalid("this change is in a content stream that can't be decoded, and rewriting it would lose it".into()));
            }
        }
    }
    let mut contents: Vec<Object> = Vec::with_capacity(streams.len());
    for (s, data) in streams.iter().zip(new) {
        if s.data == data {
            contents.push(s.entry.clone());
            continue;
        }
        let mut dict = match &*doc.resolve(&s.entry) {
            Object::Stream(s) => s.dict.clone(),
            _ => Dict::new(),
        };
        dict.remove(b"Length");
        contents.push(Object::Ref(doc.add(Object::Stream(Stream::flate(dict, &data)))));
    }
    Ok(match <[Object; 1]>::try_from(contents) {
        Ok([one]) => one,
        Err(many) => Object::Array(many),
    })
}

pub(crate) fn page_dict(doc: &Document, page: usize) -> Result<pdfcraft_model::Page, EditError> {
    pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))
}

fn fill_color(fill: &[Op]) -> [f64; 3] {
    let Some(op) = fill.iter().rev().find(|op| matches!(op.op.as_slice(), b"g" | b"rg" | b"k" | b"sc" | b"scn")) else {
        return [0.0, 0.0, 0.0];
    };
    let space = match op.op.as_slice() {
        b"sc" | b"scn" => match op.operands.len() {
            1 => b"g".as_slice(),
            3 => b"rg".as_slice(),
            4 => b"k".as_slice(),
            _ => return [0.0; 3],
        },
        other => other,
    };
    match space {
        b"g" => {
            let v = op.operands.first().and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0);
            [v, v, v]
        }
        b"rg" => [
            op.operands.first().and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0),
            op.operands.get(1).and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0),
            op.operands.get(2).and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0),
        ],
        b"k" => {
            let c = op.operands.first().and_then(Object::as_f64).unwrap_or(0.0);
            let m = op.operands.get(1).and_then(Object::as_f64).unwrap_or(0.0);
            let y = op.operands.get(2).and_then(Object::as_f64).unwrap_or(0.0);
            let k = op.operands.get(3).and_then(Object::as_f64).unwrap_or(0.0);
            [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]
        }
        _ => [0.0, 0.0, 0.0],
    }
}

/// The lines of text on a page (0-based), in content order.
pub fn text_lines(doc: &Document, page: usize) -> Result<Vec<TextLine>, EditError> {
    Ok(text_lines_in(doc, &page_dict(doc, page)?, Reading::Editor { invisible_apart: false })?.lines)
}

/// [`text_lines`] with every show of invisible text a line of its own, however alike its
/// neighbours: an OCR layer's words, which an edit can keep or change one by one.
pub(crate) fn word_lines(doc: &Document, page: usize) -> Result<Vec<TextLine>, EditError> {
    Ok(text_lines_in(doc, &page_dict(doc, page)?, Reading::Editor { invisible_apart: true })?.lines)
}

/// What a page's glyphs show, for checks that must not trust its marked-content metadata (the
/// redaction audit): see [`glyph_text`].
pub struct GlyphText {
    /// The page's own lines, then those of the forms it draws, in content order.
    pub lines: Vec<TextLine>,
    /// Its forward `TJ` adjustments that paint no glyphs, when asked for.
    pub gaps: Vec<GlyphAdvanceGap>,
    /// Some form's text is missing from `lines`: a form drawn inside itself, one whose content
    /// doesn't decode cleanly, or forms past [`MAX_FORM_DEPTH`], [`MAX_FORM_VISITS`] or the page's decode
    /// budget.
    pub unread_forms: bool,
}

/// A forward text adjustment that painted no glyphs, in page user space.
pub struct GlyphAdvanceGap {
    /// Where it is painted, as [`TextLine::paint_order`] gives it for a line.
    pub paint_pos: Vec<usize>,
    pub from: (f64, f64),
    pub to: (f64, f64),
}

/// The lines `page`'s glyphs show, in content order, the text of the form XObjects it draws
/// included, without `/ActualText` replacements: what the page draws, not what its marked-content
/// metadata says the glyphs mean. With `gaps`, its large numeric `TJ` adjustments too. Takes the
/// page itself, so a caller going through every page doesn't walk the page tree again for each.
pub fn glyph_text(doc: &Document, page: &pdfcraft_model::Page, gaps: bool) -> Result<GlyphText, EditError> {
    text_lines_in(doc, page, Reading::Glyphs { gaps })
}

/// How [`text_lines_in`] reads a page.
#[derive(Clone, Copy, PartialEq)]
enum Reading {
    /// The page's own content, with `/ActualText` replacing the glyphs it spans: what the editor
    /// can rewrite. With `invisible_apart`, no show of invisible text joins the one before it.
    Editor { invisible_apart: bool },
    /// The glyphs alone, those its forms draw included, with or without the advance gaps.
    Glyphs { gaps: bool },
}

/// The lines of `p`, read as `reading` says.
fn text_lines_in(doc: &Document, p: &pdfcraft_model::Page, reading: Reading) -> Result<GlyphText, EditError> {
    let glyphs = matches!(reading, Reading::Glyphs { .. });
    let want_gaps = reading == Reading::Glyphs { gaps: true };
    let invisible_apart = reading == Reading::Editor { invisible_apart: true };
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut cache = HashMap::new();
    let mut lines: Vec<TextLine> = Vec::new();
    let mut carry = Carry::new();
    if want_gaps {
        carry.advance_gaps = Some(Vec::new());
    }
    // The streams are one stream in pieces (§7.8.2), split between any two tokens: parse them
    // joined, so an operator whose operands end one piece still has them.
    let mut decode = DecodeBudget::page();
    let pieces = joined(&content_streams_within(doc, &p.dict, &mut decode)?);
    let ops = pieces.parse().ops;
    let properties = res.get(b"Properties").and_then(|o| doc.dict(o)).unwrap_or_default();
    let mut invoked = Vec::new();
    let shown = interpret(doc, &ops, &res, None, &mut cache, &mut carry, glyphs.then_some(&mut invoked), (!glyphs).then_some(&properties))?;
    let layer_scopes = Arc::<[LayerScope]>::from(carry.layer_scopes);
    let mut gaps: Vec<GlyphAdvanceGap> =
        carry.advance_gaps.into_iter().flatten().map(|gap| GlyphAdvanceGap { paint_pos: vec![gap.op], from: gap.from, to: gap.to }).collect();
    let mut last: Option<(usize, Pen)> = None; // the stream piece and where its last run ended
    for mut s in shown {
        // A single RTL glyph's ActualText (e.g. lam-alef) is an inline ligature, not a
        // separate logical line. Put its letters into visual order before joining the
        // surrounding glyphs; the completed line is recovered to typing order below.
        // Multi-glyph and mixed-direction replacements keep their authoritative order.
        if s.logical_order && s.glyph_count == 1 && s.text.chars().all(crate::bidi::is_rtl) {
            s.text = crate::bidi::to_visual(&s.text);
            s.logical_order = false;
        }
        // A line belongs to one stream piece. Within it, PDF producers often put every word
        // in its own BT/ET: geometry and text state, not object boundaries, define a line.
        let si = ops.get(s.op).map_or(0, |op| pieces.pieces_of(op).0);
        let joins = last.is_some_and(|(piece, pen)| {
            let bt = pen.bt;
            let (near, gap) = pen.continues(&s, if bt == s.bt { 3.0 } else { 1.5 });
            piece == si
                && lines.last().is_some_and(|l| l.scope == s.scope && l.paint == s.paint && l.logical_order == s.logical_order)
                && (bt == s.bt || lines.last().is_some_and(|l| compatible_word(l, &s)))
                && (s.text.trim().is_empty() || lines.last().is_some_and(|l| same_font(l, &s.font, &s.base_font, s.face.as_deref())))
                && (s.text.trim().is_empty()
                    || lines.last().is_some_and(|l| {
                        (s.size - l.size).abs() < 0.01
                            && s.bold == l.bold
                            && s.italic == l.italic
                            && fill_color(&s.state.fill) == l.color
                            && same_paint(l, &s.state)
                    }))
                && near
                // An OCR layer is one text object over the whole scan: its words continue a line
                // only across a word space, not across a table cell or a tab stop.
                && (s.state.render_mode != 3 || (gap < pen.size * 0.6 && !invisible_apart))
        });
        if joins && let Some(l) = lines.last_mut() {
            let gap = last.map_or(0.0, |(_, pen)| pen.continues(&s, 0.0).1);
            let spaced = gap - s.tracking > l.size * s.state.scale.abs() * 0.2 && !l.text.ends_with(' ') && !s.text.starts_with(' ');
            if spaced {
                l.text.push(' ');
            }
            l.text.push_str(&s.text);
            l.rect = [l.rect[0].min(s.rect[0]), l.rect[1].min(s.rect[1]), l.rect[2].max(s.rect[2]), l.rect[3].max(s.rect[3])];
            l.frame = union(l.frame, s.frame);
            l.ink.extend(s.ink.iter().copied());
            l.runs.push((s.text.clone(), l.ink.len(), spaced));
            l.modes.push((s.state.render_mode, s.ocr_layer, s.oc, s.op));
            l.ops.push(s.op);
            l.ops.extend(s.extra_ops.iter().copied());
            l.decodable &= s.decodable;
            l.visual_order |= s.visual_order;
            l.actual_text |= s.actual_text;
            l.clips |= s.state.render_mode >= 4;
            if l.unreadable_mode.is_none() {
                l.unreadable_mode = s.state.unreadable.as_deref().map(str::to_owned);
            }
            // Spaces may use a separate subset font; they do not change the visible style.
        } else {
            lines.push(TextLine {
                text: s.text.clone(),
                rect: s.rect,
                frame: s.frame,
                ink: s.ink.clone(),
                font: String::from_utf8_lossy(&s.font).into_owned(),
                base_font: s.base_font.clone(),
                size: s.size,
                bold: s.bold,
                italic: s.italic,
                color: fill_color(&s.state.fill),
                decodable: s.decodable,
                visual_order: s.visual_order,
                face: s.face.clone(),
                stream: si,
                drawn_by: Vec::new(),
                scope: s.scope,
                paint: s.paint.clone(),
                logical_order: s.logical_order,
                actual_text: s.actual_text,
                clips: s.state.render_mode >= 4,
                unreadable_mode: s.state.unreadable.as_deref().map(str::to_owned),
                runs: vec![(s.text.clone(), s.ink.len(), false)],
                modes: vec![(s.state.render_mode, s.ocr_layer, s.oc, s.op)],
                layer_scopes: Arc::clone(&layer_scopes),
                soft_break: s.soft_break,
                hard_break: s.hard_break,
                soft_gap: s.soft_gap,
                ops: std::iter::once(s.op).chain(s.extra_ops.iter().copied()).collect(),
                origin: Origin {
                    tm: s.tm.0,
                    tlm: s.tlm.0,
                    k: (s.tm.then(&s.state.ctm).0[2].powi(2) + s.tm.then(&s.state.ctm).0[3].powi(2)).sqrt(),
                    ctm: s.state.ctm.0,
                    baseline: s.baseline,
                    x: s.start_x,
                    state: TextState::of(&s.state),
                    bt_op: s.bt_op,
                    bt_state: TextState::of(&s.bt_state),
                },
            });
        }
        last = Some((si, Pen::after(&s)));
    }
    // The forms the page draws, each read from the state at its `Do`, on the page's text budget.
    // Strictly: a form read leniently past corrupt data could lose text a cover hides.
    let mut budget = FormBudget::new(decode);
    budget.strict = true;
    let mut forms = FormWalk::new(doc, pieces.len(), budget, false, want_gaps);
    forms.fonts = cache;
    for (at, name, ts, saved) in invoked {
        forms.enter(&res, None, at, &name, ts, saved, &mut carry.text_left)?;
    }
    lines.append(&mut forms.lines);
    gaps.append(&mut forms.gaps);
    // Content streams draw right-to-left lines in visual order; the editor works in typing order.
    for l in lines.iter_mut().filter(|l| !l.logical_order) {
        // A visual-order Indic font draws pre-base vowel signs before their consonant.
        let shown = l.text.clone();
        if l.visual_order && !l.actual_text {
            l.text = crate::indic::to_logical(&l.text);
        }
        l.text = crate::bidi::to_logical(&l.text);
        // The runs hold the text in drawing order, which no longer reads as the line does.
        if l.text != shown {
            l.runs.clear();
        }
    }
    // Lines of only spaces aren't editable text.
    lines.retain(|l| !l.text.trim().is_empty());
    Ok(GlyphText { lines, gaps, unread_forms: forms.budget.truncated || forms.budget.skipped })
}

/// Joining independent text objects is safe only when they use the same text and graphics
/// transforms and spacing. In particular, a nearby rotated label isn't part of this line.
fn compatible_word(line: &TextLine, shown: &Shown) -> bool {
    let within = |a: &[f64], b: &[f64], tolerance: f64| a.iter().zip(b).all(|(a, b)| (a - b).abs() < tolerance);
    let same = |a: &[f64], b: &[f64]| within(a, b, 1e-6);
    let (a, b) = (&line.origin.state, TextState::of(&shown.state));
    // Spacing that differs by less than a thousandth of an em, the unit glyph widths are given in,
    // can't be seen. Some producers still write it (`0.0001 Tc` on one word of a line).
    let em = shown.state.size.abs() / 1000.0;
    line.scope == shown.scope
        && line.paint == shown.paint
        && same(&line.origin.tm[..4], &shown.tm.0[..4])
        && same(&line.origin.ctm, &shown.state.ctm.0)
        && within(&[a.char_spacing, a.word_spacing], &[b.char_spacing, b.word_spacing], em.max(1e-6))
        && same(&[a.scale, a.rise], &[b.scale, b.rise])
        && a.fill.len() == b.fill.len()
        && a.fill.iter().zip(&b.fill).all(|(a, b)| a.op == b.op && a.operands == b.operands)
}

/// The line `s` continues, or a new one: `last` is the previous run of the same stream
/// (`bt`, baseline, end x, size). `drawn_by` is [`TextLine::drawn_by`] for a new line.
fn add_shown(lines: &mut Vec<TextLine>, last: &mut Option<Pen>, s: Shown, stream: usize, layer_scopes: &Arc<[LayerScope]>, drawn_by: &[usize]) {
    let joins = last.is_some_and(|pen| {
        let bt = pen.bt;
        lines.last().is_some_and(|l| l.scope == s.scope && l.paint == s.paint && l.logical_order == s.logical_order)
            && (bt == s.bt || lines.last().is_some_and(|l| compatible_word(l, &s)))
            && (s.text.trim().is_empty() || lines.last().is_some_and(|l| same_font(l, &s.font, &s.base_font, s.face.as_deref())))
            && (s.text.trim().is_empty()
                || lines.last().is_some_and(|l| {
                    (s.size - l.size).abs() < 0.01
                        && s.bold == l.bold
                        && s.italic == l.italic
                        && fill_color(&s.state.fill) == l.color
                        && same_paint(l, &s.state)
                }))
            && pen.continues(&s, if bt == s.bt { 3.0 } else { 1.5 }).0
    });
    if joins && let Some(l) = lines.last_mut() {
        let gap = last.map_or(0.0, |pen| pen.continues(&s, 0.0).1);
        let spaced = gap - s.tracking > l.size * s.state.scale.abs() * 0.2 && !l.text.ends_with(' ') && !s.text.starts_with(' ');
        if spaced {
            l.text.push(' ');
        }
        l.text.push_str(&s.text);
        l.rect = [l.rect[0].min(s.rect[0]), l.rect[1].min(s.rect[1]), l.rect[2].max(s.rect[2]), l.rect[3].max(s.rect[3])];
        l.frame = union(l.frame, s.frame);
        l.ink.extend(s.ink.iter().copied());
        l.runs.push((s.text.clone(), l.ink.len(), spaced));
        l.modes.push((s.state.render_mode, s.ocr_layer, s.oc, s.op));
        l.ops.push(s.op);
        l.ops.extend(s.extra_ops.iter().copied());
        l.decodable &= s.decodable;
        l.visual_order |= s.visual_order;
        l.actual_text |= s.actual_text;
        l.clips |= s.state.render_mode >= 4;
        if l.unreadable_mode.is_none() {
            l.unreadable_mode = s.state.unreadable.as_deref().map(str::to_owned);
        }
    } else {
        lines.push(TextLine {
            text: s.text.clone(),
            rect: s.rect,
            frame: s.frame,
            ink: s.ink.clone(),
            font: String::from_utf8_lossy(&s.font).into_owned(),
            base_font: s.base_font.clone(),
            size: s.size,
            bold: s.bold,
            italic: s.italic,
            color: fill_color(&s.state.fill),
            decodable: s.decodable,
            visual_order: s.visual_order,
            face: s.face.clone(),
            stream,
            drawn_by: drawn_by.to_vec(),
            scope: s.scope,
            paint: s.paint.clone(),
            logical_order: s.logical_order,
            actual_text: s.actual_text,
            clips: s.state.render_mode >= 4,
            unreadable_mode: s.state.unreadable.as_deref().map(str::to_owned),
            runs: vec![(s.text.clone(), s.ink.len(), false)],
            modes: vec![(s.state.render_mode, s.ocr_layer, s.oc, s.op)],
            layer_scopes: Arc::clone(layer_scopes),
            soft_break: s.soft_break,
            hard_break: s.hard_break,
            soft_gap: s.soft_gap,
            ops: std::iter::once(s.op).chain(s.extra_ops.iter().copied()).collect(),
            origin: Origin {
                tm: s.tm.0,
                tlm: s.tlm.0,
                k: (s.tm.then(&s.state.ctm).0[2].powi(2) + s.tm.then(&s.state.ctm).0[3].powi(2)).sqrt(),
                ctm: s.state.ctm.0,
                baseline: s.baseline,
                x: s.start_x,
                state: TextState::of(&s.state),
                bt_op: s.bt_op,
                bt_state: TextState::of(&s.bt_state),
            },
        });
    }
    *last = Some(Pen::after(&s));
}

/// How deep form XObjects nest before reading stops (they may also refer to themselves). The
/// renderer stops at the same depth, so export reads the text the page shows (#3120).
pub const MAX_FORM_DEPTH: usize = 50;

/// The most form XObjects one page's walk enters, at any depth. The depth cap alone still lets
/// a form that draws another form many times fan out exponentially (100 `Do`s on each of 50
/// levels), which would hang export on a hostile PDF. The renderer paints as many forms nested
/// in forms; this budget also counts the forms the page draws itself, so a page that draws one
/// a million times stays bounded too. A label or table cell drawn as a form a few thousand
/// times is read in full (#3120).
pub const MAX_FORM_VISITS: usize = 50_000;

/// One page's walk through the form XObjects it draws: how many it entered (the page's
/// [`MAX_FORM_VISITS`]), the bytes the page's content and forms may still decode to, and whether
/// a limit left a form unread.
pub(crate) struct FormBudget {
    visits: usize,
    decode: DecodeBudget,
    /// A form was left unread because it nests deeper than [`MAX_FORM_DEPTH`], comes past
    /// [`MAX_FORM_VISITS`] or past the decode budget: what it draws is missing from the walk.
    pub(crate) truncated: bool,
    /// A form was passed over because it is drawn inside itself, or read as empty because its
    /// content doesn't decode. Export goes on without it; the audit can't vouch for what it draws.
    pub(crate) skipped: bool,
    /// Decode forms as [`DecodeBudget::decode_strict`] does, so that corrupt data counts as a
    /// form that doesn't decode rather than as the part read before it.
    pub(crate) strict: bool,
}

impl FormBudget {
    /// The walk of a page's forms, spending what `decode` has left after the page's content.
    pub(crate) fn new(decode: DecodeBudget) -> Self {
        Self { visits: 0, decode, truncated: false, skipped: false, strict: false }
    }
}

/// A form XObject a page draws: its object, content, resources, its matrix composed with the
/// CTM at the `Do`, and the clip its `/BBox` makes (page space, under that same matrix, so a
/// turned form keeps its turned box; a box of no area hides everything) if it has a valid one.
pub(crate) struct FormCall {
    pub(crate) obj: Option<pdfcraft_cos::ObjRef>,
    pub(crate) data: Vec<u8>,
    /// The form, and its `/Resources` when they are an object of their own, which is what
    /// [`FormCall::resources`] reads.
    form: Arc<Object>,
    own: Option<Arc<Object>>,
    pub(crate) ctm: Matrix,
    bbox: Option<Clip>,
}

impl FormCall {
    /// The resources its content is read with: its own, or if it has none, `drawn_with`, the ones
    /// it was drawn with (PDF 1.1 files). Read in place, as a page can draw a form thousands of times:
    /// a copy for each took time in the square of them.
    pub(crate) fn resources<'a>(&'a self, drawn_with: &'a Dict) -> &'a Dict {
        self.own_resources().unwrap_or(drawn_with)
    }

    /// Its own resources, if it has them: see [`FormCall::resources`].
    fn own_resources(&self) -> Option<&Dict> {
        let direct = || match &*self.form {
            Object::Stream(s) => s.dict.get(b"Resources").and_then(Object::as_dict),
            _ => None,
        };
        self.own.as_deref().and_then(Object::as_dict).or_else(direct)
    }
}

/// `o` as a dictionary, read in place for the same reason as [`FormCall::resources`]. `held` keeps it
/// when it is an object of its own. Nothing else is one, and [`Document::resolve`] would copy it.
pub(crate) fn dict_in_place<'a>(doc: &Document, o: &'a Object, held: &'a mut Option<Arc<Object>>) -> Option<&'a Dict> {
    match o {
        Object::Dict(dict) => Some(dict),
        Object::Ref(_) => held.insert(doc.resolve(o)).as_dict(),
        _ => None,
    }
}

/// The dictionary at `key` of `resources`, `/XObject` or `/Font`, say, as each form reads these.
pub(crate) fn resource_dict<'a>(doc: &Document, resources: &'a Dict, key: &[u8], held: &'a mut Option<Arc<Object>>) -> Option<&'a Dict> {
    dict_in_place(doc, resources.get(key)?, held)
}

/// The form XObject `name` in `resources`, drawn with `ctm`. `None` for images, missing names
/// and forms already being read (`path`), and for forms nested too deep or past the page's
/// budget, which `budget` records as truncated; each form entered counts one visit.
pub(crate) fn form_call(
    doc: &Document,
    resources: &Dict,
    name: &[u8],
    ctm: Matrix,
    path: &[pdfcraft_cos::ObjRef],
    budget: &mut FormBudget,
) -> Option<FormCall> {
    let limited = path.len() >= MAX_FORM_DEPTH || budget.visits >= MAX_FORM_VISITS || budget.decode.remaining() == 0;
    // Once a limit has cut the page short, refusing the rest costs nothing more.
    if limited && budget.truncated {
        return None;
    }
    let mut held = None;
    let o = resource_dict(doc, resources, b"XObject", &mut held)?.get(name)?;
    // Only a reference can name a stream, so nothing else needs reading, or copying.
    let obj = Some(o.as_ref()?);
    if obj.is_some_and(|r| path.contains(&r)) {
        budget.skipped = true;
        return None;
    }
    let form = doc.resolve(o);
    let Object::Stream(s) = &*form else { return None };
    // The subtype may be a reference to the name, like any other value.
    if s.dict.get(b"Subtype").map(|t| doc.resolve(t)).as_deref().and_then(Object::as_name) != Some(b"Form") {
        return None;
    }
    // Only a form left unread loses content; an image at the limit is still read by its caller.
    if limited {
        budget.truncated = true;
        return None;
    }
    let m = s.dict.get(b"Matrix").map(|m| doc.resolve(m)).and_then(|m| {
        let a = m.as_array()?;
        let v: Vec<f64> = a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
        <[f64; 6]>::try_from(v).ok().filter(|v| v.iter().all(|x| x.is_finite()))
    });
    let own = s.dict.get(b"Resources").filter(|r| matches!(r, Object::Ref(_))).map(|r| doc.resolve(r));
    // Four finite numbers, or no bound at all: a member that isn't one makes the box unknown, and
    // reading the numbers around it would invent a clip.
    let bbox = s.dict.get(b"BBox").map(|b| doc.resolve(b)).and_then(|b| {
        let values: Vec<f64> = b.as_array()?.iter().map(|x| doc.resolve(x).as_f64().filter(|v| v.is_finite())).collect::<Option<_>>()?;
        <[f64; 4]>::try_from(values).ok()
    });
    let decoded = if budget.strict { budget.decode.decode_strict(s) } else { budget.decode.decode(s) };
    let data = match decoded {
        Ok(data) => data,
        Err(_) if budget.decode.remaining() == 0 => {
            budget.truncated = true;
            return None;
        }
        Err(_) => {
            budget.skipped = true;
            Vec::new()
        }
    };
    budget.visits = budget.visits.saturating_add(1);
    let ctm = m.map_or(ctm, |m| Matrix(m).then(&ctm));
    Some(FormCall { obj, data, form, own, ctm, bbox: bbox.and_then(|b| region(&ctm, b)).map(|r| if r.is_empty() { Vec::new() } else { vec![r] }) })
}

/// A walk through the form XObjects a page draws, collecting the lines of text they show. Lines
/// from a form carry a stream number past the page's streams, one per form drawn.
struct FormWalk<'a> {
    doc: &'a Document,
    lines: Vec<TextLine>,
    gaps: Vec<GlyphAdvanceGap>,
    next_stream: usize,
    path: Vec<pdfcraft_cos::ObjRef>,
    /// The `Do` that drew each form being read, outermost first: [`TextLine::drawn_by`].
    calls: Vec<usize>,
    budget: FormBudget,
    /// The fonts read so far, kept for the whole page: a form drawn thousands of times would
    /// otherwise read its fonts, CMaps and font programs again each time.
    fonts: HashMap<FontKey, CachedFont>,
    /// Read `/ActualText` and named optional content as a reader does, or the glyphs alone.
    logical_text: bool,
    want_gaps: bool,
}

impl<'a> FormWalk<'a> {
    fn new(doc: &'a Document, next_stream: usize, budget: FormBudget, logical_text: bool, want_gaps: bool) -> Self {
        FormWalk {
            doc,
            lines: Vec::new(),
            gaps: Vec::new(),
            next_stream,
            path: Vec::new(),
            calls: Vec::new(),
            budget,
            fonts: HashMap::new(),
            logical_text,
            want_gaps,
        }
    }

    /// `stream` gives the stream number of the operator at an index of `ops`; `resources_of` the
    /// form whose resources `resources` are, `None` for the page's.
    fn walk(
        &mut self,
        ops: &[Op],
        resources: &Dict,
        resources_of: Option<ObjRef>,
        carry: &mut Carry,
        stream: &dyn Fn(usize) -> usize,
    ) -> Result<(), EditError> {
        let mut invoked = Vec::new();
        let (mut held, none) = (None, Dict::new());
        let properties = if self.logical_text { Some(resource_dict(self.doc, resources, b"Properties", &mut held).unwrap_or(&none)) } else { None };
        let shown = interpret(self.doc, ops, resources, resources_of, &mut self.fonts, carry, Some(&mut invoked), properties)?;
        for gap in carry.advance_gaps.take().into_iter().flatten() {
            let paint_pos = self.calls.iter().copied().chain(std::iter::once(gap.op)).collect();
            self.gaps.push(GlyphAdvanceGap { paint_pos, from: gap.from, to: gap.to });
        }
        let layer_scopes = Arc::<[LayerScope]>::from(std::mem::take(&mut carry.layer_scopes));
        let mut last = None;
        let mut last_stream = None;
        let mut forms = invoked.into_iter().peekable();
        for s in shown {
            while let Some((at, name, ts, saved)) = forms.next_if(|f| f.0 < s.op) {
                self.enter(resources, resources_of, at, &name, ts, saved, &mut carry.text_left)?;
                // Text after a form starts a new line.
                last = None;
            }
            // A run that the clips in force (forms' `/BBox`es and content-stream `re`/`W`/`n`
            // clips, intersected) hide is not drawn, so it is not read either (#3503). The glyph
            // reading keeps it: the redaction audit checks what the file holds, not what shows.
            if self.logical_text && s.state.clip.as_ref().is_some_and(|clip| !clip_shows(s.rect, clip)) {
                last = None;
                continue;
            }
            let si = stream(s.op);
            // Text from another stream starts a new line, as when each stream was read alone.
            if last_stream.replace(si).is_some_and(|prev| prev != si) {
                last = None;
            }
            add_shown(&mut self.lines, &mut last, s, si, &layer_scopes, &self.calls);
        }
        for (at, name, ts, saved) in forms {
            self.enter(resources, resources_of, at, &name, ts, saved, &mut carry.text_left)?;
        }
        Ok(())
    }

    /// Read the form `name` that the `Do` at index `at` draws with state `ts` and `saved` graphics
    /// states saved. `text_left`: the page's text budget, shared with the forms the page draws.
    /// `resources_of`: as for [`FormWalk::walk`].
    #[allow(clippy::too_many_arguments)]
    fn enter(
        &mut self,
        resources: &Dict,
        resources_of: Option<ObjRef>,
        at: usize,
        name: &[u8],
        ts: Ts,
        saved: usize,
        text_left: &mut usize,
    ) -> Result<(), EditError> {
        let Some(form) = form_call(self.doc, resources, name, ts.ctm, &self.path, &mut self.budget) else { return Ok(()) };
        let ops = parse(&form.data).ops;
        // The form starts with the graphics state at its `Do`, its own matrix applied.
        // Its content is clipped to its `/BBox` within whatever clips the page had in force at the
        // `Do`. It starts with a stack of its own, so no `Q` of its own can lift that clip.
        let clip = match (&ts.clip, &form.bbox) {
            (Some(outer), Some(own)) => Some(clip_within(outer, own)),
            (outer, own) => outer.clone().or_else(|| own.clone()),
        let advance_gaps = self.want_gaps.then(Vec::new);
        let mut carry =
            Carry { ts: Ts { ctm: form.ctm, clip, ..ts }, text_left: *text_left, advance_gaps, saved_outside: saved, ..Carry::new() };
        let stream = self.next_stream;
        self.next_stream = self.next_stream.saturating_add(1);
        if let Some(r) = form.obj {
            self.path.push(r);
        }
        self.calls.push(at);
        // A form with no resources of its own uses those it was drawn with, and their fonts.
        let (resources, resources_of) = form.own_resources().map_or((resources, resources_of), |own| (own, form.obj));
        self.walk(&ops, resources, resources_of, &mut carry, &|_| stream)?;
        self.calls.pop();
        *text_left = carry.text_left;
        if form.obj.is_some() {
            self.path.pop();
        }
        Ok(())
    }
}

/// The lines a page shows as it reads, including text drawn by form XObjects (which
/// [`text_lines`] leaves out because it can only rewrite the page's own streams). Lines from a
/// form carry a stream number past the page's streams, one per form drawn. The flag is `true`
/// when forms past [`MAX_FORM_DEPTH`] or [`MAX_FORM_VISITS`] were left unread.
pub(super) fn reading_lines(doc: &Document, page: usize) -> Result<(Vec<TextLine>, bool), EditError> {
    let p = page_dict(doc, page)?;
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut decode = DecodeBudget::page();
    let streams = content_streams_within(doc, &p.dict, &mut decode)?;
    let mut w = FormWalk::new(doc, streams.len(), FormBudget::new(decode), true, false);
    let mut carry = Carry::new();
    // Read the page's streams as the one stream they are, in one pass, as rendering and editing
    // do: a comment, an operator's operands or a marked-content sequence (its /ActualText too)
    // can run on from one stream into the next. Text still counts as the stream its operator
    // starts in, which keeps lines from different streams apart.
    let pieces = joined(&streams);
    let ops = pieces.parse().ops;
    let stream_of = |i: usize| ops.get(i).map_or(0, |op| pieces.pieces_of(op).0);
    w.walk(&ops, &res, None, &mut carry, &stream_of)?;
    let mut lines = w.lines;
    // A visual-order Indic font draws pre-base vowel signs before their consonant.
    for l in lines.iter_mut().filter(|l| l.visual_order && !l.actual_text) {
        let shown = l.text.clone();
        l.text = crate::indic::to_logical(&l.text);
        if l.text != shown {
            l.runs.clear();
        }
    }
    lines.retain(|l| !l.text.trim().is_empty());
    Ok((lines, w.budget.truncated))
}
