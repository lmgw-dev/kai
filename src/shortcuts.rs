//! Global shortcuts (§8): `talk`, `new-conversation` and `mute`, delivered as [`Command`]s, and
//! `hide-overlays` (§10.1, WP10), which goes straight to the tool-controlled overlays
//! ([`Target`]).
//!
//! - **The XDG GlobalShortcuts portal** through ashpd (§8.2): `CreateSession`, `BindShortcuts`,
//!   then `Activated` for a press and `Deactivated` for a release. WP0 (f) saw them strictly
//!   alternate on KWin 6, with no repeat while held, so the portal is the path.
//! - **Kai binds no keys by default** (§8.1): no preferred trigger is offered, so the portal's
//!   bind dialog asks at the first start and System Settings → Shortcuts changes them later. Right
//!   Alt is never offered (AltGr on many layouts).
//! - **KGlobalAccel over D-Bus** (component `dev.rootshell.kai`) only for a session without the
//!   portal (§8.2): `doRegister` and `setShortcut` with no key (the keys set in System Settings
//!   load), then the component's `globalShortcutPressed` and `globalShortcutReleased`.
//! - The tooltip says what is missing: the portal, the bind, or a key for an action.
//!
//! **Press and release in their true order.** Each deliverer's signals are read from *one*
//! message stream (one match rule on its interface and object, from its own unique name), so a
//! release can never be handled before the press that came first; two streams polled by a
//! `select!` could swap them, and a release handled first would be dropped while its press then
//! held the talk key down. [`Edges`] adds a guard on top, from the signals' timestamps: a press
//! that arrives after a release with a newer timestamp is that release's own press, late, and is
//! dropped. The voice adds `shortcuts.max_hold_s`, which ends a hold no release ended.
//!
//! Repeated presses while held are ignored by the voice (K6), whatever delivers them.
//!
//! **Routines** (WP20, K28): each routine that can run from a key when Kai starts (on, synchronous,
//! no required parameter) is one more action, `routine-<name>`, with no key bound (§8.1's rule);
//! its press runs it as the owner's own call (`crate::routines::trigger`). A routine switched on
//! later gets its action at the next start.

use std::collections::{HashMap, HashSet};

use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::core::Command;

pub const TALK: &str = "talk";
pub const NEW_CONVERSATION: &str = "new-conversation";
pub const MUTE: &str = "mute";
pub const HIDE_OVERLAYS: &str = "hide-overlays";

/// The actions (§8.1) and how System Settings names them.
pub const ACTIONS: [(&str, &str); 4] = [
    (TALK, "Talk to Kai (tap: listening on or off; hold: talk)"),
    (NEW_CONVERSATION, "Kai: new conversation"),
    (MUTE, "Kai: mute or unmute the microphone"),
    (HIDE_OVERLAYS, "Kai: hide every overlay"),
];

/// A press (`true`) or release of shortcut `id` as the core's command.
pub fn command(id: &str, pressed: bool) -> Option<Command> {
    match (id, pressed) {
        (TALK, down) => Some(Command::Talk(down)),
        (MUTE, true) => Some(Command::ToggleMute),
        (NEW_CONVERSATION, true) => Some(Command::NewConversation),
        _ => None,
    }
}

/// Where a press or release goes.
pub enum Target {
    Core(Command),
    /// Every tool-controlled overlay is hidden (`crate::widgets::hide`): the GTK side's, not the
    /// core's, like the tray's Hide all.
    HideOverlays,
}

pub fn target(id: &str, pressed: bool) -> Option<Target> {
    match (id, pressed) {
        (HIDE_OVERLAYS, true) => Some(Target::HideOverlays),
        (HIDE_OVERLAYS, false) => None,
        _ => command(id, pressed).map(Target::Core),
    }
}

/// The tooltip's note from what is bound (`(id, trigger description)`): the actions with no key.
pub fn note(bound: &[(String, String)]) -> Option<String> {
    let unset: Vec<&str> = ACTIONS
        .iter()
        .filter(|(id, _)| {
            !bound
                .iter()
                .any(|(b, trigger)| b == id && !trigger.trim().is_empty())
        })
        .map(|(id, _)| *id)
        .collect();
    (!unset.is_empty()).then(|| {
        format!(
            "no key set for {} (System Settings → Shortcuts → Kai)",
            unset.join(", ")
        )
    })
}

