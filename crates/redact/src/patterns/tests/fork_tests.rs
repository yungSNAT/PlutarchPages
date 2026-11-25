//! The fork's Australian presets: tax file numbers and business numbers, matched by format.

use super::*;

#[test]
fn tax_file_numbers_match_together_or_in_groups_of_three() {
    assert_eq!(
        found(Pattern::Tfn, "TFN 123 456 782, TFN: 876-543-210 and 459599230."),
        ["123 456 782", "876-543-210", "459599230"]
    );
    // By format alone: a number the usual check rejects is marked too.
    assert_eq!(found(Pattern::Tfn, "Quoted 123 456 789 on the form"), ["123 456 789"]);
}

#[test]
fn tax_file_numbers_are_whole_numbers_only() {
    // Part of a longer run, mixed separators, uneven groups or 8 and 10 digits: not a TFN.
    assert_eq!(
        found(
            Pattern::Tfn,
            "id 1234567890, card 4111 111 111 111, phone +61 412 345 678, mixed 123 456-789, groups 12 345 6789, short 12345678"
        ),
        Vec::<String>::new()
    );
}

#[test]
fn business_numbers_match_the_register_example_and_its_layouts() {
    // The Australian Business Register's own worked example, as printed and written together.
    assert_eq!(found(Pattern::Abn, "ABN 51 824 753 556 (51824753556)"), ["51 824 753 556", "51824753556"]);
    // A mistyped number still looks like one, so it is marked as well.
    assert_eq!(found(Pattern::Abn, "ABN 51 824 753 557"), ["51 824 753 557"]);
}

#[test]
fn business_numbers_are_whole_numbers_only() {
    assert_eq!(
        found(Pattern::Abn, "acct 518247535561, 51-824-753-556, 518 247 535 56, 51 824 753 556 7, tel 02 9374 4000"),
        Vec::<String>::new()
    );
}

#[test]
fn the_australian_presets_are_registered_by_id() {
    assert_eq!(Pattern::from_id("tfn"), Some(Pattern::Tfn));
    assert_eq!(Pattern::from_id("abn"), Some(Pattern::Abn));
    assert!(PATTERNS.contains(&Pattern::Tfn) && PATTERNS.contains(&Pattern::Abn));
}
