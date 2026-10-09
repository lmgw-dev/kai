//! The Kai conversation's folder (§4): which Chat folder the conversation lives in, whether it is
//! ready (an ongoing conversation, which needs a model), and what the owner must do on the
//! dashboard when it is not. The thread inside it is the voice's (`current`, the rollover, the
//! rebind; `crate::voice`).
//!
//! Sans-IO: the core hands [`Conversation`] what lmgw said (the folder list, a create's answer, the
//! feed's folder events, a `current` refused for the folder's sake) and carries out the
//! [`Action`]s it returns: requests through the `VoiceLink` seam, the remembered id into the
//! configuration, the folder for the voice, notifications.
//!
//! **The lookup** ([`pick`]) runs at every connected start (the feed's `hello`) and after a
//! `resync`, by `conversation.folder_name` (default "Kai"; folder names are not unique):
//! 1. the remembered folder (`conversation.folder_id`, written by Kai once it found an ongoing
//!    folder, or set by hand) when it is ready, whatever its name now;
//! 2. else the ongoing folder of that name with the lowest id, the other namesakes logged; Kai
//!    remembers it;
//! 3. else the remembered folder, not ready;
//! 4. else the plain folder of that name with the lowest id;
//! 5. else Kai creates a plain folder of that name. It sends only the name: Kai never chooses the
//!    conversation's model (R5), marking a folder ongoing needs one, and a folder's own retention
//!    is the owner's (a device key that writes `archive_days` or `purge_days` is refused, api L5's
//!    W5-1 note).
//!
//! A remembered id the list no longer has is forgotten. A lookup never replaces Kai's own create
//! on its way: a reconnect meanwhile waits for it, and after a create Kai lists again and takes the
//! namesake with the lowest id, so two creates that raced (a reconnect, two first runs) converge
//! (review KAI-2 K2-22).
//!
//! **A 404 on `current`** is checked against the folder list before anything is forgotten (review
//! KAI-2 K2-11): the list, asked after a backoff (1 s, doubling up to `lmgw.retry_max_s`), decides.
//! Gone from it, the folder is forgotten as at `folder.deleted`; still in it, the 404 was something
//! else (a gateway without the route, a proxy, the list and `current` disagreeing), and Kai stops
//! there with a note instead of asking again. The configuration is written only on a change.
//!
//! **Not ready** (no model, or not marked ongoing): the tooltip names the owner's steps in the
//! dashboard's own words (the folder's menu → Folder settings: Model, Ongoing conversation,
//! Retention), and one notification says them, once per folder and state; a `current` refused for
//! it (409 `not_ongoing` or `folder_no_model`) says them again when the owner pressed. Kai follows
//! `folder.updated` and recovers once the folder is ready: it remembers the id and asks `current`.
//!
//! **Ready:** the voice gets the folder's id and its idle rollover (`ongoing.idle_minutes`; `0`
//! rolls over only on request). A folder that keeps its past threads less than a year (§16 Q1) gets
//! a tooltip line naming its Retention fields.

#[cfg(test)]
mod tests;

use std::time::Duration;

use lmgw_client::requests::Folder;

use crate::voice::Notice;
use crate::voice::link::Refusal;

/// §16 answer 9: the idle rollover the owner sets when marking the folder ongoing. Advice in the
/// owner's steps only; Kai follows whatever the folder says.
pub const SUGGESTED_IDLE_MINUTES: i64 = 120;
/// §16 answer 1: the Kai folder keeps its history at least this long (its own retention).
pub const HISTORY_DAYS: i64 = 365;