/// The actions to bind: [`ACTIONS`], then the routines' (module doc).
fn actions() -> Vec<(String, String)> {
    ACTIONS
        .iter()
        .map(|(id, d)| ((*id).to_owned(), (*d).to_owned()))
        .chain(crate::routines::shortcut_actions())
        .collect()
}

fn send(cmds: &mpsc::UnboundedSender<Command>, id: &str, pressed: bool) {
    tracing::debug!(id, pressed, "shortcut");
    if let Some(name) = id.strip_prefix(crate::routines::SHORTCUT_PREFIX) {
        if pressed {
            crate::routines::trigger(crate::routines::Trigger::Run(name.to_owned()));
        }
        return;
    }
    match target(id, pressed) {
        Some(Target::Core(c)) => {
            let _ = cmds.send(c);
        }
        Some(Target::HideOverlays) => crate::widgets::hide(None),
        None => {}
    }
}

/// One press or release, as the portal or KGlobalAccel delivers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub id: String,
    pub pressed: bool,
    /// The event's timestamp in ms on the deliverer's clock; 0 when it gives none.
    pub ts: u64,
}

/// The edges in their true order (module doc). A release that comes while its shortcut is up is
/// remembered with its timestamp: if the next press is not newer, it is that release's own press
/// arriving late, and it is dropped, so no key is left down. Anything else passes in arrival
/// order. Without timestamps (0) nothing is dropped.
#[derive(Debug, Default)]
pub struct Edges {
    down: HashSet<String>,
    /// Releases that came with their shortcut up: the timestamp of each.
    orphan: HashMap<String, u64>,
}

impl Edges {
    /// The edge to act on, or `None` for a late press.
    pub fn edge(&mut self, e: Edge) -> Option<(String, bool)> {
        if e.pressed {
            if let Some(release) = self.orphan.remove(&e.id)
                && e.ts != 0
                && release != 0
                && e.ts <= release
            {
                tracing::warn!(
                    id = e.id,
                    press_ms = e.ts,
                    release_ms = release,
                    "a press arrived after its own release; dropped, so the key is not left down"
                );
                return None;
            }
            self.down.insert(e.id.clone());
        } else if !self.down.remove(&e.id) {
            self.orphan.insert(e.id.clone(), e.ts);
        }
        Some((e.id, e.pressed))
    }

    #[cfg(test)]
    fn is_down(&self, id: &str) -> bool {
        self.down.contains(id)
    }
}

/// The portal's interface and object (xdg-desktop-portal).
const PORTAL_NAME: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_IFACE: &str = "org.freedesktop.portal.GlobalShortcuts";

/// What one message of the portal's stream says.
#[derive(Debug)]
enum PortalSignal {
    Edge(Edge),
    Changed(Vec<(String, String)>),
}

/// Reads a portal signal for Kai's session `session` (its object path); other sessions' and
/// other members' messages are `None`.
fn portal_signal(msg: &zbus::Message, session: &str) -> Option<PortalSignal> {
    use ashpd::desktop::global_shortcuts::{Activated, Deactivated, ShortcutsChanged};
    let header = msg.header();
    let member = header.member()?.as_str();
    let body = msg.body();
    let edge = |handle: &str, id: &str, ts: std::time::Duration, pressed: bool| {
        (handle == session).then(|| {
            PortalSignal::Edge(Edge {
                id: id.to_owned(),
                pressed,
                ts: ts.as_millis() as u64,
            })
        })
    };
    match member {
        "Activated" => {
            let a: Activated = body.deserialize().ok()?;
            edge(
                a.session_handle().as_str(),
                a.shortcut_id(),
                a.timestamp(),
                true,
            )
        }
        "Deactivated" => {
            let d: Deactivated = body.deserialize().ok()?;
            edge(
                d.session_handle().as_str(),
                d.shortcut_id(),
                d.timestamp(),
                false,
            )
        }
        "ShortcutsChanged" => {
            let c: ShortcutsChanged = body.deserialize().ok()?;
            (c.session_handle().as_str() == session).then(|| {
                PortalSignal::Changed(
                    c.shortcuts()
                        .iter()
                        .map(|s| (s.id().to_owned(), s.trigger_description().to_owned()))
                        .collect(),
                )
            })
        }
        _ => None,
    }
}

