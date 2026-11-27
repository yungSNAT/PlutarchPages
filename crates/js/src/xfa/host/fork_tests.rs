//! The fork's tests of the XFA host's snapshot and effect budgets.

use super::*;

fn one_field_host() -> XHost {
    let root = Arc::new(XfaNode {
        name: "form1".into(),
        som: "form1[0]".into(),
        kind: Some(XfaKind::Subform),
        children: vec![XfaNode {
            name: "f".into(),
            som: "form1[0].f[0]".into(),
            kind: Some(XfaKind::Field),
            formatted: Some("old_fmt".into()),
            presence: "visible".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    XHost::new(root, false, "form1[0].f[0]", &XfaDoc::default())
}

#[test]
fn flattening_stops_at_the_depth_limit() {
    let mut deep = XfaNode { name: "leaf".into(), som: "leaf".into(), kind: Some(XfaKind::Field), ..Default::default() };
    for i in (0..=(MAX_DEPTH + 2)).rev() {
        deep = XfaNode { name: format!("s{i}"), som: format!("s{i}"), kind: Some(XfaKind::Subform), children: vec![deep], ..Default::default() };
    }
    let nodes = flattened(&deep);
    assert_eq!(nodes.len(), MAX_DEPTH + 1);
    assert!(nodes.iter().all(|n| &*n.name != "leaf"));
}

#[test]
fn a_new_value_drops_the_formatted_text_and_other_effects_leave_state_alone() {
    let mut h = one_field_host();
    let field = h.current;
    assert!(h.set_value(field, "new".into()));
    assert_eq!(h.nodes.get(field).and_then(|n| n.formatted.as_deref()), None);

    h.set_state(field, XfaEffect::Beep);
    assert!(h.effects.contains(&XfaEffect::Beep));
    assert_eq!(h.nodes.get(field).map(|n| n.presence.to_string()).as_deref(), Some("visible"));
}

#[test]
fn long_values_are_cut_effects_past_the_limit_are_dropped_and_console_bytes_are_budgeted() {
    let mut h = one_field_host();
    h.push(XfaEffect::SetValue { som: "form1[0].f[0]".into(), value: "a".repeat(MAX_VALUE + 5) });
    assert!(h.notes().iter().any(|n| n.contains("cut")), "{:?}", h.notes());

    h.effects.resize(MAX_EFFECTS, XfaEffect::Beep);
    h.record(XfaEffect::SetValue { som: "form1[0].other[0]".into(), value: "v".into() });
    assert_eq!(h.effects.len(), MAX_EFFECTS);
    assert!(h.notes().iter().any(|n| n.contains("changes")), "{:?}", h.notes());

    h.kept = MAX_PAYLOAD;
    h.print("overflow".into());
    assert!(h.console.is_empty());
    assert!(h.notes().iter().any(|n| n.contains("MiB")), "{:?}", h.notes());
}
