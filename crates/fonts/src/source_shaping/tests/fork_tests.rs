//! The fork's bounds on [`drawn_like_page`]: a page font whose layout is too large to size, or too
//! large for the text it would shape, is refused before any shaping.

use super::*;

/// An sfnt holding only a GSUB of `lookups` lookups, each with one `SingleSubst` subtable.
fn gsub_only(lookups: u16) -> Vec<u8> {
    with_table(&[0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], b"GSUB", aliased_single_gsub(lookups, 1, 1))
}

/// With no page words and text that joins nothing, nothing is shaped, so only the bounds decide.
fn passes(program: &[u8], text: &str) -> bool {
    drawn_like_page(program, program, &[], text)
}

#[test]
fn a_page_font_with_more_than_4096_gsub_lookups_is_refused() {
    let text = "a".repeat(8);
    assert!(passes(&gsub_only(4096), &text), "4096 lookups can be sized");
    assert!(!passes(&gsub_only(4097), &text), "4097 lookups can't");
}

#[test]
fn a_layout_too_large_for_the_text_it_would_shape_is_refused() {
    // 4096 lookups times 32768 bytes is the bound, 1 << 27; one byte more passes it.
    let program = gsub_only(4096);
    assert!(passes(&program, &"a".repeat(32_768)), "at the bound");
    assert!(!passes(&program, &"a".repeat(32_769)), "past the bound");
}
