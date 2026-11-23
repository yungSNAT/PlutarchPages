use super::*;

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn converted_launch_files_preserve_display_override_layout_target_and_activation() {
    // Mixed PDF + text file with `--two-paged --layout single`: `ResultKind::Conversion` must apply
    // its display override and update `launch_display_target` so `--layout single` hits the final
    // converted tab rather than the earlier PDF.
    let pdf = tool_open_file("conv-first", "pdf", &fixture(3));
    let txt = tool_open_file("conv-notes", "txt", b"Hello converted launch world");
    let pdf_path = pdf.0.to_string_lossy().into_owned();
    let txt_path = txt.0.to_string_lossy().into_owned();
    let (gate, started, release, finished) = open_worker::prepare_gate();

    let mut harness = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate);
        app.attach_context(&creation.egui_ctx);
        app.open_path_with_display(&pdf_path, PageLayout::TwoUp, false);
        app.open_path_with_display(&txt_path, PageLayout::TwoUp, false);
        app.set_launch_layout("single").expect("queue --layout single");
        app
    });

    release_one(&started, &release, &finished, &mut harness);
    release_one(&started, &release, &finished, &mut harness);

    let app = harness.state();
    assert_eq!(app.views.len(), 2);
    assert_eq!(app.views[0].layout, PageLayout::TwoUp, "earlier PDF kept --two-paged");
    assert!(!app.views[0].continuous());
    assert_eq!(app.views[1].layout, PageLayout::Single, "converted text tab received deferred --layout single");
    assert!(!app.views[1].continuous());

    // `ResultKind::Converted` (Office worker output) also honours display_override, launch_display_target
    // and `activate = false` when Combine holds focus.
    let mut app2 = PdfCraftApp::new();
    app2.open_combine_tab();
    let (tx_bg, rx_bg) = std::sync::mpsc::channel();
    app2.pending_opens.push(crate::PendingOpen {
        token: 1,
        receiver: rx_bg,
        name: "Report.docx".into(),
        path: None,
        stamp: None,
        activate: false,
        after_open: None,
        recovered: None,
        then: None,
        display_override: Some((PageLayout::TwoUp, false)),
        reopen_page: None,
        from_session_restore: false,
        ready: None,
    });
    tx_bg
        .send(open_worker::ResultMessage { token: 1, kind: open_worker::ResultKind::Converted(Ok(Arc::new(fixture(2)))) })
        .expect("send converted bg");
    app2.poll_pending_opens();
    assert_eq!(app2.views.len(), 1);
    assert!(app2.combine_showing(), "background Converted completion must not steal focus from Combine");
    assert_eq!(app2.views[0].layout, PageLayout::TwoUp);

    // Background `ResultKind::Converted` also preserves an already-active document tab.
    app2.select_tab(0);
    let (tx_bg2, rx_bg2) = std::sync::mpsc::channel();
    app2.pending_opens.push(crate::PendingOpen {
        token: 3,
        receiver: rx_bg2,
        name: "Sheet.xlsx".into(),
        path: None,
        stamp: None,
        activate: false,
        after_open: None,
        recovered: None,
        then: None,
        display_override: None,
        reopen_page: None,
        from_session_restore: false,
        ready: None,
    });
    tx_bg2
        .send(open_worker::ResultMessage { token: 3, kind: open_worker::ResultKind::Converted(Ok(Arc::new(fixture(2)))) })
        .expect("send second converted bg");
    app2.poll_pending_opens();
    assert_eq!(app2.views.len(), 2);
    assert_eq!(app2.active, Some(0), "background Converted completion must preserve an already-active document tab");

    let mut app3 = PdfCraftApp::new();
    let (tx_fg, rx_fg) = std::sync::mpsc::channel();
    app3.pending_opens.push(crate::PendingOpen {
        token: 2,
        receiver: rx_fg,
        name: "Deck.pptx".into(),
        path: None,
        stamp: None,
        activate: true,
        after_open: None,
        recovered: None,
        then: None,
        display_override: Some((PageLayout::TwoUp, false)),
        reopen_page: None,
        from_session_restore: false,
        ready: None,
    });
    app3.set_launch_layout("continuous").expect("defer --layout continuous");
    tx_fg
        .send(open_worker::ResultMessage { token: 2, kind: open_worker::ResultKind::Converted(Ok(Arc::new(fixture(2)))) })
        .expect("send converted fg");
    app3.poll_pending_opens();
    assert_eq!(app3.views.len(), 1);
    assert_eq!(app3.active, Some(0));
    assert_eq!(app3.views[0].layout, PageLayout::Single);
    assert!(app3.views[0].continuous(), "Converted tab recorded launch_display_target for deferred --layout");
}

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn pending_open_coalescing_preserves_requested_tool_and_combine_focus() {
    let file = tool_open_file("coalesce-tool", "pdf", &fixture(3));
    let path = crate::persist::last_session::absolute(&file.0.to_string_lossy());
    let (gate, started, release, finished) = open_worker::prepare_gate();

    let mut harness = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate);
        app.attach_context(&creation.egui_ctx);
        assert_eq!(app.open_session_file(&path, 2, false), crate::OpenOutcome::Pending);
        assert_eq!(app.open_path_then(&path, Some("export.text")), crate::OpenOutcome::Pending);
        app.combine_paths(&[]);
        assert_eq!(app.pending_opens.len(), 1);
        assert_eq!(app.pending_opens[0].then, Some("export.text"), "coalescing preserved requested tool");
        assert!(app.pending_opens[0].activate, "pending open with tool retains focus against Combine");
        app
    });

    release_one(&started, &release, &finished, &mut harness);

    let app = harness.state();
    assert!(app.pending_opens.is_empty());
    assert_eq!(app.views.len(), 1);
    assert_eq!(app.active, Some(0));
    assert_eq!(app.dialog, Some(Dialog::Export(crate::export_ui::ExportKind::Text)), "requested tool executed on completion");
}

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn restored_session_falls_back_to_surviving_tab_when_saved_active_fails_or_cancels() {
    // Case 1: saved active file is encrypted and password prompt is cancelled -> surviving restored tab takes focus.
    let plain1 = tool_open_file("fallback-plain-cancel", "pdf", &fixture(4));
    let locked1 = tool_open_file("fallback-locked-cancel", "pdf", &encrypted_tool_fixture());
    let plain1_path = crate::persist::last_session::absolute(&plain1.0.to_string_lossy());
    let locked1_path = crate::persist::last_session::absolute(&locked1.0.to_string_lossy());
    let (gate1, started1, release1, finished1) = open_worker::prepare_gate();

    let mut harness1 = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate1);
        app.reopen_last_session = true;
        app.last_session = crate::persist::last_session::LastSession {
            files: vec![
                crate::persist::last_session::SessionFile { path: plain1_path, page: 2 },
                crate::persist::last_session::SessionFile { path: locked1_path.clone(), page: 1 },
            ],
            active: Some(locked1_path),
        };
        app.attach_context(&creation.egui_ctx);
        app.reopen_last_files(&[]);
        app
    });

    release_one(&started1, &release1, &finished1, &mut harness1);
    release_one(&started1, &release1, &finished1, &mut harness1);
    assert!(harness1.state().password_prompt.is_some());
    harness1.state_mut().submit_password(None);
    assert_eq!(harness1.state().views.len(), 1);
    assert_eq!(harness1.state().active, Some(0), "surviving restored tab took focus after active password cancel");
    assert_eq!(harness1.state().views[0].current, 2);

    // Case 2: saved active file fails preparation (corrupt PDF) -> surviving restored tab takes focus.
    let plain2 = tool_open_file("fallback-plain-corrupt", "pdf", &fixture(3));
    let corrupt2 = tool_open_file("fallback-corrupt", "pdf", b"%PDF-1.7\nnot a valid pdf");
    let plain2_path = crate::persist::last_session::absolute(&plain2.0.to_string_lossy());
    let corrupt2_path = crate::persist::last_session::absolute(&corrupt2.0.to_string_lossy());
    let (gate2, started2, release2, finished2) = open_worker::prepare_gate();

    let mut harness2 = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate2);
        app.reopen_last_session = true;
        app.last_session = crate::persist::last_session::LastSession {
            files: vec![
                crate::persist::last_session::SessionFile { path: plain2_path, page: 1 },
                crate::persist::last_session::SessionFile { path: corrupt2_path.clone(), page: 0 },
            ],
            active: Some(corrupt2_path),
        };
        app.attach_context(&creation.egui_ctx);
        app.reopen_last_files(&[]);
        app
    });

    release_one(&started2, &release2, &finished2, &mut harness2);
    release_one(&started2, &release2, &finished2, &mut harness2);
    assert_eq!(harness2.state().views.len(), 1);
    assert_eq!(harness2.state().active, Some(0), "surviving restored tab took focus after active parse failure");

    // Case 3: explicit `--home` still keeps Home active even when fallback triggers.
    let plain3 = tool_open_file("fallback-plain-home", "pdf", &fixture(3));
    let corrupt3 = tool_open_file("fallback-corrupt-home", "pdf", b"%PDF-1.7\nnot a valid pdf");
    let plain3_path = crate::persist::last_session::absolute(&plain3.0.to_string_lossy());
    let corrupt3_path = crate::persist::last_session::absolute(&corrupt3.0.to_string_lossy());
    let (gate3, started3, release3, finished3) = open_worker::prepare_gate();

    let mut harness3 = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate3);
        app.reopen_last_session = true;
        app.last_session = crate::persist::last_session::LastSession {
            files: vec![
                crate::persist::last_session::SessionFile { path: plain3_path, page: 1 },
                crate::persist::last_session::SessionFile { path: corrupt3_path.clone(), page: 0 },
            ],
            active: Some(corrupt3_path),
        };
        app.attach_context(&creation.egui_ctx);
        app.reopen_last_files(&[]);
        app.set_launch_option("home", "").expect("defer --home");
        app
    });

    release_one(&started3, &release3, &finished3, &mut harness3);
    release_one(&started3, &release3, &finished3, &mut harness3);
    assert_eq!(harness3.state().views.len(), 1);
    assert_eq!(harness3.state().active, None, "explicit --home wins over restored fallback");

    // Case 4: interactive Home button click while a saved-active restore is still unresolved
    // cancels the fallback so Home stays active when the saved-active file fails or cancels.
    let plain4 = tool_open_file("fallback-plain-interactive-home", "pdf", &fixture(3));
    let locked4 = tool_open_file("fallback-locked-interactive-home", "pdf", &encrypted_tool_fixture());
    let plain4_path = crate::persist::last_session::absolute(&plain4.0.to_string_lossy());
    let locked4_path = crate::persist::last_session::absolute(&locked4.0.to_string_lossy());
    let (gate4, started4, release4, finished4) = open_worker::prepare_gate();

    let mut harness4 = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").expect("language en");
        app.open_prepare_gate = Some(gate4);
        app.reopen_last_session = true;
        app.last_session = crate::persist::last_session::LastSession {
            files: vec![
                crate::persist::last_session::SessionFile { path: plain4_path, page: 1 },
                crate::persist::last_session::SessionFile { path: locked4_path.clone(), page: 0 },
            ],
            active: Some(locked4_path),
        };
        app.attach_context(&creation.egui_ctx);
        app.reopen_last_files(&[]);
        app
    });

    // Background tab arrives first while the saved-active encrypted file is still paused at the gate.
    release_one(&started4, &release4, &finished4, &mut harness4);
    assert_eq!(harness4.state().views.len(), 1);
    assert_eq!(harness4.state().active, None);
    // User explicitly clicks the tab-strip Home button before the saved-active file resolves.
    harness4.get_by_label("Home").click();
    harness4.step();
    // Now release the saved-active encrypted file and cancel its password prompt.
    release_one(&started4, &release4, &finished4, &mut harness4);
    assert!(harness4.state().password_prompt.is_some());
    harness4.state_mut().submit_password(None);
    assert_eq!(harness4.state().views.len(), 1);
    assert_eq!(harness4.state().active, None, "interactive Home click stays active after saved-active password cancel");

    // Case 5: interactive Home button click while saved-active restore is pending prevents a
    // successful delayed completion from stealing focus from Home.
    let plain5 = tool_open_file("suppress-home-bg", "pdf", &fixture(4));
    let active5 = tool_open_file("suppress-home-active", "pdf", &fixture(4));
    let plain5_path = crate::persist::last_session::absolute(&plain5.0.to_string_lossy());
    let active5_path = crate::persist::last_session::absolute(&active5.0.to_string_lossy());
    let (gate5, started5, release5, finished5) = open_worker::prepare_gate();

    let mut harness5 = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").expect("language en");
        app.open_prepare_gate = Some(gate5);
        app.reopen_last_session = true;
        app.last_session = crate::persist::last_session::LastSession {
            files: vec![
                crate::persist::last_session::SessionFile { path: plain5_path, page: 1 },
                crate::persist::last_session::SessionFile { path: active5_path.clone(), page: 3 },
            ],
            active: Some(active5_path),
        };
        app.attach_context(&creation.egui_ctx);
        app.reopen_last_files(&[]);
        app
    });

    release_one(&started5, &release5, &finished5, &mut harness5);
    assert_eq!(harness5.state().views.len(), 1);
    assert!(harness5.state().pending_opens[0].activate, "saved-active restore starts with activate=true");
    harness5.get_by_label("Home").click();
    harness5.step();
    assert!(!harness5.state().pending_opens[0].activate, "explicit Home click revoked saved-active activation");
    release_one(&started5, &release5, &finished5, &mut harness5);
    assert_eq!(harness5.state().views.len(), 2);
    assert_eq!(harness5.state().active, None, "successful delayed restore did not steal focus from Home");
    assert_eq!(harness5.state().views[1].current, 3, "delayed restore still applied saved page in background");

    // Case 6: explicit tab selection while saved-active restore is pending prevents a successful
    // delayed completion from stealing focus from the chosen tab.
    let plain6 = tool_open_file("suppress-tab-bg", "pdf", &fixture(4));
    let active6 = tool_open_file("suppress-tab-active", "pdf", &fixture(4));
    let plain6_path = crate::persist::last_session::absolute(&plain6.0.to_string_lossy());
    let active6_path = crate::persist::last_session::absolute(&active6.0.to_string_lossy());
    let (gate6, started6, release6, finished6) = open_worker::prepare_gate();

    let mut harness6 = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate6);
        app.reopen_last_session = true;
        app.last_session = crate::persist::last_session::LastSession {
            files: vec![
                crate::persist::last_session::SessionFile { path: plain6_path, page: 1 },
                crate::persist::last_session::SessionFile { path: active6_path.clone(), page: 2 },
            ],
            active: Some(active6_path),
        };
        app.attach_context(&creation.egui_ctx);
        app.reopen_last_files(&[]);
        app
    });

    release_one(&started6, &release6, &finished6, &mut harness6);
    assert_eq!(harness6.state().views.len(), 1);
    harness6.state_mut().select_tab(0);
    assert!(!harness6.state().pending_opens[0].activate, "explicit tab selection revoked saved-active activation");
    release_one(&started6, &release6, &finished6, &mut harness6);
    assert_eq!(harness6.state().views.len(), 2);
    assert_eq!(harness6.state().active, Some(0), "successful delayed restore did not steal focus from selected tab");
    assert_eq!(harness6.state().views[1].current, 2, "delayed restore still applied saved page in background");
}
