//! What a desktop window shows of Firefox, over a fake linked Firefox: which windows are
//! Firefox's, which tab one shows, what Kai cannot tell (a title two windows fit, a tab not
//! settled, a page without a site), the refusals by reach, which links a look lists, and the frame
//! with the keyboard focus.

use kai_desktop::caps::windows::Window;
use serde_json::json;

use super::fake::{FakeFirefox, behind, context, front, listing, shown};
use super::*;

const PID: u32 = 4242;

fn window(id: &str, app: &str, title: &str, pid: u32, active: bool) -> Window {
    Window {
        id: id.into(),
        app: app.into(),
        title: title.into(),
        pid,
        active,
        ..Default::default()
    }
}

fn ff(id: &str, title: &str, active: bool) -> Window {
    window(id, "org.mozilla.firefox", title, PID, active)
}

/// Two windows: 1 shows the mail (the user's tab), 2 the shop, the news behind it.
fn two_windows() -> serde_json::Value {
    listing(
        vec![
            front(3, 1, "Inbox", "https://mail.example/inbox", true),
            front(5, 2, "Lamps", "https://shop.example/lamps", false),
            behind(6, 2, "News", "https://news.example/"),
        ],
        &[1, 2],
        Some(1),
    )
}

async fn sight(s: &Shown, w: &Window) -> Sight {
    s.look(s.sites(), &[w], None).await.sight(w)
}

#[tokio::test]
async fn a_window_shows_the_tab_in_front_its_title_names() {
    let fx = FakeFirefox::start(Some(PID), two_windows());
    let s = shown(&fx.links, &["mail.example"], &["shop.example"]);
    let mail = ff("w1", "Inbox — Mozilla Firefox", true);
    let shop = ff("w2", "Lamps — Mozilla Firefox", false);
    assert_eq!(sight(&s, &mail).await, Sight::Site(Rule::None));
    assert_eq!(sight(&s, &shop).await, Sight::Site(Rule::Read));
    let look = s.look(s.sites(), &[&mail, &shop], None).await;
    assert_eq!(
        look.refusal(&mail, Reach::See).as_deref(),
        Some("Firefox's window w1 shows a site Kai does not reach (Kai's site rules)"),
        "never the window's title"
    );
    assert!(look.refusal(&mail, Reach::Act).is_some());
    assert_eq!(look.refusal(&shop, Reach::See), None, "reading a read site");
    assert_eq!(
        look.refusal(&shop, Reach::Act).as_deref(),
        Some("Firefox's window w2 shows a site where Kai may only read (Kai's site rules)")
    );
    // A tab behind another does not decide, and an act site is reached.
    let news = ff("w3", "News — Mozilla Firefox", false);
    assert!(matches!(sight(&s, &news).await, Sight::Unknown(_)));
    let plain = shown(&fx.links, &["mail.example"], &[]);
    assert_eq!(sight(&plain, &shop).await, Sight::Site(Rule::Act));
    assert_eq!(
        plain
            .look(plain.sites(), &[&shop], None)
            .await
            .refusal(&shop, Reach::Act),
        None
    );
}

/// The window with the desktop's focus is also the window the extension says has the focus: a
/// title still changing is covered by it; where both fit different tabs, the stricter decides.
#[tokio::test]
async fn the_focused_window_and_every_fitting_title_count() {
    let fx = FakeFirefox::start(Some(PID), two_windows());
    let s = shown(&fx.links, &["mail.example"], &["shop.example"]);
    // Firefox's window with the focus, its title not yet the tab's: the extension's focused window.
    let loading = ff("w1", "Mozilla Firefox", true);
    assert_eq!(sight(&s, &loading).await, Sight::Site(Rule::None));
    // Not focused: Kai cannot tell.
    let loading = ff("w1", "Mozilla Firefox", false);
    assert!(matches!(sight(&s, &loading).await, Sight::Unknown(_)));
    // The shop's title with the desktop's focus while the extension says the mail has it: both.
    let shop = ff("w2", "Lamps — Mozilla Firefox", true);
    assert_eq!(sight(&s, &shop).await, Sight::Site(Rule::None));
    // Two windows whose tabs share a title: the stricter one.
    fx.set(listing(
        vec![
            front(3, 1, "Start", "https://mail.example/", false),
            front(5, 2, "Start", "https://news.example/", false),
        ],
        &[1, 2],
        None,
    ));
    let start = ff("w9", "Start — Mozilla Firefox", false);
    assert_eq!(sight(&s, &start).await, Sight::Site(Rule::None));
    // An exact prefix: "Start" is not "Star"'s.
    fx.set(listing(
        vec![front(5, 2, "Star", "https://mail.example/", false)],
        &[2],
        None,
    ));
    assert!(matches!(sight(&s, &start).await, Sight::Unknown(_)));
}

