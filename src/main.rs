//! Kai: the owner's Plasma client of lmgw (docs/design/2026-10-06-kai-design.md).
//!
//! Built so far: the NVIDIA rule, configuration, single instance, the device's key in the Secret
//! Service with the locked-wallet state, pairing over lmgw's device link (paste dialog and K13's
//! handler path, enrolment, mutual TLS), the tray
//! and the lmgw link's feed (WP2), audio (WP3), the voice session and the global shortcuts (WP4's
//! local half), the ring overlay (WP5), `lmgw-client`'s rules (WP1), the self-updater on lmgw's
//! `lmgw-update` (§13), and the hosted MCP server with K15's policy (WP8, [`host`]). The realtime
//! session plugs in at [`voice::link`].

mod approvals;
mod audio;
mod autostart;
mod cli;
mod config;
mod conversation;
mod core;
mod delegate;
mod desktop;
mod dnd;
mod host;
mod instance;
mod jobs;
mod launch;
#[cfg(feature = "live-test")]
mod live_test;
mod lmgw_link;
mod logging;
mod notify;
mod nvidia;
mod overlay;
mod pairing;
mod paths;
mod personality;
mod routines;
mod screen_lock;
mod secret;
mod settings;
mod shortcuts;
mod sleep;
mod tray;
mod ui;
mod update;
mod voice;
mod widgets;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{mpsc, watch};

use crate::config::Config;
use crate::core::{Command, Core, Ended};
use crate::instance::{BUS_NAME, HandoffError, PairGate, PairingService};
use crate::pairing::PairingLink;
use crate::paths::Paths;
use crate::secret::SecretServiceStore;
use crate::tray::model::TrayModel;
use crate::ui::UiCmd;

/// The app id (K12): D-Bus name, `.desktop` file, icon, portal id.
pub const APP_ID: &str = "dev.rootshell.kai";

/// The version this build reports (`--version`, the log) and the updater compares: the package's,
/// which the RPM's build hands to cargo as `KAI_VERSION` (`<X.Y.Z>+<pipeline IID>` in CI, so a
/// private build sorts after the release it follows), else Cargo.toml's. Cargo.toml itself keeps
/// X.Y.Z, so the spec's `cargo build --locked` holds.
pub const VERSION: &str = match option_env!("KAI_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => env!("CARGO_PKG_VERSION"),
};

const USAGE: &str = "\
usage: kai                  run Kai (one per session; a second start does nothing)
       kai 'lmgw-pair:?…'   pair: hand the link to the running Kai, or start Kai with it; Kai
                            asks you to confirm it in its own dialog before it stores anything
       kai --pair-stdin     run Kai with a pairing link read from stdin (how the handler starts it,
                            so the link never sits in Kai's argv)
       kai --tool-list <label> print the hosted tools as lmgw sends them to a model under <label>
                            (OpenAI's tools array; `just tool-cost` counts its tokens)
       kai --forget-remote-control
                            forget the RemoteDesktop portal's restore token, so typing through the
                            portal asks again (revoking Kai in System Settings does not void it)
       kai tool [<name> [--<param> <value>]...]   call one of the running Kai's tools (none:
                            list them; <name> --help: its parameters), as the caller `cli`, or
                            as the routine whose script runs it (K28)
       kai model <alias>    the prompt on stdin, the answer of lmgw's <alias> on stdout, through
                            the running Kai's key
       kai --version | --help
configuration: $XDG_CONFIG_HOME/kai/config.toml; log level: KAI_LOG (default info)
";

/// The flag the scheme handler starts Kai with (K13): the link comes on stdin, not in argv.
const PAIR_STDIN: &str = "--pair-stdin";

enum Invocation {
    Run {
        link_on_stdin: bool,
    },
    Pair(String),
    ToolList(String),
    ForgetRemoteControl,
    /// `kai tool …` and `kai model …` (K28): their own arguments.
    Tool(Vec<OsString>),
    Model(Vec<OsString>),
    Help,
    Version,
}

/// Never echoes an argument: the scheme handler's argv carries a key (K13).
fn parse_args(args: &[OsString]) -> Result<Invocation, &'static str> {
    match args {
        [] => Ok(Invocation::Run {
            link_on_stdin: false,
        }),
        [a] if a == PAIR_STDIN => Ok(Invocation::Run {
            link_on_stdin: true,
        }),
        [a] if a == "--help" || a == "-h" => Ok(Invocation::Help),
        [a, rest @ ..] if a == "tool" => Ok(Invocation::Tool(rest.to_vec())),
        [a, rest @ ..] if a == "model" => Ok(Invocation::Model(rest.to_vec())),
        [a] if a == "--version" || a == "-V" => Ok(Invocation::Version),
        [a] if a == "--forget-remote-control" => Ok(Invocation::ForgetRemoteControl),
        [a] if a == "--tool-list" => {
            Err("--tool-list needs the label lmgw lists Kai's tools under")
        }
        [a, label] if a == "--tool-list" => match label.to_str() {
            Some(l) if !l.trim().is_empty() => Ok(Invocation::ToolList(l.to_owned())),
            _ => Err("--tool-list needs the label lmgw lists Kai's tools under"),
        },
        [a] => match a.to_str() {
            Some(s)
                if s.trim_start()
                    .get(..10)
                    .is_some_and(|p| p.eq_ignore_ascii_case("lmgw-pair:")) =>
            {
                Ok(Invocation::Pair(s.to_owned()))
            }
            _ => Err("unexpected argument (not shown: it may carry a key)"),
        },
        _ => Err("too many arguments (not shown: they may carry a key)"),
    }
}

