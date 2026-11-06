//! `kai-browser-host`: Kai for Firefox's native-messaging host (kai-firefox design §1, §2, §4.1,
//! §4.5). Firefox starts it when the extension's background calls
//! `connectNative("dev.rootshell.kai")` and ends it with the port: stdin closes (the host exits at
//! once), then SIGTERM after 3 s (the host keeps SIGTERM's default action, so it ends there and
//! then), then SIGKILL. It holds nothing: it relays frames between its stdio and Kai's socket,
//! `$XDG_RUNTIME_DIR/kai/browser.sock` (`$KAI_BROWSER_SOCKET` when that is an absolute path; without
//! a runtime folder, `$XDG_STATE_HOME/kai/browser.sock`, as Kai does).
//!
//! - **Frames**, on both sides: a 4-byte little-endian length and that many bytes of UTF-8 JSON,
//!   native messaging's own framing.
//! - **What it reads**: the type `t` of the extension's frames, nothing else. A `config` frame is
//!   the host's own (`retry_ms`: how often it tries Kai's socket while Kai is not there, default
//!   2000; a whole number of at least 1): it is answered with a `kai` frame saying whether Kai's
//!   socket is connected now. Every other frame goes to Kai unread, and Kai's frames go to the
//!   extension unread.
//! - **`kai` frames** say whether Kai's socket is connected: `{"t":"kai","up":true}` when it
//!   connects, `{"t":"kai","up":false}` when the first try fails and whenever it drops. While Kai
//!   is not there the host tries again every `retry_ms` (a `config` frame makes it try at once),
//!   and a frame the extension sends meanwhile is dropped and logged: the extension knows from
//!   `up:false`, and says hello again at the next `up:true` (a new connection is a new link).
//! - **Firefox takes at most 1 MiB in one message from the host**: 1 048 576 bytes of body, that
//!   size included (Firefox's pref `webextensions.native-messaging.max-input-message-bytes`; one
//!   byte more ends the port, and Firefox starts a new host). Kai splits longer frames into
//!   `chunk` frames; a frame from Kai that is still longer is not passed on: the host logs it and
//!   sends the extension
//!   `{"t":"error","code":"frame_too_large","size":<bytes>,"limit":1048576,"message":…}`.
//!   Toward Kai a frame may be as long as its length says (4 GiB), as Firefox allows; the
//!   extension splits its long ones into `chunk` frames too (Firefox writes one large message
//!   slowly), which the host relays unread like any frame.
//! - **Nothing waits behind a write**: stdin is read on the main thread, which only hands frames
//!   on; each connection to Kai has its writer thread and stdout has its own, so a write that
//!   blocks (Kai or Firefox not reading) never keeps the host from seeing stdin close, and it ends
//!   at once. What waits for a blocked write is held in memory, as much as was sent.
//! - **Pipes**: the host asks for the largest pipe buffer the system lets a user set
//!   (`/proc/sys/fs/pipe-max-size`) on its stdin and stdout, which makes Firefox's large messages
//!   several times faster, and logs the sizes it got.
//! - **Logs** go to stderr, which Firefox shows in its browser console. A connection Kai closes
//!   before the extension sent more than one frame (Kai refused the peer or its hello, and will
//!   again at every try) is logged the first time; the same again is counted, said at 2, 4, 8, …
//!   times in a row and when it stops, so a refused link does not fill the console every
//!   `retry_ms`.

