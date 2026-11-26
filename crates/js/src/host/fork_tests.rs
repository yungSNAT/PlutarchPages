//! The fork's tests of the script host's budgets and field values.

use super::*;
use crate::fork_test_support::bare_host;

#[test]
fn a_console_line_past_the_byte_budget_is_dropped_and_counted() {
    let mut h = Host { kept: xfa::MAX_PAYLOAD, ..bare_host() };
    h.print("overflow".into());
    assert!(h.console.is_empty());
    assert_eq!(h.dropped, 1);
}

#[test]
fn a_long_focus_name_is_clipped_before_it_is_kept() {
    let mut h = bare_host();
    assert!(h.request(Request::Focus("f".repeat(5000))));
    let Some(Request::Focus(name)) = h.requests.first() else { panic!("no focus request: {:?}", h.requests) };
    assert!(name.ends_with('…') && name.len() < 5000, "{} bytes", name.len());
}

#[test]
fn no_colour_is_transparent_and_a_multi_select_list_reads_as_an_array() {
    let mut ctx = Context::default();
    let trans = colour(None, &mut ctx);
    assert_eq!(text(&trans, &mut ctx).ok().as_deref(), Some("T"));
    let list = FieldState::new("multi", FieldType::ListBox, vec!["a".into(), "b".into()]);
    let val = value_of(&list, &mut ctx);
    assert!(val.as_object().is_some_and(|o| o.is_array()));
    assert_eq!(text(&val, &mut ctx).ok().as_deref(), Some("a,b"));
}
