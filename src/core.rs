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
use crate::lmgw_link::tls::{Reach, Tls};
use crate::lmgw_link::{Credentials, DeviceFacts, FolderEvent, KeyLoss, LinkEvent, LmgwLink};
use crate::overlay::{OverlayCmd, RingInput};
use crate::pairing::{
    self, Enrolled, NotStored, PairError, PairFacts, Paired, PairingLink, StoredParts, TlsKey,
    now_ms,
};
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
    /// owner confirmed there. `shown` is the pairing the dialog said the link replaces (its first
    /// URL as the dialog last heard); a pairing that changed meanwhile is refused, so no key is
    /// deleted the owner was not shown (review KAI-2 K2-5). The answer goes back to the dialog once
    /// the enrolment is done.
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
    /// The owner ticked "always allow" in a hosted tool's confirmation and allowed the call
    /// (`crate::host::server::Shared::always_option`): `policy.tools.<tool> = "allow_trusted"`.
    AllowAlways {
        tool: String,
    },
    /// The configuration window's Personality page: what the core knows of Kai's conversation
    /// (`crate::settings::window::profiles::Conversation`).
    WindowConversation {
        reply: oneshot::Sender<crate::settings::window::profiles::Conversation>,
    },
    /// One of the Personality page's profile-editor requests: sent to the paired lmgw over the
    /// device link, lmgw's answer (or the refusal) going back to the scheme handler.
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

/// Which pairing a request went under: the SHA-256 of the device's certificate
/// (`crate::lmgw_link::tls::Tls::identity`).
type Identity = [u8; 32];

/// A request to lmgw on its way, and the pairing it went under: its key refusal answers for that
/// pairing only (the security review's S1: a request of the pairing a same-lmgw re-pair replaced
/// must not revoke the new one).
struct Asked<T> {
    by: Option<Identity>,
    call: Pending<T>,
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

/// Where lmgw's dashboard is for this device, as the feed's last `hello` on this pairing said it
/// (lmgw's device link §7.2): what Open in lmgw opens.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Dashboard {
    /// No `hello` on this pairing yet.
    NotConnected,
    /// lmgw sent none: its dashboard listens on its own machine only, and this device is another.
    NotOffered,
    /// `hello.dashboard_url` as lmgw sent it.
    At(String),
}

/// lmgw's dashboard address as Open in lmgw hands it to `xdg-open`: an `http` or `https` URL with a
/// host and no user info, query or fragment, returned without a trailing slash. The error is the
/// rule.
fn dashboard_base(s: &str) -> Result<String, String> {
    let u = url::Url::parse(s.trim()).map_err(|e| format!("not a URL ({e})"))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err("the scheme must be http or https".into());
    }
    if u.host_str().is_none_or(str::is_empty) {
        return Err("it has no host".into());
    }
    if !u.username().is_empty() || u.password().is_some() || u.query().is_some() {
        return Err("it must not carry a user name, password or query".into());
    }
    if u.fragment().is_some() {
        return Err("it must not carry a fragment (#…)".into());
    }
    Ok(u.as_str().trim_end_matches('/').to_owned())
}

/// The dashboard URL for `page` under lmgw's dashboard address.
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

/// The enrolment's call (`crate::pairing::enrol` of the link and the key it sends, bounded by
/// `lmgw.timeout_s`); the tests give their own.
pub type Enroller = std::sync::Arc<
    dyn Fn(PairingLink, TlsKey) -> Pending<Result<Enrolled, PairError>> + Send + Sync,
>;

/// The key an enrolment sends with a link's token (lmgw's device link §3.3, D8's lost answer): one
/// per token, kept in memory until another link's enrolment replaces it, so trying the same link
/// again (the paste dialog's Pair, the link clicked again) sends the same key. While the token
/// lives lmgw answers that key what it answered it, the same certificate, so an answer lost on the
/// way (a connection that broke, `lmgw.timeout_s` passing) comes back; a new key would meet 410
/// `pairing_token_used` and mark the row contested.
struct EnrolKey {
    /// The token's SHA-256: the token itself is not kept.
    token: [u8; 32],
    key: TlsKey,
}

/// A confirmed link on its way to lmgw (lmgw's device link §3.4): the link, the paste dialog's
/// answer when the paste dialog asked, and the call.
struct Enrolling {
    link: PairingLink,
    reply: Option<oneshot::Sender<Result<(), PasteError>>>,
    call: Pending<Result<Enrolled, PairError>>,
}

/// A pairing Kai runs from memory because part of it is not stored yet (decision 5 as the owner
/// settled it, 2026-10-15; the security review's S4): lmgw enrolled it, so Kai connects with it at
/// once, stores what is missing every `wallet.retry_s`, and says at quit that the pairing is lost
/// when it still could not.
struct Unstored {
    enrolled: Enrolled,
    /// What is stored already (the wallet's item, `device-link.json`).
    done: StoredParts,
    /// Why the rest is not.
    why: NotStored,
    /// What the pairing it replaced leaves (another gateway's key item, the bearer key and
    /// `lmgw.url` from before TLS): deleted once this one is stored, so a restart before then finds
    /// the old pairing as it was.
    old: Option<Paired>,
    v1: Option<String>,
}

/// A link's first URL, which names it in the dialogs and the tooltip.
fn first_url(link: &PairingLink) -> String {
    link.urls.as_slice()[0].clone()
}

/// What a pairing from before TLS says (lmgw's device link §12).
fn before_tls(url: &str) -> String {
    format!(
        "Kai was paired with lmgw at {url} before TLS, which lmgw no longer accepts: pair it again \
         with a new link from lmgw's Devices card"
    )
}

/// What a lapsed certificate says (lmgw's device link §5.3): Kai knows its own date.
fn lapsed(p: &Paired) -> String {
    format!(
        "this device's certificate lapsed on {}; pair it again",
        p.not_after()
    )
}

