// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Flow layout for documents created from structured text (Markdown, CSV tables, email
//! messages): blocks of styled text in the standard 14 fonts (WinAnsiEncoding, so characters
//! outside it print as `?`, as in [`crate::from_text`]), wrapped and paginated, with simple
//! tables and link annotations.

use pdfcraft_cos::{Dict, Document, Object, PdfString};
use pdfcraft_fonts::{helvetica_width, literal, win_ansi, win_ansi_encodable};

use crate::{CreateError, Pages, set_title};

/// More pages than this is refused rather than written.
pub(crate) const MAX_PAGES: usize = 10_000;
/// The most columns a table lays out; cells past it are dropped (and counted by the caller).
pub(crate) const MAX_COLUMNS: usize = 64;
/// The most lines one table cell shows; the rest is cut with an ellipsis.
const MAX_CELL_LINES: usize = 40;
/// The longest link address written as a link annotation.
const MAX_URI: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Face {
    Regular,
    Bold,
    Italic,
    BoldItalic,
    Mono,
    MonoBold,
}

/// Resource name and standard 14 base font of each face.
const FACES: [(Face, &str, &str); 6] = [
    (Face::Regular, "F1", "Helvetica"),
    (Face::Bold, "F2", "Helvetica-Bold"),
    (Face::Italic, "F3", "Helvetica-Oblique"),
    (Face::BoldItalic, "F4", "Helvetica-BoldOblique"),
    (Face::Mono, "F5", "Courier"),
    (Face::MonoBold, "F6", "Courier-Bold"),
];

impl Face {
    pub(crate) fn styled(bold: bool, italic: bool, mono: bool) -> Face {
        match (mono, bold, italic) {
            (true, true, _) => Face::MonoBold,
            (true, false, _) => Face::Mono,
            (false, true, true) => Face::BoldItalic,
            (false, true, false) => Face::Bold,
            (false, false, true) => Face::Italic,
            (false, false, false) => Face::Regular,
        }
    }

    fn resource(self) -> &'static str {
        FACES.iter().find(|(f, ..)| *f == self).map_or("F1", |(_, r, _)| r)
    }

    /// Approximate advance (PdfCraft's own estimates; no vendor metrics are bundled). Courier is
    /// monospaced at 600 units; bold Helvetica runs a little wider than regular.
    pub(crate) fn width(self, s: &str, size: f64) -> f64 {
        match self {
            Face::Mono | Face::MonoBold => s.chars().count() as f64 * 0.6 * size,
            Face::Bold | Face::BoldItalic => helvetica_width(s, size) * 1.07,
            Face::Regular | Face::Italic => helvetica_width(s, size),
        }
    }
}

/// A run of text in one face, optionally a link.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Span {
    pub text: String,
    pub face: Face,
    pub link: Option<String>,
}

impl Span {
    pub(crate) fn new(text: impl Into<String>, face: Face) -> Span {
        Span { text: text.into(), face, link: None }
    }
}

/// A paragraph: spans wrapped to the width (a `\n` inside a span is a hard line break), `size`
/// points, indented `indent` points, with an optional marker (bullet or number) hung in front of
/// the first line.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Para {
    pub spans: Vec<Span>,
    pub size: f64,
    pub indent: f64,
    pub marker: Option<String>,
    pub before: f64,
    pub after: f64,
    /// Keep the first lines on the same page as what follows (headings).
    pub keep_with_next: bool,
}

impl Para {
    pub(crate) fn body(spans: Vec<Span>, size: f64) -> Para {
        Para { spans, size, indent: 0.0, marker: None, before: 0.0, after: size * 0.6, keep_with_next: false }
    }
}

