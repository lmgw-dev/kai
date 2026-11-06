//! The catalog's entries, section by section, in the order the window shows them. The help lines
//! say what a setting does in the owner's words; the module docs of the sections' types
//! (`crate::config` and the ones it names) keep the full rules.

use super::{Control, Opt, Source};

/// One section and its keys.
pub(super) struct Section {
    pub id: &'static str,
    pub label: &'static str,
    pub specs: &'static [Spec],
}

/// One key (or a map's entries, `policy.tools.*`).
#[derive(Clone, Copy)]
pub(super) struct Spec {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub unit: Option<&'static str>,
    pub control: Kind,
    pub advanced: bool,
    pub group: Option<&'static str>,
}

/// [`Control`] as the table writes it.
#[derive(Clone, Copy)]
pub(super) enum Kind {
    Switch,
    Choice {
        options: &'static [(&'static str, &'static str)],
        unset: Option<&'static str>,
    },
    Number {
        min: Option<i64>,
        max: Option<i64>,
    },
    Text {
        unset: Option<&'static str>,
    },
    List,
    ReadOnly,
    /// Not shown at all (Kai's own bookkeeping).
    Hidden,
    /// One of the things Kai can list (devices, screens), by the name it has for each.
    Pick {
        from: Source,
        unset: &'static str,
    },
    SharedName {
        shared: &'static str,
        own: &'static str,
    },
}

impl Kind {
    pub(super) fn wire(&self) -> Control {
        match self {
            Kind::Switch => Control::Switch,
            Kind::Choice { options, unset } => Control::Choice {
                options: options
                    .iter()
                    .map(|(word, name)| Opt {
                        word: (*word).to_owned(),
                        name: (*name).to_owned(),
                    })
                    .collect(),
                unset: unset.map(str::to_owned),
            },
            Kind::Number { min, max } => Control::Number {
                min: *min,
                max: *max,
            },
            Kind::Text { unset } => Control::Text {
                unset: unset.map(str::to_owned),
            },
            Kind::List => Control::List,
            Kind::ReadOnly => Control::ReadOnly,
            Kind::Hidden => Control::Hidden,
            Kind::Pick { from, unset } => Control::Pick {
                from: *from,
                unset: (*unset).to_owned(),
            },
            Kind::SharedName { shared, own } => Control::SharedName {
                shared: (*shared).to_owned(),
                // An empty `own` is this computer's name, read when the window asks.
                own: if own.is_empty() {
                    super::own_conversation_name()
                } else {
                    (*own).to_owned()
                },
            },
        }
    }
}

const fn spec(key: &'static str, label: &'static str, control: Kind, help: &'static str) -> Spec {
    Spec {
        key,
        label,
        help,
        unit: None,
        control,
        advanced: false,
        group: None,
    }
}

impl Spec {
    const fn unit(self, unit: &'static str) -> Self {
        Self {
            unit: Some(unit),
            ..self
        }
    }
    const fn advanced(self) -> Self {
        Self {
            advanced: true,
            ..self
        }
    }
    const fn group(self, group: &'static str) -> Self {
        Self {
            group: Some(group),
            ..self
        }
    }
}

/// A number of at least `min` (the loader's rule), with no upper bound.
const fn at_least(min: i64) -> Kind {
    Kind::Number {
        min: Some(min),
        max: None,
    }
}

/// An unsigned number the loader takes whole (0 included).
const ANY: Kind = at_least(0);

const TEXT: Kind = Kind::Text { unset: None };

const fn optional_text(unset: &'static str) -> Kind {
    Kind::Text { unset: Some(unset) }
}

const fn choice(options: &'static [(&'static str, &'static str)]) -> Kind {
    Kind::Choice {
        options,
        unset: None,
    }
}

/// The nine anchors of the ring, the approval card and the overlays.
const ANCHORS: &[(&str, &str)] = &[
    ("top-left", "Top left"),
    ("top", "Top"),
    ("top-right", "Top right"),
    ("left", "Left"),
    ("center", "Center"),
    ("right", "Right"),
    ("bottom-left", "Bottom left"),
    ("bottom", "Bottom"),
    ("bottom-right", "Bottom right"),
];

const PROVIDERS: Kind = Kind::Choice {
    options: &[
        ("plasma", "Plasma"),
        ("fdo", "freedesktop"),
        ("none", "None (off)"),
    ],
    unset: Some("Automatic"),
};

/// The overlay kinds' tables share their keys; each kind is a group of its own.
macro_rules! kind {
    ($kind:literal, $group:literal, $height_min:expr, $height_help:expr) => {
        [
            spec(
                concat!("overlay.", $kind, ".position"),
                "Position",
                choice(ANCHORS),
                "Where it shows when the call names no position.",
            )
            .group($group),
            spec(
                concat!("overlay.", $kind, ".margin"),
                "Distance from the edge",
                ANY,
                "Space between it and the screen edges it sits at.",
            )
            .unit("px")
            .group($group),
            spec(
                concat!("overlay.", $kind, ".width"),
                "Width",
                at_least(1),
                "Fitted to the screen when it is larger.",
            )
            .unit("px")
            .group($group),
            spec(
                concat!("overlay.", $kind, ".height"),
                "Height",
                at_least($height_min),
                $height_help,
            )
            .unit("px")
            .group($group),
            spec(
                concat!("overlay.", $kind, ".timeout_s"),
                "Hide after",
                ANY,
                "How long it stays up; 0 keeps it until it is hidden.",
            )
            .unit("s")
            .group($group),
            spec(
                concat!("overlay.", $kind, ".interactive"),
                "Takes clicks",
                Kind::Switch,
                "When off, clicks go through to the window below.",
            )
            .group($group),
        ]
    };
}

