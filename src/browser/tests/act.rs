//! The acting tools (design §5.4, WP6) through the host's dispatch against the fake Kai for
//! Firefox: each tool's operation with the commit guard and its lines; the page side's question
//! and the re-sent confirmed call; the network gate's questions; the trusted path's order and its
//! refusals; the site rules and the focus rule on a ref's tab; the hands-and-eyes switch.

use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::fake::{Firefox, Host, Twist, WP4A, WP4B, all_ops, shop, stranger};
use crate::browser::settings::{BrowserConfig, Sites, TrustedPress};
use crate::host::policy::Caller;
use crate::host::settings::{PolicyConfig, ToolState};

fn switch_on() -> BrowserConfig {
    BrowserConfig {
        hands_and_eyes: true,
        ..Default::default()
    }
}

const HEAD: &str = "\"Checkout\" https://shop.example/cart (tab 12)";

fn element(ff: &Firefox, r: i64, tab: i64, role: &str, name: &str) {
    ff.model()
        .elements
        .insert(r, json!({"tab": tab, "role": role, "name": name}));
}

/// Sets `key` of element `r` (`guard`, `needs_trusted`, `focus`, `answer`, …).
fn with(ff: &Firefox, r: i64, key: &str, v: Value) {
    ff.model().elements.get_mut(&r).unwrap()[key] = v;
}

/// The last operation that arrived, and its arguments.
fn last(ff: &Firefox) -> (String, Value) {
    ff.acted().last().cloned().unwrap()
}

/// A POST the page sends after an action.
fn post(url: &str) -> Value {
    json!({"tab": 12, "frame": 0, "origin": "https://shop.example", "method": "POST", "url": url,
           "type": "xmlhttprequest",
           "body": {"kind": "form", "fields": [["item", "trail-lamp"],
                                               ["password", "password field, filled (16 characters)"]]}})
}

/// The acting tools are listed while their operations are served; `element_press` needs the
/// focus check and the gate besides its own (the trusted path), `element_show` no listing.
#[tokio::test]
async fn the_acting_tools_are_listed_by_their_operations() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    let names = h.names();
    for t in [
        "element_press",
        "element_fill",
        "element_choose",
        "element_check",
        "element_file",
        "element_focus",
        "element_hover",
        "element_drag",
        "element_show",
        "element_scroll",
        "page_scroll",
        "page_key",
    ] {
        assert!(names.contains(&t), "{t}: {names:?}");
    }
    ff.quit();
    h.rig.links_are(0).await;
    let mut ops: Vec<&str> = WP4A.iter().chain(WP4B).copied().collect();
    ops.extend(["element.press@1", "element.fill@1"]);
    let _ff = h.firefox("c0ffee", &ops, shop()).await;
    let names = h.names();
    assert!(names.contains(&"element_fill"));
    assert!(
        !names.contains(&"element_press"),
        "no focus check, no trusted path"
    );
}

#[tokio::test]
async fn each_acting_tool_sends_the_guard_and_says_what_it_did() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 41, 12, "textbox", "Full name");
    element(&ff, 50, 12, "combobox", "Country");
    element(&ff, 55, 12, "checkbox", "Gift wrap");
    element(&ff, 61, 12, "textbox", "Notes");
    element(&ff, 63, 12, "button", "Account");
    element(&ff, 66, 12, "button", "Full screen");
    element(&ff, 70, 12, "listitem", "Tent");
    element(&ff, 72, 12, "listitem", "Pack");
    element(&ff, 80, 12, "generic", "Terms text");
    element(&ff, 52, 12, "button", "Edit address");

    assert_eq!(
        h.owner("element_press", json!({"ref": 52})).await,
        (false, format!("{HEAD}: pressed 52 button \"Edit address\""))
    );
    let (op, args) = last(&ff);
    assert_eq!(op, "element.press@1");
    assert_eq!(args["ref"], 52);
    assert_eq!(args["guard"]["window_ms"], 2000);
    assert_eq!(
        args["guard"]["answer_ms"], 2_147_483_647,
        "no deadline: the contract's most"
    );
    let words: Vec<&str> = args["guard"]["words"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        words.contains(&"buy") && words.contains(&"überweis"),
        "{words:?}"
    );
    assert!(args.get("confirmed").is_none());

    assert_eq!(
        h.owner("element_fill", json!({"ref": 41, "text": "Ada Example"}))
            .await
            .1,
        format!(
            "{HEAD}: filled 41 textbox \"Full name\"\nnow: 41 textbox \"Full name\" = \"Ada \
             Example\""
        )
    );
    assert_eq!(last(&ff).1["text"], "Ada Example");
    // A sensitive field's value never comes back.
    with(
        &ff,
        61,
        "answer",
        json!({"masked": true, "length": 16, "value": null, "how": "keys", "took": false,
               "note": "the page cancelled the input"}),
    );
    assert_eq!(
        h.owner(
            "element_fill",
            json!({"ref": 61, "text": "hunter2hunter2!!"})
        )
        .await
        .1,
        format!(
            "{HEAD}: filled 61 textbox \"Notes\", typing key by key, so a list of suggestions \
             may be open\nthe field does not show the text afterwards\nnow: 61 textbox \
             \"Notes\": password field, filled (16 characters)\nnote: the page cancelled the \
             input"
        )
    );
    assert_eq!(
        h.owner("element_choose", json!({"ref": 50, "option": "Austria"}))
            .await
            .1,
        format!(
            "{HEAD}: chose \"Austria\" in 50 combobox \"Country\"\nnow: 50 combobox \
             \"Country\" = \"Austria\""
        )
    );
    assert_eq!(
        h.owner("element_check", json!({"ref": 55, "on": true}))
            .await
            .1,
        format!(
            "{HEAD}: checked 55 checkbox \"Gift wrap\"\nnow: 55 checkbox \"Gift wrap\" (checked)"
        )
    );
    assert_eq!(last(&ff).1["on"], true);
    with(&ff, 55, "answer", json!({"changed": false}));
    assert!(
        h.owner("element_check", json!({"ref": 55, "on": true}))
            .await
            .1
            .contains("55 checkbox \"Gift wrap\" was checked already; nothing changed")
    );
    assert_eq!(
        h.owner("element_focus", json!({"ref": 61})).await.1,
        format!(
            "{HEAD}: moved the keyboard focus to 61 textbox \"Notes\"\nnow: 61 textbox \
             \"Notes\": password field, filled (16 characters)\nnote: the page cancelled the \
             input"
        )
    );
    with(
        &ff,
        63,
        "answer",
        json!({"expanded": true, "scrolled": true}),
    );
    assert_eq!(
        h.owner("element_hover", json!({"ref": 63})).await.1,
        format!(
            "{HEAD}: scrolled 63 button \"Account\" into view and moved the pointer over 63 \
             button \"Account\"\nnow: 63 button \"Account\" (expanded)"
        )
    );
    assert_eq!(
        h.owner("element_drag", json!({"ref": 70, "to": 72}))
            .await
            .1,
        format!("{HEAD}: dragged 70 listitem \"Tent\" onto 72 listitem \"Pack\"")
    );
    assert_eq!(last(&ff).1["to"], 72);
    assert_eq!(
        h.owner("element_show", json!({"ref": 66})).await.1,
        format!("{HEAD}: scrolled 66 button \"Full screen\" into view")
    );
    assert!(last(&ff).1.get("guard").is_none(), "a view operation");
    assert_eq!(
        h.owner("element_scroll", json!({"ref": 80, "to": "Down"}))
            .await
            .1,
        format!("{HEAD}: scrolled 80 generic \"Terms text\" down: 18 % down")
    );
    assert_eq!(last(&ff).1["to"], "down");
    let (err, text) = h
        .owner("element_scroll", json!({"ref": 80, "to": "sideways"}))
        .await;
    assert!(err && text.contains("down, up, top or bottom"), "{text}");
    assert_eq!(
        h.owner("page_scroll", json!({"to": "bottom"})).await.1,
        format!("{HEAD}: scrolled the page bottom: 40 % down")
    );
    assert_eq!(last(&ff).1["tab"], 12);
    assert_eq!(
        h.owner("page_key", json!({"keys": "Escape ctrl+k"}))
            .await
            .1,
        format!("{HEAD}: sent 2 keys\nEscape to the page's body\nctrl+k to the page's body")
    );
    let keys: Vec<Value> = ff
        .acted()
        .iter()
        .filter(|(o, _)| o == "page.key@1")
        .map(|(_, a)| a["keys"].clone())
        .collect();
    assert_eq!(keys, [json!("Escape"), json!("ctrl+k")], "one key a call");

    // The guard off: "off", never left out.
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            commit_guard: false,
            ..switch_on()
        },
    );
    h.owner("element_press", json!({"ref": 52})).await;
    assert_eq!(last(&ff).1["guard"], "off");
}