/// The object path a value serializes to (an ashpd `Session` keeps its path private, but it
/// serializes as it).
fn object_path_of<T: serde::Serialize + zbus::zvariant::Type>(v: &T) -> Option<String> {
    use zbus::zvariant::{LE, OwnedObjectPath, serialized::Context, to_bytes};
    let data = to_bytes(Context::new_dbus(LE, 0), v).ok()?;
    let (path, _): (OwnedObjectPath, _) = data.deserialize().ok()?;
    Some(path.to_string())
}

/// One ordered stream of `interface`'s signals on `path` from `name`'s current owner.
async fn signal_stream(
    conn: &zbus::Connection,
    name: &str,
    path: &str,
    interface: &str,
) -> zbus::Result<zbus::MessageStream> {
    let dbus = zbus::fdo::DBusProxy::new(conn).await?;
    let owner = dbus
        .get_name_owner(zbus::names::BusName::try_from(name)?)
        .await?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(owner.as_str())?
        .interface(interface)?
        .path(path)?
        .build()
        .to_owned();
    zbus::MessageStream::for_match_rule(rule, conn, None).await
}

/// Why the portal path ended.
enum PortalEnd {
    /// No GlobalShortcuts portal in this session: KGlobalAccel takes over.
    Missing(String),
    /// It is there but failed; the reason is shown.
    Failed(String),
    /// Its signals ended (the bus went away).
    Ended,
}

/// Runs until Kai quits (the task is aborted then) or delivery ends.
pub async fn run(cmds: mpsc::UnboundedSender<Command>) {
    match portal(&cmds).await {
        PortalEnd::Ended => tracing::info!("the shortcuts portal's signals ended"),
        PortalEnd::Failed(why) => {
            tracing::warn!("global shortcuts: {why}");
            let _ = cmds.send(Command::ShortcutNote(Some(why)));
        }
        PortalEnd::Missing(why) => {
            tracing::warn!("no GlobalShortcuts portal ({why}); KGlobalAccel instead (§8.2)");
            if let Err(e) = kglobalaccel(&cmds).await {
                let note = format!("no GlobalShortcuts portal ({why}), and KGlobalAccel: {e}");
                tracing::warn!("global shortcuts: {note}");
                let _ = cmds.send(Command::ShortcutNote(Some(note)));
            }
        }
    }
}

/// The portal's bus name is owned or can be activated. ashpd's proxy does not ask, so without
/// this a session with no portal at all only fails at `CreateSession`.
async fn portal_present() -> Result<bool, String> {
    const NAME: &str = "org.freedesktop.portal.Desktop";
    let conn = zbus::Connection::session()
        .await
        .map_err(|e| e.to_string())?;
    let dbus = zbus::fdo::DBusProxy::new(&conn)
        .await
        .map_err(|e| e.to_string())?;
    let name = zbus::names::BusName::try_from(NAME).map_err(|e| e.to_string())?;
    if dbus.name_has_owner(name).await.map_err(|e| e.to_string())? {
        return Ok(true);
    }
    let activatable = dbus
        .list_activatable_names()
        .await
        .map_err(|e| e.to_string())?;
    Ok(activatable.iter().any(|n| n.as_str() == NAME))
}

