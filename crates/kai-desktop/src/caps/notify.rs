//! The Notify capability (K23): do not disturb for `desktop_context` (WP14), and the owner-facing
//! `notify`, `ask` and `dnd` tools (WP16b).

use super::Fut;

pub trait Notify: Send + Sync {
    /// The server has a do-not-disturb state to read and switch (`dnd` is not listed without it).
    fn can_dnd(&self) -> bool;

    /// Do not disturb is on: the notification server holds notifications back. A server with no
    /// such state (the freedesktop base) answers `Failure::Refused`.
    fn dnd(&self) -> Fut<'_, bool>;

    /// Switches do not disturb on or off. Only what Kai switched on can be switched off by it: the
    /// answer says when the server stays in do not disturb for another reason.
    fn set_dnd(&self, on: bool) -> Fut<'_, Dnd>;

    /// Shows a notification.
    fn show<'a>(&'a self, title: &'a str, body: &'a str) -> Fut<'a, ()>;

    /// Shows `question` with one button per choice and waits for the owner. A server that goes
    /// away while the question is open ends the wait with a failure that says so.
    fn ask<'a>(&'a self, question: &'a str, choices: &'a [String]) -> Fut<'a, Asked>;
}

/// The end of an `ask`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asked {
    /// The choice's position. `None`: the owner dismissed the question, or the server closed it,
    /// without choosing.
    pub choice: Option<usize>,
    /// Do not disturb was on when the question was shown: it was sent as critical, which Plasma
    /// shows through do not disturb.
    pub under_dnd: bool,
}

/// Do not disturb after `set_dnd`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dnd {
    /// The server holds notifications back now.
    pub on: bool,
}
