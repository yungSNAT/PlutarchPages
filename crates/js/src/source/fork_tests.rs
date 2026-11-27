//! The fork's tests of `toSource`.

use crate::{DocInfo, Event, Limits, run};

fn eval(script: &str) -> Option<String> {
    let out = run(script, &Event::doc("Console"), &DocInfo::default(), &[], &[], Limits::default());
    assert_eq!(out.error, None, "{script}: {:?}", out.error);
    out.result
}

#[test]
fn to_source_skips_symbols_names_index_keys_and_escapes_special_characters() {
    let made = "var o = { 0: 'zero', a: 'a\\\\b\\rc\\td\\u2028e\\u2029f\\x01' }; o[Symbol('s')] = 99;";
    assert_eq!(eval(&format!("{made} o.toSource();")).as_deref(), Some("({\"0\":\"zero\", \"a\":\"a\\\\b\\rc\\td\\u2028e\\u2029f\\u0001\"})"));
}
