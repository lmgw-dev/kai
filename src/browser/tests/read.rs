//! The reading tools, the hints and `element_point` (kai-firefox design §5.3, §5.4) through the
//! host's dispatch against a fake Kai for Firefox that answers the contract's shapes: each
//! handler's lines, the operation it sends, refs resolved to the link that gave them out (stale,
//! unknown, from a restarted extension, from a profile that closed, two profiles in one number
//! space), the site rules on a ref's tab and frame, on an answer's page and frames and on a
//! failure's tab, the masking words, the frames that could not be read, a large answer said whole,
//! and a PDF read with poppler's `pdftotext` (bounded, sandboxed, its unreadable lines left out).
//!
//! The PDF tests need poppler's `pdftotext` (`dnf install poppler-utils`) and fail without it;
//! `KAI_TEST_NO_PDFTOTEXT=1` skips the ones that run it, saying so.

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};

use super::fake::{BUILT, Firefox, Host, Model, Twist, WP4A, WP4B, all_ops, shop, tab};
use crate::browser::settings::{BrowserConfig, Sites};
use crate::browser::tools::pdf;
use crate::host::settings::PolicyConfig;

fn switch_on() -> BrowserConfig {
    BrowserConfig {
        hands_and_eyes: true,
        ..Default::default()
    }
}

fn page(id: i64, title: &str, url: &str) -> Value {
    json!({"id": id, "title": title, "url": url})
}

fn checkout() -> Value {
    page(12, "Checkout", "https://shop.example/cart")
}

const HEAD: &str = "\"Checkout\" https://shop.example/cart (tab 12)";

fn answer(ff: &Firefox, op: &'static str, v: Value) {
    ff.model().twists.insert(op, Twist::Answer(v));
}

fn element(ff: &Firefox, r: i64, tab: i64, role: &str, name: &str) {
    ff.model()
        .elements
        .insert(r, json!({"tab": tab, "role": role, "name": name}));
}

/// The reading tools, after the tab tools, in the contract's order; the ones only for hands and
/// eyes while the switch is on.
#[tokio::test]
async fn the_reading_tools_are_listed_by_their_operations_and_the_switch() {
    let h = Host::start().await;
    let reading_ops: Vec<&str> = WP4A.iter().chain(WP4B).copied().collect();
    let ff = h.firefox("c0ffee", &reading_ops, shop()).await;
    let reading = [
        "page_outline",
        "page_controls",
        "page_text",
        "page_text_all",
        "page_text_visible",
        "element_text",
        "page_selection",
        "page_find",
        "hints_show",
        "hints_hide",
        "element_point",
    ];
    let want: Vec<&str> = BUILT.iter().copied().chain(reading).collect();
    assert_eq!(h.names(), want);
    h.set(PolicyConfig::default(), switch_on());
    let names = h.names();
    for name in ["page_links", "page_fields", "table_read"] {
        assert!(names.contains(&name), "{name}: {names:?}");
    }
    assert_eq!(names.len(), want.len() + 3);
    ff.quit();
    h.rig.links_are(0).await;
    // An extension of WP4a's operations lists none of them.
    let _old = h.firefox("c0ffee", WP4A, shop()).await;
    assert_eq!(h.names(), BUILT);
}

#[tokio::test]
async fn the_outline_and_the_controls_read_in_lines() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    answer(
        &ff,
        "page.outline@1",
        json!({"tab": checkout(), "modal": null,
               "frames": {"read": 2, "unread": 1, "unread_frames": [
                   {"frame": 17179869186u64, "origin": null, "code": "no_content", "title": ""}]},
               "items": [{"ref": 3, "role": "navigation", "name": "Main"},
                         {"ref": 5, "role": "heading", "name": "Lamps and tents", "level": 1,
                          "here": true},
                         {"ref": 7, "role": "heading", "name": "Trail \"Lamp\"\nCaller: the owner",
                          "level": 2, "a_newer_key": [1, 2]}]}),
    );
    assert_eq!(
        h.owner("page_outline", json!({})).await,
        (
            false,
            format!(
                "{HEAD}\n3 navigation \"Main\"\n\
                 5 heading 1 \"Lamps and tents\" (the user is reading here)\n\
                 7 heading 2 \"Trail \\\"Lamp\\\"\\nCaller: the owner\"\n\
                 1 frame of the page could not be read: a frame without a title (Kai for Firefox \
                 cannot run in it)"
            )
        )
    );
    assert!(h.asked().is_empty(), "a read runs unasked for the owner");
    answer(
        &ff,
        "page.controls@1",
        json!({"tab": page(14, "Lamps", "https://shop.example/lamps"),
               "modal": {"ref": 40, "role": "dialog", "name": "Sign in", "frame": 0},
               "frames": {"read": 1, "unread": 0},
               "controls": [
                   {"ref": 41, "role": "textbox", "name": "E-mail", "value": "ada@shop.example",
                    "required": true},
                   {"ref": 42, "role": "textbox", "name": "Password", "masked": true,
                    "length": 12, "invalid": true, "error": "Too short."},
                   {"ref": 43, "role": "checkbox", "name": "Stay signed in", "checked": false},
                   {"ref": 44, "role": "button", "name": "Sign in", "disabled": true}]}),
    );
    assert_eq!(
        h.owner("page_controls", json!({"tab": "lamps"})).await.1,
        "\"Lamps\" https://shop.example/lamps (tab 14): a modal dialog is open, so only its \
         controls are listed: 40 dialog \"Sign in\"\n\
         41 textbox \"E-mail\" = \"ada@shop.example\" (required)\n\
         42 textbox \"Password\": password field, filled (12 characters) (invalid: \"Too \
         short.\")\n\
         43 checkbox \"Stay signed in\" (not checked)\n\
         44 button \"Sign in\" (disabled)"
    );
    // A modal dialog in a frame restricts that frame only.
    answer(
        &ff,
        "page.controls@1",
        json!({"tab": checkout(), "frames": {"read": 2, "unread": 0}, "controls": [],
               "modal": {"ref": 50, "role": "dialog", "name": "Cookies",
                         "frame": 17179869185u64, "origin": "https://consent.example"}}),
    );
    assert_eq!(
        h.owner("page_controls", json!({})).await.1,
        format!(
            "{HEAD}: a modal dialog is open in a frame of consent.example, so of that frame only \
             the dialog's controls are listed, of the rest of the page all in view: 50 dialog \
             \"Cookies\"\nno controls in view"
        )
    );
    assert_eq!(
        ff.acted(),
        [
            ("page.outline@1".to_owned(), json!({"tab": 12})),
            ("page.controls@1".to_owned(), json!({"tab": 14})),
            ("page.controls@1".to_owned(), json!({"tab": 12})),
        ]
    );
}

