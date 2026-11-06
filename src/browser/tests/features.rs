//! Firefox's own features, the source and the DOM, the style sheets, the scripts and the
//! screenshots (kai-firefox design §5.3's `page_see` and `element_see`, §5.5 to §5.8, WP7) through
//! the host's dispatch against a fake Kai for Firefox that answers the contract's shapes: each
//! handler's lines and the operation it sends, the site rules on bookmarks, the history, the
//! downloads, a document's frames and a screenshot's frames (blanked out of the image), what is
//! asked (the zoom, Reader view, the sheets and the scripts confirmed; a sheet's fetches, as the
//! extension's parser reads them, named, and one from a site the user never visited asked whatever
//! the tool's state; a script approved in lmgw names its tab), what is sent (the origin the dialog
//! showed, the origins a sheet may fetch from), and the words of their errors.

use base64::Engine as _;
use serde_json::{Value, json};

use super::fake::{Firefox, Host, Twist, all_ops, shop};
use crate::browser::settings::{BrowserConfig, Sites};
use crate::host::registry::Block;
use crate::host::settings::{PolicyConfig, ToolState};

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

fn fail(ff: &Firefox, op: &'static str, error: Value) {
    ff.model().twists.insert(op, Twist::FailWith(error));
}

fn element(ff: &Firefox, r: i64, tab: i64, role: &str, name: &str) {
    ff.model()
        .elements
        .insert(r, json!({"tab": tab, "role": role, "name": name}));
}

/// The last operation `op` sent, its arguments.
fn sent(ff: &Firefox, op: &str) -> Value {
    ff.acted()
        .into_iter()
        .rev()
        .find(|(o, _)| o == op)
        .unwrap_or_else(|| panic!("{op} was not sent"))
        .1
}

fn rules(none: &[&str], read: &[&str]) -> BrowserConfig {
    BrowserConfig {
        sites: Sites {
            none: none.iter().map(|s| (*s).to_owned()).collect(),
            read: read.iter().map(|s| (*s).to_owned()).collect(),
            act: Vec::new(),
        },
        ..Default::default()
    }
}

/// With every operation served, the usual list has all 47 tools of the area (reading aloud is
/// WP9's and only for hands and eyes); the switch adds the three hands-and-eyes reading tools.
#[tokio::test]
async fn the_features_source_styles_scripts_and_screenshots_are_listed() {
    let h = Host::start().await;
    let _ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    let names = h.names();
    for t in [
        "page_see",
        "element_see",
        "page_zoom",
        "reader_view",
        "bookmarks_find",
        "history_find",
        "downloads_list",
        "page_source",
        "page_dom",
        "element_dom",
        "page_css",
        "page_css_remove",
        "page_script",
        "page_script_main",
    ] {
        assert!(names.contains(&t), "{t}: {names:?}");
    }
    assert_eq!(names.len(), 47);
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            hands_and_eyes: true,
            ..Default::default()
        },
    );
    assert_eq!(h.names().len(), 50);
}

/// The zoom is set, confirmed (§6.4: the switch does not cover it, also when it is on), on the page
/// the dialog showed (its origin sent); the line says Firefox zooms every tab of the site. A `read`
/// site allows it (it changes the view, not the page), a `none` site does not. `page_changed` says
/// the zoom may have been set already.
#[tokio::test]
async fn the_zoom_is_set_for_every_tab_of_the_site() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    answer(
        &ff,
        "page.zoom@1",
        json!({"tab": checkout(), "percent": 150, "was": 100, "default_percent": 100,
               "scope": "per-origin"}),
    );
    assert_eq!(
        h.owner("page_zoom", json!({"percent": 150})).await,
        (
            false,
            format!(
                "{HEAD}: zoom 150 % (was 100 %, Firefox's default 100 %); Firefox zooms every tab \
                 of shop.example alike and keeps the zoom for the site"
            )
        )
    );
    assert_eq!(
        sent(&ff, "page.zoom@1"),
        json!({"tab": 12, "percent": 150, "origin": "https://shop.example"})
    );
    let asked = h.asked();
    assert_eq!(asked.len(), 1, "confirmed by default");
    assert_eq!(
        asked[0].lines,
        [
            "percent: 150",
            "zooms: 12 \"Checkout\" https://shop.example/cart"
        ]
    );
    answer(
        &ff,
        "page.zoom@1",
        json!({"tab": checkout(), "percent": 150, "was": 150, "default_percent": 100,
               "scope": "per-tab"}),
    );
    assert_eq!(
        h.owner("page_zoom", json!({"percent": 150})).await.1,
        format!(
            "{HEAD}: zoom 150 %, as it was (Firefox's default 100 %); Firefox zooms this tab only"
        )
    );
    // The zoom is read by browser_context: without percent the call does not run.
    let before = ff.count("page.zoom@1");
    let (err, text) = h.owner("page_zoom", json!({})).await;
    assert!(err && text.contains("needs percent"), "{text}");
    assert!(h.owner("page_zoom", json!({"percent": 0})).await.0);
    assert_eq!(ff.count("page.zoom@1"), before);
    // The hands-and-eyes switch leaves it asked.
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            hands_and_eyes: true,
            ..Default::default()
        },
    );
    let _ = h.asked();
    assert!(!h.owner("page_zoom", json!({"percent": 150})).await.0);
    assert_eq!(h.asked().len(), 1, "asked with the switch on");
    h.set(PolicyConfig::default(), BrowserConfig::default());
    fail(
        &ff,
        "page.zoom@1",
        json!({"code": "bad_value", "tab": 12, "title": "Checkout",
               "url": "https://shop.example/cart",
               "message": "Firefox does not zoom tab 12 to 9000 %: Zoom value 90 out of range \
                           (must be between 0.2 and 5)"}),
    );
    assert_eq!(
        h.owner("page_zoom", json!({"percent": 9000})).await,
        (
            true,
            "Firefox does not zoom tab 12 to 9000 %: Zoom value 90 out of range (must be between \
             0.2 and 5)"
                .to_owned()
        )
    );
    // The tab showed another origin right after the zoom was set.
    fail(
        &ff,
        "page.zoom@1",
        json!({"code": "page_changed", "tab": 12, "title": "Lamps",
               "url": "https://lamps.example/",
               "message": "tab 12 showed a page of another origin than the one Kai approved when \
                           its zoom was set, so the zoom may have reached that page"}),
    );
    assert_eq!(
        h.owner("page_zoom", json!({"percent": 120})).await,
        (
            true,
            "tab 12 showed a page of another origin than the one Kai approved when its zoom was \
             set, so the zoom may have reached that page; the tab shows a page of another origin \
             than the one the call was for: found before the zoom was set, nothing was done; \
             found right after, the zoom was set and may have reached that page (browser_context \
             reads the zoom now)"
                .to_owned()
        )
    );
    // Gone to a none site meanwhile: Kai's words only.
    h.set(PolicyConfig::default(), rules(&["lamps.example"], &[]));
    let (err, text) = h.owner("page_zoom", json!({"percent": 120})).await;
    assert_eq!(
        (err, text.as_str()),
        (
            true,
            "tab 12 is on a site Kai does not reach (Kai's site rules), so of what failed Kai says \
             only: its page was of another origin than the one the call was for, before the zoom \
             was set (nothing was done) or right after (the zoom may have reached that page)"
        )
    );
    h.set(
        PolicyConfig::default(),
        rules(&["mail.example"], &["shop.example"]),
    );
    answer(
        &ff,
        "page.zoom@1",
        json!({"tab": checkout(), "percent": 120, "was": 100, "default_percent": 100,
               "scope": "per-origin"}),
    );
    assert!(
        !h.owner("page_zoom", json!({"percent": 120})).await.0,
        "a read site"
    );
    let before = ff.count("page.zoom@1");
    let (err, text) = h
        .owner("page_zoom", json!({"percent": 120, "tab": "3"}))
        .await;
    assert!(err && text.contains("site Kai does not reach"), "{text}");
    assert_eq!(ff.count("page.zoom@1"), before);
}

