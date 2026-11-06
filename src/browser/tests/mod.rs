//! `browser.sock` over a real socket in a short temp folder (`local_socket::short_temp_dir`): the
//! peer check with stand-in cgroups, the handshake and its refusals, what is logged once, operations
//! with their results, errors and cancellation, the extension's events and questions, the
//! sidebar's requests through Kai's proxy to a scripted lmgw, and frames over Firefox's limit in
//! pieces.

mod accept;
mod act;
mod aloud;
mod chrome;
mod fake;
mod features;
mod handshake;
mod manifest;
mod ops;
mod owners;
mod peer;
mod provider;
mod read;
mod rules;
mod sidebar;
mod sidebar_edges;
mod sidebar_live;
mod speak;

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::BufReader;
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::watch;

use super::frame;
use super::links::Links;
use super::peer::Peer;
use super::server::{Server, serve};
use crate::host::registry::Changes;

pub const WAIT: Duration = Duration::from_secs(10);

/// The cgroup of a Firefox as Plasma starts it from a link (its "userapp" entry).
pub const FIREFOX_CGROUP: &str = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/\
    app-userapp\\x2dFirefox\\x2dAB12CD@0123456789abcdef0123456789abcdef.service\n";

/// A Firefox started from Konsole.
pub const TERMINAL_CGROUP: &str = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/\
    app-org.kde.konsole-61734.scope\n";

/// The fake Firefox's main process, the parent `in_firefox` reads.
pub const FIREFOX_PID: i32 = 4242;

pub fn in_firefox(pid: i32) -> Option<Peer> {
    Some(Peer {
        pid,
        cgroup: FIREFOX_CGROUP.into(),
        exe: Some("/usr/libexec/kai/kai-browser-host".into()),
        parent: Some(FIREFOX_PID),
    })
}

pub fn in_terminal(pid: i32) -> Option<Peer> {
    Some(Peer {
        pid,
        cgroup: TERMINAL_CGROUP.into(),
        exe: Some("/home/owner/kai/target/debug/kai-browser-host".into()),
        parent: None,
    })
}

/// A served `browser.sock` in a temp folder, and what the host's registry would see.
pub struct Rig {
    pub dir: tempfile::TempDir,
    pub socket: PathBuf,
    pub links: Links,
    /// The host registry's change signal ([`Changes`]).
    pub changed: watch::Receiver<u64>,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Rig {
    pub async fn start(peer_of: fn(i32) -> Option<Peer>) -> Self {
        Self::with(peer_of, None).await
    }

    /// With the configuration file `config` (its `browser.peer_scopes` read at each connection).
    pub async fn with(peer_of: fn(i32) -> Option<Peer>, config: Option<&str>) -> Self {
        Self::build(peer_of, config, None, None).await
    }

    /// A Firefox's link whose sidebar requests go to `proxy`.
    pub async fn with_sidebar(proxy: super::sidebar::Proxy) -> Self {
        Self::build(in_firefox, None, Some(proxy), None).await
    }

    /// A Firefox's link whose sidebar asks about sites under `sites`.
    pub async fn with_sites(sites: super::sidebar::site::Rules) -> Self {
        Self::build(in_firefox, None, None, Some(sites)).await
    }

    async fn build(
        peer_of: fn(i32) -> Option<Peer>,
        config: Option<&str>,
        sidebar: Option<super::sidebar::Proxy>,
        sites: Option<super::sidebar::site::Rules>,
    ) -> Self {
        let dir = crate::local_socket::short_temp_dir();
        let socket = dir.path().join("run/kai/browser.sock");
        let config_file = config.map(|text| {
            let f = dir.path().join("config.toml");
            std::fs::write(&f, text).unwrap();
            f
        });
        let (changes, changed) = Changes::new();
        let links = Links::new(changes);
        let mut server = tokio::spawn(serve(
            socket.clone(),
            Server {
                links: links.clone(),
                config_file,
                scopes: super::settings::BrowserConfig::default().peer_scopes,
                peer_of,
                sidebar,
                sites,
            },
        ));
        let end = std::time::Instant::now() + WAIT;
        while !socket.exists() {
            if server.is_finished() {
                let ended = (&mut server).await;
                panic!(
                    "browser.sock was not served at {}: {ended:?}",
                    socket.display()
                );
            }
            assert!(std::time::Instant::now() < end, "no socket after {WAIT:?}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Self {
            dir,
            socket,
            links,
            changed,
            server,
        }
    }

    /// Connects and says hello with `ops`; the welcome is checked.
    pub async fn link(&self, ops: &[&str]) -> Client {
        let mut c = Client::connect(&self.socket).await;
        c.send(hello(1, ops)).await;
        let welcome = c.recv().await.expect("a welcome");
        assert_eq!(
            welcome,
            json!({"t": "welcome", "kai_version": crate::VERSION, "protocol": 1,
                   "takes": crate::browser::server::TAKES})
        );
        c
    }

    /// Waits until the links' count is `n`.
    pub async fn links_are(&self, n: usize) {
        let end = std::time::Instant::now() + WAIT;
        while self.links.list().len() != n {
            assert!(std::time::Instant::now() < end, "{n} links never came");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.server.abort();
    }
}

pub fn hello(protocol: u32, ops: &[&str]) -> Value {
    json!({
        "t": "hello",
        "ext_version": "0.1.0",
        "protocol": protocol,
        "ops": ops,
        "firefox": "159.0",
        "profile": "c0ffee",
    })
}

/// The extension's side as `kai-browser-host` relays it.
pub struct Client {
    rd: BufReader<OwnedReadHalf>,
    wr: OwnedWriteHalf,
}

impl Client {
    pub async fn connect(socket: &std::path::Path) -> Self {
        let (rd, wr) = UnixStream::connect(socket).await.unwrap().into_split();
        Self {
            rd: BufReader::new(rd),
            wr,
        }
    }

    pub async fn send(&mut self, v: Value) {
        frame::write(&mut self.wr, v.to_string().as_bytes())
            .await
            .unwrap();
    }

    /// The next frame's body; `None` when Kai closed the socket.
    pub async fn recv_raw(&mut self) -> Option<Vec<u8>> {
        tokio::time::timeout(WAIT, frame::read(&mut self.rd))
            .await
            .expect("no frame from Kai")
            .unwrap()
    }

    pub async fn recv(&mut self) -> Option<Value> {
        let body = self.recv_raw().await?;
        Some(serde_json::from_slice(&body).unwrap())
    }
}
