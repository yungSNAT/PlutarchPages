//! The fork's tests of FormCalc accessors.

use std::sync::Arc;

use super::*;
use crate::Limits;
use crate::fork_test_support::{field, form1, row};
use crate::formcalc::fork_test_support::interp;
use crate::formcalc::run_formcalc;
use crate::xfa::{XHost, XfaDoc, XfaEvent, XfaNode};

fn sample_form() -> XfaNode {
    let at = |idx: usize, a: &str, b: &str| {
        let som = format!("form1[0].row[{idx}]");
        row(
            "row",
            som.clone(),
            idx,
            (1, 5),
            vec![field("item", &format!("{som}.item[0]"), a, true), field("item", &format!("{som}.item[1]"), b, true)],
        )
    };
    form1(vec![at(0, "10", "20"), at(1, "30", "40"), field("empty", "form1[0].empty[0]", "", false), field("flag", "form1[0].flag[0]", "1", true)])
}

fn eval_on(root: &XfaNode, target: &str, script: &str) -> crate::xfa::XfaOutcome {
    let event = XfaEvent { activity: "calculate".into(), target: target.into(), new_text: "new".into(), prev_text: "old".into() };
    let doc = XfaDoc { file_name: "inv.pdf".into(), page: 1, page_count: 4, highlight: false };
    run_formcalc(script, &event, &doc, root, Limits::default())
}

#[test]
fn accessors_cover_special_objects_members_indexing_and_coercions() {
    let root = sample_form();
    let ok = |target: &str, script: &str| {
        let out = eval_on(&root, target, script);
        assert_eq!(out.error, None, "{script} -> {:?}", out.error);
        out.result.unwrap_or_default()
    };

    // Coercions via Concat, truthy, is_null, ident ($template/$xfa, _row)
    assert_eq!(
        ok("form1[0].flag[0]", r#"Concat($host, "|", $layout, "|", $event, "|", $template, "|", $xfa, "|", _row, "|", $.border)"#),
        "host|layout|event|xfa|xfa|instanceManager|"
    );
    assert_eq!(ok("form1[0].flag[0]", "if (null) then 1 elseif ($) then 2 else 3 endif"), "2");
    assert_eq!(ok("form1[0].empty[0]", "HasValue($)"), "0");
    assert_eq!(ok("form1[0].flag[0]", "HasValue(row[*].missing)"), "0");
    assert_eq!(
        ok(
            "form1[0].flag[0]",
            r#"Concat($host.currentPage, "|", $host.name, "|", $host.appType, "|", $host.version, "|", $host.variation, "|", $host.language, "|", $host.platform, "|", $host.title, "|", $host.calculationsEnabled, "|", $host.validationsEnabled, "|", $host.unknown)"#
        ),
        format!("1|Acrobat|Exchange-Pro|11|Full|en|{}|inv.pdf|1|1|", crate::platform())
    );
    assert_eq!(
        ok(
            "form1[0].flag[0]",
            r#"Concat($layout.ready, "|", $event.newText, "|", $event.fullText, "|", $event.prevText, "|", $event.change, "|", $event.target.name, "|", $event.selStart, "|", $event.unknown)"#
        ),
        "|new|new|old||flag|0|"
    );
    assert_eq!(
        ok("form1[0].flag[0]", r#"Concat(xfa.record.name, "|", xfa.host.name, "|", xfa.event.name, "|", xfa.unknown)"#),
        "form1|Acrobat|calculate|"
    );
    assert_eq!(ok("form1[0].flag[0]", r#"Concat(_row.min, "|", _row.max, "|", _row.className, "|", _row.unknown)"#), "1|5|instanceManager|");

    // Node members (with parent and on synthetic root $form without parent)
    assert_eq!(
        ok(
            "form1[0].flag[0]",
            r#"Concat($.somExpression, "|", $.className, "|", $.index, "|", $.instanceIndex, "|", $.isNull, "|", $.mandatory, "|", $.missing)"#
        ),
        "form1[0].flag[0]|field|0|0|0|disabled|"
    );
    assert_eq!(ok("form1[0].row[0]", r#"Concat($.nodes.name, "|", Count($.all), "|", $.instanceManager.count)"#), "item|2|2");
    assert_eq!(ok("form1[0].flag[0]", r#"Concat($form.all.name, "|", HasValue($form.instanceManager))"#), "form|0");

    // List member returning nested lists (row[*].item) and nulls (row[*].missing)
    assert_eq!(ok("form1[0].flag[0]", "Sum(row[*].item)"), "100");
    assert_eq!(ok("form1[0].flag[0]", "HasValue(row[*].missing)"), "0");

    // Negative Abs index, all_of on local var / $ / Member fallback
    assert_eq!(ok("form1[0].flag[0]", "var neg = -1\nHasValue(row[neg])"), "0");
    assert_eq!(ok("form1[0].flag[0]", "var r = row\nCount(r[*])"), "0");
    assert_eq!(ok("form1[0].flag[0]", "Count($[0])"), "1");
    assert_eq!(ok("form1[0].row[0]", "Count($.all[*])"), "2");

    // Error paths: unknown _im, root _im, member on primitive
    assert!(eval_on(&root, "form1[0].flag[0]", "_nope").error.is_some_and(|e| e.contains("unknown instance manager")));
    assert!(eval_on(&root, "form1[0].flag[0]", "(1).foo").error.is_some_and(|e| e.contains("has no property")));
    assert!(eval_on(&root, "form1[0].flag[0]", "\"abc\".foo").error.is_some_and(|e| e.contains("has no property")));
}

#[test]
fn direct_interp_access_covers_coercion_and_limit_guards() {
    let root = Arc::new(sample_form());
    let mut host = XHost::new(root, false, "form1[0].flag[0]", &XfaDoc::default());
    let mut interp = interp(&mut host);
    let flag = interp.h.current;
    assert_eq!(interp.to_num(&Val::Node(flag)), 1.0);
    assert_eq!(interp.to_num(&Val::List(vec![Val::Node(flag)])), 1.0);
    assert_eq!(interp.to_num(&Val::Host), 0.0);
    assert_eq!(interp.to_str(&Val::Node(flag)), "1");
    assert!(interp.is_null(&Val::List(vec![])));
    assert_eq!(interp.nodes_of(&Val::List(vec![Val::Node(flag)])), vec![flag]);

    // `this` at synthetic root 0 hits all_of parent-None arm (L415) and `_this` hits parent-None error (L209).
    interp.h.current = 0;
    assert_eq!(interp.all_of(&Expr::Ident("this".into())), Ok(vec![0]));
    assert!(interp.ident("_this").is_err());

    // `declare` when scopes is empty does not panic (L182).
    interp.scopes.clear();
    interp.declare("orphan", Val::Num(1.0));

    // `gather` and `list_push` past MAX_LIST.
    let mut out_nodes = Vec::new();
    let mut seen = HashSet::new();
    assert_eq!(interp.gather(&mut out_nodes, &mut seen, 0..=MAX_LIST), Err(LIST_TOO_LONG.into()));

    interp.steps = 0;
    let mut out_vals: Vec<Val> = (0..MAX_LIST).map(|n| Val::Num(n as f64)).collect();
    let mut seen_vals = HashSet::new();
    assert_eq!(interp.list_push(&mut out_vals, &mut seen_vals, Val::Num(0.0)), Err(LIST_TOO_LONG.into()));
}
