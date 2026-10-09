//! `$XDG_CONFIG_HOME/kai/config.toml` (§2).
//!
//! Every key is optional; a missing file is all defaults. Unknown keys are an error, so a typo never
//! silently falls back to a default. Every limit here is named, with its key, in the error it
//! causes. Keys of later work packages join their sections as those packages land. The tray writes
//! `audio.echo_mode`, `audio.input`, `audio.output`, `voice.mode` and
//! `overlay.ring_over_fullscreen`; the conversation writes `conversation.folder_id`. A write
//! changes only its value: the owner's comments and formatting stay, and it is atomic ([`set_value`]).
//!
//! ```toml
//! [lmgw]
//! # Written by pairing. Selects which Secret Service item (attribute `lmgw`) holds the key.
//! url = "http://127.0.0.1:8001"
//! retry_max_s = 30        # §3.2: the feed reconnects after 1 s, doubling up to this
//! timeout_s = 10          # how long lmgw may take to answer the feed's request, and then to send
//!                         # its first bytes (any byte counts, a keep-alive comment included); a
//!                         # feed that began without its hello (lmgw sends a keep-alive comment
//!                         # first while a level change goes live, then hello within one of its
//!                         # keep-alive intervals) may then go two intervals without a byte: the
//!                         # interval lmgw stated in its last hello, or this value until it stated
//!                         # one (so keep it at least half lmgw's keep-alive). Once up, two
//!                         # hello.keepalive_s intervals without a byte are a dead link. Each drop
//!                         # is said in the tooltip and the log.
//!
//! [wallet]
//! retry_s = 5      # while the wallet is locked or the Secret Service fails, Kai checks again this often
//! timeout_s = 10   # how long one Secret Service call may take
//!
//! [pairing]
//! handoff_timeout_s = 5   # how long the scheme handler waits for the running Kai to take a link
//!                         # (Kai answers at once and asks the owner to confirm it in its own
//!                         # dialog), and how long a Kai started for a link waits for it on stdin
//!
//! [dbus]
//! timeout_s = 5           # how long one session-bus call may take that has no bound of its own:
//!                         # a desktop notification, reading the session's environment for a Kai
//!                         # the scheme handler starts, the tray's removal and the name's release
//!                         # at quit
//!
//! [audio]
//! echo_mode = "device"    # device | app | headphones | half_duplex (§6.2)
//! input = "alsa_input…"   # the picked microphone's node.name; absent: the system default
//! output = "alsa_output…" # the picked speaker's node.name; absent: the system default
//! capture_ring_ms = 2000  # microphone audio the session may leave undrained before it is dropped
//! playback_ring_ms = 2000 # reply audio handed ahead to the PipeWire thread (the rest waits queued)
//! flush_wait_ms = 1000    # how long a barge-in waits for the playback stream to take the flush
//! retry_s = 2             # while PipeWire is not reachable, Kai tries again this often
//!
//! [aec]                   # app echo cancellation (echo_mode = "app", K20)
//! ready_timeout_s = 4     # the wait for the echo-cancel module's nodes
//! restore_timeout_s = 4   # the wait for PipeWire to confirm the restored default devices
//!
//! [ring]                  # the ring overlay's placement (§9.1)
//! anchor = "bottom-right" # top-left | top | top-right | left | center | right | bottom-left
//!                         # | bottom | bottom-right
//! margin = 24             # px from the anchored edges
//! size = 240              # px, the square surface
//! output = "DP-1"         # a connector name; absent: KWin's active output
//!
//! [overlay]
//! ring_over_fullscreen = true  # the ring stays above fullscreen windows (layer OVERLAY; off: TOP)
//! # output, show_timeout_s, unresponsive_s, the fetch rules (image_max_mb,
//! # fetch_private_addresses, fetch_timeout_s) and the per-kind tables [overlay.text],
//! # [overlay.html], [overlay.image], [overlay.emote], [overlay.weather]: the tool-controlled
//! # overlays (§10.1), documented with their types in `crate::widgets::settings`
//!
//! [weather]               # overlay_weather's saved places (`crate::widgets::weather`)
//! places = { home = "52.52,13.405" }   # name = "latitude,longitude"
//!
//! [conversation]          # the Kai conversation (§4)
//! folder_name = "Kai"     # the Chat folder Kai looks up by name (and creates, plain, when none
//!                         # exists); names repeat: the ongoing one with the lowest id wins
//! folder_id = 12          # the folder Kai found: written once it found an ongoing one, removed
//!                         # when lmgw no longer has it or Kai pairs with another gateway; set by
//!                         # hand it picks the folder (used while it is an ongoing conversation)
//!
//! [voice]                 # the voice session (§5)
//! on_at_start = false     # K4: listening is off at login; true opens a session at start
//! mode = "automatic"      # automatic | push_to_talk (§5.4; the tray's radio writes it)
//! grace_s = 30            # K5: a hold-to-talk session with listening off closes this long after
//!                         # its reply has played
//! connect_timeout_s = 15  # §5.3: audio captured before session.created is kept this long; past it
//!                         # it is dropped, with a notification, and the connect given up
//! close_wait_s = 3        # quit: how long Kai waits for lmgw to take the session's close (the
//!                         # playing reply's truncate goes first)
//! keepalive_s = 15        # the voice socket's ping; nothing from lmgw for two intervals (or a
//!                         # write it does not take in that long) ends the session as a drop
//! send_queue_s = 30       # the audio the voice socket's outgoing queue holds; beyond it the
//!                         # microphone's audio is dropped, said in the tooltip (at least
//!                         # connect_timeout_s: the audio buffered before the session goes into it)
//! conversation_timeout_s = 300  # K27: conversation mode (the tool conversation_start) ends after
//!                         # this long without speech; 0: it never ends by silence
//!
//! [shortcuts]
//! hold_ms = 300           # K6: a talk key released sooner is a tap (listening on/off); held
//!                         # longer, it talks
//! max_hold_s = 300        # a stuck-key guard, not a limit on speech: a hold no release ended in
//!                         # this long ends with its audio discarded (nothing is sent), logged,
//!                         # notified and said in the tooltip
//! bind = true             # false: no global shortcuts are registered with Plasma at all (the live
//!                         # checks set it, so nothing lands in kglobalshortcutsrc)
//!
//! [update]                # Kai's self-updater (§13); only a build with an update feed looks
//! background_check = true # look for a newer Kai in the background (said only when there is one);
//!                         # off: only the tray's "Check for Updates…" looks
//! first_check_s = 20      # the first background check, this long after the start
//! interval_s = 21600      # then one every this long (6 h)
//! timeout_s = 30          # how long the feed may take to take the connection, and then between
//!                         # two reads of an answer (the manifest, the RPM's download); no bound
//!                         # on a download as a whole
//!
//! [window]                # the configuration window (WP21b): its Personality page's requests to lmgw
//! proxy_max_mb = 64       # the most a request of the page, or lmgw's answer to one, may be (lmgw's
//!                         # own max_body_mb default); 0: no bound
//! proxy_timeout_s = 0     # how long lmgw may take to answer one; 0: no bound of Kai's own (lmgw's
//!                         # own timeouts apply)
//! ```
//!
//! `[host]` and `[policy]`, the hosted MCP server's link and K15's policy, are documented with
//! their types in `crate::host::settings`; `[desktop]`, the provider per capability (K23), in
//! `crate::desktop::settings`; `[search]`, `find_files`' limit, and `[accessibility]`, the
//! walk's concurrency and Kai's accessibility switch (`switch`, `wait_ms`), in
//! `crate::desktop::read`; `[launch]`, what bounds `run_command` (WP16a), in
//! `crate::desktop::fdo::launch::settings`; `[delegate]`, the delegates' aliases (K25), in
//! `crate::delegate::settings`; `[routines]`, the routines' switches and `routine_create`'s alias
//! (K28), in `crate::routines::settings`; `[approvals]`, the approval card and the spoken answer's
//! phrases (WP19), in `crate::approvals::settings`; `[jobs]`, when a job's result is spoken (WP18),
//! in `crate::jobs::settings`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::audio::EchoMode;
use crate::overlay::placement::Anchor;
use crate::voice::VoiceMode;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub lmgw: LmgwConfig,
    pub wallet: WalletConfig,
    pub pairing: PairingConfig,
    pub dbus: DbusConfig,
    pub audio: AudioConfig,
    pub aec: AecConfig,
    pub ring: RingConfig,
    pub overlay: OverlayConfig,
    pub weather: crate::widgets::weather::WeatherConfig,
    pub conversation: ConversationConfig,
    pub voice: VoiceConfig,
    pub shortcuts: ShortcutsConfig,
    pub update: UpdateConfig,
    pub host: crate::host::settings::HostConfig,
    pub policy: crate::host::settings::PolicyConfig,
    pub desktop: crate::desktop::settings::DesktopConfig,
    pub search: crate::desktop::read::SearchConfig,
    pub accessibility: crate::desktop::read::AccessibilityConfig,
    pub launch: crate::desktop::fdo::launch::settings::LaunchConfig,
    pub delegate: crate::delegate::settings::DelegateConfig,
    pub routines: crate::routines::settings::RoutinesConfig,
    pub approvals: crate::approvals::settings::ApprovalsConfig,
    pub jobs: crate::jobs::settings::JobsConfig,
    pub window: WindowConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LmgwConfig {
    /// The base URL Kai is paired with. `None`: not paired.
    pub url: Option<String>,
    /// The feed's reconnect backoff: from 1 s, doubling up to this (§3.2).
    pub retry_max_s: u64,
    /// How long lmgw may take to answer the feed's request and send the body's first bytes (any
    /// byte, a keep-alive comment included); until lmgw states its keep-alive interval in a
    /// `hello`, it also stands in for that interval (`lmgw_link::feed::Wait`, §3.2).
    pub timeout_s: u64,
}

