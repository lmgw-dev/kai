//! The page: a rail (Tools, Desktop, Settings, Personality) and one page at a time. Every control
//! sends one operation (`api::apply`) the moment it changes and shows Kai's answer beside it:
//! applied, saved until the next start, or the loader's refusal in words. Settings is
//! `crate::settings_page`; Personality is lmgw's profile editor (`crate::personality`); leaving it
//! with unsaved edits asks first.

use leptos::prelude::*;
use leptos::task::spawn_local;
use lmgw_ui_kit::widgets::{Modal, ModalFooter, Section};

use crate::api;
use crate::model;
use crate::personality::PersonalityPage;
use crate::settings_page::SettingsPage;
use crate::wire::{self, Effect as Applies, Entry, Request, Response, ToolInfo};

/// What the page holds, shared by every component.
#[derive(Clone, Copy)]
pub(crate) struct Ctx {
    pub(crate) entries: RwSignal<Vec<Entry>>,
    tools: RwSignal<Vec<ToolInfo>>,
    /// The settings changed since the page opened that apply only when Kai restarts.
    waiting: RwSignal<Vec<model::Waiting>>,
    /// The microphones, speakers and screens Kai lists, read once for the pickers.
    pub(crate) devices: RwSignal<Option<wire::Devices>>,
}

/// What a control says about its last operation.
#[derive(Clone, PartialEq)]
pub(crate) enum Note {
    Ok(String),
    Err(String),
}

pub(crate) type NoteSignal = RwSignal<Option<Note>>;

/// How long a "Saved" stays beside its control.
const SAVED_FOR: std::time::Duration = std::time::Duration::from_millis(2500);

/// Sends `request` and folds Kai's answer into the page.
pub(crate) fn run(ctx: Ctx, request: Request, note: NoteSignal) {
    run_saying(ctx, request, note, None);
}

/// [`run`], with `said` as the line for a change that took hold at once (instead of "Saved"); it
/// stays until the next change.
pub(crate) fn run_saying(ctx: Ctx, request: Request, note: NoteSignal, said: Option<String>) {
    note.set(None);
    spawn_local(async move {
        match api::apply(&request).await {
            Ok(Response::Changed { entry, effect }) => {
                let before = ctx.entries.with_untracked(|e| {
                    e.iter()
                        .find(|x| x.key == entry.key)
                        .map(|x| x.value.clone())
                        .unwrap_or_default()
                });
                fold(ctx, &entry);
                if effect == Applies::Restart {
                    ctx.waiting
                        .update(|w| *w = model::waiting_after(std::mem::take(w), &entry, &before));
                }
                match (effect, said) {
                    (Applies::Now, Some(text)) => note.set(Some(Note::Ok(text))),
                    _ => {
                        let text = Note::Ok(model::effect_note(effect).to_owned());
                        note.set(Some(text.clone()));
                        if effect == Applies::Now {
                            set_timeout(
                                move || {
                                    if note.get_untracked().as_ref() == Some(&text) {
                                        note.set(None);
                                    }
                                },
                                SAVED_FOR,
                            );
                        }
                    }
                }
            }
            Ok(Response::Unchanged { entry }) | Ok(Response::Entry { entry }) => fold(ctx, &entry),
            Ok(Response::Entries { .. }) => {}
            Err(why) => note.set(Some(Note::Err(why))),
        }
    });
}

fn fold(ctx: Ctx, entry: &Entry) {
    ctx.entries
        .update(|e| *e = model::replaced(std::mem::take(e), entry));
}

