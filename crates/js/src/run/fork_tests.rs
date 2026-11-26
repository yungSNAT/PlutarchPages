//! The fork's tests of the script runner.

use super::*;

#[test]
fn releasing_fields_in_a_realm_without_a_host_does_nothing() {
    let ctx = Context::default();
    release_fields(&ctx);
    assert!(host(&ctx).is_err());
}

#[test]
fn eval_of_a_string_past_the_compile_limit_is_refused() {
    let out = crate::xfa::run_xfa(
        "eval('x'.repeat(300000));",
        &crate::xfa::XfaEvent::default(),
        &crate::xfa::XfaDoc::default(),
        &crate::xfa::XfaNode::default(),
        Limits::default(),
    );
    assert!(out.error.as_deref().is_some_and(|e| e.contains("too long to compile")), "{:?}", out.error);
}