#[tokio::test]
async fn links_and_fields_list_the_whole_page_with_the_switch_on() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    answer(
        &ff,
        "page.links@1",
        json!({"tab": checkout(), "modal": null, "frames": {"read": 1, "unread": 0},
               "links": [{"ref": 3, "role": "link", "name": "Home",
                          "url": "https://shop.example/", "in_view": true},
                         {"ref": 4, "role": "link", "name": "Imprint",
                          "url": "https://shop.example/imprint", "in_view": false},
                         {"ref": 6, "role": "link", "name": "Menu", "url": null,
                          "in_view": false}]}),
    );
    assert_eq!(
        h.owner("page_links", json!({})).await.1,
        format!(
            "{HEAD}\n3 links, 1 of them in view\n\
             3 link \"Home\" https://shop.example/ (in view)\n\
             4 link \"Imprint\" https://shop.example/imprint\n\
             6 link \"Menu\""
        )
    );
    answer(
        &ff,
        "page.fields@1",
        json!({"tab": checkout(), "frames": {"read": 1, "unread": 0},
               "modal": {"ref": 40, "role": "dialog", "name": "Cookies"},
               "fields": [{"ref": 48, "role": "radio", "name": "Express", "checked": true,
                           "group": "Shipping", "in_view": true, "inert": true},
                          {"ref": 49, "role": "textbox", "name": "Card number", "masked": true,
                           "length": 19, "in_view": false, "inert": true}]}),
    );
    assert_eq!(
        h.owner("page_fields", json!({})).await.1,
        format!(
            "{HEAD}: a modal dialog is open, so the page outside it cannot be used now: 40 \
             dialog \"Cookies\"\n2 form fields, 1 of them in view\n\
             48 radio \"Express\" (checked, in \"Shipping\", in view, cannot be used now)\n\
             49 textbox \"Card number\": password field, filled (19 characters) (cannot be \
             used now)"
        )
    );
}

#[tokio::test]
async fn page_text_says_the_article_and_what_it_left_out() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    answer(
        &ff,
        "page.text@1",
        json!({"tab": checkout(), "article": true, "title": "Three nights in the hills",
               "byline": "By Ada Example", "lang": "en",
               "blocks": [{"text": "Three nights in the hills", "heading": 1, "ref": 5},
                          {"text": "We walked.\nThen we slept."},
                          {"text": "Tent", "item": true},
                          {"text": "Next day", "heading": 2}],
               "left_out": {"navigation": 1, "complementary": 3, "comments": 1}}),
    );
    assert_eq!(
        h.owner("page_text", json!({})).await.1,
        format!(
            "{HEAD}\n\
             the article \"Three nights in the hills\", byline \"By Ada Example\", language en:\n\
             navigation, 3 asides and comments left out; page_text_all reads everything\n\
             # Three nights in the hills (ref 5)\n\
             We walked.\nThen we slept.\n\
             - Tent\n\
             ## Next day"
        )
    );
    answer(
        &ff,
        "page.text@1",
        json!({"tab": checkout(), "article": false,
               "why": "the article Readability found has 187 characters, fewer than 250",
               "frames": {"read": 2, "unread": 1, "unread_frames": [
                   {"frame": 17179869186u64, "origin": "https://pay.example",
                    "code": "content_timeout", "title": "Payment"}]},
               "blocks": [{"text": "Your cart"},
                          {"frame": 17179869186u64, "origin": "https://pay.example",
                           "unread": "content_timeout", "title": "Payment"},
                          {"text": "Total\t60.00 €"}]}),
    );
    assert_eq!(
        h.owner("page_text", json!({})).await.1,
        format!(
            "{HEAD}\n\
             no article (the article Readability found has 187 characters, fewer than 250), so \
             all of the page's text:\n\
             Your cart\n\
             (pay.example \"Payment\" could not be read here: it did not answer within Kai for \
             Firefox's setting content_ms)\n\
             Total\t60.00 €\n\
             1 frame of the page could not be read: pay.example \"Payment\" (it did not answer \
             within Kai for Firefox's setting content_ms)"
        )
    );
}

/// ruling 9: a page's text comes whole, however long.
#[tokio::test]
async fn the_page_text_comes_whole_and_names_the_frames_it_could_not_read() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    let n = 30_000;
    let blocks: Vec<Value> = (1..=n)
        .map(|i| json!({"text": format!("Paragraph {i} of a long page, with enough words.")}))
        .chain([json!({"frame": 9, "origin": null, "unread": "no_content", "title": null})])
        .collect();
    answer(
        &ff,
        "page.text_all@1",
        json!({"tab": checkout(), "blocks": blocks,
               "frames": {"read": 1, "unread": 2, "unread_frames": [
                   {"frame": 9, "origin": null, "code": "no_content", "title": null}]}}),
    );
    let (err, text) = h.owner("page_text_all", json!({})).await;
    assert!(!err);
    assert!(text.len() > 1_400_000, "{}", text.len());
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1 + n + 2);
    assert_eq!(lines[0], HEAD);
    assert_eq!(
        lines[n],
        format!("Paragraph {n} of a long page, with enough words.")
    );
    assert_eq!(
        lines[n + 1],
        "(a frame without a title could not be read here: Kai for Firefox cannot run in it)"
    );
    assert_eq!(
        lines[n + 2],
        "2 frames of the page could not be read: a frame without a title (Kai for Firefox \
         cannot run in it); 1 more Kai for Firefox does not name"
    );
    answer(
        &ff,
        "page.text_visible@1",
        json!({"tab": checkout(), "blocks": [], "frames": {"read": 1, "unread": 0}}),
    );
    assert_eq!(
        h.owner("page_text_visible", json!({})).await.1,
        format!("{HEAD}\nno text in view")
    );
}