#[component]
pub(crate) fn NoteView(#[prop(into)] note: Signal<Option<Note>>) -> impl IntoView {
    move || {
        note.get().map(|n| match n {
            Note::Ok(t) => view! { <span class="note ok">{t}</span> }.into_any(),
            Note::Err(t) => view! { <span class="note err" role="alert">{t}</span> }.into_any(),
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Tools,
    Desktop,
    Settings,
    Personality,
}

#[component]
pub fn App() -> impl IntoView {
    let ctx = Ctx {
        entries: RwSignal::new(Vec::new()),
        tools: RwSignal::new(Vec::new()),
        waiting: RwSignal::new(Vec::new()),
        devices: RwSignal::new(None),
    };
    provide_context(ctx);
    spawn_local(async move {
        // The pickers' lists; without them a picker offers only its unset choice and the current
        // value.
        if let Ok(d) = api::devices().await {
            ctx.devices.set(Some(d));
        }
    });
    // Lines Kai wants shown above every page (Firefox and lmgw's admin tools, kai-firefox §8.3),
    // read at the start and again whenever Kai says they changed (its `kai-notices` event: a
    // Firefox linked or gone, lmgw's admin level moved, the pairing ended).
    let notices = RwSignal::new(Vec::<String>::new());
    let read_notices = move || {
        spawn_local(async move {
            if let Ok(n) = api::notices().await {
                notices.set(n.lines);
            }
        })
    };
    let changed = window_event_listener_untyped(wire::NOTICES_EVENT, move |_| read_notices());
    on_cleanup(move || changed.remove());
    read_notices();
    let page = RwSignal::new(Page::Tools);
    let loaded = RwSignal::new(false);
    let load_error = RwSignal::new(None::<String>);
    spawn_local(async move {
        let tools = api::tools().await;
        let entries = api::apply(&Request::List { section: None }).await;
        match (tools, entries) {
            (Ok(t), Ok(Response::Entries { entries })) => {
                ctx.tools.set(t);
                ctx.entries.set(entries);
                loaded.set(true);
            }
            (Err(e), _) | (_, Err(e)) => load_error.set(Some(e)),
            _ => load_error.set(Some("Kai's answer to the list was not a list".to_owned())),
        }
    });
    // The profile editor's unsaved edits: the rail asks before it leaves the page.
    let dirty = RwSignal::new(false);
    let held = RwSignal::new(None::<Page>);
    let asking = RwSignal::new(false);
    let go = move |p: Page| {
        if page.get_untracked() == Page::Personality
            && p != Page::Personality
            && dirty.get_untracked()
        {
            held.set(Some(p));
            asking.set(true);
        } else {
            page.set(p);
        }
    };
    let discard_and_go = move |_| {
        if let Some(p) = held.get_untracked() {
            dirty.set(false);
            page.set(p);
        }
        asking.set(false);
    };
    let nav = move |p: Page, label: &'static str| {
        view! {
            <button class="rail-item" class:active=move || page.get() == p
                on:click=move |_| go(p)>{label}</button>
        }
    };
    view! {
        <div class="app density-dense">
            <nav class="rail" aria-label="Sections">
                <div class="rail-head">"Kai"</div>
                {nav(Page::Tools, "Tools")}
                {nav(Page::Desktop, "Desktop")}
                {nav(Page::Settings, "Settings")}
                {nav(Page::Personality, "Personality")}
            </nav>
            <main class="content">
                {move || load_error.get().map(|e| view! {
                    <div class="banner err" role="alert">
                        "Kai's settings could not be read: " {e}
                    </div>
                })}
                {move || notices.get().into_iter().map(|line| view! {
                    <div class="banner warn" role="status">{line}</div>
                }).collect::<Vec<_>>()}
                <Show when=move || loaded.get()>
                    <div class="page-body">
                        {move || match page.get() {
                            Page::Tools => view! { <ToolsPage/> }.into_any(),
                            Page::Desktop => view! { <DesktopPage/> }.into_any(),
                            Page::Settings => view! { <SettingsPage/> }.into_any(),
                            Page::Personality => view! { <PersonalityPage dirty=dirty/> }.into_any(),
                        }}
                    </div>
                    <RestartFooter/>
                </Show>
            </main>
            <Modal open=asking title="Unsaved changes">
                <p class="dirty-q">"The profile has unsaved changes. Leaving the page discards them."</p>
                <ModalFooter>
                    <span class="foot-danger">
                        <button type="button" class="btn danger" on:click=discard_and_go>
                            "Discard and leave"
                        </button>
                    </span>
                    <button type="button" class="btn primary" on:click=move |_| asking.set(false)>
                        "Stay"
                    </button>
                </ModalFooter>
            </Modal>
        </div>
    }
}

/// The footer: which settings still wait for a restart, and the button that restarts Kai.
#[component]
fn RestartFooter() -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let why = RwSignal::new(None::<String>);
    let restarting = RwSignal::new(false);
    let restart = move |_| {
        restarting.set(true);
        why.set(None);
        spawn_local(async move {
            if let Err(e) = api::restart().await {
                restarting.set(false);
                why.set(Some(e));
            }
        });
    };
    view! {
        <Show when=move || !ctx.waiting.with(|w| w.is_empty())>
            <div class="page-foot">
                <span class="foot-text">
                    {move || ctx.waiting.with(|w| model::waiting_line(w))}
                </span>
                {move || why.get().map(|e| view! { <span class="note err" role="alert">{e}</span> })}
                <button type="button" class="btn sm" prop:disabled=move || restarting.get()
                    on:click=restart>
                    {move || if restarting.get() { "Restarting…" } else { "Restart Kai now" }}
                </button>
            </div>
        </Show>
    }
}

#[component]
pub(crate) fn PageHead(
    title: &'static str,
    sub: &'static str,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <header class="page-head">
            <h1>{title}</h1>
            <span class="sub">{sub}</span>
            <div class="actions">{children.map(|c| c())}</div>
        </header>
    }
}

