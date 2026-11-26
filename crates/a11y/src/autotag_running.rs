//! Running headers, footers and page numbers among auto-tagged text objects, found by repetition.
//!
//! A text object is running text when it lies wholly inside the top or bottom 15% of the page as
//! displayed, and the same text appears within 4 units of the same distance from that edge on at
//! least 40% of the pages, and at least two. Text is compared lowercase with whitespace collapsed,
//! each run of digits, and a roman numeral standing alone or after `Page`, matching any number, so
//! page numbers repeat. 40% lets alternating left- and right-page headers qualify. A group whose
//! text changes from page to page and is all heading-sized (`Chapter 1`, `Chapter 2`) is a
//! sequence of headings, not running text, unless it reads as a page number (`Page 3`, `3 of 9`).
//! Every object in some qualifying span of distances counts, so spans that overlap and spans that
//! lie apart are each marked. Position alone never makes running text.
//! A text object whose extent is not known (see [`Evidence::extent`]) is never running text.
//! Fork: text that changes from page to page runs only as a page-number sequence ([`repeats`]).

use std::collections::HashMap;

use crate::autotag_roles::{Evidence, HEADING_SIZE_RATIO, MAX_KEPT_CHARS};

/// Depth of the top and bottom bands, as a fraction of the page height.
const BAND: f64 = 0.15;
/// How far, in user-space units, the same running text may move from page to page.
const TOLERANCE: f64 = 4.0;
/// Height clusters marked per group of the same text; copies at further heights stay content.
const MAX_CLUSTERS: usize = 8;

/// The page edge running text repeats at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Edge {
    Top,
    Bottom,
}

impl Edge {
    /// The artifact subtype (ISO 32000-2 Table 363) for running text at this edge.
    pub(crate) fn subtype(self) -> &'static str {
        match self {
            Edge::Top => "Header",
            Edge::Bottom => "Footer",
        }
    }
}

/// How a page is displayed: its `/Rotate` (0, 90, 180 or 270) and its crop box in user space.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    pub rotation: i64,
    pub crop: [f64; 4],
}

/// One page's text objects, in the order of its `(BT, ET)` spans.
pub(crate) struct PageText<'a> {
    pub frame: Frame,
    pub objects: &'a [Evidence],
}

struct Member<'a> {
    page: usize,
    object: usize,
    distance: f64,
    evidence: &'a Evidence,
}