#[tokio::test]
async fn kai_cannot_tell_some_windows_and_takes_the_strictest_rule() {
    let fx = FakeFirefox::start(Some(PID), two_windows());
    let s = shown(&fx.links, &["mail.example"], &[]);
    let unknown = |w: &Window| {
        let s = s.clone();
        let w = w.clone();
        async move {
            match sight(&s, &w).await {
                Sight::Unknown(why) => why,
                other => panic!("{other:?}"),
            }
        }
    };
    // Firefox without a link (another pid), with the app id only.
    let other = window("w5", "firefox", "Inbox — Mozilla Firefox", 7, false);
    assert_eq!(
        unknown(&other).await,
        "no Firefox linked with Kai for Firefox owns it"
    );
    // A Private Browsing window in any language: its tabs are never listed, so its title fits
    // none, or one another window fits too.
    let private = ff("w6", "Navigation privée — Mozilla Firefox", false);
    assert!(unknown(&private).await.contains("fits none of the tabs"));
    let mail = ff("w1", "Inbox — Mozilla Firefox", false);
    let private = ff("w6", "Inbox — Mozilla Firefox — Navigation privée", false);
    let look = s.look(s.sites(), &[&mail, &private], None).await;
    assert!(matches!(look.sight(&private), Sight::Unknown(why) if why.contains("fits too")));
    let library = ff("w7", "Library", false);
    assert!(unknown(&library).await.contains("fits none of the tabs"));
    let look = s.look(s.sites(), &[&library], None).await;
    assert_eq!(
        look.rule(&library),
        Some(Rule::None),
        "the strictest there is"
    );
    assert_eq!(
        look.refusal(&library, Reach::See).as_deref(),
        Some(
            "Kai cannot tell which site Firefox's window w7 shows (its title fits none of the \
             tabs in front of that Firefox's windows: a Private Browsing window, one of \
             Firefox's own windows, a page without a title, or a title still changing), and \
             Kai's site rules keep some sites from Kai"
        )
    );
    // With only read sites, seeing it is fine and acting in it is not.
    let r = shown(&fx.links, &[], &["shop.example"]);
    let look = r.look(r.sites(), &[&library], None).await;
    assert_eq!(look.refusal(&library, Reach::See), None);
    assert!(
        look.refusal(&library, Reach::Act)
            .unwrap()
            .ends_with("Kai's site rules let Kai only read some sites")
    );
    // The tabs could not be listed.
    fx.set(json!({"error": {"code": "busy", "message": "not now"}}));
    let mail = ff("w1", "Inbox — Mozilla Firefox", false);
    assert!(
        unknown(&mail)
            .await
            .starts_with("its tabs could not be listed:")
    );
    // A window of another app is not Firefox's.
    let konsole = window("w8", "org.kde.konsole", "Inbox — Mozilla Firefox", 9, true);
    assert_eq!(sight(&s, &konsole).await, Sight::NotFirefox);
    assert_eq!(
        s.look(s.sites(), &[&konsole], None).await.rule(&konsole),
        None
    );
}