impl Default for LmgwConfig {
    fn default() -> Self {
        Self {
            url: None,
            retry_max_s: 30,
            timeout_s: 10,
        }
    }
}

impl LmgwConfig {
    pub fn retry_max(&self) -> Duration {
        Duration::from_secs(self.retry_max_s)
    }
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WalletConfig {
    pub retry_s: u64,
    pub timeout_s: u64,
}

impl Default for WalletConfig {
    fn default() -> Self {
        Self {
            retry_s: 5,
            timeout_s: 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PairingConfig {
    pub handoff_timeout_s: u64,
}

impl Default for PairingConfig {
    fn default() -> Self {
        Self {
            handoff_timeout_s: 5,
        }
    }
}

/// The session-bus calls that have no bound of their own (notifications, the quit's tray and name
/// release, the session environment for a handler-started Kai).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DbusConfig {
    pub timeout_s: u64,
}

impl Default for DbusConfig {
    fn default() -> Self {
        Self { timeout_s: 5 }
    }
}

impl DbusConfig {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AudioConfig {
    pub echo_mode: EchoMode,
    pub input: Option<String>,
    pub output: Option<String>,
    pub capture_ring_ms: u64,
    pub playback_ring_ms: u64,
    pub flush_wait_ms: u64,
    pub retry_s: u64,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            echo_mode: EchoMode::Device,
            input: None,
            output: None,
            capture_ring_ms: 2000,
            playback_ring_ms: 2000,
            flush_wait_ms: 1000,
            retry_s: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AecConfig {
    pub ready_timeout_s: u64,
    pub restore_timeout_s: u64,
}

impl Default for AecConfig {
    fn default() -> Self {
        Self {
            ready_timeout_s: 4,
            restore_timeout_s: 4,
        }
    }
}

/// The ring overlay's placement (§9.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RingConfig {
    pub anchor: Anchor,
    pub margin: u32,
    pub size: u32,
    /// A connector name (`DP-1`); `None`: KWin's active output.
    pub output: Option<String>,
}

impl Default for RingConfig {
    fn default() -> Self {
        Self {
            anchor: Anchor::BottomRight,
            margin: 24,
            size: 240,
            output: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct OverlayConfig {
    /// §16 answer 5: on, the ring is created on the OVERLAY layer (above fullscreen windows); off,
    /// on TOP (hidden by them).
    pub ring_over_fullscreen: bool,
    /// The tool-controlled overlays' output (§10.1, `crate::widgets::settings`): a connector name;
    /// `None`: KWin's active output at each show.
    pub output: Option<String>,
    /// How long a show may take from the call to the map (`crate::widgets::settings`).
    pub show_timeout_s: u64,
    /// How long the widgets' web process may leave Kai's ping unanswered
    /// (`crate::widgets::settings`).
    pub unresponsive_s: u64,
    /// `overlay_text`'s defaults.
    #[serde(deserialize_with = "crate::widgets::settings::text")]
    pub text: crate::widgets::settings::KindConfig,
    /// `overlay_html`'s defaults.
    #[serde(deserialize_with = "crate::widgets::settings::html")]
    pub html: crate::widgets::settings::KindConfig,
    /// `overlay_image`'s, `overlay_emote`'s and `overlay_weather`'s defaults.
    #[serde(deserialize_with = "crate::widgets::settings::image")]
    pub image: crate::widgets::settings::KindConfig,
    #[serde(deserialize_with = "crate::widgets::settings::emote")]
    pub emote: crate::widgets::settings::KindConfig,
    #[serde(deserialize_with = "crate::widgets::settings::weather")]
    pub weather: crate::widgets::settings::KindConfig,
    /// `overlay_app`'s defaults (WP12).
    #[serde(deserialize_with = "crate::widgets::settings::app")]
    pub app: crate::widgets::settings::KindConfig,
    /// The most an MCP Apps view's message, or one of lmgw's `/mcp` answers to its bridge, may be,
    /// in MB (`crate::widgets::app_bridge`).
    pub app_max_mb: u64,
    /// The most requests one MCP Apps view may have in flight (`crate::widgets::app_bridge`).
    pub app_max_calls: usize,
    /// The most an image fetch may download, and the most pixel memory its decode may take, in MB
    /// (K19, `crate::widgets::fetch`).
    pub image_max_mb: u64,
    /// Whether loopback, link-local and private (LAN) addresses are fetched (K19).
    pub fetch_private_addresses: bool,
    /// The whole fetch's deadline, in seconds.
    pub fetch_timeout_s: u64,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            ring_over_fullscreen: true,
            output: None,
            show_timeout_s: crate::widgets::settings::SHOW_TIMEOUT_S,
            unresponsive_s: crate::widgets::settings::UNRESPONSIVE_S,
            text: crate::widgets::settings::KindConfig::text(),
            html: crate::widgets::settings::KindConfig::html(),
            image: crate::widgets::settings::KindConfig::image(),
            emote: crate::widgets::settings::KindConfig::emote(),
            weather: crate::widgets::settings::KindConfig::weather(),
            app: crate::widgets::settings::KindConfig::app(),
            app_max_mb: crate::widgets::settings::APP_MAX_MB,
            app_max_calls: crate::widgets::settings::APP_MAX_CALLS,
            image_max_mb: crate::widgets::settings::IMAGE_MAX_MB,
            fetch_private_addresses: true,
            fetch_timeout_s: crate::widgets::settings::FETCH_TIMEOUT_S,
        }
    }
}

/// The Kai conversation (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ConversationConfig {
    /// The Chat folder Kai looks up by name.
    pub folder_name: String,
    /// The folder Kai found (or the owner picked); `None` until it is known.
    pub folder_id: Option<i64>,
}

impl Default for ConversationConfig {
    fn default() -> Self {
        Self {
            folder_name: "Kai".to_owned(),
            folder_id: None,
        }
    }
}

/// The voice session (§5, K4, K5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct VoiceConfig {
    pub on_at_start: bool,
    pub mode: VoiceMode,
    pub grace_s: u64,
    pub connect_timeout_s: u64,
    pub close_wait_s: u64,
    /// The voice socket's ping interval (review KAI-2 K2-6).
    pub keepalive_s: u64,
    /// The audio the voice socket's outgoing queue holds (K2-6).
    pub send_queue_s: u64,
    /// K27: conversation mode ends after this long without speech; 0: never by silence.
    pub conversation_timeout_s: u64,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            on_at_start: false,
            mode: VoiceMode::Automatic,
            grace_s: 30,
            connect_timeout_s: 15,
            close_wait_s: 3,
            keepalive_s: 15,
            send_queue_s: 30,
            conversation_timeout_s: 300,
        }
    }
}

impl VoiceConfig {
    pub fn grace(&self) -> Duration {
        Duration::from_secs(self.grace_s)
    }
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_secs(self.connect_timeout_s)
    }
    pub fn close_wait(&self) -> Duration {
        Duration::from_secs(self.close_wait_s)
    }
    pub fn keepalive(&self) -> Duration {
        Duration::from_secs(self.keepalive_s)
    }
    /// `None`: conversation mode never ends by silence.
    pub fn conversation_timeout(&self) -> Option<Duration> {
        (self.conversation_timeout_s > 0).then(|| Duration::from_secs(self.conversation_timeout_s))
    }
    pub fn send_queue(&self) -> Duration {
        Duration::from_secs(self.send_queue_s)
    }
}

