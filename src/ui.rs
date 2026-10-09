//! The GTK main thread (§2): the glib loop, the layer-shell windows and WebKit views (the ring
//! overlay, [`crate::overlay`]; the tool-controlled overlays, [`crate::widgets`]), and Kai's two
//! pairing windows, plain GTK3:
//!
//! - **"Pair with lmgw…"**, the paste dialog. It parses the pasted link as it changes and shows
//!   the gateway, the device name, what the pairing replaces and the plain-http warning before its
//!   Pair button can be pressed, so the owner confirms there. What it replaces is checked again at
//!   the commit: a pairing that changed while the dialog was open makes it show the facts again
//!   instead (review KAI-2 K2-5). Once paired, the link is cleared from the clipboard.
//! - **The confirmation dialog** for a link the scheme handler brought (a click in a browser, or
//!   any process of the session calling `Pairing1.Pair`, K13). It shows the same facts; Cancel is
//!   the default, and closing the window declines. One at a time: while it is open, Kai refuses
//!   further links, so nothing replaces it under the owner's pointer (review KAI-2 K2-4).
//! - **The approval card** (WP19, [`crate::approvals::overlay`]): a layer-shell surface with the
//!   keyboard on demand, for a thread call that waits for the owner's approval.
//! - **A hosted tool's confirmation** (K15, PD4, [`crate::host::confirm`]): who calls which tool
//!   with which arguments, and the time lmgw still waits. Deny is the default and has the focus;
//!   closing the window denies; it closes by itself when lmgw cancels the call or the time is up.
//!   The arguments scroll within what the smallest workarea leaves beside a held MCP Apps view's
//!   strip above and below, never shortened; the buttons stay below them. One window per call.
//!
//! Bridges: tokio reaches GTK through an `async_channel` drained by
//! `glib::MainContext::spawn_local` ([`UiCmd`]); GTK reaches tokio through the core's tokio mpsc
//! ([`Command`]), with a `oneshot` for answers.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use tokio::sync::{mpsc, oneshot};

use crate::audio::LevelTaps;
use crate::core::{Command, PasteError};
use crate::overlay::{self, OverlayCmd, RingSettings};
use crate::pairing::{FactLine, PairFacts, PairingLink};
use crate::settings::window::SettingsWindow;

pub enum UiCmd {
    /// The paste dialog; `current` is the configured `lmgw.url` (what a pairing would replace). An
    /// open one is raised and shows its facts against `current`.
    ShowPairDialog { current: Option<String> },
    /// Ask the owner to confirm a link the scheme handler brought.
    ConfirmPair(ConfirmRequest),
    /// The link with this id no longer waits (paired by paste): close its dialog without an
    /// answer.
    WithdrawConfirm(u64),
    /// The configuration window (WP21b): opened, or brought to the front when it is open.
    ShowSettings,
    /// For the ring overlay.
    Overlay(OverlayCmd),
    /// Ask the owner about a hosted tool's call (PD4).
    ConfirmTool(crate::host::confirm::ConfirmRequest),
    /// For the tool-controlled overlays (WP10, `crate::widgets`).
    Widgets(crate::widgets::WidgetsCmd),
    /// The screen locked (`true`) or unlocked (`crate::screen_lock`): the widgets' surfaces are
    /// unmapped while it is locked and mapped again after.
    ScreenLocked(bool),
    /// The core has finished its quit sequence: the overlays go (§2's quit step 4), then the glib
    /// loop ends.
    Quit,
}

/// What the confirmation dialog shows (K13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmRequest {
    pub id: u64,
    pub facts: PairFacts,
    /// A live-test build: the live script minted this link, so it is confirmed with no dialog
    /// (`crate::live_test`, review KAI-2 K2-2).
    #[cfg(feature = "live-test")]
    pub live_accept: bool,
}