use std::borrow::Cow;
use std::io::{self, BufReader, Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

/// The most Firefox takes in one message from the host: the body's length in bytes, this size
/// included (the default of Firefox's pref
/// `webextensions.native-messaging.max-input-message-bytes`; a longer message ends the port).
const TO_FIREFOX_MAX: u32 = 1024 * 1024;

/// How often Kai's socket is tried while it is not there, until a `config` frame says otherwise.
const RETRY_MS: u64 = 2000;

fn log(message: impl std::fmt::Display) {
    eprintln!("kai-browser-host: {message}");
}

fn main() -> ExitCode {
    sigterm_ends_the_host();
    let socket = match socket_path() {
        Ok(p) => p,
        Err(e) => {
            log(format!("cannot find Kai's socket: {e}"));
            return ExitCode::FAILURE;
        }
    };
    // Firefox passes the manifest's path and the extension's id.
    let extension = std::env::args().nth(2).unwrap_or_default();
    log(format!(
        "started for {}; Kai's socket: {}",
        if extension.is_empty() {
            "an unnamed caller"
        } else {
            &extension
        },
        socket.display()
    ));
    widen_pipes();
    let (to_firefox, frames) = mpsc::channel();
    std::thread::spawn(move || stdout_loop(&frames));
    let shared = Arc::new(Shared {
        to_firefox: Mutex::new(to_firefox),
        kai: Mutex::new(None),
        up: AtomicBool::new(false),
        retry_ms: AtomicU64::new(RETRY_MS),
        nudge: Mutex::new(false),
        nudged: Condvar::new(),
        sent: AtomicU64::new(0),
        early: AtomicU64::new(0),
    });
    {
        let shared = shared.clone();
        std::thread::spawn(move || connect_loop(&shared, &socket));
    }
    // Only reads, and hands frames on: nothing here waits for a write (module doc). Returning ends
    // the process, whatever another thread is blocked in.
    let mut stdin = io::stdin().lock();
    loop {
        match read_frame(&mut stdin) {
            Ok(Some(body)) => from_firefox(&shared, body),
            Ok(None) => {
                log("Firefox closed the port; ending");
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                log(format!("reading from Firefox failed: {e}; ending"));
                return ExitCode::FAILURE;
            }
        }
    }
}

/// The largest pipe buffer an unprivileged process may set (`/proc/sys/fs/pipe-max-size`), asked
/// for on stdin and stdout; best effort, the sizes logged.
fn widen_pipes() {
    let max = std::fs::read_to_string("/proc/sys/fs/pipe-max-size")
        .ok()
        .and_then(|s| s.trim().parse::<libc::c_int>().ok());
    let said: Vec<String> = [(0, "stdin"), (1, "stdout")]
        .into_iter()
        .map(|(fd, name)| {
            // SAFETY: F_GETPIPE_SZ and F_SETPIPE_SZ on the process's own stdio descriptors, with an
            // int argument; they change no memory of ours.
            let now = unsafe { libc::fcntl(fd, libc::F_GETPIPE_SZ) };
            if now < 0 {
                return format!("{name} is no pipe");
            }
            let Some(max) = max else {
                return format!(
                    "{name} {now} bytes (the system's largest pipe size cannot be read)"
                );
            };
            if now >= max {
                return format!("{name} {now} bytes");
            }
            // SAFETY: as above.
            match unsafe { libc::fcntl(fd, libc::F_SETPIPE_SZ, max) } {
                got if got >= 0 => format!("{name} {got} bytes"),
                _ => format!(
                    "{name} {now} bytes (asked for {max}: {})",
                    io::Error::last_os_error()
                ),
            }
        })
        .collect();
    log(format!("pipes: {}", said.join(", ")));
}

/// Writes the frames for Firefox to stdout in order. Its own handle on descriptor 1, unbuffered and
/// not std's stdout, so the host's end never waits for it. Firefox gone (a write fails) ends the
/// host.
fn stdout_loop(frames: &mpsc::Receiver<Vec<u8>>) {
    // SAFETY: descriptor 1 is the process's stdout, open for its whole life; nothing else in the
    // host writes to it, and ManuallyDrop never closes it.
    let mut out = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(1) });
    for body in frames {
        if let Err(e) = write_frame(&mut *out, &body) {
            log(format!("Firefox no longer takes frames ({e}); ending"));
            std::process::exit(0);
        }
    }
}

/// SIGTERM's default action, unblocked: the host ends at once when Firefox sends it, whatever
/// disposition or mask Firefox's process handed down. Called before any thread starts.
fn sigterm_ends_the_host() {
    // SAFETY: `signal` with SIG_DFL and `pthread_sigmask` on a set built on this frame; no handler
    // runs our code.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&raw mut set);
        libc::sigaddset(&raw mut set, libc::SIGTERM);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &raw const set, std::ptr::null_mut());
    }
}