/// A folder as the conversation reads it (from `GET /chat/api/folders`, a create's answer, or the
/// feed's `folder.created` / `folder.updated`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderInfo {
    pub id: i64,
    pub name: String,
    /// `defaults.model_alias`.
    pub model: Option<String>,
    /// `ongoing.idle_minutes`; `None`: not an ongoing conversation.
    pub idle_minutes: Option<i64>,
    /// The folder's own retention (`null`: the global setting).
    pub archive_days: Option<i64>,
    pub purge_days: Option<i64>,
    /// The labels of `defaults.mcp_tools`, the tool servers a new thread of it gets: PD5 looks for
    /// the label Kai's hosted tools go under among them.
    pub tool_labels: Vec<String>,
    /// `defaults.profile_id`: the personality profile the folder's conversation talks with
    /// (`null`: none, unless lmgw's Chat setting names one for new threads).
    pub profile_id: Option<i64>,
}

impl From<&Folder> for FolderInfo {
    fn from(f: &Folder) -> Self {
        Self {
            id: f.id,
            name: f.name.clone(),
            model: f
                .defaults
                .model_alias
                .clone()
                .filter(|m| !m.trim().is_empty()),
            idle_minutes: f.ongoing.as_ref().map(|o| o.idle_minutes),
            archive_days: f.archive_days,
            purge_days: f.purge_days,
            tool_labels: f
                .defaults
                .mcp_tools
                .iter()
                .flatten()
                .map(|m| m.server_label.clone())
                .collect(),
            profile_id: f.defaults.profile_id,
        }
    }
}

/// Whether a folder can hold the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Ongoing (which needs a model): `current` answers.
    Ready,
    /// Neither model nor ongoing (a folder Kai created, or the owner's plain one).
    NeedsSetup,
    /// It has a model but is not marked ongoing.
    NotOngoing,
}

impl FolderInfo {
    pub fn readiness(&self) -> Readiness {
        match (self.idle_minutes, &self.model) {
            (Some(_), Some(_)) => Readiness::Ready,
            (_, None) => Readiness::NeedsSetup,
            (None, Some(_)) => Readiness::NotOngoing,
        }
    }

    /// The idle rollover Kai times (§4): `None` for `0` (only on request) or not ongoing.
    pub fn idle(&self) -> Option<Duration> {
        self.idle_minutes
            .filter(|m| *m > 0)
            .map(|m| Duration::from_secs((m as u64).saturating_mul(60)))
    }

    /// Its own retention keeps a past thread at least [`HISTORY_DAYS`] (§16 Q1): never archived,
    /// never deleted, or archive and delete days that add up to it. The global setting is not a
    /// device's to read, so a folder without days of its own does not count.
    pub fn keeps_a_year(&self) -> bool {
        match (self.archive_days, self.purge_days) {
            (Some(0), _) | (_, Some(0)) => true,
            (a, Some(p)) => p + a.unwrap_or(0) >= HISTORY_DAYS,
            (_, None) => false,
        }
    }
}

/// [`pick`]'s answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// Use this folder; `namesakes` are the other folders of the name (logged).
    Use {
        folder: FolderInfo,
        namesakes: Vec<i64>,
    },
    /// No folder of the name: create a plain one.
    Create,
}

/// The lookup rule (module doc): the remembered folder when ready, else the lowest-id ongoing
/// folder named `name`, else the remembered one, else the lowest-id plain folder of the name.
pub fn pick(folders: &[FolderInfo], name: &str, remembered: Option<i64>) -> Pick {
    let name = name.trim();
    let mut named: Vec<&FolderInfo> = folders.iter().filter(|f| f.name.trim() == name).collect();
    named.sort_by_key(|f| f.id);
    let remembered = remembered.and_then(|id| folders.iter().find(|f| f.id == id));
    let ready = |f: &&FolderInfo| f.readiness() == Readiness::Ready;
    let chosen = remembered
        .filter(ready)
        .or_else(|| named.iter().copied().find(ready))
        .or(remembered)
        .or_else(|| named.first().copied());
    match chosen {
        Some(f) => Pick::Use {
            folder: f.clone(),
            namesakes: named
                .iter()
                .map(|n| n.id)
                .filter(|id| *id != f.id)
                .collect(),
        },
        None => Pick::Create,
    }
}

