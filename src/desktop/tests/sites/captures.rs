//! The captures' own checks under Kai's site rules: a Firefox window not drawn now (minimized, on
//! another desktop), before and after `see_window`'s capture; a tab in front not settled; every
//! window of Firefox's process in one look (a title two windows fit); a page without a site; and
//! `see_screen` while the compositor runs an effect that moves windows. Every refusal asserts
//! that nothing was captured, or that no image was shown.

use serde_json::json;

use super::*;
use crate::browser::shown::now_ms;

fn set(d: &Desk, f: impl FnOnce(&mut Window)) {
    d.windows
        .change(|s| f(s.windows.iter_mut().find(|w| w.id == "ff3").unwrap()));
}

/// While some site is none, a Firefox window that is minimized or only on another desktop is not
/// captured: Firefox does not draw it anew, so its capture is whatever it showed last.
#[tokio::test]
async fn see_window_refuses_a_firefox_window_not_drawn() {
    for change in [
        (|w: &mut Window| w.minimized = true) as fn(&mut Window),
        |w: &mut Window| w.desktops = vec!["Mail".into()],
    ] {
        let d = Desk::new("kon");
        set(&d, change);
        let s = d.services(&[MAIL], &[]);
        let r = read(&s, "see_window", json!({"window": "ff3"})).await;
        assert!(r.error);
        assert_eq!(
            first(&r),
            "Firefox's window ff3 is minimized or only on another virtual desktop, where Firefox \
             does not draw it anew, so a capture of it shows whatever it showed last, and Kai's \
             site rules keep some sites from Kai; see_window did not run"
        );
        assert!(d.shots.asked().is_empty(), "nothing captured");
        // No none site: captured as before.
        let s = d.services(&[], &[SHOP]);
        assert!(!read(&s, "see_window", json!({"window": "ff3"})).await.error);
    }
    // Another app's minimized window is its own business.
    let d = Desk::new("ff3");
    d.windows.change(|s| {
        s.windows
            .iter_mut()
            .find(|w| w.id == "kon")
            .unwrap()
            .minimized = true;
    });
    let s = d.services(&[MAIL], &[]);
    assert!(!read(&s, "see_window", json!({"window": "kon"})).await.error);
}

/// A window minimized while it was captured: the capture is not shown.
#[tokio::test]
async fn a_firefox_window_minimized_during_the_capture_withholds_it() {
    let d = Desk::new("kon");
    let windows = d.windows.clone();
    d.shots.after(move || {
        windows.change(|s| {
            s.windows
                .iter_mut()
                .find(|w| w.id == "ff3")
                .unwrap()
                .minimized = true;
        })
    });
    let s = d.services(&[MAIL], &[]);
    let r = read(&s, "see_window", json!({"window": "ff3"})).await;
    assert!(r.error);
    assert!(
        first(&r).ends_with("by the time see_window had it, so Kai does not say what it got"),
        "{}",
        first(&r)
    );
    assert_eq!(r.blocks.len(), 1, "no image");
}

/// A tab that came to the front within browser.see_settle_ms, or is still loading, counts as a
/// page Kai cannot tell for a capture: the window may still show the page before.
#[tokio::test]
async fn a_tab_not_settled_is_not_captured() {
    let news = |at: Option<i64>, loading: bool| {
        let mut t = front(7, 3, "News", "https://news.example/", false);
        t["activated_at"] = json!(at);
        t["loading"] = json!(loading);
        listing(vec![t], &[3], None)
    };
    for (tabs, refused) in [
        (news(Some(now_ms()), false), true),
        (news(None, true), true),
        (news(Some(now_ms() - 60_000), false), false),
        (news(None, false), false),
    ] {
        let d = Desk::new("kon");
        d.fx.set(tabs.clone());
        let s = d.services(&[MAIL], &[]);
        let r = read(&s, "see_window", json!({"window": "ff3"})).await;
        assert_eq!(r.error, refused, "{tabs}: {}", first(&r));
        if refused {
            assert!(first(&r).contains("browser.see_settle_ms"), "{}", first(&r));
            assert!(d.shots.asked().is_empty(), "nothing captured");
        }
    }
    // A tab switched in while the capture ran.
    let d = Desk::new("kon");
    let fx = d.fx.clone();
    d.shots.after(move || fx.set(news(Some(now_ms()), false)));
    let s = d.services(&[MAIL], &[]);
    let r = read(&s, "see_window", json!({"window": "ff3"})).await;
    assert!(r.error && r.blocks.len() == 1, "{}", first(&r));
    // see_screen blanks such a window.
    let mut d = Desk::new("kon");
    d.fx.set(news(None, true));
    d.windows.placed = Some(Placement {
        windows: vec![placed(
            win(
                "ff3",
                "org.mozilla.firefox",
                "News — Mozilla Firefox",
                PID,
                false,
            ),
            Some((0, 0, 200, 100)),
        )],
        desktop: "Main".into(),
    });
    let s = d.services(&[MAIL], &[]);
    let r = read(&s, "see_screen", json!({})).await;
    assert_eq!(&pixels_of(&r)[..1], ["##.................."]);
}

