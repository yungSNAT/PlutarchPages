//! The fork's tests of the nesting guard.

use super::*;

#[test]
fn template_braces_escapes_and_unicode_line_breaks_are_not_nesting() {
    assert!(!too_deep("`${ { a: 1 } }\\`ok`"));
    assert!(!too_deep("// comment\r\n'a\\\r\nb'"));
    assert!(!too_deep("// line\u{2028}1\n// para\u{2029}2"));
}

#[test]
fn source_with_too_many_possible_readings_is_refused() {
    // Each `/` may start a regular expression or divide, and each `${` may open a substitution:
    // the readings fork past the cap, so the script is refused rather than read every way.
    assert!(too_deep(&"/`${".repeat(20)));
}
