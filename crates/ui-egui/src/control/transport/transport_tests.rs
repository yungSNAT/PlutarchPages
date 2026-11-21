/// A failed accept is followed by a wait before the next one, so a listener that keeps
/// failing does not spin, and the connections around the failures still come through.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_failed_accept_waits_before_the_next() {
    let failed = || Err(std::io::Error::other("no file descriptors left"));
    let incoming = vec![failed(), Ok(1), failed(), failed(), Ok(2)];
    let mut waits = 0;
    let connections: Vec<i32> = super::accepted(incoming.into_iter(), || waits += 1).collect();
    assert_eq!(connections, [1, 2]);
    assert_eq!(waits, 3, "one wait per failed accept");
}

/// Before authenticating, a line that isn't JSON gets a parse error and closes the
/// connection, as [`super::serve`] documents for any first request other than `auth`.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn malformed_input_before_auth_closes_the_connection() {
    use std::io::{BufRead, BufReader, ErrorKind, Write};
    let (_control, client) = crate::control::attach(&egui::Context::default());
    let ep = super::serve(client).unwrap();
    let s = std::net::TcpStream::connect(("127.0.0.1", ep.port)).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    writeln!(s.try_clone().unwrap(), "not json").unwrap();
    let mut r = BufReader::new(s);
    let mut first = String::new();
    r.read_line(&mut first).unwrap();
    let reply: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(reply["error"]["code"], -32700, "{first}");
    // Closed: end of stream (or a reset), not a connection left waiting for more input.
    let mut next = String::new();
    let closed = match r.read_line(&mut next) {
        Ok(n) => n == 0,
        Err(e) => matches!(e.kind(), ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted),
    };
    assert!(closed, "the connection stayed open: {next:?}");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn lines_are_read_up_to_the_limit() {
    use super::{LineRead, read_line_limited};
    let mut buf = Vec::new();
    let mut input: &[u8] = b"aaaaaaaaaa\nbb\r\ncc";
    assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::Line);
    assert_eq!(buf, b"aaaaaaaaaa", "a line exactly at the limit is accepted");
    assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::Line);
    assert_eq!(buf, b"bb", "CRLF is stripped");
    assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::Line);
    assert_eq!(buf, b"cc", "a last line without a newline is still read");
    assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::End, "then end of stream");
    let long = [b'x'; 100];
    let mut input: &[u8] = &long;
    assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::TooLong);
    assert_eq!(input.len(), 100 - 11, "at most limit + 1 bytes are read; the rest is left");
}

/// Before authenticating, a line longer than [`super::MAX_AUTH_LINE`] is refused as soon as
/// the limit is passed (no newline needed) and the connection is closed.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn an_overlong_line_before_auth_is_refused() {
    use std::io::{BufRead, BufReader, ErrorKind, Write};
    let (_control, client) = crate::control::attach(&egui::Context::default());
    let ep = super::serve(client).unwrap();
    let s = std::net::TcpStream::connect(("127.0.0.1", ep.port)).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    // Exactly one byte over the limit, without a newline: the server reads it all, so the
    // close below is a clean end of stream.
    s.try_clone().unwrap().write_all(&vec![b'x'; super::MAX_AUTH_LINE + 1]).unwrap();
    let mut r = BufReader::new(s);
    let mut first = String::new();
    r.read_line(&mut first).unwrap();
    let reply: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(reply["error"]["code"], -32600, "{first}");
    let mut next = String::new();
    let closed = match r.read_line(&mut next) {
        Ok(n) => n == 0,
        Err(e) => matches!(e.kind(), ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted),
    };
    assert!(closed, "the connection stayed open: {next:?}");
}

/// Small limits for the tests below: a 300 ms deadline, two places to wait in.
#[cfg(not(target_arch = "wasm32"))]
const TEST_LIMITS: super::Limits = super::Limits {
    auth_deadline: std::time::Duration::from_millis(300),
    max_unauthenticated: 2,
    write_timeout: std::time::Duration::from_secs(5),
    linger: std::time::Duration::from_secs(1),
    max_lingering: 2,
};

