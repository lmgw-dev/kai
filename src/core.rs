//! The tokio core (§2): owns the tray model, the pairing, the lmgw link, the audio thread's handle
//! and the voice session, and takes [`Command`]s from the tray, the shortcuts, the paste dialog
//! (GTK) and the D-Bus hand-off (K13). It tells the ring overlay on the GTK thread what the voice
//! session does ([`Core::set_ring`]). It follows the Kai conversation's folder (§4,
//! [`crate::conversation`]): the lookup and creation through the voice's link, the remembered id in
//! the configuration, the folder for the voice, and Open in lmgw. It starts and stops the hosted MCP
//! server beside the feed ([`hosting`], PD2) and runs the tools that act on it ([`calls`], PD3).

pub mod calls;
mod conversation_mode;
mod hosting;
mod jobs;
mod personality;
mod settings_ops;
mod window;
/// What the window's Personality page says while Kai is not paired (its live check's stand-in core).
#[cfg(test)]
pub(crate) use window::NOT_PAIRED as WINDOW_NOT_PAIRED;

use std::path::PathBuf;

use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use crate::audio::{AudioEvent, AudioPort, EchoMode};
use crate::autostart;
use crate::config::{self, Config};
use crate::conversation::{self, Conversation, FolderInfo};
use crate::instance::{PairGate, WAITING};
use crate::lmgw_link::{Credentials, FolderEvent, KeyLoss, LinkEvent, LmgwLink};
use crate::overlay::{OverlayCmd, RingInput};
use crate::pairing::{self, Committed, DeviceKey, PairError, PairFacts, PairingLink};
use crate::paths::Paths;
use crate::secret::{Lookup, Removed, SecretError, SecretStore};
use crate::tray::model::{DISABLED, EXPIRED, Link, LinkWait, Pairing, TrayModel};
use crate::ui::{ConfirmRequest, UiCmd};
#[cfg(test)]
use crate::voice::Notice;
use crate::voice::link::ProfileList;
use crate::voice::link::{Pending, Refusal};
use crate::voice::{FeedFact, Voice, VoiceLink, VoiceMode, VoiceSettings, Why};

pub enum Command {
    /// A pairing link from the scheme handler: handed over on D-Bus (`Pairing1.Pair`, from any
    /// process of the session) or on stdin to a Kai the handler started (K13). Nothing is stored
    /// until the owner confirms it in Kai's own dialog.
    Arrived(PairingLink),
    /// The owner's answer in the confirmation dialog for the link with this id.
    ConfirmPair {
        id: u64,
        accept: bool,
    },
    /// The paste dialog's Pair: the dialog showed the parsed gateway and device first, so the
    /// owner confirmed there. `shown` is the pairing the dialog said the link replaces (the
    /// configured `lmgw.url` as it last heard); a pairing that changed meanwhile is refused, so no
    /// key is deleted the owner was not shown (review KAI-2 K2-5). The answer goes back to the
    /// dialog.
    PairPasted {
        link: PairingLink,
        shown: Option<String>,
        reply: oneshot::Sender<Result<(), PasteError>>,
    },
    OpenPairDialog,
    /// The tray's Settings…: the configuration window (WP21b), opened or raised.
    OpenSettings,
    /// One operation of the configuration window (`crate::settings::window`), its answer going back
    /// to the scheme handler: run like the owner tools' calls, so the core's configuration and the
    /// host's policy follow what was written.
    Setting {
        request: crate::settings::Request,
        reply: oneshot::Sender<Result<crate::settings::Response, crate::settings::Refusal>>,
    },
    /// The configuration window's Personality page: what the core knows of Kai's conversation
    /// (`crate::settings::window::profiles::Conversation`).
    WindowConversation {
        reply: oneshot::Sender<crate::settings::window::profiles::Conversation>,
    },
    /// One of the Personality page's profile-editor requests: sent to the paired lmgw with the
    /// device key, lmgw's answer (or the refusal) going back to the scheme handler.
    WindowProxy {
        forward: crate::lmgw_link::proxy::Forward,
        reply: oneshot::Sender<crate::lmgw_link::proxy::Proxied>,
    },
    /// The configuration window's Desktop page: whether the RemoteDesktop portal's restore token
    /// is stored, or (`forget`) its removal, as `kai --forget-remote-control` does (WP16c).
    WindowRemoteControl {
        forget: bool,
        reply: oneshot::Sender<Result<crate::settings::window::remote::RemoteControl, String>>,
    },
    SetAutostart(bool),
    /// The tray's Echo mode radio (§7), or the device warning's offer of half duplex.
    SetEchoMode(EchoMode),
    /// The tray's Microphone and Speaker menus: a `node.name`, or `None` for Default.
    SetInput(Option<String>),
    SetOutput(Option<String>),
    /// The tray's Overlays → "Ring over fullscreen windows" (§16 answer 5).
    SetRingOverFullscreen(bool),
    /// The ring overlay: its line for the tooltip while the ring is down after a second crash of
    /// its web process, `None` once it is not (§9.4).
    RingNote(Option<String>),
    /// The `talk` shortcut (§8.1): the portal's `Activated` (true) and `Deactivated` (false).
    Talk(bool),
    /// Left click on the tray.
    ToggleListening,
    /// The menu's Start/Stop listening.
    SetListening(bool),
    /// Middle click and the `mute` shortcut.
    ToggleMute,
    /// The menu's Mute check item.
    SetMuted(bool),
    /// The menu's Automatic / Push-to-talk radio (`voice.mode`).
    SetVoiceMode(VoiceMode),
    /// The `new-conversation` shortcut and the menu item.
    NewConversation,
    /// The menu's Open in lmgw (§7 item 5): the thread's dashboard page.
    OpenInLmgw,
    /// The tray's Personality radio (WP22): a profile's id, `None` for Default.
    SetPersonality(Option<i64>),
    /// The shortcuts' state for the tooltip (§7).
    ShortcutNote(Option<String>),
    /// logind: the machine resumed (`crate::sleep`); the idle rollover is looked at now (K2-8).
    Resumed,
    /// The session's screen lock changed (`crate::screen_lock`, the owner's decision of
    /// 2026-10-09): while locked Kai is not active; after the unlock it is idle. The shared flag
    /// says which: its watch is the flag's only writer, and the core reads it when this comes (a
    /// burst of changes queues several of these; each reads the flag as it is then).
    ScreenLock,
    /// Do not disturb is on or off (`crate::dnd`): a job's result is notified, not spoken, while
    /// it is on (WP18).
    Dnd(bool),
    /// The tray's Try again, offered while the device is disabled on lmgw or its key expired
    /// (§3.3, review KAI-3 K3-6): the stored key is tried again.
    TryAgain,
    /// The tray's "Check for Updates…": handed to the updater (`crate::update`), which answers.
    CheckForUpdates,
    Quit,
    /// PD3: a hosted tool that acts on the core, with its answer's channel (boxed: a call carries
    /// its context, and the other commands are small).
    Tool(Box<calls::CoreCall>),
    /// The updater's Restart, after an update was installed: the quit sequence of [`Command::Quit`],
    /// then `main` execs the installed binary ([`Ended::Restart`]). After a quit began it does
    /// nothing but a log line.
    Restart,
}

/// How the core's run ended: what `main` does once the quit sequence is done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    Quit,
    /// The updater's Restart: `main` execs the installed binary in place (the same app scope).
    Restart,
}

/// Why the paste dialog's Pair did nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteError {
    /// The pairing changed while the dialog was open: it shows the facts again with `current`.
    Changed { current: Option<String> },
    /// Said in the dialog (a locked wallet, a failed store).
    Failed(String),
}

impl std::fmt::Display for PasteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PasteError::Changed { .. } => f.write_str(
                "Kai's pairing changed while this dialog was open: check what the link replaces \
                 now, then press Pair again (nothing was stored)",
            ),
            PasteError::Failed(m) => f.write_str(m),
        }
    }
}

/// A folder request's answer (§4's lookup and creation).
enum FolderAnswer {
    Listed(Result<Vec<FolderInfo>, Refusal>),
    Created(Result<FolderInfo, Refusal>),
}

/// What Open in lmgw opens on the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardPage {
    /// The conversation's thread (`/chat?t=<id>`, the page's own deep link).
    Thread(i64),
    /// The folder's settings form, for a folder that waits for the owner's steps (`/chat?folder=
    /// <id>&settings=1`, `lmgw_client::requests::folder_settings_page`).
    FolderSettings(i64),
    /// The Chat page.
    Chat,
}

/// The dashboard URL for `page` under the paired base URL.
pub fn dashboard_url(base: &str, page: DashboardPage) -> String {
    let base = base.trim_end_matches('/');
    match page {
        DashboardPage::Thread(t) => format!("{base}/chat?t={t}"),
        DashboardPage::FolderSettings(f) => {
            format!("{base}{}", lmgw_client::requests::folder_settings_page(f))
        }
        DashboardPage::Chat => format!("{base}/chat"),
    }
}

/// A link from the scheme handler on its way to the owner (K13). It lives only in memory: it
/// waits for the wallet to open, then for the owner's answer in Kai's confirmation dialog. The
/// pairing Kai has is untouched until the owner confirms. One at a time: while it waits, every
/// further link is refused ([`PairGate`], review KAI-2 K2-4).
#[derive(Debug)]
struct Arrival {
    id: u64,
    link: PairingLink,
    stage: Stage,
    /// The "unlock KWallet" notification went out for it.
    told_locked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The wallet is locked (or the Secret Service fails): Kai checks again every
    /// `wallet.retry_s`.
    Wallet,
    /// The confirmation dialog shows it.
    Owner,
}

pub struct Core<S, L, A, V> {
    config: Config,
    paths: Paths,
    /// The binary "Start with Plasma" starts.
    exe: PathBuf,
    store: S,
    link: L,
    link_events: mpsc::UnboundedReceiver<LinkEvent>,
    audio: A,
    audio_events: mpsc::UnboundedReceiver<AudioEvent>,
    model: TrayModel,
    model_tx: watch::Sender<TrayModel>,
    ui: async_channel::Sender<UiCmd>,
    /// For desktop notifications; `None` in tests.
    bus: Option<zbus::Connection>,
    /// Notifications on their way (each bounded by `dbus.timeout_s`): the quit waits for them.
    notifying: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// A link from the scheme handler waiting for the wallet or the owner (one at a time).
    arrival: Option<Arrival>,
    /// Held while `arrival` waits: the D-Bus `Pair` refuses further links (review KAI-2 K2-4).
    gate: PairGate,
    /// Numbers the arrivals, so a late answer for a link that no longer waits is told apart.
    arrivals: u64,
    /// When to look at the wallet again (locked or failing).
    retry_at: Option<Instant>,
    /// What the ring overlay was last told.
    ring: RingInput,
    /// Audio notices said since PipeWire's link last changed, with how often each came: one
    /// notification each per outage, the repeats counted in the log.
    audio_notices: std::collections::HashMap<String, u64>,
    /// The voice session (§5, WP4).
    voice: Voice<V>,
    /// `voice.on_at_start` (K4) still has to open the first session: at the feed's first `hello`
    /// when the folder is known, else once it is ready.
    start_listening: bool,
    /// The Kai conversation's folder (§4).
    conversation: Conversation,
    /// The folder lookup or creation on its way (a newer one replaces it).
    folder_req: Option<Pending<FolderAnswer>>,
    /// The one question after the feed's `revoked` of a kind Kai does not know: does the key
    /// still open lmgw (§3.3, [`Core::revoked_unknown`])?
    key_ask: Option<Pending<Result<(), Refusal>>>,
    /// The last sentence of a revocation of a kind Kai does not know that was said: said once per
    /// reason (review KAI-4 K4-7); a revocation that decides clears it.
    unknown_said: Option<String>,
    /// Lmgw's personality profiles and the threads' (WP22).
    personality: crate::personality::Personality,
    /// The profile list on its way (a newer ask replaces it).
    profiles_req: Option<Pending<Result<ProfileList, Refusal>>>,
    /// The personality sets, which run in spawned tasks (WP22).
    sets: personality::Sets,
    /// Writing `conversation.folder_id` failed: why (a tooltip line until it works).
    remember_error: Option<String>,
    /// The updater's requests (the tray's check); `None` in tests that run none.
    updates: Option<mpsc::UnboundedSender<crate::update::Request>>,
    /// The hosted MCP server (PD2).
    hosting: hosting::Hosting,
    /// The screen lock, shared with the host (`crate::screen_lock`).
    screen: crate::screen_lock::ScreenLock,
    /// K26's jobs, the host's table (WP18, `crate::jobs`).
    jobs: crate::jobs::Jobs,
    /// The lock as the core last acted on it.
    lock_applied: bool,
    /// Kai's accessibility switch (`crate::desktop::fdo::atspi::switch`); `None` in tests that run
    /// none.
    a11y: Option<crate::desktop::fdo::atspi::switch::A11ySwitch>,
    /// What the switch was last told of conversation mode.
    a11y_conversation: bool,
    /// Where notifications go instead of the bus (the tests).
    #[cfg(test)]
    notices: Option<mpsc::UnboundedSender<Notice>>,
    /// Where Open in lmgw's URLs go instead of `xdg-open` (the tests).
    #[cfg(test)]
    opened: Option<mpsc::UnboundedSender<String>>,
}