/// A `<select>` over `words`, the first option being "no entry in the file".
#[component]
fn WordSelect(
    #[prop(into)] current: Signal<Option<String>>,
    words: &'static [&'static str],
    #[prop(into)] default_label: String,
    #[prop(into)] label: String,
    on_pick: Callback<Option<String>>,
) -> impl IntoView {
    view! {
        <select class="select" aria-label=label on:change=move |ev| {
            let v = event_target_value(&ev);
            on_pick.run((!v.is_empty()).then_some(v));
        }>
            <option value="" prop:selected=move || current.get().is_none()>{default_label}</option>
            {words.iter().map(|w| {
                let w = *w;
                view! {
                    <option value=w prop:selected=move || current.get().as_deref() == Some(w)>
                        {model::word_label(w)}
                    </option>
                }
            }).collect_view()}
        </select>
    }
}

// ---------------------------------------------------------------- Tools

#[component]
fn ToolsPage() -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let filter = RwSignal::new(String::new());
    let groups = Memo::new(move |_| {
        let rows = ctx
            .entries
            .with(|e| ctx.tools.with(|t| model::tool_rows(t, e)));
        model::by_area(rows, &filter.get())
    });
    // Rows are keyed by the group and tool names, so a change of one row's setting does not build
    // the table again (the open select and the scroll stay).
    let areas = Memo::new(move |_| groups.get().into_iter().map(|(a, _)| a).collect::<Vec<_>>());
    let config_on = Memo::new(move |_| ctx.entries.with(|e| model::config_tools_on(e)));
    let config_note = RwSignal::new(None);
    view! {
        <PageHead title="Tools" sub="What a caller may do on this desktop. By default Kai confirms; \"allow if trusted\" lets the owner and trusted callers pass and asks about every other (switching a routine on is always asked unless it is set to \"allow\")">
            <input class="input search" type="search" placeholder="Filter tools"
                prop:value=move || filter.get()
                on:input=move |ev| filter.set(event_target_value(&ev))/>
        </PageHead>
        <label class="check-line">
            <input type="checkbox" prop:checked=move || config_on.get()
                on:change=move |ev| run(ctx, Request::Set {
                    key: "policy.config_tools".into(),
                    value: event_target_checked(&ev).to_string(),
                }, config_note)/>
            <span>"Offer Kai's configuration tools"</span>
            <span class="hint">
                "You and Kai's own key may then read and change these settings by tool call."
            </span>
            <NoteView note=config_note/>
        </label>
        <For each=move || areas.get() key=|a| a.clone() let:area>
            {
                let a = area.clone();
                let names = Memo::new(move |_| {
                    groups.get().into_iter().find(|(g, _)| *g == a).map(|(_, rows)| {
                        rows.into_iter().map(|r| r.info).collect::<Vec<_>>()
                    }).unwrap_or_default()
                });
                let count = Signal::derive(move || names.get().len().to_string());
                view! {
                    <Section title=model::title_case(&area) count=count>
                        <table class="data tools">
                            <thead><tr>
                                <th>"Tool"</th><th>"Policy"</th><th></th>
                            </tr></thead>
                            <tbody>
                                <For each=move || names.get() key=|i| i.name.clone() let:info>
                                    <ToolRowView info=info/>
                                </For>
                            </tbody>
                        </table>
                    </Section>
                }
            }
        </For>
        <Show when=move || areas.get().is_empty()>
            <p class="empty">"No tool matches."</p>
        </Show>
    }
}

