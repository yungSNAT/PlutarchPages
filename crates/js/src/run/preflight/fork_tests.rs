//! The fork's tests of the syntax preflight.

use super::*;

#[test]
fn escaped_backticks_and_optional_chaining_pass_and_oversized_source_is_refused() {
    assert_eq!(refuse("var x = `escaped \\` tick`; var y = x?.length;"), None);
    assert_eq!(refuse("var t = `a${ `b${1}` }c`;"), None);
    assert!(refuse(&"a".repeat(MAX_SCRIPT_BYTES + 1)).is_some_and(|m| m.contains("too long")));
}