fn main() -> ExitCode {
    let nvidia = nvidia::apply(); // §2: the first statement, before GTK and before any thread.
    logging::init();
    tracing::debug!(?nvidia, "NVIDIA rule");
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match parse_args(&args) {
        Ok(Invocation::Run { link_on_stdin }) => run(link_on_stdin, nvidia),
        Ok(Invocation::Pair(link)) => pair_handler(link),
        Ok(Invocation::ToolList(label)) => tool_list(&label),
        Ok(Invocation::ForgetRemoteControl) => forget_remote_control(),
        Ok(Invocation::Tool(rest)) => cli::client::tool(&rest),
        Ok(Invocation::Model(rest)) => cli::client::model(&rest),
        Ok(Invocation::Help) => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Invocation::Version) => {
            println!("kai {VERSION}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprint!("kai: {e}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// The hosted server (WP8): its providers, K15's policy from the configuration, the GTK dialog for
/// its confirmations. The `[policy]` settings that name no tool come with it, for the tooltip.
fn hosted_server(
    config: &Config,
    cmds: mpsc::UnboundedSender<Command>,
    ui: async_channel::Sender<UiCmd>,
    bus: host::registry::SessionBus,
    screen: screen_lock::ScreenLock,
    a11y: desktop::fdo::atspi::switch::A11ySwitch,
) -> (host::HostLink, Vec<String>) {
    let (changes, changed) = host::registry::Changes::new();
    let (creds_tx, creds) = watch::channel(None);
    let deps = host::registry::Deps {
        cmds,
        ui: ui.clone(),
        bus,
        config: Arc::new(config.clone()),
        creds,
        policy: Arc::new(std::sync::RwLock::new(host::policy::Policy::new(
            &config.policy,
        ))),
        changes,
        screen,
        a11y,
        jobs: jobs::Jobs::default(),
    };
    let link = host::HostLink::new(
        host::link::LinkSettings::from_config(config),
        deps.policy.clone(),
        host::registry::providers(&deps),
        changed,
        Arc::new(host::confirm::Dialog(ui)),
        creds_tx,
        deps.screen.clone(),
        deps.jobs.clone(),
    );
    link.set_core(deps.cmds.clone());
    let notes = link.unknown_policy_names();
    (link, notes)
}

/// `kai --tool-list <label>`: the tool list this build offers with this configuration's policy,
/// as lmgw renders it for a model (`just tool-cost`).
fn tool_list(label: &str) -> ExitCode {
    let config = match Paths::from_env()
        .map_err(|e| e.to_string())
        .and_then(|p| Config::load(&p.config_file).map_err(|e| e.to_string()))
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("kai: configuration: {e}");
            return ExitCode::FAILURE;
        }
    };
    // `[policy]` settings that name no tool are logged by the host as it is built.
    let (host, _) = hosted_server(
        &config,
        mpsc::unbounded_channel().0,
        async_channel::unbounded().0,
        host::registry::SessionBus::lazy(config.dbus.timeout()),
        screen_lock::ScreenLock::default(),
        desktop::fdo::atspi::switch::A11ySwitch::inert(),
    );
    println!("{}", host.tool_list_json(label));
    ExitCode::SUCCESS
}

/// `kai --forget-remote-control`: removes the stored restore token of the RemoteDesktop portal
/// (WP16c), so the next use through the portal shows the desktop's dialog again.
fn forget_remote_control() -> ExitCode {
    let paths = match Paths::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("kai: {e}");
            return ExitCode::FAILURE;
        }
    };
    let file = desktop::fdo::input::token_file(&paths.state_dir);
    match file.forget() {
        Ok(true) => {
            println!(
                "kai: forgot the remote-control permission ({})",
                file.0.display()
            );
            ExitCode::SUCCESS
        }
        Ok(false) => {
            println!("kai: no remote-control permission was stored");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("kai: cannot remove {}: {e}", file.0.display());
            ExitCode::FAILURE
        }
    }
}

