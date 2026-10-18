//! The routines without the session: routine folders and Kai's configuration in temp folders under
//! target/, the host built over the routines and a test provider whose tools record their caller,
//! scripts started through the launch tests' fake user manager on a private bus, the CLI on a
//! socket under target/ (never the running Kai's). The real scope and cgroup are `live_tests`',
//! ignored by default.

mod cli;
mod cli_stream;
mod list;
mod live_tests;
mod model;
mod runs;
mod switches;
mod tray;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use kai_desktop::contract::{Kind, Param, ToolSpec};
use tokio::sync::{mpsc, watch};

use super::{Inner, Routines, Runtime, State};
use crate::config::Config;
use crate::host::confirm::{ConfirmRequest, Confirmer};
use crate::host::link::LinkSettings;
use crate::host::local::Local;
use crate::host::policy::Policy;
use crate::host::registry::{Changes, Entry, Provider, Reply, SessionBus};
use crate::host::settings::PolicyConfig;
use crate::host::{HostLink, ToolHost};
use crate::lmgw_link::Credentials;
use crate::lmgw_link::test_pki::creds as test_creds;

pub const T: Duration = Duration::from_secs(10);

const TEXT: Param = Param::new("text", Kind::Text);
pub const ECHO: ToolSpec = ToolSpec {
    name: "echo",
    description: "Answer with the text.",
    params: &[TEXT],
    ..ToolSpec::READ
};
pub const POKE: ToolSpec = ToolSpec {
    name: "poke",
    description: "Change a thing.",
    params: &[TEXT],
    ..ToolSpec::CHANGE
};

/// What the test tools ran: `<tool> by <caller>: <text>`.
#[derive(Clone, Default)]
pub struct Ran(Arc<Mutex<Vec<String>>>);

impl Ran {
    pub fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

pub struct TestTools(pub Ran);

impl Provider for TestTools {
    fn entries(&self) -> Vec<Entry> {
        [&ECHO, &POKE]
            .into_iter()
            .map(|spec| {
                let ran = self.0.clone();
                Entry {
                    area: "test",
                    spec,
                    handler: Arc::new(move |call| {
                        let ran = ran.clone();
                        Box::pin(async move {
                            let text = call.args.text("text").unwrap_or_default().to_owned();
                            ran.0.lock().unwrap().push(format!(
                                "{} by {}: {text}",
                                call.spec.name,
                                call.caller.describe()
                            ));
                            if text == "fail" {
                                return Reply::error("it failed");
                            }
                            Reply::text(format!("{}: {text}", call.spec.name))
                        })
                    }),
                    prepare: None,
                }
            })
            .collect()
    }
}

/// The fake confirmer: each question goes to the test.
pub struct Asks(pub mpsc::UnboundedSender<ConfirmRequest>);
impl Confirmer for Asks {
    fn ask(&self, req: ConfirmRequest) {
        let _ = self.0.send(req);
    }
}

/// A temp home for the routines: `routines/` and `config/kai/config.toml`.
pub struct Home {
    pub dir: tempfile::TempDir,
}

impl Home {
    pub fn new() -> Self {
        let dir = crate::desktop::tests::bus::temp_dir();
        std::fs::create_dir_all(dir.path().join("routines")).unwrap();
        std::fs::create_dir_all(dir.path().join("config/kai")).unwrap();
        std::fs::write(dir.path().join("config/kai/config.toml"), "").unwrap();
        Self { dir }
    }

    pub fn routines(&self) -> PathBuf {
        self.dir.path().join("routines")
    }

    pub fn config_file(&self) -> PathBuf {
        self.dir.path().join("config/kai/config.toml")
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.path().join("run/cli.sock")
    }

    /// Writes a routine folder: its `routine.toml`, and `run.sh` when `script` is given.
    pub fn routine(&self, name: &str, toml: &str, script: Option<&str>) {
        let d = self.routines().join(name);
        std::fs::create_dir_all(&d).unwrap();
        if let Some(body) = script {
            crate::desktop::fdo::launch::tests::script(&d.join("run.sh"), body);
        }
        crate::desktop::fdo::launch::tests::write(&d.join(super::def::FILE), toml);
    }