/// `$KAI_BROWSER_SOCKET` (absolute), else `browser.sock` in Kai's runtime folder: the same rule as
/// Kai's (`paths::browser_socket`, `audio::runtime_dir_for`).
fn socket_path() -> Result<PathBuf, String> {
    let absolute = |var: &str| {
        std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    if let Some(p) = absolute("KAI_BROWSER_SOCKET") {
        return Ok(p);
    }
    if let Some(run) = absolute("XDG_RUNTIME_DIR") {
        return Ok(run.join("kai").join("browser.sock"));
    }
    let state = match absolute("XDG_STATE_HOME") {
        Some(s) => s,
        None => absolute("HOME")
            .ok_or("neither XDG_RUNTIME_DIR, XDG_STATE_HOME nor HOME is set")?
            .join(".local/state"),
    };
    Ok(state.join("kai").join("browser.sock"))
}

/// What the stdin side and the socket side share.
struct Shared {
    /// The frames for Firefox, to [`stdout_loop`]. The `kai` frames are sent under this lock with
    /// `up` read or changed, so they go out in the order the state changed.
    to_firefox: Mutex<mpsc::Sender<Vec<u8>>>,
    /// Kai's socket while it is connected: its writer thread's queue.
    kai: Mutex<Option<ToKai>>,
    /// Whether Kai's socket is connected, as last told to the extension.
    up: AtomicBool,
    retry_ms: AtomicU64,
    /// A `config` frame came: the connect loop's wait ends at once.
    nudge: Mutex<bool>,
    nudged: Condvar,
    /// The extension's frames written to Kai on this connection.
    sent: AtomicU64,
    /// Connections in a row that Kai closed before the extension sent more than one frame
    /// (module doc: logged as a count).
    early: AtomicU64,
}

/// A connection to Kai's writer thread: the extension's frames (with their type, for the log).
type ToKai = mpsc::Sender<(String, Vec<u8>)>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One frame to the extension, in its turn ([`stdout_loop`]).
fn to_firefox(shared: &Shared, body: Vec<u8>) {
    let _ = lock(&shared.to_firefox).send(body);
}

/// One frame's body; `None` when the stream ended cleanly between frames. A stream that ends inside
/// a frame is an error.
fn read_frame(r: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let Some(len) = read_len(r)? else {
        return Ok(None);
    };
    read_body(r, len).map(Some)
}

/// A frame's length; `None` at a clean end.
fn read_len(r: &mut impl Read) -> io::Result<Option<u32>> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < len.len() {
        match r.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the stream ended inside a frame's length",
                ));
            }
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(Some(u32::from_le_bytes(len)))
}

/// `len` bytes, read as they arrive (memory follows what was sent, not what the length claims).
fn read_body(r: &mut impl Read, len: u32) -> io::Result<Vec<u8>> {
    let mut body = Vec::new();
    r.take(u64::from(len)).read_to_end(&mut body)?;
    if body.len() as u64 != u64::from(len) {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "the stream ended inside a frame of {len} bytes ({} came)",
                body.len()
            ),
        ));
    }
    Ok(body)
}

/// Writes one frame and flushes it.
fn write_frame(w: &mut impl Write, body: &[u8]) -> io::Result<()> {
    let len = u32::try_from(body.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "a frame of {} bytes is longer than a frame's length can say (4 GiB)",
                body.len()
            ),
        )
    })?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(body)?;
    w.flush()
}

/// A frame's type, read without taking anything else out of it.
fn frame_type(body: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    struct Typed<'a> {
        #[serde(borrow, default)]
        t: Option<Cow<'a, str>>,
    }
    serde_json::from_slice::<Typed>(body)
        .ok()?
        .t
        .map(Cow::into_owned)
}

/// A frame of the extension's: `config` is the host's, the rest goes to Kai's writer thread.
fn from_firefox(shared: &Shared, body: Vec<u8>) {
    let t = frame_type(&body);
    if t.as_deref() == Some("config") {
        config(shared, &body);
        return;
    }
    let what = t.unwrap_or_else(|| "untyped".into());
    let kai = lock(&shared.kai);
    let Some(tx) = kai.as_ref() else {
        log(format!(
            "Kai is not running: the extension's {what} frame was dropped"
        ));
        return;
    };
    if let Err(mpsc::SendError((what, _))) = tx.send((what, body)) {
        log(format!(
            "the connection to Kai is ending: the extension's {what} frame was dropped"
        ));
    }
}

