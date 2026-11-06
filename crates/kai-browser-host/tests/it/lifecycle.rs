//! Kai coming and going (and closing a link before it was used, again and again, or inside a
//! frame), a `config` that cuts a long wait short, and Firefox ending the host.

use std::os::unix::process::ExitStatusExt;

use serde_json::json;

use super::*;

#[test]
fn kai_down_is_said_and_retried_until_it_is_up() {
    let (_dir, socket) = temp();
    let mut host = Host::start(&socket);
    assert!(!host.kai(), "Kai is not running");
    host.logs("Kai is not running");
    host.send(json!({"t": "config", "retry_ms": 50}));
    assert!(!host.kai());
    // Said once, not at every try.
    host.quiet(Duration::from_millis(300));
    // A frame while Kai is down is dropped, and logged.
    host.send(json!({"t": "event", "kind": "lost"}));
    host.logs("the extension's event frame was dropped");

    let kai = FakeKai::bind(&socket);
    let mut conn = kai.accept();
    assert!(host.kai(), "up once Kai is there");
    host.send(json!({"t": "hello"}));
    assert_eq!(
        conn.recv(),
        json!({"t": "hello"}),
        "the dropped frame never came"
    );

    // Kai restarts: down, then up again on a new connection.
    drop(conn);
    assert!(!host.kai());
    let mut conn = kai.accept();
    assert!(host.kai());
    host.send(json!({"t": "hello", "again": true}));
    assert_eq!(conn.recv(), json!({"t": "hello", "again": true}));

    // Kai gone for good (its socket removed): down, and the host keeps trying.
    drop(kai);
    drop(conn);
    std::fs::remove_file(&socket).unwrap();
    assert!(!host.kai());
    host.quiet(Duration::from_millis(300));
    let kai = FakeKai::bind(&socket);
    let _conn = kai.accept();
    assert!(host.kai());
}

/// With `retry_ms` at a minute, a `config` frame still makes the host try Kai's socket at once.
#[test]
fn a_config_cuts_a_long_wait_short() {
    let (_dir, socket) = temp();
    let mut host = Host::start(&socket);
    assert!(!host.kai());
    host.send(json!({"t": "config", "retry_ms": 60_000}));
    assert!(!host.kai(), "the answer; its own nudge's try fails too");
    // That try is behind us; the next is a minute away.
    host.quiet(Duration::from_millis(500));
    let kai = FakeKai::bind(&socket);
    host.quiet(Duration::from_millis(300));
    host.send(json!({"t": "config"}));
    assert!(!host.kai(), "the answer: not connected yet");
    // Within the tests' wait (10 s), far less than the minute.
    let _conn = kai.accept();
    assert!(host.kai());
}

/// Kai refusing the link (closing it before the extension sent more than its hello) every time the
/// host tries: the first close is logged, the same again only as a count at 2, 4, 8 … times, and
/// the link that is used at last says how many came before.
#[test]
fn a_link_kai_keeps_closing_at_its_start_is_logged_as_a_count() {
    let (_dir, socket) = temp();
    let kai = FakeKai::bind(&socket);
    let mut host = Host::start(&socket);
    let conn = kai.accept();
    assert!(host.kai());
    host.send(json!({"t": "config", "retry_ms": 20}));
    assert!(host.kai());
    // Refused at once (the peer check), then seven times at the hello.
    drop(conn);
    assert!(!host.kai());
    for _ in 1..8 {
        let mut conn = kai.accept();
        assert!(host.kai());
        host.send(json!({"t": "hello"}));
        assert_eq!(conn.recv(), json!({"t": "hello"}));
        drop(conn);
        assert!(!host.kai());
    }
    host.logs("8 times in a row");
    let log = host.stderr();
    assert_eq!(log.matches("connected to Kai").count(), 1, "{log}");
    assert_eq!(
        log.matches("before the link was used").count(),
        4,
        "the first, 2, 4 and 8 times: {log}"
    );

    let mut conn = kai.accept();
    assert!(host.kai());
    host.send(json!({"t": "hello"}));
    host.send(json!({"t": "event", "kind": "loaded"}));
    assert_eq!(conn.recv()["t"], "hello");
    assert_eq!(conn.recv()["t"], "event");
    host.logs(
        "connected to Kai, and the link is in use (Kai had closed the connection 8 times in a \
         row before it was used)",
    );
    // Its end is logged as a link's.
    drop(conn);
    assert!(!host.kai());
    host.logs("Kai's socket closed (Kai closed it); trying again every 20 ms");
}