struct Setup {
    rt: tokio::runtime::Runtime,
    bus: zbus::Connection,
    paths: Paths,
    config: Config,
}

/// The tokio runtime, the session bus and the configuration. A bad configuration is an error
/// naming the file and the key, logged and shown as a notification. The bus connect is bounded by
/// `dbus.timeout_s` (the default when the configuration cannot be read): the scheme handler's argv
/// carries a key, so it never waits unbounded (review KAI-2 K2-24).
fn setup() -> Option<Setup> {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("kai-tokio")
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("cannot start the tokio runtime: {e}");
            return None;
        }
    };
    let loaded = Paths::from_env()
        .map_err(|e| e.to_string())
        .and_then(|paths| {
            Config::load(&paths.config_file)
                .map(|config| (paths, config))
                .map_err(|e| e.to_string())
        });
    let timeout = match &loaded {
        Ok((_, config)) => config.dbus.timeout(),
        Err(_) => config::DbusConfig::default().timeout(),
    };
    // The timer is made inside the runtime (a `timeout` built outside it panics).
    let bus = match rt
        .block_on(async { tokio::time::timeout(timeout, zbus::Connection::session()).await })
    {
        Ok(Ok(bus)) => bus,
        Ok(Err(e)) => {
            tracing::error!("cannot connect to the D-Bus session bus: {e}");
            return None;
        }
        Err(_) => {
            tracing::error!(
                "the D-Bus session bus did not answer within {} s (dbus.timeout_s)",
                timeout.as_secs()
            );
            return None;
        }
    };
    match loaded {
        Ok((paths, config)) => Some(Setup {
            rt,
            bus,
            paths,
            config,
        }),
        Err(e) => {
            tracing::error!("configuration: {e}");
            rt.block_on(notify::send(
                &bus,
                "Kai cannot start",
                &format!("configuration: {e}"),
                timeout,
            ));
            None
        }
    }
}

/// This binary's path, for the autostart entry and the handler's start. `/proc/self/exe` reads
/// `<path> (deleted)` once the file was replaced while Kai runs (a rebuild, `dnf upgrade`); the
/// path is the new binary's (review KAI-2 K2-19).
fn current_exe() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("kai"));
    exe_path(exe)
}

fn exe_path(exe: PathBuf) -> PathBuf {
    match exe.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(live) => PathBuf::from(live),
        None => exe,
    }
}

fn audio_settings(config: &Config, paths: &Paths) -> audio::AudioSettings {
    audio::AudioSettings {
        echo_mode: config.audio.echo_mode,
        input: config.audio.input.clone(),
        output: config.audio.output.clone(),
        capture_ring_ms: config.audio.capture_ring_ms,
        playback_ring_ms: config.audio.playback_ring_ms,
        retry: std::time::Duration::from_secs(config.audio.retry_s),
        aec_ready_timeout: config.aec.ready_timeout(),
        aec_restore_timeout: config.aec.restore_timeout(),
        aec_file: paths.aec_file.clone(),
        runtime_dir: paths.runtime_dir.clone(),
    }
}

