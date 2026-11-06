//! The window's parts against Firefox 159's tree as WP12 and its review round saw it on a private
//! bus: what is read (an allow-list), what is left out and counted, and what a prompt is.

use super::*;

fn el(role: &str, name: &str, id: &str, children: Vec<Element>) -> Element {
    Element {
        role: role.into(),
        name: name.into(),
        id: id.into(),
        showing: true,
        children,
        ..Default::default()
    }
}

fn at(e: Element, path: &str) -> Element {
    Element {
        at: Some((":1.1".into(), path.into())),
        pressable: matches!(e.role.as_str(), "button" | "check box" | "link"),
        ..e
    }
}

fn classed(mut e: Element, class: &str) -> Element {
    e.class = class.into();
    e
}

/// The prompt Firefox 159 shows for a site's location request (the probe's tree): its
/// description, a link, a check box, its two buttons, each with its own label.
pub fn location_prompt(host: &str) -> Element {
    el(
        "alert",
        "",
        "notification-popup",
        vec![
            classed(
                el(
                    "label",
                    &format!("Allow {host} to access your location?"),
                    "",
                    vec![],
                ),
                "popup-notification-description",
            ),
            at(el("link", "Learn more", "", vec![]), "/learn"),
            at(
                el("check box", "Remember this decision", "", vec![]),
                "/remember",
            ),
            el(
                "section",
                "",
                "",
                vec![at(
                    el(
                        "button",
                        "Block",
                        "main-button",
                        vec![el("label", "Block", "", vec![])],
                    ),
                    "/block",
                )],
            ),
            el(
                "section",
                "",
                "",
                vec![at(
                    el(
                        "button",
                        "Allow",
                        "main-button",
                        vec![el("label", "Allow", "", vec![])],
                    ),
                    "/allow",
                )],
            ),
        ],
    )
}

#[test]
fn the_origin_a_dialog_names_is_read_in_any_language() {
    assert_eq!(
        named_origin("The page at https://shop.example says:").as_deref(),
        Some("https://shop.example")
    );
    assert_eq!(
        named_origin("Die Seite unter http://127.0.0.1:8080 meldet:").as_deref(),
        Some("http://127.0.0.1:8080")
    );
    assert_eq!(
        named_origin("This page is asking you to confirm that you want to leave"),
        None
    );
}

#[test]
fn the_sites_a_prompt_names_are_read_in_any_language() {
    assert_eq!(
        named_hosts("Allow 127.0.0.1:34027 to access your location?"),
        ["127.0.0.1:34027"]
    );
    assert_eq!(
        named_hosts("Darf „shop.example“ auf Ihren Standort zugreifen?"),
        ["shop.example"]
    );
    assert_eq!(
        named_hosts("Allow tracker.example to use its cookies on shop.example?"),
        ["tracker.example", "shop.example"]
    );
    assert!(named_hosts("Software installation is currently disabled.").is_empty());
    assert!(named_hosts("Allow this local file to access your location?").is_empty());
}

/// What a prompt in `notification-popup` is: a site's permission for the tab's site, a screen
/// share, an add-on's, and every other prompt.
#[test]
fn a_prompt_is_a_sites_permission_only_where_it_names_the_tabs_site() {
    let site = Some("127.0.0.1:34027");
    assert_eq!(
        prompt(&location_prompt("127.0.0.1:34027"), site),
        Prompt::Permission
    );
    // Another site, no site, or a tab whose site Kai cannot name.
    assert_eq!(
        prompt(&location_prompt("evil.example"), site),
        Prompt::Other
    );
    assert_eq!(
        prompt(&location_prompt("this local file"), site),
        Prompt::Other
    );
    assert_eq!(
        prompt(&location_prompt("127.0.0.1:34027"), None),
        Prompt::Other
    );
    // The camera and microphone (Firefox 159's ids), and a screen share.
    let mut camera = location_prompt("127.0.0.1:34027");
    camera.children[0].id = "webRTC-shareDevices-notification-description".into();
    camera.children.push(el(
        "label",
        "Default Video Device",
        "webRTC-selectCamera-single-device-label",
        vec![],
    ));
    assert_eq!(prompt(&camera, site), Prompt::Permission);
    let mut screen = camera.clone();
    screen.children.push(el(
        "combo box",
        "Window or screen to share",
        "webRTC-selectWindow-menulist",
        vec![],
    ));
    assert_eq!(prompt(&screen, site), Prompt::Screen);
    // An add-on's prompt (the permissions an add-on asks for, an optional one too), named or not.
    let mut addon = location_prompt("127.0.0.1:34027");
    addon.children.push(el(
        "label",
        "Access your data for all websites",
        "addon-webext-perm-list-optional",
        vec![],
    ));
    assert_eq!(prompt(&addon, site), Prompt::Addon);
    let mut blocked = location_prompt("127.0.0.1:34027");
    blocked.children[1].class = "addon-install-blocked-info".into();
    assert_eq!(prompt(&blocked, site), Prompt::Addon);
    // The password manager's prompt, a card's save prompt, a sign-in: other prompts.
    let mut password = location_prompt("127.0.0.1:34027");
    password.children.push(el(
        "entry",
        "Username",
        "password-notification-username",
        vec![],
    ));
    assert_eq!(prompt(&password, site), Prompt::Other);
    let mut card = location_prompt("127.0.0.1:34027");
    card.children[0].class = "popup-notification-description credit-card-capture".into();
    assert_eq!(prompt(&card, site), Prompt::Other);
}