#[tokio::test]
async fn selection_and_find_say_the_page_text_whole() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    answer(
        &ff,
        "page.selection@1",
        json!({"tab": checkout(), "frames": {"read": 2, "unread": 0},
               "selection": {"text": "Trail Lamp\nA lamp for the trail.", "length": 32,
                             "masked": false, "field": null, "frame": 17179869185u64,
                             "origin": "https://consent.example"}}),
    );
    assert_eq!(
        h.owner("page_selection", json!({})).await.1,
        format!(
            "{HEAD}\nselection: 32 characters in a frame of consent.example:\nTrail Lamp\nA lamp \
             for the trail."
        )
    );
    answer(
        &ff,
        "page.selection@1",
        json!({"tab": checkout(), "frames": {"read": 1, "unread": 0},
               "selection": {"text": null, "length": 12, "masked": true,
                             "field": {"ref": 42, "role": "textbox", "name": "Password"}}}),
    );
    assert_eq!(
        h.owner("page_selection", json!({})).await.1,
        format!(
            "{HEAD}\nselection in 42 textbox \"Password\": password field, 12 characters \
             selected, not shown"
        )
    );
    answer(
        &ff,
        "page.selection@1",
        json!({"tab": checkout(), "frames": {"read": 1, "unread": 0},
               "selection": {"text": null, "length": null, "masked": true, "field": null}}),
    );
    assert_eq!(
        h.owner("page_selection", json!({})).await.1,
        format!("{HEAD}\na password field has the focus; its selection is not shown")
    );
    answer(
        &ff,
        "page.selection@1",
        json!({"tab": checkout(), "selection": null, "frames": {"read": 1, "unread": 0}}),
    );
    assert_eq!(
        h.owner("page_selection", json!({})).await.1,
        format!("{HEAD}\nnothing is selected")
    );
    answer(
        &ff,
        "page.find@1",
        json!({"tab": checkout(), "text": "cart", "count": 3, "highlighted": true,
               "matches": [
                   {"text": "cart", "sentence": "Add Trail Lamp to cart",
                    "element": {"ref": 214, "role": "button", "name": "Add Trail Lamp to cart"}},
                   {"text": "Cart", "sentence": "shop.example uses cookies to remember your cart.",
                    "element": {"ref": 215, "role": "paragraph", "name": ""},
                    "frame": 17179869185u64, "origin": "https://consent.example"},
                   {"text": "cart", "element": null, "sentence": null}]}),
    );
    assert_eq!(
        h.owner("page_find", json!({"text": "cart"})).await.1,
        format!(
            "{HEAD}\n3 matches for \"cart\", highlighted in the page:\n\
             214 button \"Add Trail Lamp to cart\": \"Add Trail Lamp to cart\"\n\
             215 paragraph (in a frame of consent.example): \"shop.example uses cookies to \
             remember your cart.\"\n\
             a match Kai for Firefox could not place in the page: \"cart\""
        )
    );
    assert_eq!(
        h.owner("page_find", json!({"text": "  "})).await,
        (
            true,
            "page_find needs the text to find; it was empty".to_owned()
        )
    );
    assert_eq!(
        ff.acted().last().unwrap(),
        &("page.find@1".to_owned(), json!({"text": "cart", "tab": 12}))
    );
}

#[tokio::test]
async fn hints_show_lists_the_controls_and_hide_says_what_it_removed() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    answer(
        &ff,
        "hints.show@1",
        json!({"tab": checkout(), "modal": null, "frames": {"read": 1, "unread": 0},
               "controls": [{"ref": 214, "role": "button", "name": "Buy"},
                            {"ref": 215, "role": "link", "name": "Help",
                             "url": "https://shop.example/help"}]}),
    );
    assert_eq!(
        h.owner("hints_show", json!({})).await.1,
        format!(
            "{HEAD}\nhints on 2 controls in view\n\
             214 button \"Buy\"\n215 link \"Help\" https://shop.example/help"
        )
    );
    assert!(h.asked().is_empty(), "the switch lets the owner through");
    answer(
        &ff,
        "hints.hide@1",
        json!({"tab": checkout(), "shown": true, "removed": 37}),
    );
    assert_eq!(
        h.owner("hints_hide", json!({})).await.1,
        format!("{HEAD}: hints hidden, 37 labels removed")
    );
    answer(
        &ff,
        "hints.hide@1",
        json!({"tab": checkout(), "shown": false, "removed": 0}),
    );
    assert_eq!(
        h.owner("hints_hide", json!({})).await.1,
        format!("{HEAD}: no hints were shown")
    );
}

#[tokio::test]
async fn a_ref_is_read_in_the_tab_it_names() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    element(&ff, 7, 14, "heading", "Trail Lamp");
    answer(
        &ff,
        "element.text@1",
        json!({"ref": 7, "tab": page(14, "Lamps", "https://shop.example/lamps"),
               "role": "heading", "name": "Trail Lamp", "section": true,
               "blocks": [{"text": "Trail Lamp", "heading": 2, "ref": 7},
                          {"text": "A lamp for the trail."},
                          {"text": "Code: password field, filled (6 characters)"}]}),
    );
    assert_eq!(
        h.owner("element_text", json!({"ref": 7})).await,
        (
            false,
            "\"Lamps\" https://shop.example/lamps (tab 14)\n\
             7 heading \"Trail Lamp\", its section:\n\
             ## Trail Lamp (ref 7)\n\
             A lamp for the trail.\n\
             Code: password field, filled (6 characters)"
                .to_owned()
        )
    );
    element(&ff, 9, 12, "paragraph", "");
    answer(
        &ff,
        "element.text@1",
        json!({"ref": 9, "tab": checkout(), "role": "paragraph", "name": "", "section": false,
               "hidden": true, "blocks": []}),
    );
    assert_eq!(
        h.owner("element_text", json!({"ref": 9})).await.1,
        format!("{HEAD}\n9 paragraph is hidden, and nothing of it is shown")
    );
    // element_point: the ring for browser.point_ms; confirmed until the switch is on, the dialog
    // naming the element and its tab.
    element(&ff, 214, 12, "button", "Buy");
    answer(
        &ff,
        "element.point@1",
        json!({"ref": 214, "tab": checkout(), "role": "button", "name": "Buy", "ms": 3000,
               "scrolled": true}),
    );
    assert_eq!(
        h.owner("element_point", json!({"ref": 214})).await.1,
        format!("{HEAD}: scrolled 214 button \"Buy\" into view and ringed it for 3000 ms")
    );
    let asked = h.asked();
    assert_eq!(asked.len(), 1);
    assert!(
        asked[0].lines.contains(
            &"shows on: 214 button \"Buy\" in 12 \"Checkout\" https://shop.example/cart".to_owned()
        ),
        "{:?}",
        asked[0].lines
    );
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            point_ms: 1500,
            ..switch_on()
        },
    );
    h.owner("element_point", json!({"ref": 214})).await;
    assert!(h.asked().is_empty());
    // table_read, with the switch on.
    element(&ff, 40, 12, "table", "Tents compared");
    answer(
        &ff,
        "table.read@1",
        json!({"ref": 40, "tab": checkout(), "role": "table", "name": "Tents compared",
               "caption": "Tents compared", "rows": 2, "columns": 2,
               "cells": [{"row": 1, "column": 1, "text": "Tent", "header": "column",
                          "column_headers": [], "row_headers": []},
                         {"row": 1, "column": 2, "text": "Weight", "header": "column",
                          "column_headers": [], "row_headers": []},
                         {"row": 2, "column": 1, "text": "Valley Tent", "header": "row",
                          "column_headers": ["Tent"], "row_headers": []},
                         {"row": 2, "column": 2, "text": "2.9 kg",
                          "column_headers": ["Weight"], "row_headers": ["Valley Tent"]}]}),
    );
    assert_eq!(
        h.owner("table_read", json!({"ref": 40})).await.1,
        format!(
            "{HEAD}\n40 table \"Tents compared\", 2 rows, 2 columns\n\
             r1 c1 column header: Tent\n\
             r1 c2 column header: Weight\n\
             r2 c1 row header: Valley Tent\n\
             r2 c2 Weight, Valley Tent: 2.9 kg"
        )
    );
    assert_eq!(
        ff.acted(),
        [
            ("element.describe@1".to_owned(), json!({"ref": 7})),
            ("element.text@1".to_owned(), json!({"ref": 7})),
            ("element.describe@1".to_owned(), json!({"ref": 9})),
            ("element.text@1".to_owned(), json!({"ref": 9})),
            ("element.describe@1".to_owned(), json!({"ref": 214})),
            (
                "element.point@1".to_owned(),
                json!({"ref": 214, "ms": 3000})
            ),
            ("element.describe@1".to_owned(), json!({"ref": 214})),
            (
                "element.point@1".to_owned(),
                json!({"ref": 214, "ms": 1500})
            ),
            ("element.describe@1".to_owned(), json!({"ref": 40})),
            ("table.read@1".to_owned(), json!({"ref": 40})),
        ]
    );
}