/// The global shortcuts (§8, K6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ShortcutsConfig {
    pub hold_ms: u64,
    pub max_hold_s: u64,
    /// Off: Kai registers no global shortcuts with Plasma (neither the portal nor KGlobalAccel), so
    /// a Kai started for a check leaves nothing in `kglobalshortcutsrc`.
    pub bind: bool,
}

impl Default for ShortcutsConfig {
    fn default() -> Self {
        Self {
            hold_ms: 300,
            max_hold_s: 300,
            bind: true,
        }
    }
}

impl ShortcutsConfig {
    pub fn hold(&self) -> Duration {
        Duration::from_millis(self.hold_ms)
    }
    pub fn max_hold(&self) -> Duration {
        Duration::from_secs(self.max_hold_s)
    }
}

/// Kai's self-updater (§13, the owner's decision 2026-10-08).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UpdateConfig {
    pub background_check: bool,
    pub first_check_s: u64,
    pub interval_s: u64,
    pub timeout_s: u64,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            background_check: true,
            first_check_s: 20,
            interval_s: 6 * 60 * 60,
            timeout_s: 30,
        }
    }
}

impl UpdateConfig {
    pub fn first_check(&self) -> Duration {
        Duration::from_secs(self.first_check_s)
    }
    pub fn interval(&self) -> Duration {
        Duration::from_secs(self.interval_s)
    }
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_s)
    }
}

/// The configuration window (WP21b): what bounds its Personality page's requests to lmgw, which
/// Kai's scheme handler forwards with the device key (`crate::settings::window::profiles`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WindowConfig {
    /// The most a request of the page, or lmgw's answer to one, may be, in MB; 0: no bound.
    pub proxy_max_mb: u64,
    /// How long lmgw may take to answer one, in seconds; 0: no bound.
    pub proxy_timeout_s: u64,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            // lmgw's own `max_body_mb` default: what it takes in, Kai takes back.
            proxy_max_mb: 64,
            proxy_timeout_s: 0,
        }
    }
}

