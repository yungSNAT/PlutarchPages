//! The fork's tests of the timer table.

use super::*;

#[test]
fn a_realm_without_timers_ignores_restore_and_clear_and_hands_out_no_timer() {
    let mut ctx = Context::default();
    restore(&ctx, Timers::default());
    assert!(clear(&JsValue::undefined(), &[JsValue::from(123)], &mut ctx).is_ok());
    let obj = ObjectInitializer::new(&mut ctx).build();
    assert!(clear(&JsValue::undefined(), &[obj.into()], &mut ctx).is_ok());
    assert!(set_time_out(&JsValue::undefined(), &[crate::host::s("1"), JsValue::from(50)], &mut ctx).is_ok());
    assert!(report(&ctx).is_empty() && save(&ctx).is_none());
}

#[test]
fn enabling_twice_keeps_the_timers_already_set() {
    let mut ctx = Context::default();
    enable(&mut ctx);
    assert!(set_interval(&JsValue::undefined(), &[crate::host::s("1"), JsValue::from(50)], &mut ctx).is_ok());
    enable(&mut ctx);
    assert_eq!(report(&ctx).len(), 1);
    // Anything but a timer's own handle clears nothing.
    assert!(clear(&JsValue::undefined(), &[JsValue::from(1)], &mut ctx).is_ok());
    assert_eq!(report(&ctx).len(), 1);
}

#[test]
fn a_timer_script_past_the_size_limit_is_refused() {
    let mut ctx = Context::default();
    enable(&mut ctx);
    let huge = crate::host::s(&"x".repeat(MAX_SCRIPT_BYTES + 1));
    let set = set_interval(&JsValue::undefined(), &[huge, JsValue::from(50)], &mut ctx);
    assert!(set.is_err_and(|e| e.to_string().contains("too long")));
    assert!(report(&ctx).is_empty());
}