    /// Switches `name` on in the configuration file, as the routine operation does after the
    /// owner saw it.
    pub fn switch_on(&self, name: &str) {
        crate::settings::apply(
            &self.config_file(),
            crate::settings::Request::Routine {
                name: name.into(),
                enabled: true,
            },
        )
        .unwrap();
    }
}

/// The host over the test tools and the routines of `home`, the confirmations to the test.
pub struct Rig {
    pub home: Home,
    pub host: HostLink,
    pub local: Local,
    pub inner: Arc<Inner>,
    pub ran: Ran,
    pub asks: mpsc::UnboundedReceiver<ConfirmRequest>,
    pub policy: Arc<RwLock<Policy>>,
    pub creds: watch::Sender<Option<Credentials>>,
    /// The routine scopes the rig's runs mark (`script::LiveScopes`), for the CLI's server.
    pub scopes: super::LiveScopes,
}

pub fn settings() -> LinkSettings {
    LinkSettings {
        retry_max: Duration::from_secs(1),
        timeout: Duration::from_secs(5),
        keepalive: Duration::from_secs(15),
        fallback: crate::host::limits::LinkLimits::fallback(64, 16),
    }
}

/// The routines' state of `home` as a provider over it would read it, without the provider.
pub fn inner(
    home: &Home,
    changes: Changes,
    policy: Arc<RwLock<Policy>>,
    asks: Arc<dyn Confirmer>,
    creds: watch::Receiver<Option<Credentials>>,
) -> Inner {
    Inner {
        dir: Some(home.routines()),
        state: RwLock::new(State::default()),
        changes,
        publish: false,
        runtime: OnceLock::new(),
        runs: Default::default(),
        policy,
        confirmer: asks,
        creds,
        config: Arc::new(Config::default()),
        drafting: Default::default(),
    }
}

/// `policy` with what the routines `names` ask granted, as the switch-on dialog's box writes it
/// (K28): unasked for trusted callers (a read-only routine's ask) and any caller level.
pub fn granted(mut policy: PolicyConfig, names: &[&str]) -> PolicyConfig {
    for n in names {
        policy
            .tools
            .insert((*n).into(), crate::host::settings::ToolState::AllowTrusted);
        policy
            .levels
            .insert((*n).into(), crate::host::settings::LevelSetting::Any);
    }
    policy
}

pub fn rig_with(home: Home, policy: PolicyConfig) -> (Rig, watch::Receiver<u64>) {
    let ran = Ran::default();
    let (asks_tx, asks) = mpsc::unbounded_channel();
    let asks_tx: Arc<dyn Confirmer> = Arc::new(Asks(asks_tx));
    let (changes, changed) = Changes::new();
    let policy = Arc::new(RwLock::new(Policy::new(&policy)));
    let (creds_tx, creds) = watch::channel(None);
    let routines = Routines::new(inner(
        &home,
        changes,
        policy.clone(),
        asks_tx.clone(),
        creds.clone(),
    ));
    let inner = routines.inner.clone();
    let host = HostLink::new(
        settings(),
        policy.clone(),
        vec![Box::new(TestTools(ran.clone())), Box::new(routines)],
        changed.clone(),
        asks_tx,
        watch::channel(None).0,
        crate::screen_lock::ScreenLock::default(),
        crate::jobs::Jobs::default(),
    );
    let local = host.local();
    (
        Rig {
            home,
            host,
            local,
            inner,
            ran,
            asks,
            policy,
            creds: creds_tx,
            scopes: super::LiveScopes::default(),
        },
        changed,
    )
}

impl Rig {
    /// The runtime, with scripts started through `manager` (`None`: a manager never asked).
    pub fn run_with(&self, manager: Option<crate::desktop::fdo::launch::spawn::UserManager>) {
        self.run_with_kai(manager, None);
    }

    /// [`Rig::run_with`], with `kai` for the scripts in `exe_dir`.
    pub fn run_with_kai(
        &self,
        manager: Option<crate::desktop::fdo::launch::spawn::UserManager>,
        exe_dir: Option<PathBuf>,
    ) {
        self.run_with_settings(manager, exe_dir, super::Settings::File);
    }

    /// [`Rig::run_with_kai`] writing the configuration through `settings`.
    pub fn run_with_settings(
        &self,
        manager: Option<crate::desktop::fdo::launch::spawn::UserManager>,
        exe_dir: Option<PathBuf>,
        settings: super::Settings,
    ) {
        let manager = manager.unwrap_or(crate::desktop::fdo::launch::spawn::UserManager {
            bus: SessionBus::lazy(Duration::from_secs(1)),
            timeout: Duration::from_secs(1),
        });
        let rt = Runtime {
            local: self.local.clone(),
            script: super::ScriptEnv {
                manager,
                own_env: crate::desktop::fdo::launch::tests::rig_own_env,
                socket: self.home.socket(),
                exe_dir,
                scopes: self.scopes.clone(),
            },
            config_file: self.home.config_file(),
            settings,
        };
        assert!(self.inner.runtime.set(rt).is_ok());
        self.inner.rescan(Some(&self.home.config_file()));
    }

    pub async fn ask(&mut self) -> ConfirmRequest {
        tokio::time::timeout(T, self.asks.recv())
            .await
            .expect("no confirmation was asked")
            .unwrap()
    }

    pub fn names(&self) -> Vec<String> {
        self.local
            .entries()
            .iter()
            .map(|e| e.spec.name.to_owned())
            .collect()
    }

    /// Kai's credentials for a fake lmgw at `url`.
    pub fn link(&mut self, url: &str) {
        self.creds.send_replace(Some(test_creds(url)));
    }

    #[allow(dead_code)]
    pub fn start(&mut self, url: &str) -> mpsc::UnboundedReceiver<crate::host::HostEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.host.start(test_creds(url), tx);
        rx
    }
}

/// The text blocks of a reply, joined.
pub fn text(r: &Reply) -> String {
    r.blocks
        .iter()
        .filter_map(|b| match b {
            crate::host::registry::Block::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