impl AecConfig {
    pub fn ready_timeout(&self) -> Duration {
        Duration::from_secs(self.ready_timeout_s)
    }
    pub fn restore_timeout(&self) -> Duration {
        Duration::from_secs(self.restore_timeout_s)
    }
}

impl WalletConfig {
    pub fn retry(&self) -> Duration {
        Duration::from_secs(self.retry_s)
    }
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_s)
    }
}

impl PairingConfig {
    pub fn handoff_timeout(&self) -> Duration {
        Duration::from_secs(self.handoff_timeout_s)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{}: cannot read: {source}", path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{}: {message}", path.display())]
    Parse { path: PathBuf, message: String },
    #[error("{}: {key} = {value}: {rule}", path.display())]
    Invalid {
        path: PathBuf,
        key: &'static str,
        value: String,
        rule: String,
    },
    #[error("{}: cannot write: {message}", path.display())]
    Write { path: PathBuf, message: String },
}

impl Config {
    /// Reads the file; a missing file is the defaults.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text, path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_owned(),
                source,
            }),
        }
    }

    /// Parses and validates `text`; `path` only names the file in errors.
    pub fn parse(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let mut config: Config = toml::from_str(text).map_err(|e| ConfigError::Parse {
            path: path.to_owned(),
            message: one_line(&e.to_string()),
        })?;
        config.validate(path)?;
        Ok(config)
    }

    fn validate(&mut self, path: &Path) -> Result<(), ConfigError> {
        let at_least_one = |key: &'static str, value: u64| {
            if value == 0 {
                Err(ConfigError::Invalid {
                    path: path.to_owned(),
                    key,
                    value: value.to_string(),
                    rule: "must be at least 1 (seconds)".into(),
                })
            } else {
                Ok(())
            }
        };
        at_least_one("lmgw.retry_max_s", self.lmgw.retry_max_s)?;
        at_least_one("lmgw.timeout_s", self.lmgw.timeout_s)?;
        at_least_one("wallet.retry_s", self.wallet.retry_s)?;
        at_least_one("wallet.timeout_s", self.wallet.timeout_s)?;
        at_least_one("pairing.handoff_timeout_s", self.pairing.handoff_timeout_s)?;
        at_least_one("dbus.timeout_s", self.dbus.timeout_s)?;
        at_least_one("audio.retry_s", self.audio.retry_s)?;
        at_least_one("aec.ready_timeout_s", self.aec.ready_timeout_s)?;
        at_least_one("aec.restore_timeout_s", self.aec.restore_timeout_s)?;
        at_least_one("voice.connect_timeout_s", self.voice.connect_timeout_s)?;
        at_least_one("voice.close_wait_s", self.voice.close_wait_s)?;
        at_least_one("voice.keepalive_s", self.voice.keepalive_s)?;
        at_least_one("shortcuts.max_hold_s", self.shortcuts.max_hold_s)?;
        // `update.first_check_s = 0` is allowed: the first check right at the start.
        at_least_one("update.interval_s", self.update.interval_s)?;
        at_least_one("update.timeout_s", self.update.timeout_s)?;
        let at_least = |key: &'static str, value: u64, min: u64, unit: &str| {
            if value < min {
                Err(ConfigError::Invalid {
                    path: path.to_owned(),
                    key,
                    value: value.to_string(),
                    rule: format!("must be at least {min} ({unit})"),
                })
            } else {
                Ok(())
            }
        };
        // A ring must hold more than one PipeWire cycle (a quantum of up to 8192 frames at 48 kHz
        // is 171 ms).
        at_least(
            "audio.capture_ring_ms",
            self.audio.capture_ring_ms,
            200,
            "ms",
        )?;
        at_least(
            "audio.playback_ring_ms",
            self.audio.playback_ring_ms,
            200,
            "ms",
        )?;
        at_least("audio.flush_wait_ms", self.audio.flush_wait_ms, 1, "ms")?;
        // `voice.grace_s = 0` is allowed: the session closes as soon as its reply has played.
        at_least("shortcuts.hold_ms", self.shortcuts.hold_ms, 1, "ms")?;
        // The audio buffered before `session.created` goes into the queue at once.
        at_least(
            "voice.send_queue_s",
            self.voice.send_queue_s,
            self.voice.connect_timeout_s,
            "seconds: voice.connect_timeout_s, whose buffered audio goes into it",
        )?;
        for (key, value, rule) in [
            (
                "audio.input",
                &self.audio.input,
                "a node.name, or leave the key out for the system default",
            ),
            (
                "audio.output",
                &self.audio.output,
                "a node.name, or leave the key out for the system default",
            ),
            (
                "ring.output",
                &self.ring.output,
                "a connector name (DP-1), or leave the key out for KWin's active output",
            ),
        ] {
            if value.as_deref().is_some_and(|v| v.trim().is_empty()) {
                return Err(ConfigError::Invalid {
                    path: path.to_owned(),
                    key,
                    value: "\"\"".into(),
                    rule: rule.into(),
                });
            }
        }
        if self.conversation.folder_name.trim().is_empty() {
            return Err(ConfigError::Invalid {
                path: path.to_owned(),
                key: "conversation.folder_name",
                value: "\"\"".into(),
                rule: "a Chat folder's name, or leave the key out for \"Kai\"".into(),
            });
        }
        if self.ring.size == 0 {
            return Err(ConfigError::Invalid {
                path: path.to_owned(),
                key: "ring.size",
                value: "0".into(),
                rule: "must be at least 1 (px)".into(),
            });
        }
        for checked in [
            self.host.validate(),
            self.policy.validate(),
            self.delegate.validate(),
            self.routines.validate(),
            self.approvals.validate(),
            crate::widgets::settings::validate(&self.overlay),
            self.weather.validate(),
        ] {
            checked.map_err(|(key, value, rule)| ConfigError::Invalid {
                path: path.to_owned(),
                key,
                value,
                rule,
            })?;
        }
        if let Some(url) = &self.lmgw.url {
            let normalized =
                crate::pairing::normalize_base_url(url).map_err(|rule| ConfigError::Invalid {
                    path: path.to_owned(),
                    key: "lmgw.url",
                    value: format!("{url:?}"),
                    rule,
                })?;
            self.lmgw.url = Some(normalized);
        }
        Ok(())
    }
}

