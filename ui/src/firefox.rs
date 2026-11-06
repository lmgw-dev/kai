//! The Firefox section's extension block (kai-firefox design §10, Kai's side): whether Kai for
//! Firefox is connected to Kai right now, and "Install Kai for Firefox", which has Kai open the
//! signed extension's link in the browser. The link is the build's and never reaches this page
//! (it may carry a token): the page asks Kai whether there is one (`GET /api/browser`) and has it
//! opened (`POST /api/browser/install`). A build without a link says how to install by hand. The
//! connection is read again whenever Kai says the links changed (`kai-browser`).

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::app::{Note, NoteSignal, NoteView};
use crate::model;
use crate::wire;

#[component]
pub(crate) fn FirefoxExtension() -> impl IntoView {
    // `None` until Kai answered the first look.
    let facts = RwSignal::new(None::<wire::Browser>);
    let note: NoteSignal = RwSignal::new(None);
    let opening = RwSignal::new(false);
    let read = move || {
        spawn_local(async move {
            match api::browser(false).await {
                Ok(b) => facts.set(Some(b)),
                Err(why) => note.set(Some(Note::Err(why))),
            }
        })
    };
    read();
    let changed = window_event_listener_untyped(wire::BROWSER_EVENT, move |_| read());
    on_cleanup(move || changed.remove());
    let linked = Signal::derive(move || facts.with(|f| f.as_ref().map(|b| b.linked)));
    let install = Signal::derive(move || facts.with(|f| f.as_ref().is_some_and(|b| b.install)));
    let help = Signal::derive(move || {
        facts.with(|f| {
            f.as_ref()
                .map(|b| model::firefox_install_help(b.install, b.problem.as_deref()))
                .unwrap_or_default()
        })
    });
    let open = move |_| {
        opening.set(true);
        note.set(None);
        spawn_local(async move {
            match api::browser(true).await {
                Ok(b) => {
                    note.set(Some(Note::Ok(
                        model::firefox_opened_note(b.opened).to_owned(),
                    )));
                    facts.set(Some(b));
                }
                Err(why) => note.set(Some(Note::Err(why))),
            }
            opening.set(false);
        })
    };
    view! {
        <div class="firefox-ext">
            <p class="firefox-status" class:connected=move || linked.get().is_some_and(|n| n > 0)>
                {move || model::firefox_link_line(linked.get())}
            </p>
            <Show when=move || install.get()>
                <div class="ctl-main">
                    <button type="button" class="btn primary" prop:disabled=move || opening.get()
                        on:click=open>
                        "Install Kai for Firefox"
                    </button>
                </div>
            </Show>
            <p class="hint">{move || help.get()}</p>
            <NoteView note=note/>
        </div>
    }
}