#[component]
fn ToolRowView(info: ToolInfo) -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let note: NoteSignal = RwSignal::new(None);
    let state_key = format!("policy.tools.{}", info.name);
    let level_key = format!("policy.levels.{}", info.name);
    let widen_key = format!("policy.widen_by_tools.{}", info.name);
    let widen_key2 = widen_key.clone();
    let widen = Memo::new(move |_| ctx.entries.with(|e| model::flag_on(e, &widen_key)));
    let state = Memo::new(move |_| ctx.entries.with(|e| model::set_word(e, &state_key)));
    let level = Memo::new(move |_| ctx.entries.with(|e| model::set_word(e, &level_key)));
    let config_tool = info.config_tool;
    let unlisted = Memo::new(move |_| {
        config_tool && !ctx.entries.with(|e| model::config_tools_on(e))
            || state.get().as_deref() == Some("off")
    });
    let tool = info.name.clone();
    let tool2 = info.name.clone();
    let pick_state = Callback::new(move |state: Option<String>| {
        run(
            ctx,
            Request::ToolState {
                tool: tool.clone(),
                state,
            },
            note,
        )
    });
    let pick_level = Callback::new(move |level: Option<String>| {
        run(
            ctx,
            Request::ToolLevel {
                tool: tool2.clone(),
                level,
            },
            note,
        )
    });
    let why = if config_tool {
        "Listed only while the configuration tools are offered"
    } else {
        ""
    };
    // Firefox's hands-and-eyes switch changes the default of the tools it covers (read as the row
    // is built: the switch is on the Settings page, and the Tools page is built again on return).
    let default_state = if info.by_switch
        && ctx
            .entries
            .with_untracked(|e| model::flag_on(e, "browser.hands_and_eyes"))
    {
        "allow if trusted, as Firefox's hands-and-eyes switch makes it".to_owned()
    } else {
        info.default_state.clone()
    };
    view! {
        <tr class:muted=move || unlisted.get() title=why>
            <td class="tool">
                <div class="tool-name">{info.name.clone()}</div>
                <div class="tool-desc">{model::first_sentence(&info.description)}</div>
            </td>
            <td class="policy">
                <div class="pol">
                    <span class="cap">"Who may call"</span>
                    <WordSelect current=level words=&wire::LEVELS[..]
                        default_label=format!("Default ({})", info.default_level)
                        label=format!("Who may call {}", info.name) on_pick=pick_level/>
                    <span class="cap">"When called"</span>
                    <WordSelect current=state words=&wire::STATES[..]
                        default_label=format!("Default ({default_state})")
                        label=format!("What happens when {} is called", info.name) on_pick=pick_state/>
                    <span class="cap">"Config tools"</span>
                    <label class="check-line" title="Off: tool_state_set, tool_level_set, config_set and config_unset refuse to widen this tool's state or level (toward allow, toward a wider caller); for run_command also a change of launch.terminal. Narrowing always works. Only this window or the file sets it.">
                        <input type="checkbox" prop:checked=move || widen.get()
                            on:change=move |ev| {
                                let key = widen_key2.clone();
                                let request = if event_target_checked(&ev) {
                                    Request::Set { key, value: "true".into() }
                                } else {
                                    Request::Unset { key }
                                };
                                run(ctx, request, note)
                            }/>
                        <span class="hint">"may widen this tool"</span>
                    </label>
                </div>
            </td>
            <td class="status"><NoteView note=note/></td>
        </tr>
    }
}