/// Writes `lmgw.url` into the file (see [`set_value`]).
pub fn set_lmgw_url(path: &Path, url: &str) -> Result<(), ConfigError> {
    set_value(path, "lmgw", "url", Some(url))
}

/// Writes `[table] key = "value"` into the file, or removes the key for `None` (see [`set_item`]).
pub fn set_value(
    path: &Path,
    table: &str,
    key: &str,
    value: Option<&str>,
) -> Result<(), ConfigError> {
    set_item(path, table, key, value.map(toml_edit::value))
}

/// Writes `[table] key = <integer>` into the file, or removes the key for `None` (see
/// [`set_item`]).
pub fn set_int(path: &Path, table: &str, key: &str, value: Option<i64>) -> Result<(), ConfigError> {
    set_item(path, table, key, value.map(toml_edit::value))
}

/// Writes `[table] key = true|false` into the file (see [`set_item`]).
pub fn set_bool(path: &Path, table: &str, key: &str, value: bool) -> Result<(), ConfigError> {
    set_item(path, table, key, Some(toml_edit::value(value)))
}

/// Writes `[table] key = <item>` into the file, or removes the key for `None`, keeping everything
/// else in it as it was (review KAI-2 K2-9):
///
/// - a key that exists gets its new value in place: the comment lines above it, its spacing and a
///   comment after the value on its line stay;
/// - a removed key's comment lines above it move to the line that followed it (the next key, the
///   next table's header, or the end of the file);
/// - the file is written through a symlink to its target (a dotfile manager's link stays a link),
///   with the old file's permissions;
/// - the write is atomic and durable: a temp file in the same directory, synced, renamed over the
///   old one, and the directory synced.
///
/// Creates the file and its directory when missing.
fn set_item(
    path: &Path,
    table: &str,
    key: &str,
    value: Option<toml_edit::Item>,
) -> Result<(), ConfigError> {
    let text = read_text(path)?;
    let edited = edit_text(&text, path, table, key, value)?;
    write_atomically(path, edited.as_bytes()).map_err(|e| ConfigError::Write {
        path: path.to_owned(),
        message: e.to_string(),
    })
}

