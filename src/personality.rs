//! The personality (ruling 29, K31, WP22): lmgw's personality profiles as Kai's tray picker and the
//! tool `personality_set`. The profiles, and which one a thread talks with, are lmgw's; Kai keeps
//! the list (read at `hello`, after a `resync`, and by the feed's `profile.*` events), the profile
//! each thread of the feed talks with, and draws the tray's Personality submenu from them. This
//! module is the pure part; the core's side (`crate::core::personality`) asks lmgw and applies.
//!
//! **Setting** (lmgw's personality-profiles design §3.5): the name is matched case-insensitively
//! against `GET /chat/api/profiles` (`default` means null), then written to Kai's conversation
//! folder with `POST /chat/api/folders/{id}` and `{defaults_patch: {profile_id}}`. lmgw copies it
//! to the folder's current thread, so the rolled-over thread keeps it. No route of Kai's own.
//!
//! **What "default" is.** A folder's `profile_id: null` means "use the Chat setting `chat_profile`
//! for new threads" (the list's `default_profile_id`), not "no profile". Kai cannot change that
//! setting (it is the owner's, a device key does not reach it), so `default` is the gateway's own:
//! the tool's answer says what it came to (the gateway's default profile by name, or no profile).
//!
//! **What the tray checks** is what the conversation's thread talks with: the thread's own
//! `profile_id` when the feed has told it (a thread the dashboard moved to another profile shows
//! so), else the folder's choice. The Default item names the Chat setting when it names one
//! ("Default (global: Concise)"), unless the thread is known to talk with none.

use std::collections::HashMap;

use lmgw_client::requests::{FolderPatched, ProfileList, profile_named};

use crate::lmgw_link::ProfileEvent;
use crate::tray::model::{PersonalityModel, PersonalityOption};

/// The label of the item that is no particular profile.
pub const DEFAULT_LABEL: &str = "Default";

/// What a set names: a profile by the words of a call, or by the tray's item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Name(String),
    /// The tray's pick: a profile id, `None` for Default.
    Id(Option<i64>),
}

/// The profile `target` is in `list`: `Ok(None)` is `default`. The error is for the caller: an
/// unknown name lists the names in one line (lmgw-client's `unknown_profile`).
pub fn resolve(list: &ProfileList, target: &Target) -> Result<Option<i64>, String> {
    match target {
        Target::Name(name) => profile_named(list, name).map_err(|e| e.message),
        Target::Id(None) => Ok(None),
        Target::Id(Some(id)) => {
            if list.profiles.iter().any(|p| p.id == *id) {
                Ok(Some(*id))
            } else {
                Err("that profile is gone from lmgw".to_owned())
            }
        }
    }
}

/// The tool's answer: what was set, what `default` came to, and whether the thread took it now.
pub fn answer(list: &ProfileList, chosen: Option<i64>, patched: &FolderPatched) -> String {
    let name_of = |id: i64| {
        list.profiles
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.as_str())
    };
    let reached = patched
        .applied
        .as_ref()
        .is_some_and(|a| a.fields.iter().any(|f| f == "profile_id"));
    let global = list.default_profile_id.and_then(name_of);
    let said = match (chosen.and_then(name_of), global) {
        (Some(name), _) => format!("Personality set to {name}."),
        (None, Some(global)) => format!(
            "Personality set to default. Default is lmgw's own choice for new conversations, \
             which is the profile {global}, so the conversation talks as {global}."
        ),
        (None, None) => "Personality set to default: lmgw names no profile for new \
                         conversations, so the conversation talks with none."
            .to_owned(),
    };
    if reached {
        format!("{said} The conversation's thread takes it from its next response.")
    } else {
        format!("{said} New conversations start with it; the current thread was not changed.")
    }
}

/// What Kai knows of the profiles (module doc).
#[derive(Debug, Default)]
pub struct Personality {
    /// `(id, name)` in name order.
    profiles: Vec<(i64, String)>,
    /// `default_profile_id`: the Chat setting new threads start with.
    global: Option<i64>,
    /// The profile each thread of the feed talks with.
    threads: HashMap<i64, Option<i64>>,
}