/// Reader view: on and off (its origin sent), already so, still loading, no article, the page
/// changed once it had changed; `on` is required; refused on a `read` site.
#[tokio::test]
async fn reader_view_goes_on_and_off_and_says_it_cannot_be_read() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    let reader = json!({"id": 12, "window": 1, "title": "Checkout",
                        "url": "about:reader?url=https%3A%2F%2Fshop.example%2Fcart",
                        "active": true, "user": true});
    answer(
        &ff,
        "reader.view@1",
        json!({"tab": reader, "loaded": true, "on": true, "changed": true}),
    );
    assert_eq!(
        h.owner("reader_view", json!({"on": true})).await,
        (
            false,
            "\"Checkout\" about:reader?url=https%3A%2F%2Fshop.example%2Fcart (tab 12): Reader \
             view on; Kai's reading tools cannot read Reader view (page_text reads the page's \
             article with Reader view off)"
                .to_owned()
        )
    );
    assert_eq!(
        sent(&ff, "reader.view@1"),
        json!({"tab": 12, "on": true, "origin": "https://shop.example"})
    );
    assert_eq!(h.asked().len(), 1, "confirmed by default");
    let mut tab = shop().tabs[1].clone();
    tab["loading"] = json!(true);
    answer(
        &ff,
        "reader.view@1",
        json!({"tab": tab, "loaded": false, "on": false, "changed": false}),
    );
    assert_eq!(
        h.owner("reader_view", json!({"on": false})).await.1,
        format!("{HEAD}: Reader view was off already; the page is still loading")
    );
    let before = ff.count("reader.view@1");
    let (err, text) = h.owner("reader_view", json!({})).await;
    assert!(err && text.contains("needs on"), "{text}");
    assert_eq!(ff.count("reader.view@1"), before);
    fail(
        &ff,
        "reader.view@1",
        json!({"code": "no_article", "tab": 12, "title": "Checkout",
               "url": "https://shop.example/cart",
               "message": "Firefox finds no article in tab 12"}),
    );
    assert_eq!(
        h.owner("reader_view", json!({"on": true})).await,
        (
            true,
            "Firefox finds no article in tab 12; Firefox finds no article in the page, so Reader \
             view cannot show it (browser_context says when it can)"
                .to_owned()
        )
    );
    fail(
        &ff,
        "reader.view@1",
        json!({"code": "page_changed", "tab": 12, "title": "Lamps",
               "url": "https://lamps.example/",
               "message": "tab 12 showed a page of another origin than the one Kai approved once \
                           Reader view was turned on"}),
    );
    assert!(
        h.owner("reader_view", json!({"on": true}))
            .await
            .1
            .ends_with(
                "found once it had changed, Reader view was turned on or off already \
                 (browser_context says which)"
            )
    );
    h.set(PolicyConfig::default(), rules(&[], &["shop.example"]));
    let (err, text) = h.owner("reader_view", json!({"on": true})).await;
    assert!(err && text.contains("may only read"), "{text}");
}