/// Writes the extension's frames to Kai in order until the connection ends (its queue closes or a
/// write fails, which ends the connection).
fn to_kai(shared: &Shared, mut stream: UnixStream, frames: mpsc::Receiver<(String, Vec<u8>)>) {
    for (what, body) in frames {
        if let Err(e) = write_frame(&mut stream, &body) {
            log(format!("a {what} frame for Kai was lost: {e}"));
            // The connect loop's read sees the end and reports the drop.
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return;
        }
        // The link got past its start: the connect loop did not log this connection when it came.
        if shared.sent.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
            let early = shared.early.swap(0, Ordering::SeqCst);
            if early > 0 {
                log(format!(
                    "connected to Kai, and the link is in use (Kai had closed the {} before it \
                     was used)",
                    connections(early)
                ));
            }
        }
    }
}

/// `n` connections, in words.
fn connections(n: u64) -> String {
    if n == 1 {
        "connection".into()
    } else {
        format!("connection {n} times in a row")
    }
}

/// The extension's `config`: `retry_ms`, answered with Kai's state now.
fn config(shared: &Shared, body: &[u8]) {
    let frame: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let out = lock(&shared.to_firefox);
    if let Some(retry) = frame.get("retry_ms") {
        match retry.as_u64().filter(|ms| *ms >= 1) {
            Some(ms) => shared.retry_ms.store(ms, Ordering::Relaxed),
            None => {
                let keep = shared.retry_ms.load(Ordering::Relaxed);
                let message = format!(
                    "retry_ms must be a whole number of milliseconds, at least 1 (it is \
                     {retry}); the host keeps {keep}"
                );
                log(&message);
                let error = json!({"t": "error", "code": "bad_config", "message": message});
                let _ = out.send(error.to_string().into_bytes());
            }
        }
    }
    // Under the queue's lock, so this answer and the connect loop's own `kai` frames go out in the
    // order the state changed.
    let up = shared.up.load(Ordering::SeqCst);
    let _ = out.send(kai_frame(up).into_bytes());
    drop(out);
    *lock(&shared.nudge) = true;
    shared.nudged.notify_all();
}

fn kai_frame(up: bool) -> String {
    json!({"t": "kai", "up": up}).to_string()
}

/// Kai's socket is up or down now: the state and the extension's `kai` frame change together.
fn set_up(shared: &Shared, up: bool) {
    let out = lock(&shared.to_firefox);
    shared.up.store(up, Ordering::SeqCst);
    let _ = out.send(kai_frame(up).into_bytes());
}

/// Connects to Kai's socket, relays what Kai sends until it closes, and tries again every
/// `retry_ms` while Kai is not there; runs until the host ends. A connection Kai closes before it
/// was used is logged once and then counted (module doc).
fn connect_loop(shared: &Shared, socket: &std::path::Path) {
    let mut said_down = false;
    loop {
        // A handle to read, one to write and one to shut the socket down with.
        let connected =
            UnixStream::connect(socket).and_then(|s| Ok((s.try_clone()?, s.try_clone()?, s)));
        match connected {
            Ok((reader, writer, ending)) => {
                let (tx, frames) = mpsc::channel();
                shared.sent.store(0, Ordering::SeqCst);
                *lock(&shared.kai) = Some(tx);
                *lock(&shared.nudge) = false;
                set_up(shared, true);
                if shared.early.load(Ordering::SeqCst) == 0 {
                    log(format!("connected to Kai at {}", socket.display()));
                }
                // The writer ends with the connection: its queue closed, or its blocked write
                // failing once the socket is shut down; the scope waits for it.
                let why = std::thread::scope(|scope| {
                    scope.spawn(move || to_kai(shared, writer, frames));
                    let why = from_kai(shared, reader);
                    lock(&shared.kai).take();
                    let _ = ending.shutdown(std::net::Shutdown::Both);
                    why
                });
                let sent = shared.sent.load(Ordering::SeqCst);
                set_up(shared, false);
                said_down = true;
                let retry = shared.retry_ms.load(Ordering::Relaxed);
                if sent > 1 {
                    log(format!(
                        "Kai's socket closed ({why}); trying again every {retry} ms"
                    ));
                } else {
                    let early = shared.early.fetch_add(1, Ordering::SeqCst) + 1;
                    if early == 1 {
                        log(format!(
                            "Kai closed the connection before the link was used ({why}; the \
                             extension had sent at most its hello); trying again every {retry} \
                             ms, and the same again is counted"
                        ));
                    } else if early.is_power_of_two() {
                        log(format!(
                            "Kai closed the connection again before the link was used ({why}); \
                             {early} times in a row"
                        ));
                    }
                }
            }
            Err(e) => {
                let early = shared.early.swap(0, Ordering::SeqCst);
                if early > 1 && !early.is_power_of_two() {
                    log(format!(
                        "Kai closed the connection before the link was used {early} times in a \
                         row"
                    ));
                }
                if !said_down || early > 0 {
                    if !said_down {
                        set_up(shared, false);
                    }
                    said_down = true;
                    log(format!(
                        "Kai is not running ({}: {e}); trying again every {} ms",
                        socket.display(),
                        shared.retry_ms.load(Ordering::Relaxed)
                    ));
                }
            }
        }
        wait_to_retry(shared);
    }
}

