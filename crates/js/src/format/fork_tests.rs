//! The fork's tests of `util.printf`, `printd`, `printx` and `scand`.

use super::*;
use crate::{DocInfo, Event, Limits, Outcome, run};

fn go(script: &str) -> Outcome {
    run(script, &Event::doc("Console"), &DocInfo::default(), &[], &[], Limits::default())
}

#[test]
fn printf_space_sign_alternate_form_and_comma_decimal_without_grouping() {
    let out = go("util.printf('% d|%#x|%,3.2f', 5, 255, 1234.5);");
    assert_eq!((out.error, out.result.as_deref()), (None, Some(" 5|FF|1234,50")));
}

#[test]
fn printd_numbered_formats_and_twelve_hour_clock() {
    let out = go("var d0 = new Date(2024, 0, 1, 0, 5, 9); \
         var d1 = new Date(2024, 0, 1, 15, 5, 9); \
         [util.printd(0, d0), util.printd(1, d0), util.printd(2, d0), util.printd('ddd h:MM:ss t tt', d0), util.printd('ddd h:MM:ss t tt', d1)].join('|')");
    assert_eq!(out.error, None, "{:?}", out.error);
    // Midnight is 12 am; 15:05 is 3 pm.
    assert_eq!(out.result.as_deref(), Some("D:20240101000509|2024.01.01 00:05:09|1/1/24 12:05:09 am|Mon 12:05:09 a am|Mon 3:05:09 p pm"));
    let bad = go("util.printd('yyyy', new Date(NaN));");
    assert!(bad.error.as_deref().is_some_and(|e| e.contains("invalid date")), "{:?}", bad.error);
}

#[test]
fn printx_masks_cases_escapes_and_running_out_of_source() {
    let out = go("[util.printx('?X-A9*\\\\9', '!1B2rest'), util.printx('>A<A=A', 'aBc'), util.printx('?XA9', '')].join('|')");
    assert_eq!(out.error, None, "{:?}", out.error);
    // `*` copies the rest, `\9` is a literal 9; `>` and `<` set the case until `=`.
    assert_eq!(out.result.as_deref(), Some("!1-B2rest9|Abc|"));
}

#[test]
fn english_names_skip_empty_and_overlong_names() {
    let overlong = "x".repeat(MAX_NAME_CHARS + 1);
    assert_eq!(english_names("Jan 2024", "yyyy", [&[""], &[overlong.as_str()], &[], &[]]), "January 2024");
}

#[test]
fn a_month_name_slot_takes_no_number_and_a_bad_meridiem_fails_the_scan() {
    let mut sv = ScanValues::default();
    sv.set(DateUnit::MonthName, 3);
    assert_eq!(sv.mo, None);
    assert_eq!(scan_date("yyyy-mm-dd tt", "2024-01-01 xx", 2024), None);
    assert!(scan_date("yyyy-mm-dd tt", "2024-01-01 pm", 2024).is_some());
}

#[test]
fn folding_treats_dotted_and_dotless_i_and_final_sigma_alike() {
    assert_eq!([fold('I'), fold('ı'), fold('İ'), fold('ς')], ["i", "i", "i", "σ"]);
}

#[test]
fn a_dotted_short_name_touching_digits_is_spaced_apart() {
    assert_eq!(english_names("10mar.2024", "ddmmmyyyy", [&["jan.", "feb.", "mar."], &[], &[], &[]]), "10 March 2024");
}