/// The file's text; a missing file is empty.
pub(crate) fn read_text(path: &Path) -> Result<String, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(ConfigError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

/// `text` with `[table] key` set to `value` (or removed for `None`), by [`set_item`]'s rules and
/// nothing written. `table` may be a dotted path (`policy.tools`): the tables on the way are
/// created when missing, as the `[policy.tools]` header alone, without an empty `[policy]` above
/// it.
pub(crate) fn edit_text(
    text: &str,
    path: &Path,
    table: &str,
    key: &str,
    value: Option<toml_edit::Item>,
) -> Result<String, ConfigError> {
    let write_err = |message: String| ConfigError::Write {
        path: path.to_owned(),
        message,
    };
    let mut doc: toml_edit::DocumentMut =
        text.parse()
            .map_err(|e: toml_edit::TomlError| ConfigError::Parse {
                path: path.to_owned(),
                message: one_line(&e.to_string()),
            })?;
    let mut names = table.split('.');
    let root = names.next().unwrap_or_default();
    let nested: Vec<&str> = names.collect();
    let value_set = value.is_some();
    let orphan = {
        let mut section = doc
            .entry(root)
            .or_insert_with(implicit_table)
            .as_table_like_mut()
            .ok_or_else(|| write_err(format!("`{root}` is not a table")))?;
        let mut walked = root.to_owned();
        for name in &nested {
            walked = format!("{walked}.{name}");
            if value.is_none() && section.get(name).is_none() {
                // Removing a key of a table that is not there: nothing to remove.
                return Ok(text.to_owned());
            }
            section = section
                .entry(name)
                .or_insert_with(implicit_table)
                .as_table_like_mut()
                .ok_or_else(|| write_err(format!("`{walked}` is not a table")))?;
        }
        match value {
            Some(v) => {
                set_in_place(section, key, v);
                None
            }
            None => remove_keeping_comments(section, key),
        }
    };
    if value_set {
        // The table that now holds a key has its header; the tables above it, if made here, do not.
        let mut item = doc.get_mut(root);
        for name in &nested {
            item = item.and_then(|i| i.get_mut(*name));
        }
        if let Some(t) = item.and_then(|i| i.as_table_mut()) {
            t.set_implicit(false);
        }
    }
    if let Some(comments) = orphan {
        // The removed key was its table's last: the comments go before the next table's header, or
        // to the end of the file.
        let at = doc
            .get(root)
            .and_then(|t| t.as_table())
            .and_then(|t| t.position());
        let next = at.and_then(|at| {
            doc.iter()
                .filter_map(|(k, item)| Some((k.to_owned(), item.as_table()?.position()?)))
                .filter(|(_, p)| *p > at)
                .min_by_key(|(_, p)| *p)
                .map(|(k, _)| k)
        });
        match next
            .and_then(|k| doc.get_mut(&k))
            .and_then(|t| t.as_table_mut())
        {
            Some(t) => {
                let old = raw(t.decor().prefix());
                t.decor_mut().set_prefix(format!("{comments}{old}"));
            }
            None => {
                let old = raw(Some(doc.trailing()));
                doc.set_trailing(format!("{old}{comments}"));
            }
        }
    }
    Ok(doc.to_string())
}

/// A table made on the way to a key: it prints no header of its own until it holds a key.
fn implicit_table() -> toml_edit::Item {
    let mut t = toml_edit::Table::new();
    t.set_implicit(true);
    toml_edit::Item::Table(t)
}

/// A decor's text (empty when it has none).
fn raw(r: Option<&toml_edit::RawString>) -> String {
    r.and_then(|r| r.as_str()).unwrap_or_default().to_owned()
}

/// `key = value`: a new value in place, the old one's decor (spacing, a trailing comment) kept; the
/// key itself (the comments above it) is not touched. A key that is absent is added.
fn set_in_place(section: &mut dyn toml_edit::TableLike, key: &str, value: toml_edit::Item) {
    match (section.get_mut(key), value) {
        (Some(toml_edit::Item::Value(old)), toml_edit::Item::Value(mut new)) => {
            *new.decor_mut() = old.decor().clone();
            *old = new;
        }
        (_, value) => {
            section.insert(key, value);
        }
    }
}

/// Removes `key`. Its comment lines go to the key after it; returned when there is none (the caller
/// places them after the table).
fn remove_keeping_comments(section: &mut dyn toml_edit::TableLike, key: &str) -> Option<String> {
    let keys: Vec<String> = section.iter().map(|(k, _)| k.to_owned()).collect();
    let comments = section
        .key(key)
        .map(|k| raw(k.leaf_decor().prefix()))
        .filter(|p| p.contains('#'));
    section.remove(key);
    let comments = comments?;
    let next = keys
        .iter()
        .position(|k| k == key)
        .and_then(|i| keys.get(i + 1));
    match next.and_then(|n| section.key_mut(n)) {
        Some(mut k) => {
            let old = raw(k.leaf_decor_mut().prefix());
            k.leaf_decor_mut().set_prefix(format!("{comments}{old}"));
            None
        }
        None => Some(comments),
    }
}

/// A new temporary file next to the target: a name no other writer (this process's other threads,
/// another process) shares, created with `O_EXCL` (`create_new`: an existing file or symlink of that
/// name is an error, never followed), readable by its owner only until the target's mode is set.
fn create_temp(dir: &Path, name: &str) -> std::io::Result<(std::fs::File, PathBuf)> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    for _ in 0..64 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(
            ".{name}.{}.{n}.{nanos:08x}.tmp",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
        {
            Ok(f) => return Ok((f, tmp)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no unused temporary file name"))
}

/// Writes `bytes` to `path` (through a symlink, to its target) atomically and durably (see
/// [`set_item`]).
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let target = match std::fs::canonicalize(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // A dangling link writes to where it points; a missing file is created.
            match std::fs::read_link(path) {
                Ok(t) if t.is_absolute() => t,
                Ok(t) => path.parent().unwrap_or(Path::new(".")).join(t),
                Err(_) => path.to_owned(),
            }
        }
        Err(e) => return Err(e),
    };
    let dir = target
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent directory"))?;
    std::fs::create_dir_all(dir)?;
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".into());
    let (mut f, tmp) = create_temp(dir, &name)?;
    let result = (|| {
        if let Ok(meta) = std::fs::metadata(&target) {
            f.set_permissions(meta.permissions())?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &target)?;
        std::fs::File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn one_line(s: &str) -> String {
    s.split('\n')
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config, ConfigError> {
        Config::parse(text, Path::new("/x/kai/config.toml"))
    }

    #[test]
    fn empty_and_missing_are_the_defaults() {
        assert_eq!(parse("").unwrap(), Config::default());
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Config::load(&dir.path().join("nope.toml")).unwrap(),
            Config::default()
        );
        let d = Config::default();
        assert_eq!(
            (
                d.wallet.retry_s,
                d.wallet.timeout_s,
                d.pairing.handoff_timeout_s
            ),
            (5, 10, 5)
        );
        assert_eq!(d.dbus.timeout_s, 5);
        assert_eq!(d.lmgw.url, None);
        assert_eq!((d.lmgw.retry_max_s, d.lmgw.timeout_s), (30, 10), "§3.2");
        assert_eq!(d.audio.echo_mode, EchoMode::Device, "K8");
        assert_eq!(
            (d.audio.input.as_deref(), d.audio.output.as_deref()),
            (None, None)
        );
        assert_eq!(
            (
                d.audio.capture_ring_ms,
                d.audio.playback_ring_ms,
                d.audio.flush_wait_ms,
                d.audio.retry_s
            ),
            (2000, 2000, 1000, 2)
        );
        assert_eq!(
            (d.aec.ready_timeout_s, d.aec.restore_timeout_s),
            (4, 4),
            "K20"
        );
    }

    #[test]
    fn audio_keys_are_read_and_checked() {
        let c = parse(
            "[audio]\necho_mode = \"app\"\ninput = \"usb mic\"\ncapture_ring_ms = 500\n[aec]\nready_timeout_s = 9\n",
        )
        .unwrap();
        assert_eq!(c.audio.echo_mode, EchoMode::App);
        assert_eq!(c.audio.input.as_deref(), Some("usb mic"));
        assert_eq!(c.audio.capture_ring_ms, 500);
        assert_eq!(c.aec.ready_timeout(), Duration::from_secs(9));
        let e = parse("[audio]\necho_mode = \"loud\"\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown variant `loud`"), "{e}");
        for (text, key) in [
            ("[audio]\ncapture_ring_ms = 100\n", "audio.capture_ring_ms"),
            ("[audio]\nplayback_ring_ms = 0\n", "audio.playback_ring_ms"),
            ("[audio]\nretry_s = 0\n", "audio.retry_s"),
            ("[aec]\nready_timeout_s = 0\n", "aec.ready_timeout_s"),
            ("[aec]\nrestore_timeout_s = 0\n", "aec.restore_timeout_s"),
            ("[audio]\noutput = \" \"\n", "audio.output"),
        ] {
            let e = parse(text).unwrap_err().to_string();
            assert!(e.contains(key), "{text}: {e}");
        }
    }

    #[test]
    fn the_ring_and_overlay_keys_have_section_9_defaults() {
        let d = Config::default();
        assert_eq!(
            (d.ring.anchor, d.ring.margin, d.ring.size, d.ring.output),
            (Anchor::BottomRight, 24, 240, None),
            "§9.1: bottom right, 24 px, 240 px, the active output"
        );
        assert!(
            d.overlay.ring_over_fullscreen,
            "§16 answer 5: on by default"
        );
    }

    #[test]
    fn the_ring_and_overlay_keys_are_read_and_checked() {
        let c = parse(
            "[ring]\nanchor = \"top-left\"\nmargin = 0\nsize = 320\noutput = \"DP-1\"\n[overlay]\nring_over_fullscreen = false\n",
        )
        .unwrap();
        assert_eq!(
            (c.ring.anchor, c.ring.margin, c.ring.size),
            (Anchor::TopLeft, 0, 320)
        );
        assert_eq!(c.ring.output.as_deref(), Some("DP-1"));
        assert!(!c.overlay.ring_over_fullscreen);
        for a in [
            "top",
            "top-right",
            "left",
            "center",
            "right",
            "bottom-left",
            "bottom",
        ] {
            parse(&format!("[ring]\nanchor = \"{a}\"\n")).unwrap();
        }
        for (text, want) in [
            ("[ring]\nanchor = \"middle\"\n", "unknown variant `middle`"),
            ("[ring]\nsize = 0\n", "ring.size"),
            ("[ring]\nmargin = -1\n", "margin"),
            ("[ring]\noutput = \"\"\n", "ring.output"),
            ("[ring]\nplace = 1\n", "unknown field `place`"),
            (
                "[overlay]\nring_over_fullscreen = \"yes\"\n",
                "invalid type",
            ),
        ] {
            let e = parse(text).unwrap_err().to_string();
            assert!(e.contains(want), "{text}: {e}");
        }
    }

    #[test]
    fn set_bool_writes_a_boolean() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kai").join("config.toml");
        set_bool(&path, "overlay", "ring_over_fullscreen", false).unwrap();
        assert!(!Config::load(&path).unwrap().overlay.ring_over_fullscreen);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("ring_over_fullscreen = false")
        );
        set_bool(&path, "overlay", "ring_over_fullscreen", true).unwrap();
        assert!(Config::load(&path).unwrap().overlay.ring_over_fullscreen);
    }

    #[test]
    fn concurrent_writers_never_share_a_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "x = 0\n").unwrap();
        // The temporary names are distinct, and an existing file or link of a name is never reused.
        let (_a, ta) = create_temp(dir.path(), "config.toml").unwrap();
        let (_b, tb) = create_temp(dir.path(), "config.toml").unwrap();
        assert_ne!(ta, tb);
        std::fs::remove_file(&ta).unwrap();
        std::fs::remove_file(&tb).unwrap();
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for j in 0..20 {
                        let text = format!("x = {i}{j:02}\n").repeat(200);
                        write_atomically(&path, text.as_bytes()).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        // The file is one writer's whole text, and no temporary file is left.
        let text = std::fs::read_to_string(&path).unwrap();
        let first = text.lines().next().unwrap();
        assert!(text.lines().all(|l| l == first) && text.lines().count() == 200);
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("config.toml")]);
    }

    #[test]
    fn set_int_writes_and_removes_the_folder_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kai").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[conversation]\nfolder_name = \"Kai\" # mine\n").unwrap();
        set_int(&path, "conversation", "folder_id", Some(12)).unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(c.conversation.folder_id, Some(12));
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("folder_id = 12")
        );
        set_int(&path, "conversation", "folder_id", None).unwrap();
        assert_eq!(Config::load(&path).unwrap().conversation.folder_id, None);
        assert!(std::fs::read_to_string(&path).unwrap().contains("# mine"));
    }

    #[test]
    fn set_value_writes_and_removes_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kai").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "# keep\n[audio]\necho_mode = \"app\" # mine\n").unwrap();
        set_value(&path, "audio", "output", Some("hdmi")).unwrap();
        set_value(&path, "audio", "echo_mode", Some("half_duplex")).unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(c.audio.output.as_deref(), Some("hdmi"));
        assert_eq!(c.audio.echo_mode, EchoMode::HalfDuplex);
        set_value(&path, "audio", "output", None).unwrap();
        assert_eq!(Config::load(&path).unwrap().audio.output, None);
        assert!(std::fs::read_to_string(&path).unwrap().contains("# keep"));
    }

    /// Review KAI-2 K2-9: the owner's comments and formatting stay; the write goes through a
    /// symlink, keeps the permissions and leaves no temp file.
    #[test]
    fn a_write_keeps_comments_formatting_and_links() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles").join("kai.toml");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        let text = "\
