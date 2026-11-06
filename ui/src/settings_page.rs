//! The Settings page: every setting of Kai's configuration in the words of Kai's settings catalog
//! (the owner's ruling, 2026-10-19: no setting is shown by its raw configuration key). The
//! sections come in the catalog's order, each with its everyday settings first and its rare tuning
//! folded under "Advanced"; a setting shows its label, its help line, its unit after the control
//! and a control of its kind. The key itself is only the row's tooltip. Every control saves the
//! moment it changes (a text when it is left or Enter is pressed), and a refusal says why under
//! the field, naming settings by their labels.
//!
//! The Conversation is a choice of its own (`Control::SharedName`): the conversation every Kai
//! client shares, or one of this computer's own under a name of its own.

use leptos::prelude::*;
use leptos::task::spawn_local;
use lmgw_ui_kit::widgets::{Field, Section};
use serde_json::Value;

use crate::api;
use crate::app::{Ctx, Note, NoteSignal, NoteView, PageHead, run, run_saying};
use crate::firefox::FirefoxExtension;
use crate::model::{self, SectionView};
use crate::wire::{Control, Request, Source};

type Groups = Vec<(Option<String>, Vec<String>)>;

#[component]
pub(crate) fn SettingsPage() -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let filter = RwSignal::new(String::new());
    let layout = Memo::new(move |_| {
        ctx.entries
            .with(|e| model::settings_sections(e, &filter.get()))
    });
    // Only the ids: a changed value builds no section again.
    let ids =
        Memo::new(move |_| layout.with(|l| l.iter().map(|s| s.id.clone()).collect::<Vec<_>>()));
    let filtering = Signal::derive(move || !filter.get().trim().is_empty());
    view! {
        <PageHead title="Settings"
            sub="How Kai behaves on this computer; a change is saved as you make it">
            <input class="input search" type="search" placeholder="Find a setting"
                aria-label="Find a setting"
                prop:value=move || filter.get()
                on:input=move |ev| filter.set(event_target_value(&ev))/>
        </PageHead>
        <div class="settings">
            <For each=move || ids.get() key=|id| id.clone() let:id>
                <SectionBlock id=id layout=layout filtering=filtering/>
            </For>
            <Show when=move || ids.get().is_empty()>
                <p class="empty">"No setting matches."</p>
            </Show>
        </div>
    }
}

/// One section: its everyday settings, then "Advanced" (folded unless a filter is narrowing).
#[component]
fn SectionBlock(
    id: String,
    layout: Memo<Vec<SectionView>>,
    filtering: Signal<bool>,
) -> impl IntoView {
    // The Firefox section starts with the extension's block: connected now, and the install entry.
    let firefox = id == "browser";
    let section = Memo::new(move |_| {
        layout
            .with(|l| l.iter().find(|s| s.id == id).cloned())
            .unwrap_or_default()
    });
    let title = section.with_untracked(|s| s.label.clone());
    let count = Signal::derive(move || section.with(|s| s.len()).to_string());
    let main = Memo::new(move |_| section.with(|s| s.main.clone()));
    let advanced = Memo::new(move |_| section.with(|s| s.advanced.clone()));
    let advanced_len = Memo::new(move |_| section.with(|s| s.advanced_len()));
    view! {
        <Section title=title count=count force_open=filtering>
            {firefox.then(|| view! { <FirefoxExtension/> })}
            <GroupList groups=main/>
            <Show when=move || { advanced_len.get() > 0 }>
                <details class="adv" prop:open=move || filtering.get()>
                    <summary>{move || format!("Advanced ({})", advanced_len.get())}</summary>
                    <GroupList groups=advanced/>
                </details>
            </Show>
        </Section>
    }
}

/// A part's groups: the section's own keys as they are, a group under its heading.
#[component]
fn GroupList(groups: Memo<Groups>) -> impl IntoView {
    let names =
        Memo::new(move |_| groups.with(|g| g.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>()));
    view! {
        <For each=move || names.get() key=|n| n.clone() let:name>
            {
                let n = name.clone();
                let keys = Memo::new(move |_| {
                    groups.with(|g| {
                        g.iter().find(|(x, _)| *x == n).map(|(_, k)| k.clone()).unwrap_or_default()
                    })
                });
                let rows = view! {
                    <For each=move || keys.get() key=|k| k.clone() let:key>
                        <SettingRow key=key/>
                    </For>
                };
                match name {
                    None => rows.into_any(),
                    Some(title) => view! {
                        <div class="subsec">
                            <div class="subsec-head">{title}</div>
                            {rows}
                        </div>
                    }
                    .into_any(),
                }
            }
        </For>
    }
}

/// The text a typed control (a number, a text, a list) shows for `value`.
fn shown_text(control: &Control, value: &Value) -> String {
    match control {
        Control::List => model::list_lines(value),
        _ => model::show_value(value),
    }
}