/// The running Kai. `link_on_stdin`: the scheme handler started it with a pairing link on stdin.
/// `nvidia`: what the NVIDIA rule did, so a restart hands the new Kai the environment Kai started
/// with.
fn run(link_on_stdin: bool, nvidia: nvidia::Decision) -> ExitCode {
    // Resolved first: a restart after an RPM upgrade runs the binary at this path, the new one.
    let exe = current_exe();
    #[cfg(feature = "live-test")]
    if live_test::active()
        && let Err(e) = live_test::config_home_ok()
    {
        tracing::error!("live-test: {e}");
        return ExitCode::FAILURE;
    }
    let Some(Setup {
        rt,
        bus,
        paths,
        config,
    }) = setup()
    else {
        return ExitCode::FAILURE;
    };
    let arrival = if link_on_stdin {
        match read_stdin_link(config.pairing.handoff_timeout()) {
            Ok(raw) => Some(raw),
            Err(e) => {
                tracing::error!("no pairing link from the scheme handler: {e}");
                rt.block_on(notify::send(
                    &bus,
                    "Kai could not pair",
                    &e,
                    config.dbus.timeout(),
                ));
                None
            }
        }
    } else {
        None
    };
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let gate = PairGate::default();
    match rt.block_on(instance::claim(
        &bus,
        PairingService::new(cmd_tx.clone(), gate.clone()),
    )) {
        Ok(true) => tracing::info!("Kai {VERSION} runs as {BUS_NAME}"),
        Ok(false) => {
            tracing::info!(
                "Kai already runs (the D-Bus name {BUS_NAME} is owned); this start does nothing"
            );
            // A Kai started meanwhile (two clicks at once): it takes the link instead.
            if let Some(raw) = arrival {
                return rt.block_on(hand_off_or_say(&bus, &config, &raw));
            }
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            tracing::error!("cannot claim the D-Bus name {BUS_NAME}: {e}");
            return ExitCode::FAILURE;
        }
    }

    // Kai's windows carry the Wayland app id dev.rootshell.kai, so KWin matches its .desktop file.
    glib::set_prgname(Some(APP_ID));
    if let Err(e) = gtk::init() {
        tracing::error!("cannot start GTK: {e}");
        return ExitCode::FAILURE;
    }

    let (audio_tx, audio_rx) = mpsc::unbounded_channel();
    let audio = match audio::Audio::start(audio_settings(&config, &paths), audio_tx) {
        Ok(audio) => audio,
        Err(e) => {
            tracing::error!("cannot start the audio thread: {e}");
            return ExitCode::FAILURE;
        }
    };

    let taps = audio.taps();
    let ring = overlay::RingSettings::from_config(&config);
    let (ui_tx, ui_rx) = async_channel::unbounded();
    let (model_tx, model_rx) =
        watch::channel(TrayModel::starting(autostart::is_on(&paths.autostart_file)));
    let store = SecretServiceStore::new(bus.clone(), config.wallet.timeout());
    let dbus_timeout = config.dbus.timeout();
    let bind_shortcuts = config.shortcuts.bind;
    let link = lmgw_link::FeedLink::new(lmgw_link::FeedSettings::from_config(&config));
    let voice_link =
        lmgw_link::RealtimeLink::new(lmgw_link::RealtimeSettings::from_config(&config));
    if let Some(raw) = arrival {
        // Parsed by the handler already; a link that fails here is said, not kept. It takes the
        // gate as a D-Bus `Pair` would (one link waits at a time, review KAI-2 K2-4).
        match PairingLink::parse(&raw) {
            Ok(link) if gate.claim() => {
                let _ = cmd_tx.send(Command::Arrived(link));
            }
            Ok(_) => {
                tracing::warn!("the pairing link on stdin: {}", instance::WAITING);
                rt.block_on(notify::send(
                    &bus,
                    "Kai could not pair",
                    instance::WAITING,
                    config.dbus.timeout(),
                ));
            }
            Err(e) => {
                tracing::error!("pairing link refused: {e}");
                rt.block_on(notify::send(
                    &bus,
                    "Kai could not pair",
                    &e.to_string(),
                    config.dbus.timeout(),
                ));
            }
        }
    }
    let update_config = config.update.clone();
    let runtime_dir = paths.runtime_dir.clone();
    let config_file = paths.config_file.clone();
    // The owner's rules of 2026-10-09: Kai is not active on a locked screen
    // (`screen_lock`), and Kai's accessibility switch (`desktop::fdo::atspi::switch`), whose
    // marker a former Kai that could not put the switch back left is looked at first.
    // Locked until the lock's watch looked (`screen_lock::ScreenLock::unknown`).
    let screen = screen_lock::ScreenLock::unknown();
    let a11y = desktop::fdo::atspi::switch::A11ySwitch::new(
        host::registry::SessionBus::connected(bus.clone(), dbus_timeout),
        dbus_timeout,
        config.accessibility.switch_settings(),
        Some(paths.state_dir.join(desktop::fdo::atspi::switch::MARKER)),
    );
    rt.spawn({
        let a11y = a11y.clone();
        async move { a11y.recover().await }
    });
    let (hosted, policy_notes) = hosted_server(
        &config,
        cmd_tx.clone(),
        ui_tx.clone(),
        host::registry::SessionBus::connected(bus.clone(), dbus_timeout),
        screen.clone(),
        a11y.clone(),
    );
    let mut core = Core::new(
        config,
        paths,
        exe.clone(),
        store,
        link,
        voice_link,
        (audio, audio_rx),
        model_tx,
        ui_tx.clone(),
        Some(bus.clone()),
    );
    core.set_pair_gate(gate);
    core.set_desktop_state(screen.clone(), a11y.clone());
    // K28: the routines' runtime (the CLI's socket, the folders' watch, the tray's and keys'
    // triggers) on the host's own path for calls.
    let socket = paths::cli_socket(&runtime_dir);
    rt.spawn(routines::serve(
        hosted.local(),
        routines::Runtime {
            local: hosted.local(),
            script: routines::ScriptEnv {
                manager: desktop::fdo::launch::spawn::UserManager {
                    bus: host::registry::SessionBus::connected(bus.clone(), dbus_timeout),
                    timeout: dbus_timeout,
                },
                own_env: desktop::fdo::launch::process_env,
                socket: socket.clone(),
                exe_dir: exe.parent().map(Path::to_path_buf),
                scopes: routines::LiveScopes::default(),
            },
            config_file,
            settings: routines::Settings::Core(cmd_tx.clone()),
        },
        socket,
        dbus_timeout,
    ));
    // K26's jobs (WP18): the host's table, for the core's tools, the voice and the tray.
    let jobs = hosted.jobs();
    jobs.publish();
    core.set_jobs(jobs);
    core.set_host(Box::new(hosted), policy_notes);
    let (updates_tx, updates_rx) = mpsc::unbounded_channel();
    core.set_updates(updates_tx);
    let updater = update::run(
        update_config,
        runtime_dir,
        bus.clone(),
        dbus_timeout,
        cmd_tx.clone(),
        updates_rx,
    );
    let terminated = listen_for_termination(cmd_tx.clone());
    let supervisor = rt.spawn(supervise(
        core,
        cmd_rx,
        cmd_tx.clone(),
        model_rx,
        ui_tx,
        bus,
        dbus_timeout,
        bind_shortcuts,
        updater,
        screen,
    ));
    ui::run(ui_rx, cmd_tx, ring, taps);
    let ended = rt.block_on(supervisor).ok().flatten();
    // The core's quit put accessibility back; this covers a core that panicked (idempotent).
    rt.block_on(a11y.shutdown());
    // Blocking work still running (a DNS lookup of reqwest or tungstenite) is not waited for past
    // `dbus.timeout_s` (review KAI-2 K2-24).
    rt.shutdown_timeout(dbus_timeout);
    tracing::info!("Kai has quit");
    match ended {
        // Checked right before the exec: a SIGTERM or SIGINT any time until here wins.
        Some(Ended::Restart) if terminated.load(Ordering::SeqCst) => {
            tracing::info!("a SIGTERM or SIGINT came during the restart: Kai quits without it");
            ExitCode::SUCCESS
        }
        Some(Ended::Restart) => restart(&exe, nvidia),
        Some(Ended::Quit) => ExitCode::SUCCESS,
        None => ExitCode::FAILURE,
    }
}

