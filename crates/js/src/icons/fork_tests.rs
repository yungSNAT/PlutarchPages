//! The fork's tests of button icon handles.

use crate::{DocInfo, Event, FieldState, FieldType, Limits, Outcome, run};

fn go(script: &str, fields: &[FieldState]) -> Outcome {
    run(script, &Event::doc("Console"), &DocInfo::default(), fields, &[], Limits::default())
}

fn button(name: &str, icon: u64) -> FieldState {
    let mut b = FieldState::new(name, FieldType::Button, vec![]);
    b.icon = Some(icon);
    b
}

#[test]
fn only_the_normal_face_of_a_push_button_has_an_icon() {
    let fields = [button("btn", 42), FieldState::new("txt", FieldType::Text, vec![])];
    for script in ["getField('btn').buttonGetIcon(1);", "getField('btn').buttonSetIcon(null, 1);"] {
        let out = go(script, &fields);
        assert!(out.error.as_deref().is_some_and(|e| e.contains("normal icon face 0")), "{script}: {:?}", out.error);
    }
    let out = go("getField('txt').buttonGetIcon();", &fields);
    assert!(out.error.as_deref().is_some_and(|e| e.contains("push button")), "{:?}", out.error);
}

#[test]
fn one_icon_keeps_one_handle_and_handles_are_limited() {
    let fields = [button("btn", 42)];
    assert_eq!(go("getField('btn').buttonGetIcon() === getField('btn').buttonGetIcon();", &fields).result.as_deref(), Some("true"));

    let many: Vec<FieldState> = (0..4097u64).map(|i| button(&format!("b{i}"), i + 1)).collect();
    let out = go("for (var i = 0; i < 4097; i++) getField('b' + i).buttonGetIcon();", &many);
    assert!(out.error.as_deref().is_some_and(|e| e.contains("too many icon handles")), "{:?}", out.error);
    // The 4096th is still handed out.
    assert_eq!(go("for (var i = 0; i < 4096; i++) getField('b' + i).buttonGetIcon(); 'ok';", &many).result.as_deref(), Some("ok"));
}

#[test]
fn a_button_without_an_icon_has_none_and_setting_null_clears_one() {
    let fields = [FieldState::new("plain", FieldType::Button, vec![]), button("btn", 42)];
    assert_eq!(go("String(getField('plain').buttonGetIcon());", &fields).result.as_deref(), Some("null"));
    let cleared = go("var b = getField('btn'); b.buttonSetIcon(null); String(b.buttonGetIcon());", &fields);
    assert_eq!((cleared.error, cleared.result.as_deref()), (None, Some("null")));
}
