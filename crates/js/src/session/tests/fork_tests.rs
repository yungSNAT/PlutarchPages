//! The fork's tests of the persistent script session.

use super::*;
use crate::fork_test_support::job as basic_job;

#[test]
fn a_refused_document_script_and_a_refused_timer_script_leave_the_session_working() {
    let mut session = ScriptSession::default();
    let mut j = basic_job("app.setTimeOut('('.repeat(100), 10); 'ok';");
    j.doc_scripts = vec!["(".repeat(100)];
    let out = session.run(j.clone(), &[]);
    assert_eq!(out.result.as_deref(), Some("ok"));
    assert_eq!(out.doc_errors.len(), 1);
    assert_eq!(out.timers.len(), 1);

    // The timer falls due: its script is refused without touching the realm.
    let fired = session.run(Job { script: String::new(), timer: Some(1), ..j }, &[]);
    assert!(fired.realm_untouched && fired.error.is_some());
    assert_eq!(session.run(basic_job("'still here';"), &[]).result.as_deref(), Some("still here"));
}

#[test]
fn too_many_document_scripts_are_refused() {
    let mut session = ScriptSession::default();
    let mut j = basic_job("1;");
    j.doc_scripts = vec!["1;".into(); 1025];
    let out = session.run(j, &[]);
    assert!(out.execution_refused && out.realm_untouched);
    assert!(out.error.as_deref().is_some_and(|e| e.contains("exceed their budget")), "{:?}", out.error);
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::time::Duration;

    use super::*;

    /// Run `script`, turned away whilst a retired worker finishes and exits; then a new worker takes it.
    fn run_once_stopped(session: &mut ScriptSession, script: &str) -> Outcome {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let out = session.run(basic_job(script), &[]);
            let busy = out.error.as_deref().is_some_and(|e| e.contains("still stopping") || e.contains("another script is active"));
            if !busy || std::time::Instant::now() >= deadline {
                return out;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_host_task_before_any_script_is_refused() {
        let mut fresh = ScriptSession::default();
        let out = fresh.run_host_task(Duration::from_secs(30), |_| Outcome::default());
        assert!(out.execution_refused && out.realm_untouched);
        assert!(out.error.as_deref().is_some_and(|e| e.contains("not been initialized")));
    }

    #[test]
    fn a_host_task_that_dies_resets_the_realm() {
        let mut session = ScriptSession::default();
        assert_eq!(session.run(basic_job("var x = 1; x;"), &[]).result.as_deref(), Some("1"));
        let crashed = session.run_host_task(Duration::from_secs(30), |_| std::panic::resume_unwind(Box::new("simulated failure")));
        assert!(crashed.execution_refused);
        assert!(crashed.error.as_deref().is_some_and(|e| e.contains("stopped unexpectedly")));
        // A new realm: the variable is gone.
        assert_eq!(session.run(basic_job("typeof x;"), &[]).result.as_deref(), Some("undefined"));
    }

    #[test]
    fn a_disconnected_worker_is_reported_and_the_session_is_idle() {
        let (tx, rx) = mpsc::sync_channel(1);
        drop(rx);
        let mut session = ScriptSession { sender: Some(tx), ..Default::default() };
        let out = session.run(basic_job("1;"), &[]);
        assert!(out.execution_refused && !session.is_executing() && !session.is_pending());
        assert!(out.error.as_deref().is_some_and(|e| e.contains("disconnected")));
    }

    #[test]
    fn a_script_past_its_limit_is_abandoned_and_the_worker_retired() {
        let mut session = ScriptSession::default();
        // Any job outlasts a limit of 1 ns.
        let out = session.run_with_limit(basic_job("1;"), &[], Duration::from_nanos(1));
        assert!(out.execution_refused);
        assert!(out.error.as_deref().is_some_and(|e| e.contains("did not complete")));
        let next = run_once_stopped(&mut session, "2;");
        assert_eq!((next.error, next.result.as_deref()), (None, Some("2")));
    }

    #[test]
    fn no_time_left_refuses_the_job_without_starting_it() {
        let mut session = ScriptSession::default();
        let out = session.run_with_limit(basic_job("1;"), &[], Duration::ZERO);
        assert!(out.execution_refused && out.realm_untouched);
        assert!(out.error.as_deref().is_some_and(|e| e.contains("no time remaining")));
    }

    #[test]
    fn a_reset_after_refusal_starts_a_new_realm_on_the_same_worker() {
        let mut session = ScriptSession::default();
        assert_eq!(session.run(basic_job("var kept = 5; kept;"), &[]).result.as_deref(), Some("5"));
        assert_eq!(session.run(basic_job("typeof kept;"), &[]).result.as_deref(), Some("number"));
        session.reset_after_refusal();
        assert_eq!(session.run(basic_job("typeof kept;"), &[]).result.as_deref(), Some("undefined"));
        assert!(!session.is_executing() && !session.is_pending());
    }

    #[test]
    fn a_retired_worker_is_replaced_by_the_next_job() {
        let mut session = ScriptSession::default();
        assert_eq!(session.run(basic_job("var gone = 1; gone;"), &[]).result.as_deref(), Some("1"));
        session.retire_timed_out_worker();
        assert!(!session.is_pending());
        let out = run_once_stopped(&mut session, "typeof gone;");
        assert_eq!((out.error, out.result.as_deref()), (None, Some("undefined")));
    }
}

#[test]
fn stamping_gives_each_present_object_a_token_and_a_returning_object_a_new_one() {
    let mut session = ScriptSession::default();
    let mut a = FieldState::new("a", crate::FieldType::Text, vec![]);
    a.object_id = Some(7);
    let mut fields = Arc::new(vec![a, FieldState::new("b", crate::FieldType::Text, vec![])]);
    session.stamp_fields(&mut fields);
    let ids = |f: &Arc<Vec<FieldState>>| f.iter().map(|f| f.instance_id).collect::<Vec<_>>();
    assert_eq!(ids(&fields), [1, 0]);
    // Stamping again changes nothing and copies nothing.
    let before = Arc::clone(&fields);
    session.stamp_fields(&mut fields);
    assert!(Arc::ptr_eq(&before, &fields));
    // Once gone and back, object 7 is another instance.
    let mut without = Arc::new(vec![FieldState::new("b", crate::FieldType::Text, vec![])]);
    session.stamp_fields(&mut without);
    drop(before);
    session.stamp_fields(&mut fields);
    assert_eq!(ids(&fields), [2, 0]);
}