#[tokio::test]
async fn a_stale_or_unknown_ref_says_why_and_what_to_call() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    assert_eq!(
        h.owner("element_text", json!({"ref": 999})).await,
        (
            true,
            "ref 999 was never given out in this Firefox run; refs come from the page tools: \
             call page_controls, page_outline or hints_show for the page's refs now"
                .to_owned()
        )
    );
    ff.model().elements.insert(
        8,
        json!({"error": ["stale_document",
                         "ref 8 is from the page before the last navigation in tab 12 ('Checkout')"]}),
    );
    assert_eq!(
        h.owner("element_text", json!({"ref": 8})).await.1,
        "ref 8 is from the page before the last navigation in tab 12 ('Checkout'); call \
         page_controls or page_outline for the refs of the page it shows now"
    );
    ff.model().elements.insert(
        5,
        json!({"error": ["stale_tab", "ref 5 was in tab 7, which is closed"]}),
    );
    assert_eq!(
        h.owner("element_text", json!({"ref": 5})).await.1,
        "ref 5 was in tab 7, which is closed; tab_list lists the open tabs"
    );
    // Described, then gone when it is read.
    element(&ff, 6, 12, "button", "Buy");
    ff.model().twists.insert(
        "element.text@1",
        Twist::Fail(
            "stale_element",
            "ref 6's element is no longer in the page of tab 12 ('Checkout') (the page removed it)",
        ),
    );
    assert_eq!(
        h.owner("element_text", json!({"ref": 6})).await.1,
        "ref 6's element is no longer in the page of tab 12 ('Checkout') (the page removed it); \
         call page_controls or page_outline for the page's refs now"
    );
    ff.model().twists.insert(
        "table.read@1",
        Twist::Fail(
            "not_a_table",
            "ref 6 (button 'Buy') is no table and is in none",
        ),
    );
    h.set(PolicyConfig::default(), switch_on());
    assert_eq!(
        h.owner("table_read", json!({"ref": 6})).await.1,
        "ref 6 (button 'Buy') is no table and is in none; table_read takes a table, or an \
         element in one"
    );
    // A ref Kai saw before the extension linked again (it or Firefox restarted) names nothing
    // now: Kai says so and sends nothing.
    answer(
        &ff,
        "page.outline@1",
        json!({"tab": checkout(), "items": [{"ref": 214, "role": "heading", "name": "Cart",
                                             "level": 1}],
               "frames": {"read": 1, "unread": 0}}),
    );
    h.owner("page_outline", json!({})).await;
    let tabs = ff.model().tabs.clone();
    ff.quit();
    h.rig.links_are(0).await;
    let ff = h.firefox("c0ffee", &all_ops(), Model::with(tabs)).await;
    element(&ff, 214, 12, "heading", "Another");
    let (err, text) = h.owner("element_text", json!({"ref": 214})).await;
    assert!(err);
    assert!(
        text.starts_with("ref 214 names no element Kai can reach now"),
        "{text}"
    );
    assert!(ff.acted().is_empty(), "nothing was sent");
}

#[tokio::test]
async fn site_rules_reach_a_refs_tab_and_an_answers_page() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            sites: Sites {
                none: vec!["mail.example".into()],
                read: vec!["shop.example".into()],
                act: Vec::new(),
            },
            ..switch_on()
        },
    );
    element(&ff, 9, 3, "paragraph", "Dear Ada");
    assert_eq!(
        h.owner("element_text", json!({"ref": 9})).await,
        (
            true,
            "ref 9 is in tab 3, on a site Kai does not reach (Kai's site rules); element_text \
             did not run"
                .to_owned()
        )
    );
    assert_eq!(
        h.owner("element_point", json!({"ref": 9})).await.1,
        "ref 9 is in tab 3, on a site Kai does not reach (Kai's site rules); element_point did \
         not run"
    );
    assert_eq!(
        h.owner("page_text", json!({"tab": "3"})).await.1,
        "tab 3 is on a site Kai does not reach (Kai's site rules); page_text did not run"
    );
    assert_eq!(
        ff.acted(),
        [
            ("element.describe@1".to_owned(), json!({"ref": 9})),
            ("element.describe@1".to_owned(), json!({"ref": 9})),
        ],
        "only the describes: nothing was read"
    );
    // A read site: reading, the hints and the ring are not acting on the page.
    element(&ff, 214, 12, "button", "Buy");
    answer(
        &ff,
        "element.point@1",
        json!({"ref": 214, "tab": checkout(), "role": "button", "name": "Buy", "ms": 3000,
               "scrolled": false}),
    );
    assert_eq!(
        h.owner("element_point", json!({"ref": 214})).await.1,
        format!("{HEAD}: ringed 214 button \"Buy\" for 3000 ms")
    );
    answer(
        &ff,
        "hints.show@1",
        json!({"tab": checkout(), "controls": [], "frames": {"read": 1, "unread": 0}}),
    );
    assert!(!h.owner("hints_show", json!({})).await.0);
    // The page went to a none site while the call ran: nothing of it is said.
    answer(
        &ff,
        "page.outline@1",
        json!({"tab": page(12, "Inbox", "https://mail.example/inbox"),
               "items": [{"ref": 1, "role": "heading", "name": "Dear Ada", "level": 1}],
               "frames": {"read": 1, "unread": 0}}),
    );
    assert_eq!(
        h.owner("page_outline", json!({})).await,
        (
            true,
            "tab 12 went to a site Kai does not reach (Kai's site rules) while page_outline ran, \
             so nothing of it is said"
                .to_owned()
        )
    );
}