#[component]
fn SettingRow(key: String) -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let Some(first) = ctx
        .entries
        .with_untracked(|e| e.iter().find(|x| x.key == key).cloned())
    else {
        return ().into_any();
    };
    let meta = first.meta.clone();
    let note: NoteSignal = RwSignal::new(None);
    // What the page itself found wrong with a value before sending it.
    let invalid = RwSignal::new(None::<String>);
    let k = key.clone();
    let entry = Memo::new(move |_| {
        ctx.entries
            .with(|e| e.iter().find(|x| x.key == k).cloned())
            .unwrap_or_else(|| first.clone())
    });
    let value = Memo::new(move |_| entry.get().value);
    let set = Memo::new(move |_| entry.get().set);
    let control = meta.control.clone();
    let shown = {
        let control = control.clone();
        Memo::new(move |_| shown_text(&control, &value.get()))
    };
    let draft = RwSignal::new(shown.get_untracked());
    // A value that came in (our own write, or a reset) replaces what was typed.
    Effect::new(move |_| {
        draft.set(shown.get());
        invalid.set(None);
    });
    let typed = matches!(
        control,
        Control::Number { .. } | Control::Text { .. } | Control::List
    );
    let dirty = Signal::derive(move || typed && draft.get() != shown.get());
    let error = Signal::derive(move || {
        invalid.get().or_else(|| match note.get() {
            Some(Note::Err(t)) => Some(t),
            _ => None,
        })
    });
    let ok = Signal::derive(move || note.get().filter(|n| matches!(n, Note::Ok(_))));
    let send = {
        let key = key.clone();
        move |request: Option<String>| {
            invalid.set(None);
            let key = key.clone();
            // The switch's own convenience: a visible second write, never a hidden one.
            if model::announces_with(
                &key,
                request.as_deref(),
                &value.get_untracked(),
                &ctx.entries.get_untracked(),
            ) {
                run(
                    ctx,
                    Request::Set {
                        key: model::ANNOUNCE_KEY.to_owned(),
                        value: "true".to_owned(),
                    },
                    RwSignal::new(None),
                );
            }
            let request = match request {
                Some(value) => Request::Set { key, value },
                None => Request::Unset { key },
            };
            run(ctx, request, note);
        }
    };
    let label = meta.label.clone();
    let editor = match control.clone() {
        Control::Switch => {
            let send = send.clone();
            view! {
                <label class="switch-line">
                    <input type="checkbox" aria-label=label.clone()
                        prop:checked=move || value.get() == Value::Bool(true)
                        on:change=move |ev| send(Some(event_target_checked(&ev).to_string()))/>
                </label>
            }
            .into_any()
        }
        Control::Pick { from, unset } => {
            let send = send.clone();
            view! {
                <DevicePick from=from unset=unset value=value label=label.clone() send=Callback::new(send)/>
            }
            .into_any()
        }
        Control::Choice { options, unset } => {
            let send = send.clone();
            view! {
                <select class="select" aria-label=label.clone() on:change=move |ev| {
                    let word = event_target_value(&ev);
                    send((!word.is_empty()).then(|| Value::String(word).to_string()))
                }>
                    {unset.map(|u| view! {
                        <option value="" prop:selected=move || value.get().is_null()>{u}</option>
                    })}
                    {options.into_iter().map(|o| {
                        let word = o.word.clone();
                        view! {
                            <option value=o.word
                                prop:selected=move || value.get().as_str() == Some(word.as_str())>
                                {o.name}
                            </option>
                        }
                    }).collect_view()}
                </select>
            }
            .into_any()
        }
        Control::Number { min, max } => {
            let unit = meta.unit.clone();
            let send = send.clone();
            let commit = move || {
                let text = draft.get_untracked();
                if text == shown.get_untracked() {
                    return;
                }
                match model::number_value(&text, min, max, unit.as_deref()) {
                    Err(why) => invalid.set(Some(why)),
                    Ok(None) if !set.get_untracked() => draft.set(shown.get_untracked()),
                    Ok(n) => send(n.map(|n| n.to_string())),
                }
            };
            view! {
                <input class="input num" type="text" inputmode="numeric" spellcheck="false"
                    aria-label=label.clone()
                    prop:value=move || draft.get()
                    on:input=move |ev| draft.set(event_target_value(&ev))
                    on:change=move |_| commit()
                    on:keydown=move |ev| if ev.key() == "Escape" { draft.set(shown.get_untracked()) }/>
            }
            .into_any()
        }
        Control::Text { unset } => {
            let send = send.clone();
            let commit = move || {
                let text = draft.get_untracked();
                if text == shown.get_untracked() {
                    return;
                }
                let text = text.trim().to_owned();
                // Quoted: a text is a text, whatever it looks like (`5`, `true`, `[a]`).
                send((!text.is_empty()).then(|| Value::String(text).to_string()))
            };
            let placeholder = unset.unwrap_or_default();
            view! {
                <input class="input" type="text" spellcheck="false" aria-label=label.clone()
                    placeholder=placeholder
                    prop:value=move || draft.get()
                    on:input=move |ev| draft.set(event_target_value(&ev))
                    on:change=move |_| commit()
                    on:keydown=move |ev| if ev.key() == "Escape" { draft.set(shown.get_untracked()) }/>
            }
            .into_any()
        }
        Control::List => {
            let send = send.clone();
            let commit = move || {
                let text = draft.get_untracked();
                if text == shown.get_untracked() {
                    return;
                }
                send(model::list_value(&text))
            };
            view! {
                <textarea class="input ta list" spellcheck="false" aria-label=label.clone()
                    rows=move || shown.get().lines().count().clamp(2, 8).to_string()
                    prop:value=move || draft.get()
                    on:input=move |ev| draft.set(event_target_value(&ev))
                    on:change=move |_| commit()
                    on:keydown=move |ev| if ev.key() == "Escape" { draft.set(shown.get_untracked()) }>
                </textarea>
            }
            .into_any()
        }
        Control::ReadOnly => view! {
            <span class="readonly">{move || model::read_only_text(&value.get())}</span>
        }
        .into_any(),
        Control::Hidden => return ().into_any(),
        Control::SharedName { shared, own } => {
            let shared_name = shared.clone();
            let say = Callback::new(move |name: Option<String>| {
                let text = name
                    .as_deref()
                    .and_then(|n| serde_json::from_str::<String>(n).ok())
                    .unwrap_or_else(|| shared_name.clone());
                let request = match name {
                    Some(value) => Request::Set {
                        key: "conversation.folder_name".to_owned(),
                        value,
                    },
                    None => Request::Unset {
                        key: "conversation.folder_name".to_owned(),
                    },
                };
                invalid.set(None);
                run_saying(
                    ctx,
                    request,
                    note,
                    Some(format!("Now using \u{201c}{text}\u{201d}")),
                );
                follow_up(note, text);
            });
            view! {
                <ConversationChoice value=value shared=shared own=own send=say invalid=invalid/>
            }
            .into_any()
        }
    };
    let resettable = !matches!(control, Control::ReadOnly | Control::SharedName { .. });
    let can_reset = Signal::derive(move || resettable && model::resettable(&entry.get()));
    let reset = {
        let send = send.clone();
        move |_| send(None)
    };
    let unit = meta.unit.clone();
    let tip = format!("{key} in Kai's config.toml");
    view! {
        <div class="setting" title=tip data-key=key.clone()>
            <Field label=meta.label.clone() hint=meta.help.clone() dirty=dirty error=error>
                <div class="ctl">
                    <div class="ctl-main">
                        {editor}
                        {unit.map(|u| view! { <span class="unit">{u}</span> })}
                        <NoteView note=ok/>
                    </div>
                    <div class="ctl-reset">
                        <Show when=move || can_reset.get()>
                            <button type="button" class="btn sm ghost"
                                title="Remove your value: the default applies again"
                                on:click=reset.clone()>"Reset"</button>
                        </Show>
                    </div>
                </div>
            </Field>
        </div>
    }
    .into_any()
}

