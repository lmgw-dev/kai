//! What Kai never reads, and what it never presses, in Firefox (kai-firefox §3, WP12's review
//! round): refused whatever Kai's site rules, the commit guard and the tool's state say.
//!
//! - **Never read, never pressed in** ([`never_read`]): Firefox's password manager
//!   (`about:logins`), another extension's page (`moz-extension:`: a password manager's vault in a
//!   tab, Kai for Firefox's own pages too) and Firefox's saved cards and addresses (its
//!   `chrome://formautofill/` pages and dialogs). In Firefox's window `super::parts` leaves out the
//!   password manager's prompt, the logins' autocomplete and the save prompts of cards and
//!   addresses, and any password field's text.
//! - **Never pressed in** ([`never_pressed`]; read as the rules say): Firefox's pages where a
//!   press changes how Firefox itself works: `about:config`, `about:addons`, `about:debugging`,
//!   `about:profiles`, `about:support`, `about:policies`, and the whole of `about:preferences`
//!   (design decision: its panes are one document whose categories, search and sub-dialogs
//!   reach every pane, and its General pane holds the proxy, the applications that open each file
//!   type and the updates, no less critical than the privacy and security pane). In Firefox's
//!   window `super::parts` marks the add-on doorhangers (an install, an add-on's permissions, an
//!   optional permission such as `userScripts` or private-window access) and a site's request to
//!   share the screen or a window as never pressed, and `super::press` a page's dialog that holds
//!   a password field (a sign-in).

use crate::browser::sites::effective;

/// The `about:` page `url` shows (its name, lower case, without query or fragment), Reader view
/// and `view-source:` unwrapped.
fn about(url: &str) -> Option<String> {
    let u = url::Url::parse(url.trim()).ok()?;
    let u = if u.scheme() == "about" {
        u
    } else {
        effective(url.trim())?
    };
    (u.scheme() == "about").then(|| u.path().to_ascii_lowercase())
}

/// The scheme of `url` and of the URL it wraps (`view-source:`, `blob:`, Reader view).
fn schemes(url: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(u) = url::Url::parse(url.trim()) {
        out.push(u.scheme().to_ascii_lowercase());
    }
    if let Some(u) = effective(url.trim()) {
        out.push(u.scheme().to_ascii_lowercase());
    }
    out
}

/// What `url` is, where Kai never reads or presses in it (module doc); the words for a refusal.
pub fn never_read(url: &str) -> Option<&'static str> {
    if about(url).is_some_and(|p| p == "logins" || p == "loginsimportreport") {
        return Some("Firefox's password manager (about:logins)");
    }
    if schemes(url).iter().any(|s| s == "moz-extension") {
        return Some("another extension's page (moz-extension:)");
    }
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("chrome://formautofill/")
        || effective(url.trim()).is_some_and(|u| {
            u.as_str()
                .to_ascii_lowercase()
                .starts_with("chrome://formautofill/")
        })
    {
        return Some("Firefox's saved cards and addresses");
    }
    None
}

/// Firefox's own pages Kai never presses in (module doc), by their `about:` name.
const NEVER_PRESSED: &[(&str, &str)] = &[
    ("config", "about:config, Firefox's advanced settings"),
    ("addons", "about:addons, Firefox's add-ons"),
    ("debugging", "about:debugging, where add-ons are loaded"),
    ("profiles", "about:profiles, Firefox's profiles"),
    ("support", "about:support, Firefox's troubleshooting page"),
    ("policies", "about:policies, Firefox's enterprise policies"),
    ("preferences", "about:preferences, Firefox's settings"),
    ("settings", "about:preferences, Firefox's settings"),
];

/// What `url` is, where Kai never presses in it (module doc, both lists); the words for a refusal.
pub fn never_pressed(url: &str) -> Option<&'static str> {
    if let Some(why) = never_read(url) {
        return Some(why);
    }
    let page = about(url)?;
    NEVER_PRESSED
        .iter()
        .find(|(name, _)| *name == page)
        .map(|(_, why)| *why)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lists_take_every_form_of_their_pages() {
        for url in [
            "about:logins",
            "ABOUT:Logins?filter=bank",
            "about:logins#login-id",
            "moz-extension://4f0c/vault.html",
            "view-source:moz-extension://4f0c/vault.html",
            "blob:moz-extension://4f0c/0b1c",
            "about:reader?url=moz-extension%3A%2F%2F4f0c%2Fvault.html",
            "chrome://formautofill/content/manageCreditCards.xhtml",
        ] {
            assert!(never_read(url).is_some(), "{url}");
            assert!(never_pressed(url).is_some(), "{url}");
        }
        for url in [
            "about:config",
            "about:addons",
            "about:debugging#/runtime/this-firefox",
            "about:profiles",
            "about:support",
            "about:policies",
            "about:preferences",
            "about:preferences#privacy",
            "about:preferences#general",
            "about:settings",
        ] {
            assert!(never_read(url).is_none(), "{url} is read as the rules say");
            assert!(never_pressed(url).is_some(), "{url}");
        }
        for url in [
            "about:newtab",
            "about:reader?url=https%3A%2F%2Fnews.example%2F",
            "view-source:https://news.example/",
            "https://news.example/about:config",
            "https://moz-extension.example/",
            "chrome://browser/content/browser.xhtml",
            "",
        ] {
            assert!(never_pressed(url).is_none(), "{url}");
        }
    }
}