/// The confirmation dialog's lines, in order.
pub fn confirm_lines(r: &ConfirmRequest) -> Vec<FactLine> {
    let mut lines = vec![FactLine::Plain(
        "A pairing link asks Kai to use this gateway with the device key it carries. Pair only if \
         you just opened the link from lmgw's Devices card; Cancel changes nothing."
            .into(),
    )];
    lines.extend(r.facts.lines());
    lines
}

/// The open paste dialog: its window, the pairing its facts are shown against, and how to show
/// them again.
struct PasteDialog {
    window: gtk::Window,
    current: Rc<RefCell<Option<String>>>,
    refresh: Rc<dyn Fn()>,
}

type Slot = Rc<RefCell<Option<PasteDialog>>>;

/// The open confirmation dialog: its link's id, its window, and whether it was answered (or
/// withdrawn), so closing it does not answer twice.
type ConfirmSlot = Rc<RefCell<Option<(u64, gtk::Window, Rc<Cell<bool>>)>>>;

/// The open configuration window, if any (one at a time; closed means gone, K1's RAM rule).
type SettingsSlot = Rc<RefCell<Option<SettingsWindow>>>;

/// Runs the glib loop on the calling (main) thread until [`UiCmd::Quit`]. `gtk::init` must have
/// succeeded on this thread.
pub fn run(
    ui: async_channel::Receiver<UiCmd>,
    cmds: mpsc::UnboundedSender<Command>,
    ring: RingSettings,
    taps: LevelTaps,
) {
    let paste: Slot = Rc::default();
    let confirm: ConfirmSlot = Rc::default();
    let settings: SettingsSlot = Rc::default();
    let notes = cmds.clone();
    let overlay = overlay::start(
        ring,
        taps,
        Box::new(move |note| {
            let _ = notes.send(Command::RingNote(note));
        }),
    );
    let widgets = crate::widgets::start();
    // WP19: the approval card (`crate::approvals`), reached from the voice through its own channel.
    let approvals = crate::approvals::overlay::start();
    glib::MainContext::default().spawn_local(async move {
        while let Ok(cmd) = ui.recv().await {
            match cmd {
                UiCmd::ShowPairDialog { current } => show_paste_dialog(&paste, &cmds, current),
                #[cfg(feature = "live-test")]
                UiCmd::ConfirmPair(req) if req.live_accept => {
                    tracing::warn!(
                        id = req.id,
                        url = %req.facts.url,
                        "live-test: the pairing link is confirmed without a dialog (the live \
                         script minted it)"
                    );
                    let _ = cmds.send(Command::ConfirmPair {
                        id: req.id,
                        accept: true,
                    });
                }
                UiCmd::ConfirmPair(req) => show_confirm_dialog(&confirm, &cmds, req),
                UiCmd::WithdrawConfirm(id) => withdraw(&confirm, Some(id)),
                UiCmd::ShowSettings => show_settings(&settings, &cmds),
                UiCmd::Overlay(cmd) => overlay.send(cmd),
                UiCmd::ConfirmTool(req) => show_tool_dialog(req),
                UiCmd::Widgets(cmd) => widgets.send(cmd),
                UiCmd::ScreenLocked(locked) => {
                    widgets.send(crate::widgets::WidgetsCmd::Locked(locked))
                }
                UiCmd::Quit => break,
            }
        }
        widgets.quit().await;
        approvals.quit().await;
        overlay.quit().await;
        let open = paste.borrow_mut().take();
        if let Some(d) = open {
            d.window.close();
        }
        withdraw(&confirm, None);
        // Dropped: the window, its view and its context go with it.
        drop(settings.borrow_mut().take());
        gtk::main_quit();
    });
    gtk::main();
}

/// Closes the confirmation dialog without answering: the one for `id`, or any.
fn withdraw(slot: &ConfirmSlot, id: Option<u64>) {
    let open = slot
        .borrow_mut()
        .take_if(|(i, _, _)| id.is_none_or(|id| id == *i));
    if let Some((_, window, answered)) = open {
        answered.set(true);
        window.close();
    }
}

