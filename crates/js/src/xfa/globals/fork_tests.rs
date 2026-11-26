//! The fork's tests of the XFA script globals.

use crate::Limits;
use crate::xfa::{XfaDoc, XfaEffect, XfaEvent, XfaKind, XfaNode, run_xfa};

#[test]
fn menu_items_relayout_response_and_layout_measures() {
    let root = XfaNode { name: "form1".into(), som: "form1[0]".into(), kind: Some(XfaKind::Subform), ..Default::default() };
    let event = XfaEvent { activity: "click".into(), target: "form1[0]".into(), ..Default::default() };
    let out = run_xfa(
        "app.execMenuItem('Print'); app.execMenuItem('NoSuchMenu'); \
         xfa.layout.relayout(); xfa.layout.relayoutPageArea(); xfa.remerge(); \
         [xfa.host.response('q'), app.response('q'), xfa.layout.h(this), xfa.layout.w(this), xfa.layout.x(this), xfa.layout.y(this), xfa.layout.pageSpan(this)].join('|')",
        &event,
        &XfaDoc::default(),
        &root,
        Limits::default(),
    );
    assert_eq!(out.error, None);
    // No one to answer a prompt, and no layout to measure.
    assert_eq!(out.result.as_deref(), Some("||0|0|0|0|0"));
    // An unknown menu item does nothing; Print prints once.
    assert_eq!(out.effects.iter().filter(|e| matches!(e, XfaEffect::Print)).count(), 1);
    assert_eq!(out.effects.iter().filter(|e| matches!(e, XfaEffect::Relayout)).count(), 3);
}
