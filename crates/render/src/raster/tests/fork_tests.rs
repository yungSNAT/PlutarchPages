use std::time::{Duration, Instant};

#[test]
fn retired_threads_report_unfinished_until_the_worker_exits() {
    let (release, blocked) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || blocked.recv().unwrap());
    let mut retiring = super::super::Retiring(vec![worker]);
    assert!(!retiring.exited());
    assert_eq!(retiring.running(), 1);

    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !retiring.exited() {
        assert!(Instant::now() < deadline, "the released worker did not exit");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(retiring.running(), 0);
}