/// A label for one line; warnings in Breeze's neutral (orange) colour.
fn line_label(line: &FactLine) -> gtk::Label {
    let label = gtk::Label::new(None);
    match line {
        FactLine::Plain(t) => label.set_text(t),
        FactLine::Warning(t) => label.set_markup(&format!(
            "<span foreground=\"#f67400\">{}</span>",
            glib::markup_escape_text(t)
        )),
    }
    label.set_line_wrap(true);
    label.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_max_width_chars(64);
    label.set_xalign(0.0);
    label.set_selectable(true);
    label
}

fn window(title: &str) -> (gtk::Window, gtk::Box) {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title(title);
    window.set_icon_name(Some(crate::APP_ID));
    window.set_default_size(560, -1);
    window.set_position(gtk::WindowPosition::Center);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_border_width(18);
    window.add(&body);
    (window, body)
}

// ---------------------------------------------------------------------------- the confirmation

fn show_confirm_dialog(
    slot: &ConfirmSlot,
    cmds: &mpsc::UnboundedSender<Command>,
    req: ConfirmRequest,
) {
    // One at a time: the core withdrew an older one; anything left goes without an answer.
    withdraw(slot, None);
    let (window, answered) = build_confirm_dialog(cmds.clone(), &req);
    let slot_on_destroy = slot.clone();
    let id = req.id;
    window.connect_destroy(move |_| {
        slot_on_destroy.borrow_mut().take_if(|(i, _, _)| *i == id);
    });
    window.show_all();
    window.present();
    *slot.borrow_mut() = Some((req.id, window, answered));
}

/// The dialog and its "answered" flag. Cancel is the default and has the focus; closing the window
/// declines.
pub fn build_confirm_dialog(
    cmds: mpsc::UnboundedSender<Command>,
    req: &ConfirmRequest,
) -> (gtk::Window, Rc<Cell<bool>>) {
    let (window, body) = window("Kai: confirm the pairing");
    let heading = gtk::Label::new(None);
    heading.set_markup("<b>Pair Kai with this lmgw?</b>");
    heading.set_xalign(0.0);
    body.pack_start(&heading, false, false, 0);
    for line in confirm_lines(req) {
        body.pack_start(&line_label(&line), false, false, 0);
    }
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let pair = gtk::Button::with_label("Pair with this gateway");
    cancel.set_can_default(true);
    buttons.pack_start(&cancel, false, false, 0);
    buttons.pack_start(&pair, false, false, 0);
    body.pack_end(&buttons, false, false, 0);
    window.set_default(Some(&cancel));
    window.set_focus(Some(&cancel));

    let answered = Rc::new(Cell::new(false));
    let id = req.id;
    let answer = {
        let (cmds, answered) = (cmds.clone(), answered.clone());
        move |accept: bool| {
            if !answered.replace(true) {
                let _ = cmds.send(Command::ConfirmPair { id, accept });
            }
        }
    };
    let (w, a) = (window.clone(), answer.clone());
    cancel.connect_clicked(move |_| {
        a(false);
        w.close();
    });
    let (w, a) = (window.clone(), answer.clone());
    pair.connect_clicked(move |_| {
        a(true);
        w.close();
    });
    // The window manager's close, or Kai's quit: declined, unless already answered.
    window.connect_destroy(move |_| answer(false));
    hold_views(&window);
    (window, answered)
}

/// While `window` (a Kai confirmation) exists, MCP Apps views take no click and shrink to Kai's
/// strip (`crate::widgets::DialogOpen`, §10.1).
fn hold_views(window: &gtk::Window) {
    let open = RefCell::new(Some(crate::widgets::DialogOpen::new()));
    window.connect_destroy(move |_| {
        open.borrow_mut().take();
    });
}

// ---------------------------------------------------------------------------- a tool's confirmation

/// How often the tool dialog looks at its call (the time left, a cancel).
const TOOL_DIALOG_TICK: std::time::Duration = std::time::Duration::from_millis(250);

