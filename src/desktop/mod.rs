//! Kai's desktop module (K23, WP14): the providers behind the capability traits
//! (`kai_desktop::caps`), the probe that picks one per capability, and the tools of
//! `contract::windows` on top.
//!
//! - **The probe** runs once, the first time the host lists its tools: every provider's probe (in
//!   priority order, [`plasma`] then [`fdo`]), then the choice per capability with `[desktop]`'s
//!   overrides ([`probe`]). Outside a runtime (the host is built in `main` before the runtime runs
//!   it, and `kai --tool-list`) it runs there and then, on a runtime of its own, each D-Bus call
//!   bounded by `dbus.timeout_s`, so the very first list already holds the desktop's tools (K22: a
//!   changed list costs a full prefill); inside one it runs in a task and signals the change.
//! - **The capability table** goes to the log (one line per capability: provider and how, or why
//!   none) and to the tray (one line, [`tray_lines`]).
//! - **The tools** a desktop cannot support are not listed (K23): the window tools need Windows,
//!   `desktop_context` any of its parts, the session and media tools (WP16b) their capability's
//!   service and, for do not disturb, brightness and Night Light, the provider's feature, WP15's
//!   seeing and reading tools ([`read`]) theirs. A missing grant is the tool's error (§11.1).
//! - **K22's resolution** of `window`, `desktop` and `output` is [`resolve`].

pub mod fdo;
#[cfg(test)]
pub(crate) mod live_tests;
pub mod plasma;
pub mod probe;
pub mod read;
pub mod resolve;
pub mod settings;
#[cfg(test)]
pub(crate) mod tests;
pub mod tools;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::Duration;

use kai_desktop::caps::{Capability, Failure};
use kai_desktop::contract::windows::{AREA, needs_windows};
use kai_desktop::contract::{input, launch, session};
use tokio::sync::watch;
use zbus::zvariant::{DynamicType, OwnedValue};

use crate::host::registry::{Changes, Deps, Entry, Provider, SessionBus};
use probe::{Chosen, Service};
use settings::DesktopConfig;
use tools::Services;

/// The capability table's tray line (empty until the probe ran). One per process: the tray reads
/// it beside the core's model.
static TABLE: LazyLock<watch::Sender<Vec<String>>> = LazyLock::new(|| watch::channel(Vec::new()).0);

/// The capability table's lines for the tray's tooltip.
pub fn tray_lines() -> watch::Receiver<Vec<String>> {
    TABLE.subscribe()
}

/// The system bus for a provider: made on first use, in the runtime of that use, bounded by
/// `dbus.timeout_s`; `address` names another bus (the tests' private one).
#[derive(Clone)]
pub struct SystemBus {
    cell: Arc<tokio::sync::OnceCell<zbus::Connection>>,
    address: Option<String>,
    timeout: Duration,
}

impl SystemBus {
    pub fn new(address: Option<String>, timeout: Duration) -> Self {
        Self {
            cell: Arc::new(tokio::sync::OnceCell::new()),
            address,
            timeout,
        }
    }

    /// A new connection, not kept (the probe's).
    pub async fn connect(&self) -> Result<zbus::Connection, String> {
        let build = async {
            match &self.address {
                Some(a) => {
                    zbus::connection::Builder::address(a.as_str())?
                        .build()
                        .await
                }
                None => zbus::Connection::system().await,
            }
        };
        match tokio::time::timeout(self.timeout, build).await {
            Ok(Ok(bus)) => Ok(bus),
            Ok(Err(e)) => Err(format!("cannot reach the system bus: {e}")),
            Err(_) => Err(format!(
                "the system bus did not answer within {} s (dbus.timeout_s)",
                self.timeout.as_secs()
            )),
        }
    }

    pub async fn get(&self) -> Result<zbus::Connection, String> {
        self.cell.get_or_try_init(|| self.connect()).await.cloned()
    }
}