/// Bookmarks, the history and the downloads: one line each, read-only (the owner is not asked),
/// an entry with any URL on a `none` site left out and counted; a finished download is opened
/// with Kai's `open`; two linked profiles each under its heading.
#[tokio::test]
async fn bookmarks_history_and_downloads_leave_out_what_the_site_rules_hide() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), rules(&["mail.example"], &[]));
    answer(
        &ff,
        "bookmarks.find@1",
        json!({"query": "care notes", "bookmarks": [
            {"title": "Tent care notes", "url": "https://shop.example/tents/care",
             "added": 1_767_225_600_000i64, "folder": ["Bookmarks Toolbar", "Gear"]},
            {"title": "Inbox", "url": "https://mail.example/inbox", "added": null, "folder": []},
            {"title": "Plain", "url": "https://plain.example/", "added": null, "folder": []}]}),
    );
    assert_eq!(
        h.owner("bookmarks_find", json!({"query": "care notes"}))
            .await,
        (
            false,
            "\"Tent care notes\" https://shop.example/tents/care (added 2026-01-01 00:00 UTC, in \
             \"Bookmarks Toolbar\" › \"Gear\")\n\
             \"Plain\" https://plain.example/\n\
             1 more on sites Kai does not reach (Kai's site rules)"
                .to_owned()
        )
    );
    assert_eq!(
        sent(&ff, "bookmarks.find@1"),
        json!({"query": "care notes"})
    );
    assert!(h.asked().is_empty(), "a read runs unasked for the owner");
    assert!(h.owner("bookmarks_find", json!({"query": " "})).await.0);
    answer(
        &ff,
        "history.find@1",
        json!({"query": "help", "pages": [
            {"title": "Help: Northwind Gear", "url": "https://shop.example/help",
             "last_visit": 1_767_225_600_000i64, "visits": 3},
            {"title": "Mail help", "url": "https://mail.example/help", "last_visit": null,
             "visits": 1}]}),
    );
    assert_eq!(
        h.owner("history_find", json!({"query": "help"})).await.1,
        "\"Help: Northwind Gear\" https://shop.example/help (last visited 2026-01-01 00:00 UTC, 3 \
         visits)\n1 more on sites Kai does not reach (Kai's site rules)"
    );
    answer(
        &ff,
        "history.find@1",
        json!({"query": "nothing", "pages": []}),
    );
    assert_eq!(
        h.owner("history_find", json!({"query": "nothing"})).await.1,
        "no pages in the history for \"nothing\""
    );
    answer(
        &ff,
        "downloads.list@1",
        json!({"downloads": [
            {"file": "price-list.txt", "path": "/home/ada/Downloads/price-list.txt",
             "url": "https://shop.example/download", "final_url": "https://files.example/p.txt",
             "referrer": "https://shop.example/cart", "state": "complete", "size": 14,
             "received": 14, "started": 1_767_225_600_000i64, "exists": true,
             "mime": "text/plain"},
            {"file": "invoice.pdf", "path": "/home/ada/Downloads/invoice.pdf",
             "url": "https://files.example/i.pdf", "final_url": "https://files.example/i.pdf",
             "referrer": "https://mail.example/inbox", "state": "complete", "size": 900,
             "received": 900, "started": null, "exists": true, "mime": null},
            {"file": "tent.iso", "path": "/home/ada/Downloads/tent.iso",
             "url": "https://shop.example/tent.iso", "final_url": null, "referrer": null,
             "state": "in_progress", "size": 3_145_728, "received": 1_048_576, "paused": true,
             "started": null, "exists": true, "danger": "uncommon", "mime": null},
            {"file": "old.zip", "path": "/home/ada/Downloads/old.zip",
             "url": "https://shop.example/old.zip", "state": "interrupted",
             "error": "NETWORK_FAILED", "size": null, "received": 10, "started": null,
             "exists": false, "mime": null},
            // Its address on a none site, where it went after a redirect not.
            {"file": "attachment.pdf", "path": "/home/ada/Downloads/attachment.pdf",
             "url": "https://mail.example/att/1", "final_url": "https://files.example/a.pdf",
             "referrer": null, "state": "complete", "size": 5, "received": 5, "started": null,
             "exists": true, "mime": null},
            // Redirected to a none site.
            {"file": "export.csv", "path": "/home/ada/Downloads/export.csv",
             "url": "https://files.example/export", "final_url": "https://mail.example/e.csv",
             "referrer": "https://shop.example/cart", "state": "complete", "size": 5,
             "received": 5, "started": null, "exists": true, "mime": null}]}),
    );
    assert_eq!(
        h.owner("downloads_list", json!({})).await.1,
        "\"price-list.txt\" /home/ada/Downloads/price-list.txt: complete, 14 bytes, started \
         2026-01-01 00:00 UTC, from https://files.example/p.txt\n\
         \"tent.iso\" /home/ada/Downloads/tent.iso: downloading, 1.0 MiB of 3.0 MiB, paused, \
         Firefox flags it as dangerous (uncommon), from https://shop.example/tent.iso\n\
         \"old.zip\" /home/ada/Downloads/old.zip: interrupted, Firefox says NETWORK_FAILED, no \
         longer on disk, from https://shop.example/old.zip\n\
         3 more on sites Kai does not reach (Kai's site rules)\n\
         a finished download opens with Kai's open tool on its path (open {target: <path>}): \
         Firefox opens one only on a click in the extension"
    );
    assert_eq!(sent(&ff, "downloads.list@1"), json!({}));
    // A second profile: each under its heading.
    let other = h.firefox("b0b", &all_ops(), shop()).await;
    answer(&other, "downloads.list@1", json!({"downloads": []}));
    let text = h.owner("downloads_list", json!({})).await.1;
    assert!(
        text.starts_with("profile c0ffee (Firefox 159.0):\n"),
        "{text}"
    );
    assert!(
        text.contains("\nprofile b0b (Firefox 159.0):\nno downloads"),
        "{text}"
    );
}

/// The source: what was fetched and whether the cache or the server answered, then the text whole;
/// a page from a `POST` or of unknown origin points to `page_dom`; a source redirected to a `none`
/// site is not said.
#[tokio::test]
async fn the_source_says_who_answered_and_points_to_the_dom() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    let source = |cache: Value| {
        json!({"tab": checkout(), "url": "https://shop.example/cart", "method": "GET",
               "cache": cache, "status": 200, "content_type": "text/html; charset=utf-8",
               "charset": "UTF-8", "bytes": 48,
               "text": "<!doctype html>\r\n<title>Checkout</title>\r\n<p>Lamp\u{202e}</p>"})
    };
    answer(&ff, "page.source@1", source(json!(true)));
    assert_eq!(
        h.owner("page_source", json!({})).await,
        (
            false,
            format!(
                "{HEAD}\nsource of https://shop.example/cart: GET, HTTP 200, text/html; \
                 charset=utf-8, read as UTF-8, 48 bytes uncompressed, the browser's cache \
                 answered, the server was not asked again:\n<!doctype html>\n\
                 <title>Checkout</title>\n\"<p>Lamp\\u202e</p>\""
            )
        )
    );
    assert!(h.asked().is_empty(), "a read runs unasked for the owner");
    answer(&ff, "page.source@1", source(json!(false)));
    assert!(
        h.owner("page_source", json!({}))
            .await
            .1
            .contains("the server was asked again, so it may differ from what the tab shows:")
    );
    answer(&ff, "page.source@1", source(Value::Null));
    assert!(
        h.owner("page_source", json!({}))
            .await
            .1
            .contains("Kai for Firefox could not tell whether the cache or the server answered")
    );
    fail(
        &ff,
        "page.source@1",
        json!({"code": "not_get", "tab": 12, "title": "Checkout",
               "url": "https://shop.example/cart", "method": "POST",
               "message": "tab 12 shows the answer to a POST; page.dom reads it"}),
    );
    assert_eq!(
        h.owner("page_source", json!({})).await,
        (
            true,
            "tab 12 shows the answer to a POST; page.dom reads it; fetching the page again would \
             send its POST again, so Kai for Firefox did not: page_dom reads the live document \
             instead"
                .to_owned()
        )
    );
    fail(
        &ff,
        "page.source@1",
        json!({"code": "source_unknown", "tab": 12, "title": "Checkout",
               "url": "https://shop.example/cart",
               "message": "Kai for Firefox does not know how tab 12's page was requested"}),
    );
    assert_eq!(
        h.owner("page_source", json!({})).await.1,
        "Kai for Firefox does not know how tab 12's page was requested; page_dom reads the live \
         document instead (a page that has just loaded may have its request reported late by \
         Firefox, and asking again then finds it)"
    );
    fail(
        &ff,
        "page.source@1",
        json!({"code": "fetch_failed", "tab": 12, "title": "Checkout",
               "url": "https://shop.example/cart", "message": "NetworkError"}),
    );
    assert_eq!(
        h.owner("page_source", json!({})).await.1,
        "Kai for Firefox could not fetch the page's source again: NetworkError"
    );
    h.set(PolicyConfig::default(), rules(&["login.example"], &[]));
    let mut redirected = source(json!(false));
    redirected["response_url"] = json!("https://login.example/sso");
    answer(&ff, "page.source@1", redirected);
    assert_eq!(
        h.owner("page_source", json!({})).await,
        (
            true,
            "the source of tab 12 came from a site Kai does not reach (Kai's site rules), so Kai \
             does not say it"
                .to_owned()
        )
    );
}