/// The design decision: a frame's own origin decides its rule, so a `none` site framed in a page
/// Kai reads is said as where it is and nothing more; a failure on a `none` site says what failed
/// and nothing of the page; an answer without its page's URL is not said.
#[tokio::test]
async fn frames_of_a_none_site_and_failures_on_one_say_nothing_of_them() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    let rules = |hands: bool| BrowserConfig {
        hands_and_eyes: hands,
        sites: Sites {
            none: vec!["mail.example".into(), "pay.example".into()],
            ..Default::default()
        },
        ..Default::default()
    };
    h.set(PolicyConfig::default(), rules(true));
    let pay = "https://pay.example";
    answer(
        &ff,
        "page.text_all@1",
        json!({"tab": checkout(),
               "frames": {"read": 3, "unread": 1, "unread_frames": [
                   {"frame": 9, "origin": pay, "code": "content_timeout", "title": "Card"}]},
               "blocks": [{"text": "Your cart"},
                          {"text": "Card 4111 1111 1111 1111", "frame": 7, "origin": pay},
                          {"text": "Expires 12/29", "frame": 7, "origin": pay},
                          {"text": "Cookies?", "frame": 8, "origin": "https://consent.example"},
                          {"frame": 9, "origin": pay, "unread": "content_timeout",
                           "title": "Card"},
                          {"text": "Total"}]}),
    );
    assert_eq!(
        h.owner("page_text_all", json!({})).await.1,
        format!(
            "{HEAD}\nYour cart\n(a frame of a site Kai does not reach)\nCookies?\n\
             (a frame of a site Kai does not reach)\nTotal\n\
             1 frame of the page could not be read: a frame of a site Kai does not reach"
        )
    );
    // Its controls are left out, and their refs never become Kai's.
    answer(
        &ff,
        "page.controls@1",
        json!({"tab": checkout(), "frames": {"read": 2, "unread": 0},
               "controls": [{"ref": 30, "role": "button", "name": "Buy"},
                            {"ref": 31, "role": "textbox", "name": "Card number",
                             "value": "4111", "frame": 7, "origin": pay},
                            {"ref": 32, "role": "button", "name": "Pay", "frame": 7,
                             "origin": pay}]}),
    );
    assert_eq!(
        h.owner("page_controls", json!({})).await.1,
        format!("{HEAD}\n30 button \"Buy\"\n(a frame of a site Kai does not reach)")
    );
    // Matches in it are neither listed nor counted.
    answer(
        &ff,
        "page.find@1",
        json!({"tab": checkout(), "text": "4111", "count": 1, "highlighted": true,
               "matches": [{"text": "4111", "sentence": "Card 4111",
                            "element": {"ref": 31, "role": "textbox", "name": "Card number"},
                            "frame": 7, "origin": pay}]}),
    );
    assert_eq!(
        h.owner("page_find", json!({"text": "4111"})).await.1,
        format!("{HEAD}\nno match for \"4111\"")
    );
    // A ref in such a frame is refused before anything is read.
    ff.model().elements.insert(
        31,
        json!({"tab": 12, "role": "textbox", "name": "Card number", "frame": 7, "origin": pay}),
    );
    assert_eq!(
        h.owner("element_text", json!({"ref": 31})).await,
        (
            true,
            "ref 31 is in a frame of a site Kai does not reach (Kai's site rules), in tab 12; \
             element_text did not run"
                .to_owned()
        )
    );
    // A ref tool whose answer comes from a none page: not said.
    element(&ff, 30, 12, "button", "Buy");
    answer(
        &ff,
        "element.text@1",
        json!({"ref": 30, "tab": page(12, "Inbox", "https://mail.example/inbox"),
               "role": "button", "name": "Buy", "blocks": [{"text": "Dear Ada"}]}),
    );
    assert_eq!(
        h.owner("element_text", json!({"ref": 30})).await,
        (
            true,
            "tab 12 went to a site Kai does not reach (Kai's site rules) while element_text ran, \
             so nothing of it is said"
                .to_owned()
        )
    );
    // ... and one whose error does: neither the tab's title and URL nor the message are said,
    // and the ref is retired.
    ff.model().twists.insert(
        "element.text@1",
        Twist::FailWith(
            json!({"code": "stale_document", "tab": 12, "title": "Inbox",
                               "url": "https://mail.example/inbox",
                               "message": "ref 30 is from the page before the last navigation \
                                           in tab 12"}),
        ),
    );
    assert_eq!(
        h.owner("element_text", json!({"ref": 30})).await,
        (
            true,
            "tab 12 is on a site Kai does not reach (Kai's site rules), so of what failed Kai \
             says only: the ref is from a page the tab has left"
                .to_owned()
        )
    );
    let (err, text) = h.owner("element_text", json!({"ref": 30})).await;
    assert!(
        err && text.starts_with("ref 30 names no element Kai can reach now"),
        "{text}"
    );
    // An error that names its tab without the URL: Kai lists the tabs again. The tab went to a
    // none site while the owner was asked about the ring.
    h.set(PolicyConfig::default(), rules(false));
    element(&ff, 33, 12, "button", "Gift wrap");
    ff.model().twists.insert(
        "element.point@1",
        Twist::FailWith(json!({"code": "content_timeout", "tab": 12,
                               "message": "tab 12 did not answer within 15000 ms"})),
    );
    let model = ff.model.clone();
    h.while_asked(move || {
        let mut m = model.lock().unwrap();
        let t = m.tabs.iter_mut().find(|t| t["id"] == 12).unwrap();
        t["url"] = json!("https://mail.example/inbox");
    });
    assert_eq!(
        h.owner("element_point", json!({"ref": 33})).await.1,
        "tab 12 is on a site Kai does not reach (Kai's site rules), so of what failed Kai says \
         only: its page did not answer in time"
    );
    ff.model().tabs[1]["url"] = json!("https://shop.example/cart");
    h.set(PolicyConfig::default(), rules(true));
    // The hints of a page that went to a none site while they were drawn come off again.
    answer(
        &ff,
        "hints.show@1",
        json!({"tab": page(12, "Inbox", "https://mail.example/inbox"),
               "controls": [{"ref": 40, "role": "button", "name": "Send"}],
               "frames": {"read": 1, "unread": 0}}),
    );
    answer(
        &ff,
        "hints.hide@1",
        json!({"tab": page(12, "Inbox", "https://mail.example/inbox"), "shown": true,
               "removed": 1}),
    );
    assert_eq!(
        h.owner("hints_show", json!({})).await.1,
        "tab 12 went to a site Kai does not reach (Kai's site rules) while hints_show ran, so \
         nothing of it is said; Kai took the hint labels off that page again"
    );
    assert_eq!(
        ff.acted().last().unwrap(),
        &("hints.hide@1".to_owned(), json!({"tab": 12}))
    );
    // An answer that names no URL is not said either.
    answer(
        &ff,
        "page.outline@1",
        json!({"tab": {"id": 12, "title": "Checkout"}, "items": [],
               "frames": {"read": 1, "unread": 0}}),
    );
    assert_eq!(
        h.owner("page_outline", json!({})).await.1,
        "Kai for Firefox answered page_outline without the URL of tab 12, so Kai cannot tell its \
         site (Kai's site rules) and says nothing of it"
    );
}

