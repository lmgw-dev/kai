//! The Personality page in the window's live check: unpaired, it says so and asks lmgw nothing;
//! paired, the kit's profile editor lists the dev lmgw's profiles, creates one, saves a change and
//! deletes it, every request through the scheme handler with the device key added by the stand-in
//! core (as Kai's core adds it); each step is read back from the dev lmgw itself. What else is
//! under `/lmgw/` stays unreachable, and the key is nowhere in the page.

use serde_json::Value;

use super::{Run, sleep};

const NAME: &str = "Kai live check";
const PERSONA: &str = "You are Kai, a calm desktop assistant.";
const RULE: &str = "One sentence, details on request.";

/// lmgw's profile list, read with the device key outside the page.
async fn listed(r: &Run) -> Vec<Value> {
    let Some(creds) = r.lmgw.creds.clone() else {
        return Vec::new();
    };
    let client = r.lmgw.client.clone();
    let got = r
        .lmgw
        .rt
        .spawn(async move {
            let url = format!("{}/chat/api/profiles", creds.url.trim_end_matches('/'));
            let resp = client
                .get(url)
                .bearer_auth(creds.key.expose())
                .send()
                .await
                .ok()?;
            resp.json::<Value>().await.ok()
        })
        .await
        .ok()
        .flatten();
    got.and_then(|v| v["profiles"].as_array().cloned())
        .unwrap_or_default()
}

async fn ours(r: &Run) -> Option<Value> {
    listed(r).await.into_iter().find(|p| p["name"] == NAME)
}

/// The status a fetch of the page gets for `path` (`blocked` when it was not let out at all).
async fn status(r: &Run, method: &str, path: &str) -> String {
    let js = format!(
        "window.__st = 'pending'; fetch('{path}', {{method: '{method}', \
         body: '{method}' === 'POST' ? '{{}}' : undefined}}).then(x => window.__st = String(x.status)) \
         .catch(() => window.__st = 'blocked'); 'started'"
    );
    r.js(&js).await;
    for _ in 0..40 {
        let s = r.js("window.__st").await;
        if s != "pending" {
            return s;
        }
        sleep(100).await;
    }
    "no answer".into()
}

/// Sets a text control's value as typing would (`input` event).
fn typed(selector: &str, text: &str) -> String {
    format!(
        "(() => {{ const e = document.querySelector('{selector}'); if (!e) return 'no {selector}'; \
         e.value = {text:?}; e.dispatchEvent(new Event('input', {{bubbles: true}})); return 'done'; }})()"
    )
}

/// Clicks the first button under `scope` whose text is `label`.
fn click(scope: &str, label: &str) -> String {
    format!(
        "(() => {{ const b = [...document.querySelectorAll('{scope} button')]\
         .find(b => b.textContent.trim() === '{label}'); if (!b) return 'no button {label}'; \
         if (b.disabled) return 'disabled {label}'; b.click(); return 'done'; }})()"
    )
}