/// A look lists only the linked Firefoxes whose process owns one of the windows asked about.
#[tokio::test]
async fn a_look_lists_only_the_firefox_that_owns_a_window() {
    let fx = FakeFirefox::start(Some(PID), two_windows());
    let other = FakeFirefox::join(&fx.links, Some(77), two_windows());
    let s = shown(&fx.links, &["mail.example"], &[]);
    let konsole = window("w8", "org.kde.konsole", "build", 9, true);
    let unlinked = window("w5", "firefox", "Inbox — Mozilla Firefox", 7, false);
    s.look(s.sites(), &[&konsole, &unlinked], None).await;
    assert_eq!((fx.lists(), other.lists()), (0, 0));
    let mail = ff("w1", "Inbox — Mozilla Firefox", false);
    s.look(s.sites(), &[&konsole, &mail], None).await;
    assert_eq!((fx.lists(), other.lists()), (1, 0));
    // A link whose pid Kai could not read is never matched.
    let blind = FakeFirefox::join(&fx.links, None, two_windows());
    let zero = window("w0", "firefox", "Inbox — Mozilla Firefox", 0, false);
    assert!(matches!(
        sight(&s, &zero).await,
        Sight::Unknown(why) if why.starts_with("no Firefox linked")
    ));
    assert_eq!(blind.lists(), 0);
}

#[test]
fn the_fast_path_follows_the_strictest_rule() {
    let rules = |none: &[&str], read: &[&str]| {
        super::fake::policy(none, read)
            .read()
            .unwrap()
            .browser()
            .sites
            .clone()
    };
    let none = rules(&["mail.example"], &[]);
    let read = rules(&[], &["shop.example"]);
    let neither = rules(&[], &[]);
    assert!(applies(&none, Reach::See) && applies(&none, Reach::Act));
    assert!(!applies(&read, Reach::See) && applies(&read, Reach::Act));
    assert!(!applies(&neither, Reach::See) && !applies(&neither, Reach::Act));
}

/// The spoof of a window whose own tab does not fit its title: one look over every window of
/// that Firefox's process, and a tab in front whose title fits two of them decides for neither.
#[tokio::test]
async fn a_title_two_windows_fit_decides_for_neither() {
    // Window 1 went to the mail ("Inbox"), its title still the shop's it showed before; window 2
    // shows the shop.
    let fx = FakeFirefox::start(
        Some(PID),
        listing(
            vec![
                front(3, 1, "Inbox", "https://mail.example/inbox", false),
                front(5, 2, "Lamps", "https://shop.example/lamps", false),
            ],
            &[1, 2],
            None,
        ),
    );
    let s = shown(&fx.links, &["mail.example"], &[]);
    let lagging = ff("w1", "Lamps — Mozilla Firefox", false);
    let shop = ff("w2", "Lamps — Mozilla Firefox", false);
    // Alone it would pass for the shop's.
    assert_eq!(sight(&s, &lagging).await, Sight::Site(Rule::Act));
    let look = s.look(s.sites(), &[&lagging, &shop], None).await;
    for w in [&lagging, &shop] {
        assert!(
            matches!(look.sight(w), Sight::Unknown(ref why) if why.contains("fits too")),
            "{:?}",
            look.sight(w)
        );
        assert_eq!(look.rule(w), Some(Rule::None));
    }
    // Another app's window of that title, or another Firefox's, is no peer.
    let konsole = window("k", "org.kde.konsole", "Lamps — Mozilla Firefox", 9, false);
    let look = s.look(s.sites(), &[&shop, &konsole], None).await;
    assert_eq!(look.sight(&shop), Sight::Site(Rule::Act));
}

/// For a capture, a tab in front that came to the front after the settling point, or is loading,
/// is not settled: Kai cannot tell what the window's pixels show.
#[tokio::test]
async fn a_tab_not_settled_counts_as_untold_for_a_capture() {
    let mut news = front(7, 3, "News", "https://news.example/", false);
    news["activated_at"] = json!(10_000);
    let fx = FakeFirefox::start(Some(PID), listing(vec![news.clone()], &[3], None));
    let s = shown(&fx.links, &["mail.example"], &[]);
    let w = ff("w3", "News — Mozilla Firefox", false);
    let at = |after: Option<i64>| {
        let s = s.clone();
        let w = w.clone();
        async move { s.look(s.sites(), &[&w], after).await.sight(&w) }
    };
    assert_eq!(at(None).await, Sight::Site(Rule::Act), "not a capture");
    assert_eq!(at(Some(10_000)).await, Sight::Site(Rule::Act), "settled");
    assert!(matches!(at(Some(9_999)).await, Sight::Unknown(why) if why.contains("see_settle_ms")));
    news["activated_at"] = json!(null);
    news["loading"] = json!(true);
    fx.set(listing(vec![news], &[3], None));
    assert!(matches!(at(Some(10_000)).await, Sight::Unknown(why) if why.contains("loading")));
}