fn show_tool_dialog(req: crate::host::confirm::ConfirmRequest) {
    let window = build_tool_dialog(req);
    window.show_all();
    window.present();
}

/// The dialog for one call (module doc): [`crate::host::confirm::DialogModel`] laid out. Its answer
/// goes once; the host's end closing (a cancel, the time up, the link gone) closes it. The
/// arguments scroll, the buttons stay below them.
pub fn build_tool_dialog(req: crate::host::confirm::ConfirmRequest) -> gtk::Window {
    use crate::host::confirm::{Button, DialogModel, Tick, args_max_height};
    let (window, body) = window("Kai: confirm a tool call");
    let (text, model) = DialogModel::new(req);
    let model = Rc::new(RefCell::new(model));
    let heading = gtk::Label::new(None);
    heading.set_markup(&format!(
        "<b>{}</b>",
        glib::markup_escape_text(&text.heading)
    ));
    heading.set_xalign(0.0);
    heading.set_line_wrap(true);
    heading.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
    heading.set_max_width_chars(64);
    body.pack_start(&heading, false, false, 0);
    body.pack_start(
        &line_label(&FactLine::Plain(text.description.clone())),
        false,
        false,
        0,
    );
    if let Some(w) = &text.warning {
        body.pack_start(&line_label(&FactLine::Warning(w.clone())), false, false, 0);
    }
    for n in &text.notes {
        body.pack_start(&line_label(&FactLine::Plain(n.clone())), false, false, 0);
    }
    // The arguments: scrolled, never cut; as tall as they are up to what the workarea leaves.
    let args = gtk::Box::new(gtk::Orientation::Vertical, 6);
    for line in &text.args {
        args.pack_start(&line_label(&FactLine::Plain(line.clone())), false, false, 0);
    }
    let scroller = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_propagate_natural_height(true);
    body.pack_start(&scroller, true, true, 0);
    // The preparation's checkbox (`routine_enable`'s grant), unticked, outside the scrolled part.
    for (index, label) in text.options.iter().enumerate() {
        let check = gtk::CheckButton::with_label(label);
        if let Some(l) = check.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            l.set_line_wrap(true);
            l.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
            l.set_max_width_chars(64);
        }
        let m = model.clone();
        check.connect_toggled(move |c| m.borrow_mut().set_option(index, c.is_active()));
        body.pack_start(&check, false, false, 0);
    }
    let left = gtk::Label::new(None);
    left.set_xalign(0.0);
    body.pack_start(&left, false, false, 0);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    body.pack_end(&buttons, false, false, 0);
    for b in Button::ALL {
        let button = gtk::Button::with_label(b.label());
        buttons.pack_start(&button, false, false, 0);
        if b == Button::DEFAULT {
            button.set_can_default(true);
            window.set_default(Some(&button));
            window.set_focus(Some(&button));
        }
        let (w, m) = (window.clone(), model.clone());
        button.connect_clicked(move |_| {
            m.borrow_mut().press(b);
            w.close();
        });
    }
    let first = model.borrow().tick(std::time::Instant::now());
    if let Tick::Left(Some(t)) = first {
        left.set_text(&t);
    }
    // The rest of the dialog, measured while the scroller is still empty, and one argument line.
    let rest = body.preferred_height().1;
    let one_line = args
        .children()
        .first()
        .map_or(0, |l| l.preferred_height().1);
    scroller.add(&args);
    // A held MCP Apps view's strip and margin stay free above and below (§10.1).
    let reserve = i32::try_from(crate::widgets::held_reserve()).unwrap_or(i32::MAX);
    if let Some(h) = args_max_height(&workarea_heights(), rest, one_line, reserve) {
        scroller.set_max_content_height(h);
    }

    // The window manager's close, or Kai's quit: denied, unless already answered.
    let m = model.clone();
    window.connect_destroy(move |_| m.borrow_mut().destroyed());
    hold_views(&window);
    let w = window.clone();
    glib::timeout_add_local(TOOL_DIALOG_TICK, move || {
        // Read first: closing destroys the window, whose handler borrows the model.
        let tick = model.borrow().tick(std::time::Instant::now());
        match tick {
            Tick::Close => {
                w.close();
                return glib::ControlFlow::Break;
            }
            Tick::Left(Some(t)) => left.set_text(&t),
            Tick::Left(None) => {}
        }
        glib::ControlFlow::Continue
    });
    window
}