/// What the voice gets of the folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Using {
    pub id: i64,
    /// The idle rollover to time; `None`: none (only on request, or not ready).
    pub idle: Option<Duration>,
    pub ready: bool,
}

/// What the core carries out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `GET /chat/api/folders`, its answer to [`Conversation::listed`] (a newer lookup replaces one
    /// still waiting).
    Lookup,
    /// The same after a wait: the backoff of a lookup a refusal asked for (K2-11).
    LookupAfter(Duration),
    /// `POST /chat/api/folders {name}`, its answer to [`Conversation::created`].
    Create(String),
    /// Writes `conversation.folder_id` (`None` removes it).
    Remember(Option<i64>),
    /// The voice's folder (`None`: Kai has none).
    Use(Option<Using>),
    /// Kai asks `current` by itself: at a connected start once the folder is ready, and when it
    /// became ready or another folder took its place.
    AskCurrent,
    /// The folder is gone: the voice leaves its conversation (the session ends, listening off),
    /// with this note.
    Leave(String),
    Notify(Notice),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Stage {
    /// Nothing asked (before the first `hello`), or the last answer settled.
    Settled,
    /// The list is asked for; `start`: by a connected start (its ready folder gets `current`).
    Looking {
        start: bool,
    },
    Creating,
    /// The last lookup or create failed: why.
    Failed(String),
}

/// The first wait of a lookup a refusal asked for; each further one doubles, up to
/// `lmgw.retry_max_s`, as the feed's reconnects do.
const FIRST_RETRY: Duration = Duration::from_secs(1);

pub struct Conversation {
    name: String,
    remembered: Option<i64>,
    folder: Option<FolderInfo>,
    stage: Stage,
    /// The not-ready state last notified: one notification per folder and state.
    told: Option<(i64, Readiness)>,
    /// A 404 on `current` for this folder, with its words: the list on its way decides (K2-11).
    checking: Option<(i64, String)>,
    /// The next wait of a lookup a refusal asks for, and its cap (`lmgw.retry_max_s`).
    backoff: Duration,
    retry_max: Duration,
    /// A connected start (`true`) or a resync came while Kai created its folder: the list after the
    /// create is theirs (K2-22).
    after_create: Option<bool>,
}

impl Conversation {
    /// `name` is `conversation.folder_name`, `remembered` `conversation.folder_id`.
    pub fn new(name: impl Into<String>, remembered: Option<i64>) -> Self {
        Self {
            name: name.into(),
            remembered,
            folder: None,
            stage: Stage::Settled,
            told: None,
            checking: None,
            backoff: Duration::ZERO,
            retry_max: Duration::from_secs(30),
            after_create: None,
        }
    }

    /// The cap of the lookups' backoff (`lmgw.retry_max_s`).
    pub fn retrying_up_to(mut self, retry_max: Duration) -> Self {
        self.retry_max = retry_max.max(FIRST_RETRY);
        self
    }

    /// The next backoff wait: 1 s, then doubling up to the cap.
    fn next_wait(&mut self) -> Duration {
        let wait = self.backoff.max(FIRST_RETRY);
        self.backoff = (wait * 2).min(self.retry_max);
        wait
    }

    /// The folder the voice should use before the first lookup answers: the remembered one.
    pub fn initial(&self) -> Option<Using> {
        self.remembered.map(|id| Using {
            id,
            idle: None,
            ready: true,
        })
    }

    pub fn folder(&self) -> Option<&FolderInfo> {
        self.folder.as_ref()
    }

    pub fn ready(&self) -> bool {
        self.folder
            .as_ref()
            .is_some_and(|f| f.readiness() == Readiness::Ready)
    }

    /// Forgets the gateway's folder (a confirmed re-pair to another gateway): the remembered id
    /// is the core's to remove from the configuration.
    pub fn reset(&mut self) {
        self.remembered = None;
        self.folder = None;
        self.stage = Stage::Settled;
        self.told = None;
        self.checking = None;
        self.backoff = Duration::ZERO;
        self.after_create = None;
    }