impl Personality {
    /// The list as lmgw gave it.
    pub fn loaded(&mut self, list: &ProfileList) {
        self.profiles = list
            .profiles
            .iter()
            .map(|p| (p.id, p.name.clone()))
            .collect();
        self.global = list.default_profile_id;
    }

    /// A pairing or a gateway that ends: nothing is known.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The thread's profile as the feed or a set told it.
    pub fn thread_is(&mut self, thread_id: i64, profile: Option<i64>) {
        self.threads.insert(thread_id, profile);
    }

    pub fn event(&mut self, e: ProfileEvent) {
        match e {
            ProfileEvent::Upsert { id, name } => {
                match self.profiles.iter_mut().find(|(i, _)| *i == id) {
                    Some(p) => p.1 = name,
                    None => self.profiles.push((id, name)),
                }
                self.profiles.sort_by_cached_key(|(_, n)| n.to_lowercase());
            }
            ProfileEvent::Removed { id } => {
                self.profiles.retain(|(i, _)| *i != id);
                if self.global == Some(id) {
                    // lmgw empties its Chat setting with the profile.
                    self.global = None;
                }
            }
            ProfileEvent::Thread {
                thread_id,
                profile_id,
            } => self.thread_is(thread_id, profile_id),
        }
    }

    /// What the Chat setting names now, as a profile Kai knows.
    pub fn global(&self) -> Option<i64> {
        self.global
            .filter(|g| self.profiles.iter().any(|(i, _)| i == g))
    }

    /// The tray's submenu for Kai's conversation: its folder's choice and its thread. Empty (no
    /// submenu) while Kai knows no profile.
    pub fn menu(&self, folder_profile: Option<i64>, thread: Option<i64>) -> PersonalityModel {
        if self.profiles.is_empty() {
            return PersonalityModel::default();
        }
        let known = |id: i64| self.profiles.iter().any(|(i, _)| *i == id);
        // The thread's own word first; a profile Kai does not know is none.
        let thread_says = thread.and_then(|t| self.threads.get(&t)).copied();
        let shown = match thread_says {
            Some(own) => own,
            None => folder_profile,
        }
        .filter(|id| known(*id));
        let thread_has_none = thread_says == Some(None);
        let default_label = match self.global().and_then(|g| self.name(g)) {
            Some(g) if !thread_has_none => format!("{DEFAULT_LABEL} (global: {g})"),
            _ => DEFAULT_LABEL.to_owned(),
        };
        let mut options = vec![PersonalityOption {
            id: None,
            label: default_label,
        }];
        options.extend(self.profiles.iter().map(|(id, name)| PersonalityOption {
            id: Some(*id),
            label: name.clone(),
        }));
        let selected = options.iter().position(|o| o.id == shown).unwrap_or(0);
        PersonalityModel { options, selected }
    }

    fn name(&self, id: i64) -> Option<&str> {
        self.profiles
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n.as_str())
    }
}

#[cfg(test)]
mod tests {
    use lmgw_client::requests::{AppliedToCurrent, Profile};

    use super::*;

    pub fn list(names: &[(i64, &str)], global: Option<i64>) -> ProfileList {
        ProfileList {
            profiles: names
                .iter()
                .map(|(id, name)| Profile {
                    id: *id,
                    name: (*name).into(),
                    ..Profile::default()
                })
                .collect(),
            default_profile_id: global,
        }
    }

    fn patched(reached: bool) -> FolderPatched {
        FolderPatched {
            applied: reached.then(|| AppliedToCurrent {
                thread_id: 7,
                fields: vec!["profile_id".into()],
            }),
            ..FolderPatched::default()
        }
    }

