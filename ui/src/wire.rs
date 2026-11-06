//! What the page and Kai say to each other, over `kai://settings/api/…` (WP21b). These are the
//! frontend's copies of `src/settings`'s serde types (`Request`, `Response`, `Entry`, the
//! catalog's `Meta`) and of the scheme handler's `ToolInfo` and `Conversation`, with words for the
//! enums; Kai's side compiles this very file in its tests (`src/settings/window/tests.rs`) and
//! reads its real answers with it, so a change of shape on either side fails there.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One setting as the owner sees it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Entry {
    pub key: String,
    /// The effective value; `null` for an optional setting that is not set.
    pub value: Value,
    /// The file has the key; otherwise the value is the default.
    pub set: bool,
    /// What the page shows of it, from Kai's settings catalog: never the key as its label.
    pub meta: Meta,
}

/// A setting's words and control (`src/settings/catalog.rs`'s `Meta`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Meta {
    /// The section's id (`voice`) and heading (`Voice`).
    pub section: String,
    pub section_label: String,
    /// A heading inside the section ("Text overlay").
    #[serde(default)]
    pub group: Option<String>,
    pub label: String,
    pub help: String,
    /// "s", "ms", "MB", "px", "dBFS", after the control.
    #[serde(default)]
    pub unit: Option<String>,
    pub control: Control,
    /// Rare tuning, under the section's "Advanced" heading.
    pub advanced: bool,
    /// The catalog's order.
    pub order: u32,
    /// What the setting is when the file does not set it (`null` for an optional one).
    #[serde(default)]
    pub default: Value,
}

/// How a setting is edited.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Control {
    Switch,
    /// Fixed words with their names; `unset` names the state without a value ("Automatic").
    Choice {
        options: Vec<Opt>,
        #[serde(default)]
        unset: Option<String>,
    },
    Number {
        #[serde(default)]
        min: Option<i64>,
        #[serde(default)]
        max: Option<i64>,
    },
    /// One line; `unset` says what an empty field means.
    Text {
        #[serde(default)]
        unset: Option<String>,
    },
    /// Texts, one per line.
    List,
    /// Kai's own flows write it: shown, not edited.
    ReadOnly,
    /// Kai's own bookkeeping: not shown.
    Hidden,
    /// One of the things Kai lists (`GET /api/devices`); `unset` names none picked.
    Pick {
        from: Source,
        unset: String,
    },
    /// The name every Kai client shares, or this computer's own (the conversation).
    SharedName {
        shared: String,
        own: String,
    },
}

/// What a [`Control::Pick`] lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Microphones,
    Speakers,
    Screens,
}

/// A thing to pick: the identifier the configuration stores, and the name it is shown under.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Choice {
    pub name: String,
    pub label: String,
}

/// `GET /api/devices`: the microphones, speakers and screens Kai can list. A list Kai could not
/// read is empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Devices {
    pub microphones: Vec<Choice>,
    pub speakers: Vec<Choice>,
    pub screens: Vec<Choice>,
}

impl Devices {
    pub fn of(&self, from: Source) -> &[Choice] {
        match from {
            Source::Microphones => &self.microphones,
            Source::Speakers => &self.speakers,
            Source::Screens => &self.screens,
        }
    }
}

/// One word of a choice and its name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Opt {
    pub word: String,
    pub name: String,
}

/// The sections that have pages of their own (the policy table, the providers): the Settings page
/// leaves them out.
pub const OWN_PAGE_SECTIONS: [&str; 2] = ["tools", "desktop"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Now,
    Restart,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    Entries { entries: Vec<Entry> },
    Entry { entry: Entry },
    Changed { entry: Entry, effect: Effect },
    Unchanged { entry: Entry },
}

/// The operations the page asks for (`POST /api/apply`). `None` as a word removes the key.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    List {
        section: Option<String>,
    },
    Set {
        key: String,
        value: String,
    },
    Unset {
        key: String,
    },
    ToolState {
        tool: String,
        state: Option<String>,
    },
    ToolLevel {
        tool: String,
        level: Option<String>,
    },
    Provider {
        capability: String,
        provider: Option<String>,
    },
}