#[tokio::test]
async fn element_file_reads_the_file_and_sends_it_whole() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 57, 12, "generic", "Receipt");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("receipt.txt");
    std::fs::write(&path, "the receipt's text\n").unwrap();
    let p = path.to_str().unwrap();
    let (err, text) = h.owner("element_file", json!({"ref": 57, "path": p})).await;
    assert!(!err, "{text}");
    assert_eq!(
        text,
        format!("{HEAD}: gave 57 generic \"Receipt\" the file \"receipt.txt\" (19 bytes)")
    );
    // Confirmed by default (K24), with the switch on too: the dialog names the file.
    let asked = h.asked();
    assert_eq!(asked.len(), 1);
    assert!(
        asked[0]
            .lines
            .iter()
            .any(|l| l.starts_with("sends: \"receipt.txt\" (19 bytes")),
        "{:?}",
        asked[0].lines
    );
    let (_, args) = last(&ff);
    let f = &args["files"][0];
    assert_eq!(f["name"], "receipt.txt");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(f["base64"].as_str().unwrap())
        .unwrap();
    assert_eq!(bytes, b"the receipt's text\n");
    // A file that is not there is refused before anyone is asked.
    let (err, text) = h
        .owner(
            "element_file",
            json!({"ref": 57, "path": "/nonexistent/x.pdf"}),
        )
        .await;
    assert!(
        err && text.starts_with("Kai cannot read /nonexistent/x.pdf"),
        "{text}"
    );
    assert!(h.asked().is_empty());
}

/// The page side (§6.4): a press the guard stops is asked about, naming the tab, the element and
/// the reason, and sent again confirmed; declined, nothing is sent again.
#[tokio::test]
async fn the_page_side_asks_and_a_yes_sends_the_press_again_confirmed() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 52, 12, "button", "Buy now");
    let buy = json!({"reason": "word", "word": "buy", "where": "name", "text": "Buy now"});
    with(&ff, 52, "guard", json!([buy]));
    assert_eq!(
        h.owner("element_press", json!({"ref": 52})).await,
        (false, format!("{HEAD}: pressed 52 button \"Buy now\""))
    );
    let asked = h.asked();
    assert_eq!(
        asked.len(),
        1,
        "only the guard asks: the switch lets the owner act"
    );
    assert_eq!(asked[0].tool, "element_press (the commit guard)");
    assert!(!asked[0].always);
    assert_eq!(
        asked[0].lines,
        [
            format!("tab: {HEAD}"),
            "element: 52 button \"Buy now\"".into(),
            "why: its name \"Buy now\" holds the guard's word \"buy\"".into(),
        ]
    );
    let presses: Vec<Value> = ff
        .acted()
        .iter()
        .filter(|(o, _)| o == "element.press@1")
        .map(|(_, a)| a.get("confirmed").cloned().unwrap_or(Value::Null))
        .collect();
    assert_eq!(
        presses,
        [Value::Null, json!([buy])],
        "the verdict as the guard gave it"
    );

    // Declined: nothing is sent again.
    h.answers(&[Some(false)]);
    let n = ff.acted().len();
    assert_eq!(
        h.owner("element_press", json!({"ref": 52})).await,
        (
            true,
            format!(
                "{HEAD}: 52 button \"Buy now\" was not pressed: the owner declined it (the \
                 commit guard asked because its name \"Buy now\" holds the guard's word \"buy\")"
            )
        )
    );
    assert_eq!(ff.acted().len(), n + 2, "the describe and the one press");
    // An lmgw approval of the press does not spare the guard's question: its approver saw a ref.
    let _ = h.asked();
    h.approved("element_press", json!({"ref": 52})).await;
    assert_eq!(h.asked().len(), 1);
}

/// The network gate (§6.4): each request the page sends after the action is asked about with its
/// method, URL and body, and goes only on a yes; nobody answering cancels it.
#[tokio::test]
async fn the_network_gates_requests_are_asked_about() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 53, 12, "button", "Add to cart");
    ff.model().asks = vec![post("https://shop.example/cart?token=s3cret")];
    let (err, text) = h.owner("element_press", json!({"ref": 53})).await;
    assert!(!err, "{text}");
    assert_eq!(
        text,
        format!(
            "{HEAD}: pressed 53 button \"Add to cart\"\na POST to https://shop.example/cart \
             (xmlhttprequest; its query left out) went: the owner allowed it"
        )
    );
    assert_eq!(ff.model().answers, [true]);
    let asked = h.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].tool, "element_press (a request the page sends)");
    assert_eq!(
        asked[0].lines,
        [
            format!("after Kai acted on: 53 button \"Add to cart\" in {HEAD}"),
            "request: POST https://shop.example/cart?token=s3cret".into(),
            "sent by: https://shop.example".into(),
            "kind: xmlhttprequest".into(),
            "field \"item\": \"trail-lamp\"".into(),
            "field \"password\": \"password field, filled (16 characters)\"".into(),
        ]
    );
    // Declined.
    h.answers(&[Some(false)]);
    ff.model().asks = vec![post("https://shop.example/cart")];
    let text = h.owner("element_press", json!({"ref": 53})).await.1;
    assert!(
        text.ends_with(
            "a POST to https://shop.example/cart (xmlhttprequest) was cancelled: the owner \
             declined it"
        ),
        "{text}"
    );
    assert_eq!(ff.model().answers, [true, false]);
    // Nobody at the desk: the gate's own time ends it.
    {
        let mut m = ff.model();
        m.asks = vec![post("https://shop.example/cart")];
        m.gate_times_out = true;
    }
    h.answers(&[None]);
    let text = h.owner("element_press", json!({"ref": 53})).await.1;
    assert!(
        text.ends_with(
            "was cancelled: nobody answered within 2147483647 ms (the call's time less the \
             commit guard's watch, browser.guard_window_ms)"
        ),
        "{text}"
    );
    // A request to a site Kai does not reach is not named to the model.
    h.answers(&[]);
    {
        let mut m = ff.model();
        m.asks = vec![post("https://mail.example/send")];
        m.gate_times_out = false;
    }
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            sites: Sites {
                none: vec!["mail.example".into()],
                ..Default::default()
            },
            ..switch_on()
        },
    );
    let text = h.owner("element_press", json!({"ref": 53})).await.1;
    assert!(
        text.ends_with(
            "a POST to a site Kai does not reach (Kai's site rules) (xmlhttprequest) went: the \
             owner allowed it"
        ),
        "{text}"
    );
}