/// The source's frame addresses on `none` sites (an opaque one too, while some site is `none`)
/// are taken out of its text wherever the text has them, as written (`&amp;` and all); a source
/// whose frames the extension does not list is not said while some site is `none`.
#[tokio::test]
async fn the_source_takes_out_the_addresses_of_frames_kai_does_not_reach() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), rules(&["pay.example"], &[]));
    let text = "<iframe src=\"https://consent.example/b?site=shop&amp;v=2\"></iframe>\n\
                <iframe src='https://pay.example/card?o=1&amp;amt=9'></iframe>\n\
                <script>var u = \"https://pay.example/card?o=1&amp;amt=9\";</script>\n\
                <object data=https://pay.example/card></object>\n\
                <iframe src=\"data:text/html,<p>x\"></iframe>\n\
                <iframe src=\"help.html\"></iframe>";
    answer(
        &ff,
        "page.source@1",
        json!({"tab": checkout(), "url": "https://shop.example/cart", "method": "GET",
               "cache": true, "status": 200, "content_type": "text/html", "charset": "UTF-8",
               "bytes": 300, "text": text,
               "frames": [
                   {"src": "https://consent.example/b?site=shop&amp;v=2",
                    "origin": "https://consent.example"},
                   {"src": "https://pay.example/card?o=1&amp;amt=9", "origin": "https://pay.example"},
                   {"src": "https://pay.example/card", "origin": "https://pay.example"},
                   {"src": "data:text/html,<p>x", "origin": null},
                   {"src": "help.html", "origin": "https://shop.example"}]}),
    );
    let (err, said) = h.owner("page_source", json!({})).await;
    assert!(!err, "{said}");
    assert_eq!(
        said,
        format!(
            "{HEAD}\nsource of https://shop.example/cart: GET, HTTP 200, text/html, read as \
             UTF-8, 300 bytes uncompressed, the browser's cache answered, the server was not \
             asked again, the addresses of 3 frames of sites Kai does not reach taken out of the \
             text:\n\
             <iframe src=\"https://consent.example/b?site=shop&amp;v=2\"></iframe>\n\
             <iframe src=''></iframe>\n\
             <script>var u = \"\";</script>\n\
             <object data=></object>\n\
             <iframe src=\"\"></iframe>\n\
             <iframe src=\"help.html\"></iframe>"
        )
    );
    assert!(!said.contains("pay.example"));
    // Without the frames listed: not said while a site is none, said otherwise.
    answer(
        &ff,
        "page.source@1",
        json!({"tab": checkout(), "url": "https://shop.example/cart", "method": "GET",
               "cache": true, "status": 200, "content_type": null, "charset": "UTF-8",
               "bytes": 1, "text": "x"}),
    );
    let (err, said) = h.owner("page_source", json!({})).await;
    assert!(
        err && said.starts_with("Kai for Firefox did not list the frame addresses"),
        "{said}"
    );
    h.set(PolicyConfig::default(), BrowserConfig::default());
    assert_eq!(
        h.owner("page_source", json!({})).await,
        (
            false,
            format!(
                "{HEAD}\nsource of https://shop.example/cart: GET, HTTP 200, read as UTF-8, 1 \
                 byte uncompressed, the browser's cache answered, the server was not asked \
                 again:\nx"
            )
        )
    );
}

/// The DOM whole, Kai's marks where frames begin and end, a frame of a `none` site dropped by its
/// own origin and named only by where it is, an unread one with its reason; one element's HTML
/// under its line.
#[tokio::test]
async fn the_dom_marks_its_frames_and_drops_the_none_sites() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), rules(&["pay.example"], &[]));
    answer(
        &ff,
        "page.dom@1",
        json!({"tab": checkout(), "url": "https://shop.example/cart",
               "html": ["<!DOCTYPE html><html><body><iframe src=\"https://consent.example\">",
                        {"frame": 17_179_869_185i64, "url": "https://consent.example/",
                         "origin": "https://consent.example",
                         "src": "https://consent.example/?site=shop",
                         "html": ["<p>Cookies?</p>"]},
                        "</iframe><iframe src=\"https://pay.example\">",
                        {"frame": 17_179_869_186i64, "url": "https://pay.example/card",
                         "origin": "https://pay.example", "src": "https://pay.example/card",
                         "html": ["<input value=\"4111\">"]},
                        "</iframe><iframe title=\"Ad\">",
                        {"frame": 17_179_869_187i64, "url": "https://ads.example/",
                         "origin": "https://ads.example", "unread": "content_timeout",
                         "title": "Ad"},
                        "</iframe></body></html>"],
               "frames": {"read": 2, "unread": 1, "unread_frames": [
                   {"frame": 17_179_869_187i64, "origin": "https://ads.example",
                    "code": "content_timeout", "title": "Ad"}]}}),
    );
    let (err, text) = h.owner("page_dom", json!({})).await;
    assert!(!err, "{text}");
    assert_eq!(
        text,
        format!(
            "{HEAD}\n\
             <!DOCTYPE html><html><body><iframe src=\"https://consent.example\">\n\
             <!-- frame https://consent.example/ (its element's src \
             https://consent.example/?site=shop) begins -->\n\
             <p>Cookies?</p>\n\
             <!-- frame https://consent.example/ ends -->\n\
             </iframe><iframe src=\"https://pay.example\">\n\
             <!-- (a frame of a site Kai does not reach) -->\n\
             </iframe><iframe title=\"Ad\">\n\
             <!-- frame ads.example \"Ad\" could not be read here: it did not answer within Kai \
             for Firefox's setting content_ms -->\n\
             </iframe></body></html>\n\
             1 frame of the page could not be read: ads.example \"Ad\" (it did not answer within \
             Kai for Firefox's setting content_ms)"
        )
    );
    assert!(!text.contains("4111"));
    element(&ff, 41, 12, "region", "Consent area");
    answer(
        &ff,
        "element.dom@1",
        json!({"ref": 41, "tab": checkout(), "role": "region", "name": "Consent area",
               "html": ["<section aria-label=\"Consent area\"><h2>Consent</h2></section>"]}),
    );
    assert_eq!(
        h.owner("element_dom", json!({"ref": 41})).await,
        (
            false,
            format!(
                "{HEAD}\n41 region \"Consent area\":\n<section aria-label=\"Consent \
                 area\"><h2>Consent</h2></section>"
            )
        )
    );
    assert_eq!(sent(&ff, "element.dom@1"), json!({"ref": 41}));
}