/// The updater's Restart, after the whole quit sequence: the installed binary replaces this process
/// (`exec`), so the new Kai keeps its PID and with it the app scope and cgroup Plasma started it in.
/// It starts plain (no argument: a pairing link is never carried over) with this environment, less
/// the NVIDIA variable when Kai set it itself, so the new Kai's rule decides as at a fresh start.
/// Returns only when the exec failed.
fn restart(exe: &Path, nvidia: nvidia::Decision) -> ExitCode {
    use std::os::unix::process::CommandExt;
    tracing::info!("restarting: {}", exe.display());
    let mut cmd = std::process::Command::new(exe);
    if nvidia == nvidia::Decision::Set {
        cmd.env_remove(nvidia::VAR);
    }
    let e = cmd.exec();
    tracing::error!(
        "cannot start {} for the restart: {e}; start Kai again",
        exe.display()
    );
    ExitCode::FAILURE
}

/// Runs the core, the tray, the global shortcuts (unless `shortcuts.bind` is off) and the
/// updater; after the core's quit sequence it removes the tray item, releases the name
/// and ends the glib loop. The tray's removal and the name's release each wait at most
/// `dbus.timeout_s`. How the core ended, `None` when it panicked.
#[allow(clippy::too_many_arguments)]
async fn supervise<
    S: secret::SecretStore,
    L: lmgw_link::LmgwLink,
    A: audio::AudioPort,
    V: voice::VoiceLink,