const FIXED_HEIGHT: &str = "This kind cannot measure its content, so it needs a height.";

const TEXT_KIND: [Spec; 6] = kind!(
    "text",
    "Text overlay",
    0,
    "0 makes it as tall as its text (longer text scrolls)."
);
const HTML_KIND: [Spec; 6] = kind!("html", "Web overlay", 1, FIXED_HEIGHT);
const IMAGE_KIND: [Spec; 6] = kind!("image", "Picture overlay", 1, FIXED_HEIGHT);
const EMOTE_KIND: [Spec; 6] = kind!("emote", "Emote overlay", 1, FIXED_HEIGHT);
const WEATHER_KIND: [Spec; 6] = kind!("weather", "Weather card", 1, FIXED_HEIGHT);
const APP_KIND: [Spec; 6] = kind!("app", "App views", 1, FIXED_HEIGHT);

const CONVERSATION: &[Spec] = &[
    spec(
        "conversation.folder_name",
        "Conversation",
        Kind::SharedName {
            shared: "Kai",
            own: "",
        },
        "Kai's conversation lives in an lmgw Chat folder; a change switches to it at once.",
    ),
    spec(
        "conversation.folder_id",
        "Folder Kai found",
        Kind::Hidden,
        "The Chat folder of the conversation, which Kai remembers itself.",
    )
    .advanced(),
];

const VOICE: &[Spec] = &[
    spec(
        "voice.on_at_start",
        "Listen when Kai starts",
        Kind::Switch,
        "Kai begins listening as soon as it starts; when off, you switch listening on yourself.",
    ),
    spec(
        "voice.mode",
        "Listening mode",
        choice(&[("automatic", "Automatic"), ("push_to_talk", "Push-to-talk")]),
        "Automatic notices when you stop talking; push-to-talk sends audio only while the talk \
         key is held.",
    ),
    spec(
        "voice.conversation_timeout_s",
        "Conversation mode timeout",
        ANY,
        "Conversation mode ends after this long without speech; 0 never ends it by silence.",
    )
    .unit("s"),
    spec(
        "voice.grace_s",
        "Keep a hold-to-talk session open",
        ANY,
        "With listening off, a session you opened by holding the key closes this long after the \
         reply.",
    )
    .unit("s"),
    spec(
        "shortcuts.hold_ms",
        "Talk key hold time",
        at_least(1),
        "A shorter press toggles listening; a longer hold talks.",
    )
    .unit("ms"),
    spec(
        "shortcuts.max_hold_s",
        "Stuck key guard",
        at_least(1),
        "A hold that lasts longer is treated as a stuck key and its audio is dropped.",
    )
    .unit("s")
    .advanced(),
    spec(
        "shortcuts.bind",
        "Register global shortcuts",
        Kind::Switch,
        "When off, Kai registers no keyboard shortcuts with Plasma.",
    )
    .advanced(),
    spec(
        "voice.connect_timeout_s",
        "Connect timeout",
        at_least(1),
        "Audio spoken while the voice session connects is kept this long, then the connect is \
         given up.",
    )
    .unit("s")
    .advanced(),
    spec(
        "voice.close_wait_s",
        "Wait at quit",
        at_least(1),
        "How long Kai waits for lmgw to take the session's end when it quits.",
    )
    .unit("s")
    .advanced(),
    spec(
        "voice.keepalive_s",
        "Keep-alive interval",
        at_least(1),
        "Kai pings the voice connection this often; two silent intervals end it.",
    )
    .unit("s")
    .advanced(),
    spec(
        "voice.send_queue_s",
        "Outgoing audio buffer",
        Kind::Number {
            min: None,
            max: None,
        },
        "Microphone audio queued for lmgw; at least as long as the connect timeout.",
    )
    .unit("s")
    .advanced(),
];

