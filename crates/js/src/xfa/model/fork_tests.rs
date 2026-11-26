//! The fork's tests of the XFA model helpers.

use super::*;

#[test]
fn every_kind_has_its_class_name() {
    let kinds = [
        (XfaKind::Form, "form"),
        (XfaKind::Subform, "subform"),
        (XfaKind::Field, "field"),
        (XfaKind::ExclGroup, "exclGroup"),
        (XfaKind::Draw, "draw"),
        (XfaKind::Area, "area"),
    ];
    for (kind, name) in kinds {
        assert_eq!(kind.class_name(), name);
    }
}

#[test]
fn instance_effects_weigh_their_som() {
    let som = "form1[0].row[1]";
    assert_eq!(payload(&XfaEffect::AddInstance { som: som.into() }), som.len());
    assert_eq!(payload(&XfaEffect::RemoveInstance { som: som.into() }), som.len());
}

#[test]
fn reset_names_stop_at_the_node_limit() {
    let list: Vec<String> = (0..=MAX_NODES).map(|i| format!("f{i}")).collect();
    let names = reset_names(&list.join(","));
    assert_eq!(names.len(), MAX_NODES);
    assert_eq!(names.last().map(String::as_str), Some(format!("f{}", MAX_NODES - 1).as_str()));
}