/// The edge each text object repeats at, for running text, per page and object.
pub(crate) fn find(pages: &[PageText<'_>], body: Option<f64>) -> Vec<Vec<Option<Edge>>> {
    let mut out: Vec<Vec<Option<Edge>>> = pages.iter().map(|p| vec![None; p.objects.len()]).collect();
    if pages.len() < 2 {
        return out;
    }
    let needed = pages.len().saturating_mul(2).div_ceil(5).max(2);
    for ((edge, key), mut members) in groups(pages) {
        members.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        for cluster in clusters(&members, needed) {
            if is_heading_sequence(cluster, &key, body) || !repeats(cluster, &key) {
                continue;
            }
            for m in cluster {
                if let Some(slot) = out.get_mut(m.page).and_then(|p| p.get_mut(m.object)) {
                    *slot = Some(edge);
                }
            }
        }
    }
    out
}

/// `Chapter 1`, `Chapter 2`: heading-sized text that changes, unless it reads as a page number.
fn is_heading_sequence(window: &[Member<'_>], key: &str, body: Option<f64>) -> bool {
    let texts_differ = window.windows(2).any(|w| matches!(w, [a, b] if folded(&a.evidence.text) != folded(&b.evidence.text)));
    let heading_sized = body.is_some_and(|body| window.iter().all(|m| m.evidence.size >= body * HEADING_SIZE_RATIO));
    let worded = window.iter().all(|m| m.evidence.letters) && !is_page_number(key);
    texts_differ && heading_sized && worded
}

/// Fork guard: a group whose text changes repeats only as a page-number sequence. Every member has
/// the same count of numbers, exactly one of them changes, and it steps with the page index (the
/// same offset on every member, so numbering may start anywhere and skip unnumbered pages); the
/// others stay the same. The key must read as a page number or keep a word. So differing amounts at
/// the same height (`1,234.56`, `Total 7,890.12`) stay content; upstream marks them as footers.
fn repeats(window: &[Member<'_>], key: &str) -> bool {
    let Some(first) = window.first().map(|m| folded(&m.evidence.text)) else {
        return false;
    };
    if window.iter().all(|m| folded(&m.evidence.text) == first) {
        return true;
    }
    if !is_page_number(key) && !key.chars().any(char::is_alphabetic) {
        return false;
    }
    let Some(rows) = window.iter().map(|m| Some((i128::try_from(m.page).ok()?, numbers(&m.evidence.text)?))).collect::<Option<Vec<_>>>() else {
        return false;
    };
    let width = rows.first().map_or(0, |(_, n)| n.len());
    if rows.iter().any(|(_, n)| n.len() != width) {
        return false;
    }
    let mut stepping = 0usize;
    for slot in 0..width {
        let values: Vec<(i128, u64)> = rows.iter().filter_map(|(page, n)| n.get(slot).map(|v| (*page, *v))).collect();
        if values.windows(2).all(|w| matches!(w, [a, b] if a.1 == b.1)) {
            continue;
        }
        let offsets: Vec<Option<i128>> = values.iter().map(|(page, v)| i128::from(*v).checked_sub(*page)).collect();
        if offsets.iter().any(Option::is_none) || offsets.windows(2).any(|w| matches!(w, [a, b] if a != b)) {
            return false;
        }
        stepping = stepping.saturating_add(1);
    }
    stepping == 1
}

/// The numbers [`normalise`] replaces by `#`, in order: each digit run, and a roman numeral standing
/// alone or after `page`. `None` when a digit run is too long to be a page number.
fn numbers(text: &str) -> Option<Vec<u64>> {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    if let [w] | ["page", w] = words.as_slice()
        && let Some(v) = roman_value(w)
    {
        return Some(vec![u64::from(v)]);
    }
    let mut out = Vec::new();
    for run in lower.split(|c: char| !c.is_ascii_digit()).filter(|r| !r.is_empty()) {
        out.push(run.parse::<u64>().ok().filter(|_| run.len() <= 18)?);
    }
    Some(out)
}

/// Lowercase with whitespace collapsed, digits kept: `ANNUAL  REPORT` and `Annual report` are the
/// same heading, `Chapter 1` and `Chapter 2` are not.
fn folded(text: &str) -> String {
    text.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A normalised key that is a page number alone or with its label: `#`, `page #`, `p. #`,
/// `# of #`, `page # of #`, `#/#`, optionally in dashes or brackets.
fn is_page_number(key: &str) -> bool {
    let bare = key.trim_matches(|c: char| matches!(c, '-' | '\u{2013}' | '\u{2014}' | '[' | ']' | '(' | ')' | ' '));
    matches!(bare, "#" | "page #" | "p. #" | "# of #" | "page # of #" | "#/#" | "# / #")
}

/// Text objects inside a band, grouped by edge and normalised text.
fn groups<'a>(pages: &'a [PageText<'a>]) -> HashMap<(Edge, String), Vec<Member<'a>>> {
    let mut groups: HashMap<(Edge, String), Vec<Member<'a>>> = HashMap::new();
    for (page, text) in pages.iter().enumerate() {
        let (low, high) = vertical(text.frame.rotation, text.frame.crop);
        let band = (high - low) * BAND;
        for (object, evidence) in text.objects.iter().enumerate() {
            let Some(extent) = evidence.extent.filter(|_| evidence.chars > 0 && evidence.text.chars().count() < MAX_KEPT_CHARS) else {
                continue;
            };
            let (bottom, top) = vertical(text.frame.rotation, extent);
            let (edge, distance) = if bottom >= high - band && top <= high {
                (Edge::Top, high - top)
            } else if top <= low + band && bottom >= low {
                (Edge::Bottom, bottom - low)
            } else {
                continue;
            };
            let key = normalise(&evidence.text);
            if key.is_empty() || !distance.is_finite() {
                continue;
            }
            groups.entry((edge, key)).or_default().push(Member { page, object, distance, evidence });
        }
    }
    groups
}

/// The clusters of `members` (sorted by distance) that run: the union of every window of members
/// within [`TOLERANCE`] of the window's first that covers at least `needed` pages, with windows that
/// share a member joined into one cluster. A dense run of such windows makes one cluster wider than
/// [`TOLERANCE`], still inside the edge band, and the heading check then judges the cluster as a
/// whole. At most [`MAX_CLUSTERS`] are returned, nearest the edge first.
fn clusters<'m, 'a>(members: &'m [Member<'a>], needed: usize) -> Vec<&'m [Member<'a>]> {
    let mut on_page: HashMap<usize, usize> = HashMap::new();
    let mut found: Vec<&'m [Member<'a>]> = Vec::new();
    let mut current: Option<(usize, usize)> = None;
    let mut end = 0usize;
    for (start, first) in members.iter().enumerate() {
        while let Some(next) = members.get(end).filter(|m| m.distance - first.distance <= TOLERANCE) {
            let n = on_page.entry(next.page).or_default();
            *n = n.saturating_add(1);
            end = end.saturating_add(1);
        }
        if on_page.len() >= needed {
            current = match current {
                Some((from, to)) if start < to => Some((from, to.max(end))),
                done => {
                    if let Some(cluster) = done.and_then(|(from, to)| members.get(from..to)) {
                        found.push(cluster);
                    }
                    if found.len() >= MAX_CLUSTERS {
                        return found;
                    }
                    Some((start, end))
                }
            };
        }
        if let Some(n) = on_page.get_mut(&first.page) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                on_page.remove(&first.page);
            }
        }
    }
    if let Some(cluster) = current.and_then(|(from, to)| members.get(from..to)) {
        found.push(cluster);
    }
    found
}

