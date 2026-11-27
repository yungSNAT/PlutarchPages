//! The fork's tests of the `app`, `util`, `console` and document globals.

use crate::{DocInfo, Event, FieldState, FieldType, Limits, Request, run, run_shared_calculations};
use std::sync::Arc;

fn page_doc() -> DocInfo {
    DocInfo {
        num_pages: 1,
        // Media, crop, bleed, trim and art.
        page_boxes: vec![[
            [0.0, 0.0, 612.0, 792.0],
            [10.0, 10.0, 600.0, 780.0],
            [2.0, 2.0, 610.0, 790.0],
            [5.0, 5.0, 605.0, 785.0],
            [20.0, 20.0, 590.0, 770.0],
        ]],
        page_rotations: vec![0],
        ..Default::default()
    }
}

#[test]
fn url_beep_console_zoom_page_boxes_and_simple_calculations() {
    let fields = vec![FieldState::new("blank", FieldType::Text, vec!["$ %".into()]), FieldState::new("eu", FieldType::Text, vec!["12,5".into()])];
    let out = run(
        "app.launchURL({ cURL: 'https://example.test' }); \
         app.beep(); \
         app.runtimeHighlight = false; \
         console.println('gone'); console.clear(); \
         this.zoom = 150; \
         var bleed = getPageBox('Bleed', 0).join(','); \
         var trim = getPageBox('Trim', 0).join(','); \
         var sum = AFSimple_Calculate('SUM', ['blank', 'eu']); \
         var unk = AFSimple_Calculate('UNKNOWN', ['eu']); \
         [this.zoom, bleed, trim, sum, unk].join('|')",
        &Event::doc("Console"),
        &page_doc(),
        &fields,
        &[],
        Limits::default(),
    );
    assert_eq!(out.error, None, "{:?}", out.error);
    // Boxes are [left, top, right, bottom] from the crop box's lower left; text with no number
    // counts as 0 and a decimal comma is read.
    assert_eq!(out.result.as_deref(), Some("150|-8,780,600,-8|-5,775,595,-5|12.5|0"));
    assert!(out.console.is_empty());
    for r in [Request::LaunchUrl("https://example.test".into()), Request::Beep, Request::Zoom(150.0)] {
        assert!(out.requests.contains(&r), "{r:?} in {:?}", out.requests);
    }
}

#[test]
fn bad_pages_boxes_and_zoom_are_errors() {
    let mut doc = page_doc();
    let go = |script: &str, doc: &DocInfo| run(script, &Event::doc("Console"), doc, &[], &[], Limits::default()).error;
    assert!(go("getPageBox('Crop', 'zero');", &doc).is_some_and(|e| e.contains("non-negative")));
    assert!(go("this.zoom = 0;", &doc).is_some_and(|e| e.contains("between 1 and 6400")));
    doc.page_boxes[0][1][0] = f64::INFINITY;
    assert!(go("getPageBox('Crop', 0);", &doc).is_some_and(|e| e.contains("non-finite")));
}

#[test]
fn calculate_now_stops_on_a_refused_script_and_respects_rc_false() {
    let fields = Arc::new(vec![FieldState::new("a", FieldType::Text, vec!["1".into()])]);
    let calc = |calculation: String| {
        run_shared_calculations(
            "calculateNow();",
            &Event::doc("Console"),
            &DocInfo::default(),
            Arc::clone(&fields),
            &[],
            &[(0, calculation)],
            Limits::default(),
        )
    };
    let refused = calc("(".repeat(100));
    assert!(refused.calculation_limit_exceeded && refused.error.is_some());

    let skipped = calc("event.value = '99'; event.rc = false;".into());
    assert_eq!(skipped.error, None);
    assert!(skipped.changed.is_empty());
}

#[test]
fn a_throwing_event_getter_does_not_leak_calculate_depth() {
    use boa_engine::{Context, JsValue, Source};
    let mut ctx = Context::default();
    ctx.insert_data(std::cell::RefCell::new(crate::fork_test_support::bare_host()));
    let defined = ctx.eval(Source::from_bytes("Object.defineProperty(globalThis, 'event', { get() { throw new Error('boom'); } });"));
    assert!(defined.is_ok());
    // Past the 8 nested calls calculateNow allows: a leaked depth would end in its recursion error.
    for _ in 0..10 {
        let err = super::calculate_now(&JsValue::undefined(), &[], &mut ctx).err().map(|e| e.to_string());
        assert!(err.as_deref().is_some_and(|e| e.contains("boom")), "{err:?}");
    }
    assert_eq!(super::host(&ctx).map(|h| h.borrow().calculate_depth).ok(), Some(0));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn interactive_alert_answers_are_checked_against_its_buttons() {
    use crate::fork_test_support::job;
    use crate::session::ScriptSession;
    let mut session = ScriptSession::default();
    assert_eq!(session.run(job("app.alert('q', 0, 1);"), &[Some("2".into())]).result.as_deref(), Some("2"));
    let invalid = session.run(job("app.alert('q', 0, 0);"), &[Some("9".into())]);
    assert!(invalid.error.as_deref().is_some_and(|e| e.contains("invalid alert response")), "{:?}", invalid.error);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_yes_no_cancel_alert_takes_no_as_its_answer() {
    let mut session = crate::session::ScriptSession::default();
    let out = session.run(crate::fork_test_support::job("app.alert('q', 0, 3);"), &[Some("3".into())]);
    assert_eq!((out.error, out.result.as_deref()), (None, Some("3")));
}

#[test]
fn a_decimal_comma_after_thousands_dots_is_read() {
    let fields = vec![FieldState::new("de", FieldType::Text, vec!["1.234,5".into()])];
    let out = run("AFSimple_Calculate('SUM', ['de']);", &Event::doc("Console"), &DocInfo::default(), &fields, &[], Limits::default());
    assert_eq!((out.error, out.result.as_deref()), (None, Some("1234.5")));
}

#[test]
fn a_calculation_may_not_open_another_document() {
    let fields = Arc::new(vec![FieldState::new("a", FieldType::Text, vec!["1".into()])]);
    let out = run_shared_calculations(
        "calculateNow();",
        &Event::doc("Console"),
        &DocInfo::default(),
        fields,
        &[],
        &[(0, "app.openDoc('other.pdf');".into())],
        Limits::default(),
    );
    assert_eq!(out.error, None, "{:?}", out.error);
    assert!(out.console.iter().any(|l| l.contains("other.pdf") && l.contains("may open another document")), "{:?}", out.console);
    assert!(!out.requests.iter().any(|r| matches!(r, Request::OpenDoc(_))), "{:?}", out.requests);
}
