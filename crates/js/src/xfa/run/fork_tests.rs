//! The fork's tests of running XFA scripts on their own thread.

use std::time::Duration;

use super::*;

const LONG: Duration = Duration::from_secs(30);

#[test]
fn a_refused_script_returns_its_reason_without_running() {
    let out = run_xfa_within(&"(".repeat(100), &XfaEvent::default(), &XfaDoc::default(), XfaNode::default(), Limits::default(), LONG);
    assert!(out.error.as_deref().is_some_and(|e| e.contains("nested too deeply")), "{:?}", out.error);
    assert!(!out.abandoned);
}

#[test]
fn a_null_result_reads_as_empty_text() {
    let out = run_xfa_within("null", &XfaEvent::default(), &XfaDoc::default(), XfaNode::default(), Limits::default(), LONG);
    assert_eq!((out.error, out.result.as_deref()), (None, Some("")));
}

#[test]
fn a_second_start_signal_changes_nothing() {
    let out = run_within_limits("pdfcraft-xfa-test", LONG, LONG, |started| {
        started();
        started();
        XfaOutcome { result: Some("ok".into()), ..Default::default() }
    });
    assert_eq!((out.result.as_deref(), out.abandoned), (Some("ok"), false));
}

#[test]
fn a_worker_that_dies_reports_an_internal_error() {
    let out = run_within_limits("pdfcraft-xfa-test-unwind", LONG, LONG, |started| {
        started();
        std::panic::resume_unwind(Box::new("simulated worker failure"))
    });
    assert!(out.error.as_deref().is_some_and(|e| e.contains("internal error")), "{:?}", out.error);
    assert!(!out.abandoned);
}
