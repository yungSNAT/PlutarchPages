// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// A harness on `docs` copies of the 5-page fixture (the last one active), with the app's launch
/// time set when `launched` is.
fn first_paint_harness(docs: usize, launched: Option<std::time::Instant>) -> (Harness<'static, PdfCraftApp>, ControlClient) {
    let slot: Arc<Mutex<Option<ControlClient>>> = Arc::default();
    let s = slot.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.launched_at = launched;
        *s.lock().unwrap() = Some(app.attach_control(&cc.egui_ctx));
        for i in 0..docs {
            app.open_bytes(&format!("doc{i}.pdf"), None, fixture(5)).unwrap();
        }
        app
    });
    h.run_steps(1);
    let client = slot.lock().unwrap().take().unwrap();
    (h, client)
}

/// Poll `ui.first_page_paint` until it reports a drawn page.
fn first_page_paint(h: &mut Harness<'static, PdfCraftApp>, c: &ControlClient) -> Value {
    let mut reply = json!(null);
    let drawn = crate::support::step_until(h, |h| {
        reply = ok(h, c, "ui.first_page_paint", json!({}));
        reply["drawn"] == true
    });
    assert!(drawn, "no page was painted in 30 s: {reply}");
    reply
}

#[test]
fn first_page_paint_reports_the_active_documents_first_painted_page_once() {
    // Before the app exists, so any pass that could paint comes after it.
    let before = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let (mut h, c) = first_paint_harness(1, None);
    let paint = first_page_paint(&mut h, &c);
    let after = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let active = ok(&mut h, &c, "ui.state", json!({}))["active"]["doc"].clone();
    assert_eq!(paint["doc"], active, "the active document: {paint}");
    assert_eq!(paint["page"], 1, "the first page is the one in view: {paint}");
    assert!(h.state().views[0].drawn_page_size(0).is_some(), "a raster of page 1 is on screen");
    let unix_ms = paint["unix_ms"].as_u64().unwrap();
    assert!((before..=after).contains(&unix_ms), "{unix_ms} is the wall clock of the painting pass, between {before} and {after}");
    // No launch time (a test, or the web build): no elapsed time to report.
    assert!(paint["since_launch_ms"].is_null(), "{paint}");
    // Recorded once: later passes paint the page again without moving the figure.
    h.run_steps(5);
    assert_eq!(ok(&mut h, &c, "ui.first_page_paint", json!({})), paint);
    let err = call(&mut h, &c, "ui.first_page_paint", json!({"page": 1})).unwrap_err();
    assert!(err.contains("page"), "an unknown parameter is refused: {err}");
}

#[test]
fn first_page_paint_is_not_drawn_without_a_document() {
    let (mut h, c) = first_paint_harness(0, None);
    h.run_steps(5);
    assert_eq!(ok(&mut h, &c, "ui.first_page_paint", json!({})), json!({ "drawn": false }));
}

#[test]
fn first_page_paint_times_from_the_launch() {
    let launched = std::time::Instant::now();
    let (mut h, c) = first_paint_harness(1, Some(launched));
    let paint = first_page_paint(&mut h, &c);
    let elapsed = launched.elapsed().as_secs_f64() * 1000.0;
    let since = paint["since_launch_ms"].as_f64().unwrap();
    assert!(since > 0.0 && since <= elapsed, "{since} ms from the launch to the paint, within the {elapsed} ms since: {paint}");
}

#[test]
fn first_page_paint_follows_the_active_document() {
    let (mut h, c) = first_paint_harness(2, None);
    let second = first_page_paint(&mut h, &c);
    ok(&mut h, &c, "ui.command", json!({"id": "view.next_tab"}));
    let first = first_page_paint(&mut h, &c);
    assert_ne!(first["doc"], second["doc"], "the newly active document, not the last one's paint: {first}");
    assert_eq!(first["doc"], ok(&mut h, &c, "ui.state", json!({}))["active"]["doc"]);
    // Each document keeps its own record: switching back reports the second one's as it was.
    ok(&mut h, &c, "ui.command", json!({"id": "view.next_tab"}));
    assert_eq!(ok(&mut h, &c, "ui.first_page_paint", json!({})), second);
}