const AUDIO: &[Spec] = &[
    spec(
        "audio.echo_mode",
        "Echo handling",
        choice(&[
            ("device", "Input device cancels echo"),
            ("app", "Kai cancels echo"),
            ("headphones", "Headphones"),
            ("half_duplex", "Half duplex"),
        ]),
        "How Kai keeps its own voice out of the microphone. Half duplex ignores the microphone \
         while Kai speaks.",
    ),
    spec(
        "audio.input",
        "Microphone",
        Kind::Pick {
            from: Source::Microphones,
            unset: "System default",
        },
        "The microphone Kai listens with; the tray's menu picks it too.",
    ),
    spec(
        "audio.output",
        "Speaker",
        Kind::Pick {
            from: Source::Speakers,
            unset: "System default",
        },
        "The speaker Kai talks through; the tray's menu picks it too.",
    ),
    spec(
        "audio.capture_ring_ms",
        "Microphone buffer",
        at_least(200),
        "Microphone audio held before it is dropped.",
    )
    .unit("ms")
    .advanced(),
    spec(
        "audio.playback_ring_ms",
        "Playback buffer",
        at_least(200),
        "Reply audio handed ahead to the speaker.",
    )
    .unit("ms")
    .advanced(),
    spec(
        "audio.flush_wait_ms",
        "Interruption wait",
        at_least(1),
        "How long an interruption waits for playback to stop.",
    )
    .unit("ms")
    .advanced(),
    spec(
        "audio.retry_s",
        "Retry PipeWire every",
        at_least(1),
        "While PipeWire cannot be reached, Kai tries again this often.",
    )
    .unit("s")
    .advanced(),
    spec(
        "aec.ready_timeout_s",
        "Echo canceller start timeout",
        at_least(1),
        "How long Kai waits for its echo canceller to come up.",
    )
    .unit("s")
    .advanced(),
    spec(
        "aec.restore_timeout_s",
        "Device restore timeout",
        at_least(1),
        "How long Kai waits for PipeWire to restore the default devices.",
    )
    .unit("s")
    .advanced(),
];

const RING: &[Spec] = &[
    spec(
        "ring.anchor",
        "Position",
        choice(ANCHORS),
        "Where the voice ring sits on the screen.",
    ),
    spec(
        "ring.margin",
        "Distance from the edge",
        ANY,
        "Space between the ring and the screen edges it sits at.",
    )
    .unit("px"),
    spec("ring.size", "Size", at_least(1), "The ring's square size.").unit("px"),
    spec(
        "ring.output",
        "Screen",
        Kind::Pick {
            from: Source::Screens,
            unset: "Active screen",
        },
        "Which screen the ring shows on; by default the one KWin has active.",
    ),
    spec(
        "overlay.ring_over_fullscreen",
        "Show above fullscreen windows",
        Kind::Switch,
        "When off, fullscreen windows cover the ring.",
    ),
];

const OVERLAYS: &[Spec] = &[
    spec(
        "overlay.output",
        "Screen",
        Kind::Pick {
            from: Source::Screens,
            unset: "Active screen",
        },
        "Which screen overlays show on; by default the one KWin has active at each show.",
    ),
    spec(
        "overlay.fetch_private_addresses",
        "Fetch from local network addresses",
        Kind::Switch,
        "When off, pictures and weather are never fetched from this computer or your local network.",
    ),
    TEXT_KIND[0],
    TEXT_KIND[1],
    TEXT_KIND[2],
    TEXT_KIND[3],
    TEXT_KIND[4],
    TEXT_KIND[5],
    HTML_KIND[0],
    HTML_KIND[1],
    HTML_KIND[2],
    HTML_KIND[3],
    HTML_KIND[4],
    HTML_KIND[5],
    IMAGE_KIND[0],
    IMAGE_KIND[1],
    IMAGE_KIND[2],
    IMAGE_KIND[3],
    IMAGE_KIND[4],
    IMAGE_KIND[5],
    EMOTE_KIND[0],
    EMOTE_KIND[1],
    EMOTE_KIND[2],
    EMOTE_KIND[3],
    EMOTE_KIND[4],
    EMOTE_KIND[5],
    WEATHER_KIND[0],
    WEATHER_KIND[1],
    WEATHER_KIND[2],
    WEATHER_KIND[3],
    WEATHER_KIND[4],
    WEATHER_KIND[5],
    APP_KIND[0],
    APP_KIND[1],
    APP_KIND[2],
    APP_KIND[3],
    APP_KIND[4],
    APP_KIND[5],
    spec(
        "overlay.app.auto_show",
        "Show app views by themselves",
        Kind::Switch,
        "A tool call in Kai's conversation that has a view shows it on its own; when off, only on \
         request.",
    )
    .group("App views"),
    spec(
        "weather.places.*",
        "{}",
        TEXT,
        "A saved place for the weather card, as latitude,longitude.",
    )
    .group("Weather places"),
    spec(
        "overlay.show_timeout_s",
        "Show timeout",
        at_least(3),
        "How long an overlay may take to appear before the call fails.",
    )
    .unit("s")
    .advanced(),
    spec(
        "overlay.unresponsive_s",
        "Unresponsive after",
        at_least(1),
        "An overlay page that does not answer Kai's ping this long is closed.",
    )
    .unit("s")
    .advanced(),
    spec(
        "overlay.image_max_mb",
        "Largest picture",
        at_least(1),
        "The most a picture may download or unpack to.",
    )
    .unit("MB")
    .advanced(),
    spec(
        "overlay.fetch_timeout_s",
        "Fetch timeout",
        at_least(1),
        "The whole deadline of a picture or weather fetch.",
    )
    .unit("s")
    .advanced(),
    spec(
        "overlay.app_max_mb",
        "Largest app message",
        at_least(1),
        "The most one message of an app view, or lmgw's answer to it, may be.",
    )
    .unit("MB")
    .advanced(),
    spec(
        "overlay.app_max_calls",
        "App view requests at once",
        at_least(1),
        "The most requests one app view may have waiting.",
    )
    .advanced(),
];

