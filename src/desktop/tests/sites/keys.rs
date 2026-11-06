//! `type_text`, `key_press` and `ui_press` under Kai's site rules (`crate::desktop::sites::keys`):
//! Firefox's own tab, address and history chords, the desktop's window shortcuts, a window whose
//! title changes while the keys go, the frame with the keyboard focus, the check once the focus is
//! back after the dialog, the check after the dialog for `key_press` and `ui_press`, and refusals
//! that never quote a Firefox window's title. Every refusal asserts that nothing was typed or
//! pressed.

use std::time::Duration;

use serde_json::json;

use super::*;

/// A key press refused before anything was sent.
async fn refused(s: &Services, d: &Desk, keys: &str) -> String {
    let e = act(s, "key_press", json!({ "keys": keys }))
        .await
        .unwrap_err();
    assert_eq!(d.input.units(), 0, "{keys}: nothing pressed");
    e
}

#[tokio::test]
async fn key_press_refuses_firefoxs_tab_address_and_history_chords() {
    let d = Desk::new("ff3");
    let s = d.services(&[MAIL], &[SHOP]);
    for (keys, n, says) in [
        ("ctrl+tab space", 1, "tab_switch"),
        ("alt+3", 1, "tab_switch"),
        ("ctrl+pagedown enter", 1, "tab_switch"),
        ("ctrl+l", 1, "tab_go"),
        ("f6", 1, "tab_go"),
        ("a ctrl+t", 2, "tab_open"),
        ("alt+left", 1, "tab_back"),
        ("ctrl+h", 1, "other sites' titles"),
    ] {
        let e = refused(&s, &d, keys).await;
        assert!(e.starts_with(&format!("chord {n} ")), "{keys}: {e}");
        assert!(e.contains(says), "{keys}: {e}");
        assert!(e.ends_with("; key_press did not run"), "{keys}: {e}");
    }
    assert!(d.input.sent_nothing());
    // Only read sites: still refused in Firefox.
    let s = d.services(&[], &[SHOP]);
    refused(&s, &d, "ctrl+tab").await;
    // Another app's window, and no rule at all: pressed.
    let k = Desk::new("kon");
    let s = k.services(&[MAIL], &[SHOP]);
    assert_eq!(
        act(&s, "key_press", json!({"keys": "ctrl+tab ctrl+l"})).await,
        Ok("pressed 2 chords".into())
    );
    let f = Desk::new("ff3");
    let s = f.services(&[], &[]);
    assert_eq!(
        act(&s, "key_press", json!({"keys": "ctrl+tab"})).await,
        Ok("pressed 1 chord".into())
    );
}

#[tokio::test]
async fn key_press_refuses_the_desktops_window_shortcuts_while_a_site_is_none() {
    let d = Desk::new("kon");
    let s = d.services(&[MAIL], &[]);
    for keys in ["super+w", "ctrl+super+right", "alt+tab", "enter ctrl+f9"] {
        let e = refused(&s, &d, keys).await;
        assert!(e.contains("the desktop's own shortcuts"), "{keys}: {e}");
        assert!(e.contains("the window tools"), "{keys}: {e}");
    }
    assert!(d.input.sent_nothing());
    // Only read sites: see_screen blanks nothing, so the shortcut goes.
    let s = d.services(&[], &[SHOP]);
    assert!(
        act(&s, "key_press", json!({"keys": "super+w"}))
            .await
            .is_ok()
    );
}

/// The window's title changes after the first chord (the tab in front went to the mail): the
/// window is listed and checked again, and the call stops there.
#[tokio::test]
async fn a_title_that_changes_while_the_keys_go_is_checked_again() {
    for (tool, v, unit) in [
        ("key_press", json!({"keys": "a b c"}), "chords pressed"),
        ("type_text", json!({"text": "abc"}), "characters typed"),
    ] {
        let d = Desk::new("ff3");
        let s = d.services(&[MAIL], &[SHOP]);
        let (fx, windows) = (d.fx.clone(), d.windows.clone());
        d.input.after(move |done| {
            if done == 1 {
                fx.set(listing(
                    vec![front(7, 3, "Mail", "https://mail.example/", false)],
                    &[3],
                    None,
                ));
                windows.change(|s| {
                    s.windows.iter_mut().find(|w| w.id == "ff3").unwrap().title =
                        "Mail — Mozilla Firefox".into();
                });
            }
        });
        let lists = d.fx.lists();
        let e = act(&s, tool, v).await.unwrap_err();
        assert_eq!(
            e,
            format!(
                "Firefox's window ff3 shows a site Kai does not reach (Kai's site rules); 1 of 3 \
                 {unit}"
            )
        );
        assert_eq!(d.input.units(), 1, "{tool}");
        assert!(d.fx.lists() > lists, "listed again");
    }
}