/// Kai's stream ending inside a frame (its body, or its length) ends the connection with that
/// said, and the host tries again.
#[test]
fn kai_ending_inside_a_frame_closes_the_connection() {
    let (_dir, kai, mut conn, mut host) = linked();
    host.send(json!({"t": "hello"}));
    host.send(json!({"t": "event", "kind": "loaded"}));
    conn.recv();
    conn.recv();
    conn.w
        .write_all(&[10, 0, 0, 0, b'{', b'"', b't', b'"'])
        .unwrap();
    drop(conn);
    assert!(!host.kai());
    host.logs("Kai's socket closed (the stream ended inside a frame of 10 bytes (4 came))");

    let mut conn = kai.accept();
    assert!(host.kai());
    conn.w.write_all(&[10, 0]).unwrap();
    drop(conn);
    assert!(!host.kai());
    host.logs("the stream ended inside a frame's length");
}

#[test]
fn the_host_ends_when_firefox_closes_stdin() {
    let (_dir, _kai, mut conn, mut host) = linked();
    host.close_stdin();
    let status = host.exits();
    assert!(status.success(), "{status:?}");
    assert_eq!(conn.recv_raw(), None, "Kai sees the link end");
    assert!(host.stderr().contains("Firefox closed the port"));
}

#[test]
fn the_host_ends_on_sigterm() {
    let (_dir, _kai, mut conn, mut host) = linked();
    // SAFETY: kill(2) on our own child's pid, which is not reaped until `exits`.
    assert_eq!(unsafe { libc::kill(host.pid(), libc::SIGTERM) }, 0);
    let status = host.exits();
    assert_eq!(status.signal(), Some(libc::SIGTERM), "{status:?}");
    assert_eq!(conn.recv_raw(), None);
}

/// Firefox ends the host by closing its stdin, and sends SIGTERM only 3 s later (a host that
/// misses the close delays Firefox's quit by that, measured in Firefox 159): the host ends at once
/// even while its write to Kai is blocked, Kai not reading and the socket's buffer full.
#[test]
fn the_host_ends_on_stdin_close_while_its_write_to_kai_is_blocked() {
    let (_dir, _kai, conn, mut host) = linked();
    let mut stdin = host.take_stdin();
    let (written, done) = mpsc::channel();
    std::thread::spawn(move || {
        // 8 MiB toward Kai, far more than a socket's buffer: the host's write to Kai blocks.
        let mut frame = br#"{"t":"op_result","id":1,"ok":""#.to_vec();
        frame.resize(1024 * 1024 - 2, b'a');
        frame.extend_from_slice(br#""}"#);
        for _ in 0..8 {
            write_frame(&mut stdin, &frame);
        }
        let _ = written.send(stdin);
    });
    let stdin = done
        .recv_timeout(WAIT)
        .expect("the host stopped reading its stdin behind its blocked write to Kai");
    let closed = Instant::now();
    drop(stdin);
    let status = host.exits();
    assert!(
        closed.elapsed() < Duration::from_secs(2),
        "the host took {:?} to end, Firefox's SIGTERM comes at 3 s",
        closed.elapsed()
    );
    assert!(status.success(), "{status:?}");
    host.logs("Firefox closed the port");
    drop(conn);
}

/// The host asks for large pipes on its stdio, and says what it got.
#[test]
fn the_host_says_its_pipe_sizes() {
    let (_dir, socket) = temp();
    let host = Host::start(&socket);
    host.logs("pipes: stdin ");
    let log = host.stderr();
    let line = log.lines().find(|l| l.contains("pipes:")).unwrap();
    assert!(line.contains(", stdout "), "{line}");
    assert!(line.contains(" bytes"), "{line}");
}

/// Also while Kai is not there (the connect loop waiting), and with the stdin read blocking.
#[test]
fn the_host_ends_on_sigterm_while_kai_is_down() {
    let (_dir, socket) = temp();
    let mut host = Host::start(&socket);
    assert!(!host.kai());
    // SAFETY: as above.
    assert_eq!(unsafe { libc::kill(host.pid(), libc::SIGTERM) }, 0);
    assert_eq!(host.exits().signal(), Some(libc::SIGTERM));
}