const APPROVALS: &[Spec] = &[
    spec(
        "approvals.anchor",
        "Position",
        choice(ANCHORS),
        "Where the approval card sits on the overlays' screen.",
    ),
    spec(
        "approvals.margin",
        "Distance from the edge",
        ANY,
        "Space between the card and the screen edges it sits at.",
    )
    .unit("px"),
    spec(
        "approvals.width",
        "Width",
        at_least(1),
        "The card's width; its height follows its content.",
    )
    .unit("px"),
    spec(
        "approvals.yes",
        "Spoken phrases that approve",
        Kind::List,
        "One phrase per line; your whole answer must be one of them. Empty restores the defaults.",
    ),
    spec(
        "approvals.no",
        "Spoken phrases that decline",
        Kind::List,
        "One phrase per line, none of them also an approving one. Empty restores the defaults.",
    ),
    spec(
        "approvals.arm_ms",
        "Arming delay",
        ANY,
        "The card takes no key, click or spoken answer until this long has passed without input.",
    )
    .unit("ms")
    .advanced(),
    spec(
        "approvals.speech_dbfs",
        "Speech level",
        Kind::Number {
            min: Some(-120),
            max: Some(0),
        },
        "How loud a spoken answer must be to count as speech.",
    )
    .unit("dBFS")
    .advanced(),
    spec(
        "approvals.speech_ms",
        "Speech needed",
        ANY,
        "How much speech a spoken answer needs before it is sent; 0 skips the check.",
    )
    .unit("ms")
    .advanced(),
];

const HELPERS: &[Spec] = &[
    spec(
        "delegate.screenshots",
        "Who looks at screenshots",
        choice(&[
            ("main", "The calling model"),
            ("delegate", "Only the look helper"),
        ]),
        "With the look helper, the calling model needs no vision and gets the screenshot as text.",
    ),
    spec(
        "delegate.look",
        "Model for looking at the screen",
        optional_text("The conversation's model"),
        "An lmgw alias with vision; empty uses the Kai conversation's model.",
    ),
    spec(
        "delegate.find",
        "Model for finding things",
        optional_text("The conversation's model"),
        "An lmgw alias; empty uses the Kai conversation's model.",
    ),
    spec(
        "delegate.max_turns",
        "Most turns per task",
        ANY,
        "A helper task stops as partial after this many model turns; 0 sets no limit.",
    )
    .advanced(),
];

const ROUTINES: &[Spec] = &[
    spec(
        "routines.create_alias",
        "Model that writes routines",
        optional_text("The conversation's model"),
        "An lmgw alias, ideally a capable coding model; empty uses the Kai conversation's model.",
    ),
    spec(
        "routines.run_timeout_s",
        "Stop a run after",
        ANY,
        "A routine started from the tray or a key is stopped after this long; 0 lets it run.",
    )
    .unit("s"),
    spec(
        "routines.enabled.*",
        "Routine {}",
        Kind::ReadOnly,
        "Switch routines on from the tray's Routines menu, which shows them in full first.",
    ),
    spec(
        "jobs.quiet_ms",
        "Quiet time before a result is spoken",
        ANY,
        "A finished job's result waits until nothing else has gone on this long.",
    )
    .unit("ms"),
    spec(
        "jobs.keep_ended_s",
        "Keep unconfirmed results",
        ANY,
        "A finished job lmgw never confirmed is dropped after this long; 0 keeps it until Kai \
         quits.",
    )
    .unit("s")
    .advanced(),
];

const DESKTOP_USE: &[Spec] = &[
    spec(
        "launch.terminal",
        "Terminal for terminal apps",
        Kind::List,
        "The command that runs an app needing a terminal, one word per line (konsole, -e); empty \
         uses Plasma's terminal.",
    ),
    spec(
        "search.limit",
        "Most files found",
        ANY,
        "The most matches a file search returns; 0 returns every match. The result says when there were more.",
    ),
    spec(
        "accessibility.switch",
        "Switch accessibility on",
        choice(&[
            ("conversation", "During conversation mode and reads"),
            ("read", "Only while reading a window"),
            ("never", "Never"),
        ]),
        "Reading a window needs the desktop's accessibility switched on; Kai puts it back after.",
    ),
    spec(
        "accessibility.wait_ms",
        "Wait for an app to join",
        ANY,
        "After switching accessibility on, how long a read waits for the window's app.",
    )
    .unit("ms")
    .advanced(),
    spec(
        "accessibility.concurrency",
        "Parallel accessibility calls",
        ANY,
        "Calls in flight at once while reading a window; 0 sets no limit.",
    )
    .advanced(),
    spec(
        "accessibility.action_settle_ms",
        "Wait for an element's actions",
        ANY,
        "An element whose actions are not ready when Kai first asks (Firefox's links and buttons, \
         sometimes) is asked again this long after, before a press gives up on it.",
    )
    .unit("ms")
    .advanced(),
    spec(
        "launch.command_timeout_s",
        "Command time limit",
        ANY,
        "Stops a command sooner than its call's own deadline; 0 sets no limit of its own.",
    )
    .unit("s")
    .advanced(),
    spec(
        "launch.output_max_mb",
        "Command output limit",
        ANY,
        "Cuts a command's output sooner than the link's own limit; 0 sets no limit of its own.",
    )
    .unit("MB")
    .advanced(),
];