/// Every monitor's workarea height (application pixels). Under Wayland GTK 3 knows no panels, so
/// it is the monitor's height there.
fn workarea_heights() -> Vec<i32> {
    let Some(display) = gtk::gdk::Display::default() else {
        return Vec::new();
    };
    (0..display.n_monitors())
        .filter_map(|i| display.monitor(i))
        .map(|m| m.workarea().height())
        .collect()
}

// ---------------------------------------------------------------------------- the configuration window

fn show_settings(slot: &SettingsSlot, cmds: &mpsc::UnboundedSender<Command>) {
    if let Some(w) = slot.borrow().as_ref() {
        w.present();
        return;
    }
    let on_closed = slot.clone();
    let window = SettingsWindow::open(cmds.clone(), move || {
        // Dropped after the signal returns: the window is mid-destroy, and a dropped
        // `SettingsWindow` destroys it once more (a no-op), so this is safe either way.
        let slot = on_closed.clone();
        glib::idle_add_local_once(move || drop(slot.borrow_mut().take()));
    });
    *slot.borrow_mut() = Some(window);
}

// ---------------------------------------------------------------------------- the paste dialog

fn show_paste_dialog(slot: &Slot, cmds: &mpsc::UnboundedSender<Command>, current: Option<String>) {
    if let Some(d) = slot.borrow().as_ref() {
        // Shown again: its facts against the pairing Kai has now.
        *d.current.borrow_mut() = current;
        (d.refresh)();
        d.window.present();
        return;
    }
    let d = build_paste_dialog(cmds.clone(), current);
    let slot_on_destroy = slot.clone();
    d.window.connect_destroy(move |_| {
        slot_on_destroy.borrow_mut().take();
    });
    d.window.show_all();
    d.window.present();
    *slot.borrow_mut() = Some(d);
}

/// What the paste dialog shows under the entry for `text`: the facts of a link that parses, else
/// why it does not (nothing for an empty entry). The second element: Pair may be pressed.
pub fn paste_lines(text: &str, current: Option<&str>) -> (Vec<FactLine>, bool) {
    if text.trim().is_empty() {
        return (Vec::new(), false);
    }
    match PairingLink::parse(text) {
        Ok(link) => match crate::pairing::verify_fingerprint(&link) {
            Ok(()) => (PairFacts::new(&link, current).lines(), true),
            Err(why) => (vec![FactLine::Warning(why)], false),
        },
        Err(e) => (vec![FactLine::Warning(e.to_string())], false),
    }
}