async fn portal(cmds: &mpsc::UnboundedSender<Command>) -> PortalEnd {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    match portal_present().await {
        Ok(true) => {}
        Ok(false) => {
            return PortalEnd::Missing(
                "org.freedesktop.portal.Desktop is neither running nor activatable".into(),
            );
        }
        Err(e) => return PortalEnd::Missing(format!("the session bus: {e}")),
    }
    // One connection for the session and its signals: the portal sends them to the session's
    // owner.
    let conn = match zbus::Connection::session().await {
        Ok(c) => c,
        Err(e) => return PortalEnd::Missing(format!("the session bus: {e}")),
    };
    let gs = match GlobalShortcuts::with_connection(conn.clone()).await {
        Ok(gs) => gs,
        Err(e @ ashpd::Error::PortalNotFound(_)) => return PortalEnd::Missing(e.to_string()),
        Err(e) => return PortalEnd::Missing(format!("the portal did not answer: {e}")),
    };
    let fail = |what: &str, e: ashpd::Error| PortalEnd::Failed(format!("the portal's {what}: {e}"));
    let session = match gs.create_session(Default::default()).await {
        Ok(s) => s,
        Err(e) => return fail("CreateSession", e),
    };
    let Some(session_path) = object_path_of(&session) else {
        return PortalEnd::Failed("the portal's session has no object path".into());
    };
    // Before the bind, so no edge after it is missed.
    let mut signals = match signal_stream(&conn, PORTAL_NAME, PORTAL_PATH, PORTAL_IFACE).await {
        Ok(s) => s,
        Err(e) => return PortalEnd::Failed(format!("the portal's signals: {e}")),
    };
    let shortcuts: Vec<NewShortcut> = actions()
        .iter()
        .map(|(id, description)| NewShortcut::new(id.as_str(), description.as_str()))
        .collect();
    tracing::info!(
        version = gs.version(),
        "binding the global shortcuts (the portal asks for keys the first time)"
    );
    let bound = match gs
        .bind_shortcuts(&session, &shortcuts, None, Default::default())
        .await
    {
        Ok(request) => request.response(),
        Err(e) => return fail("BindShortcuts", e),
    };
    match bound {
        Ok(b) => {
            let list: Vec<(String, String)> = b
                .shortcuts()
                .iter()
                .map(|s| (s.id().to_owned(), s.trigger_description().to_owned()))
                .collect();
            tracing::info!(?list, "global shortcuts bound");
            let _ = cmds.send(Command::ShortcutNote(note(&list)));
        }
        Err(e) => {
            // The owner cancelled the dialog: nothing is bound this run.
            let why = format!("not bound ({e}); they are asked for again at the next start");
            tracing::warn!("global shortcuts: {why}");
            let _ = cmds.send(Command::ShortcutNote(Some(why)));
        }
    }
    let mut edges = Edges::default();
    while let Some(msg) = signals.next().await {
        let Ok(msg) = msg else { continue };
        match portal_signal(&msg, &session_path) {
            Some(PortalSignal::Edge(e)) => {
                if let Some((id, pressed)) = edges.edge(e) {
                    send(cmds, &id, pressed);
                }
            }
            Some(PortalSignal::Changed(list)) => {
                tracing::info!(?list, "global shortcuts changed");
                let _ = cmds.send(Command::ShortcutNote(note(&list)));
            }
            None => {}
        }
    }
    drop(session);
    PortalEnd::Ended
}

/// KGlobalAccel's D-Bus API (KF6), for a session without the portal. Not exercised live: on
/// Plasma 6 the portal answers (WP0 (f)), and registering would leave a component behind.
async fn kglobalaccel(cmds: &mpsc::UnboundedSender<Command>) -> zbus::Result<()> {
    const DEST: &str = "org.kde.kglobalaccel";
    /// `KGlobalAccel::SetPresent`: the action exists now; without `NoAutoloading` the keys stored
    /// for it (System Settings) load.
    const SET_PRESENT: u32 = 2;
    let conn = zbus::Connection::session().await?;
    let kga = zbus::Proxy::new(&conn, DEST, "/kglobalaccel", "org.kde.KGlobalAccel").await?;
    let mut list = Vec::new();
    for (id, description) in actions() {
        let action = vec![crate::APP_ID, id.as_str(), "Kai", description.as_str()];
        kga.call_method("doRegister", &(action.clone(),)).await?;
        let keys: Vec<i32> = kga
            .call("setShortcut", &(action, Vec::<i32>::new(), SET_PRESENT))
            .await?;
        list.push((
            id.to_owned(),
            if keys.iter().any(|k| *k != 0) {
                format!("{keys:?}")
            } else {
                String::new()
            },
        ));
    }
    let path: zbus::zvariant::OwnedObjectPath = kga.call("getComponent", &(crate::APP_ID,)).await?;
    // Pressed and released from one ordered stream (module doc).
    let mut signals =
        signal_stream(&conn, DEST, path.as_str(), "org.kde.kglobalaccel.Component").await?;
    tracing::info!(?list, "global shortcuts through KGlobalAccel");
    let _ = cmds.send(Command::ShortcutNote(note(&list)));
    let mut edges = Edges::default();
    while let Some(msg) = signals.next().await {
        let Ok(msg) = msg else { continue };
        if let Some(e) = kglobalaccel_edge(&msg)
            && let Some((id, pressed)) = edges.edge(e)
        {
            send(cmds, &id, pressed);
        }
    }
    Ok(())
}