/// Firefox's window: what is read, what is left out, and that only the tab's own panel's
/// dialog is taken.
#[test]
fn the_windows_parts_are_an_allow_list() {
    let page = (":1.1".to_owned(), "/page".to_owned());
    let doc = |path: &str, role: &str, url: &str| Element {
        document: Some(url.into()),
        ..at(el(role, "", "", Vec::new()), path)
    };
    let mut urlbar = el(
        "combo box",
        "Search or enter address",
        "urlbar-input",
        vec![],
    );
    urlbar.text = "https://bank.example/account".into();
    let mut bar = el(
        "alert",
        "Firefox prevented this site from opening a pop-up window.",
        "",
        vec![],
    );
    bar.class = "notification-message".into();
    let frame = el(
        "frame",
        "Shop — Mozilla Firefox",
        "main-window",
        vec![
            el(
                "tool bar",
                "Browser tabs",
                "TabsToolbar",
                vec![
                    el(
                        "page tab list",
                        "",
                        "tabbrowser-tabs",
                        vec![el("page tab", "Bank", "", vec![])],
                    ),
                    at(
                        el("button", "List all tabs", "alltabs-button", vec![]),
                        "/all",
                    ),
                ],
            ),
            el(
                "tool bar",
                "Navigation",
                "nav-bar",
                vec![
                    at(el("button", "Back", "back-button", vec![]), "/back"),
                    urlbar,
                ],
            ),
            el(
                "tool bar",
                "Bookmarks Toolbar",
                "PersonalToolbar",
                vec![el("button", "Bank", "", vec![])],
            ),
            location_prompt("shop.example"),
            el(
                "panel",
                "",
                "appMenu-popup",
                vec![
                    classed(
                        el("button", "Bank — account", "", vec![]),
                        "subviewbutton bookmark-item",
                    ),
                    at(
                        el("button", "Settings", "appMenu-settings-button", vec![]),
                        "/settings",
                    ),
                    el(
                        "button",
                        "Recently closed tabs",
                        "appMenu-library-recentlyClosedTabs",
                        vec![],
                    ),
                ],
            ),
            el(
                "panel",
                "",
                "tabbrowser-tabpanels",
                vec![
                    el(
                        "scroll pane",
                        "",
                        "",
                        vec![
                            el(
                                "internal frame",
                                "",
                                "",
                                vec![doc("/page", "document web", "https://shop.example/")],
                            ),
                            el(
                                "internal frame",
                                "",
                                "",
                                vec![doc("/dialog", "dialog", COMMON_DIALOG)],
                            ),
                            el(
                                "internal frame",
                                "",
                                "",
                                vec![doc(
                                    "/handler",
                                    "dialog",
                                    "chrome://mozapps/content/handling/permissionDialog.xhtml",
                                )],
                            ),
                            bar,
                        ],
                    ),
                    el(
                        "scroll pane",
                        "",
                        "",
                        vec![el(
                            "internal frame",
                            "",
                            "",
                            vec![doc("/other-dialog", "dialog", COMMON_DIALOG)],
                        )],
                    ),
                ],
            ),
            el("alert", "", "a11y-announcement", vec![]),
            el("panel", "History", "sidebar-box", vec![]),
            el("panel", "", "downloadsPanel", vec![]),
            el("panel", "", "customizationui-widget-panel", vec![]),
        ],
    );
    let p = parts(&frame, &page, Some("shop.example"));
    let mut out = Vec::new();
    for r in &p.read {
        out.push(format!("[{}]", r.part.words()));
        lines(&r.root, 0, &mut out);
    }
    assert_eq!(
        out,
        [
            "[Firefox's tool bar]",
            "tool bar \"Browser tabs\"",
            "  button \"List all tabs\"",
            "[Firefox's tool bar]",
            "tool bar \"Navigation\"",
            "  button \"Back\"",
            "  combo box \"Search or enter address\"",
            "[Firefox's prompt for a site]",
            "label \"Allow shop.example to access your location?\"",
            "link \"Learn more\"",
            "check box \"Remember this decision\"",
            "button \"Block\"",
            "button \"Allow\"",
            "[Firefox's menu]",
            "button \"Settings\"",
            "[Firefox's notification bar]",
            "alert \"Firefox prevented this site from opening a pop-up window.\"",
        ]
    );
    assert_eq!(p.dialogs.len(), 1, "only the tab's own panel's page dialog");
    assert_eq!(p.dialogs[0].at.as_ref().unwrap().1, "/dialog");
    assert_eq!(p.read[2].hosts, ["shop.example"]);
    // The tab strip, the bookmarks bar, the history entry and the recently closed tabs, the
    // protocol handler's dialog, the sidebar, the downloads and the all-tabs panels.
    assert_eq!(p.left, 8);
    let mut found = Vec::new();
    for r in &p.read {
        pressables(&r.root, &mut found);
    }
    let names: Vec<&str> = found.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "List all tabs",
            "Back",
            "Learn more",
            "Remember this decision",
            "Block",
            "Allow",
            "Settings"
        ]
    );
    // A prompt that names another site than the tab's is not read.
    let p = parts(&frame, &page, Some("mail.example"));
    assert!(p.read.iter().all(|r| r.part != Part::Prompt));
    // What a prompt says, for the confirmation.
    let mut w = Vec::new();
    says(&location_prompt("shop.example"), &mut w);
    assert_eq!(
        w,
        [
            "\"Allow shop.example to access your location?\"",
            "check box \"Remember this decision\" (not checked)"
        ]
    );
}
