//! The fork's tests of the FormCalc evaluator.

use std::sync::Arc;

use super::*;
use crate::Limits;
use crate::fork_test_support::{field, form1, row};
use crate::formcalc::fork_test_support::interp;
use crate::formcalc::{MAX_LIST, run_formcalc};
use crate::xfa::{XHost, XfaDoc, XfaEvent, XfaNode};

fn sample_form() -> XfaNode {
    let at = |idx: usize, val: &str| {
        let som = format!("form1[0].row[{idx}]");
        row("row", som.clone(), idx, (1, 5), vec![field("amount", &format!("{som}.amount[0]"), val, true)])
    };
    form1(vec![at(0, "10"), at(1, "20"), field("target", "form1[0].target[0]", "5", true)])
}

fn run_on(root: &XfaNode, target: &str, script: &str, limits: Limits) -> crate::xfa::XfaOutcome {
    let ev = XfaEvent { activity: "calculate".into(), target: target.into(), ..Default::default() };
    run_formcalc(script, &ev, &XfaDoc::default(), root, limits)
}

#[test]
fn eval_covers_operators_methods_assignments_and_control_flow() {
    let root = sample_form();
    let ok = |script: &str| {
        let out = run_on(&root, "form1[0].target[0]", script, Limits::default());
        assert_eq!(out.error, None, "{script} -> {:?}", out.error);
        out
    };

    // Descend empty, short-circuit &, null + null, num vs non-numeric string
    assert_eq!(ok("HasValue($form..nope)").result.as_deref(), Some("0"));
    assert_eq!(ok("0 & (1 / 0)").result.as_deref(), Some("0"));
    assert_eq!(ok("null + null").result.as_deref(), Some(""));
    assert_eq!(ok("10 == \"abc\"").result.as_deref(), Some("0"));

    // Non-callable error and user call recursion limit
    assert!(run_on(&root, "form1[0].target[0]", "(1)(2)", Limits::default()).error.is_some_and(|e| e.contains("not a function")));
    let small = Limits { recursion: 8, ..Limits::default() };
    assert!(run_on(&root, "form1[0].target[0]", "func r(n) do r(n) endfunc\nr(1)", small).error.is_some_and(|e| e.contains("nested too deeply")));

    // Host, layout, xfa, node, instanceManager, sink/event/list methods
    let out = ok("$host.setFocus($)\n$host.setFocus()\n$host.unknown()\n\
         $layout.relayout()\n$layout.relayoutPageArea()\n$layout.unknown()\n\
         xfa.recalculate()\nxfa.remerge()\nxfa.unknown()\n\
         $.recalculate()\n$.remerge()\n$.unknown()\n\
         _row.setInstances(3)\n_row.removeInstance(2)\n_row.unknown()\n\
         $event.foo()\n$.border.foo()\nrow[*].foo()\n\
         Concat($.resolveNode(\"row\").name, \"|\", Count($.resolveNodes(\"row[*]\")), \"|\", HasValue(xfa.resolveNode(\"missing\")), \"|\", Count(xfa.resolveNode(\"row[*]\")), \"|\", _row.count)");
    assert_eq!(out.result.as_deref(), Some("row|2|0|2|2"));
    assert!(out.effects.contains(&XfaEffect::SetFocus("form1[0].target[0]".into())));
    assert!(out.effects.contains(&XfaEffect::SetFocus(String::new())));
    assert!(out.effects.contains(&XfaEffect::Relayout));
    assert!(out.effects.contains(&XfaEffect::Recalculate));

    // Method errors on null and primitives
    assert!(run_on(&root, "form1[0].target[0]", "null.foo()", Limits::default()).error.is_some_and(|e| e.contains("no object")));
    assert!(run_on(&root, "form1[0].target[0]", "(1).foo()", Limits::default()).error.is_some_and(|e| e.contains("has no method")));

    // Assignments: index/descend, presence, access, _im, Val::List, _im.count, invalid target
    let assign_out = ok("row[0].amount = 11\n$form..target = 77\n\
         $.presence = \"hidden\"\n$.access = \"readOnly\"\n\
         form1._row = 3\nform1._missing = 2\n_row.count = 2\n\
         row[*].amount = 99\nSum(row[*].amount)");
    assert_eq!(assign_out.result.as_deref(), Some("198"));
    assert!(assign_out.effects.contains(&XfaEffect::SetPresence { som: "form1[0].target[0]".into(), presence: "hidden".into() }));
    assert!(assign_out.effects.contains(&XfaEffect::SetAccess { som: "form1[0].target[0]".into(), access: "readOnly".into() }));
    assert!(run_on(&root, "form1[0].target[0]", "1 = 2", Limits::default()).error.is_some_and(|e| e.contains("cannot assign")));

    // Block depth limit
    let deep_if = format!("{}1{}", "if 1 then\n".repeat(12), "\nendif".repeat(12));
    assert!(run_on(&root, "form1[0].target[0]", &deep_if, small).error.is_some_and(|e| e.contains("nested too deeply")));

    // Statements: bare var, bare return, bad for bounds, break/return/exit in while/for/foreach
    assert_eq!(ok("var uninit\nHasValue(uninit)").result.as_deref(), Some("0"));
    assert_eq!(ok("func emptyRet() do\nreturn\nendfunc\nHasValue(emptyRet())").result.as_deref(), Some("0"));
    assert!(
        run_on(&root, "form1[0].target[0]", "for i = 1 upto 5 step 0 do\nendfor", Limits::default())
            .error
            .is_some_and(|e| e.contains("bad for loop bounds"))
    );

    assert_eq!(ok("var k = 0\nwhile k < 5 do\nk = k + 1\nif k == 2 then\nbreak\nendif\nendwhile\nk").result.as_deref(), Some("2"));
    assert_eq!(ok("func wret() do\nwhile 1 do\nreturn 7\nendwhile\nendfunc\nwret()").result.as_deref(), Some("7"));
    assert_eq!(ok("while 1 do\nexit\nendwhile\n99").result.as_deref(), Some(""));

    assert_eq!(ok("func fret() do\nfor i = 1 upto 5 do\nreturn i + 10\nendfor\nendfunc\nfret()").result.as_deref(), Some("11"));
    assert_eq!(ok("for i = 1 upto 5 do\nexit\nendfor\n99").result.as_deref(), Some(""));

    assert_eq!(ok("var s = 0\nforeach x in (1, 2, 3) do\ns = s + x\nbreak\nendfor\ns").result.as_deref(), Some("1"));
    assert_eq!(ok("func feret() do\nforeach x in (4, 5) do\nreturn x\nendfor\nendfunc\nferet()").result.as_deref(), Some("4"));
    assert_eq!(ok("foreach x in (1, 2) do\nexit\nendfor\n99").result.as_deref(), Some(""));
}

