//! The fork's tests of the FormCalc lexer.

use super::*;

#[test]
fn strings_span_lines_and_numbers_take_signed_exponents() {
    let toks = lex("\"line1\nline2\"\n1.5e+2 2.5E-1").unwrap_or_default();
    let found: Vec<(&Tok, usize)> = toks.iter().map(|t| (&t.tok, t.line)).collect();
    assert!(found.contains(&(&Tok::Str("line1\nline2".into()), 1)), "{found:?}");
    // The string's own line break counts: the numbers are on line 3.
    assert!(found.contains(&(&Tok::Num(150.0), 3)), "{found:?}");
    assert!(found.contains(&(&Tok::Num(0.25), 3)), "{found:?}");
}

#[test]
fn a_string_past_the_length_limit_is_an_error() {
    let too_long = format!("\"{}\"", "a".repeat(MAX_STRING + 1));
    assert!(lex(&too_long).is_err_and(|e| e.contains("too long")));
    let at_limit = format!("\"{}\"", "a".repeat(MAX_STRING));
    assert!(lex(&at_limit).is_ok());
}