/// What the providers probe and use.
#[derive(Clone)]
pub struct Env {
    /// `XDG_CURRENT_DESKTOP` as the session set it.
    pub current_desktop: String,
    pub session: Option<SessionBus>,
    pub system: Option<SystemBus>,
    /// A Wayland display to try the window management protocol on.
    pub wayland: bool,
    /// `dbus.timeout_s`: every D-Bus call's bound, and a KWin script's answer's.
    pub timeout: Duration,
    /// Kai's runtime directory (`$XDG_RUNTIME_DIR/kai`): the KWin scripts' files go below it.
    pub runtime_dir: PathBuf,
    /// The programs some providers drive (WP16b), where the search path has them.
    pub pactl: Option<PathBuf>,
    pub wl_copy: Option<PathBuf>,
    /// The kernel's backlight devices (`/sys/class/backlight`).
    pub backlight: PathBuf,
    /// What the reading providers read and run (WP15).
    pub read: read::ReadEnv,
    /// `[launch]` (WP16a).
    pub launch: fdo::launch::settings::LaunchConfig,
    /// Kai's state directory (`$XDG_STATE_HOME/kai`), where the portal's restore token is kept
    /// (WP16c); empty: nothing is stored.
    pub state_dir: PathBuf,
    /// Kai's accessibility switch (`fdo::atspi::switch`), shared with the core; `None`: Kai
    /// switches nothing.
    pub a11y: Option<fdo::atspi::switch::A11ySwitch>,
}

/// `name` as a program on the search path.
pub fn in_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

impl Env {
    /// The running Kai's: the process's environment, the host's session bus.
    #[cfg_attr(test, allow(dead_code))]
    pub fn from_deps(deps: &Deps) -> Self {
        let paths = crate::paths::Paths::from_env().ok();
        Self {
            current_desktop: std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
            session: Some(deps.bus.clone()),
            system: Some(SystemBus::new(None, deps.config.dbus.timeout())),
            wayland: std::env::var_os("WAYLAND_DISPLAY").is_some(),
            timeout: deps.config.dbus.timeout(),
            runtime_dir: paths
                .as_ref()
                .map(|p| p.runtime_dir.clone())
                .unwrap_or_default(),
            pactl: in_path("pactl"),
            wl_copy: in_path("wl-copy"),
            backlight: PathBuf::from("/sys/class/backlight"),
            read: read::ReadEnv::from_env(&deps.config.search, &deps.config.accessibility),
            launch: deps.config.launch.clone(),
            state_dir: paths.map(|p| p.state_dir).unwrap_or_default(),
            a11y: Some(deps.a11y.clone()),
        }
    }

    /// Nothing to probe: no desktop, no bus, no display (the unit tests' providers, which must
    /// never touch the session they run in).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn inert() -> Self {
        Self {
            current_desktop: String::new(),
            session: None,
            system: None,
            wayland: false,
            timeout: Duration::from_secs(1),
            runtime_dir: PathBuf::new(),
            pactl: None,
            wl_copy: None,
            backlight: PathBuf::new(),
            read: read::ReadEnv::default(),
            launch: Default::default(),
            state_dir: PathBuf::new(),
            a11y: None,
        }
    }

    /// `XDG_CURRENT_DESKTOP` names KDE (a colon-separated list).
    pub fn is_plasma(&self) -> bool {
        self.current_desktop
            .split(':')
            .any(|d| d.eq_ignore_ascii_case("KDE"))
    }
}

/// A method call bounded by `timeout`, its answer's body read as `R`.
pub async fn call<B, R>(
    bus: &zbus::Connection,
    dest: &str,
    path: &str,
    iface: &str,
    method: &str,
    body: &B,
    timeout: Duration,
) -> Result<R, Failure>
where
    B: serde::Serialize + DynamicType,
    R: zbus::zvariant::Type + for<'d> serde::Deserialize<'d>,
{
    let short = dest.rsplit('.').next().unwrap_or(dest);
    match tokio::time::timeout(
        timeout,
        bus.call_method(Some(dest), path, Some(iface), method, body),
    )
    .await
    {
        Ok(Ok(m)) => m
            .body()
            .deserialize()
            .map_err(|e| Failure::Failed(format!("{dest} answered {method} oddly: {e}"))),
        Ok(Err(zbus::Error::MethodError(name, _, _)))
            if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" =>
        {
            Err(Failure::Failed(format!("{short} is not running ({dest})")))
        }
        Ok(Err(e)) => Err(Failure::Failed(format!("{dest} refused {method}: {e}"))),
        Err(_) => Err(Failure::Failed(format!(
            "{dest} did not answer {method} within {} s (dbus.timeout_s)",
            timeout.as_secs()
        ))),
    }
}