fn build_paste_dialog(
    cmds: mpsc::UnboundedSender<Command>,
    current: Option<String>,
) -> PasteDialog {
    let (window, body) = window("Pair with lmgw");
    let current = Rc::new(RefCell::new(current));

    let intro = line_label(&FactLine::Plain(
        "Paste the pairing link that lmgw shows when you pair a device (Usage → Keys → Devices). \
         It starts with lmgw-pair: and carries the device key, which Kai keeps only in the wallet. \
         Check the gateway and the device below before you press Pair. Once paired, Kai clears the \
         link from the clipboard; Klipper's history may still hold a copy."
            .into(),
    ));
    intro.set_selectable(false);

    // The link carries a key: masked like a password, so it does not show on a shared screen.
    let entry = gtk::Entry::new();
    entry.set_visibility(false);
    entry.set_input_purpose(gtk::InputPurpose::Password);
    entry.set_placeholder_text(Some("lmgw-pair:?v=1&url=…"));

    let facts = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let message = gtk::Label::new(None);
    message.set_line_wrap(true);
    message.set_max_width_chars(64);
    message.set_xalign(0.0);
    message.set_selectable(true);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let pair = gtk::Button::with_label("Pair");
    pair.style_context().add_class("suggested-action");
    pair.set_sensitive(false);
    buttons.pack_start(&cancel, false, false, 0);
    buttons.pack_start(&pair, false, false, 0);

    body.pack_start(&intro, false, false, 0);
    body.pack_start(&entry, false, false, 0);
    body.pack_start(&facts, false, false, 0);
    body.pack_start(&message, false, false, 0);
    body.pack_end(&buttons, false, false, 0);

    let w = window.clone();
    cancel.connect_clicked(move |_| w.close());

    // The facts for the entry's text against the pairing Kai has (the message is the caller's).
    let refresh: Rc<dyn Fn()> = {
        let (f, p, e, c) = (facts.clone(), pair.clone(), entry.clone(), current.clone());
        Rc::new(move || {
            for child in f.children() {
                f.remove(&child);
            }
            let (lines, ok) = paste_lines(&e.text(), c.borrow().as_deref());
            for line in &lines {
                f.pack_start(&line_label(line), false, false, 0);
            }
            f.show_all();
            p.set_sensitive(ok);
        })
    };
    let (r, m) = (refresh.clone(), message.clone());
    entry.connect_changed(move |_| {
        m.set_text("");
        r();
    });

    let (w, c, r) = (window.clone(), current.clone(), refresh.clone());
    pair.connect_clicked(move |button| {
        let text = entry.text().to_string();
        let Ok(link) = PairingLink::parse(&text) else {
            return;
        };
        let (reply, answer) = oneshot::channel();
        let shown = c.borrow().clone();
        if cmds
            .send(Command::PairPasted { link, shown, reply })
            .is_err()
        {
            return show_error(&message, "Kai is quitting");
        }
        button.set_sensitive(false);
        message.set_text("Storing the key in the wallet…");
        let (w, c, r, button, message, entry) = (
            w.clone(),
            c.clone(),
            r.clone(),
            button.clone(),
            message.clone(),
            entry.clone(),
        );
        glib::MainContext::default().spawn_local(async move {
            match answer.await {
                Ok(Ok(())) => {
                    entry.set_text("");
                    clear_clipboard_of(text);
                    w.close();
                }
                Ok(Err(e @ PasteError::Changed { .. })) => {
                    // The facts again, against the pairing Kai has now (K2-5).
                    if let PasteError::Changed { current } = &e {
                        *c.borrow_mut() = current.clone();
                    }
                    r();
                    show_error(&message, &e.to_string());
                }
                Ok(Err(e)) => {
                    show_error(&message, &e.to_string());
                    button.set_sensitive(true);
                }
                Err(_) => show_error(&message, "Kai is quitting"),
            }
        });
    });
    PasteDialog {
        window,
        current,
        refresh,
    }
}

/// After a paste paired (api §1.4: a client drops the link once it is stored, review KAI-2 K2-20):
/// a clipboard that still holds exactly `link` gets a note in its place. Not emptied: Klipper's
/// "prevent empty clipboard" would put the link back from its history.
fn clear_clipboard_of(link: String) {
    let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
    clipboard.request_text(move |cb, text| {
        if text.is_some_and(|t| t.trim() == link.trim()) {
            cb.set_text("(Kai removed the pairing link from the clipboard)");
            tracing::info!("the pairing link was removed from the clipboard");
        }
    });
}

