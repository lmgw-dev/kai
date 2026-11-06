//! Kai for Firefox's link to Kai (kai-firefox design, `docs/design/2026-11-03-kai-firefox-design.md`
//! in the kai-firefox repository: §1, §2, §4): the local socket `$XDG_RUNTIME_DIR/kai/browser.sock`
//! (mode 0600 in Kai's 0700 runtime folder; `KAI_BROWSER_SOCKET` names another for the tests and
//! live checks), which `kai-browser-host` (`crates/kai-browser-host`), the native-messaging host
//! Firefox starts for the extension, connects to and relays.
//!
//! - **Frames** ([`frame`]): native messaging's framing, a 4-byte little-endian length and UTF-8
//!   JSON with a type `t`. Firefox takes at most 1 MiB (1 048 576 bytes, that size included) in one
//!   message toward the extension, so every frame Kai sends that is longer goes as `chunk` frames
//!   and an `end` ([`chunk`]). The extension splits its long frames toward Kai the same way, since
//!   Firefox writes one large message slowly, and Kai joins them; a split that cannot be joined is
//!   answered with an `error` frame (`bad_split`).
//! - **Who may connect** ([`peer`]): a process of Kai's own user in Firefox's systemd unit, as
//!   `browser.peer_scopes` names it ([`settings`]). It raises the bar; it is not a boundary.
//! - **The handshake** ([`server`]): the extension's first frame is `hello` (`ext_version`,
//!   `protocol`, `ops`, `firefox`, `profile`); Kai answers `welcome` (`kai_version`, `protocol`)
//!   or `refused` with a reason ("update Kai", "update the extension") and closes. A peer the
//!   check refuses gets `refused` at once.
//! - **The links** ([`links`]): one per connection, so one per Firefox profile or instance. The
//!   browser provider (the design's WP5) sends operations and waits for their results, follows
//!   the extensions' events and answers the network gate's questions through [`Links`]; a link
//!   that comes or goes signals the host's registry, which lists the tools again.
//! - **The sidebar's requests** ([`sidebar`], design §8.2, §8.3): the extension's `req` frames, the
//!   Chat's requests only, sent to lmgw over Kai's device link and answered as `res`, `chunk` and
//!   `end` frames under a window the sidebar's `ack`s move; `abort` ends one. A request that starts
//!   a turn marks its thread, so the voice reads that turn as the sidebar's, not Kai's own.
//! - **The browser tools** (design §5, WP5, WP6): their contract is
//!   `kai_desktop::contract::browser`; [`provider`] lists those whose operations a linked extension
//!   serves (D3), [`tools`] runs them (the tab tools, `browser_context`, the reading tools and
//!   the screenshots, the acting tools, the hints and `element_point`, Firefox's own features,
//!   the source and the DOM, the style sheets, the scripts and reading aloud) over [`links`],
//!   resolving a `tab` across
//!   the linked profiles ([`tabs`]) and a `ref` to the link that gave it out ([`refs`], Kai's one
//!   number space for the profiles' refs).
//! - **Their policy** (design §6.4): [`policy`] is `[browser]`'s part of the host's K15 policy (the
//!   hands-and-eyes switch, the focus rule's exception, [`sites`]' rules, the commit guard, the
//!   trusted press), and [`moves`] reads the extension's tab facts and keeps the tabs Kai moved,
//!   for the focus rule. A press that needs a real key goes through the desktop ([`desk`]), and
//!   every input Kai sends through the desktop is bracketed by the extension's input spans
//!   ([`spans`]), so Firefox never takes it for the user's. The site rules hold for the desktop's
//!   tools too ([`shown`]): what Firefox's window shows on the desktop is captured, read or acted
//!   in by its tab in front's site.
//! - **Events and announcements** (design §7.1, §7.2: [`announce`]): `events.config@1` after each
//!   `welcome` and at each change of `browser.announce` or the switch, the site rules on each
//!   event, a sentence per event spoken by Kai itself (`crate::voice::speak`), and the list
//!   `browser_context` reads.
//! - **The warning** of design §8.3 ([`warning`]): Kai's lmgw admin level with a Firefox linked.
//! - **The install entry** of design §10 ([`install`]): the settings window's "Install Kai for
//!   Firefox", which opens the signed XPI's link compiled into the build (`KAI_FIREFOX_INSTALL_URL`),
//!   and whether an extension is linked now.
//!
//! Kai's side never closes a link for being quiet and has no deadline of its own on an operation:
//! a call ends with its result, its caller's cancellation or the link.

pub mod announce;
pub mod chunk;
pub mod desk;
pub mod frame;
pub mod install;
pub mod links;
pub mod moves;
pub mod peer;
pub mod policy;
pub mod provider;
pub mod refs;
pub mod server;
pub mod settings;
pub mod shown;
pub mod sidebar;
pub mod sites;
pub mod spans;
pub mod tabs;
pub mod tools;
pub mod warning;

#[cfg(test)]
mod tests;

pub use links::Links;

/// The link's protocol (design §4.2): one integer, raised by a change that is not additive.
pub const PROTOCOL: u32 = 1;
