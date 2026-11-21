use super::{ControlClient, Value, json};

// ---- transport (native only) -----------------------------------------------------------------

/// Where a running app's control channel can be reached (written to the `--control` file).
#[cfg(not(target_arch = "wasm32"))]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
}

/// What a connection may do before it authenticates, so an unauthenticated local client cannot
/// hold a thread for ever or exhaust them (#1806).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
struct Limits {
    /// How long a connection has to authenticate, counted from its acceptance: an absolute
    /// deadline, so a client feeding one byte at a time cannot extend it.
    auth_deadline: std::time::Duration,
    /// How many connections may wait for authentication at once; further ones are refused.
    max_unauthenticated: usize,
    /// How long a write may block on a client that never reads its replies.
    write_timeout: std::time::Duration,
    /// How long a connection refused after a reply is kept half-open, discarding input, so the
    /// reply is not lost to a reset (see [`close_after_reply`]).
    linger: std::time::Duration,
    /// How many refused connections may linger at once. A lingering connection has given back
    /// its place among [`Self::max_unauthenticated`], so this is what bounds their threads;
    /// beyond it a refused connection is closed at once.
    max_lingering: usize,
}

#[cfg(not(target_arch = "wasm32"))]
const LIMITS: Limits = Limits {
    auth_deadline: std::time::Duration::from_secs(10),
    max_unauthenticated: 8,
    write_timeout: std::time::Duration::from_secs(30),
    linger: std::time::Duration::from_secs(1),
    max_lingering: 8,
};

/// One of a limited number of places, given back when dropped: a place among
/// [`Limits::max_unauthenticated`] (given back when the connection authenticates or is refused,
/// and on every way its thread can end), or among [`Limits::max_lingering`].
#[cfg(not(target_arch = "wasm32"))]
struct Slot(std::sync::Arc<std::sync::atomic::AtomicUsize>);

