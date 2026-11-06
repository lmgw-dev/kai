//! kai-browser-host's tests: the built binary between a test that plays Firefox (its stdio) and a
//! fake Kai socket in a temp folder (one binary, one module per subject).

mod lifecycle;
mod limits;
mod relay;

use std::io::{BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub const WAIT: Duration = Duration::from_secs(10);

/// A temp folder with the socket's path, kept short: a socket's path has at most 107 bytes
/// (`sun_path`), which a folder under a worktree's `target/` can come close to. In the session's
/// runtime folder (`$XDG_RUNTIME_DIR`), else the system's temp folder; it holds the socket only.
pub fn temp() -> (tempfile::TempDir, PathBuf) {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    let dir = tempfile::Builder::new()
        .prefix("kai-host-test-")
        .tempdir_in(base)
        .unwrap();
    let socket = dir.path().join("browser.sock");
    (dir, socket)
}

pub fn write_frame(w: &mut impl Write, body: &[u8]) {
    w.write_all(&(body.len() as u32).to_le_bytes()).unwrap();
    w.write_all(body).unwrap();
    w.flush().unwrap();
}

/// One frame's body; `None` at a clean end.
pub fn read_frame(r: &mut impl Read) -> Option<Vec<u8>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return None,
        Err(e) => panic!("reading a frame: {e}"),
    }
    let mut body = vec![0u8; u32::from_le_bytes(len) as usize];
    r.read_exact(&mut body).unwrap();
    Some(body)
}

/// The host as Firefox runs it: its stdin and stdout are the test's, its stderr collected.
pub struct Host {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: mpsc::Receiver<Vec<u8>>,
    stderr: Arc<Mutex<String>>,
}

impl Host {
    pub fn start(socket: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kai-browser-host"))
            .args(["/fake/dev.rootshell.kai.json", "kai-firefox@rootshell.dev"])
            .env("KAI_BROWSER_SOCKET", socket)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (tx, frames) = mpsc::channel();
        std::thread::spawn(move || {
            while let Some(f) = read_frame(&mut stdout) {
                if tx.send(f).is_err() {
                    break;
                }
            }
        });
        let mut err = child.stderr.take().unwrap();
        let stderr = Arc::new(Mutex::new(String::new()));
        {
            let stderr = stderr.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = err.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    stderr
                        .lock()
                        .unwrap()
                        .push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            });
        }
        let stdin = child.stdin.take();
        Self {
            child,
            stdin,
            frames,
            stderr,
        }
    }

    pub fn send_raw(&mut self, body: &[u8]) {
        write_frame(self.stdin.as_mut().unwrap(), body);
    }

    pub fn send(&mut self, v: Value) {
        self.send_raw(v.to_string().as_bytes());
    }

    /// Closes the host's stdin, as Firefox does when the port ends.
    pub fn close_stdin(&mut self) {
        self.stdin.take();
    }

    /// The host's stdin, to write from another thread (dropping it closes it).
    pub fn take_stdin(&mut self) -> ChildStdin {
        self.stdin.take().expect("stdin taken already")
    }

    pub fn recv_raw(&self) -> Vec<u8> {
        self.frames
            .recv_timeout(WAIT)
            .expect("no frame from the host")
    }

    pub fn recv(&self) -> Value {
        serde_json::from_slice(&self.recv_raw()).unwrap()
    }

    /// The next frame, which must be a `kai` frame; whether it says up.
    pub fn kai(&self) -> bool {
        let f = self.recv();
        assert_eq!(f["t"], "kai", "{f}");
        f["up"].as_bool().unwrap()
    }

    /// Nothing more comes for a short while.
    pub fn quiet(&self, d: Duration) {
        if let Ok(f) = self.frames.recv_timeout(d) {
            panic!("unexpected frame {}", String::from_utf8_lossy(&f));
        }
    }

    /// Waits for the host to end.
    pub fn exits(&mut self) -> ExitStatus {
        let end = Instant::now() + WAIT;
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                return s;
            }
            assert!(Instant::now() < end, "the host did not end");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn pid(&self) -> i32 {
        self.child.id() as i32
    }

    pub fn stderr(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    /// Waits until the host's log says `text`.
    pub fn logs(&self, text: &str) {
        let end = Instant::now() + WAIT;
        while !self.stderr().contains(text) {
            assert!(
                Instant::now() < end,
                "the log never said {text:?}: {}",
                self.stderr()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // Our own child, by its pid (Child::kill), only if it still runs.
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// Kai's side of the socket.
pub struct FakeKai {
    listener: UnixListener,
}

impl FakeKai {
    pub fn bind(socket: &Path) -> Self {
        let listener = UnixListener::bind(socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        Self { listener }
    }

    /// The host's connection.
    pub fn accept(&self) -> Conn {
        let end = Instant::now() + WAIT;
        loop {
            match self.listener.accept() {
                Ok((s, _)) => {
                    s.set_nonblocking(false).unwrap();
                    s.set_read_timeout(Some(WAIT)).unwrap();
                    return Conn {
                        r: BufReader::new(s.try_clone().unwrap()),
                        w: s,
                    };
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < end, "the host never connected");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("accept: {e}"),
            }
        }
    }
}

pub struct Conn {
    r: BufReader<UnixStream>,
    pub w: UnixStream,
}

impl Conn {
    pub fn send_raw(&mut self, body: &[u8]) {
        write_frame(&mut self.w, body);
    }

    pub fn send(&mut self, v: Value) {
        self.send_raw(v.to_string().as_bytes());
    }

    /// The next frame's body; `None` when the host closed its side.
    pub fn recv_raw(&mut self) -> Option<Vec<u8>> {
        read_frame(&mut self.r)
    }

    pub fn recv(&mut self) -> Value {
        serde_json::from_slice(&self.recv_raw().expect("the host closed")).unwrap()
    }
}

/// A host linked to a fake Kai, past the first `kai` frame and the answer to a `config` that sets
/// `retry_ms` to 50.
pub fn linked() -> (tempfile::TempDir, FakeKai, Conn, Host) {
    let (dir, socket) = temp();
    let kai = FakeKai::bind(&socket);
    let mut host = Host::start(&socket);
    let conn = kai.accept();
    assert!(host.kai(), "up once connected");
    host.send(json!({"t": "config", "retry_ms": 50}));
    assert!(host.kai(), "the answer to config");
    (dir, kai, conn, host)
}