/// The next line the server sends, as JSON, or `None` when it closed the connection instead.
#[cfg(not(target_arch = "wasm32"))]
fn next_line(r: &mut std::io::BufReader<std::net::TcpStream>) -> Option<serde_json::Value> {
    use std::io::BufRead;
    let mut line = String::new();
    match r.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(serde_json::from_str(&line).unwrap()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn connect(port: u16) -> std::net::TcpStream {
    let s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    s
}

/// #1806: a connection that never authenticates is closed at the deadline, counted from its
/// acceptance, and a client feeding a byte at a time cannot push that deadline back.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn an_unauthenticated_connection_is_closed_at_the_deadline() {
    use std::io::{BufReader, Write};
    let (_control, client) = crate::control::attach(&egui::Context::default());
    let ep = super::serve_with(client, TEST_LIMITS).unwrap();
    // Idle.
    let started = std::time::Instant::now();
    let mut r = BufReader::new(connect(ep.port));
    let reply = next_line(&mut r).expect("the server says why before closing");
    assert_eq!(reply["error"]["code"], -32004, "{reply}");
    let took = started.elapsed();
    assert!(took >= std::time::Duration::from_millis(250) && took < std::time::Duration::from_secs(3), "{took:?}");
    assert!(next_line(&mut r).is_none(), "then the connection is closed");
    // Dripping: a byte every 50 ms for as long as the server keeps the connection.
    let started = std::time::Instant::now();
    let s = connect(ep.port);
    let mut w = s.try_clone().unwrap();
    let drip = std::thread::spawn(move || {
        while w.write_all(b"x").is_ok() {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if started.elapsed() > std::time::Duration::from_secs(3) {
                break;
            }
        }
    });
    let mut r = BufReader::new(s);
    let reply = next_line(&mut r).expect("the server says why before closing");
    assert_eq!(reply["error"]["code"], -32004, "{reply}");
    let took = started.elapsed();
    assert!(took < std::time::Duration::from_secs(2), "the deadline is absolute: {took:?}");
    drop(r);
    drip.join().unwrap();
}

/// A client refused while the server still has its input unread gets the reason, not a
/// reset: closing on top of unread input sends a reset, which can discard the reply before
/// the client reads it (Linux does). Here a line far over the limit leaves most of itself
/// unread when the server refuses it.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_client_refused_with_input_unread_gets_the_reason() {
    use std::io::{BufReader, Write};
    let (_control, client) = crate::control::attach(&egui::Context::default());
    let ep = super::serve_with(client, TEST_LIMITS).unwrap();
    let s = connect(ep.port);
    let mut w = s.try_clone().unwrap();
    w.write_all(&vec![b'x'; 16 * super::MAX_AUTH_LINE]).unwrap();
    // Let the server refuse it (and, before the fix, close) before reading the reply.
    std::thread::sleep(std::time::Duration::from_millis(200));
    let mut r = BufReader::new(s);
    let reply = next_line(&mut r).expect("the server says why before closing");
    assert_eq!(reply["error"]["code"], -32600, "{reply}");
}

/// A refused connection gives back its place among those waiting to authenticate before it
/// lingers, so lingering does not keep real clients out.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_lingering_connection_holds_no_place_to_authenticate() {
    use std::io::{BufReader, Write};
    // A linger far longer than the checks below take, even on a loaded machine.
    let limits = super::Limits { linger: std::time::Duration::from_secs(5), ..TEST_LIMITS };
    let (_control, client) = crate::control::attach(&egui::Context::default());
    let ep = super::serve_with(client, limits).unwrap();
    let auth = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "auth", "params": { "token": ep.token } });
    // Refused with most of its line unread, and left open: it lingers for `linger`.
    let refused = connect(ep.port);
    refused.try_clone().unwrap().write_all(&vec![b'x'; 16 * super::MAX_AUTH_LINE]).unwrap();
    let mut r = BufReader::new(refused.try_clone().unwrap());
    assert_eq!(next_line(&mut r).expect("refused with a reason")["error"]["code"], -32600);
    // Well within the linger, both places are free for new connections.
    let started = std::time::Instant::now();
    let others: Vec<_> = (0..limits.max_unauthenticated).map(|_| connect(ep.port)).collect();
    for s in &others {
        // A refused one has already been closed, so its write may fail; its reply says why.
        let _ = writeln!(s.try_clone().unwrap(), "{auth}");
        let reply = next_line(&mut BufReader::new(s.try_clone().unwrap())).expect("a reply");
        assert_eq!(reply["result"]["ok"], true, "admitted while the refused one lingers: {reply}");
    }
    assert!(started.elapsed() < limits.linger, "checked while it lingered: {:?}", started.elapsed());
    drop(refused);
}

