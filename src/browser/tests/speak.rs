//! WP9's announcements through the fake Firefox and a fake speech (kai-firefox design §7.1, §7.2):
//! the events turned on after each `welcome` and when `browser.announce` or the switch changes;
//! events of a `none` site dropped, a download by its URLs, its referrer and its source; the
//! page's text said as the page's, last; a newer focus or atomic region's change replacing the one
//! that waits while a log's changes queue; a field's error and its live region said once in either
//! order; Kai's own actions; `seq` order; the gate the speech runs again (switched off, a site
//! that became `none`, Firefox's focus now); and `browser_context`'s list with what it dropped
//! counted. Every drop comes with its positive control.

use std::time::Duration;

use serde_json::{Value, json};

use super::fake::{Host, all_ops, answer, shop};
use crate::browser::settings::{BrowserConfig, Sites};
use crate::host::settings::PolicyConfig;
use crate::voice::speak::{Dropped, Kind, Said, Spoken, Utterance};

/// Every operation, the events' and reading aloud's with them.
pub fn ops() -> Vec<&'static str> {
    let mut ops = all_ops();
    ops.extend([
        "events.config@1",
        "read.open@1",
        "read.paragraph@1",
        "read.mark@1",
        "read.close@1",
    ]);
    ops
}

pub fn announcing(none: &[&str]) -> BrowserConfig {
    BrowserConfig {
        announce: true,
        sites: Sites {
            none: none.iter().map(|s| (*s).to_owned()).collect(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// An event of the user's tab 12 (the checkout), focused, at `1_767_225_660_020` unless `more`
/// says when.
pub fn of_checkout(kind: &str, more: Value) -> Value {
    let mut v = json!({"kind": kind, "at": 1_767_225_660_020i64, "focused": true, "tab": 12,
                       "title": "Checkout", "url": "https://shop.example/cart", "frame": 0,
                       "origin": "https://shop.example", "by_kai": false});
    for (k, x) in more.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

fn en(u: &Utterance) -> &str {
    u.text.en()
}

/// What `browser_context` lists after its first line about the tabs.
async fn listed(h: &Host) -> Vec<String> {
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (error, said) = h.owner("browser_context", json!({})).await;
    assert!(!error, "{said}");
    said.lines()
        .skip_while(|l| !l.starts_with("announcements"))
        .map(str::to_owned)
        .collect()
}

/// §7.1: a link starts with events off; Kai sends `events.config@1` after its welcome and again
/// whenever `browser.announce` changes what it should be, in order; the hands-and-eyes switch
/// does not turn them on.
#[tokio::test]
async fn the_events_follow_announce_alone() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &ops(), shop()).await;
    let first = ff.arrived("events.config@1", 1).await;
    assert_eq!(
        first[0],
        json!({"on": false}),
        "off by default, said all the same"
    );
    h.set(PolicyConfig::default(), announcing(&[]));
    let on = ff.arrived("events.config@1", 2).await;
    assert_eq!(on[1], json!({"on": true, "quiet_ms": 1000}));
    // The switch changes nothing for them; a new quiet period goes out.
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            hands_and_eyes: true,
            announce: true,
            announce_quiet_ms: 250,
            ..Default::default()
        },
    );
    let quiet = ff.arrived("events.config@1", 3).await;
    assert_eq!(quiet[2], json!({"on": true, "quiet_ms": 250}));
    // Hands and eyes on with announce off: events off, whatever the switch.
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            hands_and_eyes: true,
            announce_quiet_ms: 250,
            ..Default::default()
        },
    );
    let off = ff.arrived("events.config@1", 4).await;
    assert_eq!(off[3], json!({"on": false}));
    // A second Firefox gets the policy as it is at its welcome.
    h.set(PolicyConfig::default(), announcing(&[]));
    ff.arrived("events.config@1", 5).await;
    let other = h.firefox("deadbeef", &ops(), shop()).await;
    assert_eq!(
        other.arrived("events.config@1", 1).await[0],
        json!({"on": true, "quiet_ms": 1000})
    );
}