/// Waits `retry_ms`, or until a `config` frame nudges.
fn wait_to_retry(shared: &Shared) {
    let wait = Duration::from_millis(shared.retry_ms.load(Ordering::Relaxed));
    let nudge = lock(&shared.nudge);
    let mut nudge = shared
        .nudged
        .wait_timeout_while(nudge, wait, |n| !*n)
        .map(|(g, _)| g)
        .unwrap_or_else(|e| e.into_inner().0);
    *nudge = false;
}

/// Kai's frames to the extension until the socket ends; why it ended.
fn from_kai(shared: &Shared, stream: UnixStream) -> String {
    let mut r = BufReader::with_capacity(64 * 1024, stream);
    loop {
        let len = match read_len(&mut r) {
            Ok(Some(len)) => len,
            Ok(None) => return "Kai closed it".into(),
            Err(e) => return e.to_string(),
        };
        if len > TO_FIREFOX_MAX {
            // Read past it, so the next frame is read from its start.
            match io::copy(&mut (&mut r).take(u64::from(len)), &mut io::sink()) {
                Ok(n) if n == u64::from(len) => {}
                Ok(n) => return format!("it ended inside a frame of {len} bytes ({n} came)"),
                Err(e) => return e.to_string(),
            }
            let message = format!(
                "Kai sent a frame of {len} bytes; Firefox takes at most {TO_FIREFOX_MAX} in one \
                 message (its pref webextensions.native-messaging.max-input-message-bytes), so it \
                 was not passed on (Kai splits longer ones into chunk frames)"
            );
            log(&message);
            let error = json!({
                "t": "error",
                "code": "frame_too_large",
                "size": len,
                "limit": TO_FIREFOX_MAX,
                "message": message,
            });
            to_firefox(shared, error.to_string().into_bytes());
            continue;
        }
        match read_body(&mut r, len) {
            Ok(body) => to_firefox(shared, body),
            Err(e) => return e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_end_cleanly_only_between_frames() {
        let mut wire = Vec::new();
        for body in [
            &br#"{"t":"hello"}"#[..],
            b"",
            "{\"s\":\"\u{e4}\"}".as_bytes(),
        ] {
            write_frame(&mut wire, body).unwrap();
        }
        assert_eq!(&wire[..4], &13u32.to_le_bytes());
        let mut r = &wire[..];
        assert_eq!(read_frame(&mut r).unwrap().unwrap(), br#"{"t":"hello"}"#);
        assert_eq!(read_frame(&mut r).unwrap().unwrap(), b"");
        assert_eq!(
            read_frame(&mut r).unwrap().unwrap(),
            "{\"s\":\"\u{e4}\"}".as_bytes()
        );
        assert!(read_frame(&mut r).unwrap().is_none());
        let e = read_frame(&mut &[3u8, 0][..]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
        let e = read_frame(&mut &[3u8, 0, 0, 0, b'{'][..]).unwrap_err();
        assert!(e.to_string().contains("3 bytes (1 came)"), "{e}");
    }

    #[test]
    fn only_the_type_is_read() {
        assert_eq!(
            frame_type(br#"{"t":"config","retry_ms":5}"#).as_deref(),
            Some("config")
        );
        assert_eq!(
            frame_type(br#"{"id":1,"args":{"t":"x"},"t":"op_result"}"#).as_deref(),
            Some("op_result")
        );
        assert_eq!(frame_type(br#"{"id":1}"#), None);
        assert_eq!(frame_type(b"[1,2]"), None);
        assert_eq!(frame_type(b"not json"), None);
        assert_eq!(frame_type(br#"{"t":"a\"b"}"#).as_deref(), Some("a\"b"));
    }
}