    /// The name is matched in any case, `default` is null, and an unknown name lists the names in
    /// one line.
    #[test]
    fn a_name_resolves_by_the_words_of_the_call() {
        let l = list(&[(1, "Calm"), (2, "Concise")], None);
        let name = |n: &str| resolve(&l, &Target::Name(n.into()));
        assert_eq!(name("calm"), Ok(Some(1)));
        assert_eq!(name("  CONCISE "), Ok(Some(2)));
        assert_eq!(name("Default"), Ok(None));
        let e = name("jovial").unwrap_err();
        assert_eq!(
            e,
            "there is no profile named 'jovial'; the profiles are: Default, Calm, Concise"
        );
        assert!(!e.contains('\n'));
        assert_eq!(resolve(&l, &Target::Id(Some(2))), Ok(Some(2)));
        assert!(resolve(&l, &Target::Id(Some(9))).is_err());
        assert_eq!(resolve(&l, &Target::Id(None)), Ok(None));
    }

    /// The answer says what `default` came to.
    #[test]
    fn the_answer_says_what_default_came_to() {
        let l = list(&[(1, "Calm"), (2, "Concise")], Some(2));
        assert_eq!(
            answer(&l, Some(1), &patched(true)),
            "Personality set to Calm. The conversation's thread takes it from its next response."
        );
        let a = answer(&l, None, &patched(true));
        assert!(a.contains("Default is lmgw's own choice"), "{a}");
        assert!(a.contains("the profile Concise"), "{a}");
        let none = list(&[(1, "Calm")], None);
        let a = answer(&none, None, &patched(false));
        assert!(a.contains("talks with none"), "{a}");
        assert!(a.ends_with("the current thread was not changed."), "{a}");
    }

    /// The tray checks what the thread talks with; the Default item names the Chat setting.
    #[test]
    fn the_menu_checks_what_the_thread_talks_with() {
        let mut p = Personality::default();
        assert!(
            p.menu(None, None).options.is_empty(),
            "no profile, no submenu"
        );
        p.loaded(&list(&[(1, "Calm"), (2, "Concise")], Some(2)));
        let m = p.menu(None, Some(7));
        let labels: Vec<_> = m.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["Default (global: Concise)", "Calm", "Concise"]);
        assert_eq!(m.selected, 0);
        // The folder's choice, while the thread is not known.
        assert_eq!(p.menu(Some(1), Some(7)).selected, 1);
        // The thread's own word wins: the dashboard moved it.
        p.event(ProfileEvent::Thread {
            thread_id: 7,
            profile_id: Some(2),
        });
        assert_eq!(p.menu(Some(1), Some(7)).selected, 2);
        // A thread known to talk with none: Default, and no claim of a global.
        p.event(ProfileEvent::Thread {
            thread_id: 7,
            profile_id: None,
        });
        let m = p.menu(Some(1), Some(7));
        assert_eq!((m.selected, m.options[0].label.as_str()), (0, "Default"));
        // Another thread's word is not this one's.
        assert_eq!(p.menu(Some(1), Some(8)).selected, 1);
    }

    /// The tray follows `profile.*`: created and renamed in name order, deleted gone, and the
    /// Chat setting goes with its profile.
    #[test]
    fn the_list_follows_the_profile_events() {
        let mut p = Personality::default();
        p.loaded(&list(&[(2, "Concise")], Some(2)));
        let names = |p: &Personality| -> Vec<String> {
            p.menu(None, None)
                .options
                .into_iter()
                .map(|o| o.label)
                .collect()
        };
        p.event(ProfileEvent::Upsert {
            id: 1,
            name: "calm".into(),
        });
        assert_eq!(names(&p), ["Default (global: Concise)", "calm", "Concise"]);
        p.event(ProfileEvent::Upsert {
            id: 1,
            name: "Zen".into(),
        });
        assert_eq!(names(&p), ["Default (global: Concise)", "Concise", "Zen"]);
        p.event(ProfileEvent::Removed { id: 2 });
        assert_eq!(names(&p), ["Default", "Zen"]);
        assert_eq!(p.global(), None);
        p.event(ProfileEvent::Removed { id: 1 });
        assert!(names(&p).is_empty());
    }
}