/// The refusal's body (HTTP 422).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Refused {
    pub refused: String,
}

/// One tool Kai could offer (`GET /api/tools`): the contract's facts, which no setting changes.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    /// The contract file the tool is in (`windows`, `read`, `config`, …).
    pub area: String,
    pub description: String,
    pub read_only: bool,
    /// What happens with no `policy.tools` entry, in a few words (`confirm`, `allow if trusted`:
    /// the trusted callers are let through, every other caller is asked about).
    pub default_state: String,
    /// The contract's level word (`own`, `owner`, `trusted`, `any`).
    pub default_level: String,
    /// Only listed while `policy.config_tools` is on (K30).
    pub config_tool: bool,
    /// Firefox's hands-and-eyes switch (`browser.hands_and_eyes`) makes its default "allow if
    /// trusted" while it is on.
    #[serde(default)]
    pub by_switch: bool,
}

/// `GET /api/notices`: lines the window shows above its pages.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Notices {
    pub lines: Vec<String>,
}

/// The window event Kai sends the page when the notices changed (`SettingsWindow::notices_changed`):
/// the page reads them again.
pub const NOTICES_EVENT: &str = "kai-notices";

/// The window event Kai sends the page when the Firefox extensions linked to it changed
/// (`SettingsWindow::browser_changed`): the page reads `GET /api/browser` again.
pub const BROWSER_EVENT: &str = "kai-browser";

/// The Firefox section's facts (`GET /api/browser`, and `POST /api/browser/install`, which opens
/// the link): whether this build has a link to Kai for Firefox's signed extension (never the link,
/// which may carry a token), why it is of no use when it is set but unreadable, how many Firefox
/// extensions are linked to Kai now, and after the click whether the link was handed to the desktop.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Browser {
    pub install: bool,
    pub problem: Option<String>,
    pub linked: usize,
    pub opened: Option<bool>,
}

/// The states and levels a tool may be given, and the capability providers, as the page offers
/// them (the words of `ToolState`, `LevelSetting` and `Choice`).
pub const STATES: [&str; 5] = ["allow", "allow_trusted", "confirm", "deny", "off"];
pub const LEVELS: [&str; 4] = ["own", "owner", "trusted", "any"];
pub const PROVIDERS: [&str; 3] = ["plasma", "fdo", "none"];

/// The name every page shows for a state, level or provider word: the catalog's (a test in Kai
/// compares them with the catalog's choices, so there is one set of names).
pub fn word_name(word: &str) -> &str {
    match word {
        "allow" => "Allow",
        "allow_trusted" => "Allow if trusted",
        "confirm" => "Confirm",
        "deny" => "Deny",
        "off" => "Off",
        "own" => "Kai's own key",
        "owner" => "You",
        "trusted" => "Trusted callers",
        "any" => "Any caller",
        "plasma" => "Plasma",
        "fdo" => "freedesktop",
        "none" => "None (off)",
        other => other,
    }
}

/// Kai's conversation as the core knows it (`GET /api/conversation`, the Personality page's
/// header and the editor's thread and model).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Conversation {
    /// Kai holds a working pairing; otherwise `why` says what the owner can do.
    pub paired: bool,
    pub why: Option<String>,
    pub gateway: Option<String>,
    pub folder: Option<String>,
    pub thread_id: Option<i64>,
    /// The folder's model, which a thread of it talks with.
    pub model: Option<String>,
    /// The profile the tray's Personality radio checks; `None` while Kai knows no profile.
    pub profile: Option<String>,
}

/// The RemoteDesktop portal's stored permission (`GET /api/remote-control`, `POST
/// /api/remote-control/forget`; the Desktop page, WP16c): whether Kai holds a restore token now,
/// and after a forget whether it held one.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RemoteControl {
    pub stored: bool,
    pub forgot: Option<bool>,
}
