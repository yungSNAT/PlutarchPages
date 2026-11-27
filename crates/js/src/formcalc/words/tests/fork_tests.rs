//! The fork's tests of `WordNum` and entity decoding.

use super::super::word_num;
use super::*;

#[test]
fn word_num_covers_round_numbers_and_the_supported_range() {
    assert_eq!(word_num(20.0, false), "Twenty");
    assert_eq!(word_num(0.0, false), "Zero");
    assert_eq!(word_num(1_000.0, false), "One Thousand");
    assert_eq!(word_num(2_000_000_005.0, false), "Two Billion Five");
    // Outside 0 to just under a trillion there are no words.
    for n in [-1.0, f64::NAN, f64::INFINITY, 1e12] {
        assert_eq!(word_num(n, false), "", "{n}");
    }
    // Rounding to cents can carry past the range.
    assert_eq!(word_num(999_999_999_999.999, true), "");
}

#[test]
fn an_unterminated_numeric_reference_stays_literal() {
    assert_eq!(decode_entities("&#65", false), "&#65");
    assert_eq!(decode_entities("&#x41", true), "&#x41");
    assert_eq!(decode_entities("&#65;", false), "A");
}