/// The final contract's other shapes: where hint labels could not be drawn, a hidden element's
/// ring, a heading's level, a hidden element's visible children, Reader view's page.
#[tokio::test]
async fn hints_rings_and_headings_say_the_contracts_details() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    answer(
        &ff,
        "hints.show@1",
        json!({"tab": checkout(), "frames": {"read": 2, "unread": 0},
               "controls": [{"ref": 214, "role": "button", "name": "Buy"},
                            {"ref": 216, "role": "button", "name": "Accept", "frame": 5,
                             "origin": "https://consent.example"}],
               "undrawn": [{"frame": 5, "origin": "https://consent.example",
                            "code": "content_timeout", "labels": 3}]}),
    );
    assert_eq!(
        h.owner("hints_show", json!({})).await.1,
        format!(
            "{HEAD}\nhints on 2 controls in view; 3 labels not drawn in consent.example (it did \
             not answer within Kai for Firefox's setting content_ms)\n\
             214 button \"Buy\"\n216 button \"Accept\""
        )
    );
    element(&ff, 215, 12, "button", "Gift wrap");
    answer(
        &ff,
        "element.point@1",
        json!({"ref": 215, "tab": checkout(), "role": "button", "name": "Gift wrap",
               "ms": 3000, "scrolled": false, "hidden": true}),
    );
    assert_eq!(
        h.owner("element_point", json!({"ref": 215})).await.1,
        format!("{HEAD}: 215 button \"Gift wrap\": the element is hidden; no ring")
    );
    // A heading's level, from element.describe@1 on: the dialog names it.
    ff.model().elements.insert(
        5,
        json!({"tab": 12, "role": "heading", "name": "Lamps", "level": 2}),
    );
    h.set(PolicyConfig::default(), BrowserConfig::default());
    answer(
        &ff,
        "element.point@1",
        json!({"ref": 5, "tab": checkout(), "role": "heading", "name": "Lamps", "level": 2,
               "ms": 3000, "scrolled": false}),
    );
    h.owner("element_point", json!({"ref": 5})).await;
    let asked = h.asked();
    let lines = &asked.last().unwrap().lines;
    assert!(
        lines.contains(
            &"shows on: 5 heading 2 \"Lamps\" in 12 \"Checkout\" https://shop.example/cart"
                .to_owned()
        ),
        "{lines:?}"
    );
    // A hidden element whose visible children still show.
    element(&ff, 9, 12, "region", "Offer");
    answer(
        &ff,
        "element.text@1",
        json!({"ref": 9, "tab": checkout(), "role": "region", "name": "Offer",
               "hidden": true, "blocks": [{"text": "Still shown"}]}),
    );
    assert_eq!(
        h.owner("element_text", json!({"ref": 9})).await.1,
        format!("{HEAD}\n9 region \"Offer\" is hidden; what of it is shown:\nStill shown")
    );
    // Reader view: the page it shows.
    ff.model().twists.insert(
        "page.text_all@1",
        Twist::FailWith(
            json!({"code": "reader_view", "tab": 12, "title": "Checkout",
                               "url": "about:reader?url=https%3A%2F%2Fshop.example%2Fnews",
                               "page": "https://shop.example/news",
                               "message": "tab 12 shows Reader view"}),
        ),
    );
    assert_eq!(
        h.owner("page_text_all", json!({})).await.1,
        "the tab shows Reader view of https://shop.example/news, which Kai cannot read (tab 12 \
         shows Reader view); Reader view off, the page reads"
    );
}

/// D4: two profiles' refs in one number space; a profile that closed keeps its refs from the
/// other one's elements.
#[tokio::test]
async fn two_profiles_share_one_number_space() {
    let h = Host::start().await;
    let a = h.firefox("c0ffee", &all_ops(), shop()).await;
    let b = h
        .firefox(
            "beef",
            &all_ops(),
            Model::with(vec![tab(50, 1, "News", "https://news.example/", false)]),
        )
        .await;
    h.set(PolicyConfig::default(), switch_on());
    let controls = |tab: Value, refs: [i64; 2]| {
        json!({"tab": tab, "frames": {"read": 1, "unread": 0},
               "controls": [{"ref": refs[0], "role": "button", "name": "One"},
                            {"ref": refs[1], "role": "button", "name": "Two"}]})
    };
    answer(&a, "page.controls@1", controls(checkout(), [5, 6]));
    assert!(
        h.owner("page_controls", json!({"tab": "12"}))
            .await
            .1
            .ends_with("\n5 button \"One\"\n6 button \"Two\"")
    );
    let news = page(50, "News", "https://news.example/");
    answer(&b, "hints.show@1", controls(news.clone(), [5, 9]));
    let text = h.owner("hints_show", json!({"tab": "50"})).await.1;
    assert!(
        text.ends_with("hints on 2 controls in view\n7 button \"One\" (hint 5)\n9 button \"Two\""),
        "{text}"
    );
    // Kai's 7 is beef's 5.
    element(&b, 5, 50, "button", "One");
    answer(
        &b,
        "element.text@1",
        json!({"ref": 5, "tab": news, "role": "button", "name": "One", "section": false,
               "blocks": [{"text": "One"}]}),
    );
    assert_eq!(
        h.owner("element_text", json!({"ref": 7})).await.1,
        "\"News\" https://news.example/ (tab 50)\n7 button \"One\":\nOne"
    );
    assert_eq!(
        b.acted()[1..],
        [
            ("element.describe@1".to_owned(), json!({"ref": 5})),
            ("element.text@1".to_owned(), json!({"ref": 5})),
        ]
    );
    // A stale ref's words carry Kai's number, not the extension's.
    b.model().twists.insert(
        "element.text@1",
        Twist::Fail(
            "stale_element",
            "ref 5's element is no longer in the page of tab 50 ('News') (the page removed it)",
        ),
    );
    assert!(
        h.owner("element_text", json!({"ref": 7}))
            .await
            .1
            .starts_with("ref 7's element is no longer in the page of tab 50")
    );
    // A number Kai has not seen, with two profiles linked.
    let (err, text) = h.owner("element_text", json!({"ref": 300})).await;
    assert!(
        err && text.contains("Firefox runs in 2 linked profiles"),
        "{text}"
    );
    // beef closes: its refs say so and never reach c0ffee.
    b.quit();
    h.rig.links_are(1).await;
    let (err, text) = h.owner("element_text", json!({"ref": 9})).await;
    assert!(err);
    assert!(
        text.starts_with("ref 9 is from the Firefox of profile beef, which is not linked"),
        "{text}"
    );
    assert!(
        a.acted()
            .iter()
            .all(|(op, args)| op != "element.describe@1" || args["ref"] != 9)
    );
}