const TRUST: &[Spec] = &[
    spec(
        "policy.trusted_callers",
        "Trusted callers",
        Kind::List,
        "Callers treated like you for read-only tools and overlays, one per line: a kind and the \
         name lmgw shows for it, such as device:phone, key:laptop, agent:helper, routine:morning \
         or app:notes; or cli.",
    ),
    spec(
        "policy.trusted_approvers",
        "Trusted approvers",
        Kind::List,
        "One per line, kind:name; an approval they give in a thread is not asked again.",
    ),
    spec(
        "policy.config_tools",
        "Offer configuration tools",
        Kind::Switch,
        "Lets you and Kai's own key read and change these settings by tool call.",
    ),
    spec(
        "policy.widen_trust_by_tools",
        "Let configuration tools widen trust",
        Kind::Switch,
        "When off, the configuration tools cannot add trusted callers or approvers or switch \
         themselves on.",
    ),
];

const UPDATES: &[Spec] = &[
    spec(
        "update.background_check",
        "Check for updates in the background",
        Kind::Switch,
        "When off, only the tray's Check for Updates looks.",
    ),
    spec(
        "update.first_check_s",
        "First check after start",
        ANY,
        "How long after the start the first background check runs.",
    )
    .unit("s")
    .advanced(),
    spec(
        "update.interval_s",
        "Check every",
        at_least(1),
        "How often the background check runs after the first.",
    )
    .unit("s")
    .advanced(),
    spec(
        "update.timeout_s",
        "Download timeout",
        at_least(1),
        "How long the update feed may stay silent during a check or download.",
    )
    .unit("s")
    .advanced(),
];

const CONNECTION: &[Spec] = &[
    spec(
        "lmgw.url",
        "Address from an old pairing",
        Kind::ReadOnly,
        "Written by a pairing from before encrypted links; pairing again removes it.",
    )
    .advanced(),
    spec(
        "lmgw.retry_max_s",
        "Longest reconnect wait",
        at_least(1),
        "After a drop Kai reconnects after 1 s, doubling the wait up to this.",
    )
    .unit("s")
    .advanced(),
    spec(
        "lmgw.timeout_s",
        "Answer timeout",
        at_least(1),
        "How long lmgw may take to answer the feed and send its first bytes.",
    )
    .unit("s")
    .advanced(),
    spec(
        "lmgw.url_stagger_ms",
        "Try the next address after",
        ANY,
        "With several lmgw addresses, how long one may take before the next is tried beside it; \
         0 tries all at once.",
    )
    .unit("ms")
    .advanced(),
    spec(
        "host.keepalive_s",
        "Tool link keep-alive",
        at_least(1),
        "Kai pings its tool link to lmgw this often; two silent intervals end it.",
    )
    .unit("s")
    .advanced(),
    spec(
        "host.fallback_max_message_mb",
        "Largest tool message",
        ANY,
        "Used only with an lmgw that announces no limit; 0 sets none.",
    )
    .unit("MB")
    .advanced(),
    spec(
        "host.fallback_max_frame_mb",
        "Largest tool frame",
        ANY,
        "Used only with an lmgw that announces no limit; 0 sets none (not both 0).",
    )
    .unit("MB")
    .advanced(),
    spec(
        "window.proxy_max_mb",
        "Largest request through Kai",
        ANY,
        "The most a request that Kai forwards to lmgw (this window's Personality page, Firefox's \
         sidebar) may be, and lmgw's answer when Kai takes it whole. An answer that streams (a \
         reply as it is written, the sidebar's live updates) is passed on as it comes and has no \
         such limit. 0 sets no limit.",
    )
    .unit("MB")
    .advanced(),
    spec(
        "window.proxy_timeout_s",
        "Timeout for requests through Kai",
        ANY,
        "How long lmgw may take to answer a request that Kai forwards (this window's Personality \
         page, Firefox's sidebar); for an answer that streams, only until it starts, so a long \
         reply or the sidebar's live updates stay open. 0 leaves it to lmgw.",
    )
    .unit("s")
    .advanced(),
    spec(
        "wallet.retry_s",
        "Retry the wallet every",
        at_least(1),
        "While the wallet is locked or fails, Kai checks again this often.",
    )
    .unit("s")
    .advanced(),
    spec(
        "wallet.timeout_s",
        "Wallet timeout",
        at_least(1),
        "How long one wallet call may take.",
    )
    .unit("s")
    .advanced(),
    spec(
        "pairing.handoff_timeout_s",
        "Pairing link handoff timeout",
        at_least(1),
        "How long a pairing link opened from the browser waits for Kai to take it.",
    )
    .unit("s")
    .advanced(),
    spec(
        "dbus.timeout_s",
        "Desktop call timeout",
        at_least(1),
        "How long one session-bus call may take, such as a notification.",
    )
    .unit("s")
    .advanced(),
];