    // ------------------------------------------------------------------ what lmgw says

    /// The feed's `hello`: Kai looks its folder up (each connected start). A create on its way is
    /// not replaced: the list after it is this start's (K2-22).
    pub fn connected(&mut self) -> Vec<Action> {
        self.backoff = Duration::ZERO;
        self.checking = None;
        if self.stage == Stage::Creating {
            self.after_create = Some(true);
            return Vec::new();
        }
        self.stage = Stage::Looking { start: true };
        vec![Action::Lookup]
    }

    /// The feed's `resync`: folder events may have been missed. A create on its way is not
    /// replaced (K2-22).
    pub fn resync(&mut self) -> Vec<Action> {
        if self.stage == Stage::Creating {
            self.after_create.get_or_insert(false);
            return Vec::new();
        }
        if !matches!(self.stage, Stage::Looking { start: true }) {
            self.stage = Stage::Looking { start: false };
        }
        vec![Action::Lookup]
    }

    /// The folder list's answer.
    pub fn listed(&mut self, r: Result<Vec<FolderInfo>, Refusal>) -> Vec<Action> {
        let Stage::Looking { start } = self.stage else {
            tracing::debug!("conversation: a folder list nobody waits for");
            return Vec::new();
        };
        let folders = match r {
            Ok(f) => f,
            Err(e) => return self.failed(e),
        };
        let mut acts = Vec::new();
        if let Some((id, why)) = self.checking.take() {
            if folders.iter().any(|f| f.id == id) {
                // Still listed: the 404 was not the folder going away. Kai stops here instead of
                // asking `current` again (K2-11).
                tracing::warn!(
                    folder = id,
                    "conversation: current answered 404 though the folder list shows the folder; \
                     Kai stops asking"
                );
                self.stage = Stage::Failed(format!(
                    "lmgw answered the conversation's thread with a 404 ({why}) though its folder \
                     list still shows folder {id}"
                ));
                return acts;
            }
            tracing::info!(
                folder = id,
                "conversation: the folder is gone from the list too"
            );
            acts.extend(self.forget(id));
        } else {
            self.backoff = Duration::ZERO;
        }
        if let Some(id) = self.remembered
            && !folders.iter().any(|f| f.id == id)
        {
            tracing::info!(
                folder = id,
                "conversation: the remembered folder is gone (conversation.folder_id is removed)"
            );
            self.remembered = None;
            acts.push(Action::Remember(None));
        }
        match pick(&folders, &self.name, self.remembered) {
            Pick::Create => {
                tracing::info!(name = %self.name, "conversation: no folder of that name; Kai creates a plain one (§4)");
                self.stage = Stage::Creating;
                acts.push(Action::Create(self.name.clone()));
            }
            Pick::Use { folder, namesakes } => {
                self.stage = Stage::Settled;
                if !namesakes.is_empty() {
                    tracing::warn!(
                        name = %self.name,
                        used = folder.id,
                        ?namesakes,
                        "conversation: more folders carry this name; Kai uses the one above (§4)"
                    );
                }
                acts.extend(self.adopt(folder, start));
            }
        }
        acts
    }

    /// The create's answer. Kai then lists again, so a create that raced another one (a
    /// reconnect, two first runs) converges on the namesake with the lowest id (K2-22).
    pub fn created(&mut self, r: Result<FolderInfo, Refusal>) -> Vec<Action> {
        if self.stage != Stage::Creating {
            return Vec::new();
        }
        let start = self.after_create.take();
        match r {
            Ok(f) => {
                tracing::info!(folder = f.id, name = %f.name, "conversation: Kai created its folder");
                let mut acts = self.adopt(f, false);
                self.stage = Stage::Looking {
                    start: start.unwrap_or(false),
                };
                acts.push(Action::Lookup);
                acts
            }
            Err(e) if start.is_some() => {
                tracing::warn!("conversation: the create failed ({e}); the reconnect looks again");
                self.stage = Stage::Looking {
                    start: start.unwrap_or(false),
                };
                vec![Action::Lookup]
            }
            Err(e) => self.failed(e),
        }
    }