/// A question that nobody answers counts as declined at the call's time less the margin (paused
/// time: the clock moves only to the dialog's end).
#[tokio::test(start_paused = true)]
async fn a_question_nobody_answers_is_declined_when_the_calls_time_is_up() {
    struct Silent;
    impl crate::host::confirm::Confirmer for Silent {
        fn ask(&self, req: crate::host::confirm::ConfirmRequest) {
            std::mem::forget(req.reply);
        }
    }
    let q = crate::browser::tools::guard::Question {
        tool: "element_press (a request the page sends)".into(),
        description: String::new(),
        lines: Vec::new(),
        notes: Vec::new(),
    };
    let soon = tokio::time::Instant::now() + Duration::from_millis(600);
    let a = crate::browser::tools::guard::ask_owner_spoken(
        &Silent,
        &Caller::Owner,
        q,
        None,
        Some(soon),
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(a, crate::browser::tools::guard::Answered::Late);
    assert_eq!(
        tokio::time::Instant::now(),
        soon - crate::desktop::tools::launch::ANSWER_MARGIN,
        "the margin leaves time to answer"
    );
}

/// §6.3: activate Firefox's window, check the focus (opening the gate), the key, then the gate;
/// decided under key_press's state while the switch is off, element_press's while it is on.
#[tokio::test]
async fn the_trusted_path_activates_checks_presses_and_reads_the_gate_in_order() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    element(&ff, 60, 12, "link", "Price list");
    with(&ff, 60, "needs_trusted", json!("new_window"));
    let done = format!(
        "{HEAD}: pressed 60 link \"Price list\" with Enter through the desktop (a link that \
         opens a new window, which Firefox opens only on a real key or click)"
    );
    // Switch off: element_press is asked, and the key under key_press's state again.
    let _ = h.steps();
    assert_eq!(
        h.owner("element_press", json!({"ref": 60})).await,
        (false, done.clone())
    );
    assert_eq!(
        h.steps(),
        [
            "element.describe@1",
            "element.press@1",
            "turn",
            "input.begin@1",
            "activate Checkout",
            "focus.check@1",
            "key Enter",
            "gate.result@1",
            "input.end@1"
        ],
        "the turn, then the span before the window, the check right before the key, the gate, \
         then the span's end"
    );
    let end = ff
        .acted()
        .into_iter()
        .find(|(o, _)| o == "input.end@1")
        .unwrap()
        .1;
    assert_eq!(
        end,
        json!({"span": 1, "settle_ms": 0}),
        "the gate's window waited"
    );
    let asked = h.asked();
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert_eq!(
        asked[1].tool,
        "key_press (Enter in Firefox, for element_press)"
    );
    assert!(
        asked[1]
            .description
            .ends_with("Firefox's window comes to the front first."),
        "{}",
        asked[1].description
    );
    assert_eq!(
        asked[1].lines,
        [
            format!("tab: {HEAD}"),
            "element: 60 link \"Price list\"".into(),
            "key: Enter".into()
        ]
    );
    let check = ff
        .acted()
        .into_iter()
        .find(|(o, _)| o == "focus.check@1")
        .unwrap()
        .1;
    assert_eq!(check["ref"], 60);
    assert!(check["guard"].is_object(), "the gate opens for the key");
    // The gate's question names the focus check's operation and is asked about.
    ff.model().asks = vec![post("https://shop.example/prices")];
    h.set(PolicyConfig::default(), switch_on());
    let text = h.owner("element_press", json!({"ref": 60})).await.1;
    assert_eq!(
        text,
        format!(
            "{done}\na POST to https://shop.example/prices (xmlhttprequest) went: the owner \
             allowed it"
        )
    );
    let asked = h.asked();
    assert_eq!(
        asked.len(),
        1,
        "the switch: element_press's state decides the key; only the gate asks"
    );
    assert_eq!(asked[0].tool, "element_press (a request the page sends)");
    // The switch on: key_press's own state does not matter.
    let mut deny = PolicyConfig::default();
    deny.tools.insert("key_press".into(), ToolState::Deny);
    h.set(deny.clone(), switch_on());
    assert_eq!(h.owner("element_press", json!({"ref": 60})).await.1, done);
    // The switch off: key_press's state decides.
    h.set(deny, BrowserConfig::default());
    let _ = h.steps();
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(
        err && text.contains("key_press, whose policy decides that, is refused"),
        "{text}"
    );
    assert!(!h.steps().iter().any(|s| s.starts_with("key")));
    let mut allow = PolicyConfig::default();
    allow.tools.insert("key_press".into(), ToolState::Allow);
    h.set(allow, BrowserConfig::default());
    let _ = h.asked();
    assert_eq!(h.owner("element_press", json!({"ref": 60})).await.1, done);
    assert_eq!(h.asked().len(), 1, "element_press's own question only");
    // The owner declines the key.
    h.set(PolicyConfig::default(), BrowserConfig::default());
    h.answers(&[Some(true), Some(false)]);
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(
        err && text.contains("the owner declined it (key_press's question)"),
        "{text}"
    );
}

#[tokio::test]
async fn the_trusted_path_refuses_where_the_focus_is_elsewhere_and_waits_for_the_card() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 60, 12, "link", "Price list");
    with(&ff, 60, "needs_trusted", json!("new_window"));
    with(&ff, 60, "focus", json!("firefox"));
    let _ = h.steps();
    assert_eq!(
        h.owner("element_press", json!({"ref": 60})).await,
        (
            true,
            "60 link \"Price list\" was not pressed: the sidebar or address bar has the keyboard \
             focus; Kai sends a real key only to the element it checked"
                .to_owned()
        )
    );
    let steps = h.steps();
    assert!(!steps.iter().any(|s| s.starts_with("key")), "{steps:?}");
    with(&ff, 60, "focus", json!("other_tab"));
    assert!(
        h.owner("element_press", json!({"ref": 60}))
            .await
            .1
            .contains("its tab is not the active tab of its window (tab_switch brings it")
    );
    // The approval card holds the keyboard: the press waits until it lets go.
    with(&ff, 60, "focus", Value::Null);
    h.desk
        .overlay
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = h.steps();
    let call = h.owner("element_press", json!({"ref": 60}));
    let release = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let before = h.steps();
        assert!(
            !before.iter().any(|s| s.starts_with("activate")),
            "nothing while the card holds the keyboard: {before:?}"
        );
        h.desk
            .overlay
            .store(false, std::sync::atomic::Ordering::SeqCst);
    };
    let ((err, text), ()) = tokio::join!(call, release);
    assert!(!err, "{text}");
    assert!(h.steps().contains(&"key Enter".to_owned()));
    // A file input's real press opens a picker: element_file is the way.
    with(&ff, 60, "needs_trusted", json!("file_input"));
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(
        err && text.contains("element_file gives it a file"),
        "{text}"
    );
    // A key that fails says so; the gate is read all the same.
    with(&ff, 60, "needs_trusted", json!("media"));
    *h.desk.press_fails.lock().unwrap() = Some("focus moved to org.kde.konsole".into());
    let _ = h.steps();
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(
        err && text == "60 link \"Price list\" was not pressed: focus moved to org.kde.konsole",
        "{text}"
    );
    let steps = h.steps();
    assert_eq!(steps[steps.len() - 2..], ["gate.result@1", "input.end@1"]);
}

