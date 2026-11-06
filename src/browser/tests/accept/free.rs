//! The hands-free run (kai-firefox design §6.4, WP10b and its security review): a user who never
//! clicks or types in the tab has Kai go elsewhere and act there. With the hands-and-eyes switch
//! on, a navigation Kai makes in a turn the owner started (Kai's own voice session here) is the
//! user's own act for the focus rule where it landed where it was meant to go, so Kai acts unasked
//! there: on another site a link Kai pressed took the tab to, back on the user's store after
//! `tab_go`, and in a tab Kai opened. A page that navigates itself after Kai's scroll is not the
//! owner's move: acting where it went is asked. A caller whose turn is not the owner's (lmgw's own
//! run) is asked on a site its `tab_go` took the tab to. Runs after the Hands run, on its "Order
//! received" page. The network gate's question about the partner shop (a site the profile has
//! seen only through Kai) is answered by voice, and at the dialog after the scroll.

use std::time::Duration;

use serde_json::json;

use super::stage::{Answer, Outcome, Stage, find, holds, named};
use crate::host::policy::Caller;

pub async fn run(st: &mut Stage) {
    let r = pressed_away(st).await;
    st.record(
        "Free",
        "press a link to another site (\"visit the partner shop\"; the network gate's question \
         answered by voice), then act there unasked",
        r,
    );
    let r = went_back(st).await;
    st.record(
        "Free",
        "tab_go back to the user's own store (\"go back to the store\"), then act there unasked",
        r,
    );
    let r = opened(st).await;
    st.record(
        "Free",
        "a tab Kai opened (\"open the store in a new tab\"), then act in it unasked",
        r,
    );
    let r = scrolled_away(st).await;
    st.record(
        "Free",
        "a page that navigates itself after Kai's scroll (\"show me the deals\", then a scroll): \
         acting where the page went is asked under the switch",
        r,
    );
    let r = not_the_owners(st).await;
    st.caller = Caller::Own;
    st.record(
        "Free",
        "lmgw's own run, not the owner's turn: acting on a site its tab_go took the tab to is asked",
        r,
    );
}

/// The search box of the store the tab shows, filled: what Kai answered.
async fn search_there(st: &mut Stage) -> Result<Answer, String> {
    let fields = st.ok("page_fields", json!({})).await?;
    let q = find(&fields, "searchbox", |n| n == "Search the store")?;
    Ok(st
        .call("element_fill", json!({"ref": q.r, "text": "lamp"}))
        .await)
}

/// Kai acted unasked (the step's record fails on a question it did not expect).
fn unasked(what: &str, a: &Answer) -> Result<Outcome, String> {
    if a.error {
        return Err(format!("{what} failed: {}", a.text));
    }
    Ok(Outcome::Pass)
}

/// A link to another site: the network gate asks about the GET to a site the user never visited
/// (here by voice); the press is the owner's act, so the tab's site is the partner shop now.
async fn pressed_away(st: &mut Stage) -> Result<Outcome, String> {
    let links = st.ok("page_links", json!({})).await?;
    let partner = named(&links, "link", "Visit our partner shop")?;
    st.owner
        .expect(&["voice:element_press (a request the page sends)"]);
    st.ears.will_say("Yes.");
    let pressed = st.ok("element_press", json!({"ref": partner.r})).await?;
    if !pressed.contains(&st.other) {
        return Err(format!(
            "the press did not go to the partner shop:\n{pressed}"
        ));
    }
    let heard = st
        .tts
        .heard(
            &[
                "After Kai's action the page goes to a site you have never visited",
                "May it go there? Yes or no? The page sends it to: localhost:",
            ],
            Duration::from_secs(5),
        )
        .await?;
    holds(
        "the spoken question",
        &heard.text,
        &["the commit guard holds it"],
    )?;
    let a = search_there(st).await?;
    unasked("element_fill on the partner shop", &a)
}

/// Back to the store the user had open, by tab_go: the owner's own navigation.
async fn went_back(st: &mut Stage) -> Result<Outcome, String> {
    let url = st.url("accept/store.html");
    st.ok("tab_go", json!({"url": url})).await?;
    let a = search_there(st).await?;
    unasked("element_fill on the user's own store", &a)
}

async fn opened(st: &mut Stage) -> Result<Outcome, String> {
    let url = st.url("accept/store.html");
    st.ok("tab_open", json!({"url": url})).await?;
    let a = search_there(st).await?;
    unasked("element_fill in the tab Kai opened", &a)
}

/// The deals page goes to the other origin's store on its first scroll (kai-firefox's
/// `accept/deals.html`): the owner's `tab_go` there is theirs, the scroll is no navigation of
/// theirs, so filling the search where the page went is asked (WP10b's security review, H1). The
/// network gate holds the page's GET to the other origin, which the profile has seen only through
/// Kai; the owner lets it go.
async fn scrolled_away(st: &mut Stage) -> Result<Outcome, String> {
    let url = st.url("accept/deals.html");
    st.ok("tab_go", json!({"url": url})).await?;
    st.owner
        .expect(&["page_scroll (a request the page sends)", "element_fill"]);
    st.ok("page_scroll", json!({"to": "down"})).await?;
    // The page's own navigation commits after the scroll's answer at the latest.
    let end = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let ctx = st.ok("browser_context", json!({})).await?;
        if ctx.contains(&st.other) && ctx.contains("Northwind Outdoor: lamps") {
            break;
        }
        if tokio::time::Instant::now() > end {
            return Err(format!("the page did not go to the other store:\n{ctx}"));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let a = search_there(st).await?;
    if a.error {
        return Err(format!("element_fill failed: {}", a.text));
    }
    Ok(Outcome::Pass)
}

/// lmgw's own run (a trusted caller, the switch lets it act, but its turn is not the owner's):
/// its tab_go to the partner shop, which the user has seen only through Kai, is asked, and so is
/// acting there, since the tab's site is still the owner's store.
async fn not_the_owners(st: &mut Stage) -> Result<Outcome, String> {
    st.caller = Caller::Gateway;
    st.owner.expect(&["tab_go", "element_fill"]);
    let url = format!("{}/accept/store.html", st.other);
    st.ok("tab_go", json!({"url": url})).await?;
    let a = search_there(st).await?;
    unasked("element_fill on the partner shop, allowed by the owner", &a)
}
