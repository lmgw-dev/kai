//! What the page and Kai say to each other, over `kai://settings/api/…` (WP21b). These are the
//! frontend's copies of `src/settings`'s serde types (`Request`, `Response`, `Entry`) and of the
//! scheme handler's `ToolInfo` and `Conversation`, with words for the enums; Kai's side compiles this very file in its
//! tests (`src/settings/window/tests.rs`) and reads its real answers with it, so a change of shape
//! on either side fails there.

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
    /// The words a fixed-word setting takes, from its type in Kai's core; empty for any other.
    #[serde(default)]
    pub choices: Vec<String>,
}

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
}

/// The states and levels a tool may be given, and the capability providers, as the page offers
/// them (the words of `ToolState`, `LevelSetting` and `Choice`).
pub const STATES: [&str; 5] = ["allow", "allow_trusted", "confirm", "deny", "off"];
pub const LEVELS: [&str; 4] = ["own", "owner", "trusted", "any"];
pub const PROVIDERS: [&str; 3] = ["plasma", "fdo", "none"];
/// Keys Kai refuses to write because its own flows do (`src/settings`'s `PAIRING_KEYS`).
pub const READ_ONLY_KEYS: [&str; 1] = ["lmgw.url"];

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