/// A style sheet: what it fetches asked from the extension's parser first (`page.css_check@1`) and
/// named in the dialog; added at the page the dialog showed (its origin sent) letting Firefox fetch
/// from exactly the origins named, the page's own among them; every declaration made `!important`,
/// what was not understood listed. A fetch from a site the user never visited is asked about even
/// with `page_css` set to allow and for a call approved in lmgw; one from a `none` site or from an
/// opaque origin refuses the call; an answer naming a fetch Kai did not pass is the extension's
/// broken contract, and Kai takes the sheet out again. The extension's refusals in words. Removing
/// it: removed, gone with the page, its tab closed, unknown.
#[tokio::test]
async fn a_style_sheet_names_its_fetches_and_goes_away_again() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    ff.model()
        .unvisited
        .insert("https://cdn.example".to_owned());
    let css = "body { background: url(/img/bg.png) } .x { colr: red; \
               background: url(https://cdn.example/a.png); --u: \"https://cdn.example/b.png\" }";
    let fetches = json!([
        {"origin": "https://shop.example", "url": "https://shop.example/img/bg.png"},
        {"origin": "https://cdn.example", "url": "https://cdn.example/a.png"},
        {"origin": "https://cdn.example", "url": "https://cdn.example/b.png"}]);
    answer(
        &ff,
        "page.css_check@1",
        json!({"rules": 2, "dropped": ["colr: red"], "fetches": fetches}),
    );
    answer(
        &ff,
        "page.css@1",
        json!({"id": 3, "tab": checkout(), "rules": 2, "important": true,
               "dropped": ["colr: red"], "fetches": fetches}),
    );
    let mut tools = std::collections::BTreeMap::new();
    tools.insert("page_css".to_owned(), ToolState::Allow);
    h.set(
        PolicyConfig {
            tools: tools.clone(),
            ..Default::default()
        },
        BrowserConfig::default(),
    );
    let (err, text) = h.owner("page_css", json!({"css": css})).await;
    assert!(!err, "{text}");
    assert_eq!(
        text,
        format!(
            "{HEAD}: style sheet 3 added until the page is left; every declaration of its 2 rules \
             was made !important, since a sheet of Kai's beats the page's rules only so\n\
             not understood, left out: \"colr: red\"\n\
             fetches: https://shop.example/img/bg.png\n\
             fetches from another site: https://cdn.example/a.png\n\
             fetches from another site: https://cdn.example/b.png"
        )
    );
    assert_eq!(sent(&ff, "page.css_check@1"), json!({"css": css}));
    let asked = h.asked();
    assert_eq!(asked.len(), 1, "asked although page_css is set to allow");
    assert_eq!(
        asked[0].lines[1..],
        [
            "adds a style sheet to: 12 \"Checkout\" https://shop.example/cart",
            "fetches: https://shop.example/img/bg.png",
            "fetches from another site: https://cdn.example/a.png",
            "fetches from another site: https://cdn.example/b.png",
        ]
    );
    assert_eq!(asked[0].notes.len(), 1, "one question per site");
    assert!(
        asked[0].notes[0].starts_with(
            "Asked because Firefox's history has never seen https://cdn.example: the style sheet \
             fetches it"
        ),
        "{:?}",
        asked[0].notes
    );
    assert_eq!(
        sent(&ff, "page.css@1"),
        json!({"tab": 12, "css": css, "origin": "https://shop.example",
               "fetch_origins": ["https://shop.example", "https://cdn.example"]})
    );
    // Approved in lmgw by the owner, naming its tab: still asked about the site never visited.
    let (err, text) = h
        .approved("page_css", json!({"css": css, "tab": "12"}))
        .await;
    assert!(!err, "{text}");
    let asked = h.asked();
    assert_eq!(
        asked.len(),
        1,
        "a counted approval spares no sheet's question"
    );
    assert!(asked[0].notes[0].contains("https://cdn.example"));
    // A site the user has been to is not asked about; allowed, the call runs unasked.
    ff.model().unvisited.clear();
    assert!(!h.owner("page_css", json!({"css": css})).await.0);
    assert!(h.asked().is_empty());
    // A fetch from a none site, and one from an opaque origin: refused before the sheet is sent.
    h.set(PolicyConfig::default(), rules(&["cdn.example"], &[]));
    let before = ff.count("page.css@1");
    let (err, text) = h.owner("page_css", json!({"css": css})).await;
    assert_eq!(
        (err, text.as_str()),
        (
            true,
            "the style sheet fetches from a site Kai does not reach (Kai's site rules); page_css \
             did not run"
        )
    );
    h.set(PolicyConfig::default(), BrowserConfig::default());
    answer(
        &ff,
        "page.css_check@1",
        json!({"rules": 1, "dropped": [],
               "fetches": [{"origin": null, "url": "file:///home/ada/x.png"}]}),
    );
    let (err, text) = h
        .owner(
            "page_css",
            json!({"css": "p { background: url(file:///home/ada/x.png) }"}),
        )
        .await;
    assert_eq!(
        (err, text.as_str()),
        (
            true,
            "the style sheet fetches file:///home/ada/x.png, whose origin is opaque, so Kai cannot \
             let Firefox fetch it from a site the dialog names; page_css did not run"
        )
    );
    assert_eq!(ff.count("page.css@1"), before);
    // The extension's parser refuses: nothing understood, a var( ) where a file is named.
    fail(
        &ff,
        "page.css_check@1",
        json!({"code": "bad_css", "dropped": ["colr: red"],
               "message": "nothing in the style sheet was understood"}),
    );
    assert_eq!(
        h.owner("page_css", json!({"css": "colr: red"})).await,
        (
            true,
            "nothing of the style sheet was understood, so nothing was added (nothing in the \
             style sheet was understood); not understood: \"colr: red\""
                .to_owned()
        )
    );
    fail(
        &ff,
        "page.css_check@1",
        json!({"code": "css_var_url", "uses": ["image-set(var(--u) 1x)"],
               "message": "the style sheet uses var( ) inside url( ), image-set( ) or another \
                           function that loads a file (listed in uses), so what it would load is \
                           known only once the page is styled; write the URL itself"}),
    );
    assert_eq!(
        h.owner(
            "page_css",
            json!({"css": "p { background: image-set(var(--u) 1x) }"})
        )
        .await,
        (
            true,
            "the style sheet loads a file through var( ) in \"image-set(var(--u) 1x)\", so what \
             it would load is known only once the page is styled, and nothing was added (the \
             style sheet uses var( ) inside url( ), image-set( ) or another function that loads \
             a file (listed in uses), so what it would load is known only once the page is \
             styled; write the URL itself); write each file's URL itself, so the dialog can name \
             it"
            .to_owned()
        )
    );
    assert_eq!(ff.count("page.css@1"), before);
    ff.model().twists.remove("page.css_check@1");
    // page.css@1 refuses fetches Kai did not pass (the extension's parser read the sheet
    // differently at the second look).
    fail(
        &ff,
        "page.css@1",
        json!({"code": "fetch_not_approved", "tab": 12, "title": "Checkout",
               "url": "https://shop.example/cart",
               "fetches": [{"origin": "https://late.example", "url": "https://late.example/b.png"}],
               "message": "the style sheet for tab 12 loads files from origins not in \
                           fetch_origins, so it was not added"}),
    );
    assert_eq!(
        h.owner("page_css", json!({"css": "p { color: red }"}))
            .await,
        (
            true,
            "Kai for Firefox found the style sheet loading files from origins the dialog did not \
             name (https://late.example/b.png), so nothing was added (the style sheet for tab 12 \
             loads files from origins not in fetch_origins, so it was not added); Kai and the \
             extension read the sheet differently, which an update of either may settle"
                .to_owned()
        )
    );
    assert_eq!(
        sent(&ff, "page.css@1")["fetch_origins"],
        json!([]),
        "a sheet that fetches nothing lets Firefox fetch nothing"
    );
    // An answer naming a fetch Kai did not pass: Kai takes the sheet out again.
    answer(
        &ff,
        "page.css@1",
        json!({"id": 5, "tab": checkout(), "rules": 1, "important": true, "dropped": [],
               "fetches": [{"origin": "https://late.example", "url": "https://late.example/b.png"}]}),
    );
    answer(
        &ff,
        "page.css_remove@1",
        json!({"id": 5, "tab": checkout(), "removed": true}),
    );
    assert_eq!(
        h.owner("page_css", json!({"css": "p { color: red }"}))
            .await,
        (
            true,
            format!(
                "{HEAD}: Kai for Firefox added style sheet 5 although it loads files from origins \
                 the dialog did not name (https://late.example/b.png), which page.css@1 refuses by \
                 its contract; Kai took the sheet out again (an update of Kai for Firefox fixes \
                 this)"
            )
        )
    );
    assert_eq!(sent(&ff, "page.css_remove@1"), json!({"id": 5}));
    // Nothing understood.
    fail(
        &ff,
        "page.css@1",
        json!({"code": "bad_css", "tab": 12, "title": "Checkout",
               "url": "https://shop.example/cart", "dropped": ["colr: red"],
               "message": "nothing of the style sheet was understood"}),
    );
    assert!(
        h.owner("page_css", json!({"css": "colr: red"}))
            .await
            .1
            .starts_with("nothing of the style sheet was understood, so nothing was added")
    );
    // The page moved on while the dialog waited.
    fail(
        &ff,
        "page.css@1",
        json!({"code": "page_changed", "tab": 12, "title": "Lamps",
               "url": "https://shop.example/lamps",
               "message": "tab 12 left the page Kai named"}),
    );
    assert!(
        h.owner("page_css", json!({"css": "p { color: red }"}))
            .await
            .1
            .contains("call again for the page it shows now")
    );
    // Sheet 3 is Kai's again for the removals below.
    answer(
        &ff,
        "page.css@1",
        json!({"id": 3, "tab": checkout(), "rules": 1, "important": true, "dropped": [],
               "fetches": []}),
    );
    assert!(
        !h.owner("page_css", json!({"css": "p { color: red }"}))
            .await
            .0
    );

    // Removing sheet 3, which Kai added to tab 12.
    let _ = h.asked();
    answer(
        &ff,
        "page.css_remove@1",
        json!({"id": 3, "tab": checkout(), "removed": true}),
    );
    assert_eq!(
        h.owner("page_css_remove", json!({"id": 3})).await,
        (false, format!("{HEAD}: style sheet 3 removed"))
    );
    assert_eq!(sent(&ff, "page.css_remove@1"), json!({"id": 3}));
    assert_eq!(
        h.asked()[0].lines,
        [
            "id: 3",
            "removes style sheet 3 from: 12 \"Checkout\" https://shop.example/cart"
        ]
    );
    answer(
        &ff,
        "page.css_remove@1",
        json!({"id": 3, "tab": checkout(), "removed": false, "why": "page_left"}),
    );
    assert_eq!(
        h.owner("page_css_remove", json!({"id": 3})).await.1,
        format!(
            "{HEAD}: style sheet 3 was gone already: the page it was added to was left, which \
             took it out"
        )
    );
    answer(
        &ff,
        "page.css_remove@1",
        json!({"id": 3, "tab": {"id": 12, "title": "", "url": ""}, "removed": false,
               "why": "tab_closed"}),
    );
    assert_eq!(
        h.owner("page_css_remove", json!({"id": 3})).await.1,
        "style sheet 3 is gone: tab 12 closed a moment ago, which took it out"
    );
    fail(
        &ff,
        "page.css_remove@1",
        json!({"code": "unknown_css", "message": "no style sheet 3"}),
    );
    assert_eq!(
        h.owner("page_css_remove", json!({"id": 3})).await,
        (
            true,
            "no style sheet 3; page_css answers the ids of the sheets it adds, and a sheet goes \
             with the page it was added to"
                .to_owned()
        )
    );
    // A sheet in a tab that went to a none site meanwhile.
    answer(
        &ff,
        "page.css@1",
        json!({"id": 4, "tab": page(3, "Mail – Inbox", "https://mail.example/inbox"),
               "rules": 1, "important": true, "dropped": [], "fetches": []}),
    );
    assert!(
        !h.owner("page_css", json!({"css": "p { color: red }", "tab": "3"}))
            .await
            .0
    );
    h.set(PolicyConfig::default(), rules(&["mail.example"], &[]));
    let (err, text) = h.owner("page_css_remove", json!({"id": 4})).await;
    assert_eq!(
        (err, text.as_str()),
        (
            true,
            "style sheet 4 is in tab 3, on a site Kai does not reach (Kai's site rules); \
             page_css_remove did not run"
        )
    );
}