/// `browser.trusted_press = "always"`: every press only focuses, then takes the trusted path, a
/// checkbox with Space.
#[tokio::test]
async fn trusted_press_always_sends_every_press_as_a_real_key() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            trusted_press: TrustedPress::Always,
            ..switch_on()
        },
    );
    element(&ff, 55, 12, "checkbox", "Gift wrap");
    let _ = h.steps();
    let (err, text) = h.owner("element_press", json!({"ref": 55})).await;
    assert!(!err, "{text}");
    assert!(text.contains("with Space through the desktop"), "{text}");
    let press = ff
        .acted()
        .into_iter()
        .find(|(o, _)| o == "element.press@1")
        .unwrap()
        .1;
    assert_eq!(press["activate"], false);
    assert!(h.steps().contains(&"key Space".to_owned()));
}

/// The site rules and the focus rule reach a ref's tab: a `read` site and a frame of one refuse
/// the acting tools but not `element_show`; a tab not in front, or one Kai brought there, is
/// asked about, a trusted approver's approval in lmgw too (it saw a ref).
#[tokio::test]
async fn site_rules_and_the_focus_rule_reach_an_elements_tab() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 41, 12, "textbox", "Full name");
    element(&ff, 44, 14, "button", "More lamps");
    element(&ff, 45, 3, "button", "Delete");
    ff.model().elements.insert(
        46,
        json!({"tab": 12, "role": "button", "name": "Pay", "frame": 7,
               "origin": "https://pay.example"}),
    );
    // Tab 14 is not the user's.
    let _ = h.asked();
    h.owner("element_press", json!({"ref": 44})).await;
    let a = h.asked();
    assert_eq!(a.len(), 1, "{a:?}");
    assert!(
        a[0].notes[0].starts_with("Asked because this is not the tab in front of you"),
        "{:?}",
        a[0].notes
    );
    assert!(a[0].lines.contains(
        &"acts on: 44 button \"More lamps\" in 14 \"Lamps\" https://shop.example/lamps".to_owned()
    ));
    // A trusted approver's approval does not spare it: its approver saw a ref, not the tab.
    h.approved("element_press", json!({"ref": 44})).await;
    let a = h.asked();
    assert_eq!(a.len(), 1, "{a:?}");
    assert!(
        a[0].notes[0].starts_with("Asked because this is not the tab in front of you"),
        "{:?}",
        a[0].notes
    );
    // The user's tab, but the extension activated it and the user has not acted since.
    {
        let mut m = ff.model();
        let t = m.tabs.iter_mut().find(|t| t["id"] == 12).unwrap();
        t["activated_by"] = json!("extension");
        t["activated_at"] = json!(100);
        t["user_at"] = json!(50);
        t["user_origin"] = json!("https://shop.example");
    }
    h.owner("element_fill", json!({"ref": 41, "text": "x"}))
        .await;
    let a = h.asked();
    assert!(
        a[0].notes[0].starts_with("Asked because Kai brought this tab"),
        "{:?}",
        a[0].notes
    );
    {
        let mut m = ff.model();
        let t = m.tabs.iter_mut().find(|t| t["id"] == 12).unwrap();
        t["user_at"] = json!(150);
    }
    h.owner("element_fill", json!({"ref": 41, "text": "x"}))
        .await;
    assert!(h.asked().is_empty(), "the user acted in it since");
    // Site rules.
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            sites: Sites {
                read: vec!["shop.example".into(), "pay.example".into()],
                none: vec!["mail.example".into()],
                act: Vec::new(),
            },
            ..switch_on()
        },
    );
    let n = ff.acted().len();
    let (err, text) = h
        .owner("element_fill", json!({"ref": 41, "text": "x"}))
        .await;
    assert!(
        err && text.contains("on a site where Kai may only read (Kai's site rules)"),
        "{text}"
    );
    let (err, text) = h.owner("element_press", json!({"ref": 45})).await;
    assert_eq!(
        (err, text.as_str()),
        (
            true,
            "ref 45 is in tab 3, on a site Kai does not reach (Kai's site rules); element_press \
             did not run"
        )
    );
    assert_eq!(ff.acted().len(), n + 2, "only the describes: nothing acted");
    // A view tool shows on a read site.
    assert!(!h.owner("element_show", json!({"ref": 41})).await.0);
    // A frame of a read site in an act site's page.
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            sites: Sites {
                read: vec!["pay.example".into()],
                ..Default::default()
            },
            ..switch_on()
        },
    );
    let (err, text) = h.owner("element_press", json!({"ref": 46})).await;
    assert!(err && text.contains("may only read"), "{text}");
}

/// The hands-and-eyes switch: every acting tool but element_file runs unasked for trusted callers;
/// without it every one is asked, and an untrusted caller is asked with it too.
#[tokio::test]
async fn the_switch_lets_trusted_callers_act_unasked_but_with_a_file() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    element(&ff, 41, 12, "textbox", "Full name");
    element(&ff, 72, 12, "listitem", "Pack");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "a").unwrap();
    let calls = [
        ("element_press", json!({"ref": 41})),
        ("element_fill", json!({"ref": 41, "text": "x"})),
        ("element_choose", json!({"ref": 41, "option": "x"})),
        ("element_check", json!({"ref": 41, "on": true})),
        ("element_focus", json!({"ref": 41})),
        ("element_hover", json!({"ref": 41})),
        ("element_drag", json!({"ref": 41, "to": 72})),
        ("element_show", json!({"ref": 41})),
        ("element_scroll", json!({"ref": 41, "to": "up"})),
        ("page_scroll", json!({"to": "up"})),
        ("page_key", json!({"keys": "Escape"})),
    ];
    for (tool, args) in &calls {
        let (err, text) = h.owner(tool, args.clone()).await;
        assert!(!err, "{tool}: {text}");
        assert_eq!(h.asked().len(), 1, "{tool}: asked without the switch");
    }
    h.set(PolicyConfig::default(), switch_on());
    for (tool, args) in &calls {
        let (err, text) = h.owner(tool, args.clone()).await;
        assert!(!err, "{tool}: {text}");
        assert!(h.asked().is_empty(), "{tool}: unasked with the switch");
        h.call(tool, args.clone(), &stranger()).await;
        assert_eq!(h.asked().len(), 1, "{tool}: an untrusted caller is asked");
    }
    h.owner(
        "element_file",
        json!({"ref": 41, "path": path.to_str().unwrap()}),
    )
    .await;
    assert_eq!(h.asked().len(), 1, "element_file stays asked");
}

