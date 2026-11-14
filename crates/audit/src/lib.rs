//! Verification tooling: audit a document for potential redaction leaks.
//!
//! [`audit_redactions`] finds *faux redactions* — opaque filled rectangles drawn over text
//! that is still extractable underneath — and redaction annotations that were marked but
//! never applied. [`audit_document`] also reports covered numeric text gaps, earlier
//! revisions and retained metadata. Those signals do not establish that text survived.
//!
//! A rectangle counts as a cover only when it was painted strictly *after* the text it
//! overlaps, and only when it is dark enough against the text's color to make the text
//! unreadable (WCAG contrast below 3.0). Page backgrounds, table shading and highlight
//! marks are therefore not flagged.
//!
//! Detection limits (documented, not hidden): `re` rectangles and single closed `m`/`l`
//! subpaths whose four corners form an axis-aligned rectangle in user space (#1510) are
//! considered when closed by a fill operator — other filled paths, shadings and patterns are
//! not; a rectangle drawn with a translucent ExtGState (`/ca < 1`) is not flagged; opaque
//! filled `/Square` annotations count as covers, but annotation rectangles are compared in
//! unrotated user space, so pages with `/Rotate` may misalign. Covers and text inside form
//! XObjects, nested ones included, are checked like the page's own, in the order they are painted,
//! and so are those of the appearances a viewer paints for the page's annotations, after the page
//! in `/Annots` order.
//!
//! The audit fails closed: any content that cannot be read (an undecodable content stream,
//! malformed annotations, form XObjects nested deeper than 32 or in a cycle, a form whose text
//! can't be read, an appearance without a box, text in a tiling pattern or a Type 3 glyph, which
//! it does not read) is an error, never an empty finding list.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::HashMap;

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, Object};
use pdfcraft_edit::{TextLine, text::GlyphAdvanceGap};

mod cover;

#[cfg(test)]
mod tests;

/// What the audit found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FindingKind {
    /// An opaque filled rectangle covers text that is still extractable.
    CoveredText,
    /// A `/Redact` annotation was placed but never applied: the text is untouched.
    UnappliedMark,
    /// An opaque cover aligns with a numeric text displacement whose width remains readable.
    RetainedAdvance,
}

impl FindingKind {
    /// The stable machine-readable ID for this finding class.
    ///
    /// These strings are the crate's public vocabulary: they appear in CLI output and are
    /// the IDs a future machine-readable format would serialize. Do not rename them;
    /// add a new variant instead.
    pub fn id(self) -> &'static str {
        match self {
            FindingKind::CoveredText => "covered-text",
            FindingKind::UnappliedMark => "unapplied-mark",
            FindingKind::RetainedAdvance => "retained-advance",
        }
    }
}

/// One audit finding.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditFinding {
    /// 0-based page index.
    pub page: usize,
    pub kind: FindingKind,
    /// The covering rectangle (or the mark's rectangle), in page user space, normalized so
    /// `rect[0] <= rect[2]` and `rect[1] <= rect[3]`.
    pub rect: [f64; 4],
    /// The extractable text the cover hides, lines joined with single spaces and trimmed.
    /// Empty when the document's permissions withhold copying (see [`audit_redactions`]).
    pub covered_text: String,
}

/// One document-wide exposure signal. Reports counts without disclosing metadata values.
pub struct DocumentFinding {
    pub kind: &'static str,
    pub count: usize,
}

