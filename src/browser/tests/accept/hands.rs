//! The Hands run (kai-firefox design §6.6): the user asks, Kai acts in the tab in front of them.
//! Continues where the Eyes run left the store (the delivery dialog open).

use std::time::Duration;

use serde_json::json;

use super::stage::{Outcome, Stage, find, holds, named};

/// The file the order form gets (`element_file`), in the run folder.
const PHOTO: &str = "door.txt";

pub async fn run(st: &mut Stage) {
    let r = close_dialog(st).await;
    st.record("Hands", "close the delivery dialog", r);
    let r = banner(st).await;
    st.record("Hands", "close a cookie banner in a cross-origin frame", r);
    let r = search(st).await;
    st.record("Hands", "search", r);
    let r = third(st).await;
    st.record("Hands", "open the third result", r);
    let r = to_order(st).await;
    st.record(
        "Hands",
        "open the order form: the link \"Order now\" loads a page whose URL holds the guard's \
         word, so the guard asks (WP10b's security review: a link's words stay where it may commit)",
        r,
    );
    let r = fill(st).await;
    st.record(
        "Hands",
        "fill a controlled input, a combobox and a rich editor",
        r,
    );
    let r = widgets(st).await;
    st.record(
        "Hands",
        "set a slider (its output said with the slider's name), choose an option, check a box, \
         attach a file (element_file asked, answered by voice)",
        r,
    );
    let r = submit(st).await;
    st.record("Hands", "submit through the commit guard", r);
}

async fn close_dialog(st: &mut Stage) -> Result<Outcome, String> {
    let controls = st.ok("page_controls", json!({})).await?;
    let close = named(&controls, "button", "Close")?;
    st.ok("element_press", json!({"ref": close.r})).await?;
    let ctx = st.ok("browser_context", json!({})).await?;
    if ctx.contains("modal dialog open") {
        return Err(format!("the dialog is still open:\n{ctx}"));
    }
    Ok(Outcome::Pass)
}

/// "Close the cookie banner." Its button is in a frame of the other origin.
async fn banner(st: &mut Stage) -> Result<Outcome, String> {
    let controls = st.ok("page_controls", json!({})).await?;
    let close = named(&controls, "button", "Close the cookie banner")?;
    st.ok("element_press", json!({"ref": close.r})).await?;
    let found = st.ok("page_find", json!({"text": "uses cookies"})).await?;
    let controls = st.ok("page_controls", json!({})).await?;
    if controls.contains("Close the cookie banner") || !found.contains('0') {
        return Err(format!("the banner is still there:\n{found}\n{controls}"));
    }
    Ok(Outcome::Pass)
}

/// "Search the store for lamps."
async fn search(st: &mut Stage) -> Result<Outcome, String> {
    let fields = st.ok("page_fields", json!({})).await?;
    let q = find(&fields, "searchbox", |n| n == "Search the store")?;
    st.ok("element_fill", json!({"ref": q.r, "text": "lamp"}))
        .await?;
    let controls = st.ok("page_controls", json!({})).await?;
    let go = named(&controls, "button", "Search")?;
    let pressed = st.ok("element_press", json!({"ref": go.r})).await?;
    let ctx = st.ok("browser_context", json!({})).await?;
    holds("browser_context", &ctx, &["Results for lamp"])?;
    let _ = pressed;
    Ok(Outcome::Pass)
}

/// "Open the third result."
async fn third(st: &mut Stage) -> Result<Outcome, String> {
    let links = st.ok("page_links", json!({})).await?;
    let results: Vec<_> = super::stage::lines(&links)
        .into_iter()
        .filter(|l| l.role.starts_with("link") && l.line.contains("/accept/product"))
        .collect();
    let third = results
        .get(2)
        .ok_or_else(|| format!("fewer than three results:\n{links}"))?;
    if third.name != "Headlamp Pro" {
        return Err(format!("the third result is {:?}", third.name));
    }
    st.ok("element_press", json!({"ref": third.r})).await?;
    let ctx = st.ok("browser_context", json!({})).await?;
    holds(
        "browser_context",
        &ctx,
        &["Headlamp Pro: Northwind Outdoor"],
    )?;
    Ok(Outcome::Pass)
}

/// "Order it." The link's name holds the guard's word "order", and so does its URL
/// (`/accept/order.html`): a link that loads a page by GET keeps its words where its URL holds one
/// (kai-firefox WP10b's security review, M4), so the commit guard asks and the owner says yes.
async fn to_order(st: &mut Stage) -> Result<Outcome, String> {
    let links = st.ok("page_links", json!({})).await?;
    let order = named(&links, "link", "Order now")?;
    st.owner.expect(&["element_press"]);
    st.ok("element_press", json!({"ref": order.r})).await?;
    let ctx = st.ok("browser_context", json!({})).await?;
    holds("browser_context", &ctx, &["Your order: Northwind Outdoor"])?;
    Ok(Outcome::Pass)
}

/// The value a field line shows now (`= "…"`), from a fresh `page_fields`.
async fn value_of(st: &mut Stage, role: &str, name: &str) -> Result<String, String> {
    let fields = st.ok("page_fields", json!({})).await?;
    Ok(named(&fields, role, name)?.line)
}