/// After a switch: once the new folder has had time to be found, say what it waits for when its
/// model is not set yet (Kai shows the owner the steps itself).
fn follow_up(note: NoteSignal, name: String) {
    set_timeout(
        move || {
            spawn_local(async move {
                if let Ok(c) = api::conversation().await
                    && c.paired
                    && c.folder.as_deref() == Some(name.trim())
                    && c.model.is_none()
                {
                    note.set(Some(Note::Ok(
                        "Waiting for its model in lmgw. Kai shows the steps.".to_owned(),
                    )));
                }
            });
        },
        std::time::Duration::from_millis(2500),
    );
}

/// A picker over what Kai can list (microphones, speakers, screens) by the names it has for them,
/// with `unset` for none picked. A stored value the list lacks stays listed, marked.
#[component]
fn DevicePick(
    from: Source,
    unset: String,
    value: Memo<Value>,
    label: String,
    send: Callback<Option<String>>,
) -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    view! {
        <select class="select" aria-label=label on:change=move |ev| {
            let word = event_target_value(&ev);
            send.run((!word.is_empty()).then(|| Value::String(word).to_string()))
        }>
            <option value="" prop:selected=move || value.get().is_null()>{unset}</option>
            {move || {
                let listed = ctx
                    .devices
                    .with(|d| d.as_ref().map(|d| d.of(from).to_vec()).unwrap_or_default());
                let current = value.get().as_str().map(str::to_owned);
                let missing = current
                    .clone()
                    .filter(|c| !listed.iter().any(|l| l.name == *c))
                    .map(|c| {
                        let label = if listed.is_empty() { c.clone() } else { format!("{c} (not present)") };
                        (c, label)
                    });
                listed
                    .into_iter()
                    .map(|c| (c.name, c.label))
                    .chain(missing)
                    .map(|(name, label)| {
                        let selected = current.as_deref() == Some(name.as_str());
                        view! { <option value=name prop:selected=selected>{label}</option> }
                    })
                    .collect_view()
            }}
        </select>
    }
}

