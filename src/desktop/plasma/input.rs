//! `type_text` and `key_press` through KWin's `org_kde_kwin_fake_input` (K23's plasma provider,
//! WP13 (h)): the interface is restricted, so KWin grants it per `.desktop` file
//! (`X-KDE-Wayland-Interfaces=org_kde_kwin_fake_input`, ruling 16) and shows no prompt at use time:
//! the owner's confirmation is K15's, on Kai's side. Version 6 sends a character as the key of its
//! keysym (`keyboard_keysym`), so the keymap needs no work here; keys go as evdev codes
//! (`keyboard_key`), a chord all down in order, then all up in reverse.
//!
//! - **One short connection per call**, on a blocking thread: connect, bind, authenticate, send,
//!   close. KWin judges the grant when the connection binds, so a `.desktop` written while Kai runs
//!   is honoured by the next call; a build without the grant gets [`Failure::NotGranted`] naming
//!   it. The probe (a bind that is dropped at once) lists the capability only where the grant
//!   exists, else `fdo`'s portal is tried.
//! - **Pacing:** every event is followed by a round trip to KWin (so the order and the arrival are
//!   certain), every release by a short pause (`caps::input::RELEASE_PAUSE`); it is not a limit,
//!   the whole text is typed.
//! - **The guard** (`caps::input::Guard`) is asked on the thread, before each character and each
//!   chord; a refusal stops the call there.
//! - **Cancelling:** lmgw's cancel drops the call; the thread stops before the next event and
//!   releases the keys it still holds, so no key stays down.
//! - **Nothing typed or pressed is logged**: this file never sees the text as text, only keysyms,
//!   and logs neither.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use kai_desktop::caps::input::{Chord, Guard, Input, RELEASE_PAUSE};
use kai_desktop::caps::{Capability, Failure, Fut};
use wayland_client::globals::{BindError, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

use super::protocol::fake_input::org_kde_kwin_fake_input::{self, OrgKdeKwinFakeInput};
use crate::desktop::Env;
use crate::desktop::probe::{Offer, Service};

/// The restricted interface (ruling 16).
pub const FAKE_INPUT: &str = "org_kde_kwin_fake_input";

/// The first version with `keyboard_keysym`.
const KEYSYM_VERSION: u32 = 6;

#[derive(Debug, Clone, Copy)]
pub struct FakeInput {
    /// A bound for connecting and binding (`dbus.timeout_s`); sending is bounded by the events.
    pub timeout: Duration,
}

fn not_granted() -> Failure {
    Failure::NotGranted {
        what: format!("KWin grants {FAKE_INPUT} per .desktop file"),
    }
}

/// The plasma provider's offer for Input: ready where KWin binds the interface for this process.
pub async fn offer(env: &Env) -> Offer {
    let why = |w: String| Offer::absent(Capability::Input, w);
    if !env.wayland {
        return why("no Wayland display".into());
    }
    let input = FakeInput {
        timeout: env.timeout,
    };
    match input.bound().await {
        Ok(()) => Offer::ready(
            Service::Input(Arc::new(input)),
            "KWin's fake input: characters as keysyms, keys as evdev codes (granted per .desktop \
             file)",
        ),
        Err(Failure::NotGranted { what }) => why(format!(
            "this build holds no grant: {what} (the installed Kai's .desktop file carries it)"
        )),
        Err(e) => why(e.to_string()),
    }
}

/// What the compositor said to the connection: nothing the client acts on.
struct State;

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<OrgKdeKwinFakeInput, ()> for State {
    fn event(
        _: &mut Self,
        _: &OrgKdeKwinFakeInput,
        _: org_kde_kwin_fake_input::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// A bound, authenticated fake input; keys still held are released when it is dropped.
struct Pad {
    fi: OrgKdeKwinFakeInput,
    queue: EventQueue<State>,
    state: State,
    held: Vec<u32>,
}

impl Pad {
    fn open() -> Result<Pad, Failure> {
        let conn = Connection::connect_to_env()
            .map_err(|e| Failure::Failed(format!("no Wayland display: {e}")))?;
        let (globals, queue) = registry_queue_init::<State>(&conn)
            .map_err(|e| Failure::Failed(format!("the compositor's globals: {e}")))?;
        let qh = queue.handle();
        let fi = match globals.bind::<OrgKdeKwinFakeInput, _, _>(&qh, 1..=KEYSYM_VERSION, ()) {
            Ok(fi) => fi,
            Err(BindError::NotPresent) => return Err(not_granted()),
            Err(e) => return Err(Failure::Failed(format!("cannot bind {FAKE_INPUT}: {e}"))),
        };
        fi.authenticate(
            "Kai".into(),
            "types and presses the keys the owner confirmed".into(),
        );
        let mut pad = Pad {
            fi,
            queue,
            state: State,
            held: Vec::new(),
        };
        pad.flush()?;
        Ok(pad)
    }

    /// A round trip: KWin has handled everything sent so far.
    fn flush(&mut self) -> Result<(), Failure> {
        self.queue
            .roundtrip(&mut self.state)
            .map(|_| ())
            .map_err(|e| Failure::Failed(format!("KWin ended the fake input connection: {e}")))
    }

    /// Everything sent so far has reached KWin; after a release, a pause as well.
    fn pace(&mut self, released: bool) -> Result<(), Failure> {
        self.flush()?;
        if released {
            std::thread::sleep(RELEASE_PAUSE);
        }
        Ok(())
    }

    fn key(&mut self, code: u32, down: bool) -> Result<(), Failure> {
        self.fi.keyboard_key(code, u32::from(down));
        if down {
            self.held.push(code);
        } else {
            self.held.retain(|c| *c != code);
        }
        self.pace(!down)
    }

    fn keysym(&mut self, keysym: u32, down: bool) -> Result<(), Failure> {
        self.fi.keyboard_keysym(keysym, u32::from(down));
        self.pace(!down)
    }
}

impl Drop for Pad {
    fn drop(&mut self) {
        for code in std::mem::take(&mut self.held).into_iter().rev() {
            self.fi.keyboard_key(code, 0);
        }
        let _ = self.queue.roundtrip(&mut self.state);
    }
}

/// KWin lists the restricted interface in the registry only for a process it grants it to.
fn offered() -> Result<(), Failure> {
    let conn = Connection::connect_to_env()
        .map_err(|e| Failure::Failed(format!("no Wayland display: {e}")))?;
    let (globals, _queue) = registry_queue_init::<State>(&conn)
        .map_err(|e| Failure::Failed(format!("the compositor's globals: {e}")))?;
    let listed = globals
        .contents()
        .with_list(|l| l.iter().any(|g| g.interface == FAKE_INPUT));
    if listed { Ok(()) } else { Err(not_granted()) }
}

/// What a call sends.
enum Op {
    Text(Vec<u32>),
    Keys(Vec<Chord>),
}

fn stopped() -> Failure {
    Failure::Failed("the call was cancelled before it finished".into())
}

/// Sends `op`, asking `guard` (on `rt`, from this blocking thread) before each unit.
fn send(
    op: &Op,
    cancel: &AtomicBool,
    guard: &dyn Guard,
    rt: &tokio::runtime::Handle,
) -> Result<(), Failure> {
    let mut pad = Pad::open()?;
    match op {
        Op::Text(keysyms) => {
            if pad.fi.version() < KEYSYM_VERSION {
                return Err(Failure::Refused(format!(
                    "KWin's fake input is version {}; typing text needs version {KEYSYM_VERSION}",
                    pad.fi.version()
                )));
            }
            for (done, &k) in keysyms.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    return Err(stopped());
                }
                rt.block_on(guard.check(done))?;
                pad.keysym(k, true)?;
                pad.keysym(k, false)?;
            }
        }
        Op::Keys(chords) => {
            for (done, Chord(codes)) in chords.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    return Err(stopped());
                }
                rt.block_on(guard.check(done))?;
                for &c in codes {
                    pad.key(c, true)?;
                }
                for &c in codes.iter().rev() {
                    pad.key(c, false)?;
                }
            }
        }
    }
    Ok(())
}

