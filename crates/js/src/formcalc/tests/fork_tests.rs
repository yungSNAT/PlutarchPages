//! The fork's tests of the FormCalc entry point.

use super::*;

#[test]
fn a_formcalc_script_past_the_size_limit_is_refused_before_parsing() {
    let huge = "1\n".repeat(crate::MAX_SCRIPT_BYTES);
    let out = run_formcalc(&huge, &XfaEvent::default(), &XfaDoc::default(), &XfaNode::default(), Limits::default());
    assert!(out.error.as_deref().is_some_and(|e| e.contains("too long")), "{:?}", out.error);
    assert_eq!(out.result, None);
}