>(
    core: Core<S, L, A, V>,
    cmd_rx: mpsc::UnboundedReceiver<Command>,
    cmd_tx: mpsc::UnboundedSender<Command>,
    model_rx: watch::Receiver<TrayModel>,
    ui_tx: async_channel::Sender<UiCmd>,
    bus: zbus::Connection,
    dbus_timeout: std::time::Duration,
    bind_shortcuts: bool,
    updater: impl Future<Output = ()> + Send + 'static,
    screen: screen_lock::ScreenLock,
) -> Option<Ended> {
    let mut tray = tokio::spawn(tray::run(model_rx, cmd_tx.clone()));
    let updater = tokio::spawn(updater);
    #[cfg(feature = "live-test")]
    let bind_shortcuts = bind_shortcuts && !live_test::active();
    let shortcuts = if bind_shortcuts {
        tokio::spawn(shortcuts::run(cmd_tx.clone()))
    } else {
        tracing::info!("shortcuts.bind is off: no global shortcuts are registered with Plasma");
        let _ = cmd_tx.send(Command::ShortcutNote(Some(
            "global shortcuts are off (shortcuts.bind = false)".to_owned(),
        )));
        tokio::spawn(std::future::pending::<()>())
    };
    let resume = tokio::spawn(sleep::watch(cmd_tx.clone(), dbus_timeout));
    // Do not disturb, for the jobs' results (WP18).
    let dnd = tokio::spawn(dnd::watch(cmd_tx.clone(), bus.clone(), dbus_timeout));
    // The session's lock: the screen saver service on this bus, logind on the system bus.
    let lock = tokio::spawn({
        let bus = bus.clone();
        async move {
            let system = match tokio::time::timeout(dbus_timeout, zbus::Connection::system()).await
            {
                Ok(Ok(c)) => Some(c),
                Ok(Err(e)) => {
                    tracing::warn!("screen lock: no system bus ({e}); logind is not followed");
                    None
                }
                Err(_) => {
                    tracing::warn!(
                        "screen lock: the system bus did not answer within {} s \
                         (dbus.timeout_s); logind is not followed",
                        dbus_timeout.as_secs()
                    );
                    None
                }
            };
            // Kai's own logind session: the one it was started in.
            let session_id = std::env::var("XDG_SESSION_ID").ok();
            screen_lock::watch(cmd_tx, screen, Some(bus), system, session_id, dbus_timeout).await
        }
    });
    let core_result = tokio::spawn(core.run(cmd_rx)).await;
    if let Err(e) = &core_result {
        tracing::error!("the core stopped: {e}");
    }
    resume.abort();
    dnd.abort();
    lock.abort();
    // An install still running is left to dnf: its blocking thread is not waited for past
    // `dbus.timeout_s` either.
    updater.abort();
    // Dropping the portal's proxy and session with the task ends Kai's shortcut session.
    shortcuts.abort();
    let late = |what: &str| {
        tracing::warn!(
            "{what} did not finish within {} s (dbus.timeout_s); Kai quits anyway",
            dbus_timeout.as_secs()
        )
    };
    if tokio::time::timeout(dbus_timeout, &mut tray).await.is_err() {
        late("the tray item's removal");
        tray.abort();
    }
    if tokio::time::timeout(dbus_timeout, bus.release_name(BUS_NAME))
        .await
        .is_err()
    {
        late("releasing the D-Bus name");
    }
    let _ = ui_tx.send(UiCmd::Quit).await;
    core_result.ok()
}

