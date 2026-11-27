mod focus;
mod precedence;

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
    let r_id = app.pending_opens[0].token;
    let c_id = app.pending_opens[1].token;
    app.pending_opens[0].receiver = restored_rx;
    app.pending_opens[1].receiver = cli_rx;

    cli_tx
        .send(open_worker::ResultMessage {
            token: c_id,
            kind: open_worker::ResultKind::Prepared { bytes: cli_bytes, result: Box::new(cli_prepared) },
        })
        .expect("send cli reply");
    app.poll_pending_opens();
    assert_eq!(app.views.len(), 1);
    assert_eq!(app.active, Some(0), "CLI tab is active as soon as it arrives");

    restored_tx
        .send(open_worker::ResultMessage {
            token: r_id,
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
#[allow(clippy::expect_used)]
fn worker_shutdown_discards_queued_opens_and_finishes_the_active_one() {
    let (gate, started, release, _finished) = open_worker::prepare_gate();
    let queue = Arc::new((Mutex::new(open_worker::Queue { jobs: Default::default(), shutdown: false, repaint: None }), std::sync::Condvar::new()));
    let worker = open_worker::OpenWorker { queue: queue.clone(), gate: Some(gate) };
    let (stopped, stopped_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        open_worker::run(queue);
        let _ = stopped.send(());
    });
    let data = Arc::new(fixture(1));
    let context = pdfcraft_engine::Session::new().open_context();
    let first = worker.submit(1, context.clone(), "active.pdf".into(), None, None, open_worker::Input::Bytes(data.clone())).expect("submit first");
    started.recv_timeout(Duration::from_secs(5)).expect("active open reached prepare barrier");
    let second = worker.submit(2, context, "queued.pdf".into(), None, None, open_worker::Input::Bytes(data)).expect("queue second");
    assert!(matches!(second.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));

    drop(worker);
    assert!(matches!(second.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));
    release.send(()).expect("release active open");
    let result = first.recv_timeout(Duration::from_secs(5)).expect("active open completed");
    assert_eq!(result.token, 1);
    assert!(matches!(result.kind, open_worker::ResultKind::Prepared { result, .. } if result.is_ok()));
    stopped_rx.recv_timeout(Duration::from_secs(5)).expect("worker exited after pending opens were discarded");
    thread.join().expect("worker did not panic");
}
