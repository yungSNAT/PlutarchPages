//! The fork's tests of XFA property objects.

use std::sync::Arc;

use super::super::{self as props, XHost};
use super::*;

fn rich_form() -> XfaNode {
    let mut root = form();
    root.children.push(node(
        "rich",
        XfaKind::Field,
        &[],
        vec![
            p("border", &[], vec![p("fill", &[], vec![p("radial", &[], vec![]), p("stipple", &[], vec![])])]),
            p("desc", &[], vec![t("text", &[], "hello desc")]),
        ],
    ));
    root.children.push(node("stip", XfaKind::Field, &[], vec![p("border", &[], vec![p("fill", &[], vec![p("stipple", &[], vec![])])])]));
    root.children.push(node("edge1", XfaKind::Field, &[], vec![p("border", &[], vec![t("edge", &[("stroke", "solid")], "edgeText")])]));
    root
}

#[test]
fn radial_and_stipple_defaults_and_desc_text() {
    let root = rich_form();
    assert_eq!(eval_on(&root, "this.border.fill.radial.type + '|' + this.desc.rawValue", "rich"), "toEdge|hello desc");
    assert_eq!(eval_on(&root, "this.border.fill.stipple.color.value", "stip"), "0,0,0");
}

#[test]
fn empty_proxies_symbols_reserved_names_and_list_bounds() {
    let script = "\
        var e = this.border.nonExistent; \
        var eEl = e.getElement('x'); var eSet = e.setAttribute('v', 'k'); var eLen = e.length; var eThen = e.then; \
        var symIn = Symbol('s') in this.border; \
        var pNode = typeof this.border.__xfaPropNode; var pPath = this.border.__xfaPropPath; \
        var pLen = this.border.length; var pThen = this.border.then; \
        var oneNone = this.border.oneOfChild; \
        this.border[Symbol('k')] = 'v'; this.border.className = 'ignored'; \
        this.border.setAttribute('dashed', 'stroke'); \
        var acc = this.getAttribute('access'); \
        var uiStar = this.ui.resolveNodes('*').length; \
        var negItem = this.border.nodes.item(-1); var oobItem = this.border.nodes.item(99); \
        [String(eEl), String(eSet), String(eLen), String(eThen), symIn, pNode, pPath, String(pLen), String(pThen), String(oneNone), this.border.className, this.border.stroke, acc, uiStar, String(negItem), String(oobItem)].join('|')";
    assert_eq!(
        eval_on(&rich_form(), script, "plain"),
        "null|null|undefined|undefined|true|number|border#0|undefined|undefined|null|border|dashed|open|1|null|null"
    );
}

#[test]
fn a_copied_element_keeps_attributes_written_to_it() {
    assert_eq!(
        eval_on(
            &rich_form(),
            "this.border.getElement('edge', 0).customAttr = 'hi'; var e1 = this.border.getElement('edge', 1); e1.customAttr + '|' + e1.stroke",
            "edge1"
        ),
        "hi|solid"
    );
}

#[test]
fn path_and_resolve_helpers_stop_at_their_limits() {
    let mut root = rich_form();
    let big_children: Vec<XfaProp> = (0..64).map(|_| p("a", &[], (0..65).map(|_| p("b", &[], vec![])).collect())).collect();
    root.children.push(node("many", XfaKind::Field, &[], vec![p("extras", &[], big_children)]));
    let h = XHost::new(Arc::new(root), false, "form1[0].many[0]", &XfaDoc::default());
    let many = h.current;
    assert_eq!(props::read_path(&h, many, "border#0/edge#4"), None);
    assert!(props::resolve(&h, many, "a#0/b#0/c#0/d#0/e#0/f#0/g#0/h#0", "x").is_empty());
    assert_eq!(props::resolve(&h, many, "extras#0", "a[*].b[*]").len(), props::MAX_RESOLVED);
    let deep_som = format!("{}border", "a.".repeat(crate::xfa::model::MAX_DEPTH + 1));
    assert!(props::resolve_into_props(&h, many, &deep_som).is_empty());
    assert_eq!(props::shorthand_color("unknown"), None);

    let mut copied = Vec::new();
    props::copy_of_element(&h, many, "extras#0", "", &mut copied, props::MAX_PATH);
    assert_eq!(copied.len(), 1);
}

#[test]
fn property_references_check_their_node_and_a_root_write_is_accepted() {
    let mut ctx = boa_engine::Context::default();
    let bad = boa_engine::object::ObjectInitializer::new(&mut ctx)
        .property(boa_engine::JsString::from(props::NODE), boa_engine::JsValue::from(-1.0), boa_engine::property::Attribute::READONLY)
        .build();
    assert!(props::prop_ref(&bad, &mut ctx).is_err());

    let h = XHost::new(Arc::new(rich_form()), false, "form1[0].rich[0]", &XfaDoc::default());
    let rich = h.current;
    ctx.insert_data(std::cell::RefCell::new(h));
    let nref = props::node_ref(&mut ctx, rich);
    let args = [nref.into(), boa_engine::JsValue::from(boa_engine::js_string!("attr")), boa_engine::JsValue::from(boa_engine::js_string!("val"))];
    assert!(props::prop_set(&boa_engine::JsValue::undefined(), &args, &mut ctx).is_ok());
}