pub(super) async fn drive(r: &mut Run) {
    if r.lmgw.creds.is_none() {
        r.ok(
            "KAI_LIVE_LMGW names the dev lmgw",
            false,
            "scripts/live-check-window.sh starts one",
        );
        return;
    }
    // Leftovers of an earlier run would make the list's count wrong.
    if ours(r).await.is_some() {
        r.ok("the dev lmgw has no profile of an earlier run", false, NAME);
        return;
    }

    // Unpaired: the page says so, mounts no editor, and the proxy refuses.
    r.page("Personality").await;
    let said = r
        .until(
            "(document.querySelector('.unpaired')?.textContent ?? '').includes('not paired')",
            8,
        )
        .await;
    let text = r
        .js("document.querySelector('.unpaired')?.textContent ?? document.body.innerText.slice(0, 300)")
        .await;
    r.ok("unpaired, the Personality page says so", said, text);
    r.ok(
        "and mounts no editor",
        r.js("document.querySelector('.pf-panel') === null").await == "true",
        "",
    );
    let st = status(r, "GET", "/lmgw/chat/api/profiles").await;
    r.ok("unpaired, a request for lmgw is refused", st == "503", st);

    // Paired: the header and the editor with lmgw's list.
    r.lmgw.paired.set(true);
    r.page("Settings").await;
    r.page("Personality").await;
    let up = r
        .until(
            "document.querySelector('.conv') !== null && document.querySelector('.pf-panel') !== null",
            10,
        )
        .await;
    let header = r
        .js("document.querySelector('.conv')?.innerText ?? ''")
        .await;
    r.ok(
        "paired, the header says what the conversation talks with",
        up && header.contains("talks with") && header.contains("Default"),
        header.replace('\n', " | "),
    );
    let want = listed(r).await.len();
    let shown = r
        .until(
            &format!("document.querySelectorAll('.pf-item').length === {want} && document.querySelector('.pf-list .field-err') === null"),
            10,
        )
        .await;
    let names = r
        .js("[...document.querySelectorAll('.pf-item-name')].map(e => e.textContent).join(', ')")
        .await;
    r.ok(
        "the editor lists lmgw's profiles through Kai",
        shown,
        format!("{want} on lmgw; shown: {names}"),
    );
    let models = r.js("document.querySelectorAll('.pf-panel').length").await;
    r.ok("one panel", models == "1", models);

    // Create.
    r.js(&click(".pf-list-head", "New profile")).await;
    r.until("document.querySelector('.pf-editor') !== null", 5)
        .await;
    let a = r.js(&typed(".pf-head input", NAME)).await;
    let b = r.js(&typed(".pf-editor textarea", PERSONA)).await;
    sleep(300).await;
    let c = r.js(&click(".save-bar", "Create profile")).await;
    let created = r
        .until(
            &format!("[...document.querySelectorAll('.pf-item-name')].some(e => e.textContent === '{NAME}')"),
            10,
        )
        .await;
    let stored = ours(r).await;
    r.ok(
        "a new profile is created on lmgw and listed",
        created && stored.as_ref().is_some_and(|p| p["persona"] == PERSONA),
        format!("{a}/{b}/{c}; lmgw: {:?}", stored.as_ref().map(|p| &p["id"])),
    );
    let id = stored.as_ref().and_then(|p| p["id"].as_i64()).unwrap_or(0);

    // Save a change.
    let a = r
        // The length rule: the first three-row text box (the persona's has five).
        .js(&typed(".pf-editor textarea[rows=\"3\"]", RULE))
        .await;
    sleep(300).await;
    let b = r.js(&click(".save-bar", "Save changes")).await;
    let mut saved = false;
    for _ in 0..30 {
        if ours(r).await.is_some_and(|p| p["length_rule"] == RULE) {
            saved = true;
            break;
        }
        sleep(200).await;
    }
    let clean = r
        .until(
            "!(document.querySelector('.save-bar')?.classList.contains('dirty') ?? true)",
            5,
        )
        .await;
    r.ok(
        "a saved change is on lmgw and the editor is clean",
        saved && clean,
        format!("{a}/{b}"),
    );
    // The page from its top (the header, the list, the editor's head), then the editor's foot.
    r.js("document.querySelector('.page-body').scrollTop = 0; 'done'")
        .await;
    sleep(400).await;
    r.shot("personality").await;
    r.js("const b = document.querySelector('.page-body'); b.scrollTop = b.scrollHeight; 'done'")
        .await;
    sleep(400).await;
    r.shot("personality-foot").await;

    // The kit's other requests reach lmgw too (the model catalog; a refusal reads as lmgw's).
    let st = status(r, "GET", "/lmgw/v1/models").await;
    r.ok("the model catalog comes through", st == "200", st);

    // Delete: two clicks (the armed question).
    let a = r.js(&click(".pf-foot .confirm", "Delete")).await;
    sleep(700).await;
    let b = r
        .js("(() => { const b = document.querySelector('.pf-foot .confirm button'); if (!b) return 'no armed'; b.click(); return b.textContent; })()")
        .await;
    let gone = r
        .until(
            &format!("![...document.querySelectorAll('.pf-item-name')].some(e => e.textContent === '{NAME}')"),
            10,
        )
        .await;
    let on_lmgw = ours(r).await;
    r.ok(
        "a deleted profile is gone from lmgw and the list",
        gone && on_lmgw.is_none(),
        format!("{a}; armed: {b}"),
    );

    // Nothing else under the prefix; the key is not in the page.
    for (method, path) in [
        ("GET", format!("/lmgw/chat/api/profiles/{id}")),
        ("GET", "/lmgw/chat/api/threads".to_owned()),
        ("POST", "/lmgw/api/op/key_create".to_owned()),
        ("POST", "/lmgw/v1/chat/completions".to_owned()),
    ] {
        let st = status(r, method, &path).await;
        r.ok(
            &format!("{method} {path} is not forwarded"),
            st == "404",
            st,
        );
    }
    let secret = r
        .lmgw
        .creds
        .as_ref()
        .map(|c| c.key.expose().to_owned())
        .unwrap_or_default();
    let tail = &secret[secret.len().saturating_sub(16)..];
    let seen = r
        .js(&format!(
            "document.documentElement.outerHTML.includes('{tail}') || \
             performance.getEntries().some(e => e.name.includes('{tail}'))"
        ))
        .await;
    r.ok(
        "the device key is nowhere in the page",
        seen == "false",
        seen,
    );
}