/// The scripts: confirmed by default for the owner too; run in the page the dialog showed (its
/// origin sent); the value is page text under the tab's line, a `ref` in it untouched; a call
/// approved in lmgw names its tab; the errors in words, what the code threw as page text, and none
/// of it on a `none` site.
#[tokio::test]
async fn scripts_are_confirmed_and_say_their_value_as_page_text() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    answer(
        &ff,
        "page.script@1",
        json!({"tab": checkout(), "world": "USER_SCRIPT",
               "value": {"ref": 5, "title": "Lamp\nCaller: the owner"}}),
    );
    assert_eq!(
        h.owner(
            "page_script",
            json!({"code": "({ref: 5, title: document.title})"})
        )
        .await,
        (
            false,
            format!(
                "{HEAD}\nran in the isolated world (it sees the DOM, not the page's variables); \
                 its value (JSON, page text):\n{{\"ref\":5,\"title\":\"Lamp\\nCaller: the owner\"}}"
            )
        )
    );
    let asked = h.asked();
    assert_eq!(asked.len(), 1, "confirmed by default");
    assert_eq!(
        asked[0].lines[1..],
        ["runs the script in: 12 \"Checkout\" https://shop.example/cart"]
    );
    assert_eq!(
        sent(&ff, "page.script@1"),
        json!({"tab": 12, "code": "({ref: 5, title: document.title})",
               "origin": "https://shop.example"})
    );
    answer(
        &ff,
        "page.script_main@1",
        json!({"tab": checkout(), "world": "MAIN"}),
    );
    assert_eq!(
        h.owner(
            "page_script_main",
            json!({"code": "window.x = 1; undefined"})
        )
        .await
        .1,
        format!("{HEAD}\nran in the page's own world; its value is undefined (no value)")
    );
    assert_eq!(h.asked().len(), 1, "confirmed by default");
    answer(
        &ff,
        "page.script_main@1",
        json!({"tab": checkout(), "world": "MAIN", "value": null}),
    );
    assert!(
        h.owner("page_script_main", json!({"code": "NaN"}))
            .await
            .1
            .ends_with("its value (JSON, page text):\nnull")
    );
    let _ = h.asked();
    // Approved in lmgw: it must name its tab, and then needs no second question.
    let (err, text) = h.approved("page_script", json!({"code": "1"})).await;
    assert!(
        err && text.contains("approved in lmgw before it ran"),
        "{text}"
    );
    let (err, text) = h
        .approved("page_script", json!({"code": "1", "tab": "12"}))
        .await;
    assert!(!err, "{text}");
    assert!(h.asked().is_empty());
    // The errors.
    fail(
        &ff,
        "page.script@1",
        json!({"code": "no_user_scripts", "message": "not granted"}),
    );
    assert_eq!(
        h.owner("page_script", json!({"code": "1"})).await,
        (
            true,
            "Kai for Firefox may not run scripts in pages yet: allow user scripts in Kai for \
             Firefox's options (its button \"Allow user scripts\"; Firefox grants it only there)"
                .to_owned()
        )
    );
    fail(
        &ff,
        "page.script@1",
        json!({"code": "script_error", "tab": 12, "title": "Checkout",
               "url": "https://shop.example/cart", "message": "the script in tab 12 threw TypeError",
               "thrown": {"name": "TypeError", "message": "x is null", "line": 2, "column": 7}}),
    );
    assert_eq!(
        h.owner("page_script", json!({"code": "x.y"})).await,
        (
            true,
            format!("{HEAD}\nthe script threw TypeError: \"x is null\" (line 2, column 7)")
        )
    );
    fail(
        &ff,
        "page.script@1",
        json!({"code": "not_cloneable", "tab": 12, "title": "Checkout",
               "url": "https://shop.example/cart",
               "message": "the value cannot leave tab 12"}),
    );
    assert_eq!(
        h.owner("page_script", json!({"code": "document.body"}))
            .await
            .1,
        "the value cannot leave tab 12; end the code with plain data (no DOM node, function or \
         window), which comes back as JSON"
    );
    fail(
        &ff,
        "page.script@1",
        json!({"code": "not_json", "tab": 12, "title": "Checkout",
               "url": "https://shop.example/cart",
               "message": "BigInt value can't be serialized in JSON"}),
    );
    assert!(
        h.owner("page_script", json!({"code": "1n"}))
            .await
            .1
            .ends_with("; end the code with plain data that JSON can write")
    );
    // On a none site: neither what was thrown nor the value is said.
    h.set(PolicyConfig::default(), rules(&["mail.example"], &[]));
    fail(
        &ff,
        "page.script@1",
        json!({"code": "script_error", "tab": 12, "title": "Inbox",
               "url": "https://mail.example/inbox", "message": "the script in tab 12 threw Error",
               "thrown": {"value": "secret mail text"}}),
    );
    let (err, text) = h.owner("page_script", json!({"code": "throw 1"})).await;
    assert!(err && !text.contains("secret"), "{text}");
    assert_eq!(
        text,
        "tab 12 is on a site Kai does not reach (Kai's site rules), so of what failed Kai says \
         only: the script failed"
    );
    answer(
        &ff,
        "page.script@1",
        json!({"tab": page(12, "Inbox", "https://mail.example/inbox"), "world": "USER_SCRIPT",
               "value": "secret mail text"}),
    );
    let (err, text) = h.owner("page_script", json!({"code": "1"})).await;
    assert!(err && !text.contains("secret"), "{text}");
}