/// One page of [`pdf_of`]'s PDF.
#[derive(Clone, Copy)]
enum Page<'a> {
    /// A line of text in Helvetica.
    Text(&'a str),
    /// A line in ZapfDingbats: symbols where the letters were, as a font without a usable
    /// encoding gives.
    Dingbats(&'a str),
    /// No text: a scan's page.
    Blank,
}

/// A small PDF, written by hand: one page per entry.
fn pdf_of(pages: &[Page]) -> Vec<u8> {
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            (0..pages.len())
                .map(|i| format!("{} 0 R", 5 + 2 * i))
                .collect::<Vec<_>>()
                .join(" "),
            pages.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>".to_owned(),
    ];
    for (i, p) in pages.iter().enumerate() {
        let content = match p {
            Page::Text(t) => format!("BT /F1 18 Tf 72 700 Td ({t}) Tj ET"),
            Page::Dingbats(t) => format!("BT /F2 18 Tf 72 700 Td ({t}) Tj ET"),
            Page::Blank => String::new(),
        };
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 \
             0 R /F2 4 0 R >> >> /Contents {} 0 R >>",
            6 + 2 * i
        ));
        objs.push(format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ));
    }
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// Whether the tests that run poppler's `pdftotext` run: they fail when it is missing, unless
/// `KAI_TEST_NO_PDFTOTEXT` says to skip them (module doc).
fn pdftotext_runs(test: &str) -> bool {
    if std::env::var_os("KAI_TEST_NO_PDFTOTEXT").is_some() {
        eprintln!("SKIPPED {test}: KAI_TEST_NO_PDFTOTEXT is set");
        return false;
    }
    assert!(
        pdf::Tool::find().bin.is_some(),
        "{test} needs poppler's pdftotext (dnf install poppler-utils); KAI_TEST_NO_PDFTOTEXT=1 \
         skips it"
    );
    true
}

fn limits() -> pdf::Limits {
    pdf::Limits {
        time: Duration::from_secs(30),
        text_bytes: 1 << 30,
        memory_bytes: 4 << 30,
    }
}

/// A PDF tab: `page.text@1` answers `pdf_viewer`, `pdf.bytes@1` the bytes of `pages`.
fn a_pdf(ff: &Firefox, pages: &[Page], from: &str) {
    let bytes = pdf_of(pages);
    let mut m = ff.model();
    m.twists.insert(
        "page.text@1",
        Twist::Fail("pdf_viewer", "tab 12 shows a PDF in Firefox's viewer"),
    );
    m.twists.insert(
        "pdf.bytes@1",
        Twist::Answer(json!({
            "tab": page(12, "prices.pdf", "https://shop.example/prices.pdf"),
            "url": from, "content_type": "application/pdf",
            "bytes": bytes.len(),
            "base64": base64::engine::general_purpose::STANDARD.encode(&bytes)})),
    );
}

#[tokio::test]
async fn page_text_reads_a_pdf_with_pdftotext() {
    if !pdftotext_runs("page_text_reads_a_pdf_with_pdftotext") {
        return;
    }
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    a_pdf(
        &ff,
        &[
            Page::Text("Trail Lamp: 12 EUR"),
            Page::Blank,
            Page::Text("Valley Tent: 2.9 kg"),
            Page::Dingbats("abcdefghijklmnopqrstuvwxyz ABCDEFG"),
        ],
        "https://cdn.shop.example/prices.pdf",
    );
    assert_eq!(
        h.owner("page_text", json!({})).await,
        (
            false,
            "\"prices.pdf\" https://shop.example/prices.pdf (tab 12)\n\
             a PDF of 4 pages, its text as poppler's pdftotext reads it:\n\
             page 1:\nTrail Lamp: 12 EUR\n\
             page 2 has no text layer (a scan?)\n\
             page 3:\nValley Tent: 2.9 kg\n\
             page 4's text is not readable, so Kai leaves it out: of its 33 characters, 0 are \
             private-use, control or replacement characters and 0 letters or digits"
                .to_owned()
        )
    );
    assert_eq!(
        ff.acted(),
        [
            ("page.text@1".to_owned(), json!({"tab": 12})),
            ("pdf.bytes@1".to_owned(), json!({"tab": 12})),
        ]
    );
    // What is no PDF.
    ff.model().twists.insert(
        "pdf.bytes@1",
        Twist::Answer(
            json!({"tab": checkout(), "url": "https://shop.example/cart",
                             "base64": "bm8gcGRm"}),
        ),
    );
    let (err, text) = h.owner("page_text", json!({})).await;
    assert!(err);
    assert!(
        text.starts_with("pdftotext failed (exit status: 1): it could not read the PDF"),
        "{text}"
    );
    // A memory bound below what pdftotext needs to start is said by its setting.
    let small = pdf::Limits {
        memory_bytes: 16 << 20,
        ..limits()
    };
    let e = pdf::extract(&pdf::Tool::find(), pdf_of(&[Page::Text("x")]), &small)
        .await
        .unwrap_err();
    assert!(e.contains("browser.pdf_memory_max_mb"), "{e}");
}