fn show_error(label: &gtk::Label, text: &str) {
    // Breeze's negative colour.
    label.set_markup(&format!(
        "<span foreground=\"#da4453\">{}</span>",
        glib::markup_escape_text(text)
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing::tests::{KEY, good, link};

    fn request(current: Option<&str>) -> ConfirmRequest {
        ConfirmRequest {
            id: 1,
            facts: PairFacts::new(&PairingLink::parse(&good()).unwrap(), current),
            #[cfg(feature = "live-test")]
            live_accept: false,
        }
    }

    #[test]
    fn the_confirmation_says_what_the_link_does() {
        let lines = confirm_lines(&request(None));
        assert!(matches!(&lines[0], FactLine::Plain(t) if t.contains("Cancel changes nothing")));
        assert_eq!(
            lines[1],
            FactLine::Plain("Gateway: http://127.0.0.1:8001".into())
        );
        assert_eq!(lines[2], FactLine::Plain("Device name: desktop".into()));
        assert_eq!(lines.len(), 3);
        let lines = confirm_lines(&request(Some("http://gateway.example:8001")));
        assert!(
            !format!("{lines:?}").contains("newer pairing link"),
            "no link replaces another (K2-4)"
        );
        assert!(
            matches!(lines.last(), Some(FactLine::Warning(t)) if t.contains("http://gateway.example:8001"))
        );
        assert!(
            !format!("{lines:?}").contains(&KEY[12..]),
            "the key is never shown"
        );
    }

    #[test]
    fn the_paste_dialog_shows_the_facts_before_pair() {
        assert_eq!(paste_lines("  ", None), (vec![], false));
        let (lines, ok) = paste_lines(&good(), Some("http://old:1"));
        assert!(ok);
        assert_eq!(
            lines[0],
            FactLine::Plain("Gateway: http://127.0.0.1:8001".into())
        );
        assert!(matches!(&lines[2], FactLine::Warning(t) if t.contains("http://old:1")));
        let (lines, ok) = paste_lines("lmgw-pair:?v=2", None);
        assert!(!ok, "a link that does not parse cannot be paired");
        assert!(matches!(&lines[0], FactLine::Warning(t) if t.contains("v=2")));
        let fp = format!("{}&fp=sha256:{}", good(), "0".repeat(64));
        let (lines, ok) = paste_lines(&fp, None);
        assert!(!ok && matches!(&lines[0], FactLine::Warning(t) if t.contains("refusing")));
        let bad = link(&format!("v=1&url=ftp%3A%2F%2Fh&name=n&key={KEY}"));
        assert!(!format!("{:?}", paste_lines(&bad, None)).contains(&KEY[12..]));
    }

    /// Shows the confirmation dialog once on the desktop, checks it mapped with its buttons, and
    /// closes it itself (no input is injected). The request holds no key at all: the dialog never
    /// sees one, and nothing is stored.
    #[test]
    #[ignore = "shows a window on the owner's desktop for about a second"]
    fn live_the_confirmation_dialog_renders_and_closes() {
        gtk::init().expect("a display");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let req = request(Some("http://192.0.2.20:8001"));
        let (window, answered) = build_confirm_dialog(tx, &req);
        window.show_all();
        let until = std::time::Instant::now() + std::time::Duration::from_millis(1200);
        while std::time::Instant::now() < until {
            gtk::main_iteration_do(false);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            window.is_visible() && window.is_mapped(),
            "the dialog mapped"
        );
        let a = window.allocation();
        eprintln!("the confirmation dialog: {}x{} px", a.width(), a.height());
        assert!(
            a.width() >= 300 && a.height() >= 150,
            "{}x{}",
            a.width(),
            a.height()
        );
        let focus = window
            .focused_widget()
            .and_then(|w| w.downcast::<gtk::Button>().ok());
        assert_eq!(
            focus.and_then(|b| b.label()).as_deref(),
            Some("Cancel"),
            "Cancel has the focus"
        );
        window.close();
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        assert!(answered.get());
        assert!(
            matches!(
                rx.try_recv(),
                Ok(Command::ConfirmPair {
                    id: 1,
                    accept: false
                })
            ),
            "closing the window declines"
        );
    }
}
