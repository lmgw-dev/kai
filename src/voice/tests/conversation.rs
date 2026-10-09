//! WP6 through the core, against the scripted lmgw: the folder's lookup and creation, the no-model
//! path and its recovery on `folder.updated`, `current` after the folder's idle time, after the
//! thread was deleted or archived, the folder's own events, Open in lmgw, a 401 on `current`.

use std::time::Duration;

use tokio::time::sleep;

use super::super::link::{Refusal, ThreadRef};
use super::super::mock::{MockLink, refusal};
use super::{By, Command, FeedFact, Link, LinkEvent, Pairing, rig, rig_with};
use crate::config::Config;
use crate::conversation::FolderInfo;
use crate::lmgw_link::FolderEvent;
use crate::tray::model::render;

fn calls(r: &super::Rig) -> usize {
    r.mock.with(|s| s.current_calls.len())
}

fn remembered(r: &super::Rig) -> Option<i64> {
    Config::load(&r.config_file).unwrap().conversation.folder_id
}

/// The mock's conversation moves to a new thread at its next `current`.
fn next_thread(r: &super::Rig, id: i64) {
    r.mock.with(|s| {
        s.thread = ThreadRef {
            id,
            title: String::new(),
        };
        s.rolled_over = true;
    });
}

#[tokio::test(start_paused = true)]
async fn a_first_run_creates_the_folder_says_the_steps_and_recovers_on_folder_updated() {
    let mock = MockLink::default();
    mock.with(|s| s.folders.clear());
    // No folder id: Kai looks the folder up by name.
    let mut r = rig_with("[conversation]\n", mock).await;
    let n = r.notice("Kai: set up the folder 'Kai' in lmgw").await;
    assert!(
        n.body.starts_with(
            "set the folder 'Kai''s model and mark it as an ongoing conversation in lmgw: in \
             Chat, the folder's menu → Folder settings"
        ),
        "{}",
        n.body
    );
    assert_eq!(r.mock.with(|s| s.created.clone()), ["Kai"]);
    let m = r
        .until(|m| {
            m.conversation
                .iter()
                .any(|n| n.starts_with("voice waits for its folder"))
        })
        .await;
    assert!(
        render(&m)
            .notes
            .iter()
            .any(|n| n.starts_with("voice waits for its folder")),
        "the tooltip says it"
    );
    assert_eq!(m.voice.thread, None);
    assert_eq!(remembered(&r), None, "a plain folder is not remembered");
    // A press: lmgw refuses current (not ongoing), and the owner hears the steps again.
    r.send(Command::Talk(true));
    sleep(Duration::from_millis(400)).await;
    r.send(Command::Talk(false));
    let n = r.notice("Kai: set up the folder 'Kai' in lmgw").await;
    assert!(n.body.contains("Ongoing conversation"));
    assert_eq!(r.mock.with(|s| s.current_folders.clone()), [1]);
    assert_eq!(r.mock.opens(), 0);
    let m = r.until(|m| !m.voice.on).await;
    assert!(
        !m.voice
            .notes
            .iter()
            .any(|n| n.contains("voice could not start")),
        "the steps say it, once: {:?}",
        m.voice.notes
    );
    // The owner sets the model and marks it ongoing on the dashboard: folder.updated.
    let ready = FolderInfo {
        id: 1,
        name: "Kai".into(),
        model: Some("kai-chat".into()),
        idle_minutes: Some(120),
        archive_days: None,
        purge_days: Some(365),
        tool_labels: Vec::new(),
        profile_id: None,
    };
    r.mock.with(|s| s.folders[0] = ready.clone());
    r.feed
        .raise(LinkEvent::Folder(FolderEvent::Changed(ready.clone())));
    let m = r.until(|m| m.voice.thread.is_some()).await;
    assert!(m.conversation.is_empty(), "{:?}", m.conversation);
    assert_eq!(m.voice.thread.as_deref(), Some("Kai"));
    assert_eq!(render(&m).title, "Kai: Kai");
    assert_eq!(remembered(&r), Some(1), "an ongoing folder is remembered");
    assert_eq!(r.mock.with(|s| s.current_folders.clone()), [1, 1]);
    // Voice works now.
    r.listening_on().await;
    r.mock.socket(0, |s| assert_eq!(s.thread, 812));
    r.no_notice("Kai: set up the folder 'Kai' in lmgw");
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn a_remembered_folder_lmgw_lost_gives_way_to_the_named_one() {
    let mock = MockLink::default();
    mock.with(|s| s.folders[0].id = 8);
    let mut r = rig_with("[conversation]\nfolder_id = 3\n", mock).await;
    r.until(|m| m.voice.thread.is_some()).await;
    assert_eq!(remembered(&r), Some(8));
    assert_eq!(r.mock.with(|s| s.current_folders.clone()), [8]);
    assert!(r.mock.with(|s| s.created.is_empty()));
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn on_at_start_waits_for_the_folder_on_a_first_run() {
    let mut r = rig_with(
        "[conversation]\n[voice]\non_at_start = true\n",
        MockLink::default(),
    )
    .await;
    r.created(0).await;
    r.mock.socket(0, |s| assert_eq!(s.thread, 812));
    assert_eq!(r.mock.with(|s| s.current_folders.clone()), [3]);
    assert_eq!(remembered(&r), Some(3));
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn the_folders_idle_time_with_no_turn_rolls_the_conversation_over_at_a_quiet_point() {
    let mock = MockLink::default();
    mock.with(|s| s.folders[0].idle_minutes = Some(1));
    let mut r = rig_with("", mock).await;
    r.listening_on().await;
    assert_eq!(calls(&r), 2, "the start's and the session's");
    // A turn 30 s in: the idle time counts from it.
    sleep(Duration::from_secs(30)).await;
    r.feed.fact(FeedFact::TurnDone { thread_id: 812 });
    sleep(Duration::from_secs(45)).await;
    assert_eq!(
        calls(&r),
        2,
        "75 s after the start, 45 s after the turn: not idle"
    );
    next_thread(&r, 900);
    sleep(Duration::from_secs(20)).await;
    assert_eq!(calls(&r), 3, "idle 60 s after the turn: Kai asks");
    // lmgw rolled over: Kai rebinds (nothing played, so at once).
    r.created(1).await;
    r.mock.socket(1, |s| assert_eq!(s.thread, 900));
    assert!(r.mock.socket(0, |s| s.closed_by_client));
    let m = r.until(|m| m.voice.on).await;
    assert_eq!(m.voice.thread.as_deref(), Some(crate::voice::UNTITLED));
    // The new thread has no message: never idle, so no timer until its first turn.
    sleep(Duration::from_secs(300)).await;
    assert_eq!(calls(&r), 3);
    // Its first turn (Kai's own, as the feed says it) starts the timer; lmgw keeps the thread.
    r.feed.fact(FeedFact::TurnStarted {
        thread_id: 900,
        by: By {
            name: "device 'desktop'".into(),
            own: true,
        },
    });
    r.feed.fact(FeedFact::TurnDone { thread_id: 900 });
    sleep(Duration::from_secs(61)).await;
    assert_eq!(calls(&r), 4);
    assert_eq!(r.mock.opens(), 2, "the same thread: no rebind");
    // Not rolled over: the timer starts over from the answer.
    sleep(Duration::from_secs(61)).await;
    assert_eq!(calls(&r), 5);
    r.quit().await;
}

/// Review KAI-2 K2-8: the rollover runs on the wall clock. A suspend stops tokio's clock but not
/// lmgw's: logind's resume makes Kai look at once, and without it the look every 30 s finds it.
#[tokio::test(start_paused = true)]
async fn a_suspend_past_the_idle_time_is_found_at_the_resume() {
    use std::sync::atomic::Ordering;
    let mock = MockLink::default();
    mock.with(|s| s.folders[0].idle_minutes = Some(120));
    let mut r = rig_with("", mock).await;
    r.listening_on().await;
    r.feed.fact(FeedFact::TurnDone { thread_id: 812 });
    sleep(Duration::from_secs(60)).await;
    assert_eq!(calls(&r), 2, "the start's and the session's");
    // The machine sleeps four hours: the wall clock moves, tokio's does not.
    next_thread(&r, 900);
    r.wall_skew.store(4 * 3600, Ordering::Relaxed);
    r.send(Command::Resumed);
    r.until_mock("current at the resume", |m| {
        m.with(|s| s.current_calls.len()) == 3
    })
    .await;
    r.created(1).await;
    r.mock.socket(1, |s| assert_eq!(s.thread, 900));
    r.quit().await;

    // No word from logind: the look every 30 s finds it.
    let mock = MockLink::default();
    mock.with(|s| s.folders[0].idle_minutes = Some(120));
    let mut r = rig_with("", mock).await;
    r.listening_on().await;
    r.feed.fact(FeedFact::TurnDone { thread_id: 812 });
    sleep(Duration::from_secs(5)).await;
    r.wall_skew.store(4 * 3600, Ordering::Relaxed);
    sleep(Duration::from_secs(31)).await;
    assert_eq!(calls(&r), 3, "within IDLE_CHECK of the resume");
    r.quit().await;
}

/// Review KAI-2 K2-8: lmgw says when the thread's newest message came (`last_message_at`): the
/// rollover counts from it, as lmgw's does, not from Kai's start.
#[tokio::test(start_paused = true)]
async fn the_rollover_counts_from_lmgws_newest_message() {
    let unix_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mock = MockLink::default();
    mock.with(|s| {
        s.folders[0].idle_minutes = Some(1);
        s.last_message_at = Some(unix_now - 50);
    });
    let r = rig_with("", mock).await;
    assert_eq!(calls(&r), 1);
    sleep(Duration::from_secs(5)).await;
    assert_eq!(calls(&r), 1, "45 s after the newest message: not idle");
    sleep(Duration::from_secs(8)).await;
    assert_eq!(
        calls(&r),
        2,
        "60 s after the newest message, not after Kai's start"
    );
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn the_idle_time_passing_during_a_turn_of_another_principal_waits_for_it() {
    let mock = MockLink::default();
    mock.with(|s| s.folders[0].idle_minutes = Some(1));
    let r = rig_with("", mock).await;
    assert_eq!(calls(&r), 1);
    r.feed.fact(FeedFact::TurnStarted {
        thread_id: 812,
        by: By {
            name: "device 'phone'".into(),
            own: false,
        },
    });
    sleep(Duration::from_secs(90)).await;
    assert_eq!(
        calls(&r),
        1,
        "a turn runs: lmgw would keep the thread anyway"
    );
    r.feed.fact(FeedFact::TurnDone { thread_id: 812 });
    sleep(Duration::from_secs(61)).await;
    assert_eq!(calls(&r), 2);
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn a_deleted_or_archived_thread_makes_kai_ask_for_the_conversation_again() {
    let mut r = rig("").await;
    assert_eq!(calls(&r), 1);
    next_thread(&r, 900);
    r.feed.fact(FeedFact::ThreadGone { thread_id: 812 });
    r.until_mock("current again", |m| m.with(|s| s.current_calls.len()) == 2)
        .await;
    let m = r
        .until(|m| m.voice.thread.as_deref() == Some(crate::voice::UNTITLED))
        .await;
    assert_eq!(render(&m).title, "Kai: New chat");
    // The delete's folder.current to none names the thread that went: asked for already.
    r.feed.raise(LinkEvent::Folder(FolderEvent::Current {
        folder_id: 3,
        thread_id: None,
        previous: Some(812),
        reason: "gone".into(),
    }));
    sleep(Duration::from_millis(200)).await;
    assert_eq!(calls(&r), 2);
    // The new thread gets its title from the first send.
    r.feed.fact(FeedFact::ThreadChanged {
        thread_id: 900,
        title: "Morgenrunde".into(),
        archived: false,
        last_message_at: None,
    });
    let m = r
        .until(|m| m.voice.thread.as_deref() == Some("Morgenrunde"))
        .await;
    assert_eq!(render(&m).title, "Kai: Morgenrunde");
    // Archived by hand: asked again.
    next_thread(&r, 901);
    r.feed.fact(FeedFact::ThreadChanged {
        thread_id: 900,
        title: "Morgenrunde".into(),
        archived: true,
        last_message_at: None,
    });
    r.until_mock("current a third time", |m| {
        m.with(|s| s.current_calls.len()) == 3
    })
    .await;
    // A device's feed may name no previous thread: the conversation's thread ended all the same.
    next_thread(&r, 902);
    r.feed.raise(LinkEvent::Folder(FolderEvent::Current {
        folder_id: 3,
        thread_id: None,
        previous: None,
        reason: "first".into(),
    }));
    r.until_mock("current a fourth time", |m| {
        m.with(|s| s.current_calls.len()) == 4
    })
    .await;
    // Another thread's events, another folder's current, and a folder that stopped being
    // ongoing ask nothing.
    r.feed.fact(FeedFact::ThreadGone { thread_id: 5 });
    r.feed.raise(LinkEvent::Folder(FolderEvent::Current {
        folder_id: 99,
        thread_id: Some(5000),
        previous: Some(902),
        reason: "idle".into(),
    }));
    r.feed.raise(LinkEvent::Folder(FolderEvent::Current {
        folder_id: 3,
        thread_id: None,
        previous: Some(902),
        reason: "not_ongoing".into(),
    }));
    sleep(Duration::from_millis(500)).await;
    assert_eq!(calls(&r), 4);
    assert_eq!(r.mock.opens(), 0, "no session: nothing to rebind");
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn the_folders_deletion_ends_the_session_and_kai_looks_again() {
    let mut r = rig("").await;
    r.listening_on().await;
    r.mock.with(|s| s.folders.clear());
    r.feed.raise(LinkEvent::Folder(FolderEvent::Gone(3)));
    let n = r.notice("Kai: voice stopped").await;
    assert_eq!(
        n.body,
        "the folder 'Kai' of Kai's conversation was deleted in lmgw"
    );
    r.until_mock("the session's close", |m| {
        m.socket(0, |s| s.closed_by_client)
    })
    .await;
    r.notice("Kai: set up the folder 'Kai' in lmgw").await;
    assert_eq!(r.mock.with(|s| s.created.clone()), ["Kai"]);
    let m = r.until(|m| !m.voice.on && m.voice.thread.is_none()).await;
    assert!(m.voice.notes.iter().any(|n| n.contains("deleted in lmgw")));
    assert_eq!(remembered(&r), None);
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn open_in_lmgw_opens_the_threads_dashboard_page() {
    let mut r = rig("").await;
    r.send(Command::OpenInLmgw);
    assert_eq!(
        r.opened.recv().await.unwrap(),
        "http://127.0.0.1:1/chat?t=812"
    );
    r.quit().await;
    // The folder waits for the owner's steps: its settings form (lmgw's deep link).
    let mock = MockLink::default();
    mock.with(|s| s.folders.clear());
    let mut r = rig_with("[conversation]\n", mock).await;
    r.notice("Kai: set up the folder 'Kai' in lmgw").await;
    r.send(Command::OpenInLmgw);
    assert_eq!(
        r.opened.recv().await.unwrap(),
        "http://127.0.0.1:1/chat?folder=1&settings=1"
    );
    r.quit().await;
    // No folder and no thread known: the Chat page.
    let mock = MockLink::default();
    mock.with(|s| {
        s.folders.clear();
        s.folders_error = Some(Refusal::no_answer("lmgw is not reachable"));
    });
    let mut r = rig_with("[conversation]\n", mock).await;
    sleep(Duration::from_millis(200)).await;
    r.send(Command::OpenInLmgw);
    assert_eq!(r.opened.recv().await.unwrap(), "http://127.0.0.1:1/chat");
    r.quit().await;
}

#[tokio::test(start_paused = true)]
async fn a_401_on_current_says_pair_again_with_the_servers_words() {
    let mut r = rig("").await;
    r.mock.with(|s| {
        s.current_error = Some(refusal(
            401,
            "device_key_unknown",
            "this device key is no longer valid: pair the device again",
        ))
    });
    r.send(Command::SetListening(true));
    let n = r.notice("Kai: pair again").await;
    assert!(
        n.body
            .contains("this device key is no longer valid: pair the device again"),
        "{}",
        n.body
    );
    let m = r
        .until(|m| matches!(m.pairing, Pairing::Revoked { .. }))
        .await;
    assert!(!m.voice.on);
    assert!(!matches!(
        m.pairing,
        Pairing::Paired {
            link: Link::Up { .. },
            ..
        }
    ));
    r.quit().await;
}