    /// `folder.created` / `folder.updated`, the folder as it is now.
    pub fn changed(&mut self, f: FolderInfo) -> Vec<Action> {
        if self.folder.as_ref().is_some_and(|c| c.id == f.id) {
            if matches!(self.stage, Stage::Failed(_)) {
                self.stage = Stage::Settled;
            }
            return self.adopt(f, false);
        }
        // Another folder of the name while Kai's is not ready: the owner may have readied that
        // one. An answer still awaited settles it anyway.
        let waiting = matches!(self.stage, Stage::Looking { .. } | Stage::Creating);
        if !waiting && !self.ready() && f.name.trim() == self.name.trim() {
            self.stage = Stage::Looking { start: false };
            return vec![Action::Lookup];
        }
        Vec::new()
    }

    /// `folder.deleted` (or a folder gone by delivery).
    pub fn gone(&mut self, id: i64) -> Vec<Action> {
        if self.folder.as_ref().is_none_or(|f| f.id != id) {
            return Vec::new();
        }
        tracing::info!(
            folder = id,
            "conversation: its folder is gone; Kai looks it up again"
        );
        let mut acts = self.forget(id);
        self.stage = Stage::Looking { start: false };
        acts.push(Action::Lookup);
        acts
    }

    /// Folder `id` is gone: forgotten (the configuration only when it remembered it), and the voice
    /// leaves it.
    fn forget(&mut self, id: i64) -> Vec<Action> {
        let mut acts = Vec::new();
        if self.folder.as_ref().is_some_and(|f| f.id == id) {
            self.folder = None;
            self.told = None;
        }
        if self.remembered == Some(id) {
            self.remembered = None;
            acts.push(Action::Remember(None));
        }
        acts.extend([
            Action::Use(None),
            Action::Leave(format!(
                "the folder '{}' of Kai's conversation was deleted in lmgw",
                self.name
            )),
        ]);
        acts
    }

    /// A `current` the voice asked was refused for the folder's sake: 409 `not_ongoing` or
    /// `folder_no_model`, or 404 (the folder is gone). `by_owner`: a press or the tray asked, so
    /// the owner hears the steps again.
    pub fn refused(&mut self, r: &Refusal, by_owner: bool) -> Vec<Action> {
        let Some(mut f) = self.folder.clone() else {
            // Only the remembered id was known: look it up.
            self.stage = Stage::Looking { start: false };
            return vec![Action::Lookup];
        };
        match r.code() {
            "not_ongoing" => f.idle_minutes = None,
            "folder_no_model" => f.model = None,
            _ if r.status == 404 => {
                // The list decides whether the folder is gone, after a backoff (K2-11).
                let wait = self.next_wait();
                tracing::warn!(
                    folder = f.id,
                    wait_s = wait.as_secs(),
                    "conversation: current answered 404 ({r}); Kai checks the folder list"
                );
                self.checking = Some((f.id, r.to_string()));
                self.stage = Stage::Looking { start: false };
                return vec![Action::LookupAfter(wait)];
            }
            _ => return Vec::new(),
        }
        if by_owner {
            self.told = None;
        }
        self.adopt(f, false)
    }

    fn failed(&mut self, e: Refusal) -> Vec<Action> {
        tracing::warn!(name = %self.name, "conversation: the folder lookup failed: {e}");
        self.stage = Stage::Failed(e.to_string());
        Vec::new()
    }