/// SIGTERM and SIGINT quit Kai, on a thread of their own with its own small runtime, so the
/// listener outlives Kai's runtime: a signal any time until `main` decides, the restart's quit
/// sequence and the runtime's shutdown included, is seen. Each one sends Quit (into a core that may
/// be gone) and sets the flag returned, which cancels a restart's exec.
fn listen_for_termination(cmds: mpsc::UnboundedSender<Command>) -> Arc<AtomicBool> {
    let terminated = Arc::new(AtomicBool::new(false));
    let flag = terminated.clone();
    let started = std::thread::Builder::new()
        .name("kai-signals".to_owned())
        .spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(quit_on_signal(cmds, flag)),
                Err(e) => tracing::warn!("cannot listen for SIGTERM and SIGINT: {e}"),
            }
        });
    if let Err(e) = started {
        tracing::warn!("cannot listen for SIGTERM and SIGINT: {e}");
    }
    terminated
}

async fn quit_on_signal(cmds: mpsc::UnboundedSender<Command>, terminated: Arc<AtomicBool>) {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        tracing::warn!("cannot listen for SIGTERM and SIGINT");
        return;
    };
    loop {
        let name = tokio::select! {
            _ = term.recv() => "SIGTERM",
            _ = int.recv() => "SIGINT",
        };
        terminated.store(true, Ordering::SeqCst);
        tracing::info!("{name}: quitting");
        let _ = cmds.send(Command::Quit);
    }
}

/// K13: a `kai` started by the `lmgw-pair:` scheme handler. Its argv carries the link's one-time
/// token, so it lives
/// only as long as the hand-off takes: it gives the link to the running Kai over D-Bus (which
/// answers at once), or starts Kai with the link on a stdin pipe, and exits. It stores nothing and
/// waits for no confirmation; Kai asks the owner in its own dialog. Every call it makes is bounded
/// (`pairing.handoff_timeout_s`, `dbus.timeout_s`).
fn pair_handler(raw: String) -> ExitCode {
    // Checked first, so a bad link reaches nobody.
    let checked = PairingLink::parse(&raw)
        .map(|_| ())
        .map_err(|e| e.to_string());
    if let Err(e) = checked {
        tracing::error!("pairing link refused: {e}");
        if let Some(Setup {
            rt, bus, config, ..
        }) = setup()
        {
            rt.block_on(notify::send(
                &bus,
                "Kai could not pair",
                &e,
                config.dbus.timeout(),
            ));
        }
        return ExitCode::from(2);
    }
    let Some(Setup {
        rt, bus, config, ..
    }) = setup()
    else {
        return ExitCode::FAILURE;
    };
    rt.block_on(async {
        match instance::hand_off(&bus, &raw, config.pairing.handoff_timeout()).await {
            Ok(answer) => {
                tracing::info!("the running Kai took the pairing link: {answer}");
                ExitCode::SUCCESS
            }
            Err(HandoffError::NoInstance) => {
                let env = launch::session_environment(&bus, config.dbus.timeout()).await;
                match launch::start_with_link(&current_exe(), &raw, env) {
                    Ok(unit) => {
                        tracing::info!(%unit, "no Kai was running: started it with the link on stdin");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        let e = format!("cannot start Kai through systemd-run: {e}");
                        tracing::error!("{e}");
                        notify::send(&bus, "Kai could not pair", &e, config.dbus.timeout()).await;
                        ExitCode::FAILURE
                    }
                }
            }
            Err(e) => {
                tracing::error!("{e}");
                notify::send(&bus, "Kai could not pair", &e.to_string(), config.dbus.timeout())
                    .await;
                ExitCode::FAILURE
            }
        }
    })
}