/// The frame with the keyboard focus decides by its own site: before every chord, and before the
/// first character and each one after a Tab or a line break.
#[tokio::test]
async fn the_frame_with_the_focus_is_checked_before_the_keys() {
    let in_mail = json!({"ref": 2, "role": "textbox", "name": "To", "frame": 9,
                         "origin": "https://mail.example"});
    let d = Desk::new("ff3");
    let s = d.services(&[MAIL], &[SHOP]);
    d.fx.set_context(context("https://news.example/", true, in_mail.clone(), 0));
    let e = act(&s, "type_text", json!({"text": "hi"}))
        .await
        .unwrap_err();
    assert_eq!(
        e,
        "the frame with the keyboard focus in Firefox's window ff3 shows a site Kai does not \
         reach (Kai's site rules); 0 of 2 characters typed"
    );
    assert_eq!(d.input.units(), 0);
    // A Tab moves the focus into the mail's frame: the chord after it is refused.
    let d = Desk::new("ff3");
    let s = d.services(&[MAIL], &[SHOP]);
    let fx = d.fx.clone();
    let moved = in_mail.clone();
    d.input.after(move |done| {
        if done == 1 {
            fx.set_context(context("https://news.example/", true, moved.clone(), 0));
        }
    });
    let e = act(&s, "key_press", json!({"keys": "tab space"}))
        .await
        .unwrap_err();
    assert!(e.ends_with("; 1 of 2 chords pressed"), "{e}");
    assert_eq!(d.input.units(), 1);
    // Typing: a letter does not move the focus, so the next character is not asked about; the
    // character after a Tab is.
    let d = Desk::new("ff3");
    let s = d.services(&[MAIL], &[SHOP]);
    let fx = d.fx.clone();
    d.input.after(move |done| {
        if done == 1 {
            fx.set_context(context("https://news.example/", true, in_mail.clone(), 0));
        }
    });
    let e = act(&s, "type_text", json!({"text": "a\tb"}))
        .await
        .unwrap_err();
    assert!(e.ends_with("; 2 of 3 characters typed"), "{e}");
    assert_eq!(d.input.units(), 2);
}

/// The dialog had the focus; once it is back the window is checked again: the tab in front went
/// to a read site meanwhile, after the check when the call began.
#[tokio::test]
async fn the_window_is_checked_once_the_focus_is_back_after_the_dialog() {
    let d = Desk::new("ff3");
    let mut s = d.services(&[MAIL], &[SHOP]);
    s.focus_wait = Duration::from_secs(5);
    let (fx, windows) = (d.fx.clone(), d.windows.clone());
    let focus = |windows: &FakeWindows, id: &str| {
        windows.change(|s| {
            for w in &mut s.windows {
                w.active = w.id == id;
            }
        })
    };
    let r = act_after(&s, "type_text", json!({"text": "hi"}), {
        let windows = windows.clone();
        move || focus(&windows, "kon")
    });
    let back = async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        fx.set(listing(
            vec![front(7, 3, "News", "https://shop.example/news", false)],
            &[3],
            None,
        ));
        focus(&windows, "ff3");
    };
    let (r, ()) = tokio::join!(r, back);
    assert_eq!(
        r,
        Err(
            "Firefox's window ff3 shows a site where Kai may only read (Kai's site rules); 0 of 2 \
             characters typed"
                .into()
        )
    );
    assert_eq!(d.input.units(), 0);
}

/// `key_press` and `ui_press` are checked again when they run after the dialog.
#[tokio::test]
async fn key_press_and_ui_press_are_checked_again_after_the_dialog() {
    for (tool, v) in [
        ("key_press", json!({"keys": "enter"})),
        ("ui_press", json!({"element": "Send"})),
    ] {
        let d = Desk::new("ff3");
        let s = d.services(&[MAIL], &[SHOP]);
        let fx = d.fx.clone();
        let r = act_after(&s, tool, v, move || {
            fx.set(listing(
                vec![front(7, 3, "News", "https://mail.example/", false)],
                &[3],
                None,
            ))
        })
        .await;
        assert_eq!(
            r,
            Err(format!(
                "Firefox's window ff3 shows a site Kai does not reach (Kai's site rules); {tool} \
                 did not run"
            ))
        );
        assert_eq!(d.input.units(), 0);
        assert!(
            d.access.presses.lock().unwrap().is_empty(),
            "nothing pressed"
        );
    }
}

/// A refusal of the input tools names a Firefox window by its id while a rule could refuse,
/// never by its title (the title may be a none site's page's).
#[tokio::test]
async fn a_refusal_never_quotes_a_firefox_windows_title() {
    let d = Desk::new("ff1");
    let s = d.services(&[MAIL], &[]);
    let e = act(&s, "type_text", json!({"text": "hi", "window": "kon"}))
        .await
        .unwrap_err();
    assert_eq!(
        e,
        "kon org.kde.konsole \"build — Konsole\" does not have the keyboard focus (the keys would \
         go to Firefox's window ff1); activate it first (window_activate)"
    );
    assert!(d.input.sent_nothing());
    // No rule: the titles as before.
    let s = d.services(&[], &[]);
    let e = act(&s, "type_text", json!({"text": "hi", "window": "kon"}))
        .await
        .unwrap_err();
    assert!(e.contains("\"Inbox — Mozilla Firefox\""), "{e}");
}
