//! The Eyes run (kai-firefox design §6.6): the user has the store open and asks; Kai answers in
//! text and says what changed. Only reading tools and, for the delivery check the user asks for,
//! a fill and a press; never a screenshot.

use std::time::Duration;

use serde_json::json;

use super::stage::{Outcome, Stage, find, holds, named};

pub async fn run(st: &mut Stage) {
    let r = where_am_i(st).await;
    st.record("Eyes", "where am I (browser_context)", r);
    let r = product(st).await;
    st.record("Eyes", "find a product, read its description and price", r);
    let r = table(st).await;
    st.record("Eyes", "read a comparison table", r);
    let r = error_and_dialog(st).await;
    st.record(
        "Eyes",
        "hear the error a form shows and the dialog that opens",
        r,
    );
}

async fn where_am_i(st: &mut Stage) -> Result<Outcome, String> {
    let t = st.ok("browser_context", json!({})).await?;
    holds("browser_context", &t, &["Northwind Outdoor: lamps"])?;
    Ok(Outcome::Pass)
}

/// "Find the Headlamp Pro: what is it and what does it cost?"
async fn product(st: &mut Stage) -> Result<Outcome, String> {
    let found = st.ok("page_find", json!({"text": "Headlamp Pro"})).await?;
    let heading = find(&found, "heading", |n| n == "Headlamp Pro")?;
    let t = st.ok("element_text", json!({"ref": heading.r})).await?;
    holds(
        "element_text",
        &t,
        &[
            "Headlamp Pro",
            "A headlamp with a red night mode and a tilting beam, 450 lumens.",
            "Price: 59 €",
        ],
    )?;
    Ok(Outcome::Pass)
}

/// "Read me the comparison of the lamps."
async fn table(st: &mut Stage) -> Result<Outcome, String> {
    let found = st.ok("page_find", json!({"text": "Battery"})).await?;
    let cell = find(&found, "", |_| true)?;
    let t = st.ok("table_read", json!({"ref": cell.r})).await?;
    // Each cell with its headers, as a screen reader says it.
    holds(
        "table_read",
        &t,
        &[
            "table \"Lamps compared\", 5 rows, 4 columns",
            "Battery, Headlamp Pro: 12 h",
            "Price, Headlamp Pro: 59 €",
            "Price, Desk Lamp Mini: 19 €",
        ],
    )?;
    Ok(Outcome::Pass)
}

/// "Check delivery to 1011." The page marks the field and opens a dialog; Kai says both.
async fn error_and_dialog(st: &mut Stage) -> Result<Outcome, String> {
    let fields = st.ok("page_fields", json!({})).await?;
    let zip = named(&fields, "textbox", "Postcode")?;
    st.ok("element_fill", json!({"ref": zip.r, "text": "1011"}))
        .await?;
    let controls = st.ok("page_controls", json!({})).await?;
    let check = named(&controls, "button", "Check delivery")?;
    st.ok("element_press", json!({"ref": check.r})).await?;
    // Kai's words first, the page's text last (design §7.2); both caused by Kai's press.
    st.tts
        .heard(
            &[
                "After Kai's action: the page says, at the field “Postcode”: Enter a postcode of \
                 five digits.",
            ],
            Duration::from_secs(15),
        )
        .await?;
    st.tts
        .heard(
            &["After Kai's action: a dialog opened, the page says: Delivery areas"],
            Duration::from_secs(15),
        )
        .await?;
    let ctx = st.ok("browser_context", json!({})).await?;
    holds(
        "browser_context",
        &ctx,
        &["modal dialog open: ", "dialog \"Delivery areas\""],
    )?;
    Ok(Outcome::Pass)
}
