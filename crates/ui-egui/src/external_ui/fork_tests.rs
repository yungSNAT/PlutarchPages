//! Conversion jobs and Read Out Loud as the app sees them: what opens, what it says, what it stops.

use super::*;
use std::time::{Duration, Instant};

fn app() -> PdfCraftApp {
    crate::i18n::set_current(crate::i18n::Lang::EN);
    PdfCraftApp::new()
}

fn toast(app: &PdfCraftApp) -> &str {
    app.toast.as_ref().map_or("", |(m, _)| m.as_str())
}

/// Poll until no conversion is running (each job's thread finishes at once here).
fn finish(app: &mut PdfCraftApp) {
    let started = Instant::now();
    while app.conversions_running() > 0 {
        assert!(started.elapsed() < Duration::from_secs(10), "a conversion never finished");
        std::thread::sleep(Duration::from_millis(5));
        app.poll_external();
    }
}

const PDF: &[u8] = include_bytes!("../../tests/data/form.pdf");

#[test]
fn a_finished_conversion_opens_as_a_pdf_named_after_its_source() {
    let mut app = app();
    app.spawn_external("report.docx", |_| Ok(PDF.to_vec()));
    assert_eq!(app.conversions_running(), 1);
    assert_eq!(toast(&app), "Converting report.docx…");
    finish(&mut app);
    assert_eq!(app.views.len(), 1);
    assert_eq!(app.session.get(app.views[0].id).unwrap().name, "report.pdf");
}

#[test]
fn a_failed_or_panicking_conversion_says_why_and_opens_nothing() {
    let mut app = app();
    app.spawn_external("sheet.xlsx", |_| Err("LibreOffice was not found".into()));
    finish(&mut app);
    assert_eq!(toast(&app), "Couldn't convert sheet.xlsx: LibreOffice was not found");

    app.spawn_external("deck.pptx", |_| panic!("converter bug"));
    finish(&mut app);
    assert_eq!(toast(&app), "Couldn't convert deck.pptx: the conversion stopped unexpectedly");

    app.spawn_external("notes.html", |_| Ok(b"not a pdf".to_vec()));
    finish(&mut app);
    assert!(toast(&app).starts_with("Couldn't convert notes.html: "), "{}", toast(&app));
    assert!(app.views.is_empty());
}

#[test]
fn closing_the_app_cancels_a_running_conversion() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = app();
    app.spawn_external("slow.docx", move |cancel| {
        let started = Instant::now();
        while !cancel.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = tx.send(cancel.load(Ordering::Relaxed));
        Err("cancelled".into())
    });
    drop(app);
    assert_eq!(rx.recv_timeout(Duration::from_secs(20)), Ok(true), "the job saw the cancel flag");
}

#[test]
fn a_web_address_that_is_not_http_starts_no_conversion() {
    let mut app = app();
    app.external.url = "file:///etc/passwd".into();
    app.start_web_page();
    assert_eq!(app.conversions_running(), 0);
    assert!(!toast(&app).is_empty(), "the refusal is said");
}

#[test]
fn reading_aloud_without_a_document_asks_for_one() {
    let mut app = app();
    app.read_aloud(false);
    assert_eq!(toast(&app), "Open a document to read it aloud");
    assert!(!app.reading_aloud());
}

#[test]
fn stem_drops_only_the_last_extension() {
    assert_eq!(stem("a.b.docx"), "a.b");
    assert_eq!(stem("README"), "README");
}
