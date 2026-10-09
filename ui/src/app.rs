//! The page: a rail (Tools, Desktop, Settings, Personality) and one page at a time. Every control
//! sends one operation (`api::apply`) the moment it changes and shows Kai's answer beside it:
//! applied, saved until the next start, or the loader's refusal in words. Personality is lmgw's
//! profile editor (`crate::personality`); leaving it with unsaved edits asks first.

use leptos::prelude::*;
use leptos::task::spawn_local;
use lmgw_ui_kit::widgets::{Field, Modal, ModalFooter, Section};

use crate::api;
use crate::model::{self, Editor};
use crate::personality::PersonalityPage;
use crate::wire::{self, Effect as Applies, Entry, Request, Response, ToolInfo};

/// What the page holds, shared by every component.
#[derive(Clone, Copy)]
struct Ctx {
    entries: RwSignal<Vec<Entry>>,
    tools: RwSignal<Vec<ToolInfo>>,
    /// A change since the page opened applies only when Kai restarts.
    restart: RwSignal<bool>,
}

/// What a control says about its last operation.
#[derive(Clone, PartialEq)]
enum Note {
    Ok(String),
    Err(String),
}

type NoteSignal = RwSignal<Option<Note>>;

/// Sends `request` and folds Kai's answer into the page.
fn run(ctx: Ctx, request: Request, note: NoteSignal) {
    note.set(None);
    spawn_local(async move {
        match api::apply(&request).await {
            Ok(Response::Changed { entry, effect }) => {
                fold(ctx, &entry);
                if effect == Applies::Restart {
                    ctx.restart.set(true);
                }
                note.set(Some(Note::Ok(model::effect_note(effect).to_owned())));
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
fn NoteView(#[prop(into)] note: Signal<Option<Note>>) -> impl IntoView {
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
        restart: RwSignal::new(false),
    };
    provide_context(ctx);
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
                <Show when=move || loaded.get()>
                    <div class="page-body">
                        {move || match page.get() {
                            Page::Tools => view! { <ToolsPage/> }.into_any(),
                            Page::Desktop => view! { <DesktopPage/> }.into_any(),
                            Page::Settings => view! { <SettingsPage/> }.into_any(),
                            Page::Personality => view! { <PersonalityPage dirty=dirty/> }.into_any(),
                        }}
                    </div>
                    <Show when=move || ctx.restart.get()>
                        <div class="page-foot">
                            "Some changes apply when Kai restarts: Quit from the tray, then start it again."
                        </div>
                    </Show>
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
                "(policy.config_tools) The owner and Kai's own key may then read and change this \
                 configuration by tool call."
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
                        default_label=format!("Default ({})", info.default_state)
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
                <div class="ctl">
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

// ------------------------------------------------------------- Settings

#[component]
fn SettingsPage() -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let filter = RwSignal::new(String::new());
    // Only the keys, so a changed value builds no row again; a section's keys by their table.
    let layout = Memo::new(move |_| {
        ctx.entries.with(|e| {
            model::settings_sections(e, &filter.get())
                .into_iter()
                .map(|(s, list)| {
                    let tables = model::by_table(&s, list)
                        .into_iter()
                        .map(|(t, list)| (t, list.into_iter().map(|e| e.key).collect::<Vec<_>>()))
                        .collect::<Vec<_>>();
                    (s, tables)
                })
                .collect::<Vec<_>>()
        })
    });
    let sections = Memo::new(move |_| layout.get().into_iter().map(|(s, _)| s).collect::<Vec<_>>());
    view! {
        <PageHead title="Settings" sub="Every setting in Kai's configuration file, with the value in effect">
            <input class="input search" type="search" placeholder="Filter by key"
                prop:value=move || filter.get()
                on:input=move |ev| filter.set(event_target_value(&ev))/>
        </PageHead>
        <div class="settings">
            <For each=move || sections.get() key=|s| s.clone() let:section>
                {
                    let s = section.clone();
                    let groups = Memo::new(move |_| {
                        layout.get().into_iter().find(|(n, _)| *n == s).map(|(_, g)| g).unwrap_or_default()
                    });
                    let count = Signal::derive(move || {
                        groups.with(|g| g.iter().map(|(_, k)| k.len()).sum::<usize>()).to_string()
                    });
                    let tables = Memo::new(move |_| groups.get().into_iter().map(|(t, _)| t).collect::<Vec<_>>());
                    view! {
                        <Section title=section.clone() count=count>
                            <For each=move || tables.get() key=|t| t.clone() let:table>
                                <TableRows groups=groups table=table/>
                            </For>
                        </Section>
                    }
                }
            </For>
            <Show when=move || sections.get().is_empty()>
                <p class="empty">"No setting matches."</p>
            </Show>
        </div>
    }
}

/// One table's rows of a section: the section's own keys as they are, a table under it (`[overlay.weather]`)
/// under its name, so a kind's settings read together.
#[component]
fn TableRows(
    groups: Memo<Vec<(Option<String>, Vec<String>)>>,
    table: Option<String>,
) -> impl IntoView {
    let t = table.clone();
    let keys = Memo::new(move |_| {
        groups.with(|g| {
            g.iter()
                .find(|(x, _)| *x == t)
                .map(|(_, k)| k.clone())
                .unwrap_or_default()
        })
    });
    let rows = move || {
        view! {
            <For each=move || keys.get() key=|k| k.clone() let:key>
                <SettingRow key=key/>
            </For>
        }
    };
    match table {
        None => rows().into_any(),
        Some(name) => view! {
            <div class="subsec">
                <div class="subsec-head">{format!("[{name}]")}</div>
                {rows()}
            </div>
        }
        .into_any(),
    }
}

#[component]
fn SettingRow(key: String) -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let note: NoteSignal = RwSignal::new(None);
    let k = key.clone();
    let entry = Memo::new(move |_| {
        ctx.entries
            .with(|e| e.iter().find(|x| x.key == k).cloned())
            .unwrap_or(Entry {
                key: k.clone(),
                value: serde_json::Value::Null,
                set: false,
                choices: Vec::new(),
            })
    });
    let shown = Memo::new(move |_| model::show_value(&entry.get().value));
    let draft = RwSignal::new(shown.get_untracked());
    // A value that came in (our own write, or a reset) replaces what was typed.
    Effect::new(move |_| draft.set(shown.get()));
    let dirty = Signal::derive(move || draft.get() != shown.get());
    let error = Signal::derive(move || match note.get() {
        Some(Note::Err(t)) => Some(t),
        _ => None,
    });
    let editor = Memo::new(move |_| model::editor(&entry.get()));
    let set_key = key.clone();
    let commit = move || {
        let text = draft.get_untracked();
        if text == shown.get_untracked() {
            return;
        }
        let request = if text.trim().is_empty() {
            Request::Unset {
                key: set_key.clone(),
            }
        } else {
            Request::Set {
                key: set_key.clone(),
                value: text,
            }
        };
        run(ctx, request, note);
    };
    let flag_key = key.clone();
    let choice_key = key.clone();
    let reset_key = key.clone();
    view! {
        <Field label=key.clone() dirty=dirty error=error>
            <div class="ctl">
                {move || match editor.get() {
                    Editor::ReadOnly => view! {
                        <span class="readonly" title="Written by pairing">{shown.get()}</span>
                    }.into_any(),
                    Editor::Flag => {
                        let flag_key = flag_key.clone();
                        view! {
                            <input type="checkbox" prop:checked=move || entry.get().value == serde_json::Value::Bool(true)
                                on:change=move |ev| run(ctx, Request::Set {
                                    key: flag_key.clone(),
                                    value: event_target_checked(&ev).to_string(),
                                }, note)/>
                        }.into_any()
                    }
                    Editor::Choice => {
                        let key = choice_key.clone();
                        let words = entry.get_untracked().choices;
                        view! {
                            <select class="select" aria-label=key.clone() on:change=move |ev| {
                                let word = event_target_value(&ev);
                                let key = key.clone();
                                let request = if word.is_empty() {
                                    Request::Unset { key }
                                } else {
                                    Request::Set { key, value: word }
                                };
                                run(ctx, request, note)
                            }>
                                // An unset optional word (`desktop.capture`): Reset sets it back.
                                <option value="" hidden=move || !entry.get().value.is_null()
                                    prop:selected=move || entry.get().value.is_null()>"not set"</option>
                                {words.into_iter().map(|w| {
                                    let is = w.clone();
                                    let value = w.clone();
                                    view! {
                                        <option value=value
                                            prop:selected=move || entry.get().value.as_str() == Some(is.as_str())>
                                            {w}
                                        </option>
                                    }
                                }).collect_view()}
                            </select>
                        }.into_any()
                    }
                    Editor::Text => {
                        let commit = commit.clone();
                        view! {
                            <input class="input" type="text" spellcheck="false"
                                placeholder=move || if entry.get().value.is_null() { "not set" } else { "" }
                                prop:value=move || draft.get()
                                on:input=move |ev| draft.set(event_target_value(&ev))
                                on:change=move |_| commit()
                                on:keydown=move |ev| if ev.key() == "Escape" { draft.set(shown.get_untracked()) }/>
                        }.into_any()
                    }
                }}
                <Show when=move || entry.get().set>
                    <button class="btn sm ghost" title="Remove it from the file: the default applies again"
                        on:click={
                            let reset_key = reset_key.clone();
                            move |_| run(ctx, Request::Unset { key: reset_key.clone() }, note)
                        }>"Reset"</button>
                </Show>
                <span class="chip" class:set=move || entry.get().set>
                    {move || if entry.get().set { "set" } else { "default" }}
                </span>
                <NoteView note=Signal::derive(move || note.get().filter(|n| matches!(n, Note::Ok(_))))/>
            </div>
        </Field>
    }
}
