//! §4's folder rules and the conversation's event handling, without I/O.

use std::time::Duration;

use super::*;

fn folder(id: i64, name: &str) -> FolderInfo {
    FolderInfo {
        id,
        name: name.into(),
        ..FolderInfo::default()
    }
}

/// A folder that is ready: a model, ongoing with `idle` minutes.
fn ready(id: i64, name: &str, idle: i64) -> FolderInfo {
    FolderInfo {
        model: Some("kai-chat".into()),
        idle_minutes: Some(idle),
        ..folder(id, name)
    }
}

fn used(acts: &[Action]) -> Option<Option<Using>> {
    acts.iter().rev().find_map(|a| match a {
        Action::Use(u) => Some(*u),
        _ => None,
    })
}

fn notices(acts: &[Action]) -> Vec<&Notice> {
    acts.iter()
        .filter_map(|a| match a {
            Action::Notify(n) => Some(n),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------------ the lookup rule

#[test]
fn of_repeated_names_the_ongoing_folder_with_the_lowest_id_wins() {
    let folders = [
        folder(2, "Kai"),
        ready(9, "Kai", 120),
        ready(5, "Kai", 30),
        ready(1, "Work", 0),
    ];
    assert_eq!(
        pick(&folders, "Kai", None),
        Pick::Use {
            folder: ready(5, "Kai", 30),
            namesakes: vec![2, 9],
        },
        "a plain folder with a lower id does not count; the others are logged"
    );
}

#[test]
fn the_remembered_folder_wins_while_it_is_ready_whatever_its_name() {
    let folders = [ready(3, "Kai", 0), ready(7, "Assistant", 120)];
    assert_eq!(
        pick(&folders, "Kai", Some(7)),
        Pick::Use {
            folder: ready(7, "Assistant", 120),
            namesakes: vec![3],
        }
    );
}

#[test]
fn a_remembered_folder_that_is_not_ready_yields_to_an_ongoing_namesake() {
    let mut plain = folder(4, "Kai");
    plain.model = Some("kai-chat".into());
    let folders = [folder(1, "Kai"), plain.clone(), ready(8, "Kai", 120)];
    assert!(matches!(
        pick(&folders, "Kai", Some(4)),
        Pick::Use { folder, .. } if folder.id == 8
    ));
    // With no ongoing namesake it stays the remembered one, not the lower plain one.
    let folders = [folder(1, "Kai"), plain];
    assert!(matches!(
        pick(&folders, "Kai", Some(4)),
        Pick::Use { folder, .. } if folder.id == 4
    ));
}

#[test]
fn with_no_ongoing_folder_the_plain_one_with_the_lowest_id_and_with_none_a_new_one() {
    let folders = [folder(6, "Kai"), folder(2, " Kai "), folder(1, "kai")];
    assert!(matches!(
        pick(&folders, "Kai", None),
        Pick::Use { folder, namesakes } if folder.id == 2 && namesakes == [6]
    ));
    assert_eq!(
        pick(&[folder(1, "kai"), ready(2, "Work", 0)], "Kai", None),
        Pick::Create,
        "names match exactly (trimmed)"
    );
    assert_eq!(
        pick(&[], "Kai", Some(3)),
        Pick::Create,
        "a remembered id lmgw no longer has"
    );
}

#[test]
fn readiness_idle_and_a_years_retention() {
    assert_eq!(folder(1, "Kai").readiness(), Readiness::NeedsSetup);
    let mut f = folder(1, "Kai");
    f.model = Some("m".into());
    assert_eq!(f.readiness(), Readiness::NotOngoing);
    let f = ready(1, "Kai", 120);
    assert_eq!(f.readiness(), Readiness::Ready);
    assert_eq!(f.idle(), Some(Duration::from_secs(7200)));
    assert_eq!(ready(1, "Kai", 0).idle(), None, "0: only on request");
    let days = |archive, purge| FolderInfo {
        archive_days: archive,
        purge_days: purge,
        ..folder(1, "Kai")
    };
    assert!(!days(None, None).keeps_a_year(), "the global setting");
    assert!(days(Some(0), None).keeps_a_year(), "never archived");
    assert!(days(None, Some(0)).keeps_a_year(), "never deleted");
    assert!(days(None, Some(365)).keeps_a_year());
    assert!(days(Some(14), Some(351)).keeps_a_year());
    assert!(!days(Some(14), Some(30)).keeps_a_year());
    assert!(!days(None, Some(364)).keeps_a_year());
}

// ------------------------------------------------------------------------ the conversation

#[test]
fn a_first_run_creates_a_plain_folder_says_the_owners_steps_once_and_recovers() {
    let mut c = Conversation::new("Kai", None);
    assert_eq!(c.initial(), None);
    assert_eq!(c.connected(), [Action::Lookup]);
    assert_eq!(
        c.notes(),
        ["looking up the folder 'Kai' in lmgw"],
        "while it looks"
    );
    // No folder of the name: a plain one, with nothing but its name.
    assert_eq!(
        c.listed(Ok(vec![ready(1, "Work", 0)])),
        [Action::Create("Kai".into())]
    );
    let acts = c.created(Ok(folder(4, "Kai")));
    assert_eq!(
        used(&acts),
        Some(Some(Using {
            id: 4,
            idle: None,
            ready: false
        }))
    );
    let n = notices(&acts);
    assert_eq!(n.len(), 1);
    assert_eq!(n[0].summary, "Kai: set up the folder 'Kai' in lmgw");
    for words in [
        "set the folder 'Kai''s model and mark it as an ongoing conversation in lmgw",
        "Folder settings",
        "Model",
        "Ongoing conversation (a new thread after 120 idle minutes)",
        "Retention set Delete after (days archived) to 365",
    ] {
        assert!(n[0].body.contains(words), "{words}: {}", n[0].body);
    }
    assert!(!acts.contains(&Action::AskCurrent), "nothing to ask yet");
    assert!(!acts.iter().any(|a| matches!(a, Action::Remember(_))));
    let notes = c.notes();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].starts_with("voice waits for its folder: set the folder 'Kai''s model"));
    // Kai's own folder.created comes back: nothing new to say.
    assert!(notices(&c.changed(folder(4, "Kai"))).is_empty());
    // The owner sets the model and marks it ongoing on the dashboard: folder.updated.
    let mut set = ready(4, "Kai", 120);
    set.purge_days = Some(365);
    let acts = c.changed(set);
    assert_eq!(
        acts,
        [
            Action::Use(Some(Using {
                id: 4,
                idle: Some(Duration::from_secs(7200)),
                ready: true
            })),
            Action::Remember(Some(4)),
            Action::AskCurrent,
        ]
    );
    assert!(c.ready());
    assert!(c.notes().is_empty(), "{:?}", c.notes());
    // A later update of a ready folder (its idle minutes) only reaches the voice.
    let acts = c.changed(ready(4, "Kai", 30));
    assert_eq!(
        acts,
        [Action::Use(Some(Using {
            id: 4,
            idle: Some(Duration::from_secs(1800)),
            ready: true
        }))]
    );
    assert_eq!(
        c.notes(),
        [
            "the folder 'Kai' keeps its past threads by lmgw's global retention: for a year of \
          history set its Folder settings → Retention → Delete after (days archived) to 365 or \
          more in lmgw"
        ]
    );
}

#[test]
fn a_folder_with_a_model_that_is_not_ongoing_asks_only_to_mark_it() {
    let mut c = Conversation::new("Kai", None);
    c.connected();
    let mut f = folder(3, "Kai");
    f.model = Some("kai-chat".into());
    f.archive_days = Some(0);
    let acts = c.listed(Ok(vec![f]));
    let n = notices(&acts);
    assert_eq!(n.len(), 1);
    assert!(
        n[0].body
            .starts_with("mark the folder 'Kai' as an ongoing conversation in lmgw"),
        "{}",
        n[0].body
    );
    assert!(
        !n[0].body.contains("Retention"),
        "it keeps its history already"
    );
}

#[test]
fn every_connected_start_asks_current_and_a_resync_only_when_the_folder_moved() {
    let mut c = Conversation::new("Kai", Some(3));
    assert_eq!(
        c.initial(),
        Some(Using {
            id: 3,
            idle: None,
            ready: true
        }),
        "the remembered folder serves a press before the lookup answers"
    );
    c.connected();
    let acts = c.listed(Ok(vec![ready(3, "Kai", 120)]));
    assert!(acts.contains(&Action::AskCurrent));
    assert!(
        !acts.iter().any(|a| matches!(a, Action::Remember(_))),
        "remembered already"
    );
    c.connected();
    assert!(
        c.listed(Ok(vec![ready(3, "Kai", 120)]))
            .contains(&Action::AskCurrent)
    );
    assert_eq!(c.resync(), [Action::Lookup]);
    assert!(
        !c.listed(Ok(vec![ready(3, "Kai", 120)]))
            .contains(&Action::AskCurrent)
    );
    // An answer nobody waits for changes nothing.
    assert!(c.listed(Ok(vec![])).is_empty());
}

#[test]
fn a_remembered_folder_lmgw_no_longer_has_is_forgotten() {
    let mut c = Conversation::new("Kai", Some(12));
    c.connected();
    let acts = c.listed(Ok(vec![ready(3, "Kai", 0)]));
    assert_eq!(acts[0], Action::Remember(None));
    assert!(acts.contains(&Action::Remember(Some(3))));
}

#[test]
fn the_steps_are_notified_once_per_folder_and_state_and_again_when_the_owner_asks() {
    let mut c = Conversation::new("Kai", None);
    c.connected();
    assert_eq!(notices(&c.listed(Ok(vec![folder(4, "Kai")]))).len(), 1);
    // Each reconnect looks again: the same state says nothing new.
    c.connected();
    assert!(notices(&c.listed(Ok(vec![folder(4, "Kai")]))).is_empty());
    // A press: current is refused for it; the owner hears the steps again.
    let refused = Refusal {
        status: 409,
        code: Some("not_ongoing".into()),
        message: "folder 'Kai' is not an ongoing conversation".into(),
    };
    assert_eq!(notices(&c.refused(&refused, true)).len(), 1);
    assert!(notices(&c.refused(&refused, false)).is_empty());
    // A ready folder that loses its ongoing mark (its current went with reason not_ongoing).
    c.changed(ready(4, "Kai", 0));
    assert!(c.ready());
    let acts = c.refused(&refused, false);
    assert!(!c.ready());
    assert_eq!(notices(&acts).len(), 1, "a new state");
    assert_eq!(
        used(&acts),
        Some(Some(Using {
            id: 4,
            idle: None,
            ready: false
        }))
    );
}

#[test]
fn a_deleted_folder_is_forgotten_the_voice_leaves_and_kai_looks_again() {
    let mut c = Conversation::new("Kai", Some(3));
    c.connected();
    c.listed(Ok(vec![ready(3, "Kai", 0)]));
    assert!(c.gone(9).is_empty(), "another folder");
    let acts = c.gone(3);
    assert_eq!(
        acts,
        [
            Action::Remember(None),
            Action::Use(None),
            Action::Leave("the folder 'Kai' of Kai's conversation was deleted in lmgw".into()),
            Action::Lookup,
        ]
    );
    assert_eq!(c.listed(Ok(vec![])), [Action::Create("Kai".into())]);
    // A 404 on current: the list decides, after a wait (K2-11); gone from it, the same.
    let mut c = Conversation::new("Kai", Some(3));
    c.connected();
    c.listed(Ok(vec![ready(3, "Kai", 0)]));
    let acts = c.refused(&not_found(), true);
    assert_eq!(acts, [Action::LookupAfter(Duration::from_secs(1))]);
    let acts = c.listed(Ok(vec![]));
    assert_eq!(
        acts,
        [
            Action::Remember(None),
            Action::Use(None),
            Action::Leave("the folder 'Kai' of Kai's conversation was deleted in lmgw".into()),
            Action::Create("Kai".into()),
        ]
    );
}

fn not_found() -> Refusal {
    Refusal {
        status: 404,
        code: Some("not_found".into()),
        message: "folder not found".into(),
    }
}

/// Review KAI-2 K2-11: a 404 on `current` for a folder the list still shows stops the loop with a
/// note (no `current` again, no configuration written); each such round waits longer, 1 s doubling
/// up to `lmgw.retry_max_s`.
#[test]
fn a_404_for_a_folder_the_list_still_shows_stops_with_a_note() {
    let mut c = Conversation::new("Kai", Some(3)).retrying_up_to(Duration::from_secs(4));
    c.connected();
    c.listed(Ok(vec![ready(3, "Kai", 0)]));
    let mut waits = Vec::new();
    for _ in 0..4 {
        let acts = c.refused(&not_found(), false);
        match acts.as_slice() {
            [Action::LookupAfter(w)] => waits.push(w.as_secs()),
            other => panic!("{other:?}"),
        }
        let acts = c.listed(Ok(vec![ready(3, "Kai", 0)]));
        assert!(acts.is_empty(), "no current, no write: {acts:?}");
        assert!(c.ready(), "the folder stays");
        assert!(
            c.notes()[0].contains("404") && c.notes()[0].contains("still shows folder 3"),
            "{:?}",
            c.notes()
        );
    }
    assert_eq!(waits, [1, 2, 4, 4]);
    // A reconnect starts the backoff over.
    c.connected();
    c.listed(Ok(vec![ready(3, "Kai", 0)]));
    assert_eq!(
        c.refused(&not_found(), false),
        [Action::LookupAfter(Duration::from_secs(1))]
    );
}

/// Review KAI-2 K2-22: a reconnect while Kai creates its folder does not replace the create; after
/// it Kai lists again and takes the namesake with the lowest id (another device's create raced).
#[test]
fn a_create_settles_before_a_new_lookup_and_the_lowest_namesake_wins() {
    let mut c = Conversation::new("Kai", None);
    c.connected();
    assert_eq!(c.listed(Ok(vec![])), [Action::Create("Kai".into())]);
    assert!(c.connected().is_empty(), "the create is not replaced");
    assert!(c.resync().is_empty());
    let acts = c.created(Ok(folder(5, "Kai")));
    assert_eq!(acts.last(), Some(&Action::Lookup), "{acts:?}");
    assert_eq!(c.folder().map(|f| f.id), Some(5));
    let acts = c.listed(Ok(vec![folder(4, "Kai"), folder(5, "Kai")]));
    assert_eq!(
        used(&acts),
        Some(Some(Using {
            id: 4,
            idle: None,
            ready: false
        }))
    );
    assert_eq!(c.folder().map(|f| f.id), Some(4));
}

#[test]
fn another_folder_of_the_name_is_looked_at_only_while_kais_is_not_ready() {
    let mut c = Conversation::new("Kai", None);
    c.connected();
    c.listed(Ok(vec![folder(4, "Kai")]));
    assert_eq!(c.changed(ready(8, "Kai", 120)), [Action::Lookup]);
    // The lookup lands on the ongoing one and remembers it.
    let acts = c.listed(Ok(vec![folder(4, "Kai"), ready(8, "Kai", 120)]));
    assert!(acts.contains(&Action::Remember(Some(8))));
    assert!(acts.contains(&Action::AskCurrent));
    assert!(c.changed(ready(9, "Kai", 0)).is_empty(), "Kai's is ready");
    assert!(c.changed(folder(10, "Work")).is_empty());
}

#[test]
fn a_failed_lookup_is_said_and_the_folders_own_event_clears_it() {
    let mut c = Conversation::new("Kai", Some(3));
    c.connected();
    c.listed(Ok(vec![ready(3, "Kai", 0)]));
    c.connected();
    assert!(
        c.listed(Err(Refusal::no_answer(
            "lmgw is not reachable: Connection refused"
        )))
        .is_empty()
    );
    assert_eq!(
        c.notes()[0],
        "Kai could not look up its folder 'Kai' in lmgw (lmgw is not reachable: Connection \
         refused); it tries again when the feed reconnects"
    );
    c.changed(ready(3, "Kai", 0));
    assert!(
        !c.notes().iter().any(|n| n.contains("could not look up")),
        "{:?}",
        c.notes()
    );
}

#[test]
fn a_reset_forgets_the_gateways_folder() {
    let mut c = Conversation::new("Kai", Some(3));
    c.connected();
    c.listed(Ok(vec![ready(3, "Kai", 0)]));
    c.reset();
    assert_eq!(c.initial(), None);
    assert!(c.folder().is_none());
    c.connected();
    assert_eq!(
        c.listed(Ok(vec![ready(3, "Kai", 0)]))
            .iter()
            .filter(|a| matches!(a, Action::Remember(Some(3))))
            .count(),
        1,
        "found and remembered anew"
    );
}