/// One property, bounded by `timeout`.
pub async fn property<R>(
    bus: &zbus::Connection,
    dest: &str,
    path: &str,
    iface: &str,
    name: &str,
    timeout: Duration,
) -> Result<R, Failure>
where
    R: TryFrom<OwnedValue>,
{
    let v: OwnedValue = call(
        bus,
        dest,
        path,
        "org.freedesktop.DBus.Properties",
        "Get",
        &(iface, name),
        timeout,
    )
    .await?;
    R::try_from(v).map_err(|_| Failure::Failed(format!("{dest}'s {name} is not what Kai expects")))
}

/// Whether `name` has an owner on `bus`.
pub async fn has_owner(
    bus: &zbus::Connection,
    name: &str,
    timeout: Duration,
) -> Result<bool, String> {
    call(
        bus,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        &(name,),
        timeout,
    )
    .await
    .map_err(|e| e.to_string())
}

/// The probe's outcome: the table and the entries it lists.
struct Ready {
    chosen: Vec<Chosen>,
    entries: Vec<Entry>,
}

struct State {
    env: Env,
    config: DesktopConfig,
    ready: OnceLock<Ready>,
    started: AtomicBool,
    changes: Changes,
    /// The running Kai's provider publishes the tray line; a test's does not.
    publish: bool,
}

/// The desktop module's provider (module doc). A clone shares the probe and its entries (the
/// delegates use them, `crate::delegate`).
#[derive(Clone)]
pub struct DesktopTools {
    state: Arc<State>,
}

/// The provider `registry::providers` lists. In the unit tests it probes nothing (no session,
/// no display): `tests` build their own over private buses.
pub fn provider(deps: &Deps) -> DesktopTools {
    #[cfg(not(test))]
    let (env, publish) = (Env::from_deps(deps), true);
    #[cfg(test)]
    let (env, publish) = (Env::inert(), false);
    DesktopTools::new(
        env,
        deps.config.desktop.clone(),
        deps.changes.clone(),
        publish,
    )
}

impl DesktopTools {
    pub fn new(env: Env, config: DesktopConfig, changes: Changes, publish: bool) -> Self {
        Self {
            state: Arc::new(State {
                env,
                config,
                ready: OnceLock::new(),
                started: AtomicBool::new(false),
                changes,
                publish,
            }),
        }
    }

    /// Runs the probe now, in this task, unless it ran or runs (the tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub async fn probe_now(&self) {
        if !self.state.started.swap(true, Ordering::SeqCst) {
            self.state.probe().await;
        }
    }

    /// The table's choices, once probed.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn chosen(&self) -> Vec<Chosen> {
        self.state
            .ready
            .get()
            .map(|r| r.chosen.clone())
            .unwrap_or_default()
    }

    fn start_probe(&self) {
        if self.state.started.swap(true, Ordering::SeqCst) {
            return;
        }
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                let state = self.state.clone();
                rt.spawn(async move {
                    state.probe().await;
                    state.changes.signal();
                });
            }
            Err(_) => match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(self.state.probe()),
                Err(e) => tracing::error!("desktop: cannot run the probe: {e}; no desktop tools"),
            },
        }
    }
}

impl State {
    async fn probe(&self) {
        let (plasma, fdo) = tokio::join!(plasma::probe(&self.env), fdo::probe(&self.env));
        let probes = [plasma, fdo];
        let chosen = probe::choose(&probes, &self.config);
        let built = probe::built(&probes);
        tracing::info!(
            "desktop: the capability table (XDG_CURRENT_DESKTOP={:?})",
            self.env.current_desktop
        );
        for line in probe::log_lines(&chosen) {
            tracing::info!("desktop: {line}");
        }
        let mut services = Services {
            find_limit: self.env.read.find_limit,
            focus_wait: self.env.timeout,
            card_keyboard: crate::approvals::keyboard::Holder::global(),
            a11y: self.env.a11y.clone(),
            ..Services::default()
        };
        for c in &chosen {
            match &c.service {
                Some(Service::Windows(s)) => services.windows = Some(s.clone()),
                Some(Service::Session(s)) => services.session = Some(s.clone()),
                Some(Service::Media(s)) => services.media = Some(s.clone()),
                Some(Service::Notify(s)) => services.notify = Some(s.clone()),
                Some(Service::Audio(s)) => services.audio = Some(s.clone()),
                Some(Service::Capture(s)) => services.capture = Some(s.clone()),
                Some(Service::Accessibility(s)) => services.accessibility = Some(s.clone()),
                Some(Service::Search(s)) => services.search = Some(s.clone()),
                Some(Service::Clipboard(s)) => services.clipboard = Some(s.clone()),
                Some(Service::Launch(s)) => services.launch = Some(s.clone()),
                Some(Service::Input(s)) => services.input = Some(s.clone()),
                None => {}
            }
            if c.capability == Capability::Input
                && c.service.is_some()
                && c.provider == Some(probe::ProviderName::Fdo)
                && !self.env.state_dir.as_os_str().is_empty()
            {
                services.input_token = Some(fdo::input::token_file(&self.env.state_dir));
            }
        }
        let entries = entries(&services);
        if self.publish {
            TABLE.send_replace(vec![probe::tray_line(&chosen, &built)]);
        }
        let _ = self.ready.set(Ready { chosen, entries });
    }
}

