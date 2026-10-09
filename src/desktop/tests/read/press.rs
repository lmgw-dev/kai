//! `ui_press` (WP16c) against the fake accessibility bus of the parent module: which element a
//! query picks, which action runs, what is refused, a press whose handler never returns, and a
//! found element that is gone or changed before its press.

use super::*;

const ENABLED: u32 = 8;

fn button(name: &'static str, actions: &'static [&'static str]) -> Spec {
    Spec {
        role: 43,
        name,
        states: &[VISIBLE, ENABLED],
        actions,
        ..Default::default()
    }
}

fn press_tree(done: &Done) -> Spec {
    let with = |mut s: Spec| {
        s.done = Some(done.clone());
        s
    };
    let frame = Spec {
        role: 23,
        name: "Kai Test",
        states: SHOWN,
        children: vec![
            with(button("Save", &["click"])),
            with(button("Save as…", &["menu", "Press"])),
            with(Spec {
                role: 35,
                name: "Save",
                states: &[VISIBLE, ENABLED],
                actions: &["click"],
                ..Default::default()
            }),
            with(Spec {
                role: 7,
                name: "Autosave",
                states: &[VISIBLE, ENABLED],
                actions: &["toggle"],
                ..Default::default()
            }),
            // Not enabled: a control in the "disabled" list of roles.
            with(Spec {
                states: &[VISIBLE],
                ..button("Delete", &["click"])
            }),
            // Not shown.
            with(Spec {
                states: &[ENABLED],
                ..button("Hidden", &["click"])
            }),
            // Nothing to name it by.
            with(button("", &["click"])),
            // A label has no action.
            Spec {
                role: 29,
                name: "Docs label",
                states: SHOWN,
                ..Default::default()
            },
            with(Spec {
                role: 88,
                name: "Docs",
                states: &[VISIBLE, ENABLED],
                actions: &["jump"],
                ..Default::default()
            }),
            with(button("Odd", &["frobnicate", "other"])),
            with(Spec {
                hangs: Some(Duration::from_secs(5)),
                ..button("Wait", &["click"])
            }),
            // Manages its descendants: not looked into.
            Spec {
                role: 54,
                name: "Big table",
                states: &[VISIBLE, ENABLED, 31],
                child_count: Some(5000),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    Spec {
        role: 75,
        name: "kai-test",
        states: SHOWN,
        children: vec![frame],
        ..Default::default()
    }
}

async fn find(a: &Atspi, query: &str) -> Result<Element, Failure> {
    a.find(
        &Target {
            pid: std::process::id(),
            title: "Kai Test".into(),
        },
        &Query::parse(query).unwrap(),
    )
    .await
}

/// Found, then pressed (the confirmation in between is the host's).
async fn press(a: &Atspi, query: &str) -> Result<Pressed, Failure> {
    let found = find(a, query).await?;
    a.press(&found).await
}

#[tokio::test]
async fn ui_press_picks_one_element_and_runs_its_preferred_action() {
    let done: Done = Done::default();
    let Some(b) = a11y_with(true, press_tree(&done), None).await else {
        return;
    };
    let a = Atspi {
        bus: SessionBus::connected(b.session.connect().await, T),
        timeout: Duration::from_millis(400),
        concurrency: 16,
        switch: None,
    };
    let took = |done: &Done| std::mem::take(&mut *done.lock().unwrap());

    let p = press(&a, "button \"Save\"").await.unwrap();
    assert_eq!(
        (p.role.as_str(), p.name.as_str(), p.action.as_str()),
        ("button", "Save", "click")
    );
    assert_eq!(took(&done), [("Save", 0)]);

    // The name alone is ambiguous between the button and the menu item: both are listed.
    let e = press(&a, "save").await.unwrap_err();
    assert_eq!(
        e.to_string(),
        "2 elements match; name one with its role, as in button \"Name\":\nbutton \"Save\"\nmenu \
         item \"Save\""
    );
    assert!(took(&done).is_empty(), "nothing was pressed");

    // A fragment; the action `Press` is preferred to `menu`, whatever the case.
    let p = press(&a, "as…").await.unwrap();
    assert_eq!((p.name.as_str(), p.action.as_str()), ("Save as…", "Press"));
    assert_eq!(took(&done), [("Save as…", 1)]);

    let p = press(&a, "Autosave").await.unwrap();
    assert_eq!(
        (p.role.as_str(), p.action.as_str()),
        ("check box", "toggle")
    );
    let p = press(&a, "Docs").await.unwrap();
    assert_eq!((p.role.as_str(), p.action.as_str()), ("link", "jump"));
    // No preferred action: the first one.
    let p = press(&a, "Odd").await.unwrap();
    assert_eq!(p.action, "frobnicate");
    took(&done);

    // A disabled control is not pressed.
    let e = press(&a, "Delete").await.unwrap_err();
    assert_eq!(e.to_string(), "button \"Delete\" is disabled");
    assert!(took(&done).is_empty());

    // Hidden, unnamed and action-less elements are not candidates; the miss says what was not seen.
    for q in ["Hidden", "Docs label", "nothing at all"] {
        let e = press(&a, q).await.unwrap_err();
        assert_eq!(
            e.to_string(),
            "no pressable element of the window matches (read_window shows its controls); \
             elements that manage their own descendants were not looked into: 1",
            "{q}"
        );
    }
    assert!(took(&done).is_empty());
}

/// A handler that opens a dialog and waits in it never answers: the press was sent, and the result
/// does not turn into an error.
#[tokio::test]
async fn a_press_whose_handler_never_returns_counts_as_made() {
    let done: Done = Done::default();
    let Some(b) = a11y_with(true, press_tree(&done), None).await else {
        return;
    };
    let a = Atspi {
        bus: SessionBus::connected(b.session.connect().await, T),
        timeout: Duration::from_millis(400),
        concurrency: 16,
        switch: None,
    };
    let started = std::time::Instant::now();
    let p = press(&a, "Wait").await.unwrap();
    assert_eq!(p.action, "click");
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "bounded by dbus.timeout_s"
    );
    assert_eq!(done.lock().unwrap().clone(), [("Wait", 0)]);
}

/// The press is of the element that was found and confirmed: one whose object is gone, or whose
/// path now answers with another role or name, is refused and nothing is pressed.
#[tokio::test]
async fn a_found_element_that_is_gone_or_changed_is_not_pressed() {
    let done: Done = Done::default();
    let Some(b) = a11y_with(true, press_tree(&done), None).await else {
        return;
    };
    let a = Atspi {
        bus: SessionBus::connected(b.session.connect().await, T),
        timeout: Duration::from_millis(400),
        concurrency: 16,
        switch: None,
    };
    let found = find(&a, "button \"Save\"").await.unwrap();
    assert_eq!(
        (found.role.as_str(), found.name.as_str()),
        ("button", "Save")
    );
    assert!(done.lock().unwrap().is_empty(), "finding presses nothing");
    let gone = "button \"Save\" is no longer in the window (it changed after the confirmation); \
                nothing was pressed";
    let elsewhere = Element {
        at: (found.at.0.clone(), format!("{}/gone", found.at.1)),
        ..found.clone()
    };
    assert_eq!(a.press(&elsewhere).await.unwrap_err().to_string(), gone);
    let renamed = Element {
        name: "Save as…".into(),
        ..found.clone()
    };
    assert!(
        a.press(&renamed)
            .await
            .unwrap_err()
            .to_string()
            .starts_with("button \"Save as…\" is no longer in the window")
    );
    let other_role = Element {
        role: "menu item".into(),
        ..found.clone()
    };
    assert!(a.press(&other_role).await.is_err());
    assert!(done.lock().unwrap().is_empty(), "nothing was pressed");
    // The same element still there is pressed.
    a.press(&found).await.unwrap();
    assert_eq!(done.lock().unwrap().clone(), [("Save", 0)]);
}
