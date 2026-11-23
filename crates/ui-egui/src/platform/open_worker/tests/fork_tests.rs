use super::*;
use crate::canvas::PageLayout;
use crate::state::{Dialog, LeftPanel, PropsTab};

fn fixture_with_named_dest(pages: usize, name: &str, target_page: usize) -> Vec<u8> {
    let target_obj = 4 + 2 * target_page;
    let mut objects = vec![format!("<< /Type /Catalog /Pages 2 0 R /Dests << /{name} [{target_obj} 0 R /Fit] >> >>")];
    let kids = (0..pages).map(|page| format!("{} 0 R", 4 + 2 * page)).collect::<Vec<_>>().join(" ");
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {pages} /MediaBox [0 0 200 300] >>"));
    objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for page in 0..pages {
        objects.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * page));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", page + 1);
        objects.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

#[allow(clippy::expect_used)]
fn release_one(
    started: &std::sync::mpsc::Receiver<()>,
    release: &std::sync::mpsc::Sender<()>,
    finished: &std::sync::mpsc::Receiver<()>,
    harness: &mut Harness<'static, PdfCraftApp>,
) {
    started.recv_timeout(Duration::from_secs(5)).expect("worker reached prepare barrier");
    release.send(()).expect("release prepare barrier");
    finished.recv_timeout(Duration::from_secs(5)).expect("worker finished preparation");
    harness.step();
}

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
    let (gate, started, release, finished) = open_worker::prepare_gate();

    let mut harness = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate);
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

    release_one(&started, &release, &finished, &mut harness);
    release_one(&started, &release, &finished, &mut harness);

    let app = harness.state();
    assert!(app.pending_opens.is_empty());
    assert!(app.deferred_opens.is_empty());
    assert_eq!(app.views.len(), 2);
    assert_eq!(app.active, Some(0), "restored active tab stays active");
    assert_eq!(app.views[0].current, 2, "first tab restored to saved page");
    assert_eq!(app.views[1].current, 1, "second tab restored to saved page");
}

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn cli_open_keeps_focus_and_launch_options_when_restored_file_finishes_later() {
    let restored = tool_open_file("race-restored", "pdf", &fixture(4));
    let cli = tool_open_file("race-cli", "pdf", &fixture(5));
    let restored_path = crate::persist::last_session::absolute(&restored.0.to_string_lossy());
    let cli_path = crate::persist::last_session::absolute(&cli.0.to_string_lossy());
    let (gate, _started, _release, _finished) = open_worker::prepare_gate();

    let mut harness = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate);
        app.reopen_last_session = true;
        app.last_session = crate::persist::last_session::LastSession {
            files: vec![crate::persist::last_session::SessionFile { path: restored_path.clone(), page: 2 }],
            active: Some(restored_path),
        };
        app.attach_context(&creation.egui_ctx);
        app.reopen_last_files(std::slice::from_ref(&cli_path));
        assert_eq!(app.open_path(&cli_path), crate::OpenOutcome::Pending);
        app.set_launch_option("page", "4").expect("defer --page");
        app.set_launch_option("zoom", "150").expect("defer --zoom");
        assert_eq!(app.pending_opens.len(), 2);
        assert!(!app.pending_opens[0].activate, "restored session file must not steal focus from CLI file");
        assert!(app.pending_opens[1].activate, "CLI file opens active");
        app
    });

    // Replace worker receivers with controlled channels so the CLI file completes BEFORE the
    // restored session file, exercising the exact out-of-order completion race.
    let app = harness.state_mut();
    let restored_bytes = Arc::new(fixture(4));
    let cli_bytes = Arc::new(fixture(5));
    let restored_prepared = app.session.prepare_open("race-restored.pdf", app.pending_opens[0].path.clone(), restored_bytes.clone(), None);
    let cli_prepared = app.session.prepare_open("race-cli.pdf", app.pending_opens[1].path.clone(), cli_bytes.clone(), None);
    let (restored_tx, restored_rx) = std::sync::mpsc::channel();
    let (cli_tx, cli_rx) = std::sync::mpsc::channel();
    let restored_token = app.pending_opens[0].token;
    let cli_token = app.pending_opens[1].token;
    app.pending_opens[0].receiver = restored_rx;
    app.pending_opens[1].receiver = cli_rx;

    cli_tx
        .send(open_worker::ResultMessage {
            token: cli_token,
            kind: open_worker::ResultKind::Prepared { bytes: cli_bytes, result: Box::new(cli_prepared) },
        })
        .expect("send cli reply");
    app.poll_pending_opens();
    assert_eq!(app.views.len(), 1);
    assert_eq!(app.active, Some(0), "CLI tab is active as soon as it arrives");

    restored_tx
        .send(open_worker::ResultMessage {
            token: restored_token,
            kind: open_worker::ResultKind::Prepared { bytes: restored_bytes, result: Box::new(restored_prepared) },
        })
        .expect("send restored reply");
    app.poll_pending_opens();

    assert!(app.pending_opens.is_empty());
    assert_eq!(app.views.len(), 2);
    assert_eq!(app.active, Some(0), "late-finishing restored tab must not steal focus from CLI tab");
    assert_eq!(app.views[0].current, 3, "deferred --page 4 landed on CLI tab");
    assert!((app.views[0].zoom - 1.5).abs() < 0.01, "deferred --zoom 150 landed on CLI tab");
    assert_eq!(app.views[1].current, 2, "restored tab still restored its saved page in background");
}

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn cli_open_parameter_overrides_restored_session_page_and_deduplicates_session() {
    let manual = tool_open_file("named-manual", "pdf", &fixture_with_named_dest(5, "Intro", 1));
    let other = tool_open_file("named-other", "pdf", &fixture(4));
    let manual_path = crate::persist::last_session::absolute(&manual.0.to_string_lossy());
    let other_path = crate::persist::last_session::absolute(&other.0.to_string_lossy());
    let cli_arg = format!("{manual_path}#nameddest=Intro");
    let (gate, started, release, finished) = open_worker::prepare_gate();

    let manual_for_build = manual_path.clone();
    let other_for_build = other_path.clone();
    let cli_for_build = cli_arg.clone();
    let mut harness = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate);
        app.reopen_last_session = true;
        app.last_session = crate::persist::last_session::LastSession {
            files: vec![
                crate::persist::last_session::SessionFile { path: manual_for_build.clone(), page: 4 },
                crate::persist::last_session::SessionFile { path: manual_for_build, page: 3 },
                crate::persist::last_session::SessionFile { path: other_for_build.clone(), page: 2 },
                crate::persist::last_session::SessionFile { path: other_for_build, page: 1 },
            ],
            active: Some("/nonexistent/stale-session-entry.pdf".into()),
        };
        app.attach_context(&creation.egui_ctx);
        app.reopen_last_files(std::slice::from_ref(&cli_for_build));
        assert_eq!(app.pending_opens.len(), 1, "CLI file with #nameddest skipped and duplicate session entries deduplicated");
        assert_eq!(app.open_path(&cli_for_build), crate::OpenOutcome::Pending);
        assert_eq!(app.pending_opens.len(), 2, "one restored tab plus one CLI tab");
        app
    });

    release_one(&started, &release, &finished, &mut harness);
    release_one(&started, &release, &finished, &mut harness);

    // Wait for the off-thread named destination lookup (`#nameddest=Intro` -> page 2, index 1).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !harness.state().pending_destinations.is_empty() && std::time::Instant::now() < deadline {
        harness.step();
        std::thread::yield_now();
    }
    harness.step();

    let app = harness.state();
    assert!(app.pending_opens.is_empty());
    assert!(app.pending_destinations.is_empty(), "named destination resolved");
    assert_eq!(app.views.len(), 2, "no duplicate tabs opened");
    let manual_idx = app.tab_showing(&manual_path).expect("manual tab open");
    let other_idx = app.tab_showing(&other_path).expect("other tab open");
    assert_eq!(app.active, Some(manual_idx), "CLI file stays active over stale session active");
    assert_eq!(app.views[manual_idx].current, 1, "CLI #nameddest=Intro won over restored page 4");
    assert_eq!(app.views[other_idx].current, 2, "first duplicate session entry page preserved");

    // Defense-in-depth: coalescing an explicit #page=2 open onto a pending session open clears reopen_page.
    let manual2 = tool_open_file("coalesce-manual", "pdf", &fixture(5));
    let manual2_path = crate::persist::last_session::absolute(&manual2.0.to_string_lossy());
    let (gate2, started2, release2, finished2) = open_worker::prepare_gate();
    let manual2_for_build = manual2_path.clone();
    let mut harness2 = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate2);
        app.attach_context(&creation.egui_ctx);
        assert_eq!(app.open_session_file(&manual2_for_build, 4, false), crate::OpenOutcome::Pending);
        assert_eq!(app.open_path(&format!("{manual2_for_build}#page=2")), crate::OpenOutcome::Pending);
        assert_eq!(app.pending_opens.len(), 1);
        assert!(app.pending_opens[0].reopen_page.is_none(), "explicit destination clears session reopen_page");
        assert!(app.pending_opens[0].activate, "explicit CLI open upgrades activation");
        app
    });
    release_one(&started2, &release2, &finished2, &mut harness2);
    let app2 = harness2.state();
    assert_eq!(app2.views.len(), 1);
    assert_eq!(app2.active, Some(0));
    assert_eq!(app2.views[0].current, 1, "coalesced #page=2 won over session page 4");
}

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn combine_keeps_focus_over_restored_queue_overflow_and_encrypted_files() {
    for unlock in [true, false] {
        let total_restored = open_worker::MAX_QUEUED_OPENS + 2;
        let locked = tool_open_file(&format!("combine-locked-{unlock}"), "pdf", &encrypted_tool_fixture());
        let plain: Vec<ToolOpenFile> =
            (1..total_restored).map(|i| tool_open_file(&format!("combine-plain-{unlock}-{i}"), "pdf", &fixture(2))).collect();
        let combine_input = tool_open_file(&format!("combine-input-{unlock}"), "pdf", &fixture(1));

        let mut session_files =
            vec![crate::persist::last_session::SessionFile { path: crate::persist::last_session::absolute(&locked.0.to_string_lossy()), page: 1 }];
        for f in &plain {
            session_files
                .push(crate::persist::last_session::SessionFile { path: crate::persist::last_session::absolute(&f.0.to_string_lossy()), page: 1 });
        }
        let active_path = session_files.last().expect("non-empty").path.clone();
        let combine_path = combine_input.0.to_string_lossy().into_owned();
        let (gate, started, release, finished) = open_worker::prepare_gate();

        let mut harness = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
            let mut app = PdfCraftApp::new();
            app.open_prepare_gate = Some(gate);
            app.reopen_last_session = true;
            app.last_session = crate::persist::last_session::LastSession { files: session_files, active: Some(active_path) };
            app.attach_context(&creation.egui_ctx);
            app.reopen_last_files(&[]);
            assert_eq!(app.pending_opens.len(), open_worker::MAX_QUEUED_OPENS);
            assert_eq!(app.deferred_opens.len(), 2);
            app.combine_paths(&[combine_path]);
            assert!(app.combine_showing(), "Combine tab takes focus");
            assert!(app.pending_opens.iter().all(|p| !p.activate), "pending opens yield focus to Combine");
            assert!(app.deferred_opens.iter().all(|d| !d.activate), "deferred opens yield focus to Combine");
            app
        });

        // First restored file is encrypted and raises the password prompt with activate = false.
        release_one(&started, &release, &finished, &mut harness);
        {
            let prompt = harness.state().password_prompt.as_ref().expect("encrypted restored file raised prompt");
            assert!(!prompt.activate, "prompt keeps background focus under Combine");
            assert!(harness.state().combine_showing());
        }

        if unlock {
            harness.state_mut().submit_password(Some("tool-password".into()));
            // Release all remaining jobs: the priority retry + the remaining plain files (including deferred overflow).
            for _ in 0..total_restored {
                release_one(&started, &release, &finished, &mut harness);
            }
        } else {
            harness.state_mut().submit_password(None);
            assert!(harness.state().password_prompt.is_none());
            // All remaining plain files (pending + deferred overflow) drain cleanly.
            for _ in 1..total_restored {
                release_one(&started, &release, &finished, &mut harness);
            }
        }

        let app = harness.state();
        assert!(app.pending_opens.is_empty(), "unlock={unlock}: pending_opens drained");
        assert!(app.deferred_opens.is_empty(), "unlock={unlock}: deferred_opens drained");
        assert!(app.password_prompt.is_none(), "unlock={unlock}: password_prompt cleared");
        let expected_views = if unlock { total_restored } else { total_restored - 1 };
        assert_eq!(app.views.len(), expected_views, "unlock={unlock}: all non-cancelled restored files opened");
        assert!(app.combine_showing(), "unlock={unlock}: Combine tab retained focus throughout");
    }
}

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn two_paged_and_deferred_launch_options_preserve_cli_order_and_modal_safety() {
    // Case A: `--two-paged --continuous on --layout single --dialog properties` -> final `(Single, false)` + dialog.
    let file_a = tool_open_file("order-a", "pdf", &fixture(3));
    let path_a = file_a.0.to_string_lossy().into_owned();
    let (gate_a, started_a, release_a, finished_a) = open_worker::prepare_gate();
    let mut harness_a = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate_a);
        app.attach_context(&creation.egui_ctx);
        app.open_path_with_display(&path_a, PageLayout::TwoUp, false);
        app.set_launch_option("continuous", "on").expect("defer --continuous");
        app.set_launch_layout("single").expect("queue --layout after --continuous");
        app.set_launch_option("dialog", "properties").expect("defer --dialog");
        assert!(app.dialog.is_none(), "deferred dialog must not open before worker finishes");
        assert!(!app.modal_open(), "no immediate modal holds worker replies");
        app
    });
    release_one(&started_a, &release_a, &finished_a, &mut harness_a);
    let app_a = harness_a.state();
    assert_eq!(app_a.views.len(), 1);
    assert_eq!(app_a.views[0].layout, PageLayout::Single);
    assert!(!app_a.views[0].continuous(), "--layout single after --continuous on wins");
    assert_eq!(app_a.dialog, Some(Dialog::Properties(PropsTab::Description)));

    // Case B: `--two-paged --layout single --continuous on` -> final `(Single, true)`.
    let file_b = tool_open_file("order-b", "pdf", &fixture(3));
    let path_b = file_b.0.to_string_lossy().into_owned();
    let (gate_b, started_b, release_b, finished_b) = open_worker::prepare_gate();
    let mut harness_b = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate_b);
        app.attach_context(&creation.egui_ctx);
        app.open_path_with_display(&path_b, PageLayout::TwoUp, false);
        app.set_launch_layout("single").expect("set --layout");
        app.set_launch_option("continuous", "on").expect("defer --continuous after --layout");
        app.set_launch_option("tool", "organize").expect("defer --tool");
        app.set_launch_option("left", "closed").expect("defer --left after --tool");
        app
    });
    release_one(&started_b, &release_b, &finished_b, &mut harness_b);
    let app_b = harness_b.state();
    assert_eq!(app_b.views.len(), 1);
    assert_eq!(app_b.views[0].layout, PageLayout::Single);
    assert!(app_b.views[0].continuous(), "--continuous on after --layout single wins");
    assert_eq!(app_b.left, LeftPanel::Tool("organize"));
    assert!(!app_b.left_open, "--left closed after --tool organize stays closed");
}

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

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn explicit_navigation_commands_suppress_delayed_session_restore_activation() {
    #[derive(Clone, Copy, Debug)]
    enum NavMode {
        NextTab,
        WindowDocument,
        SplitPane,
    }
    for (mode, label) in [(NavMode::NextTab, "next-tab"), (NavMode::WindowDocument, "window-document"), (NavMode::SplitPane, "split-pane")] {
        let plain = tool_open_file(&format!("nav-{label}-bg"), "pdf", &fixture(4));
        let active = tool_open_file(&format!("nav-{label}-active"), "pdf", &fixture(4));
        let plain_path = crate::persist::last_session::absolute(&plain.0.to_string_lossy());
        let active_path = crate::persist::last_session::absolute(&active.0.to_string_lossy());
        let (gate, started, release, finished) = open_worker::prepare_gate();

        let mut harness = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
            let mut app = PdfCraftApp::new();
            app.open_prepare_gate = Some(gate);
            app.reopen_last_session = true;
            app.last_session = crate::persist::last_session::LastSession {
                files: vec![
                    crate::persist::last_session::SessionFile { path: plain_path, page: 1 },
                    crate::persist::last_session::SessionFile { path: active_path.clone(), page: 3 },
                ],
                active: Some(active_path),
            };
            app.attach_context(&creation.egui_ctx);
            app.reopen_last_files(&[]);
            app
        });

        // Background tab arrives while saved-active document is still pending at the gate.
        release_one(&started, &release, &finished, &mut harness);
        assert_eq!(harness.state().views.len(), 1);
        assert!(harness.state().pending_opens[0].activate, "{label}: saved-active starts with activate=true");

        match mode {
            NavMode::NextTab => {
                harness.state_mut().execute("view.next_tab");
                assert_eq!(harness.state().active, Some(0));
            }
            NavMode::WindowDocument => {
                let ctx = harness.ctx.clone();
                let cmd = craft_menu::CommandRef::with_params(crate::menus::DOCUMENT, json!({ "index": 0 }));
                harness.state_mut().run_menu_command(&ctx, &cmd);
                assert_eq!(harness.state().active, Some(0));
            }
            NavMode::SplitPane => {
                harness.state_mut().split_tab(0, false);
                harness.state_mut().activate_place(crate::panes::Place::Main);
                assert_eq!(harness.state().active, Some(0));
            }
        }

        assert!(!harness.state().pending_opens[0].activate, "{label}: explicit navigation revoked saved-active activation");
        release_one(&started, &release, &finished, &mut harness);

        let app = harness.state();
        assert_eq!(app.active, Some(0), "{label}: delayed saved-active completion did not steal focus");
        let last_view = app.views.last().expect("delayed restored tab opened");
        assert_eq!(last_view.current, 3, "{label}: delayed restored tab kept saved page in background");
    }
}

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn create_blank_and_open_created_suppress_delayed_session_restore_activation() {
    for use_open_created in [false, true] {
        let active = tool_open_file(&format!("create-restore-{use_open_created}"), "pdf", &fixture(4));
        let active_path = crate::persist::last_session::absolute(&active.0.to_string_lossy());
        let (gate, started, release, finished) = open_worker::prepare_gate();

        let mut harness = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
            let mut app = PdfCraftApp::new();
            app.open_prepare_gate = Some(gate);
            app.reopen_last_session = true;
            app.last_session = crate::persist::last_session::LastSession {
                files: vec![crate::persist::last_session::SessionFile { path: active_path.clone(), page: 2 }],
                active: Some(active_path),
            };
            app.attach_context(&creation.egui_ctx);
            app.reopen_last_files(&[]);
            assert_eq!(app.pending_opens.len(), 1);
            assert!(app.pending_opens[0].activate);
            app
        });

        harness.step();
        started.recv_timeout(Duration::from_secs(5)).expect("worker reached prepare barrier");
        if use_open_created {
            harness.state_mut().open_created("Generated.pdf", Arc::new(fixture(1)), "Created");
        } else {
            harness.state_mut().execute("create.blank");
        }
        assert_eq!(harness.state().views.len(), 1);
        assert_eq!(harness.state().active, Some(0), "newly created document is active");
        assert!(!harness.state().pending_opens[0].activate, "explicit document creation revoked saved-active activation");

        release.send(()).expect("release prepare barrier");
        finished.recv_timeout(Duration::from_secs(5)).expect("worker finished preparation");
        harness.step();

        let app = harness.state();
        assert!(app.pending_opens.is_empty());
        assert_eq!(app.views.len(), 2);
        assert_eq!(app.active, Some(0), "delayed saved-active restore did not steal focus from created document");
        assert_eq!(app.views[1].current, 2, "restored PDF still applied saved page in background");
    }
}