/// The extension's acting errors read in words with what to call.
#[tokio::test]
async fn the_acting_errors_read_in_words() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 50, 12, "combobox", "Country");
    ff.model().twists.insert(
        "element.choose@1",
        Twist::FailWith(
            json!({"code": "no_option", "message": "ref 50 has no option \"Atlantis\"",
                               "options": ["Austria", "Belgium"], "tab": 12,
                               "title": "Checkout", "url": "https://shop.example/cart"}),
        ),
    );
    assert_eq!(
        h.owner("element_choose", json!({"ref": 50, "option": "Atlantis"}))
            .await,
        (
            true,
            "ref 50 has no option \"Atlantis\"; its options: \"Austria\", \"Belgium\"".to_owned()
        )
    );
    ff.model().twists.insert(
        "element.choose@1",
        Twist::Fail("inert", "ref 50 is outside the open modal dialog"),
    );
    let text = h
        .owner("element_choose", json!({"ref": 50, "option": "x"}))
        .await
        .1;
    assert!(text.contains("only its controls can be used"), "{text}");
    ff.model().twists.insert(
        "element.fill@1",
        Twist::Fail(
            "not_fillable",
            "ref 50 is a checkbox: element.check sets it",
        ),
    );
    let text = h
        .owner("element_fill", json!({"ref": 50, "text": "x"}))
        .await
        .1;
    assert!(text.contains("element_check sets a box"), "{text}");
}

/// Waits until `ff` got `op` `n` times (what Kai sends in the background: a span's end).
async fn arrived(ff: &Firefox, op: &str, n: usize) {
    tokio::time::timeout(super::WAIT, async {
        while ff.count(op) < n {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{op} did not arrive {n} times"));
}

/// The page side's verdicts are a list, asked about in one dialog; the call goes again with every
/// verdict allowed in it, and a second look that stops the press after its pointer events went is
/// asked about again, with what went and the requests the gate held meanwhile.
#[tokio::test]
async fn the_guards_verdicts_round_trip_and_a_late_look_asks_again() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 52, 12, "button", "Buy now");
    let buy = json!({"reason": "word", "word": "buy", "where": "name", "text": "Buy now"});
    let order = json!({"reason": "word", "word": "order", "where": "form_action",
                       "text": "https://shop.example/order"});
    let late = json!({"reason": "cross_origin_submit", "action": "https://evil.example/x",
                      "origin": "https://evil.example", "method": "post"});
    with(&ff, 52, "guard", json!([buy, order]));
    with(&ff, 52, "late", json!([late]));
    ff.model().asks = vec![post("https://shop.example/cart")];
    let (err, text) = h.owner("element_press", json!({"ref": 52})).await;
    assert!(!err, "{text}");
    assert_eq!(
        text,
        format!(
            "{HEAD}: pressed 52 button \"Buy now\"\na POST to https://shop.example/cart \
             (xmlhttprequest) went: the owner allowed it"
        ),
        "the request the late look's attempt let go is said"
    );
    let asked = h.asked();
    let tools: Vec<&str> = asked.iter().map(|a| a.tool.as_str()).collect();
    assert_eq!(
        tools,
        [
            "element_press (the commit guard)",
            "element_press (a request the page sends)",
            "element_press (the commit guard)"
        ]
    );
    assert_eq!(
        asked[0].lines[2..],
        [
            "why: its name \"Buy now\" holds the guard's word \"buy\"".to_owned(),
            "why: it submits a form to https://shop.example/order, which holds the guard's word \
             \"order\""
                .to_owned()
        ],
        "one dialog for the whole list"
    );
    assert!(
        asked[2]
            .lines
            .iter()
            .any(|l| l.starts_with("already: the page got the press's pointer")),
        "{:?}",
        asked[2].lines
    );
    let confirmed: Vec<Value> = ff
        .acted()
        .iter()
        .filter(|(o, _)| o == "element.press@1")
        .map(|(_, a)| a.get("confirmed").cloned().unwrap_or(Value::Null))
        .collect();
    assert_eq!(
        confirmed,
        [Value::Null, json!([buy, order]), json!([buy, order, late])]
    );
    // Declined at the late look: the click did not go, and the lines say what did.
    h.answers(&[Some(true), Some(true), Some(false)]);
    ff.model().asks = vec![post("https://shop.example/cart")];
    let (err, text) = h.owner("element_press", json!({"ref": 52})).await;
    assert!(err, "{text}");
    assert!(
        text.contains("was not pressed: the owner declined it (the commit guard asked because it sends a form (POST) to another site: https://evil.example/x); the page got the press's pointer events already, not its click\na POST to https://shop.example/cart (xmlhttprequest) went: the owner allowed it"),
        "{text}"
    );
}

/// page_key's page side: an Enter in an editor is asked about, and the key goes again confirmed.
#[tokio::test]
async fn an_enter_in_an_editor_is_asked_about() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    let editor = json!({"reason": "enter_in_editor", "text": "Message"});
    ff.model().key_guard = vec![editor.clone()];
    let (err, text) = h.owner("page_key", json!({"keys": "ctrl+Enter"})).await;
    assert!(!err, "{text}");
    let asked = h.asked();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].lines.contains(
        &"why: it is Enter in the editable field \"Message\", whose page may send what the field \
          holds"
            .to_owned()
    ));
    let keys: Vec<Value> = ff
        .acted()
        .iter()
        .filter(|(o, _)| o == "page.key@1")
        .map(|(_, a)| a.get("confirmed").cloned().unwrap_or(Value::Null))
        .collect();
    assert_eq!(keys, [Value::Null, json!([editor])]);
}

/// After the owner declined one of an operation's held requests, the rest of that operation's are
/// declined without a second dialog.
#[tokio::test]
async fn a_declined_request_declines_the_operations_others() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 53, 12, "button", "Add to cart");
    ff.model().asks = vec![
        post("https://shop.example/cart"),
        post("https://shop.example/track"),
    ];
    h.answers(&[Some(false)]);
    let text = h.owner("element_press", json!({"ref": 53})).await.1;
    assert_eq!(h.asked().len(), 1, "one dialog");
    assert_eq!(ff.model().answers, [false, false]);
    assert!(
        text.ends_with("track (xmlhttprequest) was cancelled: the owner declined it"),
        "{text}"
    );
}

