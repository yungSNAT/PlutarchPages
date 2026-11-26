//! The fork's tests of FormCalc calendar helpers.

use super::*;

#[test]
fn month_lengths_follow_the_calendar_and_an_invalid_month_keeps_31_days() {
    assert_eq!((days_in(2024, 2), days_in(2023, 2), days_in(1900, 2), days_in(2000, 2)), (29, 28, 28, 29));
    assert_eq!((days_in(2024, 4), days_in(2024, 12)), (30, 31));
    assert_eq!((days_in(2024, 0), days_in(2024, 13)), (31, 31));
}

#[test]
fn date_pictures_cover_short_year_month_day_and_weekday_tokens() {
    assert_eq!(format_date(2024, 3, 5, "YY-M-D MMM"), "24-3-5 Mar");
    // 9 November 2024 was a Saturday: day 7 counting from Sunday.
    assert_eq!(format_date(2024, 11, 9, "Y/MM/DD EEE EEEE E"), "24/11/09 Sat Saturday 7");
    assert_eq!(format_date(2024, 11, 9, "YYYY MMMM"), "2024 November");
}