/// §6.4, §7.2: an event of a `none` site (the tab's, a frame's own origin, a tab it opened) is
/// dropped; the others are said as the page's in English and German, with a gate that reads
/// Firefox's focus as the link's newest event says it, and `browser_context` lists them once.
#[tokio::test]
async fn none_sites_say_nothing_and_page_text_is_the_pages() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &ops(), shop()).await;
    h.set(PolicyConfig::default(), announcing(&["bank.example"]));
    ff.arrived("events.config@1", 2).await;
    let bank = json!({"tab": 3, "title": "Bank", "url": "https://bank.example/",
                      "origin": "https://bank.example"});
    ff.event(of_checkout("loaded", bank.clone()));
    ff.event(of_checkout(
        "live",
        json!({"frame": 7, "origin": "https://bank.example", "text": "Balance 4 000",
               "region": {"ref": 9, "role": "status", "name": ""}, "politeness": "polite",
               "atomic": true}),
    ));
    ff.event(of_checkout(
        "tab_opened",
        json!({"opened_tab": 15, "opened_url": "https://bank.example/login",
               "opened_window": 1, "new_window": false}),
    ));
    // The positive control: the same tab_opened to an `act` site is spoken, its site the page's.
    ff.event(of_checkout(
        "tab_opened",
        json!({"opened_tab": 16, "opened_url": "https://help.example/faq",
               "opened_window": 1, "new_window": false}),
    ));
    let opened = h.mouth.next().await;
    assert_eq!(
        en(&opened),
        "The page opened a new tab; the page gives its address on: help.example"
    );
    ff.event(of_checkout(
        "field_error",
        json!({"field": {"ref": 214, "role": "textbox", "name": "E-mail"},
               "error": "invalid address", "description": null, "regions": []}),
    ));
    let u = h.mouth.next().await;
    assert_eq!(
        en(&u),
        "The page says, at the field “E-mail”: invalid address"
    );
    let crate::voice::speak::Text::Kai(w) = &u.text else {
        panic!("Kai's words");
    };
    assert_eq!(w.de, "Die Seite sagt, zum Feld „E-mail“: invalid address");
    assert_eq!(u.kind, Kind::Announcement { urgent: false });
    assert!(u.voice.is_none(), "Kai's own voice");
    assert_eq!(
        (u.gate.as_ref().unwrap())(),
        Ok(true),
        "Firefox has the focus"
    );
    h.mouth.silent(100).await;
    answer(&opened, Said::Done(Spoken::Heard));
    answer(&u, Said::Started);
    answer(&u, Said::Done(Spoken::Heard));
    // An alert while Firefox lost the focus: the gate says so (the speech drops it).
    ff.event(of_checkout(
        "live",
        json!({"focused": false, "text": "Payment failed: card declined.",
               "region": {"ref": 88, "role": "alert", "name": ""}, "politeness": "assertive",
               "atomic": true}),
    ));
    let alert = h.mouth.next().await;
    assert_eq!(en(&alert), "The page says: Payment failed: card declined.");
    assert_eq!(alert.kind, Kind::Announcement { urgent: true });
    assert_eq!((alert.gate.as_ref().unwrap())(), Ok(false));
    answer(&alert, Said::Done(Spoken::Dropped(Dropped::NotFocused)));
    let list = listed(&h).await;
    assert_eq!(
        list[0], "announcements since the last look (3):",
        "{list:?}"
    );
    assert!(
        list[2].ends_with("tab 12: error at 214 textbox \"E-mail\": \"invalid address\""),
        "{list:?}"
    );
    assert!(
        list[3].ends_with(
            "tab 12: the page says (88 alert): \"Payment failed: card declined.\" (not spoken: \
             Firefox did not have the focused window and conversation mode is off)"
        ),
        "{list:?}"
    );
    let all = list.join("\n");
    assert!(!all.contains("bank") && !all.contains("Balance"), "{all}");
    assert_eq!(
        listed(&h).await,
        ["announcements since the last look: none"]
    );
}