/// Kai's own trusted key is not the user's act: inside the input span the fake extension moves
/// `kai_input_at`, so a tab Kai brought stays asked about after Kai's key; a Firefox whose span did
/// not open moves `user_at`, which falls into Kai's unbracketed moment and is discounted all the
/// same.
#[tokio::test]
async fn a_fact_during_kais_key_keeps_the_tab_asked() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 41, 12, "textbox", "Full name");
    element(&ff, 60, 12, "link", "Price list");
    with(&ff, 60, "needs_trusted", json!("new_window"));
    {
        let mut m = ff.model();
        let t = m.tabs.iter_mut().find(|t| t["id"] == 12).unwrap();
        t["activated_by"] = json!("extension");
        t["activated_at"] = json!(crate::browser::moves::now_ms() - 5000);
        t["user_at"] = json!(crate::browser::moves::now_ms() - 9000);
        t["user_origin"] = json!("https://shop.example");
    }
    let model = ff.model.clone();
    *h.desk.on_key.lock().unwrap() =
        Some(Box::new(move || model.lock().unwrap().trusted_input(12)));
    h.owner("element_fill", json!({"ref": 41, "text": "x"}))
        .await;
    assert!(h.asked()[0].notes[0].starts_with("Asked because Kai brought this tab"));
    // Kai's key: the press itself is asked (the tab is Kai's), then the key goes.
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(!err, "{text}");
    let _ = h.asked();
    assert!(
        ff.model().tabs[1]["kai_input_at"].is_number(),
        "Kai's, not the user's"
    );
    h.owner("element_fill", json!({"ref": 41, "text": "x"}))
        .await;
    let a = h.asked();
    assert!(
        a[0].notes[0].starts_with("Asked because Kai brought this tab"),
        "Kai's key did not make it the user's: {:?}",
        a[0].notes
    );
    // A Firefox whose span did not open: the key moves user_at, which Kai discounts.
    ff.model().begin_fails = Some("internal");
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(!err, "{text}");
    let _ = h.asked();
    {
        let m = ff.model();
        assert!(m.tabs[1]["user_at"].as_i64() > m.tabs[1]["activated_at"].as_i64());
    }
    h.owner("element_fill", json!({"ref": 41, "text": "x"}))
        .await;
    let a = h.asked();
    assert!(
        a[0].notes[0].starts_with("Asked because Kai's own keys reached this tab"),
        "{:?}",
        a[0].notes
    );
    // The user's own act afterwards makes it theirs.
    tokio::time::sleep(Duration::from_millis(10)).await;
    ff.model().begin_fails = None;
    ff.model().trusted_input(12);
    h.owner("element_fill", json!({"ref": 41, "text": "x"}))
        .await;
    assert!(h.asked().is_empty(), "the user acted in it since");
}

/// The desktop's input tools bracket their keys with an input span in each linked Firefox: opened
/// before the first key, ended with `browser.input_settle_ms`; a Firefox whose span did not open
/// has its tabs counted as touched by Kai.
#[tokio::test]
async fn type_text_is_bracketed_by_an_input_span() {
    use crate::desktop::tests::{FakeWindows, all_services, snapshot};
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            input_settle_ms: 300,
            ..switch_on()
        },
    );
    let services = crate::desktop::tools::Services {
        input: Some(crate::desktop::tests::input::fake_input()),
        spans: Some(h.spans.clone()),
        ..all_services(FakeWindows {
            snap: snapshot(),
            ..Default::default()
        })
    };
    let spec = kai_desktop::contract::find("type_text").unwrap().1;
    let args = crate::host::registry::Args::read(spec, json!({"text": "hi"}).as_object().cloned())
        .unwrap();
    let r = crate::desktop::tools::run_with(
        &services,
        "type_text",
        &args,
        &crate::desktop::tools::Ctx::default(),
    )
    .await;
    assert!(!r.error, "{r:?}");
    assert_eq!(ff.count("input.begin@1"), 1, "opened before the keys");
    arrived(&ff, "input.end@1", 1).await;
    let end = ff
        .acted()
        .into_iter()
        .find(|(o, _)| o == "input.end@1")
        .unwrap()
        .1;
    assert_eq!(end, json!({"span": 1, "settle_ms": 300}));
    assert!(ff.model().spans.is_empty());
    // key_press too; its span did not open in this Firefox: its tabs' facts in Kai's moment are
    // Kai's.
    ff.model().begin_fails = Some("internal");
    element(&ff, 41, 12, "textbox", "Full name");
    let spec = kai_desktop::contract::find("key_press").unwrap().1;
    let args =
        crate::host::registry::Args::read(spec, json!({"keys": "enter"}).as_object().cloned())
            .unwrap();
    let r = crate::desktop::tools::run_with(
        &services,
        "key_press",
        &args,
        &crate::desktop::tools::Ctx::default(),
    )
    .await;
    assert!(!r.error, "{r:?}");
    assert_eq!(ff.count("input.begin@1"), 2);
    ff.model().trusted_input(12);
    h.owner("element_fill", json!({"ref": 41, "text": "x"}))
        .await;
    let a = h.asked();
    assert!(
        a[0].notes[0].starts_with("Asked because Kai's own keys reached this tab"),
        "{:?}",
        a[0].notes
    );
}

/// However the trusted press ends once its gate is open (the call cancelled, the screen locked
/// right before the key), the gate is read and the span ends, in that order; no key goes.
#[tokio::test]
async fn the_gate_closes_and_the_span_ends_when_the_press_ends_early() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 60, 12, "link", "Price list");
    with(&ff, 60, "needs_trusted", json!("new_window"));
    // lmgw cancels the call as Firefox's window comes to the front.
    let cancel = CancellationToken::new();
    *h.desk.on_activate.lock().unwrap() = Some(cancel.clone());
    let _ = h.steps();
    let (err, text) = h
        .call_until("element_press", json!({"ref": 60}), &Caller::Owner, cancel)
        .await;
    assert!(err, "{text}");
    arrived(&ff, "input.end@1", 1).await;
    let steps = h.steps();
    let at = |s: &str| steps.iter().position(|x| x == s).unwrap_or(usize::MAX);
    assert!(
        at("focus.check@1") < at("gate.result@1") && at("gate.result@1") < at("input.end@1"),
        "{steps:?}"
    );
    assert!(!steps.iter().any(|s| s.starts_with("key")), "{steps:?}");
    // The screen locked right before the key.
    h.desk
        .locked
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(
        err && text.contains("the screen locked, and Kai sends no key while it is"),
        "{text}"
    );
    arrived(&ff, "input.end@1", 2).await;
    let steps = h.steps();
    let tail: Vec<&str> = steps
        .iter()
        .map(String::as_str)
        .filter(|s| *s != "element.describe@1")
        .collect();
    assert_eq!(
        tail[tail.len() - 3..],
        ["focus.check@1", "gate.result@1", "input.end@1"]
    );
    assert_eq!(ff.count("gate.result@1"), 2, "each gate read once");
}