/// One look takes every window of Firefox's process: window 1 went to the mail while its title
/// still says the news, which window 3 shows; neither title tells which window shows the news.
#[tokio::test]
async fn a_title_two_firefox_windows_fit_is_not_captured() {
    let d = Desk::new("kon");
    d.windows.change(|s| {
        s.windows.iter_mut().find(|w| w.id == "ff1").unwrap().title =
            "News — Mozilla Firefox".into();
    });
    let s = d.services(&[MAIL], &[]);
    for id in ["ff1", "ff3"] {
        let r = read(&s, "see_window", json!({ "window": id })).await;
        assert!(r.error, "{id}");
        assert!(
            first(&r).contains("another of that Firefox's windows fits too"),
            "{}",
            first(&r)
        );
    }
    assert!(d.shots.asked().is_empty(), "nothing captured");
}

/// A tab in front without a site (a new tab, Firefox View) gets the strictest rule.
#[tokio::test]
async fn a_page_without_a_site_is_not_captured_while_a_site_is_none() {
    let d = Desk::new("kon");
    d.fx.set(listing(
        vec![front(7, 3, "News", "about:firefoxview", false)],
        &[3],
        None,
    ));
    let s = d.services(&[MAIL], &[]);
    let r = read(&s, "see_window", json!({"window": "ff3"})).await;
    assert_eq!(
        first(&r),
        "Firefox's window ff3 shows a site Kai does not reach (Kai's site rules); see_window did \
         not run"
    );
    assert!(d.shots.asked().is_empty());
}

/// While the compositor runs an effect that may draw windows away from their frames (the
/// Overview, a desktop slide, a minimize animation), see_screen does not show the screen: before
/// the capture, nothing is captured; after it, no image.
#[tokio::test]
async fn see_screen_waits_for_the_desktop_to_be_still() {
    let placement = || Placement {
        windows: vec![placed(
            win(
                "ff1",
                "org.mozilla.firefox",
                "Inbox — Mozilla Firefox",
                PID,
                false,
            ),
            Some((0, 0, 200, 100)),
        )],
        desktop: "Main".into(),
    };
    let mut d = Desk::new("kon");
    d.windows.placed = Some(placement());
    *d.windows.effects.lock().unwrap() = vec!["blur".into(), "overview".into(), "squash".into()];
    let s = d.services(&[MAIL], &[]);
    let r = read(&s, "see_screen", json!({})).await;
    assert!(r.error);
    assert_eq!(
        first(&r),
        "the desktop runs the effects overview, squash, which may draw windows away from where \
         Kai blanks them, and Kai's site rules keep some sites from Kai; see_screen did not run \
         (it can once the desktop is still)"
    );
    assert!(d.shots.asked().is_empty(), "nothing captured");
    // Effects that leave windows where they are: captured and blanked.
    *d.windows.effects.lock().unwrap() = vec!["blur".into(), "screenshot".into()];
    let r = read(&s, "see_screen", json!({})).await;
    assert!(!r.error, "{}", first(&r));
    // One that starts while the capture runs: no image.
    let effects = d.windows.effects.clone();
    d.shots
        .after(move || effects.lock().unwrap().push("slide".into()));
    let r = read(&s, "see_screen", json!({})).await;
    assert!(r.error && r.blocks.len() == 1, "{}", first(&r));
    assert!(first(&r).contains("effect slide"), "{}", first(&r));
    // Without a none site nothing is asked of the effects.
    let s = d.services(&[], &[SHOP]);
    assert!(!read(&s, "see_screen", json!({})).await.error);
}