pub struct AuditReport {
    pub page_findings: Vec<AuditFinding>,
    pub document_findings: Vec<DocumentFinding>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    // The messages number pages from 1, as a reader does; the fields hold the 0-based index.
    #[error("page {}: {}", .0 + 1, .1)]
    Text(usize, String),
    #[error("content walk exceeded the operator bound")]
    TooManyOps,
    #[error("form XObjects nest deeper than the audit follows")]
    TooDeep,
    /// Some text a form draws was not read: the form draws itself, doesn't decode, or comes past
    /// the text reader's form limits. It could be text a cover hides.
    #[error("page {}: the text of a form XObject could not be read", .0 + 1)]
    UnreadForms(usize),
    /// A tiling pattern's cell or a Type 3 font's glyph procedure shows text. The audit doesn't
    /// place what they draw, so it can't say whether a cover hides it.
    #[error("page {}: {} shows text, which the audit does not read", .0 + 1, .1)]
    UncheckedText(usize, &'static str),
    /// An annotation's appearance has no usable `/BBox`, so where it draws can't be worked out.
    #[error("page {}: an annotation's appearance has no usable /BBox", .0 + 1)]
    BadAppearance(usize),
    #[error("content walk entered more form XObjects than the audit follows")]
    TooManyForms,
    #[error("page has a malformed /Annots entry")]
    BadAnnots,
    #[error("page has more covers and text lines than the audit compares")]
    TooManyComparisons,
    #[error("covered text exceeds the audit's report budget")]
    TooMuchOutput,
    #[error("document metadata has an invalid {0} entry")]
    BadMetadata(&'static str),
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
    #[error(transparent)]
    Edit(#[from] pdfcraft_edit::EditError),
}

/// Fraction of a text line's ink (the area of its words), or of its box, that must lie under a
/// cover to count as hidden (see [`covers`]).
const COVER_FRACTION: f64 = 0.4;
/// Fraction of one word's area that must lie under a cover to count as hidden on its own, however
/// long the line around it (see [`covers`]).
const WORD_FRACTION: f64 = 0.5;
/// WCAG contrast ratio below which the text is unreadable against the cover.
const COVER_CONTRAST: f64 = 3.0;
/// Bytes of a hidden line's text read (copied or trimmed) that cost one comparison: a byte scan
/// is far cheaper than a box comparison, but a long padded line under many covers adds up.
const TEXT_BYTES_PER_COMPARISON: usize = 16;

/// Audit every page for faux redactions and unapplied redaction marks. Fails closed: any
/// content that cannot be decoded, or that needs more work or report than the audit's bounds, is
/// returned as an error, never as an empty finding list.
///
/// When the document's permissions withhold copying (ISO 32000-2 Table 22, bit 5), findings are
/// still reported but their `covered_text` is empty: the audit hands out no text that copying
/// would refuse.
pub fn audit_redactions(doc: &Document) -> Result<Vec<AuditFinding>, AuditError> {
    audit(doc, Limits::DEFAULT)
}

/// Audit page contents, retained text advances, and document-wide exposure signals.
pub fn audit_document(doc: &Document) -> Result<AuditReport, AuditError> {
    let mut document_findings = Vec::new();
    // The xref chain can have multiple sections for one linearized revision. Reuse the
    // byte boundaries, which collapse those sections, instead of counting xrefs.
    let earlier = doc.revision_ends().len().saturating_sub(1);
    if earlier > 0 {
        document_findings.push(DocumentFinding { kind: "earlier-revisions", count: earlier });
    }
    // A metadata entry of the wrong type is reported, not an error, so the pages are still audited.
    match metadata_entry(doc, doc.trailer(), b"Info")?.as_deref() {
        Some(Object::Dict(info)) if !info.is_empty() => document_findings.push(DocumentFinding { kind: "document-info", count: info.len() }),
        Some(Object::Dict(_)) | None => {}
        Some(_) => document_findings.push(DocumentFinding { kind: "malformed-document-info", count: 1 }),
    }
    let root = doc.root().ok_or(AuditError::BadMetadata("catalogue"))?;
    let root = doc.get(root);
    let Object::Dict(catalog) = &*root else { return Err(AuditError::BadMetadata("catalogue")) };
    match metadata_entry(doc, catalog, b"Metadata")?.as_deref() {
        Some(Object::Stream(_)) => document_findings.push(DocumentFinding { kind: "xmp-metadata", count: 1 }),
        None => {}
        Some(_) => document_findings.push(DocumentFinding { kind: "malformed-xmp-metadata", count: 1 }),
    }
    Ok(AuditReport { page_findings: audit_pages(doc, Limits::DEFAULT, true)?, document_findings })
}

fn metadata_entry(doc: &Document, dict: &Dict, key: &[u8]) -> Result<Option<std::sync::Arc<Object>>, AuditError> {
    let Some(entry) = dict.get(key) else { return Ok(None) };
    let value = doc.try_resolve(entry)?;
    Ok((!matches!(&*value, Object::Null)).then_some(value))
}

/// Work and output bounds for one audit.
#[derive(Clone, Copy)]
struct Limits {
    /// Cover-against-line comparisons per page, counting a line once per word box it has (at
    /// least once), since [`covers`] walks them.
    comparisons: usize,
    /// Bytes of covered text the findings report.
    covered_text: usize,
}

impl Limits {
    /// Every cover is compared with every line of its page, and each finding repeats the text
    /// it covers, so both grow with the square of the content: a few megabytes of covers over
    /// one long line would otherwise take hours and gigabytes.
    const DEFAULT: Limits = Limits { comparisons: 50_000_000, covered_text: 64 << 20 };
}

/// The work and report the audit has spent against its [`Limits`].
struct Budget {
    limits: Limits,
    comparisons: usize,
    /// What comparing one cover with every line of the current page costs.
    page_cost: usize,
    covered_text: usize,
}

impl Budget {
    /// Resets the comparison count for a page with these `lines`. A line costs its word boxes
    /// or its runs, whichever are more (at least one): a cover check walks the boxes, and the
    /// covered text walks the runs, which can draw no words at all (`( ) Tj`).
    fn start_page(&mut self, lines: &[Painted]) {
        self.comparisons = 0;
        self.page_cost = lines.iter().fold(0, |n, Painted { line, .. }| n.saturating_add(line.ink.len().max(line.runs().count()).max(1)));
    }

    /// The text `cover` hides in the `lines` that `hides` ([`covered_words`]), joined with single
    /// spaces, or `None` when they show no text. Without `copying` the text is empty and never
    /// assembled: one long line under many covers would otherwise be copied once per cover,
    /// outside both bounds.
    fn hidden_text(
        &mut self,
        lines: &[Painted],
        copying: bool,
        cover: &[f64; 4],
        hides: impl Fn(&Painted) -> bool,
    ) -> Result<Option<String>, AuditError> {
        self.spend(self.page_cost)?;
        if !copying {
            let mut hidden = false;
            for Painted { line, .. } in lines.iter().filter(|p| hides(p)) {
                // Trimming walks the line's text (padding and all): it costs its bytes.
                self.spend(line.text.len() / TEXT_BYTES_PER_COMPARISON)?;
                if !line.text.trim().is_empty() {
                    hidden = true;
                    break;
                }
            }
            return Ok(hidden.then(String::new));
        }
        let mut covered = String::new();
        for Painted { line, .. } in lines.iter().filter(|p| hides(p)) {
            // Copying and trimming the line walks its text once per cover: a long padded
            // line under many covers would otherwise be read far more than its runs suggest.
            self.spend(line.text.len() / TEXT_BYTES_PER_COMPARISON)?;
            if !covered.is_empty() {
                covered.push(' ');
            }
            covered_words(cover, line, &mut covered);
        }
        Ok((!covered.trim().is_empty()).then_some(covered))
    }

    /// `n` units of work done against the comparison bound.
    fn spend(&mut self, n: usize) -> Result<(), AuditError> {
        self.comparisons = self.comparisons.saturating_add(n);
        if self.comparisons > self.limits.comparisons {
            return Err(AuditError::TooManyComparisons);
        }
        Ok(())
    }

    /// The text a finding reports for `covered`: none when copying is withheld. Charges what it
    /// reports, and the finding's own record, against the output bound: a finding with no text
    /// (copying withheld, or a shared annotation list on many pages) still takes memory.
    fn report_text(&mut self, covered: String, copying: bool) -> Result<String, AuditError> {
        let reported = if copying { covered } else { String::new() };
        self.covered_text = self.covered_text.saturating_add(reported.len()).saturating_add(std::mem::size_of::<AuditFinding>());
        if self.covered_text > self.limits.covered_text {
            return Err(AuditError::TooMuchOutput);
        }
        Ok(reported)
    }
}

fn audit(doc: &Document, limits: Limits) -> Result<Vec<AuditFinding>, AuditError> {
    audit_pages(doc, limits, false)
}

fn audit_pages(doc: &Document, limits: Limits, report_advances: bool) -> Result<Vec<AuditFinding>, AuditError> {
    let copying = doc.permissions().is_none_or(|p| p.copy());
    let mut budget = Budget { limits, comparisons: 0, page_cost: 0, covered_text: 0 };
    let mut out = Vec::new();
    for (n, page) in pdfcraft_model::pages(doc).iter().enumerate() {
        let annotations = annots(doc, &page.dict, n)?;
        let appearances: Vec<&cover::Appearance> = annotations.iter().filter_map(|a| a.appearance.as_ref()).collect();
        // The cover walk first: it reads one operator at a time within the page's operator bound
        // and decode budget, shared by the content, its forms and its annotations' appearances.
        // The text walk holds a stream's operators all at once, and reads each appearance with a
        // budget of its own: run first, a stream of tiny operators or an appearance listed many
        // times would cost it far more than the bounds allow.
        let rects = cover::filled_rects(doc, &page.dict, n, &appearances)?;
        let (mut lines, mut gaps) = painted_text(doc, page, n, 0, None, report_advances)?;
        // Each read has a text budget of its own; the page's lines in all are held to one.
        let mut text_bytes = text_len(&lines);
        for a in &appearances {
            let Object::Stream(form) = &*a.form else { return Err(AuditError::BadAppearance(n)) };
            // Read as a page whose content is the appearance's form, then mapped onto the page. One
            // without resources of its own reads the page's, as the cover walk does.
            let mut dict = Dict::new();
            dict.set(b"Contents".to_vec(), a.contents.clone());
            if let Some(resources) = form.dict.get(b"Resources").or_else(|| page.dict.get(b"Resources")) {
                dict.set(b"Resources".to_vec(), resources.clone());
            }
            let shown = pdfcraft_model::Page { obj: page.obj, dict };
            let (more, more_gaps) = painted_text(doc, &shown, n, a.layer, Some(&a.ctm), report_advances)?;
            text_bytes = text_bytes.saturating_add(text_len(&more));
            if text_bytes > MAX_PAGE_TEXT {
                return Err(AuditError::Text(n, "the page's text is too large to read".into()));
            }
            lines.extend(more);
            gaps.extend(more_gaps);
        }
        budget.start_page(&lines);
        // 1. Opaque filled rectangles over extractable text.
        for r in &rects {
            if let Some(covered) = budget.hidden_text(&lines, copying, &r.rect, |p| is_cover(r, p))? {
                let covered_text = budget.report_text(covered, copying)?;
                out.push(AuditFinding { page: n, kind: FindingKind::CoveredText, rect: r.rect, covered_text });
            }
            if !gaps.is_empty() {
                budget.spend(gaps.len())?;
                if gaps.iter().any(|gap| gap.paint_pos < r.pos && covers_gap(&r.rect, gap)) {
                    let covered_text = budget.report_text(String::new(), copying)?;
                    out.push(AuditFinding { page: n, kind: FindingKind::RetainedAdvance, rect: r.rect, covered_text });
                }
            }
        }
        // 2. Annotation covers over extractable text. A Square is checked by its interior colour,
        // after the page and the annotations before it, for the lines no rectangle its appearance
        // paints hides: that rectangle reports those (above), so a box is one finding. A
        // Redact mark lies over everything but the text of its own appearance, which is not text
        // it marks.
        let mut painted_by: HashMap<usize, Vec<&cover::FilledRect>> = HashMap::new();
        for r in &rects {
            if let Some(&layer) = r.pos.first().filter(|&&layer| layer > 0) {
                painted_by.entry(layer).or_default().push(r);
            }
        }
        for a in &annotations {
            let Some(annot) = &a.cover else { continue };
            let own = match annot {
                AnnotationCover::Square { .. } => painted_by.get(&a.layer).map_or(&[][..], Vec::as_slice),
                AnnotationCover::Redact(_) => &[],
            };
            // Each line is also compared with each of those rectangles.
            budget.spend(budget.page_cost.saturating_mul(own.len()))?;
            let (kind, rect) = match annot {
                AnnotationCover::Redact(rect) => (FindingKind::UnappliedMark, *rect),
                AnnotationCover::Square { rect, .. } => (FindingKind::CoveredText, *rect),
            };
            if let Some(covered) = budget.hidden_text(&lines, copying, &rect, |p| match annot {
                AnnotationCover::Redact(rect) => p.layer != a.layer && covers(rect, &p.line),
                AnnotationCover::Square { rect, rgb } => {
                    p.layer <= a.layer
                        && covers(rect, &p.line)
                        && contrast(p.line.color, *rgb) < COVER_CONTRAST
                        && !own.iter().any(|r| is_cover(r, p))
                }
            })? {
                let covered_text = budget.report_text(covered, copying)?;
                out.push(AuditFinding { page: n, kind, rect, covered_text });
            }
        }
    }
    Ok(out)
}

/// Text bytes a page's lines may hold in all, its appearances' included: the text reader's bound
/// for one read.
const MAX_PAGE_TEXT: usize = 16 << 20;

/// A line of text and where it comes in paint order: its layer (0 for the page's content, then one
/// for each annotation), then [`TextLine::paint_order`].
struct Painted {
    layer: usize,
    line: TextLine,
}

fn text_len(lines: &[Painted]) -> usize {
    lines.iter().fold(0, |n, p| n.saturating_add(p.line.text.len()))
}

/// The lines and advance gaps `page` shows, in `layer`, their boxes and points mapped by `ctm` when
/// it is read in a space of its own. The glyphs themselves: `/ActualText` is the file's own claim
/// about them, and an empty or substitute one must not hide text a cover sits on.
fn painted_text(
    doc: &Document,
    page: &pdfcraft_model::Page,
    n: usize,
    layer: usize,
    ctm: Option<&Matrix>,
    report_advances: bool,
) -> Result<(Vec<Painted>, Vec<GlyphAdvanceGap>), AuditError> {
    let pdfcraft_edit::text::GlyphText { lines, mut gaps, unread_forms } =
        pdfcraft_edit::glyph_text(doc, page, report_advances).map_err(|e| AuditError::Text(n, e.to_string()))?;
    if unread_forms {
        return Err(AuditError::UnreadForms(n));
    }
    // shortcut: boxes are mapped as boxes, so under a turn other than a quarter one a line's box
    // grows to the bounding box of the turned one, as a cover's does. Read the appearance with
    // `ctm` as its CTM if that loses a finding.
    let painted = lines
        .into_iter()
        .map(|mut line| {
            if let Some(m) = ctm {
                line.rect = m.bbox(line.rect);
                line.ink.iter_mut().for_each(|w| *w = m.bbox(*w));
            }
            Painted { layer, line }
        })
        .collect();
    for gap in &mut gaps {
        gap.paint_pos.insert(0, layer);
        if let Some(m) = ctm {
            (gap.from, gap.to) = (m.apply(gap.from.0, gap.from.1), m.apply(gap.to.0, gap.to.1));
        }
    }
    Ok((painted, gaps))
}

fn covers_gap(rect: &[f64; 4], gap: &GlyphAdvanceGap) -> bool {
    // Both endpoints inside the cover implies the whole displacement is covered, even for
    // rotated text. A small tolerance accounts for independent rounding of path and glyphs.
    let inside = |(x, y)| rect[0] - 0.05 <= x && x <= rect[2] + 0.05 && rect[1] - 0.05 <= y && y <= rect[3] + 0.05;
    let width = (gap.to.0 - gap.from.0).hypot(gap.to.1 - gap.from.1);
    width.is_finite() && width > 0.05 && inside(gap.from) && inside(gap.to)
}

/// A filled rectangle is a faux-redaction cover for a text line iff it was painted strictly
/// after the start of the line, overlaps it substantially, and is dark enough against the line's
/// color to make the text unreadable. Light covers (table shading, highlights) over dark text fail
/// the contrast check and are not flagged. A line can join runs painted before and after the
/// cover; the later ones are drawn on it in the same low-contrast color, so they are unreadable
/// too.
fn is_cover(rect: &cover::FilledRect, p: &Painted) -> bool {
    rect.pos.iter().copied().cmp(std::iter::once(p.layer).chain(p.line.paint_order())).is_gt()
        && covers(&rect.rect, &p.line)
        && contrast(p.line.color, rect.rgb) < COVER_CONTRAST
}

/// Does `cover` hide a substantial part of what `line` draws? Three ways, any is enough:
///
/// - Half of one word lies under the cover. A box over one number in a long line hides a few
///   percent of the line, and the number all the same (#2662).
///
/// - 40% of the area of the line's words (its `ink`) lies under the cover. Measured on the words,
///   not the box: an applied redaction leaves a gap that draws nothing (a `TJ` adjustment where
///   the text was), and the box still spans it, so a redaction box at the end of a line used to
///   cover most of the box and flag the words left beside it.
/// - 40% of the line's box lies under the cover (the measure used before `ink`) and intersects the ink
///   of a word still drawn. This keeps a cover over a short value after a long label
///   flagged, while a cover over the gap alone, with no word under it, stops counting.
///
/// A line without word boxes is measured on its box alone.
fn covers(cover: &[f64; 4], line: &TextLine) -> bool {
    let hidden = |b: &[f64; 4]| area(&[cover[0].max(b[0]), cover[1].max(b[1]), cover[2].min(b[2]), cover[3].min(b[3])]);
    let fraction = |boxes: &[[f64; 4]]| {
        let total: f64 = boxes.iter().map(area).sum();
        if total > 0.0 { boxes.iter().map(hidden).sum::<f64>() / total } else { 0.0 }
    };
    if line.ink.iter().any(|w| area(w) > 0.0 && hidden(w) / area(w) >= WORD_FRACTION) {
        return true;
    }
    let inked = fraction(&line.ink) >= COVER_FRACTION;
    let boxed = fraction(std::slice::from_ref(&line.rect)) >= COVER_FRACTION;
    let word_intersects = || line.ink.iter().any(|w| hidden(w) > 0.0);
    inked || (boxed && (line.ink.is_empty() || word_intersects()))
}

/// Appends the text of `line` that `cover` lies on to `out`: the runs whose words it overlaps
/// (#4164), so the words of the line painted beside the cover, which stay visible, are not
/// reported as hidden. Neighbouring runs are joined as the line joins them, so a word split
/// across runs stays one word; runs with others left out between them are spaced. The whole
/// line when its runs can't say: a single run, a line whose text was reordered for reading, or
/// a cover that overlaps the line's box but none of its words.
fn covered_words(cover: &[f64; 4], line: &TextLine, out: &mut String) {
    let overlaps = |w: &[f64; 4]| area(&[cover[0].max(w[0]), cover[1].max(w[1]), cover[2].min(w[2]), cover[3].min(w[3])]) > 0.0;
    let mut words = String::new();
    let mut last = None;
    for (i, (text, ink, spaced)) in line.runs().enumerate() {
        if text.trim().is_empty() || !ink.iter().any(overlaps) {
            continue;
        }
        let apart = last.is_some_and(|l: usize| l.saturating_add(1) != i || spaced);
        if apart && !words.ends_with(char::is_whitespace) && !text.starts_with(char::is_whitespace) {
            words.push(' ');
        }
        words.push_str(text);
        last = Some(i);
    }
    if line.runs().nth(1).is_none() || last.is_none() {
        out.push_str(line.text.trim());
    } else {
        out.push_str(words.trim());
    }
}

/// The area of a rectangle, 0 when it is empty or inverted.
fn area(r: &[f64; 4]) -> f64 {
    (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0)
}

/// WCAG 2.x relative luminance of an sRGB triple.
fn luminance(rgb: [f64; 3]) -> f64 {
    let lin = rgb.map(|c| {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    });
    0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2]
}

/// WCAG contrast ratio between two sRGB colors: 1.0 (identical) to 21.0 (black on white).
fn contrast(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (l1, l2) = (luminance(a), luminance(b));
    let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

enum AnnotationCover {
    Redact([f64; 4]),
    Square { rect: [f64; 4], rgb: [f64; 3] },
}

/// An annotation the audit reads: its layer in paint order, the cover its dictionary describes and
/// the appearance a viewer paints for it.
struct Annotation {
    layer: usize,
    cover: Option<AnnotationCover>,
    appearance: Option<cover::Appearance>,
}

/// The annotations on page `n` (unrotated user space) that cover text or paint an appearance. A
/// missing `/Annots` means none; a malformed one fails the audit rather than silently passing.
fn annots(doc: &Document, page: &Dict, n: usize) -> Result<Vec<Annotation>, AuditError> {
    let Some(a) = page.get(b"Annots") else { return Ok(Vec::new()) };
    let resolved = doc.resolve(a);
    // A missing /Annots means no marks; a malformed one fails the audit.
    let Some(items) = resolved.as_array() else { return Err(AuditError::BadAnnots) };
    let mut out = Vec::new();
    let mut appearances = 0_usize;
    for (i, a) in items.iter().enumerate() {
        // Borrowed from the resolved object: one large dictionary listed many times is not
        // copied once per listing.
        let resolved = doc.resolve(a);
        let Some(dict) = resolved.as_dict() else { continue };
        let v: Vec<f64> = match dict.get(b"Rect") {
            None => continue,
            Some(r) => doc.resolve(r).as_array().map(|arr| arr.iter().filter_map(|o| doc.resolve(o).as_f64()).collect()).unwrap_or_default(),
        };
        if v.len() != 4 || !v.iter().all(|x| x.is_finite()) {
            continue;
        }
        let rect = [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])];
        // Hidden and NoView annotations are never drawn on screen.
        let flags = dict.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
        let shown = flags & 2 == 0 && flags & 32 == 0;
        let cover = match subtype(doc, dict)?.as_deref() {
            Some(b"Redact") => Some(AnnotationCover::Redact(rect)),
            Some(b"Square") if shown => {
                let alpha = dict.get(b"CA").map(|o| doc.resolve(o)).and_then(|o| o.as_f64()).unwrap_or(1.0);
                annotation_color(doc, dict, b"IC").filter(|_| alpha >= 1.0).map(|rgb| AnnotationCover::Square { rect, rgb })
            }
            _ => None,
        };
        let layer = i.saturating_add(1);
        // Each appearance is one form entry of the page's budget: count them before reading them.
        let appearance = if shown && dict.contains(b"AP") {
            appearances = appearances.saturating_add(1);
            if appearances > pdfcraft_cos::MAX_PAGE_FORMS {
                return Err(AuditError::TooManyForms);
            }
            appearance(doc, dict, rect, n, layer)?
        } else {
            None
        };
        if cover.is_some() || appearance.is_some() {
            out.push(Annotation { layer, cover, appearance });
        }
    }
    Ok(out)
}

/// The appearance a viewer paints for an annotation at `rect` (ISO 32000-2 §12.5.5): its normal
/// appearance, or of those it has for each state the one `/AS` names, its box mapped onto `rect`.
/// `None` when there is none, or it can show nothing (its box or `rect` has no area). One without a
/// usable `/BBox` fails the audit: where it draws can't be worked out.
fn appearance(doc: &Document, dict: &Dict, rect: [f64; 4], n: usize, layer: usize) -> Result<Option<cover::Appearance>, AuditError> {
    let ap = dict.get(b"AP").map(|o| doc.resolve(o));
    let Some(normal) = ap.as_deref().and_then(Object::as_dict).and_then(|d| d.get(b"N")) else { return Ok(None) };
    // A dictionary (not a stream, whose dictionary `as_dict` would also give) holds one per state.
    let contents = match &*doc.resolve(normal) {
        Object::Dict(states) => {
            let state = dict.get(b"AS").map(|o| doc.resolve(o));
            match state.as_deref().and_then(Object::as_name).and_then(|s| states.get(s)) {
                Some(chosen) => chosen.clone(),
                None => return Ok(None),
            }
        }
        _ => normal.clone(),
    };
    let resolved = doc.resolve(&contents);
    let Object::Stream(form) = &*resolved else { return Ok(None) };
    let bbox = form.dict.get(b"BBox").map(|o| doc.resolve(o));
    let bbox: Vec<f64> =
        bbox.as_deref().and_then(Object::as_array).map(|a| a.iter().filter_map(|o| doc.resolve(o).as_f64()).collect()).unwrap_or_default();
    let Some([x0, y0, x1, y1]) = <[f64; 4]>::try_from(bbox).ok().filter(|b| b.iter().all(|x| x.is_finite())) else {
        return Err(AuditError::BadAppearance(n));
    };
    let matrix = cover::form_matrix(doc, &form.dict);
    let [bx0, by0, bx1, by1] = matrix.bbox([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]);
    let (w, h) = (bx1 - bx0, by1 - by0);
    let shows = w > 0.0 && h > 0.0 && rect[2] > rect[0] && rect[3] > rect[1];
    Ok(shows.then(|| {
        let (sx, sy) = ((rect[2] - rect[0]) / w, (rect[3] - rect[1]) / h);
        let fit = Matrix([sx, 0.0, 0.0, sy, rect[0] - bx0 * sx, rect[1] - by0 * sy]);
        cover::Appearance { layer, contents, form: std::sync::Arc::clone(&resolved), ctm: matrix.then(&fit) }
    }))
}

/// The `/Subtype` name of `dict`, following an indirect reference. A reference that cannot be
/// loaded fails the audit: the subtype decides whether the object is checked at all.
fn subtype(doc: &Document, dict: &Dict) -> Result<Option<Vec<u8>>, AuditError> {
    let name = match dict.get(b"Subtype") {
        Some(pdfcraft_cos::Object::Ref(r)) => doc.try_get(r.num)?.as_name().map(<[u8]>::to_vec),
        other => other.and_then(pdfcraft_cos::Object::as_name).map(<[u8]>::to_vec),
    };
    Ok(name)
}

fn annotation_color(doc: &Document, dict: &Dict, key: &[u8]) -> Option<[f64; 3]> {
    let values: Vec<f64> = dict.get(key).map(|o| doc.resolve(o))?.as_array()?.iter().map(|o| doc.resolve(o).as_f64()).collect::<Option<_>>()?;
    if !values.iter().all(|v| v.is_finite()) {
        return None;
    }
    match values.as_slice() {
        [g] => Some([g.clamp(0.0, 1.0); 3]),
        [r, g, b] => Some([r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]),
        [c, m, y, k] => {
            let (c, m, y, k) = (c.clamp(0.0, 1.0), m.clamp(0.0, 1.0), y.clamp(0.0, 1.0), k.clamp(0.0, 1.0));
            Some([(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)])
        }
        _ => None,
    }
}