#[test]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
fn explicit_plain_open_coalesced_with_session_restore_takes_focus() {
    // 1. Pending-open coalescing: restore A (saved active) and B (background). A finishes first,
    // then user explicitly opens plain path B (no #fragment or tool). B must take foreground focus.
    let file_a = tool_open_file("coalesce-plain-a", "pdf", &fixture(4));
    let file_b = tool_open_file("coalesce-plain-b", "pdf", &fixture(4));
    let path_a = crate::persist::last_session::absolute(&file_a.0.to_string_lossy());
    let path_b = crate::persist::last_session::absolute(&file_b.0.to_string_lossy());
    let (gate, started, release, finished) = open_worker::prepare_gate();

    let path_b_for_build = path_b.clone();
    let mut harness = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate);
        app.reopen_last_session = true;
        app.last_session = crate::persist::last_session::LastSession {
            files: vec![
                crate::persist::last_session::SessionFile { path: path_a.clone(), page: 1 },
                crate::persist::last_session::SessionFile { path: path_b_for_build, page: 3 },
            ],
            active: Some(path_a),
        };
        app.attach_context(&creation.egui_ctx);
        app.reopen_last_files(&[]);
        app
    });

    release_one(&started, &release, &finished, &mut harness);
    assert_eq!(harness.state().views.len(), 1);
    assert_eq!(harness.state().active, Some(0));
    assert!(harness.state().pending_opens[0].from_session_restore);

    assert_eq!(harness.state_mut().open_path(&path_b), crate::OpenOutcome::Pending);
    assert_eq!(harness.state().pending_opens.len(), 1);
    assert!(harness.state().pending_opens[0].activate, "explicit plain open upgraded activation");
    assert!(!harness.state().pending_opens[0].from_session_restore, "explicit plain open cleared session-only origin");
    assert_eq!(harness.state().pending_opens[0].reopen_page, Some(3), "saved page retained when no fragment given");

    release_one(&started, &release, &finished, &mut harness);
    assert_eq!(harness.state().views.len(), 2);
    assert_eq!(harness.state().active, Some(1), "explicitly opened B took focus over already-active A");
    assert_eq!(harness.state().views[1].current, 3);

    // 2. Deferred-open coalescing: overflow session restore into deferred_opens, then explicitly
    // open the deferred path -> clears from_session_restore and upgrades activate = true.
    let overflow_files: Vec<ToolOpenFile> =
        (0..open_worker::MAX_QUEUED_OPENS + 1).map(|i| tool_open_file(&format!("coalesce-def-{i}"), "pdf", &fixture(2))).collect();
    let deferred_path = crate::persist::last_session::absolute(&overflow_files.last().expect("last").0.to_string_lossy());
    let session_files: Vec<crate::persist::last_session::SessionFile> = overflow_files
        .iter()
        .map(|f| crate::persist::last_session::SessionFile { path: crate::persist::last_session::absolute(&f.0.to_string_lossy()), page: 1 })
        .collect();
    let first_active = session_files[0].path.clone();
    let (gate2, _started2, _release2, _finished2) = open_worker::prepare_gate();
    let deferred_for_build = deferred_path.clone();
    let mut harness2 = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate2);
        app.reopen_last_session = true;
        app.last_session = crate::persist::last_session::LastSession { files: session_files, active: Some(first_active) };
        app.attach_context(&creation.egui_ctx);
        app.reopen_last_files(&[]);
        assert_eq!(app.deferred_opens.len(), 1);
        assert!(app.deferred_opens[0].from_session_restore);
        assert!(!app.deferred_opens[0].activate);
        assert_eq!(app.open_path(&deferred_for_build), crate::OpenOutcome::Pending);
        assert_eq!(app.deferred_opens.len(), 1);
        assert!(app.deferred_opens[0].activate, "explicit open upgraded deferred activation");
        assert!(!app.deferred_opens[0].from_session_restore, "explicit open cleared deferred session-only origin");
        app
    });
    harness2.state_mut().select_home();
    assert!(harness2.state().deferred_opens[0].activate, "Home click must not suppress explicitly upgraded deferred open");

    // 3. Password-prompt coalescing: encrypted background restore asks for password whilst another
    // tab is active; explicit plain open clears from_session_restore so unlocking takes focus.
    let plain3 = tool_open_file("coalesce-pw-plain", "pdf", &fixture(2));
    let locked3 = tool_open_file("coalesce-pw-locked", "pdf", &encrypted_tool_fixture());
    let plain3_path = crate::persist::last_session::absolute(&plain3.0.to_string_lossy());
    let locked3_path = crate::persist::last_session::absolute(&locked3.0.to_string_lossy());
    let (gate3, started3, release3, finished3) = open_worker::prepare_gate();
    let locked3_for_build = locked3_path.clone();
    let mut harness3 = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).build_eframe(move |creation| {
        let mut app = PdfCraftApp::new();
        app.open_prepare_gate = Some(gate3);
        app.reopen_last_session = true;
        app.last_session = crate::persist::last_session::LastSession {
            files: vec![
                crate::persist::last_session::SessionFile { path: plain3_path.clone(), page: 0 },
                crate::persist::last_session::SessionFile { path: locked3_for_build, page: 1 },
            ],
            active: Some(plain3_path),
        };
        app.attach_context(&creation.egui_ctx);
        app.reopen_last_files(&[]);
        app
    });

    release_one(&started3, &release3, &finished3, &mut harness3);
    release_one(&started3, &release3, &finished3, &mut harness3);
    assert_eq!(harness3.state().active, Some(0));
    {
        let prompt = harness3.state().password_prompt.as_ref().expect("background encrypted restore raised prompt");
        assert!(prompt.from_session_restore);
        assert!(!prompt.activate);
    }
    assert_eq!(harness3.state_mut().open_path(&locked3_path), crate::OpenOutcome::Pending);
    {
        let prompt = harness3.state().password_prompt.as_ref().expect("coalesced onto existing prompt");
        assert!(prompt.activate, "explicit open upgraded prompt activation");
        assert!(!prompt.from_session_restore, "explicit open cleared prompt session-only origin");
    }
    harness3.state_mut().submit_password(Some("tool-password".into()));
    release_one(&started3, &release3, &finished3, &mut harness3);
    assert_eq!(harness3.state().views.len(), 2);
    assert_eq!(harness3.state().active, Some(1), "unlocking explicitly requested encrypted file took focus over already-active tab");
}