/// The contract's download rule (§6.4): dropped when its `url`, `final_url`, `referrer`,
/// `source.url` or `source.origin` is a `none` site, or when neither `source` nor `referrer` names
/// a page while a rule is `none` (the strictest); spoken when everything it names is `act`.
#[tokio::test]
async fn a_download_from_a_none_tab_says_nothing() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &ops(), shop()).await;
    h.set(PolicyConfig::default(), announcing(&["bank.example"]));
    ff.arrived("events.config@1", 2).await;
    let download = |file: &str, more: Value| {
        let mut v = json!({"kind": "download_done", "at": 1_767_225_660_020i64,
                           "focused": true, "by_kai": false, "file": file,
                           "path": format!("/home/u/{file}"), "size": 10,
                           "url": "https://cdn.example/f", "final_url": "https://cdn.example/f",
                           "referrer": "https://shop.example/", "source": null});
        for (k, x) in more.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    };
    // The user's tab is on a none site; the download's own URLs and referrer are act.
    ff.event(download(
        "a.pdf",
        json!({"source": {"tab": 3, "title": "Bank", "url": "https://bank.example/statements",
                          "frame": 0, "origin": "https://shop.example"}}),
    ));
    // The navigating document's origin is a none site (a frame of it).
    ff.event(download(
        "b.pdf",
        json!({"source": {"tab": 12, "title": "Checkout", "url": "https://shop.example/cart",
                          "frame": 4, "origin": "https://bank.example"}}),
    ));
    // Its final URL is.
    ff.event(download(
        "c.pdf",
        json!({"final_url": "https://bank.example/c.pdf"}),
    ));
    // Neither source nor referrer: the strictest rule (none).
    ff.event(download("d.pdf", json!({"referrer": null})));
    // The positive control: everything it names is act, a source without a referrer included.
    ff.event(download(
        "e.pdf",
        json!({"referrer": null,
               "source": {"tab": 12, "title": "Checkout", "url": "https://shop.example/cart",
                          "frame": 0, "origin": "https://shop.example"}}),
    ));
    let u = h.mouth.next().await;
    assert_eq!(
        en(&u),
        "A download finished; the page names the file: e.pdf"
    );
    h.mouth.silent(100).await;
    let all = listed(&h).await.join("\n");
    for f in ["a.pdf", "b.pdf", "c.pdf", "d.pdf"] {
        assert!(!all.contains(f), "{all}");
    }
}