# a comment at the top
[audio]
# a comment above the first key
input = \"node-a\"   # a trailing comment
# a comment above the last key of its table
echo_mode   =   \"app\" # another trailing comment

[lmgw]
# a comment above the url
url = \"http://old:1\" # the url's trailing comment

[conversation]
folder_name = \"Kai\"
# a comment above the last key of the file
folder_id = 12
";
        std::fs::write(&real, text).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let path = dir.path().join("kai").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        symlink(&real, &path).unwrap();

        set_value(&path, "audio", "echo_mode", Some("half_duplex")).unwrap();
        set_lmgw_url(&path, "http://new:2").unwrap();
        let got = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            got,
            text.replace(
                "\"app\" # another trailing comment",
                "\"half_duplex\" # another trailing comment"
            )
            .replace("http://old:1", "http://new:2"),
            "only the two values changed"
        );
        // A removed key's comment lines stay: before the next key, or the next table's header, or
        // at the end.
        set_value(&path, "audio", "input", None).unwrap();
        set_value(&path, "audio", "echo_mode", None).unwrap();
        set_int(&path, "conversation", "folder_id", None).unwrap();
        let got = std::fs::read_to_string(&path).unwrap();
        for kept in [
            "# a comment at the top",
            "# a comment above the first key",
            "# a comment above the last key of its table",
            "# a comment above the url",
            "url = \"http://new:2\" # the url's trailing comment",
            "# a comment above the last key of the file",
        ] {
            assert!(got.contains(kept), "{kept} lost:\n{got}");
        }
        assert!(
            got.find("# a comment above the last key of its table")
                .unwrap()
                < got.find("[lmgw]").unwrap()
        );
        assert!(
            !got.contains("input =") && !got.contains("folder_id ="),
            "{got}"
        );
        let c = Config::load(&path).unwrap();
        assert_eq!(c.lmgw.url.as_deref(), Some("http://new:2"));
        assert_eq!(c.audio.input, None);
        assert_eq!(c.conversation.folder_id, None);

        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link stays a link"
        );
        assert_eq!(
            std::fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o600
        );
        for d in [real.parent().unwrap(), path.parent().unwrap()] {
            let left: Vec<_> = std::fs::read_dir(d)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .filter(|n| n.to_string_lossy().ends_with(".tmp"))
                .collect();
            assert!(left.is_empty(), "{left:?}");
        }
    }

    #[test]
    fn values_are_read_and_the_url_normalized() {
        let c = parse("[lmgw]\nurl = \"http://127.0.0.1:8001/\"\n[wallet]\nretry_s = 2\ntimeout_s = 30\n[pairing]\nhandoff_timeout_s = 9\n").unwrap();
        assert_eq!(c.lmgw.url.as_deref(), Some("http://127.0.0.1:8001"));
        assert_eq!(c.wallet.retry(), Duration::from_secs(2));
        assert_eq!(c.wallet.timeout(), Duration::from_secs(30));
        assert_eq!(c.pairing.handoff_timeout(), Duration::from_secs(9));
    }

    #[test]
    fn the_conversation_folder_is_read() {
        assert_eq!(Config::default().conversation.folder_id, None);
        assert_eq!(Config::default().conversation.folder_name, "Kai");
        let c = parse("[conversation]\nfolder_id = 12\nfolder_name = \"Assistant\"\n").unwrap();
        assert_eq!(c.conversation.folder_id, Some(12));
        assert_eq!(c.conversation.folder_name, "Assistant");
        let e = parse("[conversation]\nfolder_name = \" \"\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("conversation.folder_name"), "{e}");
        let e = parse("[conversation]\nfolder_id = \"Kai\"\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("folder_id"), "{e}");
    }

    #[test]
    fn an_unknown_key_is_an_error_that_names_it() {
        let e = parse("[wallet]\nretry = 3\n").unwrap_err().to_string();
        assert!(e.contains("/x/kai/config.toml"), "{e}");
        assert!(e.contains("unknown field `retry`"), "{e}");
        let e = parse("[speech]\non_at_start = true\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown field `speech`"), "{e}");
    }

    #[test]
    fn the_voice_and_shortcut_keys_have_their_defaults_and_are_checked() {
        let d = Config::default();
        assert_eq!(
            (
                d.voice.on_at_start,
                d.voice.mode,
                d.voice.grace_s,
                d.voice.connect_timeout_s,
                d.voice.close_wait_s,
                d.shortcuts.hold_ms,
                d.shortcuts.max_hold_s
            ),
            (false, VoiceMode::Automatic, 30, 15, 3, 300, 300),
            "K4, K5, §5.3, K6"
        );
        let c = parse(
            "[voice]\non_at_start = true\nmode = \"push_to_talk\"\ngrace_s = 0\nconnect_timeout_s = 5\n[shortcuts]\nhold_ms = 250\n",
        )
        .unwrap();
        assert!(c.voice.on_at_start);
        assert_eq!(c.voice.mode, VoiceMode::PushToTalk);
        assert_eq!(c.voice.grace(), Duration::ZERO);
        assert_eq!(c.voice.connect_timeout(), Duration::from_secs(5));
        assert_eq!(c.shortcuts.hold(), Duration::from_millis(250));
        assert!(d.shortcuts.bind, "shortcuts are registered by default");
        assert!(!parse("[shortcuts]\nbind = false\n").unwrap().shortcuts.bind);
        for (text, key) in [
            (
                "[voice]\nconnect_timeout_s = 0\n",
                "voice.connect_timeout_s",
            ),
            ("[voice]\nclose_wait_s = 0\n", "voice.close_wait_s"),
            ("[shortcuts]\nhold_ms = 0\n", "shortcuts.hold_ms"),
            ("[shortcuts]\nmax_hold_s = 0\n", "shortcuts.max_hold_s"),
            ("[voice]\nkeepalive_s = 0\n", "voice.keepalive_s"),
            ("[voice]\nsend_queue_s = 14\n", "voice.send_queue_s"),
            (
                "[voice]\nconnect_timeout_s = 40\n",
                "voice.connect_timeout_s, whose buffered audio",
            ),
            ("[voice]\nmode = \"ptt\"\n", "unknown variant `ptt`"),
        ] {
            let e = parse(text).unwrap_err().to_string();
            assert!(e.contains(key), "{text}: {e}");
        }
    }

    #[test]
    fn the_update_keys_have_their_defaults_and_are_read_and_checked() {
        let d = Config::default().update;
        assert!(d.background_check, "on by default");
        assert_eq!(
            (d.first_check_s, d.interval_s, d.timeout_s),
            (20, 6 * 60 * 60, 30)
        );
        let c = parse(
            "[update]\nbackground_check = false\nfirst_check_s = 0\ninterval_s = 60\ntimeout_s = 5\n",
        )
        .unwrap()
        .update;
        assert!(!c.background_check);
        assert_eq!(
            (c.first_check(), c.interval(), c.timeout()),
            (
                Duration::ZERO,
                Duration::from_secs(60),
                Duration::from_secs(5)
            )
        );
        for (text, key) in [
            ("[update]\ninterval_s = 0\n", "update.interval_s"),
            ("[update]\ntimeout_s = 0\n", "update.timeout_s"),
        ] {
            let e = parse(text).unwrap_err().to_string();
            assert!(e.contains(key) && e.contains("must be at least 1"), "{e}");
        }
        let e = parse("[update]\nchannel = \"beta\"\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown field `channel`"), "{e}");
    }

    #[test]
    fn a_wrong_type_is_an_error() {
        let e = parse("[wallet]\nretry_s = \"5\"\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("invalid type"), "{e}");
        assert!(e.contains("retry_s"), "{e}");
    }

    #[test]
    fn a_zero_limit_is_an_error_that_names_the_key() {
        for (text, key) in [
            ("[lmgw]\nretry_max_s = 0\n", "lmgw.retry_max_s"),
            ("[lmgw]\ntimeout_s = 0\n", "lmgw.timeout_s"),
            ("[wallet]\nretry_s = 0\n", "wallet.retry_s"),
            ("[wallet]\ntimeout_s = 0\n", "wallet.timeout_s"),
            (
                "[pairing]\nhandoff_timeout_s = 0\n",
                "pairing.handoff_timeout_s",
            ),
            ("[dbus]\ntimeout_s = 0\n", "dbus.timeout_s"),
        ] {
            let e = parse(text).unwrap_err().to_string();
            assert!(e.contains(key) && e.contains("must be at least 1"), "{e}");
        }
    }

    #[test]
    fn a_bad_url_is_an_error_that_names_the_key() {
        for url in ["ftp://x", "not a url", "http://h/?q=1", "http://user:pw@h"] {
            let e = parse(&format!("[lmgw]\nurl = {url:?}\n"))
                .unwrap_err()
                .to_string();
            assert!(e.contains("lmgw.url"), "{url}: {e}");
        }
    }

    #[test]
    fn set_lmgw_url_keeps_the_rest_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kai").join("config.toml");
        set_lmgw_url(&path, "http://127.0.0.1:1").unwrap();
        assert_eq!(
            Config::load(&path).unwrap().lmgw.url.as_deref(),
            Some("http://127.0.0.1:1")
        );

        std::fs::write(
            &path,
            "# mine\n[wallet]\nretry_s = 7 # keep\n\n[lmgw]\nurl = \"http://old:1\"\n",
        )
        .unwrap();
        set_lmgw_url(&path, "http://new:2").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("# mine") && text.contains("retry_s = 7 # keep"),
            "{text}"
        );
        let c = Config::load(&path).unwrap();
        assert_eq!(c.lmgw.url.as_deref(), Some("http://new:2"));
        assert_eq!(c.wallet.retry_s, 7);
    }
}