impl<S: SecretStore, L: LmgwLink, A: AudioPort, V: VoiceLink> Core<S, L, A, V> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Config,
        paths: Paths,
        exe: PathBuf,
        store: S,
        link: L,
        voice_link: V,
        audio: (A, mpsc::UnboundedReceiver<AudioEvent>),
        model_tx: watch::Sender<TrayModel>,
        ui: async_channel::Sender<UiCmd>,
        bus: Option<zbus::Connection>,
    ) -> Self {
        let mut model = model_tx.borrow().clone();
        model.audio.mode = config.audio.echo_mode;
        model.audio.input = config.audio.input.clone();
        model.audio.output = config.audio.output.clone();
        model.overlay.ring_over_fullscreen = config.overlay.ring_over_fullscreen;
        let mut voice = Voice::new(VoiceSettings::from_config(&config), voice_link);
        let conversation = Conversation::new(
            config.conversation.folder_name.clone(),
            config.conversation.folder_id,
        )
        .retrying_up_to(config.lmgw.retry_max());
        voice.set_folder(conversation.initial());
        model.voice = voice.model(Instant::now());
        let start_listening = config.voice.on_at_start;
        let (_, link_events) = mpsc::unbounded_channel();
        let (audio, audio_events) = audio;
        Self {
            config,
            paths,
            exe,
            store,
            link,
            link_events,
            audio,
            audio_events,
            model,
            model_tx,
            ui,
            bus,
            notifying: std::sync::Mutex::new(Vec::new()),
            arrival: None,
            gate: PairGate::default(),
            arrivals: 0,
            retry_at: None,
            ring: RingInput::default(),
            audio_notices: std::collections::HashMap::new(),
            voice,
            start_listening,
            conversation,
            folder_req: None,
            personality: crate::personality::Personality::default(),
            profiles_req: None,
            sets: personality::Sets::default(),
            key_ask: None,
            unknown_said: None,
            remember_error: None,
            updates: None,
            hosting: hosting::Hosting::default(),
            screen: crate::screen_lock::ScreenLock::default(),
            jobs: crate::jobs::Jobs::default(),
            lock_applied: false,
            a11y: None,
            a11y_conversation: false,
            #[cfg(test)]
            notices: None,
            #[cfg(test)]
            opened: None,
        }
    }

    /// The gate the D-Bus `Pair` shares (`instance::PairingService`).
    pub fn set_pair_gate(&mut self, gate: PairGate) {
        self.gate = gate;
    }

    /// Where the tray's "Check for Updates…" goes (`crate::update`).
    pub fn set_updates(&mut self, tx: mpsc::UnboundedSender<crate::update::Request>) {
        self.updates = Some(tx);
    }

    /// The screen lock the host shares, and Kai's accessibility switch (the owner's rules of
    /// 2026-10-09).
    pub fn set_desktop_state(
        &mut self,
        screen: crate::screen_lock::ScreenLock,
        a11y: crate::desktop::fdo::atspi::switch::A11ySwitch,
    ) {
        self.screen = screen;
        self.a11y = Some(a11y);
    }

    /// The host's job table (WP18): the jobs tools and the voice read it.
    pub fn set_jobs(&mut self, jobs: crate::jobs::Jobs) {
        jobs.set_keep(self.config.jobs.keep_ended());
        self.voice.set_jobs(jobs.clone());
        self.jobs = jobs;
    }

    /// The screen lock's shared flag, which the tests set as the watch does.
    #[cfg(test)]
    pub fn screen_lock(&self) -> crate::screen_lock::ScreenLock {
        self.screen.clone()
    }

    /// The tests move the voice's wall clock alone (a suspend) through this.
    #[cfg(test)]
    pub fn wall_skew(&self) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
        self.voice.wall_skew()
    }

    /// The tests read Open in lmgw's URLs here.
    #[cfg(test)]
    pub fn open_into(&mut self, tx: mpsc::UnboundedSender<String>) {
        self.opened = Some(tx);
    }

    /// The tests read notifications here.
    #[cfg(test)]
    pub fn notify_into(&mut self, tx: mpsc::UnboundedSender<Notice>) {
        self.notices = Some(tx);
    }

    pub async fn run(mut self, mut cmds: mpsc::UnboundedReceiver<Command>) -> Ended {
        self.load().await;
        let ended = loop {
            let retry_at = self.retry_at;
            tokio::select! {
                cmd = cmds.recv() => match cmd {
                    None | Some(Command::Quit) => break Ended::Quit,
                    Some(Command::Restart) => break Ended::Restart,
                    Some(cmd) => self.handle(cmd).await,
                },
                Some(event) = self.link_events.recv() => self.on_link_event(event).await,
                Some(event) = self.hosting.events.recv() => self.on_host_event(event).await,
                Some(event) = self.audio_events.recv() => self.on_audio_event(event),
                wake = self.voice.next() => self.voice.handle(wake, &self.audio).await,
                answer = async {
                    match self.folder_req.as_mut() {
                        Some(f) => f.as_mut().await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.folder_req = None;
                    self.on_folder_answer(answer).await;
                }
                answer = async {
                    match self.profiles_req.as_mut() {
                        Some(f) => f.as_mut().await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.profiles_req = None;
                    self.on_profiles_answer(answer);
                }
                Some((id, step)) = self.sets.next() => self.on_personality_step(id, step).await,
                answer = async {
                    match self.key_ask.as_mut() {
                        Some(f) => f.as_mut().await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.key_ask = None;
                    self.on_key_answer(answer).await;
                }
                () = tokio::time::sleep_until(retry_at.unwrap_or_else(Instant::now)), if retry_at.is_some() => {
                    self.retry_at = None;
                    self.retry().await;
                }
            }
            self.sync_voice().await;
        };
        self.quit().await;
        // What came while the quit ran: a Quit wins over a Restart (Kai ends, nothing is started),
        // and a Restart after a Quit is not one.
        let mut ended = ended;
        while let Ok(cmd) = cmds.try_recv() {
            match cmd {
                Command::Quit if ended == Ended::Restart => {
                    tracing::info!("a Quit came during the restart's quit: Kai ends without it");
                    ended = Ended::Quit;
                }
                Command::Restart if ended == Ended::Quit => tracing::warn!(
                    "update: Kai was already quitting, so it ends without the restart; start it \
                     again to run the update"
                ),
                _ => {}
            }
        }
        ended
    }

    /// After every event: the voice's state reaches the tray and the ring, its notifications the
    /// desktop; a 4003 close of a kind Kai knows, or a 401 on `current` or the bind, revokes the
    /// pairing (§3.3).
    async fn sync_voice(&mut self) {
        let now = Instant::now();
        for n in self.voice.take_notices() {
            self.notify(n.summary, n.body);
        }
        if let Some(reason) = self.voice.take_revoked_unknown() {
            self.say_unknown(&reason);
        }
        if let Some((reason, loss)) = self.voice.take_revoked() {
            self.revoke(reason, loss).await;
        }
        if let Some((refusal, by_owner)) = self.voice.take_folder_refusal() {
            let acts = self.conversation.refused(&refusal, by_owner);
            self.apply(acts).await;
            for n in self.voice.take_notices() {
                self.notify(n.summary, n.body);
            }
        }
        if let Some(t) = self.voice.take_timing() {
            self.set_ring_timing(Some(t));
        }
        let ring = self.voice.ring(now);
        self.set_ring(ring);
        self.model.voice = self.voice.model(now);
        self.sync_personality();
        self.publish();
        // The accessibility switch follows conversation mode (the owner's rule, 2026-10-09).
        let conversation = self.voice.conversation_active();
        if conversation != self.a11y_conversation {
            self.a11y_conversation = conversation;
            if let Some(a) = &self.a11y {
                a.set_conversation(conversation);
            }
        }
    }

    /// The screen locked or unlocked (`crate::screen_lock`): the voice stops, the approval card and
    /// the widgets hide, accessibility is put back; after the unlock Kai is idle. The shared flag
    /// is read, never written here (its watch is its only writer), and only a change of it acts.
    async fn screen_locked(&mut self) {
        let locked = self.screen.is_locked();
        if locked == self.lock_applied {
            return;
        }
        self.lock_applied = locked;
        tracing::info!(locked, "the screen lock changed");
        self.voice.screen_lock(locked).await;
        self.ui_send(UiCmd::ScreenLocked(locked));
        if let Some(a) = &self.a11y {
            a.set_locked(locked);
        }
    }

    /// The paired Kai's voice commands; unpaired, they only say so.
    fn paired(&self) -> bool {
        matches!(self.model.pairing, Pairing::Paired { .. })
    }

    fn not_paired(&self) {
        tracing::info!("voice: not paired");
        if matches!(self.model.pairing, Pairing::Disabled { .. }) {
            return self.notify(
                "Kai: this device is disabled",
                format!("{DISABLED}; then the tray's \"Try again\""),
            );
        }
        if matches!(self.model.pairing, Pairing::Expired { .. }) {
            return self.notify(
                "Kai: this device's key expired",
                format!("{EXPIRED}; then the tray's \"Try again\""),
            );
        }
        self.notify(
            "Kai is not paired",
            "pair with lmgw first: the tray's \"Pair with lmgw…\"".to_owned(),
        );
    }

    async fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Arrived(link) => self.arrived(link).await,
            Command::ConfirmPair { id, accept } => self.confirmed(id, accept).await,
            Command::PairPasted { link, shown, reply } => {
                let result = self.pasted(link, shown).await;
                let _ = reply.send(result);
            }
            Command::OpenPairDialog => self.ui_send(UiCmd::ShowPairDialog {
                current: self.config.lmgw.url.clone(),
            }),
            Command::OpenSettings => self.ui_send(UiCmd::ShowSettings),
            Command::Setting { request, reply } => {
                let _ = reply.send(self.apply_request(request));
            }
            Command::WindowConversation { reply } => {
                let _ = reply.send(self.window_conversation());
            }
            Command::WindowProxy { forward, reply } => self.window_proxy(forward, reply),
            Command::WindowRemoteControl { forget, reply } => {
                let file = crate::desktop::fdo::input::token_file(&self.paths.state_dir);
                let _ = reply.send(crate::settings::window::remote::answer(&file, forget));
            }
            Command::SetAutostart(on) => self.set_autostart(on),
            Command::SetEchoMode(mode) => {
                if self.save_audio_choice("echo_mode", Some(mode.key())) {
                    self.config.audio.echo_mode = mode;
                    self.model.audio.mode = mode;
                    self.audio.set_echo_mode(mode);
                }
                self.publish();
            }
            Command::SetInput(node) => {
                if self.save_audio_choice("input", node.as_deref()) {
                    self.config.audio.input = node.clone();
                    self.model.audio.input = node.clone();
                    self.audio.set_input(node);
                }
                self.publish();
            }
            Command::SetOutput(node) => {
                if self.save_audio_choice("output", node.as_deref()) {
                    self.config.audio.output = node.clone();
                    self.model.audio.output = node.clone();
                    self.audio.set_output(node);
                }
                self.publish();
            }
            Command::SetRingOverFullscreen(on) => {
                match config::set_bool(
                    &self.paths.config_file,
                    "overlay",
                    "ring_over_fullscreen",
                    on,
                ) {
                    Ok(()) => {
                        tracing::info!(on, "overlay.ring_over_fullscreen saved");
                        self.config.overlay.ring_over_fullscreen = on;
                        self.model.overlay.ring_over_fullscreen = on;
                        self.model.overlay.error = None;
                        self.overlay(OverlayCmd::RingOverFullscreen(on));
                    }
                    Err(e) => {
                        tracing::warn!("cannot save overlay.ring_over_fullscreen: {e}");
                        self.model.overlay.error =
                            Some(format!("cannot save overlay.ring_over_fullscreen: {e}"));
                    }
                }
                self.publish();
            }
            Command::Talk(down) => {
                if !self.paired() {
                    if down {
                        self.not_paired();
                    }
                } else if down {
                    self.voice.talk_press(&self.audio).await;
                } else {
                    self.voice.talk_release(&self.audio).await;
                }
            }
            Command::ToggleListening if self.paired() => {
                self.voice.toggle_listening(&self.audio).await
            }
            Command::SetListening(on) if self.paired() => {
                self.voice.set_listening(on, &self.audio).await
            }
            Command::ToggleListening | Command::SetListening(_) => self.not_paired(),
            Command::ToggleMute => self.voice.toggle_mute(),
            Command::SetMuted(on) => self.voice.set_muted(on),
            Command::SetVoiceMode(mode) => {
                match config::set_value(&self.paths.config_file, "voice", "mode", Some(mode.key()))
                {
                    Ok(()) => {
                        tracing::info!(mode = mode.key(), "voice.mode saved");
                        self.config.voice.mode = mode;
                        self.voice.set_mode(mode);
                    }
                    Err(e) => {
                        tracing::warn!("cannot save voice.mode: {e}");
                        self.notify("Kai: cannot save voice.mode", e.to_string());
                    }
                }
            }
            Command::NewConversation if self.paired() => self.voice.new_conversation(),
            Command::NewConversation => self.not_paired(),
            Command::OpenInLmgw => self.open_in_lmgw(),
            Command::SetPersonality(id) => self.pick_personality(id).await,
            Command::ShortcutNote(note) => {
                self.model.shortcuts = note;
                self.publish();
            }
            Command::RingNote(note) => {
                self.model.overlay.note = note;
                self.publish();
            }
            Command::Resumed => self.voice.resumed(),
            Command::ScreenLock => self.screen_locked().await,
            Command::Dnd(on) => self.voice.set_dnd(on),
            Command::TryAgain => self.try_again().await,
            Command::Tool(call) => self.on_tool(*call).await,
            Command::CheckForUpdates => match &self.updates {
                Some(tx) if tx.send(crate::update::Request::CheckNow).is_ok() => {}
                _ => tracing::warn!("update: the updater does not run; nothing was checked"),
            },
            Command::Quit | Command::Restart => unreachable!("handled in run"),
        }
    }

    /// Reads the key for the configured `lmgw.url` and starts the link when there is one.
    async fn load(&mut self) {
        let Some(url) = self.config.lmgw.url.clone() else {
            tracing::info!("not paired (no lmgw.url in the configuration)");
            return self.set_pairing(Pairing::NotPaired);
        };
        match self.store.load(&url).await {
            Ok(Lookup::Found(key)) => self.connect(url, key),
            Ok(Lookup::NotFound) => {
                tracing::info!(%url, "not paired (no key in the wallet for this lmgw)");
                self.set_pairing(Pairing::NotPaired);
            }
            Ok(Lookup::Locked) => self.wallet_locked(),
            Err(e) => self.wallet_error(e),
        }
    }

    /// The tray's Try again (§3.3, review KAI-3 K3-6): the device was disabled on lmgw, or its key
    /// expired, and the owner says it may be enabled again or its expiry was moved. The stored key
    /// opens the feed once more; its `hello` is connected, and a device still disabled or a key
    /// still expired answers 401 `device_disabled` or `key_expired` again.
    async fn try_again(&mut self) {
        if !matches!(
            self.model.pairing,
            Pairing::Disabled { .. } | Pairing::Expired { .. }
        ) {
            return;
        }
        tracing::info!("the owner asked to try the stored device key again");
        self.load().await;
    }

    /// The wallet check every `wallet.retry_s`: Kai's own key while the wallet was locked or
    /// failing, and a link that waits for the wallet.
    async fn retry(&mut self) {
        if matches!(
            self.model.pairing,
            Pairing::WalletLocked { .. } | Pairing::WalletError { .. }
        ) {
            self.load().await;
        }
        if self
            .arrival
            .as_ref()
            .is_some_and(|a| a.stage == Stage::Wallet)
        {
            self.offer_arrival().await;
        }
    }

    // ---------------------------------------------------------------- pairing (§3.1, K13)

    /// A link from the scheme handler. Nothing is stored, stopped or switched here: the link waits
    /// for the wallet to open, then for the owner's answer in Kai's dialog. While one waits, a
    /// further link is refused and the waiting one stays as it is (review KAI-2 K2-4; the D-Bus
    /// `Pair` refuses it before it gets here, through the gate).
    async fn arrived(&mut self, link: PairingLink) {
        if let Err(why) = pairing::verify_fingerprint(&link) {
            tracing::warn!("pairing link refused: {why}");
            if self.arrival.is_none() {
                self.gate.release();
            }
            self.model.pair_error = Some(why.clone());
            self.publish();
            self.notify("Kai could not pair", why);
            return;
        }
        if let Some(waiting) = &self.arrival {
            tracing::warn!(
                waiting = %waiting.link.url,
                "a pairing link arrived while another waits: refused ({WAITING})"
            );
            self.notify(
                "Kai could not pair",
                format!(
                    "{WAITING} (for {}): answer that first; this link was not kept",
                    waiting.link.url
                ),
            );
            return;
        }
        self.gate.hold();
        // The caller is not authenticated (any process of the session can hand a link over, any
        // web page can open one): the owner decides.
        tracing::info!(
            url = %link.url,
            device = %link.name,
            "a pairing link arrived; Kai asks the owner to confirm it"
        );
        self.arrivals += 1;
        self.arrival = Some(Arrival {
            id: self.arrivals,
            link,
            stage: Stage::Wallet,
            told_locked: false,
        });
        self.offer_arrival().await;
    }

    /// Moves the waiting link on: with the wallet open, the owner is asked; while it is locked
    /// (or the Secret Service fails) the link waits, and Kai checks again every `wallet.retry_s`.
    async fn offer_arrival(&mut self) {
        let Some(a) = self.arrival.as_ref().filter(|a| a.stage == Stage::Wallet) else {
            return;
        };
        let url = a.link.url.clone();
        match self.store.locked().await {
            Ok(false) => {
                let a = self.arrival.as_mut().expect("checked above");
                a.stage = Stage::Owner;
                let request = ConfirmRequest {
                    id: a.id,
                    facts: PairFacts::new(&a.link, self.config.lmgw.url.as_deref()),
                    #[cfg(feature = "live-test")]
                    live_accept: crate::live_test::accepts(&a.link.url, a.link.key.expose()),
                };
                self.model.link_wait = Some(LinkWait::Owner { url });
                self.ui_send(UiCmd::ConfirmPair(request));
            }
            locked => {
                let why = match locked {
                    Err(e) => {
                        tracing::warn!("the Secret Service failed: {e}; the pairing link waits");
                        format!("the wallet is not available ({e})")
                    }
                    Ok(_) => {
                        tracing::info!("the pairing link waits: the wallet is locked");
                        "KWallet is locked".to_owned()
                    }
                };
                let a = self.arrival.as_mut().expect("checked above");
                if !std::mem::replace(&mut a.told_locked, true) {
                    self.notify(
                        "Kai: a pairing link waits",
                        format!(
                            "{why}: unlock it, and Kai then asks you to confirm the pairing link \
                             for {url}"
                        ),
                    );
                }
                self.model.link_wait = Some(LinkWait::Wallet { url });
                self.schedule_retry();
            }
        }
        self.publish();
    }

    /// The owner's answer in the confirmation dialog. Declining changes nothing.
    async fn confirmed(&mut self, id: u64, accept: bool) {
        let Some(a) = self
            .arrival
            .take_if(|a| a.id == id && a.stage == Stage::Owner)
        else {
            tracing::debug!(id, "an answer for a pairing link that no longer waits");
            return;
        };
        self.model.link_wait = None;
        self.gate.release();
        if !accept {
            tracing::info!(url = %a.link.url, "the owner declined the pairing link; nothing changed");
            self.publish();
            return;
        }
        tracing::info!(url = %a.link.url, "the owner confirmed the pairing link");
        match self.commit(&a.link).await {
            Ok(Committed::Stored { .. }) => self.notify(
                "Kai is paired with lmgw",
                format!("device '{}' at {}", a.link.name, a.link.url),
            ),
            Ok(Committed::WalletLocked) => {
                tracing::info!(
                    "the wallet locked before the link was stored; Kai asks again once it opens"
                );
                self.gate.hold();
                self.arrival = Some(Arrival {
                    stage: Stage::Wallet,
                    told_locked: false,
                    ..a
                });
                self.offer_arrival().await;
            }
            Err(e) => self.pair_failed(e),
        }
    }

    /// The paste dialog's Pair (it showed the facts first). A locked wallet is said in the dialog:
    /// the owner is right there, so nothing waits. The pairing it replaces is checked against what
    /// the dialog showed, here at the commit, where nothing can change it in between (K2-5).
    async fn pasted(&mut self, link: PairingLink, shown: Option<String>) -> Result<(), PasteError> {
        if shown != self.config.lmgw.url {
            tracing::info!(
                shown = ?shown,
                now = ?self.config.lmgw.url,
                "the paste dialog showed another pairing than Kai has now; it shows the facts again"
            );
            return Err(PasteError::Changed {
                current: self.config.lmgw.url.clone(),
            });
        }
        match self.commit(&link).await {
            Ok(Committed::Stored { .. }) => {
                if let Some(a) = self.arrival.take() {
                    tracing::info!(
                        url = %a.link.url,
                        "the owner paired by paste; the link that waited is dropped"
                    );
                    self.model.link_wait = None;
                    self.gate.release();
                    self.ui_send(UiCmd::WithdrawConfirm(a.id));
                    self.publish();
                }
                Ok(())
            }
            Ok(Committed::WalletLocked) => Err(PasteError::Failed(
                "KWallet is locked: unlock it, then press Pair again (nothing was stored)".into(),
            )),
            Err(e) => {
                let msg = e.to_string();
                tracing::warn!("pairing failed: {msg}");
                self.model.pair_error = Some(msg.clone());
                self.publish();
                Err(PasteError::Failed(msg))
            }
        }
    }

    /// Stores a link the owner confirmed (§3.1 step 3) and switches to it. A first pairing turns
    /// "Start with Plasma" on (§13).
    async fn commit(&mut self, link: &PairingLink) -> Result<Committed, PairError> {
        self.model.pair_error = None;
        let current = self.config.lmgw.url.clone();
        let done = pairing::commit(&self.store, &self.paths, current.as_deref(), link).await?;
        if let Committed::Stored { first } = done {
            tracing::info!(url = %link.url, device = %link.name, first, "paired: the device key is stored");
            self.config.lmgw.url = Some(link.url.clone());
            if first {
                self.set_autostart(true);
            }
            // The session bound on the gateway (or under the key) this pairing replaces ends.
            self.voice.leave_gateway().await;
            self.voice.feed(false, Some("connecting".into()));
            // The jobs the old key's link admitted: no link of the new key follows them (WP18).
            if current.is_some() {
                self.jobs.cancel_all("Kai: its pairing was replaced");
            }
            if current.as_deref() != Some(link.url.as_str()) {
                // Another gateway: its folders are not the old one's (§4).
                self.folder_req = None;
                self.profiles_req = None;
                self.personality.reset();
                self.drop_personality_sets();
                self.conversation.reset();
                self.voice.set_folder(None);
                if self.config.conversation.folder_id.is_some() {
                    self.remember_folder(None);
                }
                self.model.conversation = self.conversation_notes();
            }
            self.connect(link.url.clone(), link.key.clone());
            // The owner's decision (2026-10-07): a re-pair to another gateway deletes Kai's own
            // key for the old one. The same URL's item was replaced by the store.
            if let Some(old) = current.filter(|old| *old != link.url) {
                self.forget_key(&old).await;
            }
        }
        Ok(done)
    }

    /// Deletes Kai's own wallet item for `url` (a gateway Kai no longer pairs with). A failure
    /// leaves it, logged and said.
    async fn forget_key(&mut self, url: &str) {
        let why = match self.store.delete(url).await {
            Ok(Removed::Items(n)) => {
                tracing::info!(%url, items = n, "the old gateway's device key is deleted from the wallet");
                return;
            }
            Ok(Removed::Locked) => "the wallet is locked".to_owned(),
            Err(e) => e.to_string(),
        };
        tracing::warn!(%url, "the old gateway's device key stays in the wallet: {why}");
        self.notify(
            "Kai: the old device key stays",
            format!(
                "Kai could not delete the device key it kept for {url} ({why}); delete Kai's item \
                 for {url} in KWallet"
            ),
        );
    }

    fn pair_failed(&mut self, e: PairError) {
        let msg = e.to_string();
        tracing::warn!("pairing failed: {msg}");
        self.model.pair_error = Some(msg.clone());
        self.publish();
        self.notify("Kai could not pair", msg);
    }

    fn ui_send(&self, cmd: UiCmd) {
        if self.ui.try_send(cmd).is_err() {
            tracing::warn!("the GTK thread is gone; a dialog cannot be shown or closed");
        }
    }

    /// Starts the link for a stored key; each start gets its own event channel, so an earlier
    /// run's events never reach the model.
    fn connect(&mut self, url: String, key: DeviceKey) {
        self.link.stop();
        // A question about an earlier key's state no longer applies.
        self.key_ask = None;
        let (tx, rx) = mpsc::unbounded_channel();
        self.link_events = rx;
        self.set_pairing(Pairing::Paired {
            url: url.clone(),
            link: Link::Down {
                note: "connecting".into(),
            },
        });
        let creds = Credentials { url, key };
        self.voice.link_mut().set_credentials(Some(creds.clone()));
        self.link.start(creds.clone(), tx);
        self.host_start(creds);
    }

    async fn on_link_event(&mut self, event: LinkEvent) {
        match event {
            LinkEvent::Revoked {
                reason,
                loss: Some(loss),
            } => return self.revoke(reason, loss).await,
            LinkEvent::Revoked { reason, loss: None } => return self.revoked_unknown(reason),
            LinkEvent::DeviceRevoked { name } => {
                // Not a pairing matter: whatever the link's state, the table answers.
                let n = self.jobs.device_revoked(&name);
                tracing::info!(device = %name, cancelled = n, "lmgw: a device was deleted");
                return;
            }
            LinkEvent::Fact(FeedFact::Resync) => {
                // Folder events may have been missed too (§3.2).
                self.voice.fact(FeedFact::Resync);
                self.ask_profiles();
                let acts = self.conversation.resync();
                return self.apply(acts).await;
            }
            LinkEvent::Fact(fact) => return self.voice.fact(fact),
            LinkEvent::Folder(e) => return self.on_folder_event(e).await,
            LinkEvent::Profile(e) => return self.personality.event(e),
            _ => {}
        }
        let Pairing::Paired { link, .. } = &mut self.model.pairing else {
            return;
        };
        match event {
            LinkEvent::Connected {
                device,
                held,
                hosts_label,
                admin,
            } => {
                if let Some(host) = self.hosting.host_mut() {
                    host.feed_up(&device, hosts_label.as_deref());
                }
                // Kai's own label is no MCP server's: no app view under it.
                crate::widgets::app_bridge::set_hosts_label(hosts_label.as_deref());
                *link = Link::Up {
                    device,
                    held,
                    hosts_label,
                    admin,
                };
                self.voice.feed(true, None);
                self.voice.hold(held);
                // With the folder known (remembered) the voice can start now; else once it is
                // ready (§4).
                if self.voice.folder_known() && std::mem::take(&mut self.start_listening) {
                    tracing::info!("voice.on_at_start: listening on (K4)");
                    self.voice.listen_at_start(&self.audio).await;
                }
                let acts = self.conversation.connected();
                self.apply(acts).await;
                self.ask_profiles();
            }
            LinkEvent::Down { note, dead } => {
                *link = Link::Down { note: note.clone() };
                self.voice.feed(false, Some(note.clone()));
                if dead {
                    // The bound session's socket shares the dead path (K2-6).
                    self.voice.link_dead(note).await;
                }
            }
            LinkEvent::Hold { active } => {
                if let Link::Up { held, .. } = link {
                    *held = active;
                }
                self.voice.hold(active);
            }
            LinkEvent::AdminTools(level) => {
                if let Link::Up { admin, .. } = link
                    && *admin != level
                {
                    tracing::info!(was = ?admin, now = ?level, "lmgw: this device's admin tools moved");
                    *admin = level;
                }
            }
            LinkEvent::Revoked { .. }
            | LinkEvent::DeviceRevoked { .. }
            | LinkEvent::Fact(_)
            | LinkEvent::Folder(_)
            | LinkEvent::Profile(_) => {
                unreachable!("handled above")
            }
        }
        self.show_tools();
        self.publish();
    }

    /// §3.3: shows "pair again" with the server's reason, stops the link and keeps only pairing.
    /// A disabled device instead says to enable it (or pair again), and an expired key to move its
    /// expiry (or pair again), and both offer Try again: the key stays in the wallet and works once
    /// the owner did it (review KAI-3 K3-6; lmgw 7d6d18c for the expiry).
    async fn revoke(&mut self, reason: String, loss: KeyLoss) {
        // The feed and a voice session may both say it (the `revoked` event, a 4003 close).
        if matches!(
            self.model.pairing,
            Pairing::Revoked { .. } | Pairing::Disabled { .. } | Pairing::Expired { .. }
        ) {
            tracing::info!(%reason, "the device key's revocation was said already");
            return;
        }
        tracing::warn!(%reason, ?loss, "the device key opens nothing now");
        self.folder_req = None;
        self.key_ask = None;
        self.unknown_said = None;
        self.voice.feed(false, Some(format!("revoked: {reason}")));
        self.voice.leave_gateway().await;
        self.voice.link_mut().set_credentials(None);
        // The jobs this key's link admitted end (their handlers stopped; lmgw reads them as
        // cancelled, or as abandoned: no link of this key follows them any more).
        self.jobs.cancel_all("Kai: its device key was revoked");
        self.link.stop();
        self.host_stop();
        let (_, closed) = mpsc::unbounded_channel();
        self.link_events = closed;
        match loss {
            KeyLoss::PairAgain => {
                self.notify(
                    "Kai: pair again",
                    format!("this device's key was revoked ({reason}); pair again"),
                );
                self.set_pairing(Pairing::Revoked { reason });
            }
            KeyLoss::Disabled => {
                self.notify(
                    "Kai: this device is disabled",
                    format!("{DISABLED} (lmgw: {reason}); then the tray's \"Try again\""),
                );
                self.set_pairing(Pairing::Disabled { reason });
            }
            KeyLoss::Expired => {
                self.notify(
                    "Kai: this device's key expired",
                    format!("{EXPIRED} (lmgw: {reason}); then the tray's \"Try again\""),
                );
                self.set_pairing(Pairing::Expired { reason });
            }
        }
    }

    /// The feed's `revoked` of a kind this build does not know, or with none (an older gateway's;
    /// §3.3, api §1.6). lmgw's guidance is to say its sentence ([`Core::say_unknown`]); it gives no
    /// step beyond that, so Kai asks lmgw once with the key whether it still opens: `current` for
    /// its folder, or the folder list while it knows none. A 401 decides ([`Core::on_key_answer`]);
    /// meanwhile the feed tries again after its own backoff, which says when in the tooltip.
    fn revoked_unknown(&mut self, reason: String) {
        let Pairing::Paired { link, .. } = &mut self.model.pairing else {
            tracing::info!(%reason, "the feed's revoked event came while not paired; nothing to ask");
            return;
        };
        tracing::warn!(
            %reason,
            "the feed's revoked event is of a kind this build does not know; Kai asks lmgw once \
             whether the key still opens"
        );
        let note = format!(
            "lmgw ended the feed: {reason}; Kai asks lmgw whether this device's key still opens"
        );
        *link = Link::Down { note: note.clone() };
        self.voice.feed(false, Some(note));
        self.say_unknown(&reason);
        let folder = self.conversation.folder().map(|f| f.id);
        let link = self.voice.link_mut();
        self.key_ask = Some(match folder {
            Some(id) => {
                let ask = link.current(id, false);
                Box::pin(async move { ask.await.map(|_| ()) })
            }
            None => {
                let ask = link.folders();
                Box::pin(async move { ask.await.map(|_| ()) })
            }
        });
        self.publish();
    }

    /// A revocation of a kind this build does not know, the feed's or a 4003's: lmgw's sentence is
    /// said once per reason, whichever said it, however often it comes (review KAI-4 K4-7), until a
    /// revocation decides.
    fn say_unknown(&mut self, reason: &str) {
        if self.unknown_said.as_deref() == Some(reason) {
            tracing::info!(%reason, "the unknown revocation's sentence was said already");
            return;
        }
        self.unknown_said = Some(reason.to_owned());
        self.notify(
            "Kai: lmgw ended the connection",
            format!("lmgw: {reason}; Kai asks lmgw whether this device's key still opens"),
        );
    }

    /// The answer to [`Core::revoked_unknown`]'s question. A 401 says what the key's loss is, by
    /// its code (§3.3), and stops the link. Any other answer means lmgw still takes the key, and
    /// no answer leaves it undecided: either way the feed's own try after its backoff goes on, and
    /// its answer (`hello`, a 401, or the same `revoked` met later still) says the rest.
    async fn on_key_answer(&mut self, answer: Result<(), Refusal>) {
        match answer {
            Err(e) if e.key_refused() => {
                tracing::warn!("the key's question answered 401: {e}");
                let loss = KeyLoss::of_code(e.code());
                self.revoke(e.message, loss).await
            }
            Ok(()) => tracing::info!("the key still opens lmgw: the feed tries again"),
            Err(e) => tracing::info!(
                "the key's question answered with no 401 ({e}): the feed tries again"
            ),
        }
    }

    // ---------------------------------------------------------------- the conversation (§4)

    /// A folder event of the feed: Kai's own folder's are followed, its `folder.current` goes on to
    /// the voice.
    async fn on_folder_event(&mut self, e: FolderEvent) {
        let acts = match e {
            FolderEvent::Changed(f) => self.conversation.changed(f),
            FolderEvent::Gone(id) => self.conversation.gone(id),
            FolderEvent::Current {
                folder_id,
                thread_id,
                previous,
                reason,
            } => {
                if self
                    .conversation
                    .folder()
                    .is_some_and(|f| f.id == folder_id)
                {
                    self.voice.fact(FeedFact::FolderCurrent {
                        thread_id,
                        previous,
                        reason,
                    });
                }
                Vec::new()
            }
        };
        self.apply(acts).await;
    }

    async fn on_folder_answer(&mut self, answer: FolderAnswer) {
        let refused = match &answer {
            FolderAnswer::Listed(Err(e)) | FolderAnswer::Created(Err(e)) => e
                .key_refused()
                .then(|| (e.message.clone(), KeyLoss::of_code(e.code()))),
            _ => None,
        };
        if let Some((reason, loss)) = refused {
            return self.revoke(reason, loss).await;
        }
        let acts = match answer {
            FolderAnswer::Listed(r) => self.conversation.listed(r),
            FolderAnswer::Created(r) => self.conversation.created(r),
        };
        self.apply(acts).await;
    }

    /// Carries out what the conversation decided.
    async fn apply(&mut self, acts: Vec<conversation::Action>) {
        use conversation::Action;
        for a in acts {
            match a {
                Action::Lookup => {
                    let f = self.voice.link_mut().folders();
                    self.folder_req = Some(Box::pin(async move { FolderAnswer::Listed(f.await) }));
                }
                Action::LookupAfter(wait) => {
                    // The request goes once the wait is over (the future starts when polled).
                    let f = self.voice.link_mut().folders();
                    self.folder_req = Some(Box::pin(async move {
                        tokio::time::sleep(wait).await;
                        FolderAnswer::Listed(f.await)
                    }));
                }
                Action::Create(name) => {
                    let f = self.voice.link_mut().create_folder(&name);
                    self.folder_req = Some(Box::pin(async move { FolderAnswer::Created(f.await) }));
                }
                Action::Remember(id) => self.remember_folder(id),
                Action::Use(using) => {
                    self.voice.set_folder(using);
                    if using.is_some_and(|u| u.ready) && std::mem::take(&mut self.start_listening) {
                        tracing::info!("voice.on_at_start: listening on (K4), the folder is ready");
                        self.voice.listen_at_start(&self.audio).await;
                    }
                }
                Action::AskCurrent => self.voice.follow(Why::Start),
                Action::Leave(note) => self.voice.leave_conversation(note).await,
                Action::Notify(n) => self.notify(n.summary, n.body),
            }
        }
        self.model.conversation = self.conversation_notes();
        self.show_tools();
        self.publish();
    }

    fn conversation_notes(&self) -> Vec<String> {
        let mut notes = self.conversation.notes();
        if let Some(e) = &self.remember_error {
            notes.push(e.clone());
        }
        notes
    }

    /// Writes `conversation.folder_id` (§4: Kai remembers its folder); `None` removes it. Only a
    /// change is written (review KAI-2 K2-11).
    fn remember_folder(&mut self, id: Option<i64>) {
        if id == self.config.conversation.folder_id && self.remember_error.is_none() {
            return;
        }
        match config::set_int(&self.paths.config_file, "conversation", "folder_id", id) {
            Ok(()) => {
                tracing::info!(?id, "conversation.folder_id saved");
                self.config.conversation.folder_id = id;
                self.remember_error = None;
            }
            Err(e) => {
                tracing::warn!("cannot save conversation.folder_id: {e}");
                self.remember_error = Some(format!("cannot save conversation.folder_id: {e}"));
            }
        }
    }

    /// Open in lmgw (§4, §7 item 5): with `xdg-open`, the folder's settings form while the folder
    /// waits for the owner's steps, else the thread's dashboard page, or the Chat page while no
    /// thread is known.
    fn open_in_lmgw(&mut self) {
        let Some(base) = self.config.lmgw.url.clone().filter(|_| self.paired()) else {
            return self.not_paired();
        };
        let page = match (self.conversation.folder(), self.voice.thread_id()) {
            (Some(f), _) if !self.conversation.ready() || self.host_step().is_some() => {
                DashboardPage::FolderSettings(f.id)
            }
            (_, Some(t)) => DashboardPage::Thread(t),
            _ => DashboardPage::Chat,
        };
        let url = dashboard_url(&base, page);
        tracing::info!(%url, "Open in lmgw");
        #[cfg(test)]
        if let Some(tx) = &self.opened {
            let _ = tx.send(url);
            return;
        }
        #[cfg(feature = "live-test")]
        if crate::live_test::active() {
            tracing::info!(%url, "live test: no browser is opened");
            return;
        }
        // xdg-open hands the URL to the desktop's browser (KIO puts it in its own app scope) and
        // returns; its exit is reaped beside Kai, never waited on.
        match std::process::Command::new("xdg-open")
            .arg(&url)
            .stdin(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                std::thread::spawn(move || match child.wait() {
                    Ok(st) if !st.success() => tracing::warn!("xdg-open {url}: {st}"),
                    Err(e) => tracing::warn!("xdg-open {url}: {e}"),
                    Ok(_) => {}
                });
            }
            Err(e) => {
                tracing::warn!("cannot run xdg-open: {e}");
                self.notify(
                    "Kai: cannot open lmgw",
                    format!("xdg-open {url} did not start: {e}"),
                );
            }
        }
    }

    /// Writes `audio.<key>` (the tray's choices live in the configuration, §2). A failed write
    /// keeps the old choice and shows why.
    fn save_audio_choice(&mut self, key: &str, value: Option<&str>) -> bool {
        match config::set_value(&self.paths.config_file, "audio", key, value) {
            Ok(()) => {
                tracing::info!(key, value, "audio choice saved");
                self.model.audio.error = None;
                true
            }
            Err(e) => {
                tracing::warn!("cannot save audio.{key}: {e}");
                self.model.audio.error = Some(format!("cannot save audio.{key}: {e}"));
                false
            }
        }
    }

    /// The voice session's facts for the ring overlay (§9.3, §9.4): `bound` from the bind until
    /// the session ends, `active` while Kai is not in standby (the ring shows exactly then, K11),
    /// `turn` from `speech_started` or a talk press to the end of the reply's playback, and the
    /// voice state with muted, loading and held. The ring's view exists while `bound` or `active`
    /// holds. Only changes reach the GTK thread.
    pub fn set_ring(&mut self, input: RingInput) {
        if input != self.ring {
            self.ring = input.clone();
            self.overlay(OverlayCmd::Ring(input));
        }
    }

    /// `lmgw.response.timing` for the ring's `setTiming`.
    pub fn set_ring_timing(&mut self, timing: Option<serde_json::Value>) {
        self.overlay(OverlayCmd::Timing(timing));
    }

    fn overlay(&self, cmd: OverlayCmd) {
        if self.ui.try_send(UiCmd::Overlay(cmd)).is_err() {
            tracing::debug!("the GTK thread is gone; the overlay command is dropped");
        }
    }

    fn on_audio_event(&mut self, event: AudioEvent) {
        match event {
            AudioEvent::Devices(devices) => self.model.audio.devices = devices,
            AudioEvent::Echo(status) => {
                self.voice.set_half_duplex(status.half_duplex());
                self.model.audio.echo = Some(status);
            }
            AudioEvent::Link(note) => {
                if note != self.model.audio.link {
                    self.audio_notices.clear();
                }
                self.voice.audio_link(note.is_none(), &self.audio);
                self.model.audio.link = note;
            }
            AudioEvent::Notice(line) => {
                let n = self.audio_notices.entry(line.clone()).or_insert(0);
                *n += 1;
                if *n == 1 {
                    tracing::info!("{line}");
                    self.notify("Kai: audio", line);
                } else {
                    tracing::info!(
                        times = *n,
                        "{line} (notified once until PipeWire's connection changes)"
                    );
                }
                return;
            }
        }
        self.publish();
    }

    fn wallet_locked(&mut self) {
        tracing::info!(
            retry_s = self.config.wallet.retry_s,
            "the wallet is locked; checking again until it opens"
        );
        self.set_pairing(Pairing::WalletLocked {
            retry_s: self.config.wallet.retry_s,
        });
        self.schedule_retry();
    }

    fn wallet_error(&mut self, e: SecretError) {
        tracing::warn!(
            retry_s = self.config.wallet.retry_s,
            "the Secret Service failed: {e}"
        );
        self.set_pairing(Pairing::WalletError {
            message: e.0,
            retry_s: self.config.wallet.retry_s,
        });
        self.schedule_retry();
    }

    fn schedule_retry(&mut self) {
        self.retry_at = Instant::now().checked_add(self.config.wallet.retry());
    }

    fn set_autostart(&mut self, on: bool) {
        match autostart::set(&self.paths.autostart_file, &self.exe, on) {
            Ok(()) => {
                tracing::info!(on, file = %self.paths.autostart_file.display(), "Start with Plasma");
                self.model.autostart = on;
                self.model.autostart_error = None;
            }
            Err(e) => {
                tracing::warn!("Start with Plasma: {e}");
                self.model.autostart_error = Some(e);
            }
        }
        self.publish();
    }

    fn set_pairing(&mut self, pairing: Pairing) {
        self.model.pairing = pairing;
        self.publish();
    }

    /// Publishes the model when it changed (the voice ticks every 20 ms during a session).
    fn publish(&self) {
        self.model_tx.send_if_modified(|m| {
            if *m == self.model {
                false
            } else {
                *m = self.model.clone();
                true
            }
        });
    }

    fn notify(&self, summary: impl Into<String>, body: String) {
        let summary = summary.into();
        #[cfg(test)]
        if let Some(tx) = &self.notices {
            let _ = tx.send(Notice { summary, body });
            return;
        }
        if let Some(bus) = self.bus.clone() {
            let timeout = self.config.dbus.timeout();
            let task =
                tokio::spawn(
                    async move { crate::notify::send(&bus, &summary, &body, timeout).await },
                );
            let mut notifying = self.notifying.lock().expect("never poisoned");
            notifying.retain(|t| !t.is_finished());
            notifying.push(task);
        }
    }

    /// §2's quit: (1) a playing reply gets its truncate, (2) the sessions close (the voice waits for
    /// lmgw's answer at most `voice.close_wait_s`), (3) app AEC restores the default devices
    /// (bounded by `aec.restore_timeout_s`), (4) the overlays go on the GTK thread at
    /// `UiCmd::Quit`, which the supervisor sends after this. Dropping the core drops the model's
    /// sender, which removes the tray item. Notifications still on their way are waited for.
    async fn quit(mut self) {
        tracing::info!("quitting");
        // Running jobs end with Kai, and lmgw is told before the link closes (WP18; all within
        // lmgw.timeout_s, and none while the screen is locked).
        self.jobs.quit();
        self.jobs.sent(self.config.lmgw.timeout()).await;
        self.voice.quit().await;
        for n in self.voice.take_notices() {
            self.notify(n.summary, n.body);
        }
        self.link.stop();
        self.host_stop();
        // Accessibility goes back to what it was before Kai switched it (each call bounded by
        // dbus.timeout_s).
        if let Some(a) = &self.a11y {
            a.shutdown().await;
        }
        self.audio.shutdown(self.config.aec.restore_timeout()).await;
        // The last notifications go out before the runtime drops (each at most dbus.timeout_s).
        let notifying = std::mem::take(&mut *self.notifying.lock().expect("never poisoned"));
        for task in notifying {
            let _ = task.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::pairing::tests::{KEY, good};
    use crate::secret::MemoryStore;

    #[derive(Clone, Default)]
    struct FakeAudio {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl AudioPort for FakeAudio {
        fn set_echo_mode(&self, mode: EchoMode) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("echo {}", mode.key()));
        }
        fn set_input(&self, node: Option<String>) {
            self.calls.lock().unwrap().push(format!("input {node:?}"));
        }
        fn set_output(&self, node: Option<String>) {
            self.calls.lock().unwrap().push(format!("output {node:?}"));
        }
        fn open_playback(&self) -> crate::audio::Player {
            crate::audio::test_support::playback(1000).0
        }
        fn open_capture(&self) -> crate::audio::Capture {
            crate::audio::test_support::capture(1000).0
        }
        async fn shutdown(&mut self, limit: Duration) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("shutdown {}", limit.as_secs()));
        }
    }

    #[derive(Clone, Default)]
    struct FakeLink {
        starts: Arc<Mutex<Vec<String>>>,
        stops: Arc<Mutex<usize>>,
        events: Arc<Mutex<Option<mpsc::UnboundedSender<LinkEvent>>>>,
    }

    impl LmgwLink for FakeLink {
        fn start(&mut self, creds: Credentials, events: mpsc::UnboundedSender<LinkEvent>) {
            assert_eq!(creds.key.expose(), KEY);
            self.starts.lock().unwrap().push(creds.url);
            *self.events.lock().unwrap() = Some(events);
        }
        fn stop(&mut self) {
            *self.stops.lock().unwrap() += 1;
            *self.events.lock().unwrap() = None;
        }
    }

    impl FakeLink {
        fn raise(&self, e: LinkEvent) {
            self.events
                .lock()
                .unwrap()
                .as_ref()
                .expect("link running")
                .send(e)
                .unwrap();
        }
    }

    type FeedUp = (String, Option<String>);

    /// The hosted server's seam: what the core asked of it, and its events.
    #[derive(Clone, Default)]
    struct FakeHost {
        starts: Arc<Mutex<Vec<String>>>,
        stops: Arc<Mutex<usize>>,
        /// Each `feed_up`: the device and the label.
        feed_ups: Arc<Mutex<Vec<FeedUp>>>,
        events: Arc<Mutex<Option<mpsc::UnboundedSender<crate::host::HostEvent>>>>,
        /// Each `set_policy`.
        policies: Arc<Mutex<Vec<crate::host::policy::Policy>>>,
        /// Each `set_screenshots`.
        screenshots: Arc<Mutex<Vec<crate::delegate::settings::Screenshots>>>,
    }

    impl crate::host::ToolHost for FakeHost {
        fn start(
            &mut self,
            creds: Credentials,
            events: mpsc::UnboundedSender<crate::host::HostEvent>,
        ) {
            self.starts.lock().unwrap().push(creds.url);
            *self.events.lock().unwrap() = Some(events);
        }
        fn stop(&mut self) {
            *self.stops.lock().unwrap() += 1;
            *self.events.lock().unwrap() = None;
        }
        fn feed_up(&mut self, device: &str, label: Option<&str>) {
            self.feed_ups
                .lock()
                .unwrap()
                .push((device.to_owned(), label.map(str::to_owned)));
        }
        fn set_policy(&mut self, policy: crate::host::policy::Policy) {
            self.policies.lock().unwrap().push(policy);
        }
        fn set_screenshots(&mut self, seen_by: crate::delegate::settings::Screenshots) {
            self.screenshots.lock().unwrap().push(seen_by);
        }
    }

    impl FakeHost {
        fn raise(&self, e: crate::host::HostEvent) {
            self.events
                .lock()
                .unwrap()
                .as_ref()
                .expect("host running")
                .send(e)
                .unwrap();
        }
    }

    struct Rig {
        store: Arc<MemoryStore>,
        link: FakeLink,
        host: FakeHost,
        audio: FakeAudio,
        audio_tx: mpsc::UnboundedSender<AudioEvent>,
        model: watch::Receiver<TrayModel>,
        cmds: mpsc::UnboundedSender<Command>,
        ui: async_channel::Receiver<UiCmd>,
        notices: mpsc::UnboundedReceiver<Notice>,
        paths: Paths,
        _dir: tempfile::TempDir,
        updates: mpsc::UnboundedReceiver<crate::update::Request>,
        task: tokio::task::JoinHandle<Ended>,
    }

    fn rig(config_text: &str, prepare: impl FnOnce(&MemoryStore)) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under_config_home(dir.path());
        std::fs::create_dir_all(paths.config_file.parent().unwrap()).unwrap();
        std::fs::write(&paths.config_file, config_text).unwrap();
        let config = Config::load(&paths.config_file).unwrap();
        let store = Arc::new(MemoryStore::default());
        prepare(&store);
        let link = FakeLink::default();
        let host = FakeHost::default();
        let audio = FakeAudio::default();
        let (audio_tx, audio_rx) = mpsc::unbounded_channel();
        let (model_tx, model) = watch::channel(TrayModel::starting(false));
        let (ui_tx, ui) = async_channel::unbounded();
        let (cmds, cmd_rx) = mpsc::unbounded_channel();
        let (ntx, notices) = mpsc::unbounded_channel();
        let mut core = Core::new(
            config,
            paths.clone(),
            "/usr/bin/kai".into(),
            store.clone(),
            link.clone(),
            crate::voice::NoVoice,
            (audio.clone(), audio_rx),
            model_tx,
            ui_tx,
            None,
        );
        core.notify_into(ntx);
        core.set_host(Box::new(host.clone()), vec!["policy.tools.nothing".into()]);
        let (updates_tx, updates) = mpsc::unbounded_channel();
        core.set_updates(updates_tx);
        let task = tokio::spawn(core.run(cmd_rx));
        Rig {
            store,
            link,
            host,
            audio,
            audio_tx,
            model,
            cmds,
            ui,
            notices,
            paths,
            _dir: dir,
            updates,
            task,
        }
    }

    impl Rig {
        /// The next notification with `summary` (the ones before it are skipped).
        async fn notice(&mut self, summary: &str) -> Notice {
            loop {
                let n = tokio::time::timeout(Duration::from_secs(60), self.notices.recv())
                    .await
                    .unwrap_or_else(|_| panic!("no notification '{summary}'"))
                    .unwrap();
                if n.summary == summary {
                    return n;
                }
            }
        }

        async fn until(&mut self, f: impl FnMut(&TrayModel) -> bool) -> TrayModel {
            tokio::time::timeout(Duration::from_secs(600), self.model.wait_for(f))
                .await
                .expect("model state")
                .unwrap()
                .clone()
        }
        /// The paste dialog's Pair, the dialog opened just now (it shows Kai's pairing as it is).
        async fn paste(&self, text: &str) -> Result<(), String> {
            self.paste_shown(text, self.config_url())
                .await
                .map_err(|e| e.to_string())
        }
        /// The paste dialog's Pair, the dialog having shown `shown` as what the link replaces.
        async fn paste_shown(&self, text: &str, shown: Option<String>) -> Result<(), PasteError> {
            let (reply, rx) = oneshot::channel();
            let link = PairingLink::parse(text).unwrap();
            self.cmds
                .send(Command::PairPasted { link, shown, reply })
                .unwrap();
            rx.await.unwrap()
        }
        /// A link from the scheme handler.
        fn arrive(&self, text: &str) {
            let link = PairingLink::parse(text).unwrap();
            self.cmds.send(Command::Arrived(link)).unwrap();
        }
        fn answer(&self, id: u64, accept: bool) {
            self.cmds.send(Command::ConfirmPair { id, accept }).unwrap();
        }
        /// The next pairing dialog command for the GTK thread (overlay commands are skipped).
        async fn dialog(&self) -> UiCmd {
            loop {
                let cmd = tokio::time::timeout(Duration::from_secs(600), self.ui.recv())
                    .await
                    .expect("a dialog command")
                    .unwrap();
                if !matches!(cmd, UiCmd::Overlay(_)) {
                    return cmd;
                }
            }
        }
        async fn confirm_request(&self) -> ConfirmRequest {
            match self.dialog().await {
                UiCmd::ConfirmPair(r) => r,
                _ => panic!("not a confirmation"),
            }
        }
        fn no_dialog(&self) {
            while let Ok(cmd) = self.ui.try_recv() {
                assert!(
                    matches!(cmd, UiCmd::Overlay(_)),
                    "an unexpected dialog command"
                );
            }
        }
        /// Waits until the wallet holds exactly `urls` (the old item goes after the switch).
        async fn until_stored(&self, urls: &[&str]) {
            for _ in 0..100 {
                if self.stored_urls() == urls {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(self.stored_urls(), urls);
        }
        fn stored_urls(&self) -> Vec<String> {
            let mut v: Vec<String> = self.store.items.lock().unwrap().keys().cloned().collect();
            v.sort();
            v
        }
        fn config_url(&self) -> Option<String> {
            Config::load(&self.paths.config_file).unwrap().lmgw.url
        }
        async fn quit(self) {
            self.cmds.send(Command::Quit).unwrap();
            assert_eq!(self.task.await.unwrap(), Ended::Quit);
        }
    }

    fn paired(m: &TrayModel) -> bool {
        matches!(m.pairing, Pairing::Paired { .. })
    }

    #[tokio::test(start_paused = true)]
    async fn no_url_is_not_paired() {
        let mut r = rig("", |_| {});
        let m = r.until(|m| m.pairing != Pairing::Checking).await;
        assert_eq!(m.pairing, Pairing::NotPaired);
        r.cmds.send(Command::OpenPairDialog).unwrap();
        assert!(matches!(
            r.ui.recv().await,
            Ok(UiCmd::ShowPairDialog { current: None })
        ));
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_stored_key_starts_the_link_and_its_events_reach_the_model() {
        let mut r = rig("[lmgw]\nurl = \"http://127.0.0.1:1\"\n", |s| {
            s.items.lock().unwrap().insert(
                "http://127.0.0.1:1".into(),
                ("d".into(), DeviceKey::parse(KEY).unwrap()),
            );
        });
        r.until(paired).await;
        assert_eq!(
            *r.link.starts.lock().unwrap(),
            vec!["http://127.0.0.1:1".to_owned()]
        );
        r.link.raise(LinkEvent::Down {
            note: "lmgw not reachable; retry in 2 s".into(),
            dead: false,
        });
        r.until(|m| matches!(&m.pairing, Pairing::Paired { link: Link::Down { note }, .. } if note.contains("retry in 2 s"))).await;
        r.link.raise(LinkEvent::Connected {
            device: "desktop".into(),
            held: false,
            hosts_label: None,
            admin: Default::default(),
        });
        r.until(|m| {
            matches!(
                &m.pairing,
                Pairing::Paired {
                    link: Link::Up { held: false, .. },
                    ..
                }
            )
        })
        .await;
        r.link.raise(LinkEvent::Hold { active: true });
        r.until(|m| {
            matches!(
                &m.pairing,
                Pairing::Paired {
                    link: Link::Up { held: true, .. },
                    ..
                }
            )
        })
        .await;
        // `state` moves the device's admin-tools level; the tray says it unless it is off.
        r.link.raise(LinkEvent::AdminTools(
            crate::lmgw_link::AdminLevel::ReadOnly,
        ));
        r.until(|m| {
            matches!(
                &m.pairing,
                Pairing::Paired {
                    link: Link::Up {
                        admin: crate::lmgw_link::AdminLevel::ReadOnly,
                        ..
                    },
                    ..
                }
            )
        })
        .await;
        let (link, stops) = (r.link.clone(), *r.link.stops.lock().unwrap());
        r.quit().await;
        assert!(*link.stops.lock().unwrap() > stops, "quit closes the link");
    }

    #[tokio::test(start_paused = true)]
    async fn a_locked_wallet_shows_locked_and_recovers_when_it_opens() {
        let mut r = rig(
            "[lmgw]\nurl = \"http://127.0.0.1:1\"\n[wallet]\nretry_s = 3\n",
            |s| {
                s.items.lock().unwrap().insert(
                    "http://127.0.0.1:1".into(),
                    ("d".into(), DeviceKey::parse(KEY).unwrap()),
                );
                s.set_locked(true);
            },
        );
        let m = r.until(|m| m.pairing != Pairing::Checking).await;
        assert_eq!(m.pairing, Pairing::WalletLocked { retry_s: 3 });
        // still locked after a retry
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert!(matches!(
            r.model.borrow().pairing,
            Pairing::WalletLocked { .. }
        ));
        r.store.set_locked(false);
        r.until(paired).await;
        r.quit().await;
    }

    const OLD: &str = "http://127.0.0.1:1";
    const NEW: &str = "http://127.0.0.1:8001";

    /// Kai paired with OLD, its feed up.
    async fn paired_rig() -> Rig {
        let mut r = rig(&format!("[lmgw]\nurl = \"{OLD}\"\n"), |s| {
            s.items
                .lock()
                .unwrap()
                .insert(OLD.into(), ("d".into(), DeviceKey::parse(KEY).unwrap()));
        });
        r.until(paired).await;
        r.link.raise(LinkEvent::Connected {
            device: "desktop".into(),
            held: false,
            hosts_label: None,
            admin: Default::default(),
        });
        r.until(|m| {
            matches!(
                m.pairing,
                Pairing::Paired {
                    link: Link::Up { .. },
                    ..
                }
            )
        })
        .await;
        r
    }

    #[tokio::test(start_paused = true)]
    async fn a_handed_over_link_is_stored_only_once_the_owner_confirms_it() {
        let mut r = paired_rig().await;
        let stops = *r.link.stops.lock().unwrap();
        r.arrive(&good());
        let req = r.confirm_request().await;
        // The dialog shows the new gateway, the device, and the pairing it replaces.
        assert_eq!(req.facts.url, NEW);
        assert_eq!(req.facts.name, "desktop");
        assert_eq!(req.facts.replaces.as_deref(), Some(OLD));
        let m = r.until(|m| m.link_wait.is_some()).await;
        assert_eq!(m.link_wait, Some(LinkWait::Owner { url: NEW.into() }));
        // Nothing is stored, stopped or switched before the answer.
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(r.stored_urls(), [OLD]);
        assert_eq!(r.config_url().as_deref(), Some(OLD));
        assert_eq!(
            *r.link.stops.lock().unwrap(),
            stops,
            "the working link runs on"
        );
        assert_eq!(*r.link.starts.lock().unwrap(), [OLD]);
        assert!(matches!(&r.model.borrow().pairing, Pairing::Paired { url, .. } if url == OLD));
        // The owner confirms.
        r.answer(req.id, true);
        let m = r
            .until(|m| matches!(&m.pairing, Pairing::Paired { url, .. } if url == NEW))
            .await;
        assert_eq!(m.link_wait, None);
        // The owner's decision (2026-10-07): the old gateway's item goes, Kai's own only.
        r.until_stored(&[NEW]).await;
        assert_eq!(r.config_url().as_deref(), Some(NEW));
        assert_eq!(*r.link.starts.lock().unwrap(), [OLD, NEW]);
        assert!(
            !m.autostart,
            "not a first pairing: Start with Plasma is left as it was"
        );
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_re_pair_deletes_kais_item_for_the_old_gateway_and_nothing_else() {
        let r = paired_rig().await;
        let other = "http://127.0.0.1:2";
        r.store
            .items
            .lock()
            .unwrap()
            .insert(other.into(), ("x".into(), DeviceKey::parse(KEY).unwrap()));
        // The same gateway again: its item is replaced, nothing is deleted.
        let same = crate::pairing::tests::link(&format!(
            "v=1&url=http%3A%2F%2F127.0.0.1%3A1&name=again&key={KEY}"
        ));
        assert_eq!(r.paste(&same).await, Ok(()));
        r.until_stored(&[OLD, other]).await;
        assert_eq!(r.store.items.lock().unwrap()[OLD].0, "again");
        // Another gateway: the old one's item goes, the unrelated one stays.
        assert_eq!(r.paste(&good()).await, Ok(()));
        r.until_stored(&[other, NEW]).await;
        assert_eq!(r.config_url().as_deref(), Some(NEW));
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn declining_changes_nothing() {
        let mut r = paired_rig().await;
        let stops = *r.link.stops.lock().unwrap();
        r.arrive(&good());
        let req = r.confirm_request().await;
        r.answer(req.id, false);
        r.until(|m| m.link_wait.is_none()).await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_eq!(r.stored_urls(), [OLD]);
        assert_eq!(r.config_url().as_deref(), Some(OLD));
        assert_eq!(*r.link.stops.lock().unwrap(), stops);
        assert_eq!(*r.link.starts.lock().unwrap(), [OLD]);
        // A late second answer for it does nothing either.
        r.answer(req.id, true);
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(r.stored_urls(), [OLD]);
        r.quit().await;
    }

    /// Review KAI-2 K2-4: while a link waits for the owner, a further one is refused and said;
    /// the open dialog stays as it is (no withdrawal, no new dialog), and its answer still counts.
    #[tokio::test(start_paused = true)]
    async fn a_link_while_another_waits_is_refused_and_nothing_swaps() {
        let mut r = rig("", |_| {});
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        r.arrive(&good());
        let first = r.confirm_request().await;
        assert_eq!(
            first.facts.replaces, None,
            "a first pairing replaces nothing"
        );
        let other = crate::pairing::tests::link(&format!(
            "v=1&url=http%3A%2F%2F192.0.2.20%3A8001&name=other&key={KEY}"
        ));
        r.arrive(&other);
        let n = r.notices.recv().await.unwrap();
        assert_eq!(n.summary, "Kai could not pair");
        assert!(
            n.body.contains(WAITING) && n.body.contains(NEW),
            "names the link that waits: {n:?}"
        );
        r.no_dialog();
        assert_eq!(
            r.model.borrow().link_wait,
            Some(LinkWait::Owner { url: NEW.into() })
        );
        r.answer(first.id, true);
        let m = r.until(paired).await;
        assert_eq!(r.stored_urls(), [NEW]);
        assert!(
            m.autostart,
            "a confirmed first pairing switches Start with Plasma on"
        );
        assert!(autostart::is_on(&r.paths.autostart_file));
        // Answered: the next link is asked about again.
        r.arrive(&other);
        let second = r.confirm_request().await;
        assert!(second.facts.plain_http_remote);
        assert_eq!(second.facts.replaces.as_deref(), Some(NEW));
        r.quit().await;
    }

    /// Review KAI-2 K2-5: the paste dialog said what the link replaces when it opened; a pairing
    /// made meanwhile (a confirmed link) makes the paste refuse, so no key is deleted unnamed. The
    /// dialog then shows the pairing Kai has, and a Pair with it goes through.
    #[tokio::test(start_paused = true)]
    async fn a_paste_against_a_pairing_that_changed_is_refused() {
        let mut r = rig("", |_| {});
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        // The paste dialog opens: nothing to replace.
        let shown = r.config_url();
        assert_eq!(shown, None);
        // Meanwhile a link is confirmed.
        let other = crate::pairing::tests::link(&format!(
            "v=1&url=http%3A%2F%2F192.0.2.20%3A8001&name=other&key={KEY}"
        ));
        r.arrive(&other);
        let req = r.confirm_request().await;
        r.answer(req.id, true);
        r.until_stored(&["http://192.0.2.20:8001"]).await;
        let e = r.paste_shown(&good(), shown).await.unwrap_err();
        assert_eq!(
            e,
            PasteError::Changed {
                current: Some("http://192.0.2.20:8001".into())
            }
        );
        assert!(e.to_string().contains("nothing was stored"), "{e}");
        assert_eq!(
            r.stored_urls(),
            ["http://192.0.2.20:8001"],
            "nothing deleted"
        );
        assert_eq!(r.config_url().as_deref(), Some("http://192.0.2.20:8001"));
        // The dialog shows the facts again against what Kai has; Pair now replaces it, named.
        assert_eq!(
            r.paste_shown(&good(), Some("http://192.0.2.20:8001".into()))
                .await,
            Ok(())
        );
        r.until_stored(&[NEW]).await;
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_link_while_locked_waits_in_memory_and_never_tears_down_the_pairing() {
        let mut r = paired_rig().await;
        let stops = *r.link.stops.lock().unwrap();
        r.store.set_locked(true);
        r.arrive(&good());
        let m = r.until(|m| m.link_wait.is_some()).await;
        assert_eq!(m.link_wait, Some(LinkWait::Wallet { url: NEW.into() }));
        assert!(
            matches!(&m.pairing, Pairing::Paired { url, link: Link::Up { .. } } if url == OLD),
            "the working pairing stays: {:?}",
            m.pairing
        );
        let n = r.notices.recv().await.unwrap();
        assert_eq!(n.summary, "Kai: a pairing link waits");
        assert!(
            n.body.contains("KWallet is locked") && n.body.contains(NEW),
            "{n:?}"
        );
        // Still locked a few checks later: no question, nothing stored, said once.
        tokio::time::sleep(Duration::from_secs(12)).await;
        r.no_dialog();
        assert!(
            r.notices.try_recv().is_err(),
            "the wallet notice is said once"
        );
        assert_eq!(*r.link.stops.lock().unwrap(), stops);
        assert_eq!(r.stored_urls(), [OLD]);
        // The wallet opens: now the owner is asked, and only the answer stores.
        r.store.set_locked(false);
        let req = r.confirm_request().await;
        assert_eq!(req.facts.replaces.as_deref(), Some(OLD));
        assert_eq!(r.stored_urls(), [OLD]);
        r.answer(req.id, true);
        r.until(|m| matches!(&m.pairing, Pairing::Paired { url, .. } if url == NEW))
            .await;
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_link_for_a_kai_whose_wallet_is_locked_at_start_is_asked_once_it_opens() {
        // The handler started Kai with the link; the wallet is locked at login.
        let mut r = rig("", |s| s.set_locked(true));
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        r.arrive(&good());
        r.until(|m| matches!(m.link_wait, Some(LinkWait::Wallet { .. })))
            .await;
        assert!(!r.model.borrow().autostart);
        r.store.set_locked(false);
        let req = r.confirm_request().await;
        assert!(r.stored_urls().is_empty());
        r.answer(req.id, true);
        let m = r.until(paired).await;
        assert!(
            m.autostart,
            "the first pairing switches Start with Plasma on"
        );
        assert_eq!(r.config_url().as_deref(), Some(NEW));
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_paste_is_confirmed_in_its_dialog_and_a_locked_wallet_is_said_there() {
        let mut r = rig("[lmgw]\nurl = \"http://127.0.0.1:1\"\n", |s| {
            s.set_locked(true)
        });
        r.until(|m| matches!(m.pairing, Pairing::WalletLocked { .. }))
            .await;
        r.cmds.send(Command::OpenPairDialog).unwrap();
        assert!(matches!(r.dialog().await,
            UiCmd::ShowPairDialog { current: Some(c) } if c == "http://127.0.0.1:1"));
        let e = r.paste(&good()).await.unwrap_err();
        assert!(
            e.contains("KWallet is locked") && e.contains("nothing was stored"),
            "{e}"
        );
        assert!(r.stored_urls().is_empty());
        assert_eq!(
            r.model.borrow().link_wait,
            None,
            "nothing waits behind the owner's back"
        );
        r.store.set_locked(false);
        assert_eq!(r.paste(&good()).await, Ok(()));
        r.until(|m| matches!(&m.pairing, Pairing::Paired { url, .. } if url == NEW))
            .await;
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_wallet_is_shown_and_retried() {
        let mut r = rig("[lmgw]\nurl = \"http://127.0.0.1:1\"\n", |s| {
            *s.fail.lock().unwrap() = Some(
                "the Secret Service did not answer a lookup within 10 s (wallet.timeout_s)".into(),
            );
        });
        let m = r.until(|m| m.pairing != Pairing::Checking).await;
        assert!(
            matches!(&m.pairing, Pairing::WalletError { message, retry_s: 5 } if message.contains("wallet.timeout_s"))
        );
        *r.store.fail.lock().unwrap() = None;
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn revocation_through_the_seam_says_pair_again_and_a_new_pairing_recovers() {
        let mut r = rig("", |_| {});
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        assert_eq!(r.paste(&good()).await, Ok(()));
        r.until(paired).await;
        let stops = *r.link.stops.lock().unwrap();
        r.link.raise(LinkEvent::Revoked {
            reason: "device 'desktop' was rotated — pair it again".into(),
            loss: Some(KeyLoss::PairAgain),
        });
        let m = r
            .until(|m| matches!(m.pairing, Pairing::Revoked { .. }))
            .await;
        assert_eq!(
            m.pairing,
            Pairing::Revoked {
                reason: "device 'desktop' was rotated — pair it again".into()
            }
        );
        assert_eq!(
            *r.link.stops.lock().unwrap(),
            stops + 1,
            "revocation stops the link"
        );
        assert_eq!(
            crate::tray::model::render(&m).menu,
            vec![
                crate::tray::model::MenuEntry::PairWithLmgw,
                crate::tray::model::MenuEntry::Settings,
                crate::tray::model::MenuEntry::CheckForUpdates,
                crate::tray::model::MenuEntry::Quit
            ]
        );
        assert_eq!(r.paste(&good()).await, Ok(()));
        r.until(paired).await;
        r.quit().await;
    }

    /// Review KAI-3 K3-6: a disabled device says to enable it or pair again, keeps its key and
    /// stops retrying; the tray's Try again opens the link with the stored key once more.
    #[tokio::test(start_paused = true)]
    async fn a_disabled_device_keeps_its_key_and_try_again_reconnects_with_it() {
        let mut r = rig("[lmgw]\nurl = \"http://127.0.0.1:1\"\n", |s| {
            s.items.lock().unwrap().insert(
                "http://127.0.0.1:1".into(),
                ("d".into(), DeviceKey::parse(KEY).unwrap()),
            );
        });
        r.until(paired).await;
        let stops = *r.link.stops.lock().unwrap();
        let words = "device 'desktop' is disabled — enable it on Usage → Keys";
        r.link.raise(LinkEvent::Revoked {
            reason: words.into(),
            loss: Some(KeyLoss::Disabled),
        });
        let m = r
            .until(|m| matches!(m.pairing, Pairing::Disabled { .. }))
            .await;
        assert_eq!(
            m.pairing,
            Pairing::Disabled {
                reason: words.into()
            }
        );
        let n = r.notice("Kai: this device is disabled").await;
        assert!(
            n.body.starts_with(DISABLED) && n.body.contains(words),
            "{}",
            n.body
        );
        assert_eq!(*r.link.stops.lock().unwrap(), stops + 1, "the link stops");
        assert!(
            r.store
                .items
                .lock()
                .unwrap()
                .contains_key("http://127.0.0.1:1"),
            "the key stays in the wallet"
        );
        let view = crate::tray::model::render(&m);
        assert_eq!(view.status, DISABLED);
        assert_eq!(
            view.menu,
            vec![
                crate::tray::model::MenuEntry::TryAgain,
                crate::tray::model::MenuEntry::PairWithLmgw,
                crate::tray::model::MenuEntry::Settings,
                crate::tray::model::MenuEntry::CheckForUpdates,
                crate::tray::model::MenuEntry::Quit
            ]
        );
        // A tray click meanwhile says what to do.
        r.cmds.send(Command::ToggleListening).unwrap();
        let n = r.notice("Kai: this device is disabled").await;
        assert!(n.body.contains("Try again"), "{}", n.body);
        // The owner enabled it: Try again starts the link with the stored key.
        r.cmds.send(Command::TryAgain).unwrap();
        r.until(paired).await;
        assert_eq!(
            *r.link.starts.lock().unwrap(),
            ["http://127.0.0.1:1", "http://127.0.0.1:1"]
        );
        r.link.raise(LinkEvent::Connected {
            device: "desktop".into(),
            held: false,
            hosts_label: None,
            admin: Default::default(),
        });
        r.until(|m| {
            matches!(
                &m.pairing,
                Pairing::Paired {
                    link: Link::Up { .. },
                    ..
                }
            )
        })
        .await;
        // Try again outside the disabled state does nothing.
        r.cmds.send(Command::TryAgain).unwrap();
        r.link.raise(LinkEvent::Hold { active: true });
        r.until(|m| {
            matches!(
                &m.pairing,
                Pairing::Paired {
                    link: Link::Up { held: true, .. },
                    ..
                }
            )
        })
        .await;
        assert_eq!(r.link.starts.lock().unwrap().len(), 2);
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_link_with_fp_is_refused_and_the_pairing_is_unchanged() {
        let mut r = rig("", |_| {});
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        let fp = format!("{}&fp=sha256:{}", good(), "0".repeat(64));
        r.arrive(&fp);
        let m = r.until(|m| m.pair_error.is_some()).await;
        assert!(m.pair_error.unwrap().contains("refusing"));
        assert_eq!(m.pairing, Pairing::NotPaired);
        assert_eq!(m.link_wait, None);
        r.no_dialog();
        assert!(r.store.items.lock().unwrap().is_empty());
        assert!(
            !r.paths.config_file.exists()
                || Config::load(&r.paths.config_file)
                    .unwrap()
                    .lmgw
                    .url
                    .is_none()
        );
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn check_for_updates_goes_to_the_updater() {
        let mut r = rig("", |_| {});
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        r.cmds.send(Command::CheckForUpdates).unwrap();
        assert_eq!(
            r.updates.recv().await,
            Some(crate::update::Request::CheckNow)
        );
        r.quit().await;
    }

    /// The updater's Restart runs the quit sequence as Quit does (the audio's shutdown, which gives
    /// app AEC's default devices back, the link stopped) and ends with `Ended::Restart` for `main`.
    #[tokio::test(start_paused = true)]
    async fn restart_runs_the_quit_sequence_and_ends_as_a_restart() {
        let r = rig("", |_| {});
        r.cmds.send(Command::Restart).unwrap();
        assert_eq!(r.task.await.unwrap(), Ended::Restart);
        assert_eq!(
            r.audio.calls.lock().unwrap().last().map(String::as_str),
            Some("shutdown 4"),
            "aec.restore_timeout_s bounds the shutdown"
        );
        assert!(*r.link.stops.lock().unwrap() >= 1);
    }

    /// A Quit during a Restart's quit sequence wins: Kai ends without the restart.
    #[tokio::test(start_paused = true)]
    async fn a_quit_after_restart_is_a_quit() {
        let r = rig("", |_| {});
        r.cmds.send(Command::Restart).unwrap();
        r.cmds.send(Command::Quit).unwrap();
        assert_eq!(r.task.await.unwrap(), Ended::Quit);
    }

    /// A Restart after the quit began is not one: Kai ends as a quit.
    #[tokio::test(start_paused = true)]
    async fn a_restart_after_quit_is_a_quit() {
        let r = rig("", |_| {});
        r.cmds.send(Command::Quit).unwrap();
        r.cmds.send(Command::Restart).unwrap();
        assert_eq!(r.task.await.unwrap(), Ended::Quit);
    }

    #[tokio::test(start_paused = true)]
    async fn start_with_plasma_toggles_the_autostart_file() {
        let mut r = rig("", |_| {});
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        r.cmds.send(Command::SetAutostart(true)).unwrap();
        r.until(|m| m.autostart).await;
        assert!(autostart::is_on(&r.paths.autostart_file));
        r.cmds.send(Command::SetAutostart(false)).unwrap();
        r.until(|m| !m.autostart).await;
        assert!(!autostart::is_on(&r.paths.autostart_file));
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn tray_audio_choices_are_saved_forwarded_and_shown() {
        let mut r = rig(
            "[lmgw]\nurl = \"http://127.0.0.1:1\"\n[audio]\noutput = \"hdmi\"\n",
            |_| {},
        );
        let m = r.until(|m| m.pairing != Pairing::Checking).await;
        assert_eq!(
            m.audio.output.as_deref(),
            Some("hdmi"),
            "the model starts from the config"
        );
        r.cmds.send(Command::SetEchoMode(EchoMode::App)).unwrap();
        r.until(|m| m.audio.mode == EchoMode::App).await;
        r.cmds
            .send(Command::SetInput(Some("usb mic".into())))
            .unwrap();
        r.until(|m| m.audio.input.is_some()).await;
        r.cmds.send(Command::SetOutput(None)).unwrap();
        r.until(|m| m.audio.output.is_none()).await;
        let c = Config::load(&r.paths.config_file).unwrap();
        assert_eq!(c.audio.echo_mode, EchoMode::App);
        assert_eq!(c.audio.input.as_deref(), Some("usb mic"));
        assert_eq!(c.audio.output, None);
        assert_eq!(
            c.lmgw.url.as_deref(),
            Some("http://127.0.0.1:1"),
            "the rest is kept"
        );
        // The audio thread's reports reach the tray.
        let status = crate::audio::EchoStatus {
            chosen: EchoMode::App,
            effective: EchoMode::HalfDuplex,
            note: Some("echo: Kai's echo cancellation failed: x; half duplex instead".into()),
            offer_half_duplex: false,
        };
        r.audio_tx.send(AudioEvent::Echo(status.clone())).unwrap();
        r.audio_tx
            .send(AudioEvent::Link(Some("PipeWire is not reachable".into())))
            .unwrap();
        let m = r.until(|m| m.audio.link.is_some()).await;
        assert_eq!(m.audio.echo, Some(status));
        let audio = r.audio.clone();
        r.quit().await;
        assert_eq!(
            *audio.calls.lock().unwrap(),
            [
                "echo app",
                "input Some(\"usb mic\")",
                "output None",
                "shutdown 4"
            ],
            "quit ends with the audio thread's restore, bounded by aec.restore_timeout_s"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_audio_notice_is_notified_once_per_pipewire_outage() {
        let mut r = rig("", |_| {});
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        let line = "cannot open the playback stream: not connected to PipeWire";
        r.audio_tx
            .send(AudioEvent::Link(Some("PipeWire went away".into())))
            .unwrap();
        for _ in 0..3 {
            r.audio_tx.send(AudioEvent::Notice(line.into())).unwrap();
        }
        r.audio_tx
            .send(AudioEvent::Notice("another line".into()))
            .unwrap();
        r.audio_tx.send(AudioEvent::Link(None)).unwrap();
        r.audio_tx.send(AudioEvent::Notice(line.into())).unwrap();
        r.until(|m| m.audio.link.is_none()).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut got = Vec::new();
        while let Ok(n) = r.notices.try_recv() {
            got.push(n.body);
        }
        assert_eq!(
            got,
            [line, "another line", line],
            "once per outage, and once more after PipeWire came back"
        );
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn ring_over_fullscreen_is_saved_shown_and_sent_to_the_overlay() {
        let mut r = rig("[lmgw]\nurl = \"http://127.0.0.1:1\"\n", |_| {});
        let m = r.until(|m| m.pairing != Pairing::Checking).await;
        assert!(m.overlay.ring_over_fullscreen, "the default (§16 answer 5)");
        r.cmds.send(Command::SetRingOverFullscreen(false)).unwrap();
        r.until(|m| !m.overlay.ring_over_fullscreen).await;
        assert!(
            !Config::load(&r.paths.config_file)
                .unwrap()
                .overlay
                .ring_over_fullscreen
        );
        assert!(matches!(
            r.ui.recv().await,
            Ok(UiCmd::Overlay(OverlayCmd::RingOverFullscreen(false)))
        ));
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_overlays_ring_note_shows_in_the_tooltip_until_it_is_cleared() {
        let mut r = rig("[lmgw]\nurl = \"http://127.0.0.1:1\"\n", |_| {});
        r.until(|m| m.pairing != Pairing::Checking).await;
        let line = "its web process ended twice, so the ring is down";
        r.cmds
            .send(Command::RingNote(Some(line.to_owned())))
            .unwrap();
        let m = r.until(|m| m.overlay.note.is_some()).await;
        let notes = crate::tray::model::render(&m).notes;
        assert!(notes.contains(&format!("ring: {line}")), "{notes:?}");
        r.cmds.send(Command::RingNote(None)).unwrap();
        r.until(|m| m.overlay.note.is_none()).await;
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_ring_seam_sends_only_changes() {
        let (model_tx, _model) = watch::channel(TrayModel::starting(false));
        let (ui_tx, ui) = async_channel::unbounded();
        let dir = tempfile::tempdir().unwrap();
        let (_audio_tx, audio_rx) = mpsc::unbounded_channel();
        let mut core = Core::new(
            Config::default(),
            Paths::under_config_home(dir.path()),
            "/usr/bin/kai".into(),
            Arc::new(MemoryStore::default()),
            FakeLink::default(),
            crate::voice::NoVoice,
            (FakeAudio::default(), audio_rx),
            model_tx,
            ui_tx,
            None,
        );
        let bound = RingInput {
            bound: true,
            ..RingInput::default()
        };
        core.set_ring(bound.clone());
        core.set_ring(bound.clone());
        core.set_ring(RingInput {
            turn: true,
            ..bound
        });
        let mut got = Vec::new();
        while let Ok(UiCmd::Overlay(OverlayCmd::Ring(i))) = ui.try_recv() {
            got.push((i.bound, i.turn));
        }
        assert_eq!(got, [(true, false), (true, true)]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_choice_that_cannot_be_saved_is_not_applied_and_says_why() {
        let mut r = rig("", |_| {});
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        // A directory where the config file should be: the write fails.
        std::fs::remove_file(&r.paths.config_file).unwrap();
        std::fs::create_dir(&r.paths.config_file).unwrap();
        r.cmds
            .send(Command::SetEchoMode(EchoMode::HalfDuplex))
            .unwrap();
        let m = r.until(|m| m.audio.error.is_some()).await;
        assert!(m.audio.error.unwrap().contains("audio.echo_mode"));
        assert_eq!(m.audio.mode, EchoMode::Device);
        assert!(r.audio.calls.lock().unwrap().is_empty());
        r.quit().await;
    }

    /// PD2: the hosted server starts and stops where the feed does, hears the feed's hello, and
    /// its status is the tooltip's tools line; its revocation goes the feed's path.
    #[tokio::test]
    async fn the_hosted_server_runs_beside_the_feed() {
        use crate::host::{HostEvent, HostStatus};
        let mut r = paired_rig().await;
        assert_eq!(*r.host.starts.lock().unwrap(), [OLD]);
        assert_eq!(
            *r.host.feed_ups.lock().unwrap(),
            [("desktop".to_owned(), None)]
        );
        r.link.raise(LinkEvent::Connected {
            device: "desktop".into(),
            held: false,
            hosts_label: Some("kai".into()),
            admin: Default::default(),
        });
        r.host
            .raise(HostEvent::Status(HostStatus::Linked { tools: 3 }));
        let m = r
            .until(|m| {
                m.tools
                    .first()
                    .is_some_and(|l| l.starts_with("tools: linked"))
            })
            .await;
        assert_eq!(
            m.tools,
            [
                "tools: linked as kai (3 tools)",
                "policy.tools.nothing: Kai offers no such tool; the setting does nothing"
            ]
        );
        assert!(
            crate::tray::model::render(&m)
                .notes
                .contains(&"tools: linked as kai (3 tools)".to_owned())
        );
        r.host.raise(HostEvent::Status(HostStatus::NotGranted {
            retry_s: 30,
            message: "the MCP row of device 'desktop' is switched off".into(),
        }));
        r.until(|m| {
            m.tools.first().is_some_and(|l| {
                l.starts_with(
                    "tools: not granted (the MCP row of device 'desktop' is switched off;",
                )
            })
        })
        .await;

        // PD3: a tool that acts on the core is one command with its answer.
        let (reply, answer) = oneshot::channel();
        r.cmds
            .send(Command::Tool(Box::new(calls::CoreCall {
                area: "ping",
                call: calls::tests::call(&calls::tests::PING.tools[0]),
                reply,
            })))
            .unwrap();
        assert_eq!(
            answer.await.unwrap(),
            crate::host::registry::Reply::text("pong (paired: true)")
        );

        // A 4003 on the tool link: the feed's path, the host stopped with the feed.
        let (feed_stops, host_stops) =
            (*r.link.stops.lock().unwrap(), *r.host.stops.lock().unwrap());
        r.host.raise(HostEvent::Revoked {
            reason: "device_disabled: device 'desktop' was disabled".into(),
            loss: Some(KeyLoss::Disabled),
        });
        let m = r
            .until(|m| matches!(m.pairing, Pairing::Disabled { .. }))
            .await;
        assert!(
            m.tools.iter().all(|l| !l.starts_with("tools:")),
            "{:?}",
            m.tools
        );
        assert_eq!(*r.link.stops.lock().unwrap(), feed_stops + 1);
        assert_eq!(*r.host.stops.lock().unwrap(), host_stops + 1);
        // Try again starts both again.
        r.cmds.send(Command::TryAgain).unwrap();
        r.until(paired).await;
        assert_eq!(*r.host.starts.lock().unwrap(), [OLD, OLD]);
        let host = r.host.clone();
        let stops = *host.stops.lock().unwrap();
        r.quit().await;
        assert!(
            *host.stops.lock().unwrap() > stops,
            "quit closes the tool link"
        );
    }

    /// K30 through the core: a configuration tool writes the owner's file (here a temp one),
    /// keeps its comments, hands the host the new policy, and a refusal writes nothing.
    #[tokio::test]
    async fn the_config_tools_write_the_file_and_reach_the_host() {
        use crate::host::policy::Caller;
        use crate::host::registry::{Args, Block, Call, Provider};

        // The owner's opt-ins: the trust writes, and widening conversation_start, are permitted.
        const FILE: &str = "# mine\n[voice]\ngrace_s = 30 # half a minute\n\
            [policy]\nwiden_trust_by_tools = true\n[policy.widen_by_tools]\n\
            conversation_start = true\n";
        let r = rig(FILE, |_| {});
        let entries = calls::provider(r.cmds.clone(), crate::jobs::Jobs::default()).entries();
        let run = |tool: &str, args: serde_json::Value| {
            let entry = entries.iter().find(|e| e.spec.name == tool).unwrap();
            let call = Call::new(
                entry.spec,
                Args::read(entry.spec, args.as_object().cloned()).unwrap(),
                Caller::Owner,
                tokio_util::sync::CancellationToken::new(),
            );
            (entry.handler)(call)
        };
        let said = |reply: crate::host::registry::Reply| match &reply.blocks[..] {
            [Block::Text(t)] => (t.clone(), reply.error),
            other => panic!("{other:?}"),
        };
        let (text, error) = said(
            run(
                "config_set",
                serde_json::json!({"key": "voice.grace_s", "value": "45"}),
            )
            .await,
        );
        assert!(!error, "{text}");
        assert_eq!(text, "voice.grace_s = 45 (applies at Kai's next start)");
        assert_eq!(
            std::fs::read_to_string(&r.paths.config_file).unwrap(),
            FILE.replace("30", "45")
        );
        assert!(
            r.host.policies.lock().unwrap().is_empty(),
            "no policy change yet"
        );
        let (text, error) = said(
            run(
                "config_set",
                serde_json::json!({"key": "policy.config_tools", "value": "true"}),
            )
            .await,
        );
        assert!(!error, "{text}");
        let (text, error) = said(
            run(
                "tool_state_set",
                serde_json::json!({"tool": "conversation_start", "state": "allow"}),
            )
            .await,
        );
        assert!(!error, "{text}");
        assert_eq!(
            text,
            "policy.tools.conversation_start = \"allow\" (applied now)"
        );
        {
            let policies = r.host.policies.lock().unwrap();
            assert_eq!(policies.len(), 2, "one per policy write");
            let spec = kai_desktop::contract::find("config_get").unwrap().1;
            assert!(policies[0].listed(spec) && policies[1].listed(spec));
        }
        // A tool the owner did not permit stays as it is, naming how to permit it; narrowing and
        // the opt-in key itself: the tools cannot widen.
        let before = std::fs::read_to_string(&r.paths.config_file).unwrap();
        for (tool, args) in [
            (
                "tool_state_set",
                serde_json::json!({"tool": "run_command", "state": "allow"}),
            ),
            (
                "config_set",
                serde_json::json!({"key": "policy.widen_by_tools.run_command", "value": "true"}),
            ),
        ] {
            let (text, error) = said(run(tool, args).await);
            assert!(error && text.contains("widen_by_tools"), "{text}");
        }
        assert_eq!(
            std::fs::read_to_string(&r.paths.config_file).unwrap(),
            before
        );
        let (text, error) = said(
            run(
                "config_set",
                serde_json::json!({"key": "host.keepalive_s", "value": "0"}),
            )
            .await,
        );
        assert!(error && text.contains("host.keepalive_s"), "{text}");
        assert_eq!(
            std::fs::read_to_string(&r.paths.config_file).unwrap(),
            before
        );
        assert_eq!(r.host.policies.lock().unwrap().len(), 2);
    }

    /// WP21b: the window's operations run in the core like the tools' (the file, the host's policy),
    /// answer with the structured response, and a refusal writes nothing; the tray's Settings
    /// reaches the GTK side.
    #[tokio::test]
    async fn the_windows_operations_run_in_the_core() {
        use crate::settings::{Request, Response};

        const FILE: &str = "# mine\n[voice]\ngrace_s = 30\n";
        let r = rig(FILE, |_| {});
        r.cmds.send(Command::OpenSettings).unwrap();
        assert!(matches!(r.ui.recv().await, Ok(UiCmd::ShowSettings)));
        let ask = |request: Request| {
            let (reply, rx) = oneshot::channel();
            r.cmds.send(Command::Setting { request, reply }).unwrap();
            rx
        };
        let state: Request = serde_json::from_value(serde_json::json!(
            {"op": "tool_state", "tool": "run_command", "state": "allow"}
        ))
        .unwrap();
        let answer = ask(state).await.unwrap().unwrap();
        assert!(matches!(answer, Response::Changed { .. }), "{answer:?}");
        assert!(
            std::fs::read_to_string(&r.paths.config_file)
                .unwrap()
                .contains("run_command = \"allow\"")
        );
        assert_eq!(
            r.host.policies.lock().unwrap().len(),
            1,
            "the host has the new policy"
        );
        let before = std::fs::read_to_string(&r.paths.config_file).unwrap();
        let bad: Request = serde_json::from_value(serde_json::json!(
            {"op": "set", "key": "host.keepalive_s", "value": "0"}
        ))
        .unwrap();
        let refusal = ask(bad).await.unwrap().unwrap_err();
        assert!(refusal.0.contains("host.keepalive_s"), "{refusal}");
        assert_eq!(
            std::fs::read_to_string(&r.paths.config_file).unwrap(),
            before
        );
        // Who looks at screenshots reaches the host at once, and only when it changes.
        use crate::delegate::settings::Screenshots;
        let set = |value: &str| -> Request {
            serde_json::from_value(serde_json::json!(
                {"op": "set", "key": "delegate.screenshots", "value": value}
            ))
            .unwrap()
        };
        let answer = ask(set("delegate")).await.unwrap().unwrap();
        assert!(
            matches!(
                answer,
                Response::Changed {
                    effect: crate::settings::Effect::Now,
                    ..
                }
            ),
            "{answer:?}"
        );
        ask(set("delegate")).await.unwrap().unwrap();
        ask(set("main")).await.unwrap().unwrap();
        assert_eq!(
            r.host.screenshots.lock().unwrap().clone(),
            [Screenshots::Delegate, Screenshots::Main]
        );
        assert_eq!(
            r.host.policies.lock().unwrap().len(),
            1,
            "not a policy change"
        );
    }

    /// The configuration window's Desktop page (WP16c): the core looks at and forgets the
    /// RemoteDesktop portal's token in its own state directory, where the portal stores it.
    #[tokio::test]
    async fn the_window_forgets_the_remote_control_permission_in_kais_state_directory() {
        let r = rig("", |_| {});
        let file = crate::desktop::fdo::input::token_file(&r.paths.state_dir);
        let ask = |forget: bool| {
            let (reply, rx) = oneshot::channel();
            r.cmds
                .send(Command::WindowRemoteControl { forget, reply })
                .unwrap();
            rx
        };
        let look = ask(false).await.unwrap().unwrap();
        assert!(!look.stored && look.forgot.is_none(), "{look:?}");
        file.store("restore-token").unwrap();
        assert!(ask(false).await.unwrap().unwrap().stored);
        let forgot = ask(true).await.unwrap().unwrap();
        assert_eq!((forgot.stored, forgot.forgot), (false, Some(true)));
        assert!(!file.0.exists());
    }

    /// WP21b's Personality page: unpaired, the facts and every forwarded request say so and
    /// nothing goes out; paired, a request goes to the voice's link (this rig's has no proxy, so it
    /// answers that), one over `window.proxy_max_mb` is refused first.
    #[tokio::test]
    async fn the_windows_lmgw_requests_need_a_pairing_and_stay_bounded() {
        use crate::lmgw_link::proxy::{Forward, Method, Proxied};

        let ask = |r: &Rig, body: Vec<u8>| {
            let (reply, rx) = oneshot::channel::<Proxied>();
            let forward = Forward {
                method: Method::Post,
                path: "/chat/api/profiles".into(),
                body,
            };
            r.cmds
                .send(Command::WindowProxy { forward, reply })
                .unwrap();
            rx
        };
        let facts = |r: &Rig| {
            let (reply, rx) = oneshot::channel();
            r.cmds.send(Command::WindowConversation { reply }).unwrap();
            rx
        };
        let code = |p: &Proxied| {
            serde_json::from_slice::<serde_json::Value>(&p.body).unwrap()["code"]
                .as_str()
                .unwrap()
                .to_owned()
        };

        let mut r = rig("", |_| {});
        r.until(|m| matches!(m.pairing, Pairing::NotPaired)).await;
        let f = facts(&r).await.unwrap();
        assert!(
            !f.paired && f.why.as_deref() == Some(window::NOT_PAIRED),
            "{f:?}"
        );
        assert_eq!((f.thread_id, f.profile), (None, None));
        let p = ask(&r, b"{}".to_vec()).await.unwrap();
        assert_eq!((p.status, code(&p).as_str()), (503, "not_paired"));

        let r = paired_rig().await;
        let f = facts(&r).await.unwrap();
        assert!(f.paired && f.why.is_none(), "{f:?}");
        assert_eq!(f.gateway.as_deref(), Some(OLD));
        let p = ask(&r, b"{}".to_vec()).await.unwrap();
        assert_eq!((p.status, code(&p).as_str()), (502, "no_proxy"));
        let p = ask(&r, vec![b' '; 64 * 1024 * 1024 + 1]).await.unwrap();
        assert_eq!((p.status, code(&p).as_str()), (413, "body_limit"));
    }
}
