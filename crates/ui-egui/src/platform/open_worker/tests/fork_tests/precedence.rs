use super::*;

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