/// The tools the services support (K23: the others are not listed): `contract::windows`', then
/// `contract::read`'s (WP15).
pub(crate) fn entries(services: &Services) -> Vec<Entry> {
    let windows = AREA
        .tools
        .iter()
        .filter(|spec| {
            if needs_windows(spec.name) {
                services.windows.is_some()
            } else {
                services.any()
            }
        })
        .map(|spec| (AREA.name, spec));
    let session = session::AREA
        .tools
        .iter()
        .filter(|spec| serves(services, spec.name))
        .map(|spec| (session::AREA.name, spec));
    let reads = read::AREA
        .tools
        .iter()
        .filter(|spec| read::serves(services, spec.name))
        .map(|spec| (read::AREA.name, spec));
    let launch = launch::AREA
        .tools
        .iter()
        .filter(|_| services.launch.is_some())
        .map(|spec| (launch::AREA.name, spec));
    let inputs = input::AREA
        .tools
        .iter()
        .filter(|spec| input_serves(services, spec.name))
        .map(|spec| (input::AREA.name, spec));
    windows
        .chain(session)
        .chain(reads)
        .chain(launch)
        .chain(inputs)
        .map(|(area, spec)| {
            let prepare = match spec.name {
                "open" => Some(tools::launch::prepare_open(services)),
                n if tools::input::handles(n) => Some(tools::input::prepare(services, spec.name)),
                n if tools::is_window_action(n) => Some(tools::prepare_window(services)),
                _ => None,
            };
            let services = services.clone();
            Entry {
                area,
                spec,
                handler: Arc::new(move |call| {
                    let services = services.clone();
                    Box::pin(async move {
                        if area == read::AREA.name {
                            read::run(&services, call.spec.name, &call.args).await
                        } else {
                            let ctx = tools::Ctx::of(&call);
                            tools::run_with(&services, call.spec.name, &call.args, &ctx).await
                        }
                    })
                }),
                prepare,
            }
        })
        .collect()
}

/// A tool of `contract::input` (WP16c) has what it needs: `ui_press` the accessibility and window
/// services, the others the Input and window services (the window with the focus is what the
/// owner confirms and what the typing checks).
fn input_serves(services: &Services, name: &str) -> bool {
    let needs = input::needs(name);
    !needs.is_empty()
        && needs.iter().all(|cap| match cap {
            Capability::Accessibility => services.accessibility.is_some(),
            Capability::Windows => services.windows.is_some(),
            Capability::Input => services.input.is_some(),
            _ => false,
        })
}

/// A tool of `contract::session` has what it needs: its capability's service, and for the few
/// tools only some providers can do (do not disturb, brightness, Night Light) that feature.
fn serves(services: &Services, name: &str) -> bool {
    let Some(cap) = session::needs(name) else {
        return false;
    };
    match cap {
        Capability::Media => services.media.is_some(),
        Capability::Audio => services.audio.is_some(),
        Capability::Clipboard => services.clipboard.is_some(),
        Capability::Notify => services
            .notify
            .as_ref()
            .is_some_and(|n| name != "dnd" || n.can_dnd()),
        Capability::Session => services.session.as_ref().is_some_and(|s| match name {
            "brightness_set" => s.features().brightness,
            "night_light" => s.features().night_light,
            _ => true,
        }),
        _ => false,
    }
}

impl Provider for DesktopTools {
    fn entries(&self) -> Vec<Entry> {
        self.start_probe();
        self.state
            .ready
            .get()
            .map(|r| r.entries.clone())
            .unwrap_or_default()
    }
}