/// A newer focus replaces the one still waiting, and so does an atomic region's change; a
/// non-atomic region's changes (a log) each say what was added, so they queue; once one plays it
/// is not replaced. Kai's own actions; events out of their link's `seq` order are dropped.
#[tokio::test]
async fn newest_only_where_the_contract_says_and_kais_own_actions() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &ops(), shop()).await;
    h.set(PolicyConfig::default(), announcing(&[]));
    ff.arrived("events.config@1", 2).await;
    ff.event(of_checkout(
        "focus",
        json!({"element": {"ref": 5, "role": "button", "name": "Back"}}),
    ));
    let first = h.mouth.next().await;
    assert_eq!(en(&first), "Focus on a button, the page says: Back");
    ff.event(of_checkout(
        "focus",
        json!({"element": {"ref": 6, "role": "link", "name": "Help"}}),
    ));
    let second = h.mouth.next().await;
    assert!(first.cancel.is_cancelled(), "the older focus is dropped");
    answer(&second, Said::Started);
    tokio::time::sleep(Duration::from_millis(20)).await;
    ff.event(of_checkout(
        "focus",
        json!({"element": {"ref": 7, "role": "textbox", "name": "Search"}}),
    ));
    let third = h.mouth.next().await;
    assert!(!second.cancel.is_cancelled(), "it plays to its end");
    answer(&third, Said::Done(Spoken::Heard));

    // An atomic region: its whole text, the newest only.
    let status = |text: &str| {
        of_checkout(
            "live",
            json!({"text": text, "region": {"ref": 40, "role": "status", "name": ""},
                   "politeness": "polite", "atomic": true}),
        )
    };
    ff.event(status("3 items"));
    let old = h.mouth.next().await;
    ff.event(status("4 items"));
    let new = h.mouth.next().await;
    assert!(old.cancel.is_cancelled() && !new.cancel.is_cancelled());
    // A log: each change what was added, none replaced.
    let log = |text: &str| {
        of_checkout(
            "live",
            json!({"text": text, "region": {"ref": 41, "role": "log", "name": ""},
                   "politeness": "polite", "atomic": false}),
        )
    };
    ff.event(log("Anna: hi"));
    let a = h.mouth.next().await;
    ff.event(log("Ben: hello"));
    let b = h.mouth.next().await;
    assert_eq!(
        (en(&a), en(&b)),
        ("The page says: Anna: hi", "The page says: Ben: hello")
    );
    assert!(
        !a.cancel.is_cancelled() && !b.cancel.is_cancelled(),
        "both wait their turn"
    );

    // Kai's own navigation: kept, not spoken; a dialog after Kai's action is.
    ff.event(of_checkout("loaded", json!({"by_kai": true})));
    ff.event(of_checkout(
        "dialog",
        json!({"by_kai": true, "open": true,
               "dialog": {"ref": 50, "role": "dialog", "name": "Confirm order"}}),
    ));
    let dialog = h.mouth.next().await;
    assert_eq!(
        en(&dialog),
        "After Kai's action: a dialog opened, the page says: Confirm order"
    );
    h.mouth.silent(50).await;

    // seq: an event that is not after its link's last is dropped; the next one is not.
    ff.event(of_checkout(
        "live",
        json!({"seq": 100, "text": "first", "region": {"ref": 60, "role": "log"}}),
    ));
    assert_eq!(en(&h.mouth.next().await), "The page says: first");
    ff.event(of_checkout(
        "live",
        json!({"seq": 99, "text": "late", "region": {"ref": 60, "role": "log"}}),
    ));
    ff.event(of_checkout(
        "live",
        json!({"seq": 101, "text": "second", "region": {"ref": 60, "role": "log"}}),
    ));
    assert_eq!(en(&h.mouth.next().await), "The page says: second");
    h.mouth.silent(50).await;

    let all = listed(&h).await.join("\n");
    assert!(
        all.contains("loaded \"Checkout\" (after Kai's action)"),
        "{all}"
    );
    assert!(
        all.contains("(not spoken: a newer one replaced it)"),
        "the dropped focus: {all}"
    );
    assert!(!all.contains("late"), "{all}");
}

/// The contract's rule for a field's error and its live region: the live event is not spoken when
/// its region is in the error's `regions` and its text is part of the error, within
/// `announce_quiet_ms` by `at`, in either order; outside that window, or with other text, it is.
#[tokio::test]
async fn a_field_error_and_its_region_are_spoken_once_in_either_order() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &ops(), shop()).await;
    h.set(PolicyConfig::default(), announcing(&[]));
    ff.arrived("events.config@1", 2).await;
    let t = 1_767_225_660_020i64;
    let error = |at: i64| {
        of_checkout(
            "field_error",
            json!({"at": at, "field": {"ref": 214, "role": "textbox", "name": "E-mail"},
                   "error": "Enter a valid e-mail address, such as anna@mail.example",
                   "description": null,
                   "regions": [{"ref": 40, "role": "status", "name": ""}]}),
        )
    };
    let live = |at: i64, text: &str| {
        of_checkout(
            "live",
            json!({"at": at, "text": text, "atomic": true, "politeness": "polite",
                   "region": {"ref": 40, "role": "status", "name": ""}}),
        )
    };
    // The error first (one batch: the same at): the region's change is not spoken.
    ff.event(error(t));
    let e1 = h.mouth.next().await;
    assert!(en(&e1).starts_with("The page says, at the field “E-mail”: Enter a valid"));
    ff.event(live(t, "Enter a valid e-mail address"));
    h.mouth.silent(100).await;
    // The region first, still waiting: the error takes it back.
    ff.event(live(
        t + 5000,
        "Enter a valid e-mail address, such as anna@mail.example",
    ));
    let l2 = h.mouth.next().await;
    ff.event(error(t + 5400));
    let e2 = h.mouth.next().await;
    assert!(
        l2.cancel.is_cancelled(),
        "the region's change is taken back"
    );
    assert!(!e2.cancel.is_cancelled());
    // Positive controls: the same text outside the quiet period is spoken; so is a change of the
    // region that the error does not hold, and the error's text does not hold the change.
    ff.event(live(t + 9000, "Enter a valid e-mail address"));
    assert!(!h.mouth.next().await.cancel.is_cancelled());
    ff.event(error(t + 20_000));
    let _ = h.mouth.next().await;
    ff.event(live(t + 20_100, "Saved"));
    assert_eq!(en(&h.mouth.next().await), "The page says: Saved");
    // Containment runs the right way: a change holding more than the error is the page's news.
    ff.event(live(
        t + 20_200,
        "Enter a valid e-mail address, such as anna@mail.example. Or call us.",
    ));
    assert!(en(&h.mouth.next().await).ends_with("Or call us."));
    h.mouth.silent(50).await;
    let all = listed(&h).await.join("\n");
    assert_eq!(
        all.matches("(not spoken: the field's error says it)")
            .count(),
        2,
        "{all}"
    );
}