/// The Conversation: the one every Kai client shares (the name `shared`, which all of them use by
/// default), or this computer's own under a name of its own (prefilled with `own`). Going back to
/// the shared one switches at once. Choosing "only on this computer" switches at once only to an
/// own name used a moment ago; otherwise it shows the name field, and the button beside it (or
/// Enter) switches, once, to the name as typed. Kai switches to the conversation at once.
#[component]
fn ConversationChoice(
    value: Memo<Value>,
    shared: String,
    own: String,
    /// The write: the name, or `None` for the shared one (the key removed).
    send: Callback<Option<String>>,
    invalid: RwSignal<Option<String>>,
) -> impl IntoView {
    let shared_name = shared.clone();
    let stored = Memo::new(move |_| model::own_conversation(&value.get(), &shared_name));
    let own_on = RwSignal::new(stored.get_untracked().is_some());
    let name = RwSignal::new(stored.get_untracked().unwrap_or(own));
    // The own name in use until the owner went back to the shared one: choosing "only on this
    // computer" again returns to it.
    let last_own = RwSignal::new(None::<String>);
    Effect::new(move |_| match stored.get() {
        Some(n) => {
            own_on.set(true);
            name.set(n);
        }
        None => own_on.set(false),
    });
    let shared_for_commit = shared.clone();
    let commit = move || match model::own_name_value(&name.get_untracked(), &shared_for_commit) {
        Err(why) => invalid.set(Some(why)),
        Ok(None) => send.run(None),
        Ok(Some(n)) if stored.get_untracked().as_deref() == Some(n.as_str()) => {}
        Ok(Some(n)) => send.run(Some(Value::String(n).to_string())),
    };
    let pick_shared = move |_| {
        own_on.set(false);
        invalid.set(None);
        if let Some(n) = stored.get_untracked() {
            last_own.set(Some(n));
            send.run(None);
        }
    };
    let commit_on_pick = commit.clone();
    let pick_own = move |_| {
        own_on.set(true);
        if let Some(n) = last_own.get_untracked() {
            name.set(n);
            commit_on_pick();
        }
    };
    let revert = move || {
        invalid.set(None);
        if let Some(n) = stored.get_untracked() {
            name.set(n);
        }
    };
    let changed = Signal::derive(move || {
        let typed = name.get();
        let typed = typed.trim();
        !typed.is_empty() && stored.get().as_deref() != Some(typed)
    });
    let shared_line = format!(
        "This computer and your Kai phones continue the conversation \u{201c}{shared}\u{201d}."
    );
    view! {
        <div class="conv-choice" role="radiogroup" aria-label="Conversation">
            <label class="radio-line">
                <input type="radio" name="kai-conversation"
                    prop:checked=move || !own_on.get() on:change=pick_shared/>
                <span class="radio-text">
                    <span class="radio-title">"Shared with your other Kai apps"</span>
                    <span class="radio-sub">{shared_line}</span>
                </span>
            </label>
            <label class="radio-line">
                <input type="radio" name="kai-conversation"
                    prop:checked=move || own_on.get() on:change=pick_own/>
                <span class="radio-text">
                    <span class="radio-title">"Only on this computer"</span>
                    <span class="radio-sub">"A conversation of its own, which your other Kai apps do not continue."</span>
                </span>
            </label>
            <Show when=move || own_on.get()>
                <div class="own-name">
                    <label class="own-label" for="kai-own-conversation">"Conversation name"</label>
                    <div class="own-row">
                        <input id="kai-own-conversation" class="input" type="text" spellcheck="false"
                            prop:value=move || name.get()
                            on:input=move |ev| name.set(event_target_value(&ev))
                            on:keydown={
                                let commit = commit.clone();
                                move |ev| match ev.key().as_str() {
                                    "Enter" => commit(),
                                    "Escape" => revert(),
                                    _ => {}
                                }
                            }/>
                        <button type="button" class="btn sm" prop:disabled=move || !changed.get()
                            on:click={
                                let commit = commit.clone();
                                move |_| commit()
                            }>
                            "Use this conversation"
                        </button>
                    </div>
                    <p class="own-note">
                        "A new conversation needs its model picked in lmgw's dashboard the first \
                         time; Kai then shows you the steps."
                    </p>
                </div>
            </Show>
        </div>
    }
}