/// With two profiles linked the press opens and ends a span in each, and the profile whose window
/// Kai brought to the front is not the user's until the user acts there; the focus check's gate
/// opens with the guard off too, so the key's way is told.
#[tokio::test]
async fn a_press_in_a_second_profile_is_bracketed_in_both_and_its_tab_stays_asked() {
    let h = Host::start().await;
    let now = crate::browser::moves::now_ms();
    let mut a_model = shop();
    a_model.windows = vec![
        json!({"id": 1, "focused": true, "focused_at": now - 1000}),
        json!({"id": 2, "focused": false, "focused_at": now - 9000}),
    ];
    let mut news = super::fake::tab(5, 1, "News", "https://news.example/", true);
    news["activated_by"] = json!("user");
    news["activated_at"] = json!(now - 60_000);
    news["user_at"] = json!(now - 60_000);
    news["user_origin"] = json!("https://news.example");
    let mut b_model = super::fake::Model::with(vec![news]);
    b_model.windows = vec![json!({"id": 1, "focused": false, "focused_at": now - 30_000})];
    let a = h.firefox("a", &all_ops(), a_model).await;
    let b = h.firefox("b", &all_ops(), b_model).await;
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            commit_guard: false,
            ..switch_on()
        },
    );
    // Refs of profile b's page, which Kai learns from it.
    element(&b, 60, 5, "link", "Archive");
    with(&b, 60, "needs_trusted", json!("new_window"));
    element(&b, 61, 5, "textbox", "Search");
    b.model().twists.insert(
        "page.controls@1",
        Twist::Answer(json!({
            "tab": {"id": 5, "title": "News", "url": "https://news.example/"},
            "frames": {"read": 1, "unread": 0},
            "controls": [{"ref": 60, "role": "link", "name": "Archive"},
                         {"ref": 61, "role": "textbox", "name": "Search"}]})),
    );
    let text = h.owner("page_controls", json!({"tab": "5"})).await.1;
    assert!(text.contains("60 link \"Archive\""), "{text}");
    let _ = h.asked();
    // Kai's activation moves b's window to the front.
    let bm = b.model.clone();
    let am = a.model.clone();
    *h.desk.on_key.lock().unwrap() = Some(Box::new(move || {
        let t = crate::browser::moves::now_ms();
        bm.lock().unwrap().windows = vec![json!({"id": 1, "focused": true, "focused_at": t})];
        am.lock().unwrap().windows[0]["focused"] = json!(false);
    }));
    // Profile a holds the user: b's tab is behind, so the press is asked.
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(!err, "{text}");
    assert!(h.asked()[0].notes[0].starts_with("Asked because this is not the tab in front"));
    for ff in [&a, &b] {
        assert_eq!(ff.count("input.begin@1"), 1);
    }
    arrived(&a, "input.end@1", 1).await;
    arrived(&b, "input.end@1", 1).await;
    let check = b
        .acted()
        .into_iter()
        .find(|(o, _)| o == "focus.check@1")
        .unwrap()
        .1;
    assert_eq!(
        check["guard"]["words"],
        json!([]),
        "the guard off: a gate that watches only the key"
    );
    // b is in front now, through Kai's own activation: its tab is not the user's yet.
    h.owner("element_fill", json!({"ref": 61, "text": "x"}))
        .await;
    let asked = h.asked();
    assert!(
        asked[0].notes[0].starts_with("Asked because Kai brought this tab"),
        "{:?}",
        asked[0].notes
    );
    // The user acts there: theirs.
    b.model().trusted_input(5);
    h.owner("element_fill", json!({"ref": 61, "text": "x"}))
        .await;
    assert!(h.asked().is_empty());
}

/// Where the key went (`key_seen`), and a gate that expired, are said.
#[tokio::test]
async fn the_trusted_press_says_where_the_key_went() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 60, 12, "link", "Price list");
    with(&ff, 60, "needs_trusted", json!("new_window"));
    {
        let mut m = ff.model();
        m.key_seen = Some(Value::Null);
        m.gate_expired = true;
    }
    let (err, text) = h.owner("element_press", json!({"ref": 60})).await;
    assert!(!err, "{text}");
    assert!(
        text.contains("\nthe page did not see the key: it went elsewhere"),
        "{text}"
    );
    assert!(
        text.contains("watch had ended before Kai read it"),
        "{text}"
    );
    ff.model().key_seen = None;
    ff.model().gate_expired = false;
    let text = h.owner("element_press", json!({"ref": 60})).await.1;
    assert!(!text.contains('\n'), "the element got it: {text}");
}

/// A call approved in lmgw: the trusted press's real key is asked at Kai's desk (its approver saw
/// a ref), unless the deciding tool is set to allow.
#[tokio::test]
async fn an_lmgw_approval_does_not_cover_the_real_key() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 60, 12, "link", "Price list");
    with(&ff, 60, "needs_trusted", json!("new_window"));
    let (err, text) = h
        .approved_by(
            "element_press",
            json!({"ref": 60}),
            &Caller::Owner,
            &Caller::Owner,
        )
        .await;
    assert!(!err, "{text}");
    let asked = h.asked();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(
        asked[0].tool,
        "key_press (Enter in Firefox, for element_press)"
    );
    let mut allow = PolicyConfig::default();
    allow.tools.insert("element_press".into(), ToolState::Allow);
    h.set(allow, switch_on());
    h.approved_by(
        "element_press",
        json!({"ref": 60}),
        &Caller::Owner,
        &Caller::Owner,
    )
    .await;
    assert!(h.asked().is_empty(), "element_press set to allow");
}

/// `browser.trusted_press = "always"` with key_press confirmed: where the guard stops the press, one
/// dialog asks about its verdicts and the real key, and the key is not asked about again.
#[tokio::test]
async fn the_guard_and_the_key_share_one_dialog() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(
        PolicyConfig::default(),
        BrowserConfig {
            trusted_press: TrustedPress::Always,
            ..Default::default()
        },
    );
    element(&ff, 52, 12, "button", "Buy now");
    with(
        &ff,
        52,
        "guard",
        json!([{"reason": "word", "word": "buy", "where": "name", "text": "Buy now"}]),
    );
    let (err, text) = h.owner("element_press", json!({"ref": 52})).await;
    assert!(!err, "{text}");
    let asked = h.asked();
    let tools: Vec<&str> = asked.iter().map(|a| a.tool.as_str()).collect();
    assert_eq!(
        tools,
        ["element_press", "element_press (the commit guard)"],
        "K15's own question, then one for the guard and the key"
    );
    assert!(asked[1].lines.contains(
        &"key: Enter through the desktop (key_press's question, asked with this one)".to_owned()
    ));
    assert!(text.contains("with Enter through the desktop"), "{text}");
}

/// `element_file` sends the file the dialog named: one changed while the owner was asked is not
/// sent; the dialog names its real path.
#[tokio::test]
async fn element_file_sends_only_the_file_the_dialog_named() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 57, 12, "generic", "Receipt");
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("receipt.txt");
    std::fs::write(&real, "the receipt's text\n").unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let changed = real.clone();
    h.while_asked(move || std::fs::write(&changed, "something else, longer\n").unwrap());
    let real = std::fs::canonicalize(&real).unwrap();
    let (err, text) = h
        .owner(
            "element_file",
            json!({"ref": 57, "path": link.to_str().unwrap()}),
        )
        .await;
    assert!(
        err && text.contains("changed since the dialog named it"),
        "{text}"
    );
    let asked = h.asked();
    assert!(
        asked[0]
            .lines
            .iter()
            .any(|l| l.starts_with("sends: \"link.txt\" (19 bytes")
                && l.ends_with(&format!("from {}", real.display()))),
        "{:?}",
        asked[0].lines
    );
    assert_eq!(ff.count("element.file@1"), 0, "nothing sent");
}