/// `globalShortcutPressed`/`globalShortcutReleased (s component, s shortcut, x timestamp)`.
fn kglobalaccel_edge(msg: &zbus::Message) -> Option<Edge> {
    let header = msg.header();
    let pressed = match header.member()?.as_str() {
        "globalShortcutPressed" => true,
        "globalShortcutReleased" => false,
        _ => return None,
    };
    match msg.body().deserialize::<(String, String, i64)>() {
        Ok((_, action, ts)) => Some(Edge {
            id: action,
            pressed,
            ts: ts.max(0) as u64,
        }),
        Err(e) => {
            tracing::warn!("KGlobalAccel: an unreadable signal: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_become_commands() {
        assert!(matches!(command(TALK, true), Some(Command::Talk(true))));
        assert!(matches!(command(TALK, false), Some(Command::Talk(false))));
        assert!(matches!(command(MUTE, true), Some(Command::ToggleMute)));
        assert!(
            command(MUTE, false).is_none(),
            "a release of mute does nothing"
        );
        assert!(matches!(
            command(NEW_CONVERSATION, true),
            Some(Command::NewConversation)
        ));
        assert!(
            command(HIDE_OVERLAYS, true).is_none(),
            "the overlays' own, not the core's"
        );
        assert!(matches!(
            target(HIDE_OVERLAYS, true),
            Some(Target::HideOverlays)
        ));
        assert!(
            target(HIDE_OVERLAYS, false).is_none(),
            "a release hides nothing"
        );
        assert!(matches!(
            target(TALK, false),
            Some(Target::Core(Command::Talk(false)))
        ));
        assert!(target("unknown", true).is_none());
    }

    fn edge(id: &str, pressed: bool, ts: u64) -> Edge {
        Edge {
            id: id.into(),
            pressed,
            ts,
        }
    }

    #[test]
    fn a_release_delivered_before_its_press_leaves_no_key_down() {
        let mut e = Edges::default();
        // The release (t = 1070 ms) is handled before its press (t = 1000 ms).
        assert_eq!(e.edge(edge(TALK, false, 1070)), Some((TALK.into(), false)));
        assert_eq!(
            e.edge(edge(TALK, true, 1000)),
            None,
            "the late press is dropped"
        );
        assert!(!e.is_down(TALK), "no stuck key");
        // The next real press and release pass.
        assert_eq!(e.edge(edge(TALK, true, 5000)), Some((TALK.into(), true)));
        assert!(e.is_down(TALK));
        assert_eq!(e.edge(edge(TALK, false, 5069)), Some((TALK.into(), false)));
        assert!(!e.is_down(TALK));
    }

    #[test]
    fn edges_in_order_pass_in_order() {
        let mut e = Edges::default();
        let got: Vec<_> = [
            edge(TALK, true, 10),
            edge(TALK, false, 80),
            edge(MUTE, true, 90),
            edge(MUTE, false, 95),
            edge(TALK, true, 100),
            edge(TALK, false, 2100),
        ]
        .into_iter()
        .filter_map(|x| e.edge(x))
        .map(|(id, p)| format!("{id}{}", if p { "+" } else { "-" }))
        .collect();
        assert_eq!(got, ["talk+", "talk-", "mute+", "mute-", "talk+", "talk-"]);
        // A stray release (its press came before Kai listened), then a newer press: it passes.
        let mut e = Edges::default();
        assert!(e.edge(edge(TALK, false, 50)).is_some());
        assert_eq!(e.edge(edge(TALK, true, 60)), Some((TALK.into(), true)));
        // No timestamps: nothing is dropped (the voice's shortcuts.max_hold_s is the net).
        let mut e = Edges::default();
        e.edge(edge(TALK, false, 0));
        assert_eq!(e.edge(edge(TALK, true, 0)), Some((TALK.into(), true)));
    }

    fn portal_msg(member: &str, session: &str, id: &str, ts: u64) -> zbus::Message {
        let options: HashMap<String, zbus::zvariant::Value> = HashMap::new();
        zbus::Message::signal(PORTAL_PATH, PORTAL_IFACE, member)
            .unwrap()
            .build(&(
                zbus::zvariant::ObjectPath::try_from(session).unwrap(),
                id,
                ts,
                options,
            ))
            .unwrap()
    }

    #[test]
    fn portal_signals_are_read_for_kais_session_only() {
        const MINE: &str = "/org/freedesktop/portal/desktop/session/1_42/kai";
        let m = portal_msg("Activated", MINE, TALK, 1234);
        assert!(matches!(
            portal_signal(&m, MINE),
            Some(PortalSignal::Edge(Edge { ref id, pressed: true, ts: 1234 })) if id == TALK
        ));
        let m = portal_msg("Deactivated", MINE, TALK, 1300);
        assert!(matches!(
            portal_signal(&m, MINE),
            Some(PortalSignal::Edge(Edge {
                pressed: false,
                ts: 1300,
                ..
            }))
        ));
        let other = portal_msg(
            "Activated",
            "/org/freedesktop/portal/desktop/session/1_7/x",
            TALK,
            1,
        );
        assert!(
            portal_signal(&other, MINE).is_none(),
            "another session's edge"
        );
        let m = zbus::Message::signal(PORTAL_PATH, PORTAL_IFACE, "SomethingElse")
            .unwrap()
            .build(&())
            .unwrap();
        assert!(portal_signal(&m, MINE).is_none());
    }

    #[test]
    fn kglobalaccel_signals_carry_their_timestamp() {
        let m = zbus::Message::signal(
            "/component/dev_rootshell_kai",
            "org.kde.kglobalaccel.Component",
            "globalShortcutReleased",
        )
        .unwrap()
        .build(&("dev.rootshell.kai", TALK, 77i64))
        .unwrap();
        assert_eq!(kglobalaccel_edge(&m), Some(edge(TALK, false, 77)));
    }

    #[test]
    fn a_session_serializes_to_its_object_path() {
        let p = zbus::zvariant::ObjectPath::try_from("/a/b").unwrap();
        assert_eq!(object_path_of(&p).as_deref(), Some("/a/b"));
    }

    #[test]
    fn the_note_names_the_actions_without_a_key() {
        let all = |t: &str| {
            ACTIONS
                .iter()
                .map(|(id, _)| (id.to_string(), t.to_owned()))
                .collect::<Vec<_>>()
        };
        assert_eq!(note(&all("Ctrl+Shift+F9")), None);
        assert_eq!(
            note(&all("")).unwrap(),
            "no key set for talk, new-conversation, mute, hide-overlays (System Settings → Shortcuts \
             → Kai)"
        );
        let mut some = all("Meta+K");
        some.retain(|(id, _)| id != MUTE);
        assert!(note(&some).unwrap().starts_with("no key set for mute "));
        // No preferred triggers anywhere: Kai binds no keys by default (§8.1).
        assert!(ACTIONS.iter().all(|(_, d)| !d.contains("Alt")));
    }

    /// The portal path up to the bind, with no dialog and nothing registered: run by
    /// `scripts/live-check-shortcuts.sh` inside Kai's app scope (`dev.rootshell.kai.desktop`
    /// installed, K12). `BindShortcuts` opens KDE's dialog, so the bind and the presses are the
    /// owner's check (`scripts/owner-check-shortcuts.sh`).
    #[tokio::test]
    #[ignore = "run by scripts/live-check-shortcuts.sh in Kai's app scope"]
    async fn live_portal_session_without_a_bind() {
        if std::env::var("KAI_LIVE_PORTAL").as_deref() != Ok("1") {
            eprintln!("not in Kai's app scope: skipped (scripts/live-check-shortcuts.sh)");
            return;
        }
        use ashpd::desktop::global_shortcuts::GlobalShortcuts;
        assert_eq!(portal_present().await, Ok(true), "the portal is there");
        // As Kai runs it: one connection for the session and its ordered signal stream.
        let conn = zbus::Connection::session().await.expect("the session bus");
        let gs = GlobalShortcuts::with_connection(conn.clone())
            .await
            .expect("the GlobalShortcuts portal");
        let session = gs
            .create_session(Default::default())
            .await
            .expect("CreateSession under Kai's app id");
        let path = object_path_of(&session).expect("the session's object path");
        assert!(
            path.starts_with("/org/freedesktop/portal/desktop/session/"),
            "{path}"
        );
        let _signals = signal_stream(&conn, PORTAL_NAME, PORTAL_PATH, PORTAL_IFACE)
            .await
            .expect("one ordered stream of the portal's signals");
        let listed = gs
            .list_shortcuts(&session, Default::default())
            .await
            .expect("ListShortcuts")
            .response()
            .expect("an answer");
        let ids: Vec<(String, String)> = listed
            .shortcuts()
            .iter()
            .map(|s| (s.id().to_owned(), s.trigger_description().to_owned()))
            .collect();
        println!(
            "PORTAL version {}: CreateSession ok ({path}); signal stream ok; ListShortcuts \
             {ids:?}; note: {:?}",
            gs.version(),
            note(&ids)
        );
        session.close().await.expect("Session.Close");
    }
}