/// The Tools page's settings: the policy table edits them, not the Settings page.
const TOOLS: &[Spec] = &[
    spec(
        "policy.tools.*",
        "When {} is called",
        Kind::Choice {
            options: &[
                ("allow", "Allow"),
                ("allow_trusted", "Allow if trusted"),
                ("confirm", "Confirm"),
                ("deny", "Deny"),
                ("off", "Off"),
            ],
            unset: Some("Default"),
        },
        "What happens when the tool is called.",
    ),
    spec(
        "policy.levels.*",
        "Who may call {}",
        Kind::Choice {
            options: &[
                ("own", "Kai's own key"),
                ("owner", "You"),
                ("trusted", "Trusted callers"),
                ("any", "Any caller"),
            ],
            unset: Some("Default"),
        },
        "Who may call the tool at all.",
    ),
    spec(
        "policy.widen_by_tools.*",
        "Configuration tools may widen {}",
        Kind::Switch,
        "When off, the configuration tools can only narrow this tool's policy.",
    ),
];

/// The Desktop page's settings: its provider table edits them.
const DESKTOP: &[Spec] = &[
    spec(
        "desktop.windows",
        "Windows",
        PROVIDERS,
        "Which provider lists and moves windows.",
    ),
    spec(
        "desktop.capture",
        "Screenshots",
        PROVIDERS,
        "Which provider takes screenshots.",
    ),
    spec(
        "desktop.accessibility",
        "Accessibility",
        PROVIDERS,
        "Which provider reads windows' contents.",
    ),
    spec(
        "desktop.launch",
        "Launching",
        PROVIDERS,
        "Which provider opens apps and files.",
    ),
    spec(
        "desktop.search",
        "Search",
        PROVIDERS,
        "Which provider finds files.",
    ),
    spec(
        "desktop.clipboard",
        "Clipboard",
        PROVIDERS,
        "Which provider reads the clipboard.",
    ),
    spec(
        "desktop.media",
        "Media",
        PROVIDERS,
        "Which provider controls media players.",
    ),
    spec(
        "desktop.audio",
        "Audio",
        PROVIDERS,
        "Which provider controls volume.",
    ),
    spec(
        "desktop.session",
        "Session",
        PROVIDERS,
        "Which provider locks the screen and asks questions.",
    ),
    spec(
        "desktop.notify",
        "Notifications",
        PROVIDERS,
        "Which provider shows notifications.",
    ),
    spec(
        "desktop.input",
        "Input",
        PROVIDERS,
        "Which provider types and presses keys.",
    ),
];