/// Sets the flag when the call's future is dropped, which stops the thread between two events.
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl FakeInput {
    /// Whether KWin offers this process the interface (the probe's check): it is read from the
    /// registry's list, nothing is bound or sent.
    async fn bound(&self) -> Result<(), Failure> {
        let work = tokio::task::spawn_blocking(offered);
        match tokio::time::timeout(self.timeout, work).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => Err(Failure::Failed(format!("the fake input check failed: {e}"))),
            Err(_) => Err(Failure::Failed(format!(
                "the compositor did not answer within {} s (dbus.timeout_s)",
                self.timeout.as_secs()
            ))),
        }
    }

    async fn run(&self, op: Op, guard: Arc<dyn Guard>) -> Result<(), Failure> {
        let cancel = Arc::new(AtomicBool::new(false));
        let _cancel = CancelOnDrop(cancel.clone());
        let rt = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || send(&op, &cancel, guard.as_ref(), &rt))
            .await
            .map_err(|e| Failure::Failed(format!("the fake input thread failed: {e}")))?
    }
}

impl Input for FakeInput {
    fn type_keysyms<'a>(&'a self, keysyms: &'a [u32], guard: Arc<dyn Guard>) -> Fut<'a, ()> {
        Box::pin(self.run(Op::Text(keysyms.to_vec()), guard))
    }

    fn press_chords<'a>(&'a self, chords: &'a [Chord], guard: Arc<dyn Guard>) -> Fut<'a, ()> {
        Box::pin(self.run(Op::Keys(chords.to_vec()), guard))
    }
}
