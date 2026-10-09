//! The Personality page (ruling 29, K31): lmgw's personality profiles, edited with the kit's
//! `ProfilesPanel`, the same component lmgw's dashboard shows (lmgw's personality-profiles design,
//! decision 7). Above it, which profile Kai's conversation talks with (the core's facts, WP22's
//! state); picking one stays the tray's and `personality_set`'s.
//!
//! The panel is mounted once Kai says it is paired, with the conversation's thread (Preview, Test
//! and Speak assemble as Kai's thread would) and the folder's model as the Test model; the model
//! catalog its pickers read is provided here and fetched then. Unpaired, the page says so and
//! mounts nothing that would ask lmgw.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use lmgw_ui_kit::catalog::provide_model_catalog;
use lmgw_ui_kit::profiles::ProfilesPanel;

use crate::api;
use crate::app::PageHead;
use crate::conversation;
use crate::wire::Conversation;

/// How long after a change in the editor the header asks again: lmgw's feed event reaches Kai's
/// core first (a profile renamed, the default's profile deleted).
const FOLLOW_UP: Duration = Duration::from_millis(1500);

#[component]
pub fn PersonalityPage(
    /// Kept `true` while the editor has unsaved edits (the rail asks before it leaves).
    dirty: RwSignal<bool>,
) -> impl IntoView {
    let facts = RwSignal::new(None::<Conversation>);
    let error = RwSignal::new(None::<String>);
    // The panel's props, fixed once Kai said it is paired: the panel is not mounted again when
    // the header refreshes.
    let mounted = RwSignal::new(None::<(Option<i64>, Option<String>)>);
    provide_model_catalog(Signal::derive(move || mounted.get().is_none()));
    let load = move || {
        spawn_local(async move {
            match api::conversation().await {
                Ok(c) => {
                    if c.paired && mounted.get_untracked().is_none() {
                        mounted.try_set(Some((c.thread_id, c.model.clone())));
                    }
                    error.try_set(None);
                    facts.try_set(Some(c));
                }
                Err(e) => {
                    error.try_set(Some(e));
                }
            }
        })
    };
    load();
    let changed = Callback::new(move |_| {
        load();
        set_timeout(load, FOLLOW_UP);
    });
    view! {
        <PageHead title="Personality"
            sub="How Kai's conversation talks: lmgw's personality profiles, the same as in lmgw's Chat"/>
        {move || error.get().map(|e| view! {
            <p class="note err" role="alert">"Kai's conversation could not be read: " {e}</p>
        })}
        {move || facts.get().map(|c| {
            if c.paired {
                view! {
                    <div class="conv">
                        <span class="now">"Kai's conversation talks with " <b>{conversation::talks_with(&c)}</b></span>
                        <span class="facts">{conversation::facts(&c)}</span>
                        <span class="how">
                            "Pick another in the tray's Personality menu, or by voice. Edits here are lmgw's \
                             profiles: they change every thread that uses one."
                        </span>
                    </div>
                }.into_any()
            } else {
                view! {
                    <p class="unpaired" role="status">
                        {c.why.unwrap_or_else(|| "Kai is not paired with lmgw".to_owned())}
                    </p>
                }.into_any()
            }
        })}
        {move || mounted.get().map(|(thread, model)| view! {
            <ProfilesPanel thread_id=thread test_model=model on_changed=changed dirty=dirty/>
        })}
    }
}