#[cfg(not(target_arch = "wasm32"))]
impl Slot {
    fn take(waiting: &std::sync::Arc<std::sync::atomic::AtomicUsize>, max: usize) -> Option<Self> {
        use std::sync::atomic::Ordering;
        let mut n = waiting.load(Ordering::Acquire);
        loop {
            if n >= max {
                return None;
            }
            match waiting.compare_exchange_weak(n, n.saturating_add(1), Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(Slot(waiting.clone())),
                Err(now) => n = now,
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// The reading half of a connection, with the authentication deadline: every read before it
/// gets the time left as its timeout, and none at all once it has passed.
#[cfg(not(target_arch = "wasm32"))]
struct Reading {
    stream: std::net::TcpStream,
    until: Option<std::time::Instant>,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::io::Read for Reading {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(until) = self.until {
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the authentication deadline passed"));
            }
            self.stream.set_read_timeout(Some(left))?;
        }
        self.stream.read(buf)
    }
}

/// Close a connection whose last reply has just been written without losing that reply.
///
/// Closing a socket with input still unread makes the kernel send a reset instead of a FIN, and
/// the reset can discard the reply before the client reads it (Linux does): a client still
/// sending when it is refused would never learn why. So send FIN first, then read and discard
/// whatever arrives until the client closes its side or `linger` passes, and only then close.
///
/// Only [`Limits::max_lingering`] connections linger at once: `place` is one of those places,
/// taken by [`refuse`] before the reply was written, and held for the drain only. Without one
/// the connection is closed at once after the FIN, accepting the risk of a reset rather than
/// holding another thread.
#[cfg(not(target_arch = "wasm32"))]
fn close_after_reply(reading: &mut Reading, place: Option<Slot>, linger: std::time::Duration) {
    use std::io::Read;
    let _ = reading.stream.shutdown(std::net::Shutdown::Write);
    let Some(_place) = place else { return };
    reading.until = Some(std::time::Instant::now() + linger);
    let mut sink = [0u8; 1024];
    // Each read is bounded by the time left, and none is made once `linger` has passed.
    while matches!(reading.read(&mut sink), Ok(n) if n > 0) {}
}

/// A connection is being refused before `auth`: give back its place among those waiting to
/// authenticate, so it does not keep a real client out while it lingers, and take a place to
/// linger in if one is free. Done before the reply is written, so a client that has read the
/// reply sees the places as they will be.
#[cfg(not(target_arch = "wasm32"))]
fn refuse(slot: &mut Option<Slot>, lingering: &std::sync::Arc<std::sync::atomic::AtomicUsize>, limits: Limits) -> Option<Slot> {
    drop(slot.take());
    Slot::take(lingering, limits.max_lingering)
}

/// Listen on `127.0.0.1` (random port) and forward authenticated requests to `client`.
///
/// Protocol: newline-delimited JSON-RPC 2.0. The first request on a connection must be
/// `{"method": "auth", "params": {"token": …}}`; anything else closes the connection, and so does
/// a line longer than [`MAX_AUTH_LINE`] before that, or [`LIMITS`] passing without it.
#[cfg(not(target_arch = "wasm32"))]
pub fn serve(client: ControlClient) -> std::io::Result<Endpoint> {
    serve_with(client, LIMITS)
}

/// How long the accepting thread waits after a failed accept. Accepting can keep failing, for
/// example while the process has no file descriptors left, and retrying at once would keep a
/// core busy until it recovers. The single-instance listener waits the same way.
#[cfg(not(target_arch = "wasm32"))]
const ACCEPT_RETRY: std::time::Duration = std::time::Duration::from_millis(100);

/// The connections `incoming` accepts, calling `wait` after each failed accept.
#[cfg(not(target_arch = "wasm32"))]
fn accepted<T>(incoming: impl Iterator<Item = std::io::Result<T>>, mut wait: impl FnMut()) -> impl Iterator<Item = T> {
    incoming.filter_map(move |stream| stream.map_err(|_| wait()).ok())
}

#[cfg(not(target_arch = "wasm32"))]
fn serve_with(client: ControlClient, limits: Limits) -> std::io::Result<Endpoint> {
    use std::io::{BufReader, Write};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let token = random_token()?;
    let expected = token.clone();
    let waiting = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let lingering = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    std::thread::Builder::new().name("pdfcraft-control".into()).spawn(move || {
        for stream in accepted(listener.incoming(), || std::thread::sleep(ACCEPT_RETRY)) {
            let _ = stream.set_write_timeout(Some(limits.write_timeout));
            // Admitted only while fewer than the limit wait to authenticate: refused on the
            // accepting thread, so a refused connection costs no thread at all.
            let Some(slot) = Slot::take(&waiting, limits.max_unauthenticated) else {
                let mut write = stream;
                let e = rpc_error(Value::Null, -32003, "too many connections waiting to authenticate; try again later");
                let _ = writeln!(write, "{e}").and_then(|_| write.flush());
                continue;
            };
            let client = client.clone();
            let expected = expected.clone();
            let lingering = lingering.clone();
            let _ = std::thread::Builder::new().name("pdfcraft-control-conn".into()).spawn(move || {
                // Held until `auth` succeeds; given back however this thread ends.
                let mut slot = Some(slot);
                let Ok(read) = stream.try_clone() else { return };
                let mut write = stream;
                let mut authed = false;
                let mut reader = BufReader::new(Reading { stream: read, until: Some(std::time::Instant::now() + limits.auth_deadline) });
                let mut buf = Vec::new();
                loop {
                    // Until `auth` succeeds, lines are capped at MAX_AUTH_LINE; afterwards they
                    // are read whole, as before.
                    let max = if authed { usize::MAX } else { MAX_AUTH_LINE };
                    match read_line_limited(&mut reader, &mut buf, max) {
                        Ok(LineRead::Line) => {}
                        // The deadline passed before `auth`: say so and close.
                        Err(e) if !authed && matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {
                            let secs = limits.auth_deadline.as_secs_f64();
                            let e = rpc_error(Value::Null, -32004, &format!("not authenticated within {secs} s"));
                            let place = refuse(&mut slot, &lingering, limits);
                            if writeln!(write, "{e}").and_then(|_| write.flush()).is_ok() {
                                close_after_reply(reader.get_mut(), place, limits.linger);
                            }
                            break;
                        }
                        // End of stream or a failed read ends the connection, as before.
                        Ok(LineRead::End) | Err(_) => break,
                        Ok(LineRead::TooLong) => {
                            let e = rpc_error(Value::Null, -32600, &format!("request line longer than {max} bytes before auth"));
                            let place = refuse(&mut slot, &lingering, limits);
                            if writeln!(write, "{e}").and_then(|_| write.flush()).is_ok() {
                                close_after_reply(reader.get_mut(), place, limits.linger);
                            }
                            break;
                        }
                    }
                    // Bytes that aren't UTF-8 end the connection, as `BufRead::lines` did.
                    let Ok(line) = std::str::from_utf8(&buf) else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let (reply, close) = match serde_json::from_str::<Value>(line) {
                        // Before `auth`, like any other first request, this closes the connection.
                        Err(e) => (rpc_error(Value::Null, -32700, &format!("parse error: {e}")), !authed),
                        Ok(msg) => {
                            let id = msg.get("id").cloned().unwrap_or(Value::Null);
                            let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
                            let params = msg.get("params").cloned().unwrap_or(Value::Null);
                            if !authed {
                                let ok =
                                    method == "auth" && params.get("token").and_then(Value::as_str).is_some_and(|t| constant_time_eq(t, &expected));
                                authed = ok;
                                if ok {
                                    // Authenticated: no deadline from here on, and its place is free.
                                    reader.get_mut().until = None;
                                    let _ = reader.get_mut().stream.set_read_timeout(None);
                                    drop(slot.take());
                                    (json!({ "jsonrpc": "2.0", "id": id, "result": { "ok": true } }), false)
                                } else {
                                    (rpc_error(id, -32001, "authenticate first: auth {token}"), true)
                                }
                            } else {
                                let rx = client.send(method, params);
                                match rx.recv_timeout(std::time::Duration::from_secs(30)) {
                                    Ok(Ok(v)) => (json!({ "jsonrpc": "2.0", "id": id, "result": v }), false),
                                    Ok(Err(e)) => (rpc_error(id, -32000, &e), false),
                                    Err(_) => (rpc_error(id, -32002, "the app did not answer within 30 s"), false),
                                }
                            }
                        }
                    };
                    // `close` is only ever set before `auth`.
                    let place = if close { refuse(&mut slot, &lingering, limits) } else { None };
                    if writeln!(write, "{reply}").and_then(|_| write.flush()).is_err() {
                        break;
                    }
                    if close {
                        close_after_reply(reader.get_mut(), place, limits.linger);
                        break;
                    }
                }
            });
        }
    })?;
    Ok(Endpoint { port, token })
}

/// Longest line read before a connection has authenticated. The only acceptable request then is
/// `auth`, about a hundred bytes; a longer line is refused without buffering the rest of it.
#[cfg(not(target_arch = "wasm32"))]
const MAX_AUTH_LINE: usize = 4 * 1024;

/// What [`read_line_limited`] found.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, PartialEq, Eq)]
enum LineRead {
    /// A line, now in the buffer (newline and a trailing `\r` stripped).
    Line,
    /// End of stream.
    End,
    /// The line is longer than the limit; at most limit + 1 bytes of it were read.
    TooLong,
}

/// Read one `\n`-terminated line of at most `max` bytes into `buf`.
#[cfg(not(target_arch = "wasm32"))]
fn read_line_limited(reader: &mut impl std::io::BufRead, buf: &mut Vec<u8>, max: usize) -> std::io::Result<LineRead> {
    use std::io::{BufRead, Read};
    buf.clear();
    // `take` bounds what is buffered for one line: never more than `max` + 1 bytes.
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    if reader.take(limit).read_until(b'\n', buf)? == 0 {
        return Ok(LineRead::End);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        return Ok(LineRead::Line);
    }
    // No newline: either the limit was passed, or this is a last line before end of stream.
    Ok(if buf.len() > max { LineRead::TooLong } else { LineRead::Line })
}

#[cfg(not(target_arch = "wasm32"))]
fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(not(target_arch = "wasm32"))]
fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 128 random bits from the OS, hex-encoded.
#[cfg(not(target_arch = "wasm32"))]
fn random_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod transport_tests;