/// A tab in front without a site's host gets the strictest rule there is.
#[tokio::test]
async fn a_page_without_a_site_gets_the_strictest_rule() {
    for url in [
        "about:newtab",
        "about:blank",
        "data:text/html,hi",
        "about:logins",
    ] {
        let fx = FakeFirefox::start(
            Some(PID),
            listing(vec![front(7, 3, "Page", url, false)], &[3], None),
        );
        let w = ff("w3", "Page — Mozilla Firefox", false);
        let none = shown(&fx.links, &["mail.example"], &["shop.example"]);
        assert_eq!(sight(&none, &w).await, Sight::Site(Rule::None), "{url}");
        let read = shown(&fx.links, &[], &["shop.example"]);
        assert_eq!(sight(&read, &w).await, Sight::Site(Rule::Read), "{url}");
        let neither = shown(&fx.links, &[], &[]);
        assert_eq!(sight(&neither, &w).await, Sight::Site(Rule::Act), "{url}");
    }
}

/// The frame with the keyboard focus decides by its own site; whatever Kai cannot tell refuses.
#[tokio::test]
async fn the_focused_frame_decides_for_keys() {
    let fx = FakeFirefox::start(Some(PID), two_windows());
    let s = shown(&fx.links, &["mail.example"], &["shop.example"]);
    let w = ff("w2", "News — Mozilla Firefox", true);
    let field = json!({"ref": 4, "role": "textbox", "name": "Search"});
    let in_frame = |origin: serde_json::Value| json!({"ref": 4, "role": "textbox", "name": "Card", "frame": 17, "origin": origin});
    let news = "https://news.example/";
    for (ctx, want) in [
        (context(news, true, field.clone(), 0), None),
        (context(news, true, json!(null), 0), None),
        (
            context(news, true, in_frame(json!("https://news.example")), 0),
            None,
        ),
        (
            context(news, true, in_frame(json!("https://mail.example")), 0),
            Some(
                "the frame with the keyboard focus in Firefox's window w2 shows a site Kai does not reach (Kai's site rules)",
            ),
        ),
        (
            context(news, true, in_frame(json!("https://shop.example")), 0),
            Some(
                "the frame with the keyboard focus in Firefox's window w2 shows a site where Kai may only read (Kai's site rules)",
            ),
        ),
        (
            context("https://shop.example/", true, field.clone(), 0),
            Some(
                "the page with the keyboard focus in Firefox's window w2 shows a site where Kai may only read (Kai's site rules)",
            ),
        ),
    ] {
        fx.set_context(ctx.clone());
        assert_eq!(s.focus_refusal(&w).await.as_deref(), want, "{ctx}");
    }
    // What Kai cannot tell: an opaque frame, the window without the focus, an unread frame while
    // nothing has the focus, a page that could not be read, a failed call.
    let mut unreadable = context(news, true, json!(null), 0);
    unreadable["page_error"] = json!({"code": "content_timeout", "message": "slow"});
    let mut reader = context(news, true, json!(null), 0);
    reader["page_error"] = json!({"code": "reader_view", "message": "reader"});
    for (ctx, refused) in [
        (context(news, true, in_frame(json!(null)), 0), true),
        (context(news, false, field.clone(), 0), true),
        (context(news, true, json!(null), 1), true),
        (unreadable, true),
        (reader, false),
        (
            json!({"error": {"code": "busy", "message": "not now"}}),
            true,
        ),
    ] {
        fx.set_context(ctx.clone());
        let r = s.focus_refusal(&w).await;
        assert_eq!(r.is_some(), refused, "{ctx}: {r:?}");
        if let Some(r) = r {
            assert!(!r.contains("News"), "never the window's title: {r}");
            if !r.contains("shows a site") {
                assert!(
                    r.starts_with("Kai cannot tell which site the frame with the keyboard focus"),
                    "{r}"
                );
            }
        }
    }
    assert!(fx.contexts() >= 12);
    // A window of no linked Firefox is the window-level check's.
    let other = window("x", "firefox", "News — Mozilla Firefox", 7, true);
    assert_eq!(s.focus_refusal(&other).await, None);
}