const BROWSER: &[Spec] = &[
    spec(
        "browser.hands_and_eyes",
        "Kai may act in Firefox without asking",
        Kind::Switch,
        "Kai's tools that switch tabs, act on pages and read aloud run unasked for you and trusted \
         callers (attaching a file is still asked about), and the tools made for using Firefox by \
         voice are offered; a tool's own setting on the Tools page still wins. Switching this on also \
         turns on announcing what changes on the page, so you hear what Kai does; you can turn \
         that off again below.",
    ),
    spec(
        "browser.act_in_background",
        "Act in background tabs without asking",
        Kind::Switch,
        "Kai's tools that act on a page follow their setting on every tab; when off, they do so \
         only on the tab in front of you that you brought there yourself, and ask elsewhere.",
    ),
    spec(
        "browser.point_ms",
        "Pointing ring",
        Kind::Number {
            min: Some(1),
            max: Some(crate::browser::settings::POINT_MS_MAX as i64),
        },
        "When Kai points at something on a page to ask \"this one?\", the ring shows this long \
         and fades out over its last third.",
    )
    .unit("ms"),
    spec(
        "browser.pdf_timeout_s",
        "PDF reading time limit",
        at_least(1),
        "Kai reads a PDF's text with poppler's pdftotext; a PDF that takes longer is given up and \
         the tool says so.",
    )
    .unit("s")
    .advanced(),
    spec(
        "browser.pdf_text_max_mb",
        "PDF text limit",
        at_least(1),
        "The most text Kai takes from one PDF; a PDF whose text is longer (a broken or hostile \
         one can be far longer than the file) is given up and the tool says so.",
    )
    .unit("MB")
    .advanced(),
    spec(
        "browser.pdf_memory_max_mb",
        "PDF reading memory limit",
        at_least(1),
        "The memory pdftotext may use for one PDF; a PDF that needs more is given up and the tool \
         says so, instead of the system running out of memory.",
    )
    .unit("MB")
    .advanced(),
    spec(
        "browser.commit_guard",
        "Ask before committing",
        Kind::Switch,
        "Kai asks you before it presses or submits something whose name or form holds one of the \
         words below, before it sends a form to another site, and before it goes to a site your \
         history has never seen; the page's writes right after each action wait for your answer, \
         with what they send.",
    ),
    spec(
        "browser.guard_words",
        "Words that ask",
        Kind::List,
        "One per line, case ignored. A word of four or more letters matches anywhere in what the \
         element says, also inside a word, so a stem covers its forms and compounds (bestell: \
         Bestellung, zahlungspflichtig bestellen); a shorter one only where a word starts (buy in \
         QuickBuy, not in Ebuy).",
    ),
    spec(
        "browser.guard_window_ms",
        "Watch the page's requests for",
        Kind::Number {
            min: Some(0),
            max: Some(crate::browser::settings::CONTRACT_MS_MAX as i64),
        },
        "How long after an action the page's requests that send or change something wait for your \
         answer; 0 watches only what the action itself sends.",
    )
    .unit("ms"),
    spec(
        "browser.input_settle_ms",
        "Kai's keys settle for",
        Kind::Number {
            min: Some(0),
            max: Some(crate::browser::settings::CONTRACT_MS_MAX as i64),
        },
        "After Kai types or presses keys through the desktop, Firefox still counts input as Kai's \
         for this long, for keys still on their way, so they never pass for yours.",
    )
    .unit("ms")
    .advanced(),
    spec(
        "browser.see_settle_ms",
        "A tab settles on screen for",
        Kind::Number {
            min: Some(0),
            max: Some(crate::browser::settings::CONTRACT_MS_MAX as i64),
        },
        "While some site is kept from Kai, a Firefox window whose tab in front changed this \
         recently, or is still loading, counts as showing a site Kai cannot tell when Kai takes a \
         screenshot of the desktop: the window may still show the page before. Such a window is \
         blanked out, or not captured.",
    )
    .unit("ms")
    .advanced(),
    spec(
        "browser.trusted_press",
        "Real key presses",
        choice(&[
            ("when_needed", "When the page needs one"),
            ("always", "For every press"),
        ]),
        "Kai presses Enter or Space through the desktop, with Firefox in front, where a page takes \
         only a real key (a link that opens a new window, a media or full-screen control); every \
         press that way helps on sites that ignore Kai's synthetic clicks.",
    ),
    spec(
        "browser.announce",
        "Announce what changes on the page",
        Kind::Switch,
        "Kai says in its own voice what changed in your tab (a page loaded, a dialog opened, an \
         error at a field, what the page reports, a finished download, where the focus went), \
         with no model turn and nothing written into a chat; the page's own words are always \
         said as the page's. Only while Firefox has the focused window or conversation mode is \
         on, never over Kai's own speech. Off by itself: switching on \"Kai may act in Firefox \
         without asking\" turns it on once, and you can turn it off again.",
    )
    .group("Announcements"),
    spec(
        "browser.announce_quiet_ms",
        "Wait for a changing region to settle",
        Kind::Number {
            min: Some(0),
            max: Some(crate::browser::settings::CONTRACT_MS_MAX as i64),
        },
        "A part of the page that changes quickly (a ticker, a countdown) or a field's error is \
         announced once it has been quiet this long, with its latest text only.",
    )
    .unit("ms")
    .group("Announcements"),
    spec(
        "browser.announce_keep_s",
        "Keep announcements for Kai's look",
        at_least(1),
        "When a model asks where you are in Firefox, it gets the announcements since it last \
         asked; older ones than this are dropped from that list, which says how many were.",
    )
    .unit("s")
    .advanced()
    .group("Announcements"),
    spec(
        "browser.announce_max_chars",
        "Longest page text to announce",
        Kind::Number {
            min: Some(0),
            max: None,
        },
        "The most characters of the page's text one announcement speaks (a whole chat log can \
         change at once); Kai then says how much was left unspoken. 0 speaks all of it. A model \
         asking where you are always gets the whole text.",
    )
    .unit("characters")
    .advanced()
    .group("Announcements"),
    spec(
        "browser.announce_max_age_s",
        "Drop announcements this old",
        at_least(1),
        "An announcement that could not be spoken this long after the page changed (Kai was \
         speaking, do not disturb was on, the sound system was down) is dropped unspoken when \
         its turn comes; a model asking where you are learns how many were.",
    )
    .unit("s")
    .advanced()
    .group("Announcements"),
    spec(
        "browser.speech_timeout_s",
        "Speech time limit",
        at_least(1),
        "How long lmgw may take to make the speech of one sentence of an announcement or a \
         paragraph, a voice model that has to load first included; past it that speech is given \
         up: a reading pauses and says so. Takes effect at Kai's next start.",
    )
    .unit("s")
    .advanced()
    .group("Announcements"),
    spec(
        "browser.speech_quiet_ms",
        "Quiet time before Kai speaks by itself",
        Kind::Number {
            min: Some(0),
            max: Some(crate::browser::settings::CONTRACT_MS_MAX as i64),
        },
        "Announcements and reading aloud start only after nothing else has gone on this long \
         (Kai's reply, your speech, a step of Kai's work), so a short pause between two steps does \
         not start one that the next reply cuts again. Takes effect at Kai's next start.",
    )
    .unit("ms")
    .advanced()
    .group("Announcements"),
    spec(
        "browser.speech_echo_guard_ms",
        "Echo guard after Kai speaks by itself",
        Kind::Number {
            min: Some(0),
            max: Some(crate::browser::settings::CONTRACT_MS_MAX as i64),
        },
        "After an announcement or a paragraph stops, the microphone sends silence this long, so \
         its echo cannot become something you said. Speaking while it plays, or within this \
         long, stops it; say what you want after that. Takes effect at Kai's next start.",
    )
    .unit("ms")
    .advanced()
    .group("Announcements"),
    spec(
        "browser.sidebar_ahead_mb",
        "Sent ahead of the sidebar",
        ANY,
        "How much of one answer from lmgw Kai sends Firefox's sidebar before the sidebar has \
         taken it in. Beyond it Kai waits, and reads no more of the answer from lmgw meanwhile, so \
         a large download or a fast stream never piles up in Firefox. 0 sets no limit.",
    )
    .unit("MB")
    .advanced()
    .group("Sidebar"),
    spec(
        "browser.read.hyphens",
        "Join hyphenated words",
        Kind::Switch,
        "Reading aloud joins words broken across lines (infor-mation) and drops soft hyphens and \
         invisible characters.",
    )
    .group("Reading aloud"),
    spec(
        "browser.read.markdown",
        "Leave out formatting marks",
        Kind::Switch,
        "Reading aloud leaves out marks such as **bold**, *italic*, # headings, bullets and table \
         pipes, and reads a link written as [text](address) as its text.",
    )
    .group("Reading aloud"),
    spec(
        "browser.read.citations",
        "Leave out reference marks",
        Kind::Switch,
        "Reading aloud leaves out marks such as [23], [12–15], [^1], [citation needed] and [edit].",
    )
    .group("Reading aloud"),
    spec(
        "browser.read.superscripts",
        "Leave out superscript digits",
        Kind::Switch,
        "Reading aloud leaves out ¹²³ and the other superscript digits; off by default, since \
         they carry meaning in units such as m² and cm³.",
    )
    .group("Reading aloud"),
    spec(
        "browser.read.urls",
        "Leave out web and e-mail addresses",
        Kind::Switch,
        "Reading aloud leaves out addresses, which a voice would spell out character by character.",
    )
    .group("Reading aloud"),
    spec(
        "browser.read.symbols",
        "Leave out symbols",
        Kind::Switch,
        "Reading aloud leaves out emoji, ★ ▪ • bullets, arrows and box-drawing characters.",
    )
    .group("Reading aloud"),
    spec(
        "browser.read.voices",
        "Voices for other languages",
        Kind::List,
        "One per line: a language, lmgw's text-to-speech model for it and optionally one of its \
         voices, such as \"de tts-german\" or \"fr kokoro siwis\" (pt also covers \
         pt-BR). A paragraph in a language without a line is read in Kai's own voice.",
    )
    .group("Reading aloud"),
    spec(
        "browser.sites.act",
        "Sites Kai may act on",
        Kind::List,
        "Exceptions to a wider rule below, one site per line: a host such as mail.example, \
         *.example for a domain and its subdomains, or an origin such as \
         https://shop.example:8443; the most specific line decides.",
    )
    .group("Site rules"),
    spec(
        "browser.sites.read",
        "Sites Kai may only read",
        Kind::List,
        "One site per line; Kai's tools that act on a page are refused there.",
    )
    .group("Site rules"),
    spec(
        "browser.sites.none",
        "Sites Kai does not reach",
        Kind::List,
        "One site per line; no tool of Kai's reads them, acts on them or names their tabs.",
    )
    .group("Site rules"),
    spec(
        "browser.peer_scopes",
        "Firefox's app scopes",
        Kind::List,
        "Kai for Firefox links to Kai only from a Firefox whose systemd unit matches one of these, \
         one per line, where * stands for any text and ? for one character; a Firefox started \
         from a terminal runs in the terminal's unit, and Kai's log names a refused one.",
    ),
];