/// "My name is Ada Example, deliver to Bremen, postcode 10115, and leave it at the back door."
async fn fill(st: &mut Stage) -> Result<Outcome, String> {
    let fields = st.ok("page_fields", json!({})).await?;
    let name = named(&fields, "textbox", "Full name")?;
    let city = named(&fields, "combobox", "City")?;
    let zip = named(&fields, "textbox", "Postcode")?;
    let note = named(&fields, "textbox", "Delivery note")?;
    for (r, text) in [
        (name.r, "Ada Example"),
        (city.r, "Bremen"),
        (zip.r, "10115"),
        (note.r, "Please leave it at the back door."),
    ] {
        st.ok("element_fill", json!({"ref": r, "text": text}))
            .await?;
    }
    // The page's own state took each: the controlled input's greeting, the editor's model.
    let greeting = st
        .ok("page_find", json!({"text": "Order for Ada Example"}))
        .await?;
    if greeting.contains("0 matches") || !greeting.contains("Order for Ada Example") {
        return Err(format!(
            "the page's state did not take the name:\n{greeting}"
        ));
    }
    let model = st
        .ok(
            "page_find",
            json!({"text": "Please leave it at the back door"}),
        )
        .await?;
    if !model.contains("Note as the editor keeps it") {
        return Err(format!(
            "the editor's model did not take the note:\n{model}"
        ));
    }
    holds(
        "City",
        &value_of(st, "combobox", "City").await?,
        &["Bremen"],
    )?;
    Ok(Outcome::Pass)
}

/// "Tip three euros, express, gift wrapped, and attach the photo of my door."
async fn widgets(st: &mut Stage) -> Result<Outcome, String> {
    let photo = st.dir.join(PHOTO);
    std::fs::write(&photo, "A green door, number 7.\n").map_err(|e| e.to_string())?;
    let fields = st.ok("page_fields", json!({})).await?;
    let tip = named(&fields, "slider", "Tip for the courier")?;
    let speed = find(&fields, "combobox", |n| n == "Delivery")?;
    let gift = named(&fields, "checkbox", "Gift wrap")?;
    // The contract's role word for a file input (kai-firefox WP10b).
    let file = named(&fields, "file", "Photo of your door")?;
    st.ok("element_fill", json!({"ref": tip.r, "text": "3"}))
        .await?;
    // The slider's <output> is said with the slider's name, not as a bare "3".
    st.tts
        .heard(
            &["page says, at “Tip for the courier”: 3"],
            Duration::from_secs(10),
        )
        .await?;
    st.ok(
        "element_choose",
        json!({"ref": speed.r, "option": "Express"}),
    )
    .await?;
    st.ok("element_check", json!({"ref": gift.r, "on": true}))
        .await?;
    // element_file stays confirmed with the switch on (design §6.4): the owner is asked, and
    // answers by voice (kai-firefox WP10b): Kai speaks the question, the owner says yes.
    st.owner.expect(&["voice:element_file"]);
    st.ears.will_say("Ja.");
    st.ok(
        "element_file",
        json!({"ref": file.r, "path": photo.display().to_string()}),
    )
    .await?;
    st.tts
        .heard(
            &[
                "May Kai attach a file to the file field? Yes or no? The page calls the file \
                 field: Photo of your door. The file is named: door.txt",
            ],
            Duration::from_secs(5),
        )
        .await?;
    let fields = st.ok("page_fields", json!({})).await?;
    holds(
        "the slider",
        &named(&fields, "slider", "Tip for the courier")?.line,
        &["3"],
    )?;
    holds(
        "the select",
        &find(&fields, "combobox", |n| n == "Delivery")?.line,
        &["Express"],
    )?;
    holds(
        "the box",
        &named(&fields, "checkbox", "Gift wrap")?.line,
        &["checked"],
    )?;
    let shown = st.ok("page_find", json!({"text": "door.txt ("})).await?;
    holds("the page's file line", &shown, &["door.txt (24 bytes)"])?;
    Ok(Outcome::Pass)
}

/// "Place the order." The commit guard asks about the button's word and the form's action, then
/// about the POST the page sends; the owner allows both.
async fn submit(st: &mut Stage) -> Result<Outcome, String> {
    let controls = st.ok("page_controls", json!({})).await?;
    let place = named(&controls, "button", "Place order")?;
    st.owner.expect(&[
        "element_press (the commit guard)",
        "element_press (a request the page sends)",
    ]);
    let pressed = st.ok("element_press", json!({"ref": place.r})).await?;
    holds(
        "element_press",
        &pressed,
        &[
            "\"Order received: Northwind Outdoor\"",
            "pressed",
            "/accept/order (main_frame) went: the owner allowed it",
        ],
    )?;
    let log = st.launcher.site_log().await?;
    let post = log
        .iter()
        .rev()
        .find(|e| e["method"] == "POST" && e["path"] == "/accept/order")
        .ok_or_else(|| format!("the site got no POST /accept/order: {log:?}"))?;
    let body = post["body"].as_str().unwrap_or_default();
    for (field, value) in [
        ("item", "headlamp-pro"),
        ("name", "Ada Example"),
        ("city", "Bremen"),
        ("postcode", "10115"),
        ("note", "Please leave it at the back door."),
        ("tip", "3"),
        ("delivery", "Express"),
        ("gift", "on"),
    ] {
        if !multipart_has(body, field, value) {
            return Err(format!("the POST lacks {field}={value:?}:\n{body}"));
        }
    }
    if !body.contains("filename=\"door.txt\"") || !body.contains("A green door, number 7.") {
        return Err(format!("the POST lacks the photo:\n{body}"));
    }
    Ok(Outcome::Pass)
}

/// Whether the multipart `body` has the field `name` with `value`.
fn multipart_has(body: &str, name: &str, value: &str) -> bool {
    body.split("Content-Disposition: form-data; ")
        .filter_map(|part| part.strip_prefix(&format!("name=\"{name}\"\r\n\r\n")))
        .any(|rest| rest.starts_with(&format!("{value}\r\n")))
}