/// Hands `raw` to the running Kai; a failure is said in a notification.
async fn hand_off_or_say(bus: &zbus::Connection, config: &Config, raw: &str) -> ExitCode {
    match instance::hand_off(bus, raw, config.pairing.handoff_timeout()).await {
        Ok(answer) => {
            tracing::info!("the running Kai took the pairing link: {answer}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            tracing::error!("{e}");
            notify::send(
                bus,
                "Kai could not pair",
                &e.to_string(),
                config.dbus.timeout(),
            )
            .await;
            ExitCode::FAILURE
        }
    }
}

/// The link the scheme handler wrote into Kai's stdin (a pipe it closed after writing). Waits at
/// most `timeout` (`pairing.handoff_timeout_s`) for the end of the input.
fn read_stdin_link(timeout: std::time::Duration) -> Result<String, String> {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    let deadline = std::time::Instant::now() + timeout;
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd for the length of the call.
        let n = unsafe { libc::poll(&mut pfd, 1, left.as_millis().min(i32::MAX as u128) as i32) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("cannot read stdin: {e}"));
        }
        if n == 0 {
            return Err(format!(
                "no complete pairing link arrived on stdin within {} s (pairing.handoff_timeout_s)",
                timeout.as_secs()
            ));
        }
        let k = stdin
            .lock()
            .read(&mut buf)
            .map_err(|e| format!("cannot read stdin: {e}"))?;
        if k == 0 {
            break;
        }
        got.extend_from_slice(&buf[..k]);
        if got.len() > launch::STDIN_LINK_MAX {
            return Err(format!(
                "the input on stdin is longer than {} bytes, which no pairing link is",
                launch::STDIN_LINK_MAX
            ));
        }
    }
    String::from_utf8(got)
        .map(|s| s.trim().to_owned())
        .map_err(|_| "the input on stdin is not text".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<OsString> {
        a.iter().map(OsString::from).collect()
    }

    #[test]
    fn a_replaced_binary_is_named_by_its_path() {
        assert_eq!(
            exe_path("/usr/bin/kai (deleted)".into()),
            PathBuf::from("/usr/bin/kai")
        );
        assert_eq!(
            exe_path("/usr/bin/kai".into()),
            PathBuf::from("/usr/bin/kai")
        );
    }

    #[test]
    fn argv_is_parsed_without_echoing_it() {
        assert!(matches!(
            parse_args(&args(&[])),
            Ok(Invocation::Run {
                link_on_stdin: false
            })
        ));
        assert!(matches!(
            parse_args(&args(&["--pair-stdin"])),
            Ok(Invocation::Run {
                link_on_stdin: true
            })
        ));
        assert!(matches!(
            parse_args(&args(&["--help"])),
            Ok(Invocation::Help)
        ));
        assert!(matches!(
            parse_args(&args(&["--forget-remote-control"])),
            Ok(Invocation::ForgetRemoteControl)
        ));
        assert!(matches!(
            parse_args(&args(&["lmgw-pair:?v=1"])),
            Ok(Invocation::Pair(_))
        ));
        assert!(matches!(
            parse_args(&args(&["tool", "notify", "--title", "x"])),
            Ok(Invocation::Tool(rest)) if rest.len() == 3
        ));
        assert!(
            matches!(parse_args(&args(&["tool"])), Ok(Invocation::Tool(rest)) if rest.is_empty())
        );
        assert!(matches!(
            parse_args(&args(&["model", "gemma"])),
            Ok(Invocation::Model(rest)) if rest.len() == 1
        ));
        assert!(matches!(
            parse_args(&args(&["LMGW-PAIR:?v=1"])),
            Ok(Invocation::Pair(_))
        ));
        let secret = "lmgw-device-0123456789abcdef";
        for bad in [args(&[secret]), args(&["lmgw-pair:?x", secret])] {
            let e = parse_args(&bad).err().unwrap();
            assert!(!e.contains(secret), "{e}");
        }
    }
}