/// Rows of cells; the first row is a header (bold, shaded, repeated on each page) when `header`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Table {
    pub rows: Vec<Vec<String>>,
    pub header: bool,
    pub size: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Block {
    Para(Para),
    /// Preformatted lines in Courier on a light background, wrapped only when wider than the page.
    Code {
        lines: Vec<String>,
        size: f64,
        indent: f64,
    },
    Rule,
    Table(Table),
    Space(f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PageSetup {
    pub width: f64,
    pub height: f64,
    pub margin: f64,
}

impl PageSetup {
    pub(crate) fn portrait((width, height): (f64, f64)) -> PageSetup {
        PageSetup { width, height, margin: 54.0 }
    }

    pub(crate) fn landscape((width, height): (f64, f64)) -> PageSetup {
        PageSetup { width: width.max(height), height: width.min(height), margin: 36.0 }
    }

    pub(crate) fn content_width(&self) -> f64 {
        (self.width - 2.0 * self.margin).max(36.0)
    }
}

/// An address safe to write as a link annotation: http, https or mailto, printable ASCII only.
pub(crate) fn safe_uri(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    url.len() <= MAX_URI
        && url.chars().all(|c| c.is_ascii_graphic())
        && (lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:"))
        && url.len() > "mailto:".len()
}

/// Lay `blocks` out on pages and return the new document, titled `title`, and how many characters
/// the standard fonts could not show (written as `?`), so the caller can say so.
pub(crate) fn render(title: &str, blocks: &[Block], page: PageSetup) -> Result<(Document, usize), CreateError> {
    if !(page.width.is_finite() && page.height.is_finite() && page.width >= 144.0 && page.height >= 144.0) {
        return Err(CreateError::Invalid("invalid page size".into()));
    }
    let mut w = Writer::new(page);
    for b in blocks {
        match b {
            Block::Para(p) => w.para(p)?,
            Block::Code { lines, size, indent } => w.code(lines, *size, *indent)?,
            Block::Rule => w.rule()?,
            Block::Table(t) => w.table(t)?,
            Block::Space(h) => w.space(*h),
        }
    }
    w.finish(title)
}

/// One laid-out piece of a line.
#[derive(Clone, Debug, PartialEq)]
struct Piece {
    text: String,
    face: Face,
    link: Option<String>,
    width: f64,
}

enum Tok<'a> {
    Word(String, &'a Span),
    Space(&'a Span),
    Break,
}

fn tokens(spans: &[Span]) -> Vec<Tok<'_>> {
    let mut out = Vec::new();
    for s in spans {
        let mut word = String::new();
        for c in s.text.chars() {
            if c == '\n' || c.is_whitespace() {
                if !word.is_empty() {
                    out.push(Tok::Word(std::mem::take(&mut word), s));
                }
                out.push(if c == '\n' { Tok::Break } else { Tok::Space(s) });
            } else {
                word.push(c);
            }
        }
        if !word.is_empty() {
            out.push(Tok::Word(word, s));
        }
    }
    out
}

fn push_piece(line: &mut Vec<Piece>, text: &str, span: &Span, size: f64) -> f64 {
    let width = span.face.width(text, size);
    match line.last_mut() {
        Some(p) if p.face == span.face && p.link == span.link => {
            p.text.push_str(text);
            p.width += width;
        }
        _ => line.push(Piece { text: text.to_string(), face: span.face, link: span.link.clone(), width }),
    }
    width
}

fn trim_end(line: &mut Vec<Piece>, size: f64) {
    while let Some(p) = line.last_mut() {
        if p.text.ends_with(' ') {
            p.text.pop();
            p.width = (p.width - p.face.width(" ", size)).max(0.0);
        }
        if p.text.is_empty() {
            line.pop();
        } else {
            break;
        }
    }
}

/// Greedy line breaking of styled spans within `width` points; a word wider than a line is
/// broken by character.
fn lines_of(spans: &[Span], size: f64, width: f64) -> Vec<Vec<Piece>> {
    let mut lines = Vec::new();
    let mut line: Vec<Piece> = Vec::new();
    let mut x = 0.0;
    for tok in tokens(spans) {
        match tok {
            Tok::Break => {
                trim_end(&mut line, size);
                lines.push(std::mem::take(&mut line));
                x = 0.0;
            }
            Tok::Space(s) => {
                if !line.is_empty() {
                    x += push_piece(&mut line, " ", s, size);
                }
            }
            Tok::Word(word, s) => {
                let ww = s.face.width(&word, size);
                if x + ww > width && !line.is_empty() {
                    trim_end(&mut line, size);
                    lines.push(std::mem::take(&mut line));
                    x = 0.0;
                }
                if ww <= width {
                    x += push_piece(&mut line, &word, s, size);
                    continue;
                }
                let mut part = String::new();
                for ch in word.chars() {
                    let mut candidate = part.clone();
                    candidate.push(ch);
                    if x + s.face.width(&candidate, size) > width && !(part.is_empty() && line.is_empty()) {
                        if !part.is_empty() {
                            push_piece(&mut line, &part, s, size);
                        }
                        trim_end(&mut line, size);
                        lines.push(std::mem::take(&mut line));
                        x = 0.0;
                        part.clear();
                    }
                    part.push(ch);
                }
                if !part.is_empty() {
                    x += push_piece(&mut line, &part, s, size);
                }
            }
        }
    }
    trim_end(&mut line, size);
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Plain text wrapped in one face (table cells, code lines).
fn wrap_plain(text: &str, face: Face, size: f64, width: f64) -> Vec<String> {
    let span = Span::new(text, face);
    lines_of(std::slice::from_ref(&span), size, width).into_iter().map(|l| l.into_iter().map(|p| p.text).collect()).collect()
}

#[derive(Default)]
struct Sheet {
    content: Vec<u8>,
    links: Vec<([f64; 4], String)>,
}

struct Writer {
    page: PageSetup,
    done: Vec<Sheet>,
    cur: Sheet,
    y: f64,
    replaced: usize,
}

fn num(v: f64) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    format!("{v:.2}")
}

impl Writer {
    fn new(page: PageSetup) -> Writer {
        Writer { page, done: Vec::new(), cur: Sheet::default(), y: page.height - page.margin, replaced: 0 }
    }

    fn top(&self) -> f64 {
        self.page.height - self.page.margin
    }

    fn left(&self) -> f64 {
        self.page.margin
    }

    fn at_top(&self) -> bool {
        (self.y - self.top()).abs() < 0.01
    }

    /// The height left on the current page.
    fn room(&self) -> f64 {
        self.y - self.page.margin
    }

    /// The usable height of a whole page.
    fn page_room(&self) -> f64 {
        (self.page.height - 2.0 * self.page.margin).max(1.0)
    }

    fn new_page(&mut self) -> Result<(), CreateError> {
        if self.done.len() + 1 >= MAX_PAGES {
            return Err(CreateError::Invalid(format!("the document would have more than {MAX_PAGES} pages")));
        }
        self.done.push(std::mem::take(&mut self.cur));
        self.y = self.top();
        Ok(())
    }

    /// Start a new page unless `h` points fit on this one (or this page is still empty).
    fn ensure(&mut self, h: f64) -> Result<(), CreateError> {
        if h > self.room() && !self.at_top() {
            self.new_page()?;
        }
        Ok(())
    }

    fn space(&mut self, h: f64) {
        if !self.at_top() && h.is_finite() && h > 0.0 {
            self.y = (self.y - h).max(self.page.margin);
        }
    }

    fn text(&mut self, x: f64, y: f64, face: Face, size: f64, s: &str) {
        self.replaced = self.replaced.saturating_add(s.chars().filter(|c| !win_ansi_encodable(*c)).count());
        let c = &mut self.cur.content;
        c.extend(format!("BT /{} {} Tf {} {} Td ", face.resource(), num(size), num(x), num(y)).into_bytes());
        c.extend(literal(&win_ansi(s)));
        c.extend_from_slice(b" Tj ET\n");
    }

    fn line(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, grey: f64) {
        self.cur.content.extend(format!("q 0.5 w {} G {} {} m {} {} l S Q\n", num(grey), num(x0), num(y0), num(x1), num(y1)).into_bytes());
    }

    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, fill: Option<f64>, stroke: Option<f64>) {
        let (x, y, w, h) = (num(x), num(y), num(w), num(h));
        if let Some(g) = fill {
            self.cur.content.extend(format!("q {} g {x} {y} {w} {h} re f Q\n", num(g)).into_bytes());
        }
        if let Some(g) = stroke {
            self.cur.content.extend(format!("q 0.5 w {} G {x} {y} {w} {h} re S Q\n", num(g)).into_bytes());
        }
    }

    fn para(&mut self, p: &Para) -> Result<(), CreateError> {
        let size = if p.size.is_finite() { p.size.clamp(4.0, 72.0) } else { 11.0 };
        let indent = if p.indent.is_finite() { p.indent.clamp(0.0, self.page.content_width() / 2.0) } else { 0.0 };
        let line_h = size * 1.3;
        let lines = lines_of(&p.spans, size, self.page.content_width() - indent);
        self.space(p.before);
        if p.keep_with_next {
            self.ensure(line_h * (lines.len().min(3) as f64 + 2.0))?;
        }
        for (i, line) in lines.iter().enumerate() {
            self.ensure(line_h)?;
            let base = self.y - size;
            let mut x = self.left() + indent;
            if i == 0
                && let Some(m) = &p.marker
            {
                let mw = Face::Regular.width(m, size);
                self.text((x - mw - size * 0.4).max(0.0), base, Face::Regular, size, m);
            }
            for piece in line {
                self.text(x, base, piece.face, size, &piece.text);
                if let Some(url) = &piece.link
                    && safe_uri(url)
                {
                    self.cur.links.push(([x, base - size * 0.25, x + piece.width, base + size * 0.85], url.clone()));
                }
                x += piece.width;
            }
            self.y -= line_h;
        }
        self.space(p.after);
        Ok(())
    }

    fn code(&mut self, lines: &[String], size: f64, indent: f64) -> Result<(), CreateError> {
        let size = if size.is_finite() { size.clamp(4.0, 72.0) } else { 9.5 };
        let indent = if indent.is_finite() { indent.clamp(0.0, self.page.content_width() / 2.0) } else { 0.0 };
        let line_h = size * 1.3;
        let width = self.page.content_width() - indent - 8.0;
        for raw in lines {
            let expanded = raw.replace('\t', "    ");
            for l in wrap_plain(&expanded, Face::Mono, size, width) {
                self.ensure(line_h)?;
                let x = self.left() + indent;
                self.rect(x, self.y - line_h, self.page.content_width() - indent, line_h, Some(0.95), None);
                self.text(x + 4.0, self.y - size, Face::Mono, size, &l);
                self.y -= line_h;
            }
        }
        self.space(size * 0.8);
        Ok(())
    }

    fn rule(&mut self) -> Result<(), CreateError> {
        self.ensure(12.0)?;
        let (x0, x1, y) = (self.left(), self.left() + self.page.content_width(), self.y - 6.0);
        self.line(x0, y, x1, y, 0.6);
        self.y -= 12.0;
        Ok(())
    }

    fn table(&mut self, t: &Table) -> Result<(), CreateError> {
        let ncols = t.rows.iter().map(Vec::len).max().unwrap_or(0).min(MAX_COLUMNS);
        if ncols == 0 {
            return Ok(());
        }
        let size = if t.size.is_finite() { t.size.clamp(4.0, 24.0) } else { 9.0 };
        let (pad, line_h) = (3.0, size * 1.25);
        let avail = self.page.content_width();
        let widths = column_widths(t, ncols, size, pad, avail);
        let mut header: Option<(Vec<Vec<String>>, f64)> = None;
        for (ri, row) in t.rows.iter().enumerate() {
            let is_header = t.header && ri == 0;
            let face = if is_header { Face::Bold } else { Face::Regular };
            let mut cells: Vec<Vec<String>> = Vec::with_capacity(ncols);
            for (ci, w) in widths.iter().enumerate() {
                let text = row.get(ci).map_or("", String::as_str);
                let mut lines = wrap_plain(text, face, size, (w - 2.0 * pad).max(size));
                // A row never grows past one page or MAX_CELL_LINES lines.
                let cap = (((self.page_room() - 2.0 * pad) / line_h).floor() as usize).clamp(1, MAX_CELL_LINES);
                if lines.len() > cap {
                    lines.truncate(cap);
                    if let Some(last) = lines.last_mut() {
                        last.push('…');
                    }
                }
                cells.push(lines);
            }
            let row_h = cells.iter().map(Vec::len).max().unwrap_or(1).max(1) as f64 * line_h + 2.0 * pad;
            if row_h > self.room() && !self.at_top() {
                self.new_page()?;
                if let Some((h_cells, h_h)) = header.clone() {
                    self.table_row(&h_cells, &widths, h_h, size, pad, line_h, true);
                }
            }
            self.table_row(&cells, &widths, row_h, size, pad, line_h, is_header);
            if is_header {
                header = Some((cells, row_h));
            }
        }
        self.space(size);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn table_row(&mut self, cells: &[Vec<String>], widths: &[f64], row_h: f64, size: f64, pad: f64, line_h: f64, header: bool) {
        let top = self.y;
        let mut x = self.left();
        let face = if header { Face::Bold } else { Face::Regular };
        for (lines, w) in cells.iter().zip(widths) {
            self.rect(x, top - row_h, *w, row_h, header.then_some(0.88), Some(0.6));
            for (i, l) in lines.iter().enumerate() {
                self.text(x + pad, top - pad - size - i as f64 * line_h, face, size, l);
            }
            x += w;
        }
        self.y = top - row_h;
    }

    fn finish(mut self, title: &str) -> Result<(Document, usize), CreateError> {
        self.done.push(std::mem::take(&mut self.cur));
        let mut doc = Document::new_empty();
        let mut fonts = Dict::new();
        for (_, name, base) in FACES {
            let mut f = Dict::new();
            f.set(b"Type".to_vec(), Object::name("Font"));
            f.set(b"Subtype".to_vec(), Object::name("Type1"));
            f.set(b"BaseFont".to_vec(), Object::name(base));
            f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
            fonts.set(name.as_bytes().to_vec(), Object::Ref(doc.add(f)));
        }
        let mut resources = Dict::new();
        resources.set(b"Font".to_vec(), Object::Dict(fonts));
        let mut tree = Pages::of(&doc)?;
        for sheet in self.done {
            let mut annots = Vec::new();
            for (r, url) in sheet.links {
                let mut action = Dict::new();
                action.set(b"S".to_vec(), Object::name("URI"));
                action.set(b"URI".to_vec(), PdfString::literal(url.into_bytes()));
                let mut a = Dict::new();
                a.set(b"Type".to_vec(), Object::name("Annot"));
                a.set(b"Subtype".to_vec(), Object::name("Link"));
                a.set(b"Rect".to_vec(), Object::Array(r.iter().map(|v| Object::Real(if v.is_finite() { *v } else { 0.0 })).collect()));
                a.set(b"Border".to_vec(), Object::Array(vec![0.into(), 0.into(), 0.into()]));
                a.set(b"A".to_vec(), Object::Dict(action));
                annots.push(Object::Ref(doc.add(a)));
            }
            let page = tree.add(&mut doc, self.page.width, self.page.height, resources.clone(), Some(sheet.content));
            if !annots.is_empty() {
                doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(annots)))
                    .map_err(|e| CreateError::Invalid(format!("couldn't add the links: {e}")))?;
            }
        }
        tree.finish(&mut doc)?;
        set_title(&mut doc, title);
        Ok((doc, self.replaced))
    }
}

/// Column widths for `ncols` columns: each column's natural width (its widest cell, capped), then
/// scaled down to fit `avail`, never narrower than a few characters unless the columns can't
/// fit at all, when they share the width equally.
fn column_widths(t: &Table, ncols: usize, size: f64, pad: f64, avail: f64) -> Vec<f64> {
    let min_w = (size * 3.0 + 2.0 * pad).min(avail / ncols as f64);
    let max_natural = avail * 0.6;
    let mut natural = vec![min_w; ncols];
    // Measuring every cell of a huge table costs more than it tells: the first rows decide.
    for (ri, row) in t.rows.iter().take(500).enumerate() {
        let face = if t.header && ri == 0 { Face::Bold } else { Face::Regular };
        for (ci, cell) in row.iter().take(ncols).enumerate() {
            let sample: String = cell.chars().take(120).collect();
            let longest = sample.split('\n').map(|l| face.width(l, size)).fold(0.0, f64::max);
            if let Some(n) = natural.get_mut(ci) {
                *n = n.max((longest + 2.0 * pad).min(max_natural));
            }
        }
    }
    let total: f64 = natural.iter().sum();
    if total <= avail {
        return natural;
    }
    // Columns at their minimum keep it; the rest share what is left in proportion.
    let scale = avail / total;
    let scaled: Vec<f64> = natural.iter().map(|w| (w * scale).max(min_w)).collect();
    let sum: f64 = scaled.iter().sum();
    if sum <= avail + 0.01 {
        scaled
    } else {
        let wide: f64 = scaled.iter().filter(|w| **w > min_w).sum();
        let narrow = sum - wide;
        let room = (avail - narrow).max(0.0);
        if wide <= 0.0 || room <= 0.0 {
            return vec![avail / ncols as f64; ncols];
        }
        scaled.iter().map(|w| if *w > min_w { (w * room / wide).max(min_w) } else { *w }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(spans: &[Span], size: f64, width: f64) -> Vec<String> {
        lines_of(spans, size, width).into_iter().map(|l| l.into_iter().map(|p| p.text).collect()).collect()
    }

    #[test]
    fn wraps_mixed_faces_and_honours_hard_breaks() {
        let spans = vec![Span::new("Hello ", Face::Regular), Span::new("bold world", Face::Bold), Span::new(" and more\nnext", Face::Regular)];
        let lines = text_of(&spans, 10.0, 1000.0);
        assert_eq!(lines, vec!["Hello bold world and more", "next"]);
        let narrow = text_of(&spans, 10.0, 60.0);
        assert!(narrow.len() > 2, "{narrow:?}");
        assert!(narrow.iter().all(|l| !l.ends_with(' ') && !l.starts_with(' ')), "{narrow:?}");
    }

    #[test]
    fn breaks_a_word_longer_than_the_line() {
        let long = "x".repeat(500);
        let lines = text_of(&[Span::new(long.clone(), Face::Mono)], 10.0, 100.0);
        assert!(lines.len() > 5);
        assert_eq!(lines.concat(), long);
        assert!(lines.iter().all(|l| Face::Mono.width(l, 10.0) <= 100.0 + 0.01));
    }

    #[test]
    fn link_addresses_are_limited_to_web_and_email() {
        assert!(safe_uri("https://example.org/a?b=c"));
        assert!(safe_uri("mailto:someone@example.org"));
        for bad in ["javascript:alert(1)", "file:///etc/passwd", "https://exa mple.org", "mailto:", "data:text/html,x", "https://\u{202e}x"] {
            assert!(!safe_uri(bad), "{bad}");
        }
        assert!(!safe_uri(&format!("https://example.org/{}", "a".repeat(MAX_URI))));
    }

    #[test]
    fn columns_fit_the_page() {
        let t = Table { rows: vec![vec!["a".repeat(300), "b".into(), "c".repeat(80)]; 3], header: true, size: 9.0 };
        let w = column_widths(&t, 3, 9.0, 3.0, 500.0);
        assert!(w.iter().sum::<f64>() <= 500.01, "{w:?}");
        let many = Table { rows: vec![vec!["wide cell text".to_string(); MAX_COLUMNS]], header: false, size: 9.0 };
        let w = column_widths(&many, MAX_COLUMNS, 9.0, 3.0, 500.0);
        assert!(w.iter().sum::<f64>() <= 500.01 && w.iter().all(|w| *w > 0.0), "{w:?}");
    }

    #[test]
    fn pages_are_capped() {
        let blocks: Vec<Block> = (0..20).map(|_| Block::Para(Para::body(vec![Span::new("line\n".repeat(400), Face::Regular)], 11.0))).collect();
        assert!(render("t", &blocks, PageSetup::portrait(crate::LETTER)).is_ok());
        let mut w = Writer::new(PageSetup::portrait(crate::LETTER));
        w.done = (0..MAX_PAGES - 1).map(|_| Sheet::default()).collect();
        assert!(w.new_page().is_err());
    }
}