/// The bottom and top of a user-space box as the page displays it.
fn vertical(rotation: i64, [x0, y0, x1, y1]: [f64; 4]) -> (f64, f64) {
    let (a, b) = match rotation {
        90 => (-x0, -x1),
        180 => (-y0, -y1),
        270 => (x0, x1),
        _ => (y0, y1),
    };
    (a.min(b), a.max(b))
}

/// Lowercase, whitespace collapsed, and each digit run, and a roman numeral standing alone or
/// after `page`, replaced by `#`.
fn normalise(text: &str) -> String {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let roman_alone = match words.as_slice() {
        [w] | ["page", w] => is_roman(w),
        _ => false,
    };
    let mut out = String::new();
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if roman_alone && i + 1 == words.len() {
            out.push('#');
            continue;
        }
        let mut in_digits = false;
        for c in w.chars() {
            if c.is_ascii_digit() {
                if !in_digits {
                    out.push('#');
                }
                in_digits = true;
            } else {
                out.push(c);
                in_digits = false;
            }
        }
    }
    out
}

/// A canonical lowercase roman numeral from 1 to 3999 (`mid` and `iiii` are not).
fn is_roman(s: &str) -> bool {
    roman_value(s).is_some()
}

/// The value of a canonical lowercase roman numeral from 1 to 3999.
fn roman_value(s: &str) -> Option<u32> {
    const UNITS: [(u32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    if s.is_empty() || s.len() > 15 {
        return None;
    }
    let (mut rest, mut value) = (s, 0u32);
    for (v, sym) in UNITS {
        let mut repeats = 0;
        while let Some(r) = rest.strip_prefix(sym) {
            (rest, value, repeats) = (r, value.saturating_add(v), repeats + 1);
            if repeats > 3 {
                return None;
            }
        }
    }
    if !rest.is_empty() || value == 0 {
        return None;
    }
    let (total, mut canonical) = (value, String::new());
    for (v, sym) in UNITS {
        while value >= v {
            canonical.push_str(sym);
            value = value.saturating_sub(v);
        }
    }
    (canonical == s).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LETTER: Frame = Frame { rotation: 0, crop: [0.0, 0.0, 612.0, 792.0] };

    /// One line of 10-unit text at baseline `y`.
    fn line(text: &str, y: f64) -> Evidence {
        Evidence {
            size: 10.0,
            lines: 1,
            chars: text.chars().filter(|c| !c.is_whitespace()).count(),
            letters: true,
            text: text.into(),
            extent: Some([72.0, y, 72.0, y + 10.0]),
            ..Evidence::default()
        }
    }

    fn run(pages: &[Vec<Evidence>], body: Option<f64>) -> Vec<Vec<Option<Edge>>> {
        let text: Vec<PageText<'_>> = pages.iter().map(|objects| PageText { frame: LETTER, objects }).collect();
        find(&text, body)
    }

    #[test]
    fn digits_and_roman_numerals_match_any_number() {
        assert_eq!(normalise("Page 12  of 30"), "page # of #");
        assert_eq!(normalise("Annual Report 2026"), "annual report #");
        assert_eq!(normalise("xiv"), "#");
        assert_eq!(normalise("Page IX"), "page #");
        assert_eq!(normalise("mid"), "mid", "not a canonical numeral");
        assert_eq!(normalise("Chapter iv"), "chapter iv", "a numeral after another word is a word");
        assert!(is_page_number("#") && is_page_number("page # of #") && is_page_number("- # -") && is_page_number("[p. #]"));
        assert!(!is_page_number("chapter #") && !is_page_number("page") && !is_page_number("annual report #"));
        assert!(is_roman("mmmcmxcix") && !is_roman("iiii") && !is_roman("ic") && !is_roman(""));
    }

    #[test]
    fn a_header_and_a_numbered_footer_repeat_but_a_body_line_does_not() {
        let page = |n: usize| vec![line("Annual report", 760.0), line("Repeated body line", 400.0), line(&format!("Page {n}"), 30.0)];
        let found = run(&[page(1), page(2), page(3)], Some(10.0));
        for page in found {
            assert_eq!(page, [Some(Edge::Top), None, Some(Edge::Bottom)]);
        }
    }

    #[test]
    fn a_single_page_has_no_running_text() {
        let found = run(&[vec![line("Annual report", 760.0), line("1", 30.0)]], Some(10.0));
        assert_eq!(found, [[None, None]]);
    }

    #[test]
    fn text_on_too_few_pages_or_at_another_height_is_not_running() {
        // Five pages: the footer on one of them, the header 20 units lower on each.
        let pages: Vec<Vec<Evidence>> = (0..5u32)
            .map(|i| {
                let mut page = vec![line("Header", 760.0 - f64::from(i) * 20.0)];
                if i < 1 {
                    page.push(line("Draft", 30.0));
                }
                page
            })
            .collect();
        assert!(run(&pages, None).iter().flatten().all(Option::is_none));
        // Within the tolerance, alternate headers on two of five pages qualify.
        let pages: Vec<Vec<Evidence>> = (0..5u32).map(|i| vec![line(if i % 2 == 0 { "Left" } else { "Right" }, 760.0 + f64::from(i))]).collect();
        let found = run(&pages, None);
        assert!(found.iter().flatten().all(|e| *e == Some(Edge::Top)), "{found:?}");
    }

    #[test]
    fn distinct_pages_are_counted_not_objects() {
        // Six pages need three. Copies at 30 and 32 on the first page and one at 31 on the second
        // are three objects in a row by height, but only two pages.
        let note = |y: f64| line("Note", y);
        let mut pages = vec![vec![note(30.0), note(32.0)], vec![note(31.0)], Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        assert!(run(&pages, None).iter().flatten().all(Option::is_none));
        pages[2].push(note(31.0));
        assert!(run(&pages, None).iter().flatten().any(Option::is_some), "on three pages it runs");
    }

    #[test]
    fn each_qualifying_cluster_of_heights_counts() {
        // Six pages need three: "Note" 20 units below the top on three pages and 40 below on the
        // other three, plus a second copy on the first page just inside the first cluster.
        let at = |distance: f64| line("Note", 792.0 - 10.0 - distance);
        let pages = vec![vec![at(20.0), at(21.0)], vec![at(20.0)], vec![at(20.0)], vec![at(40.0)], vec![at(40.0)], vec![at(40.0)]];
        let found = run(&pages, None);
        assert!(found.iter().flatten().all(|e| *e == Some(Edge::Top)), "{found:?}");
        // On two pages, a copy every 10 units from 20 down puts 9 clusters in the band, past the cap.
        let column: Vec<Evidence> = (0..12u32).map(|i| at(20.0 + f64::from(i) * 10.0)).collect();
        let found = run(&[column.clone(), column], None);
        let marked = found.iter().flatten().filter(|e| e.is_some()).count();
        assert_eq!(marked, 16, "8 clusters on 2 pages: {found:?}");
    }

    #[test]
    fn overlapping_qualifying_spans_of_heights_both_count() {
        // Six pages need three: "Note" 20 units below the top on two pages, 24 on one and 28 on
        // two. 20..24 and 24..28 each cover three pages and share the copy at 24.
        let at = |distance: f64| line("Note", 792.0 - 10.0 - distance);
        let pages = vec![vec![at(20.0)], vec![at(20.0)], vec![at(24.0)], vec![at(28.0)], vec![at(28.0)], Vec::new()];
        let found = run(&pages, None);
        assert_eq!(found.iter().flatten().filter(|e| **e == Some(Edge::Top)).count(), 5, "{found:?}");
    }

    #[test]
    fn heading_sized_text_that_changes_is_a_heading_not_a_header() {
        let big = |text: &str| Evidence { size: 20.0, ..line(text, 750.0) };
        let found = run(&[vec![big("Chapter 1")], vec![big("Chapter 2")]], Some(10.0));
        assert_eq!(found, [[None], [None]]);
        let found = run(&[vec![big("Annual report")], vec![big("Annual report")]], Some(10.0));
        assert_eq!(found, [[Some(Edge::Top)], [Some(Edge::Top)]], "the same large text on every page is still a header");
        let number = |text: &str| Evidence { letters: false, ..big(text) };
        let found = run(&[vec![number("12")], vec![number("13")]], Some(10.0));
        assert_eq!(found, [[Some(Edge::Top)], [Some(Edge::Top)]], "a large page number is still a page number");
        let found = run(&[vec![big("xii")], vec![big("xiii")]], Some(10.0));
        assert_eq!(found, [[Some(Edge::Top)], [Some(Edge::Top)]], "so is a large roman one");
        let found = run(&[vec![big("Page 1")], vec![big("Page 2")], vec![big("- 3 of 9 -")], vec![big("- 4 of 9 -")]], Some(10.0));
        assert!(found.iter().flatten().all(|e| *e == Some(Edge::Top)), "a labelled page number is too: {found:?}");
    }

    /// Fork guard (PATCHES.md, 'Stricter than upstream on purpose'): amounts in a band are content.
    #[test]
    fn differing_amounts_at_the_same_height_stay_content() {
        let amount = |text: &str| Evidence { letters: false, ..line(text, 60.0) };
        assert_eq!(run(&[vec![amount("1,234.56")], vec![amount("7,890.12")]], Some(10.0)), [[None], [None]]);
        let totals = [vec![line("Total 1,234.56", 60.0)], vec![line("Total 7,890.12", 60.0)], vec![line("Total 55.00", 60.0)]];
        assert_eq!(run(&totals, Some(10.0)), [[None], [None], [None]]);
        // A number that steps with the page but sits beside another that changes is not a page number.
        let mixed = [vec![line("Ref 3 total 40", 60.0)], vec![line("Ref 4 total 75", 60.0)]];
        assert_eq!(run(&mixed, Some(10.0)), [[None], [None]]);
        // Two members on one page cannot both be its page number.
        let doubled = [vec![amount("3"), amount("8")], vec![amount("4")]];
        assert_eq!(run(&doubled, Some(10.0)), [vec![None, None], vec![None]]);
        // The same line on every page is still a repeat.
        let same = [vec![line("Total 55.00", 60.0)], vec![line("Total 55.00", 60.0)]];
        assert_eq!(run(&same, Some(10.0)), [[Some(Edge::Bottom)], [Some(Edge::Bottom)]]);
    }

    #[test]
    fn page_numbers_that_step_with_the_page_are_still_artifacts() {
        let number = |text: &str| Evidence { letters: false, ..line(text, 30.0) };
        let pages: Vec<Vec<Evidence>> = (3..6).map(|n| vec![number(&n.to_string())]).collect();
        assert!(run(&pages, Some(10.0)).iter().flatten().all(|e| *e == Some(Edge::Bottom)), "3, 4, 5");
        let pages: Vec<Vec<Evidence>> = (3..6).map(|n| vec![line(&format!("Page {n} of 9"), 30.0)]).collect();
        assert!(run(&pages, Some(10.0)).iter().flatten().all(|e| *e == Some(Edge::Bottom)), "Page n of 9");
        let pages: Vec<Vec<Evidence>> = ["iii", "iv", "v"].iter().map(|n| vec![line(n, 30.0)]).collect();
        assert!(run(&pages, Some(10.0)).iter().flatten().all(|e| *e == Some(Edge::Bottom)), "roman");
        // Pages 1 and 3 numbered, page 2 not: the number still steps by the page index.
        let pages = vec![vec![number("7")], Vec::new(), vec![number("9")]];
        assert_eq!(run(&pages, Some(10.0)), [vec![Some(Edge::Bottom)], vec![], vec![Some(Edge::Bottom)]]);
        // A labelled header beside its page number: the label stays, the number steps.
        let pages: Vec<Vec<Evidence>> = (1..4).map(|n| vec![line(&format!("Annual report 2026, page {n}"), 760.0)]).collect();
        assert!(run(&pages, Some(10.0)).iter().flatten().all(|e| *e == Some(Edge::Top)));
        assert_eq!(numbers("Page xiv of 20"), Some(vec![20]), "a numeral not standing alone is a word");
        assert_eq!(numbers("Page xiv"), Some(vec![14]));
        assert_eq!(numbers("p. 1,234.5"), Some(vec![1, 234, 5]));
        assert_eq!(numbers(&"9".repeat(40)), None, "too long to be a page number");
    }

    #[test]
    fn a_heading_differing_only_in_case_or_spacing_does_not_change() {
        let big = |text: &str| Evidence { size: 18.0, ..line(text, 750.0) };
        let found = run(&[vec![big("ANNUAL REPORT")], vec![big("Annual  report")]], Some(12.0));
        assert_eq!(found, [[Some(Edge::Top)], [Some(Edge::Top)]]);
    }

    #[test]
    fn the_bands_follow_the_displayed_page() {
        // Turned 90° clockwise, the displayed top is the user-space left edge (x = 0).
        let turned = Frame { rotation: 90, crop: [0.0, 0.0, 612.0, 792.0] };
        let side = Evidence { extent: Some([20.0, 300.0, 30.0, 300.0]), ..line("Side title", 0.0) };
        let objects = vec![side];
        let pages = [PageText { frame: turned, objects: &objects }, PageText { frame: turned, objects: &objects }];
        assert_eq!(find(&pages, None), [[Some(Edge::Top)], [Some(Edge::Top)]]);
        assert_eq!(run(&[objects.clone(), objects], None), [[None], [None]], "upright, it sits mid-page");
        // Upside down the displayed top is the user-space bottom; turned 270° it is the right edge.
        let upside = Frame { rotation: 180, ..turned };
        let low = vec![line("Upside down", 30.0)];
        assert_eq!(
            find(&[PageText { frame: upside, objects: &low }, PageText { frame: upside, objects: &low }], None),
            [[Some(Edge::Top)], [Some(Edge::Top)]]
        );
        let left = Frame { rotation: 270, ..turned };
        let right = vec![Evidence { extent: Some([590.0, 300.0, 600.0, 300.0]), ..line("Turned left", 0.0) }];
        assert_eq!(
            find(&[PageText { frame: left, objects: &right }, PageText { frame: left, objects: &right }], None),
            [[Some(Edge::Top)], [Some(Edge::Top)]]
        );
    }

    #[test]
    fn the_bands_follow_the_crop_box() {
        // Cropped to y 100..892, text at 860 is in the top band and text at 760 is not.
        let cropped = Frame { rotation: 0, crop: [0.0, 100.0, 612.0, 892.0] };
        let objects = vec![line("Inside", 860.0), line("Below the band", 760.0)];
        let pages = [PageText { frame: cropped, objects: &objects }, PageText { frame: cropped, objects: &objects }];
        assert_eq!(find(&pages, None), [[Some(Edge::Top), None], [Some(Edge::Top), None]]);
        assert_eq!(run(&[objects.clone(), objects], None)[0], [None, Some(Edge::Top)], "against the media box it is the other way");
    }
}