#[tokio::test]
async fn a_pdf_from_a_none_site_or_no_pdf_any_more_is_not_read() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            sites: Sites {
                none: vec!["files.example".into()],
                ..Default::default()
            },
            ..Default::default()
        },
    );
    // The tab is on shop.example; its PDF came from a none site after a redirect.
    a_pdf(
        &ff,
        &[Page::Text("Secret")],
        "https://files.example/s/prices.pdf",
    );
    assert_eq!(
        h.owner("page_text", json!({})).await,
        (
            true,
            "the PDF of tab 12 came from a site Kai does not reach (Kai's site rules), so Kai \
             does not read it"
                .to_owned()
        )
    );
    // The URL answers something else now; and an HTTP error.
    ff.model().twists.insert(
        "pdf.bytes@1",
        Twist::FailWith(json!({"code": "not_pdf", "tab": 12, "title": "prices.pdf",
            "url": "https://shop.example/prices.pdf",
            "response_url": "https://shop.example/login",
            "message": "the PDF's URL now answers 'text/html; charset=utf-8' and no PDF; tab 12 \
                        shows the PDF of an earlier answer"})),
    );
    assert_eq!(
        h.owner("page_text", json!({})).await.1,
        "Kai for Firefox fetched the PDF again and https://shop.example/login answered something \
         that is no PDF (the PDF's URL now answers 'text/html; charset=utf-8' and no PDF; tab 12 \
         shows the PDF of an earlier answer); the tab shows the PDF of an earlier answer, which \
         cannot be fetched again"
    );
    ff.model().twists.insert(
        "pdf.bytes@1",
        Twist::FailWith(
            json!({"code": "fetch_failed", "tab": 12, "title": "prices.pdf",
            "url": "https://shop.example/prices.pdf",
            "response_url": "https://files.example/gone.pdf",
            "message": "HTTP 403"}),
        ),
    );
    assert_eq!(
        h.owner("page_text", json!({})).await.1,
        "tab 12 is on a site Kai does not reach (Kai's site rules), so of what failed Kai says \
         only: Kai for Firefox could not fetch its PDF"
    );
    // An extension without pdf.bytes@1, and the other text tools pointing at page_text.
    ff.quit();
    h.rig.links_are(0).await;
    let ops: Vec<&str> = all_ops()
        .into_iter()
        .filter(|o| *o != "pdf.bytes@1")
        .collect();
    let ff = h.firefox("c0ffee", &ops, shop()).await;
    ff.model().twists.insert(
        "page.text@1",
        Twist::Fail("pdf_viewer", "tab 12 shows a PDF in Firefox's viewer"),
    );
    assert_eq!(
        h.owner("page_text", json!({})).await,
        (
            true,
            "Kai for Firefox in profile c0ffee does not offer pdf.bytes@1; an update of the \
             extension there brings it"
                .to_owned()
        )
    );
    ff.model().twists.insert(
        "page.text_all@1",
        Twist::Fail("pdf_viewer", "tab 12 shows a PDF in Firefox's viewer"),
    );
    assert_eq!(
        h.owner("page_text_all", json!({})).await.1,
        "a PDF in Firefox's viewer, which page_text reads (tab 12 shows a PDF in Firefox's \
         viewer)"
    );
}

/// A stub `pdftotext` (a shell script) in `dir`.
fn stub(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("pdftotext");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    path
}

/// `pdf::extract` with a stub: a test thread that forks while the script is still open for
/// writing makes its exec fail with ETXTBSY for a moment, the test's race and not pdftotext's, so
/// that is tried again, a bounded number of times.
async fn stubbed(bin: &Path, bytes: Vec<u8>, limits: &pdf::Limits) -> Result<String, String> {
    const TRIES: usize = 50;
    for _ in 0..TRIES {
        let r = pdf::extract(&pdf::Tool::at(bin), bytes.clone(), limits).await;
        if !matches!(&r, Err(e) if e.contains("Text file busy")) {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "the stub {} stayed busy (ETXTBSY) for {TRIES} tries",
        bin.display()
    );
}

#[tokio::test]
async fn pdftotext_missing_slow_long_or_failing_is_said() {
    let missing = pdf::extract(
        &pdf::Tool {
            bin: None,
            on_path: true,
            bwrap: None,
        },
        b"%PDF-1.4".to_vec(),
        &limits(),
    )
    .await
    .unwrap_err();
    assert!(missing.contains("not installed"), "{missing}");
    assert!(missing.contains("poppler-utils"), "{missing}");
    let dir = tempfile::tempdir().unwrap();
    // A pdftotext that never answers is stopped after the setting's time, and the error names it.
    let slow = stub(dir.path(), "exec sleep 30");
    let start = std::time::Instant::now();
    let quick = pdf::Limits {
        time: Duration::from_secs(1),
        ..limits()
    };
    let e = stubbed(&slow, b"%PDF-1.4".to_vec(), &quick)
        .await
        .unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(10));
    assert!(e.starts_with("pdftotext did not finish within 1 s"), "{e}");
    assert!(e.contains("browser.pdf_timeout_s"), "{e}");
    // 4 MiB through a pdftotext that echoes it: far past both pipes' buffers, so a writer that
    // shared the reader's task would hang.
    let dir = tempfile::tempdir().unwrap();
    let echo = stub(dir.path(), "exec cat");
    let data = vec![b'x'; 4 << 20];
    let out = stubbed(&echo, data.clone(), &limits()).await.unwrap();
    assert_eq!(out.len(), data.len());
    // More text than the setting allows: stopped and named.
    let tight = pdf::Limits {
        text_bytes: 1 << 20,
        ..limits()
    };
    let e = stubbed(&echo, data, &tight).await.unwrap_err();
    assert!(
        e.starts_with("pdftotext wrote more than 1 MB of text and was stopped"),
        "{e}"
    );
    assert!(e.contains("browser.pdf_text_max_mb"), "{e}");
    // A failing run: its first lines of stderr, the rest counted.
    let dir = tempfile::tempdir().unwrap();
    let loud = stub(
        dir.path(),
        "for i in $(seq 1 100); do echo line$i >&2; done; exit 3",
    );
    let e = stubbed(&loud, Vec::new(), &limits()).await.unwrap_err();
    assert!(
        e.starts_with(
            "pdftotext failed (exit status: 3): the PDF's permissions do not allow copying its \
             text: line1 / line2 / "
        ),
        "{e}"
    );
    assert!(e.ends_with("line20 / (… 80 more)"), "{e}");
    // It runs with PATH and LANG only, and in the sandbox where bwrap is installed: the system's
    // /etc is not there.
    let dir = tempfile::tempdir().unwrap();
    let env = stub(
        dir.path(),
        "printf '%s %s' \"${HOME:-no-home}\" \"${PATH:+path}\"",
    );
    assert_eq!(
        stubbed(&env, Vec::new(), &limits()).await.unwrap(),
        "no-home path"
    );
    if Path::new(pdf::BWRAP).is_file() {
        let dir = tempfile::tempdir().unwrap();
        let peek = stub(dir.path(), "exec cat /etc/os-release");
        let e = stubbed(&peek, Vec::new(), &limits()).await.unwrap_err();
        assert!(e.contains("No such file or directory"), "{e}");
    }
}
