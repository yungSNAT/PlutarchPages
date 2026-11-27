//! The fork's tests of FormCalc number text.

use super::*;

#[test]
fn non_finite_numbers_print_by_name() {
    assert_eq!(num_text(f64::NAN), "NaN");
    assert_eq!(num_text(f64::INFINITY), "Infinity");
    assert_eq!(num_text(f64::NEG_INFINITY), "-Infinity");
}

#[test]
fn numbers_read_signed_exponents_and_stop_at_trailing_text() {
    assert_eq!(parse_num("1.5e+2"), Some(150.0));
    assert_eq!(parse_num("2.5E-1"), Some(0.25));
    assert_eq!(parse_num("3e2xyz"), Some(300.0));
    assert_eq!(parse_num("  "), None);
}
