use super::*;

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn attach_context_moves_startup_open_and_launch_options_off_ui_thread() {
    let file = tool_open_file("startup-async", "pdf", &fixture(4));
    let path = file.0.to_string_lossy().into_owned();
    let (gate, started, release, finished) = open_worker::prepare_gate();

    let mut harness = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate);
        app.attach_context(&creation.egui_ctx);
        assert_eq!(app.open_path(&path), crate::OpenOutcome::Pending);
        app.set_launch_option("theme", "dark").expect("app preference applies immediately");
        app.set_launch_option("page", "3").expect("page option defers while opening");
        app.set_launch_option("zoom", "150").expect("zoom option defers while opening");
        app
    });

    // First frame runs without waiting for the prepare barrier: UI thread is not blocked.
    harness.step();
    started.recv_timeout(Duration::from_secs(5)).expect("worker reached prepare barrier");
    assert_eq!(harness.state().views.len(), 0);
    assert_eq!(harness.state().pending_opens.len(), 1);
    assert_eq!(harness.state().theme_preference, crate::ThemePreference::Dark);

    release.send(()).expect("release prepare barrier");
    finished.recv_timeout(Duration::from_secs(5)).expect("worker finished preparation");
    harness.step();

    let app = harness.state();
    assert!(app.pending_opens.is_empty());
    assert_eq!(app.views.len(), 1);
    assert_eq!(app.active, Some(0));
    assert_eq!(app.views[0].current, 2, "deferred --page 3 applied after background open");
    assert!((app.views[0].zoom - 1.5).abs() < 0.01, "deferred --zoom 150 applied after background open");
}

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn reopen_last_files_with_attached_context_restores_pages_and_active_tab_async() {
    let first = tool_open_file("session-first", "pdf", &fixture(4));
    let second = tool_open_file("session-second", "pdf", &fixture(3));
    let first_path = crate::persist::last_session::absolute(&first.0.to_string_lossy());
    let second_path = crate::persist::last_session::absolute(&second.0.to_string_lossy());

    let mut harness = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.reopen_last_session = true;
        app.last_session = crate::persist::last_session::LastSession {
            files: vec![
                crate::persist::last_session::SessionFile { path: first_path.clone(), page: 2 },
                crate::persist::last_session::SessionFile { path: second_path.clone(), page: 1 },
            ],
            active: Some(first_path),
        };
        app.attach_context(&creation.egui_ctx);
        app.reopen_last_files(&[]);
        assert_eq!(app.pending_opens.len(), 2, "both session files queued off UI thread");
        app
    });

    for _ in 0..200 {
        harness.step();
        if harness.state().pending_opens.is_empty() && harness.state().deferred_opens.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let app = harness.state();
    assert!(app.pending_opens.is_empty());
    assert_eq!(app.views.len(), 2);
    assert_eq!(app.active, Some(0), "restored active tab stays active");
    assert_eq!(app.views[0].current, 2, "first tab restored to saved page");
    assert_eq!(app.views[1].current, 1, "second tab restored to saved page");
}