/// Only [`super::Limits::max_lingering`] refused connections linger at once: one more still
/// gets its reply, and is then closed at once instead of lingering.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn only_so_many_refused_connections_linger() {
    use std::io::{BufReader, Write};
    // A linger far longer than the checks below take, even on a loaded machine.
    let limits = super::Limits { linger: std::time::Duration::from_secs(5), max_lingering: 1, ..TEST_LIMITS };
    let (_control, client) = crate::control::attach(&egui::Context::default());
    let ep = super::serve_with(client, limits).unwrap();
    let bad = r#"{"jsonrpc": "2.0", "id": 1, "method": "page_count"}"#;
    // The first refused connection takes the only lingering place and keeps it.
    let first = connect(ep.port);
    writeln!(first.try_clone().unwrap(), "{bad}").unwrap();
    let mut r1 = BufReader::new(first.try_clone().unwrap());
    assert_eq!(next_line(&mut r1).expect("refused with a reason")["error"]["code"], -32001);
    // The second still gets its reason (it has nothing unread, so no reset)...
    let second = connect(ep.port);
    let mut w2 = second.try_clone().unwrap();
    writeln!(w2, "{bad}").unwrap();
    let mut r2 = BufReader::new(second.try_clone().unwrap());
    assert_eq!(next_line(&mut r2).expect("refused with a reason")["error"]["code"], -32001);
    // ...and is closed outright, not lingering: writing to it soon fails, where a lingering
    // connection would read and discard for the whole of `linger`.
    let started = std::time::Instant::now();
    let closed = loop {
        if w2.write_all(b"x").is_err() {
            break true;
        }
        if started.elapsed() > limits.linger {
            break false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(closed && started.elapsed() < limits.linger / 2, "it lingered: {:?}", started.elapsed());
    drop(first);
}

/// #1806: only so many connections may wait to authenticate at once. One more is refused
/// on the spot; once a waiting one authenticates or times out, its place is free again.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn only_so_many_connections_may_wait_to_authenticate() {
    use std::io::{BufReader, Write};
    let (_control, client) = crate::control::attach(&egui::Context::default());
    let ep = super::serve_with(client, TEST_LIMITS).unwrap();
    let auth = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "auth", "params": { "token": ep.token } });
    let first = connect(ep.port);
    let second = connect(ep.port);
    // The kernel accepts a connection before the listener admits it: give the listener a
    // moment to admit both, well within their deadline.
    std::thread::sleep(std::time::Duration::from_millis(100));
    // The third finds both places taken.
    let mut r3 = BufReader::new(connect(ep.port));
    let reply = next_line(&mut r3).expect("refused with a reason");
    assert_eq!(reply["error"]["code"], -32003, "{reply}");
    assert!(next_line(&mut r3).is_none(), "and closed");
    // The second authenticates: its place is free, so a new connection is admitted.
    let mut r2 = BufReader::new(second.try_clone().unwrap());
    let mut w2 = second.try_clone().unwrap();
    writeln!(w2, "{auth}").unwrap();
    assert_eq!(next_line(&mut r2).unwrap()["result"]["ok"], true);
    let fourth = connect(ep.port);
    let mut r4 = BufReader::new(fourth.try_clone().unwrap());
    let mut w4 = fourth.try_clone().unwrap();
    writeln!(w4, "{auth}").unwrap();
    assert_eq!(next_line(&mut r4).unwrap()["result"]["ok"], true, "admitted and authenticated");
    // The first was closed at the deadline (its place freed on that way out too): after it,
    // another connection is admitted even though the first never authenticated.
    drop(first);
    std::thread::sleep(TEST_LIMITS.auth_deadline + std::time::Duration::from_millis(100));
    let fifth = connect(ep.port);
    let mut r5 = BufReader::new(fifth.try_clone().unwrap());
    let mut w5 = fifth.try_clone().unwrap();
    writeln!(w5, "{auth}").unwrap();
    assert_eq!(next_line(&mut r5).unwrap()["result"]["ok"], true);
}

/// After `auth` the limit no longer applies: a long line is read as before (here it isn't
/// JSON, so it gets a parse error and the connection stays open).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn long_lines_after_auth_are_still_read() {
    use std::io::{BufRead, BufReader, Write};
    let (_control, client) = crate::control::attach(&egui::Context::default());
    let ep = super::serve(client).unwrap();
    let s = std::net::TcpStream::connect(("127.0.0.1", ep.port)).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    let mut w = s.try_clone().unwrap();
    let mut r = BufReader::new(s);
    let reply = |r: &mut BufReader<std::net::TcpStream>| {
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        serde_json::from_str::<serde_json::Value>(&line).unwrap()
    };
    let auth = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "auth", "params": { "token": ep.token } });
    writeln!(w, "{auth}").unwrap();
    assert_eq!(reply(&mut r)["result"]["ok"], true);
    writeln!(w, "{}", "x".repeat(4 * super::MAX_AUTH_LINE)).unwrap();
    assert_eq!(reply(&mut r)["error"]["code"], -32700, "a long line after auth is parsed, not refused");
    writeln!(w, "y").unwrap();
    assert_eq!(reply(&mut r)["error"]["code"], -32700, "and the connection is still open");
}