/// Every section in the window's order. `tools` and `desktop` belong to their own pages.
pub(super) const SECTIONS: &[Section] = &[
    Section {
        id: "conversation",
        label: "Conversation",
        specs: CONVERSATION,
    },
    Section {
        id: "voice",
        label: "Voice",
        specs: VOICE,
    },
    Section {
        id: "audio",
        label: "Audio",
        specs: AUDIO,
    },
    Section {
        id: "ring",
        label: "Voice ring",
        specs: RING,
    },
    Section {
        id: "overlays",
        label: "Overlays",
        specs: OVERLAYS,
    },
    Section {
        id: "approvals",
        label: "Approvals",
        specs: APPROVALS,
    },
    Section {
        id: "helpers",
        label: "Helpers",
        specs: HELPERS,
    },
    Section {
        id: "routines",
        label: "Routines and jobs",
        specs: ROUTINES,
    },
    Section {
        id: "desktop_use",
        label: "Reading and running",
        specs: DESKTOP_USE,
    },
    Section {
        id: "trust",
        label: "Trust",
        specs: TRUST,
    },
    Section {
        id: "browser",
        label: "Firefox",
        specs: BROWSER,
    },
    Section {
        id: "updates",
        label: "Updates",
        specs: UPDATES,
    },
    Section {
        id: "connection",
        label: "Connection",
        specs: CONNECTION,
    },
    Section {
        id: "tools",
        label: "Tools",
        specs: TOOLS,
    },
    Section {
        id: "desktop",
        label: "Desktop",
        specs: DESKTOP,
    },
];

/// Every entry with its place in the order and its section.
pub(super) fn all() -> impl Iterator<Item = (u32, &'static Section, &'static Spec)> {
    SECTIONS
        .iter()
        .flat_map(|s| s.specs.iter().map(move |spec| (s, spec)))
        .enumerate()
        .map(|(i, (s, spec))| (i as u32, s, spec))
}

/// `key`'s entry: its own, or its map's pattern (`policy.tools.*` for `policy.tools.key_press`).
pub(super) fn find(key: &str) -> Option<(u32, &'static Section, &'static Spec)> {
    let pattern = key.rsplit_once('.').map(|(table, _)| format!("{table}.*"));
    all()
        .find(|(_, _, s)| s.key == key)
        .or_else(|| all().find(|(_, _, s)| Some(s.key) == pattern.as_deref()))
}
