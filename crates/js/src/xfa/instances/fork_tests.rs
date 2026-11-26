//! The fork's tests of the instance manager.

use super::*;
use crate::Limits;
use crate::fork_test_support::{form1, row};
use crate::xfa::{XfaDoc, XfaEvent, XfaNode, run_xfa};

/// form1 > row (1 to 3 instances, one now) > sub (1 to 3 instances, two now)
fn repeatable_form() -> XfaNode {
    let sub = |r: usize, i: usize| row("sub", format!("form1[0].row[{r}].sub[{i}]"), i, (1, 3), vec![]);
    form1(vec![row("row", "form1[0].row[0]".into(), 0, (1, 3), vec![sub(0, 0), sub(0, 1)])])
}

fn eval(script: &str) -> Option<String> {
    let event = XfaEvent { activity: "click".into(), target: "form1[0]".into(), ..Default::default() };
    let out = run_xfa(script, &event, &XfaDoc::default(), &repeatable_form(), Limits::default());
    assert_eq!(out.error, None, "{script}: {:?}", out.error);
    out.result
}

#[test]
fn list_items_outside_the_list_are_null() {
    assert_eq!(eval("var l = this.resolveNodes('row[*]'); [l.item(-1) === null, l.item(99) === null].join('|')").as_deref(), Some("true|true"));
}

#[test]
fn instance_counts_stay_within_min_and_max_and_a_negative_count_is_ignored() {
    assert_eq!(
        eval(
            "var n = []; _row.insertInstance(); n.push(_row.count); _row.count = 3; n.push(_row.count); \
             _row.setInstances(-1); n.push(_row.count); _row.setInstances(1); n.push(_row.count); \
             _row.setInstances(10); n.push(_row.count); n.join('|')"
        )
        .as_deref(),
        Some("2|3|3|1|3")
    );
}

#[test]
fn removing_renumbers_and_new_instances_start_at_their_minimum() {
    // The added rows hold one `sub`, their minimum; the original holds two, so removing
    // instance 0 removes the original.
    assert_eq!(
        eval(
            "_row.setInstances(3); \
             var first = this.resolveNodes('row[*]').item(0).resolveNodes('sub[*]').length; \
             _row.removeInstance(0); \
             var l = this.resolveNodes('row[*]'); \
             [first, _row.count, l.item(0).somExpression, l.item(1).somExpression, l.item(0).resolveNodes('sub[*]').length].join('|')"
        )
        .as_deref(),
        Some("2|2|form1[0].row[0]|form1[0].row[1]|1")
    );
}

#[test]
fn child_soms_skip_unnamed_and_hash_names() {
    assert_eq!(child_som("form1[0]".into(), "#subform", 0), "form1[0]");
    assert_eq!(child_som("form1[0]".into(), "", 0), "form1[0]");
    assert_eq!(child_som(String::new(), "row", 2), "row[2]");
    assert_eq!(child_som("form1[0]".into(), "row", 2), "form1[0].row[2]");
}

#[test]
fn renumbering_and_cloning_stop_at_the_depth_limit() {
    let mut h = XHost::new(Arc::new(repeatable_form()), false, "form1[0]", &XfaDoc::default());
    let before: Vec<String> = h.nodes.iter().map(|n| n.som.to_string()).collect();
    if let Some(n) = h.nodes.get_mut(1) {
        n.som = "stale".into();
    }
    // Past the limit, and for a node that does not exist, nothing is renamed.
    renumber(&mut h, 1, MAX_DEPTH + 1);
    renumber(&mut h, 9999, 0);
    assert_eq!(h.nodes.get(1).map(|n| n.som.to_string()).as_deref(), Some("stale"));
    renumber(&mut h, 1, 0);
    assert_eq!(h.nodes.get(1).map(|n| n.som.to_string()), before.get(1).cloned());

    assert!(clone_subtree(&mut h, 0, None, MAX_DEPTH + 1).is_none());
    // At the limit the node is cloned without its children.
    let shallow = clone_subtree(&mut h, 1, Some(0), MAX_DEPTH);
    assert!(shallow.and_then(|i| h.nodes.get(i)).is_some_and(|n| n.children.is_empty()));
}