    /// Takes `f` as the conversation's folder (the stage is the caller's).
    fn adopt(&mut self, f: FolderInfo, start: bool) -> Vec<Action> {
        let was_ready = self.ready();
        let was = self.folder.as_ref().map(|c| c.id);
        let r = f.readiness();
        let mut acts = vec![Action::Use(Some(Using {
            id: f.id,
            idle: if r == Readiness::Ready {
                f.idle()
            } else {
                None
            },
            ready: r == Readiness::Ready,
        }))];
        if r == Readiness::Ready {
            self.told = None;
            if self.remembered != Some(f.id) {
                tracing::info!(
                    folder = f.id,
                    "conversation: remembered (conversation.folder_id)"
                );
                self.remembered = Some(f.id);
                acts.push(Action::Remember(Some(f.id)));
            }
            if start || !was_ready || was != Some(f.id) {
                acts.push(Action::AskCurrent);
            }
        } else if self.told != Some((f.id, r)) {
            self.told = Some((f.id, r));
            acts.push(Action::Notify(Notice {
                summary: format!("Kai: set up the folder '{}' in lmgw", self.name),
                body: steps(&self.name, &f),
            }));
        }
        if was != Some(f.id) || !was_ready && r == Readiness::Ready {
            tracing::info!(folder = f.id, name = %f.name, readiness = ?r, idle = ?f.idle_minutes, "conversation: its folder");
        }
        self.folder = Some(f);
        acts
    }

    // ------------------------------------------------------------------ what the tray shows

    /// Tooltip lines: the lookup, its failure, the owner's steps, a short retention.
    pub fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        match (&self.stage, &self.folder) {
            (Stage::Failed(e), _) => notes.push(format!(
                "Kai could not look up its folder '{}' in lmgw ({e}); it tries again when the feed \
                 reconnects",
                self.name
            )),
            (Stage::Looking { .. } | Stage::Creating, None) => {
                notes.push(format!("looking up the folder '{}' in lmgw", self.name))
            }
            _ => {}
        }
        if let Some(f) = &self.folder {
            match f.readiness() {
                Readiness::Ready if !f.keeps_a_year() => notes.push(format!(
                    "the folder '{}' keeps {}: for a year of history set its {}",
                    self.name,
                    kept(f),
                    RETENTION_STEP
                )),
                Readiness::Ready => {}
                _ => notes.push(format!(
                    "voice waits for its folder: {}",
                    steps(&self.name, f)
                )),
            }
        }
        notes
    }
}

/// The dashboard's own words for a year of history (lmgw's folder form, "Retention").
const RETENTION_STEP: &str =
    "Folder settings → Retention → Delete after (days archived) to 365 or more in lmgw";

/// How long a folder keeps its past threads, as far as a device can tell.
fn kept(f: &FolderInfo) -> String {
    match (f.archive_days, f.purge_days) {
        (_, None) => "its past threads by lmgw's global retention".to_owned(),
        (Some(a), Some(p)) => format!("a past thread {} days after its last message", a + p),
        (None, Some(p)) => format!(
            "a past thread {p} days after it is archived (by lmgw's global archive setting)"
        ),
    }
}

/// The owner's steps for a folder that is not ready, in the dashboard's words (lmgw's folder form:
/// the folder's menu → Folder settings: Model, Ongoing conversation, Retention).
pub fn steps(name: &str, f: &FolderInfo) -> String {
    let what = match f.readiness() {
        Readiness::NotOngoing => format!(
            "mark the folder '{name}' as an ongoing conversation in lmgw: in Chat, the folder's \
             menu → Folder settings: tick Ongoing conversation (a new thread after \
             {SUGGESTED_IDLE_MINUTES} idle minutes)"
        ),
        _ => format!(
            "set the folder '{name}''s model and mark it as an ongoing conversation in lmgw: in \
             Chat, the folder's menu → Folder settings: pick its Model, tick Ongoing conversation \
             (a new thread after {SUGGESTED_IDLE_MINUTES} idle minutes)"
        ),
    };
    let what = if f.keeps_a_year() {
        what
    } else {
        format!(
            "{what}, and under Retention set Delete after (days archived) to {HISTORY_DAYS} or \
             more, so its history stays a year"
        )
    };
    format!("{what}. The tray's Open in lmgw opens that form.")
}
