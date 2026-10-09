//! WP22: `personality_set` and the tray's Personality radio through the core, against the scripted
//! lmgw: the tool comes in as `Command::Tool` as the host sends it, the feed's `profile.*` events
//! as the link reports them, and the tray model is read as the tray gets it.

use serde_json::{Map, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::super::link::ProfileList;
use super::super::mock::{MockLink, refusal};
use super::{Command, Rig, rig_with};
use crate::core::calls::CoreCall;
use crate::host::policy::Caller;
use crate::host::registry::{Args, Block, Call, Reply};
use crate::lmgw_link::{LinkEvent, ProfileEvent};
use crate::tray::model::{MenuEntry, TrayModel, render};

fn list(names: &[(i64, &str)], global: Option<i64>) -> ProfileList {
    serde_json::from_value(json!({
        "profiles": names.iter().map(|(id, n)| json!({"id": id, "name": n})).collect::<Vec<_>>(),
        "default_profile_id": global,
    }))
    .unwrap()
}

/// A rig whose lmgw has Calm (1) and Concise (2), Concise being the Chat's own default.
async fn profiled() -> Rig {
    let mock = MockLink::default();
    mock.with(|s| s.profiles = list(&[(1, "Calm"), (2, "Concise")], Some(2)));
    let mut r = rig_with("", mock).await;
    r.until(|m| !m.personality.options.is_empty()).await;
    r
}

async fn personality_set(r: &Rig, name: &str) -> Reply {
    personality_ask(r, name).await.expect("the core answers")
}

fn personality_ask(r: &Rig, name: &str) -> oneshot::Receiver<Reply> {
    let (area, spec) = kai_desktop::contract::find("personality_set").expect("a contract tool");
    let mut args = Map::new();
    args.insert("name".into(), json!(name));
    let (reply, answer) = oneshot::channel();
    r.send(Command::Tool(Box::new(CoreCall {
        area: area.name,
        call: Call::new(
            spec,
            Args::read(spec, Some(args)).unwrap(),
            Caller::Own,
            CancellationToken::new(),
        ),
        reply,
    })));
    answer
}

fn text(r: &Reply) -> String {
    match &r.blocks[..] {
        [Block::Text(t)] => t.clone(),
        other => panic!("not one text block: {other:?}"),
    }
}

fn labels(m: &TrayModel) -> Vec<&str> {
    m.personality
        .options
        .iter()
        .map(|o| o.label.as_str())
        .collect()
}

/// The profiles are read at `hello`: Default, naming the Chat's own default, then the names; the
/// submenu is in the paired menu. The feed's `profile.*` events move it.
#[tokio::test(start_paused = true)]
async fn the_tray_follows_the_profiles_and_the_feed() {
    let mut r = profiled().await;
    let m = r.model.borrow().clone();
    assert_eq!(labels(&m), ["Default (global: Concise)", "Calm", "Concise"]);
    assert_eq!(m.personality.selected, 0);
    let menu = render(&m).menu;
    assert!(
        menu.iter()
            .any(|e| matches!(e, MenuEntry::Personality { options, .. } if options.len() == 3)),
        "{menu:?}"
    );

    r.feed.raise(LinkEvent::Profile(ProfileEvent::Upsert {
        id: 3,
        name: "Zen".into(),
    }));
    let m = r.until(|m| m.personality.options.len() == 4).await;
    assert_eq!(
        labels(&m),
        ["Default (global: Concise)", "Calm", "Concise", "Zen"]
    );
    r.feed
        .raise(LinkEvent::Profile(ProfileEvent::Removed { id: 1 }));
    let m = r.until(|m| m.personality.options.len() == 3).await;
    assert_eq!(labels(&m), ["Default (global: Concise)", "Concise", "Zen"]);

    // The dashboard moved the conversation's thread (812) to Zen: the radio shows it.
    r.feed.raise(LinkEvent::Profile(ProfileEvent::Thread {
        thread_id: 812,
        profile_id: Some(3),
    }));
    let m = r.until(|m| m.personality.selected == 2).await;
    assert_eq!(m.personality.options[2].label, "Zen");
    r.quit().await;
}

/// The name is matched in any case; the folder is written with that profile; the radio follows;
/// `default` is null and the answer says it came to the Chat's own default.
#[tokio::test(start_paused = true)]
async fn personality_set_writes_the_folders_profile_and_says_what_default_came_to() {
    let mut r = profiled().await;
    let answer = personality_set(&r, "cALM").await;
    assert!(!answer.error, "{answer:?}");
    assert_eq!(
        text(&answer),
        "Personality set to Calm. The conversation's thread takes it from its next response."
    );
    assert_eq!(r.mock.with(|s| s.profile_sets.clone()), [(3, Some(1))]);
    let m = r.until(|m| m.personality.selected == 1).await;
    assert_eq!(m.personality.options[1].label, "Calm");

    let answer = personality_set(&r, "default").await;
    assert!(!answer.error, "{answer:?}");
    let said = text(&answer);
    assert!(
        said.contains("Default is lmgw's own choice") && said.contains("talks as Concise"),
        "{said}"
    );
    assert_eq!(
        r.mock.with(|s| s.profile_sets.clone()),
        [(3, Some(1)), (3, None)]
    );
    // The thread now talks with Concise, the Chat's default; the folder's choice is Default.
    let m = r.until(|m| m.personality.selected == 2).await;
    assert_eq!(m.personality.options[2].label, "Concise");
    r.quit().await;
}

/// An unknown name is the tool's error listing the names in one line, and nothing is written; a
/// refusal of lmgw is the tool's error too.
#[tokio::test(start_paused = true)]
async fn an_unknown_name_lists_the_candidates_and_writes_nothing() {
    let r = profiled().await;
    let answer = personality_set(&r, "jovial").await;
    assert!(answer.error);
    assert_eq!(
        text(&answer),
        "there is no profile named 'jovial'; the profiles are: Default, Calm, Concise"
    );
    assert!(r.mock.with(|s| s.profile_sets.is_empty()));
    r.mock
        .with(|s| s.profile_set_error = Some(refusal(400, "unknown_profile", "no such profile")));
    let answer = personality_set(&r, "calm").await;
    assert!(answer.error);
    assert!(text(&answer).contains("no such profile (unknown_profile)"));
    r.quit().await;
}

/// The tray's pick is the same operation; a refusal is a notification, and the radio stays.
#[tokio::test(start_paused = true)]
async fn the_trays_pick_sets_it_and_a_refusal_is_said() {
    let mut r = profiled().await;
    r.send(Command::SetPersonality(Some(1)));
    r.until(|m| m.personality.selected == 1).await;
    assert_eq!(r.mock.with(|s| s.profile_sets.clone()), [(3, Some(1))]);
    r.mock
        .with(|s| s.profile_set_error = Some(refusal(500, "boom", "lmgw broke")));
    r.send(Command::SetPersonality(Some(2)));
    let n = r.notice("Kai: the personality did not change").await;
    assert!(n.body.contains("lmgw broke"), "{}", n.body);
    assert_eq!(r.model.borrow().personality.selected, 1);
    r.quit().await;
}

/// An older lmgw has no profiles: no submenu, and the tool says why.
#[tokio::test(start_paused = true)]
async fn without_profiles_there_is_no_submenu() {
    let mock = MockLink::default();
    mock.with(|s| s.profiles_error = Some(refusal(404, "not_found", "no route")));
    let r = rig_with("", mock).await;
    assert!(r.model.borrow().personality.options.is_empty());
    assert!(
        !render(&r.model.borrow())
            .menu
            .iter()
            .any(|e| matches!(e, MenuEntry::Personality { .. }))
    );
    let answer = personality_set(&r, "calm").await;
    assert!(answer.error);
    assert!(text(&answer).starts_with("cannot read lmgw's profiles"));
    r.quit().await;
}

/// The set's requests do not hold the core: while lmgw is slow to take the profile, the core
/// still handles other commands (here the mute), and two picks in a row are written in order, the
/// last one standing.
#[tokio::test(start_paused = true)]
async fn a_pending_set_does_not_hold_the_core() {
    let mut r = profiled().await;
    r.mock
        .with(|s| s.profile_set_delay = std::time::Duration::from_secs(20));
    let mut first = personality_ask(&r, "calm");
    r.send(Command::SetPersonality(Some(2)));
    r.send(Command::ToggleMute);
    r.until(|m| m.voice.muted).await;
    assert!(first.try_recv().is_err(), "the set is still pending");
    assert!(r.mock.with(|s| s.profile_sets.is_empty()));

    let answer = first.await.expect("the core answers");
    assert!(!answer.error, "{answer:?}");
    // The tray's pick waited its turn and came last.
    let m = r.until(|m| m.personality.selected == 2).await;
    assert_eq!(m.personality.options[2].label, "Concise");
    assert_eq!(
        r.mock.with(|s| s.profile_sets.clone()),
        [(3, Some(1)), (3, Some(2))]
    );
    r.quit().await;
}