// -------------------------------------------------------------- Desktop

#[component]
fn DesktopPage() -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let caps = Memo::new(move |_| {
        ctx.entries
            .with(|e| model::provider_rows(e))
            .into_iter()
            .map(|(c, _)| c)
            .collect::<Vec<_>>()
    });
    view! {
        <PageHead title="Desktop"
            sub="Which provider answers each desktop capability; automatic takes the first that works here"/>
        <table class="data providers">
            <thead><tr><th>"Capability"</th><th>"Provider"</th><th></th></tr></thead>
            <tbody>
                <For each=move || caps.get() key=|c| c.clone() let:cap>
                    <ProviderRow cap=cap/>
                </For>
            </tbody>
        </table>
        <RemoteControlView/>
    }
}

/// The RemoteDesktop portal's stored permission (WP16c, the owner's decision of 2026-10-09): Kai
/// keeps it, and this is where the owner ends it (as `kai --forget-remote-control`), after a
/// confirmation.
#[component]
fn RemoteControlView() -> impl IntoView {
    // `None` until Kai answered the first look.
    let stored = RwSignal::new(None::<bool>);
    let note: NoteSignal = RwSignal::new(None);
    let asking = RwSignal::new(false);
    spawn_local(async move {
        match api::remote_control(false).await {
            Ok(r) => stored.set(Some(r.stored)),
            Err(why) => note.set(Some(Note::Err(why))),
        }
    });
    let forget = move |_| {
        asking.set(false);
        note.set(None);
        spawn_local(async move {
            match api::remote_control(true).await {
                Ok(r) => {
                    stored.set(Some(r.stored));
                    note.set(Some(Note::Ok(model::forgot_note(r.forgot).to_owned())));
                }
                Err(why) => note.set(Some(Note::Err(why))),
            }
        });
    };
    view! {
        <Section title="Remote control">
            <div class="remote-control">
                <p class="remote-status">{move || model::remote_status(stored.get())}</p>
                <p class="hint">
                    "Kai types and presses keys through the desktop's RemoteDesktop portal where                      KWin's fake input is not granted. The permission lasts until it is forgotten                      here; revoking Kai in System Settings does not end it."
                </p>
                <div class="ctl-main">
                    <button type="button" class="btn danger"
                        prop:disabled=move || stored.get() != Some(true)
                        on:click=move |_| asking.set(true)>
                        "Forget remote-control permission"
                    </button>
                    <NoteView note=note/>
                </div>
            </div>
        </Section>
        <Modal open=asking title="Forget the remote-control permission?">
            <p class="dirty-q">
                "Kai deletes the portal's stored permission. The next time it types or presses                  keys through the portal, the desktop asks you again."
            </p>
            <ModalFooter>
                <span class="foot-danger">
                    <button type="button" class="btn danger" on:click=forget>"Forget"</button>
                </span>
                <button type="button" class="btn primary" on:click=move |_| asking.set(false)>
                    "Keep it"
                </button>
            </ModalFooter>
        </Modal>
    }
}

#[component]
fn ProviderRow(cap: String) -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let note: NoteSignal = RwSignal::new(None);
    let key = format!("desktop.{cap}");
    let current = Memo::new(move |_| ctx.entries.with(|e| model::set_word(e, &key)));
    let capability = cap.clone();
    let pick = Callback::new(move |provider: Option<String>| {
        run(
            ctx,
            Request::Provider {
                capability: capability.clone(),
                provider,
            },
            note,
        )
    });
    view! {
        <tr>
            <td class="tool"><div class="tool-name">{model::title_case(&cap)}</div></td>
            <td>
                <WordSelect current=current words=&wire::PROVIDERS[..]
                    default_label="Automatic" label=format!("Provider for {cap}") on_pick=pick/>
            </td>
            <td class="status"><NoteView note=note/></td>
        </tr>
    }
}