/// A PNG of `w` by `h` white pixels, base64.
fn white(w: u32, h: u32) -> String {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().unwrap();
    wr.write_image_data(&vec![0xFF; (w * h * 4) as usize])
        .unwrap();
    wr.finish().unwrap();
    base64::engine::general_purpose::STANDARD.encode(out)
}

/// A reply's text and its image's pixels (RGB or RGBA as encoded) with their channel count.
fn shot(r: &crate::host::registry::Reply) -> (String, Vec<u8>, usize) {
    let [Block::Text(text), Block::Image { mime, base64 }] = &r.blocks[..] else {
        panic!("{:?}", r.blocks);
    };
    assert_eq!(mime, "image/png");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64)
        .unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (text.clone(), buf, info.color_type.samples())
}

/// The screenshots: a line and the image; a frame of a `none` site blanked out grey before the
/// image leaves Kai; without the frames' places and with a `none` rule, no image; an element's
/// box, scroll and clipping; an element over frames (a frame that showed a `none` site before the
/// capture, one whose place its parent could not say); an answer without its image, or of another
/// size than it says; the page changed meanwhile.
#[tokio::test]
async fn screenshots_come_as_images_with_the_none_frames_blanked() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    let png = white(4, 4);
    answer(
        &ff,
        "page.see@1",
        json!({"tab": checkout(), "png": png, "width": 4, "height": 4,
               "view": {"width": 4, "height": 4}, "scale": {"x": 1, "y": 1}, "frames": []}),
    );
    let r = h.reply("page_see", json!({})).await;
    assert!(!r.error, "{r:?}");
    let (text, px, _) = shot(&r);
    assert_eq!(
        text,
        format!(
            "{HEAD}: the visible part, 4x4 pixels for 4x4 CSS pixels (scale 1: image pixels per \
             CSS pixel)"
        )
    );
    assert!(px.iter().all(|b| *b == 0xFF), "passed through");
    assert!(h.asked().is_empty(), "a read runs unasked for the owner");
    assert_eq!(sent(&ff, "page.see@1"), json!({"tab": 12}));

    h.set(PolicyConfig::default(), rules(&["pay.example"], &[]));
    answer(
        &ff,
        "page.see@1",
        json!({"tab": checkout(), "png": png, "width": 4, "height": 4,
               "view": {"width": 2.6667, "height": 2.6667}, "scale": {"x": 1.5, "y": 1.5},
               "not_hidden": [{"frame": 9, "code": "content_timeout"}],
               "frames": [{"frame": 7, "origin": "https://pay.example",
                           "url": "https://pay.example/card",
                           "box": {"x": 2, "y": 0, "width": 2, "height": 2}},
                          {"frame": 8, "origin": "https://consent.example",
                           "url": "https://consent.example/",
                           "box": {"x": 0, "y": 2, "width": 2, "height": 2}}]}),
    );
    let (text, px, n) = shot(&h.reply("page_see", json!({})).await);
    assert_eq!(
        text,
        format!(
            "{HEAD}: the visible part, 4x4 pixels for 2.6667x2.6667 CSS pixels (scale 1.5: image \
             pixels per CSS pixel)\n\
             1 frame of a site Kai does not reach is blanked out grey\n\
             Kai's hint labels or ring may show in 1 frame that did not answer in time to hide \
             them (Kai for Firefox's setting content_ms)"
        )
    );
    let at = |x: usize, y: usize| px[(y * 4 + x) * n];
    assert_eq!((at(2, 0), at(3, 1)), (0x80, 0x80), "the pay frame");
    assert_eq!((at(0, 0), at(0, 2), at(3, 3)), (0xFF, 0xFF, 0xFF));
    // Without where the frames are, and a none site among the rules: no image.
    answer(
        &ff,
        "page.see@1",
        json!({"tab": checkout(), "png": png, "width": 4, "height": 4,
               "view": {"width": 4, "height": 4}, "scale": {"x": 1, "y": 1}}),
    );
    let r = h.reply("page_see", json!({})).await;
    assert!(r.error);
    assert!(
        matches!(&r.blocks[..], [Block::Text(t)] if t.starts_with("Kai for Firefox did not say where the page's frames are")),
        "{:?}",
        r.blocks
    );
    // The page went to a none site while it was captured.
    answer(
        &ff,
        "page.see@1",
        json!({"tab": page(12, "Pay", "https://pay.example/card"), "png": png, "width": 4,
               "height": 4, "view": {"width": 4, "height": 4}, "scale": {"x": 1, "y": 1},
               "frames": []}),
    );
    let r = h.reply("page_see", json!({})).await;
    assert!(r.error && r.blocks.len() == 1, "{:?}", r.blocks);
    fail(
        &ff,
        "page.see@1",
        json!({"code": "not_rendered", "tab": 14, "title": "Lamps",
               "url": "https://shop.example/lamps",
               "message": "Firefox has not rendered tab 14"}),
    );
    assert_eq!(
        h.owner("page_see", json!({"tab": "14"})).await,
        (
            true,
            "Firefox has not rendered tab 14; tab_switch shows the tab, and then it can be seen"
                .to_owned()
        )
    );

    // One element, as Kai numbers it.
    element(&ff, 52, 12, "img", "");
    answer(
        &ff,
        "element.see@1",
        json!({"ref": 52, "tab": checkout(), "role": "img", "name": "", "png": white(2, 1),
               "width": 2, "height": 1, "box": {"x": 100, "y": 100.5, "width": 2, "height": 1},
               "view": {"width": 1280, "height": 900}, "scale": {"x": 1, "y": 1},
               "scrolled": true, "clipped": true, "frames": []}),
    );
    let (text, _, _) = shot(&h.reply("element_see", json!({"ref": 52})).await);
    assert_eq!(
        text,
        format!(
            "{HEAD}: 52 img, 2x1 pixels (scale 1: image pixels per CSS pixel); its part in view at \
             100,100.5, 2x1 CSS pixels of the 1280x900 view; scrolled into view first; part of \
             it is outside the view or cut off, so the image shows the part in view"
        )
    );
    assert_eq!(sent(&ff, "element.see@1"), json!({"ref": 52}));
    // An element over frames, boxes in the crop's pixels: a none frame, one that showed a none
    // site before the capture, one whose place its parent could not say (the part of the frame
    // around it), and one of a site Kai reaches.
    answer(
        &ff,
        "element.see@1",
        json!({"ref": 52, "tab": checkout(), "role": "img", "name": "", "png": png,
               "width": 4, "height": 4, "box": {"x": 10, "y": 10, "width": 4, "height": 4},
               "view": {"width": 1280, "height": 900}, "scale": {"x": 1, "y": 1},
               "scrolled": false,
               "frames": [{"frame": 7, "origin": "https://pay.example",
                           "url": "https://pay.example/card",
                           "box": {"x": 0, "y": 0, "width": 1, "height": 1}},
                          {"frame": 8, "origin": "https://consent.example",
                           "url": "https://consent.example/", "origin_before": "https://pay.example",
                           "box": {"x": 3, "y": 0, "width": 1, "height": 1},
                           "frames": [{"frame": 9, "origin": "https://pay.example",
                                       "url": "https://pay.example/inner", "unmeasured": true,
                                       "box": {"x": 3, "y": 0, "width": 1, "height": 1}}]},
                          {"frame": 10, "origin": "https://consent.example",
                           "url": "https://consent.example/",
                           "box": {"x": 0, "y": 3, "width": 4, "height": 1}},
                          {"frame": 11, "origin": "https://pay.example",
                           "url": "https://pay.example/off", "box": null}]}),
    );
    let (text, px, n) = shot(&h.reply("element_see", json!({"ref": 52})).await);
    assert_eq!(
        text,
        format!(
            "{HEAD}: 52 img, 4x4 pixels (scale 1: image pixels per CSS pixel); its part in view at \
             10,10, 4x4 CSS pixels of the 1280x900 view\n\
             3 frames of sites Kai does not reach are blanked out grey"
        )
    );
    let at = |x: usize, y: usize| px[(y * 4 + x) * n];
    assert_eq!((at(0, 0), at(3, 0)), (0x80, 0x80));
    assert_eq!(
        (at(1, 0), at(0, 3), at(3, 3), at(1, 1)),
        (0xFF, 0xFF, 0xFF, 0xFF)
    );
    // An answer without its image.
    answer(
        &ff,
        "page.see@1",
        json!({"tab": checkout(), "width": 4, "height": 4,
               "view": {"width": 4, "height": 4}, "scale": {"x": 1, "y": 1}, "frames": []}),
    );
    assert_eq!(
        h.owner("page_see", json!({})).await,
        (
            true,
            "Kai for Firefox answered page.see@1 without an image (an update of the extension \
             may be needed)"
                .to_owned()
        )
    );
    // An image of another size than the answer says, with a frame to blank: not shown.
    answer(
        &ff,
        "page.see@1",
        json!({"tab": checkout(), "png": png, "width": 8, "height": 4,
               "view": {"width": 8, "height": 4}, "scale": {"x": 1, "y": 1},
               "frames": [{"frame": 7, "origin": "https://pay.example",
                           "url": "https://pay.example/card",
                           "box": {"x": 0, "y": 0, "width": 1, "height": 1}}]}),
    );
    let (err, text) = h.owner("page_see", json!({})).await;
    assert!(
        err && text.starts_with("Kai for Firefox named its image 8x4 pixels, but the image is 4x4"),
        "{text}"
    );
    fail(
        &ff,
        "element.see@1",
        json!({"code": "page_changed", "tab": 12, "title": "Checkout",
               "url": "https://shop.example/cart",
               "message": "tab 12 showed another page when it was captured"}),
    );
    assert_eq!(
        h.owner("element_see", json!({"ref": 52})).await,
        (
            true,
            "tab 12 showed another page when it was captured; the tab shows another page now \
             than the one the call was for, so nothing was done or shown: call again for the \
             page it shows now"
                .to_owned()
        )
    );
}
