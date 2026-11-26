//! The fork's tests of XFA node objects.

use std::sync::Arc;

use super::*;
use crate::Limits;
use crate::xfa::{XfaDoc, XfaEffect, XfaEvent, XfaKind, XfaNode, XfaProp, XfaProps, run_xfa};

fn sample_form() -> XfaNode {
    let row0 = XfaNode {
        name: "row".into(),
        som: "form1[0].row[0]".into(),
        kind: Some(XfaKind::Subform),
        repeatable: true,
        index: 0,
        children: vec![XfaNode {
            name: "border".into(),
            som: "form1[0].row[0].border[0]".into(),
            kind: Some(XfaKind::Field),
            value: "childBorder".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let row1 = XfaNode {
        name: "row".into(),
        som: "form1[0].row[1]".into(),
        kind: Some(XfaKind::Subform),
        repeatable: true,
        index: 1,
        props: Arc::new(XfaProps { attrs: vec![], elements: vec![XfaProp { tag: "border".into(), ..Default::default() }] }),
        ..Default::default()
    };
    let field = XfaNode {
        name: "f".into(),
        som: "form1[0].f[0]".into(),
        kind: Some(XfaKind::Field),
        value: String::new(),
        presence: "visible".into(),
        access: "open".into(),
        ..Default::default()
    };
    XfaNode { name: "form1".into(), som: "form1[0]".into(), kind: Some(XfaKind::Subform), children: vec![row0, row1, field], ..Default::default() }
}

#[test]
fn xfa_nodes_cover_sink_has_members_setters_noop_and_shadowed_resolve() {
    let root = sample_form();
    let event = XfaEvent { activity: "click".into(), target: "form1[0].f[0]".into(), ..Default::default() };
    let out = run_xfa(
        "var sk = this.h; sk.nested = 1; var skVal = sk.value; var skSym = sk[Symbol.toStringTag]; var skDeep = sk.a.b.toString; \
         var hasRaw = 'rawValue' in this; \
         var selfRes = this.resolveNode('').name; \
         var emptyStep = this.resolveNode('..[0]'); \
         var par = this.resolveNode('parent').name; \
         var rootPar = xfa.form.parent; \
         var allLen = this.all.length; var rootAll = xfa.form.all.length; \
         var rootIm = xfa.form.instanceManager; \
         this.remerge(); this.execEvent('click'); \
         var len = this.length; var undef = this.noSuchMember; \
         this[Symbol('k')] = 'v'; this.access = 'bogus'; this.unknownWrite = 'ignored'; \
         var shadowed = this.parent.resolveNode('row[*].border').rawValue; \
         [skVal, String(skSym), skDeep, hasRaw, selfRes, String(emptyStep), par, String(rootPar), this.isNull, allLen, rootAll, String(rootIm), String(len), String(undef), shadowed].join('|')",
        &event,
        &XfaDoc::default(),
        &root,
        Limits::default(),
    );
    assert_eq!(out.error, None, "{:?}", out.error);
    assert_eq!(out.result.as_deref(), Some("|undefined||true|f|null|form1|null|true|1|1|undefined|undefined|undefined|childBorder"));
    assert!(out.effects.contains(&XfaEffect::Relayout));
}

#[test]
fn direct_node_index_and_descendants_cover_error_and_gone_paths() {
    let mut ctx = Context::default();
    let bad = ObjectInitializer::new(&mut ctx).property(JsString::from(IDX), JsValue::from(-1.0), Attribute::READONLY).build();
    assert!(node_index(&bad.into(), &mut ctx).is_err());

    let mut h = XHost::new(Arc::new(sample_form()), false, "form1[0]", &XfaDoc::default());
    assert!(descendants_named(&h, 0, "f", MAX_DEPTH + 1).is_empty());
    if let Some(last) = h.nodes.last_mut() {
        last.gone = true;
    }
    assert!(descendants_named(&h, 0, "f", 0).is_empty());
}

#[test]
fn a_node_converts_to_its_value() {
    let root = crate::fork_test_support::form1(vec![crate::fork_test_support::field("g", "form1[0].g[0]", "abc", false)]);
    let event = XfaEvent { activity: "click".into(), target: "form1[0].g[0]".into(), ..Default::default() };
    let out = run_xfa("[this.toString(), this.valueOf(), this + '!'].join('|')", &event, &XfaDoc::default(), &root, Limits::default());
    assert_eq!((out.error, out.result.as_deref()), (None, Some("abc|abc|abc!")));
}