/// The tooltip's line from the certificate's `notice_after` on (lmgw's device link §5.3); `None`
/// before it.
fn certificate_note(p: &Paired) -> Option<String> {
    (p.credentials.state(now_ms()) != lmgw_client::requests::CredentialState::Valid).then(|| {
        format!(
            "this device's certificate lapses on {}; pair it again before then",
            p.not_after()
        )
    })
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
    folder_req: Option<Asked<FolderAnswer>>,
    /// The one question after the feed's `revoked` of a kind Kai does not know: does the key
    /// still open lmgw (§3.3, [`Core::revoked_unknown`])?
    key_ask: Option<Asked<Result<(), Refusal>>>,
    /// The pairing the link runs now (`None` while none runs): what a request is tagged with
    /// ([`Asked`]).
    identity: Option<Identity>,
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
    /// The stored pairing (`device-link.json`): lmgw's URLs, the certificates, the dates.
    paired: Option<Paired>,
    /// A confirmed link's enrolment on its way (one at a time).
    enrolling: Option<Enrolling>,
    /// The pairing Kai runs from memory while part of it is not stored yet ([`Unstored`]).
    unstored: Option<Unstored>,
    enroller: Enroller,
    /// The key the last link's enrolment sent ([`EnrolKey`]).
    enrol_key: Option<EnrolKey>,
    /// What the link of the pairing that runs connects with (its reach and TLS, shared with the
    /// feed, the voice and the host link): the feed's `hello` refreshes them.
    link_creds: Option<Credentials>,
    /// lmgw's dashboard for this device, from the feed's `hello` ([`Dashboard`]).
    dashboard: Dashboard,
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
        let (timeout, stagger) = (config.lmgw.timeout(), config.lmgw.url_stagger());
        let enroller: Enroller = std::sync::Arc::new(move |link: PairingLink, key: TlsKey| {
            Box::pin(async move {
                pairing::enrol(&link, &key, &pairing::client_text(), timeout, stagger).await
            })
        });
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
            identity: None,
            unknown_said: None,
            remember_error: None,
            paired: None,
            enrolling: None,
            unstored: None,
            enroller,
            enrol_key: None,
            link_creds: None,
            dashboard: Dashboard::NotConnected,
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

    /// The enrolment's call (the tests' own).
    #[cfg(test)]
    pub fn set_enroller(&mut self, enroller: Enroller) {
        self.enroller = enroller;
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
                        Some(f) => f.call.as_mut().await,
                        None => std::future::pending().await,
                    }
                } => {
                    let by = self.folder_req.take().and_then(|f| f.by);
                    self.on_folder_answer(by, answer).await;
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
                done = async {
                    match self.enrolling.as_mut() {
                        Some(e) => e.call.as_mut().await,
                        None => std::future::pending().await,
                    }
                } => {
                    let e = self.enrolling.take().expect("polled above");
                    self.enrolled(e, done).await;
                }
                answer = async {
                    match self.key_ask.as_mut() {
                        Some(f) => f.call.as_mut().await,
                        None => std::future::pending().await,
                    }
                } => {
                    let by = self.key_ask.take().and_then(|f| f.by);
                    self.on_key_answer(by, answer).await;
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
            self.set_ring_timing(Some(Box::new(t)));
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

    /// What a voice command says while Kai has no working pairing: why, as the tray says it.
    fn not_paired(&self) {
        tracing::info!("voice: not paired");
        const PAIR: &str = "the tray's \"Pair with lmgw…\"";
        match &self.model.pairing {
            Pairing::Disabled { .. } => self.notify(
                "Kai: this device is disabled",
                format!("{DISABLED}; then the tray's \"Try again\""),
            ),
            Pairing::Expired { .. } => self.notify(
                "Kai: this device's key expired",
                format!("{EXPIRED}; then the tray's \"Try again\""),
            ),
            Pairing::PairAgain { why } => self.notify("Kai: pair again", format!("{why} ({PAIR})")),
            Pairing::Revoked { reason } => self.notify(
                "Kai: pair again",
                format!("this device's key was revoked ({reason}); pair again ({PAIR})"),
            ),
            Pairing::WalletLocked { .. } => self.notify(
                "Kai: the wallet is locked",
                "unlock KWallet: Kai reads its pairing then".to_owned(),
            ),
            Pairing::WalletError { message, .. } => self.notify(
                "Kai: the wallet is not available",
                format!("{message}; Kai reads its pairing once it is"),
            ),
            _ => self.notify("Kai is not paired", format!("pair with lmgw first: {PAIR}")),
        }
    }

    /// lmgw waits for the owner to confirm this device's code (lmgw's device link §3.5): a voice
    /// command says so instead of "lmgw is not running". `true` when it was said.
    fn says_unconfirmed(&self) -> bool {
        let Pairing::Paired {
            link: Link::Unconfirmed { code, .. },
            ..
        } = &self.model.pairing
        else {
            return false;
        };
        self.notify(
            "Kai waits for your confirmation",
            format!("confirm the code {code} on lmgw's Devices card; Kai connects then"),
        );
        true
    }

    async fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Arrived(link) => self.arrived(link).await,
            Command::ConfirmPair { id, accept } => self.confirmed(id, accept).await,
            Command::PairPasted { link, shown, reply } => self.pasted(link, shown, reply).await,
            Command::OpenPairDialog => self.ui_send(UiCmd::ShowPairDialog {
                current: self.current_url(),
            }),
            Command::OpenSettings => self.ui_send(UiCmd::ShowSettings),
            Command::Setting { request, reply } => {
                let _ = reply.send(self.apply_request(request));
            }
            Command::AllowAlways { tool } => self.allow_always(&tool),
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
                    if !self.says_unconfirmed() {
                        self.voice.talk_press(&self.audio).await;
                    }
                } else {
                    self.voice.talk_release(&self.audio).await;
                }
            }
            Command::ToggleListening if self.paired() => {
                if !self.says_unconfirmed() {
                    self.voice.toggle_listening(&self.audio).await
                }
            }
            Command::SetListening(on) if self.paired() => {
                if !(on && self.says_unconfirmed()) {
                    self.voice.set_listening(on, &self.audio).await
                }
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

    /// The pairing's first URL, which names it in the dialogs (`None`: not paired).
    fn current_url(&self) -> Option<String> {
        self.paired.as_ref().map(|p| p.first_url().to_owned())
    }

    /// Reads the stored pairing and its key, and starts the link when there is one. A pairing whose
    /// certificate lapsed is not tried (lmgw's device link §5.3: Kai knows its own date). Nothing
    /// is loaded while a pairing is on its way (an enrolment, or one the wallet has not taken
    /// yet): the stored one must not connect beside it (the security review's S2).
    async fn load(&mut self) {
        if self.enrolling.is_some() || self.unstored.is_some() {
            tracing::info!("a pairing is on its way: the stored one is not loaded meanwhile");
            return;
        }
        let paired = match Paired::load(&self.paths.device_link_file) {
            Ok(p) => p,
            Err(why) => {
                tracing::warn!("the stored pairing: {why}");
                self.paired = None;
                return self.set_pairing(Pairing::PairAgain { why });
            }
        };
        self.paired = paired.clone();
        let Some(paired) = paired else {
            if let Some(url) = self.config.lmgw.url.clone() {
                tracing::info!(%url, "paired before TLS (lmgw.url): Kai asks to pair again");
                return self.set_pairing(Pairing::PairAgain {
                    why: before_tls(&url),
                });
            }
            tracing::info!("not paired (no device-link.json)");
            return self.set_pairing(Pairing::NotPaired);
        };
        if paired.credentials.state(now_ms()) == lmgw_client::requests::CredentialState::Lapsed {
            tracing::info!(
                not_after = %paired.not_after(),
                "the device's certificate lapsed; Kai asks to pair again"
            );
            return self.set_pairing(Pairing::PairAgain {
                why: lapsed(&paired),
            });
        }
        match self.store.load(&paired.fp).await {
            Ok(Lookup::Found(key)) => self.connect(&paired, &key),
            Ok(Lookup::NotFound) => {
                tracing::info!(url = %paired.first_url(), "not paired (no key in the wallet for this pairing)");
                self.set_pairing(Pairing::NotPaired);
            }
            Ok(Lookup::Locked) => self.wallet_locked(),
            Err(e) => self.wallet_error(e),
        }
    }

    /// The tray's Try again (§3.3, review KAI-3 K3-6): the device was disabled on lmgw, or its key
    /// expired, and the owner says it may be enabled again or its expiry was moved. The stored key
    /// (or the one Kai runs from memory, [`Unstored`]) opens the feed once more; its `hello` is
    /// connected, and a device still disabled or a key still expired answers 401 `device_disabled`
    /// or `key_expired` again. Nothing while an enrolment is on its way (S2).
    async fn try_again(&mut self) {
        if !matches!(
            self.model.pairing,
            Pairing::Disabled { .. } | Pairing::Expired { .. }
        ) {
            return;
        }
        if self.enrolling.is_some() {
            tracing::info!("Try again while Kai enrols: the enrolment decides");
            return;
        }
        if let Some(u) = &self.unstored {
            tracing::info!("the owner asked to try the pairing Kai runs from memory again");
            let (paired, key) = (u.enrolled.paired.clone(), u.enrolled.key.clone());
            return self.connect(&paired, &key);
        }
        tracing::info!("the owner asked to try the stored pairing again");
        self.load().await;
    }

    /// The wallet check every `wallet.retry_s`: what of the pairing Kai runs is not stored yet,
    /// Kai's own key while the wallet was locked or failing, and a link that waits for the wallet.
    async fn retry(&mut self) {
        if self.unstored.is_some() {
            self.store_unstored().await;
        }
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
        if let Some(waiting) = &self.arrival {
            let url = first_url(&waiting.link);
            tracing::warn!(
                waiting = %url,
                "a pairing link arrived while another waits: refused ({WAITING})"
            );
            self.notify(
                "Kai could not pair",
                format!("{WAITING} (for {url}): answer that first; this link was not kept"),
            );
            return;
        }
        self.gate.hold();
        // The caller is not authenticated (any process of the session can hand a link over, any
        // web page can open one): the owner decides.
        tracing::info!(
            url = %first_url(&link),
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
        let url = first_url(&a.link);
        match self.store.locked().await {
            Ok(false) => {
                let current = self.current_url();
                let a = self.arrival.as_mut().expect("checked above");
                a.stage = Stage::Owner;
                let request = ConfirmRequest {
                    id: a.id,
                    facts: PairFacts::new(&a.link, current.as_deref()),
                    #[cfg(feature = "live-test")]
                    live_accept: crate::live_test::accepts(&url, a.link.token.expose()),
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

    /// The owner's answer in the confirmation dialog. Declining changes nothing; confirming enrols.
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
            tracing::info!(url = %first_url(&a.link), "the owner declined the pairing link; nothing changed");
            self.publish();
            return;
        }
        tracing::info!(url = %first_url(&a.link), "the owner confirmed the pairing link");
        self.begin_enrol(a.link, None).await;
    }

    /// The paste dialog's Pair (it showed the facts first). A locked wallet is said in the dialog:
    /// the owner is right there, so nothing waits. The pairing it replaces is checked against what
    /// the dialog showed, here at the commit, where nothing can change it in between (K2-5). The
    /// answer goes back once the enrolment is done.
    async fn pasted(
        &mut self,
        link: PairingLink,
        shown: Option<String>,
        reply: oneshot::Sender<Result<(), PasteError>>,
    ) {
        let current = self.current_url();
        if shown != current {
            tracing::info!(
                shown = ?shown,
                now = ?current,
                "the paste dialog showed another pairing than Kai has now; it shows the facts again"
            );
            let _ = reply.send(Err(PasteError::Changed { current }));
            return;
        }
        let refused = match self.store.locked().await {
            Ok(false) => None,
            Ok(true) => Some(
                "KWallet is locked: unlock it, then press Pair again (nothing was sent to lmgw)"
                    .to_owned(),
            ),
            Err(e) => Some(format!(
                "the wallet is not available ({e}): nothing was sent to lmgw"
            )),
        };
        if let Some(why) = refused {
            let _ = reply.send(Err(PasteError::Failed(why)));
            return;
        }
        if let Some(a) = self.arrival.take() {
            tracing::info!(
                url = %first_url(&a.link),
                "the owner paired by paste; the link that waited is dropped"
            );
            self.model.link_wait = None;
            self.gate.release();
            self.ui_send(UiCmd::WithdrawConfirm(a.id));
        }
        self.begin_enrol(link, Some(reply)).await;
    }

    /// Enrols a link the owner confirmed (lmgw's device link §3.4). The pairing it replaces drops
    /// its connections first (kai §3.1's order: lmgw ends the old key's links when the token is
    /// used, which must not read as a revocation here), and comes back when the enrolment fails.
    async fn begin_enrol(
        &mut self,
        link: PairingLink,
        reply: Option<oneshot::Sender<Result<(), PasteError>>>,
    ) {
        if self.enrolling.is_some() || self.unstored.is_some() {
            let why = match &self.unstored {
                Some(u) => format!(
                    "Kai's pairing with lmgw at {} is not stored yet ({}): Kai pairs again once it \
                     is",
                    u.enrolled.paired.first_url(),
                    u.why
                ),
                None => "Kai is pairing already: wait for it to finish, then try again".to_owned(),
            };
            tracing::info!("a second enrolment while one is on its way: refused");
            match reply {
                Some(r) => {
                    let _ = r.send(Err(PasteError::Failed(why)));
                }
                None => self.notify("Kai could not pair", why),
            }
            return;
        }
        let url = first_url(&link);
        let key = match self.key_for(&link) {
            Ok(key) => key,
            Err(why) => {
                tracing::warn!(%url, "pairing failed: {why}");
                return self.pairing_failed(format!("cannot make the device's key: {why}"), reply);
            }
        };
        self.model.pair_error = None;
        self.model.link_wait = Some(LinkWait::Enrolling { url: url.clone() });
        self.leave_pairing(&format!("paused while Kai pairs with {url}"))
            .await;
        let call = (self.enroller)(link.clone(), key);
        self.enrolling = Some(Enrolling { link, reply, call });
        self.publish();
    }

    /// The key to enrol with `link` ([`EnrolKey`]): the one sent with its token before, else a new
    /// one, kept for the token.
    fn key_for(&mut self, link: &PairingLink) -> Result<TlsKey, String> {
        use sha2::Digest;
        let token: [u8; 32] = sha2::Sha256::digest(link.token.expose().as_bytes()).into();
        if let Some(k) = self.enrol_key.as_ref().filter(|k| k.token == token) {
            tracing::info!(url = %first_url(link), "pairing: this link again; Kai enrols the key it sent with it before");
            return Ok(k.key.clone());
        }
        let key = TlsKey::generate()?;
        tracing::info!(url = %first_url(link), "pairing: enrolling a new key");
        self.enrol_key = Some(EnrolKey {
            token,
            key: key.clone(),
        });
        Ok(key)
    }

    /// Ends what the current pairing runs (the feed, the tool link, the voice session) without
    /// forgetting it. Its requests on their way go too (the folder's, the profiles', the
    /// personality's, the key's question): each went under this pairing, and is asked again at the
    /// next `hello` (the security review's S1).
    async fn leave_pairing(&mut self, note: &str) {
        self.voice.leave_gateway().await;
        self.voice.link_mut().set_credentials(None);
        self.voice.feed(false, Some(note.to_owned()));
        self.link.stop();
        self.host_stop();
        self.link_events = mpsc::unbounded_channel().1;
        self.link_creds = None;
        self.drop_requests();
        if let Pairing::Paired { link, .. } = &mut self.model.pairing {
            *link = Link::Down {
                note: note.to_owned(),
            };
        }
    }

    /// The enrolment's answer: stored and switched to, or said; on any failure the pairing Kai had
    /// connects again.
    async fn enrolled(&mut self, e: Enrolling, done: Result<Enrolled, PairError>) {
        self.model.link_wait = None;
        match done {
            Ok(enrolled) => self.store_enrolled(enrolled, e.reply).await,
            Err(err) => {
                tracing::warn!(url = %first_url(&e.link), "pairing failed: {err}");
                self.pairing_failed(err.to_string(), e.reply);
                self.load().await;
            }
        }
    }

    /// Stores an enrolment (the key in the wallet, the rest in `device-link.json`) and switches to
    /// it. lmgw has enrolled the key and its token is used, so Kai connects with the enrolment
    /// whether or not it could store it: what is missing (a wallet that locked meanwhile or fails,
    /// a write that failed) is stored every `wallet.retry_s` ([`Unstored`]; decision 5 as the owner
    /// settled it, 2026-10-15, and the security review's S4).
    async fn store_enrolled(
        &mut self,
        enrolled: Enrolled,
        reply: Option<oneshot::Sender<Result<(), PasteError>>>,
    ) {
        self.model.link_wait = None;
        let mut done = StoredParts::default();
        let stored = pairing::store(&self.store, &self.paths, &enrolled, &mut done).await;
        if let Some(r) = reply {
            let _ = r.send(Ok(()));
        }
        let (old, v1) = self.switch_to(&enrolled).await;
        match stored {
            Ok(()) => self.forget_old(old, v1).await,
            Err(why) => {
                let url = enrolled.paired.first_url().to_owned();
                tracing::warn!(%url, ?done, "the new pairing is not stored yet: {why}; Kai runs it from memory");
                self.notify(
                    "Kai: the pairing is not stored yet",
                    format!(
                        "Kai is paired with lmgw at {url} and connects, but could not store the \
                         pairing yet ({why}). Kai tries again every {} s; quitting Kai before then \
                         loses the pairing",
                        self.config.wallet.retry_s
                    ),
                );
                self.unstored = Some(Unstored {
                    enrolled,
                    done,
                    why,
                    old,
                    v1,
                });
                self.show_unstored();
                self.schedule_retry();
            }
        }
    }

    /// Stores what of the pairing Kai runs from memory is still missing; once it is all stored,
    /// the pairing it replaced is cleaned up and the tooltip's line goes.
    async fn store_unstored(&mut self) {
        let Some(mut u) = self.unstored.take() else {
            return;
        };
        match pairing::store(&self.store, &self.paths, &u.enrolled, &mut u.done).await {
            Ok(()) => {
                let url = u.enrolled.paired.first_url().to_owned();
                tracing::info!(%url, "the pairing Kai ran from memory is stored now");
                self.notify(
                    "Kai stored its pairing",
                    format!("the pairing with lmgw at {url} is stored now and survives a restart"),
                );
                self.show_unstored();
                self.forget_old(u.old, u.v1).await;
            }
            Err(why) => {
                tracing::info!(done = ?u.done, "the pairing is still not stored: {why}");
                u.why = why;
                self.unstored = Some(u);
                self.show_unstored();
                self.schedule_retry();
            }
        }
    }

    /// The tooltip's line for the pairing Kai runs from memory, or none.
    fn show_unstored(&mut self) {
        self.model.unstored = self.unstored.as_ref().map(|u| {
            format!(
                "the pairing is not stored yet ({}): Kai tries again every {} s; quitting Kai \
                 before then loses it",
                u.why, self.config.wallet.retry_s
            )
        });
        self.publish();
    }

    /// A pairing that did not complete: said in the paste dialog when it asked, else as a
    /// notification; the tooltip says it until the next attempt.
    fn pairing_failed(
        &mut self,
        msg: String,
        reply: Option<oneshot::Sender<Result<(), PasteError>>>,
    ) {
        self.model.pair_error = Some(msg.clone());
        self.publish();
        match reply {
            Some(r) => {
                let _ = r.send(Err(PasteError::Failed(msg)));
            }
            None => self.notify("Kai could not pair", msg),
        }
    }

    /// Switches to an enrolment (§3.1 step 3). A first pairing turns "Start with Plasma" on
    /// (§13). The owner confirms the code on lmgw's Devices card, checking the one it shows against
    /// Kai's or entering Kai's (Kai never confirms itself: lmgw's `device_confirm` is the owner's);
    /// until then lmgw answers the feed `device_unconfirmed`, which the tray shows with the code.
    /// Returns what the pairing it replaced leaves, for [`Core::forget_old`] once the new one is
    /// stored: another gateway's pairing, and `lmgw.url` from before TLS.
    async fn switch_to(&mut self, enrolled: &Enrolled) -> (Option<Paired>, Option<String>) {
        let p = &enrolled.paired;
        let old = self.paired.replace(p.clone());
        let v1 = self.config.lmgw.url.clone();
        let first = old.is_none() && v1.is_none();
        tracing::info!(urls = ?p.credentials.urls.as_slice(), device = %p.name, first, "paired with the new key");
        if first {
            self.set_autostart(true);
        }
        self.voice.feed(false, Some("connecting".into()));
        // The jobs the old pairing's link admitted: no link of the new one follows them (WP18).
        if old.is_some() || v1.is_some() {
            self.jobs.cancel_all("Kai: its pairing was replaced");
        }
        if old.as_ref().is_none_or(|o| o.fp != p.fp) {
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
        self.connect(p, &enrolled.key);
        self.notify(
            "Kai is paired with lmgw",
            format!(
                "device '{}' at {}: confirm the code {code} on lmgw's Devices card (check that \
                 the card shows {code}, or enter it where the card asks for it); Kai connects then",
                p.name,
                p.first_url(),
                code = p.credentials.confirm_code
            ),
        );
        (old.filter(|o| o.fp != p.fp), v1)
    }

    /// What the replaced pairing leaves goes once the new one is stored. The owner's decision
    /// (2026-10-07): a re-pair to another gateway deletes Kai's own key for the old one (the same
    /// gateway's item was replaced by the store); and the pairing from before TLS goes.
    async fn forget_old(&mut self, old: Option<Paired>, v1: Option<String>) {
        if let Some(old) = old {
            self.forget_key(&old).await;
        }
        if let Some(url) = v1 {
            self.forget_v1(&url).await;
        }
    }

    /// Deletes Kai's own wallet item for `old` (a gateway Kai no longer pairs with). A failure
    /// leaves it, logged and said.
    async fn forget_key(&mut self, old: &Paired) {
        let url = old.first_url();
        let why = match self.store.delete(&old.fp).await {
            Ok(Removed::Items(n)) => {
                tracing::info!(%url, items = n, "the old gateway's key is deleted from the wallet");
                return;
            }
            Ok(Removed::Locked) => "the wallet is locked".to_owned(),
            Err(e) => e.to_string(),
        };
        tracing::warn!(%url, "the old gateway's key stays in the wallet: {why}");
        self.notify(
            "Kai: the old key stays",
            format!(
                "Kai could not delete the key it kept for lmgw at {url} ({why}); delete Kai's item \
                 for {} in KWallet",
                old.fp
            ),
        );
    }

    /// The pairing from before TLS (lmgw's device link §12) goes once Kai is paired again: its
    /// bearer key's wallet item and `lmgw.url`. A failure leaves them, logged and said.
    async fn forget_v1(&mut self, url: &str) {
        let why = match self.store.delete_v1(url).await {
            Ok(Removed::Items(n)) => {
                tracing::info!(%url, items = n, "the bearer key from before TLS is deleted from the wallet");
                None
            }
            Ok(Removed::Locked) => Some("the wallet is locked".to_owned()),
            Err(e) => Some(e.to_string()),
        };
        if let Some(why) = why {
            tracing::warn!(%url, "the bearer key from before TLS stays in the wallet: {why}");
            self.notify(
                "Kai: the old device key stays",
                format!(
                    "Kai could not delete the device key it kept for {url} before TLS ({why}); \
                     delete Kai's item for {url} in KWallet"
                ),
            );
        }
        match config::set_value(&self.paths.config_file, "lmgw", "url", None) {
            Ok(()) => self.config.lmgw.url = None,
            Err(e) => tracing::warn!("cannot remove lmgw.url from the configuration: {e}"),
        }
    }

    fn ui_send(&self, cmd: UiCmd) {
        if self.ui.try_send(cmd).is_err() {
            tracing::warn!("the GTK thread is gone; a dialog cannot be shown or closed");
        }
    }

    /// The requests the pairing that ends had on their way (S1): nothing waits for their answers,
    /// and the conversation asks again at the next `hello`.
    fn drop_requests(&mut self) {
        self.identity = None;
        self.key_ask = None;
        self.folder_req = None;
        self.profiles_req = None;
        self.drop_personality_sets();
        self.conversation.dropped();
    }

    /// Starts the link for a stored pairing and its key; each start gets its own event channel, so
    /// an earlier run's events never reach the model.
    fn connect(&mut self, paired: &Paired, key: &TlsKey) {
        self.link.stop();
        // A question about an earlier pairing's state no longer applies.
        self.key_ask = None;
        self.identity = None;
        let (tx, rx) = mpsc::unbounded_channel();
        self.link_events = rx;
        let tls = match Tls::paired(&paired.credentials, key, self.config.lmgw.timeout()) {
            Ok(t) => t,
            Err(why) => {
                tracing::warn!("the stored pairing does not connect: {why}");
                return self.set_pairing(Pairing::PairAgain {
                    why: format!("{why}: pair again"),
                });
            }
        };
        let creds = Credentials {
            reach: Reach::new(paired.credentials.urls.clone()),
            tls,
        };
        self.link_creds = Some(creds.clone());
        self.dashboard = Dashboard::NotConnected;
        self.identity = Some(*creds.tls.identity());
        self.model.certificate = certificate_note(paired);
        self.set_pairing(Pairing::Paired {
            url: paired.first_url().to_owned(),
            link: Link::Down {
                note: "connecting".into(),
            },
        });
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
            LinkEvent::Device(facts) => return self.device_facts(facts),
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
                self.model.certificate = self.paired.as_ref().and_then(certificate_note);
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
            LinkEvent::Unconfirmed { note } => {
                let code = self
                    .paired
                    .as_ref()
                    .map(|p| p.credentials.confirm_code.clone())
                    .unwrap_or_default();
                *link = Link::Unconfirmed {
                    code: code.clone(),
                    note,
                };
                self.voice.feed(
                    false,
                    Some(format!(
                        "waiting for you to confirm {code} on lmgw's Devices card"
                    )),
                );
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
            | LinkEvent::Device(_)
            | LinkEvent::Fact(_)
            | LinkEvent::Folder(_)
            | LinkEvent::Profile(_) => {
                unreachable!("handled above")
            }
        }
        self.show_tools();
        self.publish();
    }

    /// What the feed's `hello` says about this device (lmgw's device link §7.2): its URLs replace
    /// the pairing's list and the link's reach, its certificate's dates the stored ones (the
    /// notice and the lapse go by them), both kept in `device-link.json` (a pairing Kai runs from
    /// memory stores them with the rest); its dashboard address is what Open in lmgw opens.
    fn device_facts(&mut self, facts: DeviceFacts) {
        self.dashboard = match facts.dashboard {
            Some(url) => Dashboard::At(url),
            None => Dashboard::NotOffered,
        };
        let Some(mut p) = self.paired.clone() else {
            return;
        };
        let mut changed = false;
        if let Some(urls) = facts.urls
            && urls != p.credentials.urls
        {
            tracing::info!(was = ?p.credentials.urls.as_slice(), now = ?urls.as_slice(), "lmgw's hello: this device's URLs changed");
            if let Some(c) = &self.link_creds {
                c.reach.set_urls(urls.clone());
            }
            p.credentials.urls = urls;
            changed = true;
        }
        if let Some(d) = facts.cert
            && d != p.credentials.dates()
        {
            tracing::info!(
                not_after = %pairing::date(d.not_after_ms),
                notice_after = %pairing::date(d.notice_after_ms),
                "lmgw's hello: this device's certificate dates changed"
            );
            if let Some(c) = &self.link_creds {
                c.tls.set_not_after_ms(d.not_after_ms);
            }
            p.credentials.not_after_ms = d.not_after_ms;
            p.credentials.notice_after_ms = d.notice_after_ms;
            changed = true;
        }
        if !changed {
            return;
        }
        self.model.certificate = certificate_note(&p);
        if let Pairing::Paired { url, .. } = &mut self.model.pairing {
            *url = p.first_url().to_owned();
        }
        // A pairing Kai runs from memory stores the new facts with the rest; its file, once
        // written, is written again.
        let write = match &mut self.unstored {
            Some(u) => {
                u.enrolled.paired = p.clone();
                u.done.file
            }
            None => true,
        };
        if write && let Err(why) = p.save(&self.paths.device_link_file) {
            tracing::warn!("lmgw's hello: cannot keep this device's URLs and dates: {why}");
            self.notify(
                "Kai: cannot keep lmgw's new facts",
                format!(
                    "lmgw sent new addresses or certificate dates for this device, which Kai uses \
                     now but could not write ({why}); the next start uses the old ones until \
                     lmgw sends them again"
                ),
            );
        }
        self.paired = Some(p);
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
            Pairing::Revoked { .. }
                | Pairing::Disabled { .. }
                | Pairing::Expired { .. }
                | Pairing::PairAgain { .. }
        ) {
            tracing::info!(%reason, "the device key's revocation was said already");
            return;
        }
        tracing::warn!(%reason, ?loss, "the device key opens nothing now");
        self.drop_requests();
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
            KeyLoss::Lapsed => {
                // Kai's own date where it has one; lmgw's words otherwise.
                let why = self.paired.as_ref().map_or(reason, lapsed);
                self.notify("Kai: pair again", why.clone());
                self.set_pairing(Pairing::PairAgain { why });
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
        let call: Pending<Result<(), Refusal>> = match folder {
            Some(id) => {
                let ask = link.current(id, false);
                Box::pin(async move { ask.await.map(|_| ()) })
            }
            None => {
                let ask = link.folders();
                Box::pin(async move { ask.await.map(|_| ()) })
            }
        };
        self.key_ask = Some(Asked {
            by: self.identity,
            call,
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
    async fn on_key_answer(&mut self, by: Option<Identity>, answer: Result<(), Refusal>) {
        match answer {
            Err(e) if e.key_refused() => {
                tracing::warn!("the key's question answered 401: {e}");
                let loss = KeyLoss::of_code(e.code());
                self.key_refused(by, e.message, loss).await
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

    /// A request's key refusal revokes the pairing it went under, and only while that pairing is
    /// the one the link runs (S1): one of an earlier pairing's is logged and dropped.
    async fn key_refused(&mut self, by: Option<Identity>, reason: String, loss: KeyLoss) {
        if by.is_none() || by != self.identity {
            tracing::info!(
                %reason,
                ?loss,
                "a key refusal of a request an earlier pairing sent; the pairing Kai runs now stays"
            );
            return;
        }
        self.revoke(reason, loss).await
    }

    async fn on_folder_answer(&mut self, by: Option<Identity>, answer: FolderAnswer) {
        let refused = match &answer {
            FolderAnswer::Listed(Err(e)) | FolderAnswer::Created(Err(e)) => e
                .key_refused()
                .then(|| (e.message.clone(), KeyLoss::of_code(e.code()))),
            _ => None,
        };
        if let Some((reason, loss)) = refused {
            return self.key_refused(by, reason, loss).await;
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
                    self.folder_req = Some(Asked {
                        by: self.identity,
                        call: Box::pin(async move { FolderAnswer::Listed(f.await) }),
                    });
                }
                Action::LookupAfter(wait) => {
                    // The request goes once the wait is over (the future starts when polled).
                    let f = self.voice.link_mut().folders();
                    self.folder_req = Some(Asked {
                        by: self.identity,
                        call: Box::pin(async move {
                            tokio::time::sleep(wait).await;
                            FolderAnswer::Listed(f.await)
                        }),
                    });
                }
                Action::Create(name) => {
                    let f = self.voice.link_mut().create_folder(&name);
                    self.folder_req = Some(Asked {
                        by: self.identity,
                        call: Box::pin(async move { FolderAnswer::Created(f.await) }),
                    });
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
    /// thread is known, on the dashboard address lmgw sent in the feed's `hello` (lmgw's device
    /// link §7.2): the device link's URLs are lmgw's device listener, which serves no dashboard.
    fn open_in_lmgw(&mut self) {
        if !self.paired() {
            return self.not_paired();
        }
        let base = match &self.dashboard {
            Dashboard::At(url) => match dashboard_base(url) {
                Ok(base) => base,
                Err(rule) => {
                    tracing::warn!(%url, %rule, "Open in lmgw: lmgw's dashboard address does not read");
                    return self.notify(
                        "Kai: Open in lmgw",
                        format!(
                            "lmgw's dashboard address {url:?} does not read as a web address \
                             ({rule}): Kai opens nothing"
                        ),
                    );
                }
            },
            Dashboard::NotOffered => {
                tracing::info!("Open in lmgw: lmgw sent no dashboard address for this device");
                return self.notify(
                    "Kai: lmgw's dashboard is not reachable from here",
                    "lmgw's dashboard is only reachable on lmgw's own machine: open it there"
                        .to_owned(),
                );
            }
            Dashboard::NotConnected => {
                tracing::info!("Open in lmgw: no hello on this pairing yet");
                return self.notify(
                    "Kai: Open in lmgw waits for lmgw",
                    "Kai has not connected to lmgw yet: lmgw says where its dashboard is when Kai \
                     connects"
                        .to_owned(),
                );
            }
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
    pub fn set_ring_timing(&mut self, timing: Option<Box<lmgw_client::realtime::VoiceTiming>>) {
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
        // A pairing Kai ran from memory: one last try to store it, else the owner hears it is
        // lost (decision 5 as settled 2026-10-15).
        if self.unstored.is_some() {
            self.store_unstored().await;
        }
        if let Some(u) = &self.unstored {
            let url = u.enrolled.paired.first_url();
            tracing::warn!(%url, "Kai quits with its pairing not stored ({}): it is lost", u.why);
            self.notify(
                "Kai: the pairing is lost",
                format!(
                    "Kai quit before it could store its pairing with lmgw at {url} ({}): pair it \
                     again with a new link from lmgw's Devices card",
                    u.why
                ),
            );
        }
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
    use crate::lmgw_link::test_pki::{FOREIGN, PKI, fp};
    use crate::pairing::tests::{enroller, good_at, link_for, other_at, store_pairing};
    use crate::secret::MemoryStore;
    use crate::voice::link as voice_link;

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
        /// The last start's credentials (their reach and TLS are shared with the core's).
        creds: Arc<Mutex<Option<Credentials>>>,
    }

    impl LmgwLink for FakeLink {
        fn start(&mut self, creds: Credentials, events: mpsc::UnboundedSender<LinkEvent>) {
            assert_ne!(creds.tls.identity(), &[0; 32], "a paired device's TLS");
            self.starts.lock().unwrap().push(creds.url());
            *self.creds.lock().unwrap() = Some(creds.clone());
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
            self.starts.lock().unwrap().push(creds.url());
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

    fn rig(config_text: &str, prepare: impl FnOnce(&MemoryStore, &Paths)) -> Rig {
        rig_enrolling(config_text, prepare, enroller())
    }

    /// [`rig`] whose enrolment is `enrol`.
    fn rig_enrolling(
        config_text: &str,
        prepare: impl FnOnce(&MemoryStore, &Paths),
        enrol: Enroller,
    ) -> Rig {
        rig_with(config_text, prepare, enrol, crate::voice::NoVoice)
    }

    /// [`rig_enrolling`] whose voice link is `voice`.
    fn rig_with<V: VoiceLink>(
        config_text: &str,
        prepare: impl FnOnce(&MemoryStore, &Paths),
        enrol: Enroller,
        voice: V,
    ) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under_config_home(dir.path());
        std::fs::create_dir_all(paths.config_file.parent().unwrap()).unwrap();
        std::fs::write(&paths.config_file, config_text).unwrap();
        let config = Config::load(&paths.config_file).unwrap();
        let store = Arc::new(MemoryStore::default());
        prepare(&store, &paths);
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
            voice,
            (audio.clone(), audio_rx),
            model_tx,
            ui_tx,
            None,
        );
        core.set_enroller(enrol);
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
            self.paste_shown(text, self.paired_url())
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
        /// Waits until the wallet holds the keys of exactly `gateways` (by fingerprint; the old
        /// item goes after the switch).
        async fn until_stored(&self, gateways: &[&str]) {
            let mut want: Vec<String> = gateways.iter().map(|g| (*g).to_owned()).collect();
            want.sort();
            for _ in 0..100 {
                if self.stored() == want {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(self.stored(), want);
        }
        /// The gateways whose keys the wallet holds, by fingerprint, sorted.
        fn stored(&self) -> Vec<String> {
            let mut v: Vec<String> = self.store.items.lock().unwrap().keys().cloned().collect();
            v.sort();
            v
        }
        /// The stored pairing's first URL.
        fn paired_url(&self) -> Option<String> {
            Paired::load(&self.paths.device_link_file)
                .unwrap()
                .map(|p| p.first_url().to_owned())
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
        let mut r = rig("", |_, _| {});
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
        let mut r = rig("", paired_at(OLD));
        r.until(paired).await;
        assert_eq!(*r.link.starts.lock().unwrap(), vec![OLD.to_owned()]);
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
        let mut r = rig("[wallet]\nretry_s = 3\n", |s, p| {
            paired_at(OLD)(s, p);
            s.set_locked(true);
        });
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

    /// The gateway Kai is paired with ([`PKI`]'s) and another one ([`FOREIGN`]'s).
    const OLD: &str = "https://127.0.0.1:1";
    const NEW: &str = "https://127.0.0.1:8002";
    /// Where lmgw's `hello.urls` moves a pairing.
    const MOVED: &str = "https://127.0.0.1:8003";

    fn old_fp() -> String {
        fp(&PKI.ca).to_string()
    }

    fn new_fp() -> String {
        fp(&FOREIGN.ca).to_string()
    }

    /// A pairing link for NEW.
    fn good() -> String {
        other_at(NEW)
    }

    /// A Kai paired with OLD at its start.
    fn paired_at(url: &'static str) -> impl FnOnce(&MemoryStore, &Paths) {
        move |s, p| {
            store_pairing(&PKI, p, s, url);
        }
    }

    /// Kai paired with OLD, its feed up.
    async fn paired_rig() -> Rig {
        let mut r = rig("", paired_at(OLD));
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
        assert_eq!(req.facts.urls, [NEW]);
        assert_eq!(req.facts.name, "desktop");
        assert_eq!(req.facts.replaces.as_deref(), Some(OLD));
        let m = r.until(|m| m.link_wait.is_some()).await;
        assert_eq!(m.link_wait, Some(LinkWait::Owner { url: NEW.into() }));
        // Nothing is stored, stopped or switched before the answer.
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(r.stored(), [old_fp()]);
        assert_eq!(r.paired_url().as_deref(), Some(OLD));
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
        r.until_stored(&[&new_fp()]).await;
        assert_eq!(r.paired_url().as_deref(), Some(NEW));
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
        let other = "sha256:other";
        r.store
            .items
            .lock()
            .unwrap()
            .insert(other.into(), ("x".into(), PKI.device_key.clone()));
        // The same gateway again: its item is replaced, nothing is deleted.
        let same = link_for(&fp(&PKI.ca), OLD, "again");
        assert_eq!(r.paste(&same).await, Ok(()));
        r.until_stored(&[&old_fp(), other]).await;
        assert!(
            r.store.items.lock().unwrap()[&old_fp()]
                .0
                .contains("'again'"),
            "the new key's item"
        );
        // Another gateway: the old one's item goes, the unrelated one stays.
        assert_eq!(r.paste(&good()).await, Ok(()));
        r.until_stored(&[other, &new_fp()]).await;
        assert_eq!(r.paired_url().as_deref(), Some(NEW));
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
        assert_eq!(r.stored(), [old_fp()]);
        assert_eq!(r.paired_url().as_deref(), Some(OLD));
        assert_eq!(*r.link.stops.lock().unwrap(), stops);
        assert_eq!(*r.link.starts.lock().unwrap(), [OLD]);
        // A late second answer for it does nothing either.
        r.answer(req.id, true);
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(r.stored(), [old_fp()]);
        r.quit().await;
    }

    /// Review KAI-2 K2-4: while a link waits for the owner, a further one is refused and said;
    /// the open dialog stays as it is (no withdrawal, no new dialog), and its answer still counts.
    #[tokio::test(start_paused = true)]
    async fn a_link_while_another_waits_is_refused_and_nothing_swaps() {
        let mut r = rig("", |_, _| {});
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        r.arrive(&good());
        let first = r.confirm_request().await;
        assert_eq!(
            first.facts.replaces, None,
            "a first pairing replaces nothing"
        );
        let other = link_for(&fp(&PKI.ca), "https://192.0.2.20:8002", "other");
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
        assert_eq!(r.stored(), [new_fp()]);
        assert!(
            m.autostart,
            "a confirmed first pairing switches Start with Plasma on"
        );
        assert!(autostart::is_on(&r.paths.autostart_file));
        // Answered: the next link is asked about again.
        r.arrive(&other);
        let second = r.confirm_request().await;
        assert_eq!(second.facts.replaces.as_deref(), Some(NEW));
        r.quit().await;
    }

    /// Review KAI-2 K2-5: the paste dialog said what the link replaces when it opened; a pairing
    /// made meanwhile (a confirmed link) makes the paste refuse, so no key is deleted unnamed. The
    /// dialog then shows the pairing Kai has, and a Pair with it goes through.
    #[tokio::test(start_paused = true)]
    async fn a_paste_against_a_pairing_that_changed_is_refused() {
        let mut r = rig("", |_, _| {});
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        // The paste dialog opens: nothing to replace.
        let shown = r.paired_url();
        assert_eq!(shown, None);
        // Meanwhile a link is confirmed.
        let elsewhere = "https://192.0.2.20:8002";
        r.arrive(&good_at(elsewhere));
        let req = r.confirm_request().await;
        r.answer(req.id, true);
        r.until_stored(&[&old_fp()]).await;
        let e = r.paste_shown(&good(), shown).await.unwrap_err();
        assert_eq!(
            e,
            PasteError::Changed {
                current: Some(elsewhere.into())
            }
        );
        assert!(e.to_string().contains("nothing was stored"), "{e}");
        assert_eq!(r.stored(), [old_fp()], "nothing deleted");
        assert_eq!(r.paired_url().as_deref(), Some(elsewhere));
        // The dialog shows the facts again against what Kai has; Pair now replaces it, named.
        assert_eq!(r.paste_shown(&good(), Some(elsewhere.into())).await, Ok(()));
        r.until_stored(&[&new_fp()]).await;
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
        assert_eq!(r.stored(), [old_fp()]);
        // The wallet opens: now the owner is asked, and only the answer stores.
        r.store.set_locked(false);
        let req = r.confirm_request().await;
        assert_eq!(req.facts.replaces.as_deref(), Some(OLD));
        assert_eq!(r.stored(), [old_fp()]);
        r.answer(req.id, true);
        r.until(|m| matches!(&m.pairing, Pairing::Paired { url, .. } if url == NEW))
            .await;
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_link_for_a_kai_whose_wallet_is_locked_at_start_is_asked_once_it_opens() {
        // The handler started Kai with the link; the wallet is locked at login.
        let mut r = rig("", |s, _| s.set_locked(true));
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        r.arrive(&good());
        r.until(|m| matches!(m.link_wait, Some(LinkWait::Wallet { .. })))
            .await;
        assert!(!r.model.borrow().autostart);
        r.store.set_locked(false);
        let req = r.confirm_request().await;
        assert!(r.stored().is_empty());
        r.answer(req.id, true);
        let m = r.until(paired).await;
        assert!(
            m.autostart,
            "the first pairing switches Start with Plasma on"
        );
        assert_eq!(r.paired_url().as_deref(), Some(NEW));
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_paste_is_confirmed_in_its_dialog_and_a_locked_wallet_is_said_there() {
        // A pairing whose key is not in the wallet (yet): the wallet is locked.
        let mut r = rig("", |s, p| {
            PKI.paired(&[OLD]).save(&p.device_link_file).unwrap();
            s.set_locked(true)
        });
        r.until(|m| matches!(m.pairing, Pairing::WalletLocked { .. }))
            .await;
        r.cmds.send(Command::OpenPairDialog).unwrap();
        assert!(matches!(r.dialog().await,
            UiCmd::ShowPairDialog { current: Some(c) } if c == OLD));
        let e = r.paste(&good()).await.unwrap_err();
        assert!(
            e.contains("KWallet is locked") && e.contains("nothing was sent to lmgw"),
            "{e}"
        );
        assert!(r.stored().is_empty());
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
        let mut r = rig("", |s, p| {
            PKI.paired(&[OLD]).save(&p.device_link_file).unwrap();
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
        let mut r = rig("", |_, _| {});
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        assert_eq!(r.paste(&good()).await, Ok(()));
        r.until(paired).await;
        let stops = *r.link.stops.lock().unwrap();
        r.link.raise(LinkEvent::Revoked {
            reason: "the key of device 'desktop' was replaced — pair it again".into(),
            loss: Some(KeyLoss::PairAgain),
        });
        let m = r
            .until(|m| matches!(m.pairing, Pairing::Revoked { .. }))
            .await;
        assert_eq!(
            m.pairing,
            Pairing::Revoked {
                reason: "the key of device 'desktop' was replaced — pair it again".into()
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
        let mut r = rig("", paired_at(OLD));
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
            r.store.items.lock().unwrap().contains_key(&old_fp()),
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
        assert_eq!(*r.link.starts.lock().unwrap(), [OLD, OLD]);
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

    /// lmgw's device link §3.5: the new pairing's code is said at once, and the tray shows it
    /// while lmgw waits for the owner's Confirm; once confirmed, the feed comes up.
    #[tokio::test(start_paused = true)]
    async fn the_code_is_said_and_shown_until_the_owner_confirms() {
        let mut r = rig("", |_, _| {});
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        assert_eq!(r.paste(&good()).await, Ok(()));
        let code = crate::pairing::tests::enrolled_for(&PairingLink::parse(&good()).unwrap())
            .paired
            .credentials
            .confirm_code;
        let n = r.notice("Kai is paired with lmgw").await;
        assert!(
            n.body
                .contains(&format!("confirm the code {code} on lmgw's Devices card")),
            "{}",
            n.body
        );
        r.until(paired).await;
        r.link.raise(LinkEvent::Unconfirmed {
            note: "Kai asks again in 1 s".into(),
        });
        let m = r
            .until(|m| {
                matches!(
                    &m.pairing,
                    Pairing::Paired {
                        link: Link::Unconfirmed { .. },
                        ..
                    }
                )
            })
            .await;
        assert_eq!(
            m.pairing,
            Pairing::Paired {
                url: NEW.into(),
                link: Link::Unconfirmed {
                    code: code.clone(),
                    note: "Kai asks again in 1 s".into(),
                },
            }
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
        r.quit().await;
    }

    /// While lmgw waits for the owner's Confirm, a talk press says so (not "lmgw is not running").
    #[tokio::test(start_paused = true)]
    async fn a_press_while_unconfirmed_says_to_confirm() {
        let mut r = rig("", paired_at(OLD));
        r.until(paired).await;
        r.link.raise(LinkEvent::Unconfirmed {
            note: "Kai asks again in 1 s".into(),
        });
        r.until(|m| {
            matches!(
                &m.pairing,
                Pairing::Paired {
                    link: Link::Unconfirmed { .. },
                    ..
                }
            )
        })
        .await;
        r.cmds.send(Command::Talk(true)).unwrap();
        r.cmds.send(Command::Talk(false)).unwrap();
        let n = r.notice("Kai waits for your confirmation").await;
        let code = PKI.paired(&[OLD]).credentials.confirm_code;
        assert!(n.body.contains(&code), "{}", n.body);
        // The tray's Start listening says the same.
        r.cmds.send(Command::SetListening(true)).unwrap();
        let n = r.notice("Kai waits for your confirmation").await;
        assert!(n.body.contains(&code), "{}", n.body);
        r.quit().await;
    }

    /// A voice command while the pairing must be made again says why, not "not paired".
    #[tokio::test(start_paused = true)]
    async fn a_press_while_kai_must_pair_again_says_why() {
        let lapsed = |s: &MemoryStore, p: &Paths| {
            let mut paired = store_pairing(&PKI, p, s, OLD);
            paired.credentials.not_after_ms = 86_400_000;
            paired.save(&p.device_link_file).unwrap();
        };
        let mut r = rig("", lapsed);
        r.until(|m| matches!(m.pairing, Pairing::PairAgain { .. }))
            .await;
        r.cmds.send(Command::Talk(true)).unwrap();
        let n = r.notice("Kai: pair again").await;
        assert!(
            n.body.contains("lapsed on 1970-01-02") && n.body.contains("Pair with lmgw"),
            "{}",
            n.body
        );
        r.quit().await;
    }

    /// The pairing Kai has drops its connections before the enrolment (kai §3.1's order), and comes
    /// back when the enrolment fails, which is said.
    #[tokio::test(start_paused = true)]
    async fn a_failed_enrolment_is_said_and_the_pairing_comes_back() {
        let refused = "lmgw refused the enrolment: the token expired (pairing_token_expired)";
        let fails: Enroller = std::sync::Arc::new(move |_, _| {
            Box::pin(async move { Err(PairError::Enrol(refused.into())) })
        });
        let mut r = rig_enrolling("", paired_at(OLD), fails);
        r.until(paired).await;
        let stops = *r.link.stops.lock().unwrap();
        r.arrive(&good());
        let req = r.confirm_request().await;
        r.answer(req.id, true);
        let n = r.notice("Kai could not pair").await;
        assert_eq!(n.body, refused);
        let m = r
            .until(|m| matches!(&m.pairing, Pairing::Paired { url, .. } if url == OLD))
            .await;
        assert_eq!(m.pair_error.as_deref(), Some(refused));
        assert!(
            *r.link.stops.lock().unwrap() > stops,
            "the old pairing's link stopped for the enrolment"
        );
        assert_eq!(*r.link.starts.lock().unwrap(), [OLD, OLD], "and came back");
        assert_eq!(r.stored(), [old_fp()]);
        assert_eq!(r.paired_url().as_deref(), Some(OLD));
        r.quit().await;
    }

    /// A voice link whose folder lookups wait for the test's answer: each lookup's sender, in the
    /// order they were asked. Everything else as [`crate::voice::NoVoice`].
    type FolderReply = oneshot::Sender<Result<Vec<FolderInfo>, Refusal>>;

    #[derive(Clone, Default)]
    struct HeldFolders {
        asked: Arc<Mutex<Vec<FolderReply>>>,
    }

    impl VoiceLink for HeldFolders {
        fn set_credentials(&mut self, _creds: Option<Credentials>) {}
        fn current(
            &mut self,
            folder: i64,
            new: bool,
        ) -> Pending<Result<voice_link::Current, Refusal>> {
            crate::voice::NoVoice.current(folder, new)
        }
        fn folders(&mut self) -> Pending<Result<Vec<FolderInfo>, Refusal>> {
            let (tx, rx) = oneshot::channel();
            self.asked.lock().unwrap().push(tx);
            Box::pin(async move {
                rx.await
                    .unwrap_or_else(|_| Err(Refusal::no_answer("the test dropped it")))
            })
        }
        fn create_folder(&mut self, name: &str) -> Pending<Result<FolderInfo, Refusal>> {
            crate::voice::NoVoice.create_folder(name)
        }
        fn open(
            &mut self,
            thread: i64,
            take_over: bool,
            events: mpsc::UnboundedSender<voice_link::SessionEvent>,
        ) -> Box<dyn voice_link::VoiceSocket> {
            crate::voice::NoVoice.open(thread, take_over, events)
        }
    }

    impl HeldFolders {
        /// How many lookups were asked so far.
        fn count(&self) -> usize {
            self.asked.lock().unwrap().len()
        }
        /// Answers lookup `n`; `false` when nothing waits for it any more.
        fn answer(&self, n: usize, r: Result<Vec<FolderInfo>, Refusal>) -> bool {
            let tx = std::mem::replace(&mut self.asked.lock().unwrap()[n], oneshot::channel().0);
            tx.send(r).is_ok()
        }
    }

    /// lmgw's 401 for a key it does not know (a re-paired device's old key).
    fn key_unknown() -> Refusal {
        Refusal {
            status: 401,
            code: Some("device_key_unknown".into()),
            message: "this device's key is unknown".into(),
        }
    }

    /// An enrolment as a real re-pair makes it: a new key, so a new certificate (another
    /// identity), for the link's gateway.
    fn fresh_enroller() -> Enroller {
        Arc::new(|link: PairingLink, key: TlsKey| {
            Box::pin(async move {
                let mut e = crate::pairing::tests::enrolled_for(&link);
                let spki = key.public_key().unwrap();
                e.paired.credentials.certificate =
                    crate::lmgw_link::test_pki::device_cert(&PKI.ca, &spki, (2020, 2100));
                e.paired.credentials.confirm_code = lmgw_client::requests::confirm_code(&spki);
                e.key = key;
                Ok(e)
            })
        })
    }

    /// An enrolment that waits for the test's go.
    fn held_enroller() -> (Enroller, Arc<tokio::sync::Notify>) {
        let go = Arc::new(tokio::sync::Notify::new());
        let g = go.clone();
        let e: Enroller = Arc::new(move |link: PairingLink, _key: TlsKey| {
            let g = g.clone();
            Box::pin(async move {
                g.notified().await;
                Ok(crate::pairing::tests::enrolled_for(&link))
            })
        });
        (e, go)
    }

    /// An enrolment that notes the public half of each key it is given, its first answer lost.
    fn noting_enroller() -> (Enroller, Arc<std::sync::Mutex<Vec<Vec<u8>>>>) {
        let keys = Arc::new(std::sync::Mutex::new(Vec::new()));
        let k = keys.clone();
        let e: Enroller = Arc::new(move |link: PairingLink, key: TlsKey| {
            let k = k.clone();
            Box::pin(async move {
                let first = {
                    let mut k = k.lock().unwrap();
                    k.push(key.public_key().unwrap());
                    k.len() == 1
                };
                if first {
                    return Err(PairError::Enrol(
                        "lmgw's answer to the enrolment broke off".into(),
                    ));
                }
                Ok(crate::pairing::tests::enrolled_for(&link))
            })
        });
        (e, keys)
    }

    /// D8's lost answer (lmgw's device link §3.3): the first enrolment's answer did not arrive,
    /// and the owner presses Pair again: Kai sends the key it sent with that token, which lmgw
    /// answers with the same certificate; a link with another token gets a new key.
    #[tokio::test(start_paused = true)]
    async fn trying_a_link_again_sends_the_key_sent_with_its_token() {
        let (enrol, keys) = noting_enroller();
        let mut r = rig_enrolling("", |_, _| {}, enrol);
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        let e = r.paste(&good()).await.unwrap_err();
        assert!(e.contains("broke off"), "{e}");
        assert_eq!(r.paste(&good()).await, Ok(()));
        let other = crate::pairing::tests::link_with_token(
            &fp(&PKI.ca),
            NEW,
            "desktop",
            "lmgw-enrol-fedcba9876543210fedcba9876543210",
        );
        r.until(paired).await;
        assert_eq!(r.paste(&other).await, Ok(()));
        let keys = keys.lock().unwrap().clone();
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[0], keys[1], "the same token, the same key");
        assert_ne!(keys[1], keys[2], "another token, a new key");
        r.quit().await;
    }

    /// lmgw's device link §7.2: the feed's `hello` says what lmgw holds about the device. Its URLs
    /// replace the pairing's list (`device-link.json`, the tooltip's URL, the link's reach) and its
    /// certificate's dates the stored ones (the notice, the lapse by Kai's date); a `hello`
    /// without them keeps what Kai has.
    #[tokio::test(start_paused = true)]
    async fn hellos_urls_and_dates_replace_the_stored_ones() {
        let mut r = rig("", paired_at(OLD));
        r.until(paired).await;
        let before = std::fs::read_to_string(&r.paths.device_link_file).unwrap();
        r.link.raise(LinkEvent::Device(DeviceFacts {
            urls: None,
            cert: None,
            dashboard: None,
        }));
        r.link.raise(connected());
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
        assert_eq!(
            std::fs::read_to_string(&r.paths.device_link_file).unwrap(),
            before,
            "nothing new, nothing written"
        );
        let urls: lmgw_client::pairing::Urls =
            vec![MOVED.to_owned(), OLD.to_owned()].try_into().unwrap();
        let dates = lmgw_client::requests::CertDates {
            not_after_ms: 1_900_000_000_000,
            notice_after_ms: 1_000,
        };
        r.link.raise(LinkEvent::Device(DeviceFacts {
            urls: Some(urls.clone()),
            cert: Some(dates),
            dashboard: None,
        }));
        let m = r
            .until(|m| matches!(&m.pairing, Pairing::Paired { url, .. } if url == MOVED))
            .await;
        assert_eq!(
            m.certificate.as_deref(),
            Some("this device's certificate lapses on 2030-03-17; pair it again before then"),
            "the notice from lmgw's dates"
        );
        let stored = Paired::load(&r.paths.device_link_file).unwrap().unwrap();
        assert_eq!(stored.credentials.urls, urls);
        assert_eq!(stored.credentials.dates(), dates);
        let creds = r.link.creds.lock().unwrap().clone().unwrap();
        assert_eq!(creds.reach.urls(), [MOVED, OLD], "the link's reach");
        assert_eq!(creds.url(), MOVED);
        assert_eq!(creds.tls.not_after_ms(), Some(1_900_000_000_000));
        assert!(
            creds.tls.lapsed(1_900_000_000_000),
            "the lapse by lmgw's date"
        );
        r.quit().await;
    }

    /// Open in lmgw before the feed's first `hello` on a pairing: lmgw has not said where its
    /// dashboard is yet, and Kai says so (lmgw's device link §7.2).
    #[tokio::test(start_paused = true)]
    async fn open_in_lmgw_before_the_first_hello_says_kai_waits_for_lmgw() {
        let mut r = rig("", paired_at(OLD));
        r.until(paired).await;
        r.cmds.send(Command::OpenInLmgw).unwrap();
        let n = r.notice("Kai: Open in lmgw waits for lmgw").await;
        assert!(
            n.body.contains("has not connected to lmgw yet"),
            "{}",
            n.body
        );
        r.quit().await;
    }

    fn connected() -> LinkEvent {
        LinkEvent::Connected {
            device: "desktop".into(),
            held: false,
            hosts_label: None,
            admin: Default::default(),
        }
    }

    /// The security review's S1: a folder lookup the old pairing sent, answered 401 after a
    /// re-pair to the same lmgw, never revokes the new pairing; the new pairing looks its folder up
    /// again at its `hello`, and its own 401 does revoke.
    #[tokio::test(start_paused = true)]
    async fn a_same_lmgw_re_pair_with_a_request_in_flight_keeps_the_new_pairing() {
        let voice = HeldFolders::default();
        let mut r = rig_with("", paired_at(OLD), fresh_enroller(), voice.clone());
        r.until(paired).await;
        r.link.raise(connected());
        for _ in 0..100 {
            if voice.count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(voice.count(), 1, "the old pairing's lookup is on its way");
        // The same lmgw again, with a new key.
        let same = link_for(&fp(&PKI.ca), OLD, "again");
        assert_eq!(r.paste(&same).await, Ok(()));
        r.until(|m| {
            matches!(
                &m.pairing,
                Pairing::Paired {
                    link: Link::Down { .. },
                    ..
                }
            )
        })
        .await;
        assert_eq!(*r.link.starts.lock().unwrap(), [OLD, OLD]);
        // lmgw ended the old key: its lookup's 401 reaches no one.
        assert!(
            !voice.answer(0, Err(key_unknown())),
            "the old pairing's lookup was dropped with it"
        );
        r.link.raise(connected());
        for _ in 0..100 {
            if voice.count() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(voice.count(), 2, "the new pairing looks its folder up");
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(
            matches!(r.model.borrow().pairing, Pairing::Paired { .. }),
            "{:?}",
            r.model.borrow().pairing
        );
        // The new pairing's own refusal decides.
        assert!(voice.answer(1, Err(key_unknown())));
        r.until(|m| matches!(m.pairing, Pairing::Revoked { .. }))
            .await;
        r.quit().await;
    }

    /// S1's second line: a key refusal counts only for the pairing it went under.
    #[tokio::test(start_paused = true)]
    async fn a_key_refusal_answers_for_the_pairing_it_went_under() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under_config_home(dir.path());
        let store = Arc::new(MemoryStore::default());
        store_pairing(&PKI, &paths, &store, OLD);
        let (_audio_tx, audio_rx) = mpsc::unbounded_channel();
        let (model_tx, _model) = watch::channel(TrayModel::starting(false));
        let (ui_tx, _ui) = async_channel::unbounded();
        let (ntx, _notices) = mpsc::unbounded_channel();
        let mut core = Core::new(
            Config::default(),
            paths,
            "/usr/bin/kai".into(),
            store,
            FakeLink::default(),
            crate::voice::NoVoice,
            (FakeAudio::default(), audio_rx),
            model_tx,
            ui_tx,
            None,
        );
        core.notify_into(ntx);
        core.load().await;
        let now = core.identity;
        assert!(now.is_some(), "connected");
        core.key_refused(Some([7; 32]), "an old key".into(), KeyLoss::PairAgain)
            .await;
        core.key_refused(None, "no pairing".into(), KeyLoss::PairAgain)
            .await;
        assert!(matches!(core.model.pairing, Pairing::Paired { .. }));
        core.key_refused(now, "this key".into(), KeyLoss::PairAgain)
            .await;
        assert!(matches!(core.model.pairing, Pairing::Revoked { .. }));
    }

    /// The security review's S2: while an enrolment is on its way, the wallet's retry does not
    /// load the stored pairing beside it.
    #[tokio::test(start_paused = true)]
    async fn the_wallets_retry_loads_nothing_while_kai_enrols() {
        let (enrol, go) = held_enroller();
        let mut r = rig_enrolling(
            "[wallet]\nretry_s = 3\n",
            |s, p| {
                paired_at(OLD)(s, p);
                s.set_locked(true);
            },
            enrol,
        );
        r.until(|m| matches!(m.pairing, Pairing::WalletLocked { .. }))
            .await;
        // The owner unlocks and pastes before the next check.
        r.store.set_locked(false);
        let cmds = r.cmds.clone();
        let link = PairingLink::parse(&good()).unwrap();
        let (reply, answer) = oneshot::channel();
        cmds.send(Command::PairPasted {
            link,
            shown: Some(OLD.into()),
            reply,
        })
        .unwrap();
        r.until(|m| matches!(m.link_wait, Some(LinkWait::Enrolling { .. })))
            .await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(
            r.link.starts.lock().unwrap().is_empty(),
            "the stored pairing connected during the enrolment"
        );
        go.notify_one();
        assert_eq!(answer.await.unwrap(), Ok(()));
        r.until(|m| matches!(&m.pairing, Pairing::Paired { url, .. } if url == NEW))
            .await;
        assert_eq!(*r.link.starts.lock().unwrap(), [NEW]);
        r.quit().await;
    }

    /// S2: Try again on a disabled device does nothing while an enrolment is on its way.
    #[tokio::test(start_paused = true)]
    async fn try_again_loads_nothing_while_kai_enrols() {
        let (enrol, go) = held_enroller();
        let mut r = rig_enrolling("", paired_at(OLD), enrol);
        r.until(paired).await;
        r.link.raise(LinkEvent::Revoked {
            reason: "disabled".into(),
            loss: Some(KeyLoss::Disabled),
        });
        r.until(|m| matches!(m.pairing, Pairing::Disabled { .. }))
            .await;
        let (reply, answer) = oneshot::channel();
        r.cmds
            .send(Command::PairPasted {
                link: PairingLink::parse(&good()).unwrap(),
                shown: Some(OLD.into()),
                reply,
            })
            .unwrap();
        r.until(|m| matches!(m.link_wait, Some(LinkWait::Enrolling { .. })))
            .await;
        r.cmds.send(Command::TryAgain).unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(*r.link.starts.lock().unwrap(), [OLD], "no second start");
        go.notify_one();
        assert_eq!(answer.await.unwrap(), Ok(()));
        r.until(|m| matches!(&m.pairing, Pairing::Paired { url, .. } if url == NEW))
            .await;
        assert_eq!(*r.link.starts.lock().unwrap(), [OLD, NEW]);
        r.quit().await;
    }

    /// Decision 5 as the owner settled it (2026-10-15): an enrolment the wallet did not take (it
    /// locked between the dialog and the store) connects at once from memory, is said in a
    /// notification and the tooltip, and is stored once the wallet opens; the old gateway's key
    /// goes only then. Try again on it uses the key in memory.
    #[tokio::test(start_paused = true)]
    async fn an_enrolment_the_wallet_did_not_take_runs_at_once_and_is_stored_when_it_opens() {
        let (enrol, go) = held_enroller();
        let mut r = rig_enrolling("", paired_at(OLD), enrol);
        r.until(paired).await;
        let (reply, answer) = oneshot::channel();
        r.cmds
            .send(Command::PairPasted {
                link: PairingLink::parse(&good()).unwrap(),
                shown: Some(OLD.into()),
                reply,
            })
            .unwrap();
        r.until(|m| matches!(m.link_wait, Some(LinkWait::Enrolling { .. })))
            .await;
        r.store.set_locked(true);
        go.notify_one();
        assert_eq!(answer.await.unwrap(), Ok(()), "the pairing works");
        let m = r
            .until(|m| matches!(&m.pairing, Pairing::Paired { url, .. } if url == NEW))
            .await;
        assert_eq!(
            *r.link.starts.lock().unwrap(),
            [OLD, NEW],
            "connected at once"
        );
        let n = r.notice("Kai: the pairing is not stored yet").await;
        assert!(
            n.body.contains("KWallet is locked") && n.body.contains("loses the pairing"),
            "{}",
            n.body
        );
        let line = m.unstored.clone().unwrap_or_default();
        assert!(line.contains("KWallet is locked"), "{line}");
        assert_eq!(r.stored(), [old_fp()], "nothing stored, the old key kept");
        assert_eq!(r.paired_url().as_deref(), Some(OLD));
        // Disabled meanwhile: Try again uses the key in memory.
        r.link.raise(LinkEvent::Revoked {
            reason: "disabled".into(),
            loss: Some(KeyLoss::Disabled),
        });
        r.until(|m| matches!(m.pairing, Pairing::Disabled { .. }))
            .await;
        r.cmds.send(Command::TryAgain).unwrap();
        r.until(|m| matches!(&m.pairing, Pairing::Paired { url, .. } if url == NEW))
            .await;
        assert_eq!(*r.link.starts.lock().unwrap(), [OLD, NEW, NEW]);
        // lmgw's hello names other URLs meanwhile (device link §7.2): kept with the pairing in
        // memory, and stored with it.
        r.link.raise(LinkEvent::Device(DeviceFacts {
            urls: Some(vec![MOVED.to_owned(), NEW.to_owned()].try_into().unwrap()),
            cert: None,
            dashboard: None,
        }));
        r.until(|m| matches!(&m.pairing, Pairing::Paired { url, .. } if url == MOVED))
            .await;
        assert_eq!(
            r.paired_url().as_deref(),
            Some(OLD),
            "the old pairing's file stays until the new one is stored"
        );
        // The wallet opens: stored, the old gateway's key goes, the line goes.
        r.store.set_locked(false);
        r.notice("Kai stored its pairing").await;
        r.until_stored(&[&new_fp()]).await;
        assert_eq!(r.paired_url().as_deref(), Some(MOVED));
        r.until(|m| m.unstored.is_none()).await;
        r.cmds.send(Command::Quit).unwrap();
        let (task, mut notices) = (r.task, r.notices);
        assert_eq!(task.await.unwrap(), Ended::Quit);
        while let Ok(n) = notices.try_recv() {
            assert_ne!(n.summary, "Kai: the pairing is lost", "{}", n.body);
        }
    }

    /// A quit while the pairing is still not stored says it is lost.
    #[tokio::test(start_paused = true)]
    async fn a_quit_with_the_pairing_unstored_says_it_is_lost() {
        let (enrol, go) = held_enroller();
        let mut r = rig_enrolling("", |_, _| {}, enrol);
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        let (reply, answer) = oneshot::channel();
        r.cmds
            .send(Command::PairPasted {
                link: PairingLink::parse(&good()).unwrap(),
                shown: None,
                reply,
            })
            .unwrap();
        r.until(|m| matches!(m.link_wait, Some(LinkWait::Enrolling { .. })))
            .await;
        r.store.set_locked(true);
        go.notify_one();
        assert_eq!(answer.await.unwrap(), Ok(()));
        r.until(paired).await;
        r.notice("Kai: the pairing is not stored yet").await;
        r.cmds.send(Command::Quit).unwrap();
        let n = r.notice("Kai: the pairing is lost").await;
        assert!(
            n.body.contains(NEW) && n.body.contains("KWallet is locked"),
            "{}",
            n.body
        );
        assert_eq!(r.task.await.unwrap(), Ended::Quit);
    }

    /// The security review's S4: `device-link.json` cannot be written after the wallet took the
    /// key. The pairing is kept and runs from memory, the owner hears that a restart would lose it,
    /// and the write is tried again until it works.
    #[tokio::test(start_paused = true)]
    async fn a_failed_write_after_the_wallet_took_the_key_keeps_the_pairing() {
        let mut r = rig("", |_, _| {});
        r.until(|m| m.pairing == Pairing::NotPaired).await;
        std::fs::create_dir_all(r.paths.device_link_file.join("in-the-way")).unwrap();
        assert_eq!(r.paste_shown(&good(), None).await, Ok(()));
        let m = r
            .until(|m| matches!(&m.pairing, Pairing::Paired { url, .. } if url == NEW))
            .await;
        assert_eq!(*r.link.starts.lock().unwrap(), [NEW]);
        assert_eq!(r.stored(), [new_fp()], "the wallet took the key");
        let n = r.notice("Kai: the pairing is not stored yet").await;
        assert!(
            n.body.contains("cannot write") && n.body.contains("loses the pairing"),
            "{}",
            n.body
        );
        assert!(m.unstored.is_some());
        // A further pairing waits for this one to be stored (the dialog shows the pairing Kai
        // runs).
        let e = r
            .paste_shown(&good(), Some(NEW.into()))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("not stored yet") && e.contains("cannot write"),
            "{e}"
        );
        tokio::time::sleep(Duration::from_secs(12)).await;
        assert!(r.model.borrow().unstored.is_some(), "still not written");
        std::fs::remove_dir_all(&r.paths.device_link_file).unwrap();
        r.notice("Kai stored its pairing").await;
        assert_eq!(r.paired_url().as_deref(), Some(NEW));
        r.until(|m| m.unstored.is_none()).await;
        r.quit().await;
    }

    /// lmgw's device link §12: a pairing from before TLS (`lmgw.url`, a bearer key in the wallet)
    /// says "pair again"; the next pairing removes both.
    #[tokio::test(start_paused = true)]
    async fn a_pairing_from_before_tls_says_pair_again_and_the_next_pairing_clears_it() {
        let v1 = "http://127.0.0.1:8001";
        let mut r = rig(&format!("[lmgw]\nurl = \"{v1}\"\n"), |s, _| {
            s.v1.lock().unwrap().insert(v1.into());
        });
        let m = r
            .until(|m| matches!(m.pairing, Pairing::PairAgain { .. }))
            .await;
        let Pairing::PairAgain { why } = &m.pairing else {
            unreachable!()
        };
        assert!(why.contains("before TLS") && why.contains(v1), "{why}");
        assert_eq!(
            crate::tray::model::render(&m).menu[0],
            crate::tray::model::MenuEntry::PairWithLmgw
        );
        assert_eq!(r.paste(&good()).await, Ok(()));
        r.until(paired).await;
        for _ in 0..100 {
            if r.store.v1.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(r.store.v1.lock().unwrap().is_empty(), "the bearer key went");
        assert_eq!(Config::load(&r.paths.config_file).unwrap().lmgw.url, None);
        r.quit().await;
    }

    /// lmgw's device link §5.3: a certificate past its date is not tried, and Kai says the date it
    /// knows; from `notice_after` on the tooltip says when it lapses.
    #[tokio::test(start_paused = true)]
    async fn the_certificates_dates_are_said() {
        let lapsed = |s: &MemoryStore, p: &Paths| {
            let mut paired = store_pairing(&PKI, p, s, OLD);
            paired.credentials.notice_after_ms = 1_000;
            paired.credentials.not_after_ms = 1_823_097_791_000 - 1_200_000_000_000;
            paired.save(&p.device_link_file).unwrap();
        };
        let mut r = rig("", lapsed);
        let m = r
            .until(|m| matches!(m.pairing, Pairing::PairAgain { .. }))
            .await;
        assert_eq!(
            m.pairing,
            Pairing::PairAgain {
                why: format!(
                    "this device's certificate lapsed on {}; pair it again",
                    crate::pairing::date(1_823_097_791_000 - 1_200_000_000_000)
                )
            }
        );
        assert!(r.link.starts.lock().unwrap().is_empty(), "not tried");
        r.quit().await;

        let notice = |s: &MemoryStore, p: &Paths| {
            let mut paired = store_pairing(&PKI, p, s, OLD);
            paired.credentials.notice_after_ms = 1_000;
            paired.save(&p.device_link_file).unwrap();
        };
        let mut r = rig("", notice);
        let m = r.until(paired).await;
        assert_eq!(
            m.certificate.as_deref(),
            Some("this device's certificate lapses on 2100-01-01; pair it again before then")
        );
        // The certificate lapses while Kai runs: lmgw says so, and Kai says its date.
        r.link.raise(LinkEvent::Revoked {
            reason: "the device's certificate expired".into(),
            loss: Some(KeyLoss::Lapsed),
        });
        let m = r
            .until(|m| matches!(m.pairing, Pairing::PairAgain { .. }))
            .await;
        assert_eq!(
            m.pairing,
            Pairing::PairAgain {
                why: "this device's certificate lapsed on 2100-01-01; pair it again".into()
            }
        );
        r.quit().await;
    }

    #[tokio::test(start_paused = true)]
    async fn check_for_updates_goes_to_the_updater() {
        let mut r = rig("", |_, _| {});
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
        let r = rig("", |_, _| {});
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
        let r = rig("", |_, _| {});
        r.cmds.send(Command::Restart).unwrap();
        r.cmds.send(Command::Quit).unwrap();
        assert_eq!(r.task.await.unwrap(), Ended::Quit);
    }

    /// A Restart after the quit began is not one: Kai ends as a quit.
    #[tokio::test(start_paused = true)]
    async fn a_restart_after_quit_is_a_quit() {
        let r = rig("", |_, _| {});
        r.cmds.send(Command::Quit).unwrap();
        r.cmds.send(Command::Restart).unwrap();
        assert_eq!(r.task.await.unwrap(), Ended::Quit);
    }

    #[tokio::test(start_paused = true)]
    async fn start_with_plasma_toggles_the_autostart_file() {
        let mut r = rig("", |_, _| {});
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
            "[lmgw]\nretry_max_s = 7\n[audio]\noutput = \"hdmi\"\n",
            |_, _| {},
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
        assert_eq!(c.lmgw.retry_max_s, 7, "the rest is kept");
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
        let mut r = rig("", |_, _| {});
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
        let mut r = rig("[lmgw]\nretry_max_s = 7\n", |_, _| {});
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
        let mut r = rig("[lmgw]\nretry_max_s = 7\n", |_, _| {});
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
        let mut r = rig("", |_, _| {});
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
        let r = rig(FILE, |_, _| {});
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

    /// The confirmation's "always allow" (`Command::AllowAlways`): the tool's state is written as
    /// `allow_trusted`, the file's comments stay, and the host gets the new policy; a tool Kai
    /// does not offer writes nothing and is said.
    #[tokio::test]
    async fn always_allow_writes_allow_trusted() {
        const FILE: &str = "# mine\n[policy.tools]\napp_launch = \"confirm\"\n";
        let mut r = rig(FILE, |_, _| {});
        r.cmds
            .send(Command::AllowAlways {
                tool: "app_launch".into(),
            })
            .unwrap();
        r.cmds
            .send(Command::AllowAlways {
                tool: "no_such_tool".into(),
            })
            .unwrap();
        let notice = r.notice("Kai: \"always allow\" was not saved").await;
        assert!(notice.body.contains("no_such_tool"), "{notice:?}");
        assert_eq!(
            std::fs::read_to_string(&r.paths.config_file).unwrap(),
            "# mine\n[policy.tools]\napp_launch = \"allow_trusted\"\n"
        );
        assert_eq!(
            r.host.policies.lock().unwrap().len(),
            1,
            "the host has the new policy"
        );
    }

    /// WP21b: the window's operations run in the core like the tools' (the file, the host's policy),
    /// answer with the structured response, and a refusal writes nothing; the tray's Settings
    /// reaches the GTK side.
    #[tokio::test]
    async fn the_windows_operations_run_in_the_core() {
        use crate::settings::{Request, Response};

        const FILE: &str = "# mine\n[voice]\ngrace_s = 30\n";
        let r = rig(FILE, |_, _| {});
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
        let r = rig("", |_, _| {});
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

        let mut r = rig("", |_, _| {});
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
