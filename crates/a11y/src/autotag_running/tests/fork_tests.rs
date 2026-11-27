// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

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
    // A labelled number that steps by one a page is still an amount or a row, not a page number.
    for texts in [
        ["Total 100", "Total 101", "Total 102"],
        ["Total 1.50", "Total 2.50", "Total 3.50"],
        ["Total $1,001.00", "Total $1,002.00", "Total $1,003.00"],
        ["Item 1 Widgets 40.00", "Item 2 Widgets 40.00", "Item 3 Widgets 40.00"],
        ["$1", "$2", "$3"],
        ["(500)", "(501)", "(502)"],
        ["-500", "-501", "-502"],
        ["\u{2212}500", "\u{2212}501", "\u{2212}502"],
        ["500-", "501-", "502-"],
        ["-1", "-2", "-3"],
        ["\u{2013}500", "\u{2013}501", "\u{2013}502"],
        ["500\u{2013}", "501\u{2013}", "502\u{2013}"],
        ["\u{2014}500", "\u{2014}501", "\u{2014}502"],
        ["\u{2013} 500", "\u{2013} 501", "\u{2013} 502"],
        ["[-500]", "[-501]", "[-502]"],
        ["[\u{2013}500]", "[\u{2013}501]", "[\u{2013}502]"],
    ] {
        let pages: Vec<Vec<Evidence>> = texts.iter().map(|t| vec![line(t, 60.0)]).collect();
        assert_eq!(run(&pages, Some(10.0)), [[None], [None], [None]], "{texts:?}");
    }
    // The same amount on every page stays content.
    for text in [
        "Total 55.00",
        "Amount due $1,234.56",
        "Balance 300",
        "Subtotal 12",
        "\u{20ac}40",
        "(500)",
        "-500",
        "AUD 500",
        "500 AUD",
        "\u{ff04}500",
        "\u{ffe5}500",
        "Totals 300",
        "1\u{202f}234",
        "1'234",
        "B/F 300",
        "Summe 300",
        "300",
        "GST 10%",
        "Invoice INV-1001",
        "\u{0e3f}2026",
        "-2026",
        "\u{2212}2026",
        "(2026)",
        "Balance (1999)",
    ] {
        let same = [vec![line(text, 60.0)], vec![line(text, 60.0)]];
        assert_eq!(run(&same, Some(10.0)), [[None], [None]], "{text}");
    }
    let same = [vec![amount("1,234.56")], vec![amount("1,234.56")]];
    assert_eq!(run(&same, Some(10.0)), [[None], [None]]);
}

#[test]
fn the_same_line_without_an_amount_still_repeats() {
    for text in ["Annual report 2026", "(c) 2026 Firm", "Confidential", "1999 to 2026"] {
        let same = [vec![line(text, 760.0)], vec![line(text, 760.0)]];
        assert_eq!(run(&same, Some(10.0)), [[Some(Edge::Top)], [Some(Edge::Top)]], "{text}");
    }
}

/// The known limit: an unsigned amount from 1900 to 2099 repeated on every page reads as a year.
#[test]
fn total_2000_on_every_page_runs_as_a_year() {
    let same = [vec![line("Total 2000", 760.0)], vec![line("Total 2000", 760.0)]];
    assert_eq!(run(&same, Some(10.0)), [[Some(Edge::Top)], [Some(Edge::Top)]]);
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
    let pages: Vec<Vec<Evidence>> = (1..4).map(|n| vec![line(&format!("- {n} -"), 30.0)]).collect();
    assert!(run(&pages, Some(10.0)).iter().flatten().all(|e| *e == Some(Edge::Bottom)), "- n -");
    let pages: Vec<Vec<Evidence>> = (1..4).map(|n| vec![line(&format!("\u{2013} {n} \u{2013}"), 30.0)]).collect();
    assert!(run(&pages, Some(10.0)).iter().flatten().all(|e| *e == Some(Edge::Bottom)), "– n –");
    let pages: Vec<Vec<Evidence>> = (1..4).map(|n| vec![line(&format!("[- {n} -]"), 30.0)]).collect();
    assert!(run(&pages, Some(10.0)).iter().flatten().all(|e| *e == Some(Edge::Bottom)), "[- n -]");
    // Round brackets also mark negative amounts, so a changing `(n)` fails towards content.
    let pages: Vec<Vec<Evidence>> = (1..4).map(|n| vec![line(&format!("({n})"), 30.0)]).collect();
    assert!(run(&pages, Some(10.0)).iter().flatten().all(Option::is_none), "(n)");
    let pages: Vec<Vec<Evidence>> = (1..4).map(|n| vec![line(&format!("[{n}]"), 30.0)]).collect();
    assert!(run(&pages, Some(10.0)).iter().flatten().all(|e| *e == Some(Edge::Bottom)), "[n]");
    // A page number inside other words fails towards content: only page words may label it.
    let pages: Vec<Vec<Evidence>> = (1..4).map(|n| vec![line(&format!("Annual report 2026, page {n}"), 760.0)]).collect();
    assert!(run(&pages, Some(10.0)).iter().flatten().all(Option::is_none));
    assert_eq!(numbers("Page xiv of 20"), Some(vec![20]), "a numeral not standing alone is a word");
    assert_eq!(numbers("Page xiv"), Some(vec![14]));
    assert_eq!(numbers("p. 1,234.5"), Some(vec![1, 234, 5]));
    assert_eq!(numbers(&"9".repeat(40)), None, "too long to be a page number");
}