/// A press whose element's frame went away (a cookie banner's close button removing its own
/// frame) says so, not that the page was left; a press that left the page says that (kai-firefox
/// WP10's acceptance run).
#[tokio::test]
async fn a_frame_gone_after_a_press_is_not_a_page_left() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 52, 12, "button", "Close the cookie banner");
    with(
        &ff,
        52,
        "answer",
        json!({"after": "page_left", "frame": 17_179_869_185i64, "origin": "https://consent.example"}),
    );
    let (error, text) = h.owner("element_press", json!({"ref": 52})).await;
    assert!(!error, "{text}");
    assert_eq!(
        text,
        format!(
            "{HEAD}: pressed 52 button \"Close the cookie banner\"\nthen its frame was removed or \
             went to another page, so its element is gone"
        )
    );
    element(&ff, 53, 12, "link", "Results");
    with(&ff, 53, "answer", json!({"after": "page_left"}));
    let (_, text) = h.owner("element_press", json!({"ref": 53})).await;
    assert!(
        text.ends_with("\nthen the page was left, so its element is gone"),
        "{text}"
    );
}

/// Points tab `id` of the fake Firefox at `url`, as a navigation there leaves it.
fn goes(ff: &Firefox, id: i64, url: &str) {
    let mut m = ff.model();
    let t = m.tabs.iter_mut().find(|t| t["id"] == id).unwrap();
    t["url"] = json!(url);
}

/// kai-firefox WP10b: a press that took the user's tab to another site is Kai's navigation, which
/// leaves the site the tab is anchored on where it was, also where the user never acted in the tab
/// (the probe's "Kai keeps acting there unasked"); with the switch on, a press of the owner's own
/// turn (Kai's own device) is the owner's act, and Kai acts on the new site unasked.
#[tokio::test]
async fn a_press_to_another_site_is_kais_unless_it_is_the_owners_hands_free() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 60, 12, "link", "Partner shop");
    element(&ff, 61, 12, "searchbox", "Search");
    let partner = json!({"after": "page_left",
                         "tab": {"id": 12, "title": "Partner", "url": "https://partner.example/"},
                         "goes": {"by": "link", "origin": "https://partner.example"}});
    with(&ff, 60, "answer", partner.clone());
    // lmgw's own run: a trusted caller, but not the owner's turn.
    let (err, said) = h
        .call("element_press", json!({"ref": 60}), &Caller::Gateway)
        .await;
    assert!(!err, "{said}");
    goes(&ff, 12, "https://partner.example/");
    let _ = h.asked();
    h.call(
        "element_fill",
        json!({"ref": 61, "text": "lamp"}),
        &Caller::Gateway,
    )
    .await;
    let asked = h.asked();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(
        asked[0].notes[0].contains("another site than the one you had open"),
        "{asked:?}"
    );

    // The owner's own turn hands free: the press is theirs, the fill on the new site unasked.
    goes(&ff, 12, "https://shop.example/cart");
    let (err, said) = h
        .call("element_press", json!({"ref": 60}), &Caller::Own)
        .await;
    assert!(!err, "{said}");
    let _ = h.asked();
    goes(&ff, 12, "https://partner.example/");
    let (err, said) = h
        .call(
            "element_fill",
            json!({"ref": 61, "text": "lamp"}),
            &Caller::Own,
        )
        .await;
    assert!(!err, "{said}");
    assert!(h.asked().is_empty(), "asked on the owner's own site");
    // A site the page moved on to by itself is asked about, for any caller.
    goes(&ff, 12, "https://elsewhere.example/");
    h.call(
        "element_fill",
        json!({"ref": 61, "text": "lamp"}),
        &Caller::Gateway,
    )
    .await;
    assert_eq!(h.asked().len(), 1);

    // The switch off, the acting tools set to allow: the owner's own press is Kai's navigation
    // like anyone's, and the fill on the other site is asked (the probe's case).
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    let mut tools = std::collections::BTreeMap::new();
    for t in ["element_press", "element_fill"] {
        tools.insert(t.to_owned(), ToolState::Allow);
    }
    h.set(
        PolicyConfig {
            tools,
            ..Default::default()
        },
        BrowserConfig::default(),
    );
    element(&ff, 60, 12, "link", "Partner shop");
    element(&ff, 61, 12, "searchbox", "Search");
    with(&ff, 60, "answer", partner);
    let (err, said) = h
        .call("element_press", json!({"ref": 60}), &Caller::Own)
        .await;
    assert!(!err, "{said}");
    assert!(
        h.asked().is_empty(),
        "the press itself on the user's tab is unasked"
    );
    goes(&ff, 12, "https://partner.example/");
    h.call(
        "element_fill",
        json!({"ref": 61, "text": "lamp"}),
        &Caller::Own,
    )
    .await;
    assert_eq!(h.asked().len(), 1, "the focus rule asks on the other site");
}

/// kai-firefox WP10b: the questions of a call of Kai's own carry the question in Kai's words for
/// its voice (the focus rule's, `element_file`'s, the commit guard's page side and its network
/// gate's), Kai's words first and the page's last; another caller's questions show the dialog
/// alone.
#[tokio::test]
async fn kais_own_calls_questions_are_given_words_to_speak() {
    let h = Host::start().await;
    let ff = h.firefox("c0ffee", &all_ops(), shop()).await;
    h.set(PolicyConfig::default(), switch_on());
    element(&ff, 52, 14, "button", "Buy now");
    // The focus rule: tab 14 is not the user's.
    h.call("element_press", json!({"ref": 52}), &Caller::Own)
        .await;
    let asked = h.asked();
    assert_eq!(
        asked[0].spoken.as_deref(),
        Some(
            "May Kai press the button? This tab is not the one in front of you. Yes or no? The \
             page calls the button: Buy now"
        ),
        "{asked:?}"
    );
    h.call("element_press", json!({"ref": 52}), &Caller::Gateway)
        .await;
    assert_eq!(h.asked()[0].spoken, None, "lmgw's own run: the dialog only");

    // element_file, asked by its state.
    element(&ff, 57, 12, "file", "Receipt");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("receipt.txt");
    std::fs::write(&path, "Trail Lamp, 29 EUR\n").unwrap();
    h.call(
        "element_file",
        json!({"ref": 57, "path": path.display().to_string()}),
        &Caller::Own,
    )
    .await;
    assert_eq!(
        h.asked()[0].spoken.as_deref(),
        Some(
            "May Kai attach a file to the file field? Yes or no? The page calls the file field: \
             Receipt. The file is named: receipt.txt"
        )
    );

    // The commit guard's page side, then the network gate's POST.
    element(&ff, 53, 12, "button", "Place order");
    with(
        &ff,
        53,
        "guard",
        json!([{"reason": "word", "word": "order", "where": "name", "text": "Place order"}]),
    );
    ff.model().asks = vec![post("https://shop.example/order")];
    h.call("element_press", json!({"ref": 53}), &Caller::Own)
        .await;
    let asked = h.asked();
    let spoken: Vec<&str> = asked.iter().filter_map(|a| a.spoken.as_deref()).collect();
    assert_eq!(
        spoken,
        [
            "May Kai press the button? The commit guard found the word “order” in what it \
             says. Yes or no? The page calls the button: Place order",
            "After Kai's action the page sends a POST request, and the commit guard holds it. \
             May it go? Yes or no? The page sends it to: shop.example"
        ],
        "{asked:?}"
    );
}