/// The gate and what waits follow the policy: switching announcements off takes back every one
/// that waits and the gate drops it; a site that became `none` likewise, while one of another site
/// stays; `browser_context` counts what went unspoken for its age and the screen lock, and what
/// aged out of its list.
#[tokio::test]
async fn what_waits_dies_with_the_switch_or_the_site_and_drops_are_counted() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &ops(), shop()).await;
    h.set(PolicyConfig::default(), announcing(&[]));
    ff.arrived("events.config@1", 2).await;
    ff.event(of_checkout("loaded", json!({})));
    let shop_one = h.mouth.next().await;
    ff.event(of_checkout(
        "loaded",
        json!({"tab": 3, "title": "Mail", "url": "https://mail.example/inbox",
               "origin": "https://mail.example"}),
    ));
    let mail_one = h.mouth.next().await;
    // mail.example becomes none: its announcement dies, the shop's stays.
    h.set(PolicyConfig::default(), announcing(&["mail.example"]));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(mail_one.cancel.is_cancelled());
    assert_eq!((mail_one.gate.as_ref().unwrap())(), Err(Dropped::Site));
    assert!(!shop_one.cancel.is_cancelled());
    assert_eq!((shop_one.gate.as_ref().unwrap())(), Ok(true));
    // Switched off: what waits dies.
    h.set(PolicyConfig::default(), BrowserConfig::default());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(shop_one.cancel.is_cancelled());
    assert_eq!((shop_one.gate.as_ref().unwrap())(), Err(Dropped::Off));

    // The counts: two unspoken for their age, one for the lock, two aged out of the list.
    let mut config = announcing(&[]);
    config.announce_keep_s = 1;
    h.set(PolicyConfig::default(), config);
    ff.arrived("events.config@1", 4).await;
    let _ = listed(&h).await;
    for (i, d) in [Dropped::Stale(60), Dropped::Stale(60), Dropped::Locked]
        .into_iter()
        .enumerate()
    {
        ff.event(of_checkout(
            "live",
            json!({"text": format!("change {i}"),
                                            "region": {"ref": 70 + i, "role": "log"}}),
        ));
        answer(&h.mouth.next().await, Said::Done(Spoken::Dropped(d)));
    }
    let list = listed(&h).await;
    assert!(
        list.contains(
            &"2 announcements were not spoken: older than browser.announce_max_age_s = 60 s \
              when their turn came"
                .to_owned()
        ),
        "{list:?}"
    );
    assert!(
        list.contains(&"1 announcement was dropped while the screen was locked".to_owned()),
        "{list:?}"
    );
    ff.event(of_checkout(
        "live",
        json!({"text": "a", "region": {"ref": 80, "role": "log"}}),
    ));
    answer(&h.mouth.next().await, Said::Done(Spoken::Heard));
    ff.event(of_checkout(
        "live",
        json!({"text": "b", "region": {"ref": 81, "role": "log"}}),
    ));
    answer(&h.mouth.next().await, Said::Done(Spoken::Heard));
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let list = listed(&h).await;
    assert_eq!(
        list,
        [
            "announcements since the last look: none",
            "2 older announcements were dropped from this list (kept for \
             browser.announce_keep_s = 1 s)"
        ]
    );
}