#[test]
fn arg_values_keeping_enforces_max_list_cap() {
    let root = Arc::new(sample_form());
    let mut host = XHost::new(root, false, "form1[0].target[0]", &XfaDoc::default());
    let mut interp = interp(&mut host);
    let big = Val::List(vec![Val::Num(1.0); MAX_LIST + 1]);
    interp.declare("big", big);
    assert_eq!(interp.arg_values_keeping(&[Expr::Ident("big".into())], true), Err(LIST_TOO_LONG.into()));
}

#[test]
fn numeric_text_compares_with_a_number_as_a_number() {
    let root = sample_form();
    let run = |s: &str| run_on(&root, "form1[0].target[0]", s, Limits::default()).result;
    // "10" against 9 as numbers, not as the texts "10" and "9".
    assert_eq!(run("\"10\" > 9").as_deref(), Some("1"));
}

#[test]
fn calls_and_blocks_past_the_depth_limit_are_errors() {
    let root = Arc::new(sample_form());
    let mut host = XHost::new(root, false, "form1[0].target[0]", &XfaDoc::default());
    let mut interp = interp(&mut host);
    interp.max_depth = 0;
    let f = Func { params: Vec::new(), body: Vec::new() };
    assert!(interp.call_user(&f, Vec::new()).is_err_and(|e| e == CALLS_TOO_DEEP));
    assert!(interp.block(&[]).is_err_and(|e| e == TOO_DEEP));
    assert_eq!(interp.depth, 0);
}

#[test]
fn text_against_a_number_compares_as_numbers() {
    // XFA 3.3 FormCalc relational and equality expressions: only 2 strings compare as text;
    // otherwise both operands are promoted to numbers, and text that is not a number is 0.
    let root = sample_form();
    let run = |s: &str| run_on(&root, "form1[0].target[0]", s, Limits::default()).result;
    assert_eq!(run("\"abc\" > 5").as_deref(), Some("0"));
    assert_eq!(run("5 < \"abc\"").as_deref(), Some("0"));
    assert_eq!(run("\"abc\" < 5").as_deref(), Some("1"));
    assert_eq!(run("\"abc\" == 0").as_deref(), Some("1"));
    assert_eq!(run("0 <> \"abc\"").as_deref(), Some("0"));
    assert_eq!(run("\"abc\" >= -1").as_deref(), Some("1"));
}
