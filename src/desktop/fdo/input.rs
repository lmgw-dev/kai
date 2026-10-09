//! `type_text` and `key_press` through the RemoteDesktop portal (K23's fdo provider, WP13 (h)),
//! for a desktop where `plasma` has no fake input (a build without the grant, another desktop).
//!
//! - **A session per call:** `CreateSession`, `SelectDevices` (keyboard only, `persist_mode` 2: until
//!   revoked), `Start`, the key events, `Close`. The desktop's "remote control" indicator shows
//!   only while a call runs.
//! - **Consent:** the first `Start` shows the desktop's own "Remote Control" dialog (portal-kde's);
//!   its "allow restore" box makes the portal hand out a restore token. Kai stores the token the
//!   last `Start` returned ([`TokenFile`]) and offers it to the next `SelectDevices`, so later
//!   sessions start with no dialog. That wait has no bound of Kai's own: the owner answers whenever
//!   they choose, and lmgw's cancel drops the call, which closes the session. An answer of "cancel" is a
//!   refusal that says so. The token is the key to keyboard control, so the file is private (0600,
//!   in a 0700 directory: one that existed is tightened when it is a real directory of this user's,
//!   and refused otherwise; a symlink is never followed), written through a temporary file, and its
//!   text is never logged. `kai --forget-remote-control` removes it. Revoking Kai in the desktop's
//!   settings does not invalidate a stored token (portal-kde restores from it all the same), so
//!   forgetting it is Kai's side.
//! - **Pacing** as for KWin's fake input (`caps::input`): every event waits for the portal's reply,
//!   every release then [`RELEASE_PAUSE`]. On a drop the keys still down are released before the
//!   session closes.
//! - **The guard** is asked before each character and each chord.
//! - **Text** goes as `NotifyKeyboardKeysym` (a press and a release per character), keys as
//!   `NotifyKeyboardKeycode` with evdev codes; a chord is all down, then all up in reverse.
//! - **The app id** is the portal's: Kai's app scope (`app-dev.rootshell.kai-….scope`, K12) gives
//!   it, as for the shortcuts. Nothing of the typed text is logged.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ashpd::desktop::PersistMode;
use ashpd::desktop::remote_desktop::{DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions};
use ashpd::desktop::{ResponseError, Session};
use kai_desktop::caps::input::{Chord, Guard, Input, RELEASE_PAUSE};
use kai_desktop::caps::{Capability, Failure, Fut};

use crate::desktop::probe::{Offer, Service};
use crate::desktop::{Env, call, has_owner, property};
use crate::host::registry::SessionBus;

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const REMOTE_DESKTOP: &str = "org.freedesktop.portal.RemoteDesktop";
/// `AvailableDeviceTypes`' keyboard bit.
const KEYBOARD: u32 = 1;

/// Where the restore token lives: `<Kai's state directory>/remote-desktop-token`.
pub const TOKEN_NAME: &str = "remote-desktop-token";

/// The token file in Kai's state directory (`Paths::state_dir`).
pub fn token_file(state_dir: &Path) -> TokenFile {
    TokenFile(state_dir.join(TOKEN_NAME))
}

/// The stored restore token (module doc).
#[derive(Debug, Clone)]
pub struct TokenFile(pub PathBuf);

/// Makes `dir` (and its parents) where it is missing, and makes it 0700: a directory that existed
/// is tightened only when it is a real directory (not a symlink, which is never followed) owned by
/// this user; anything else is an error and nothing is written there.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: getuid has no preconditions and cannot fail.
    let me = unsafe { libc::getuid() };
    if !meta.file_type().is_dir() || meta.uid() != me {
        return Err(std::io::Error::other(format!(
            "{} is not a directory of this user's (a symlink or another owner's); the token is \
             not stored there",
            dir.display()
        )));
    }
    if meta.mode() & 0o777 != 0o700 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

impl TokenFile {
    /// The token, if one is stored. A file that cannot be read is no token.
    pub fn load(&self) -> Option<String> {
        let t = std::fs::read_to_string(&self.0).ok()?;
        let t = t.trim();
        (!t.is_empty()).then(|| t.to_owned())
    }

    /// Stores `token` privately and atomically; unchanged when it is the stored one.
    pub fn store(&self, token: &str) -> std::io::Result<()> {
        let dir = self.0.parent().unwrap_or(Path::new("."));
        private_dir(dir)?;
        if self.load().as_deref() == Some(token) {
            return Ok(());
        }
        let tmp = self.tmp();
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(token.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &self.0)
    }

    /// Forgets the token (`kai --forget-remote-control`): the next use asks the owner again unless
    /// the desktop's own pre-authorisation stands. Whether one was stored: in the file, or in
    /// [`Self::store`]'s temporary file, which a store cut short (a crash before the rename)
    /// leaves with the token in it, and which goes too.
    pub fn forget(&self) -> std::io::Result<bool> {
        let mut was = false;
        for path in [self.0.clone(), self.tmp()] {
            match std::fs::remove_file(&path) {
                Ok(()) => was = true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(was)
    }

    /// Where [`Self::store`] writes the token before renaming it into place.
    fn tmp(&self) -> PathBuf {
        self.0.with_extension("tmp")
    }
}

pub struct Portal {
    pub bus: SessionBus,
    pub timeout: Duration,
    pub token: Option<TokenFile>,
}

/// The fdo provider's offer for Input: the RemoteDesktop portal, with keyboard control available.
pub async fn offer(env: &Env) -> Offer {
    let why = |w: String| Offer::absent(Capability::Input, w);
    let Some(session) = &env.session else {
        return why("no session bus".into());
    };
    let bus = match session.get().await {
        Ok(b) => b,
        Err(e) => return why(e),
    };
    let t = env.timeout;
    // The probe starts nothing: a portal that is only activatable is taken on trust.
    let running = match has_owner(&bus, PORTAL, t).await {
        Ok(r) => r,
        Err(e) => return why(e),
    };
    if running {
        match property::<u32>(
            &bus,
            PORTAL,
            PORTAL_PATH,
            REMOTE_DESKTOP,
            "AvailableDeviceTypes",
            t,
        )
        .await
        {
            Ok(types) if types & KEYBOARD != 0 => {}
            Ok(_) => return why("the portal's backend cannot control a keyboard".into()),
            Err(_) => {
                return why(
                    "the portal has no RemoteDesktop (the desktop's portal backend lacks it)"
                        .into(),
                );
            }
        }
    } else {
        let names: Vec<String> = match call(
            &bus,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "ListActivatableNames",
            &(),
            t,
        )
        .await
        {
            Ok(n) => n,
            Err(e) => return why(e.to_string()),
        };
        if !names.iter().any(|n| n == PORTAL) {
            return why("xdg-desktop-portal is neither running nor activatable".into());
        }
    }
    let token = (!env.state_dir.as_os_str().is_empty()).then(|| token_file(&env.state_dir));
    Offer::ready(
        Service::Input(Arc::new(Portal {
            bus: session.clone(),
            timeout: t,
            token,
        })),
        "the RemoteDesktop portal with a stored restore token (the first use shows the desktop's \
         dialog)",
    )
}

fn failed(what: &str, e: ashpd::Error) -> Failure {
    match e {
        ashpd::Error::Response(ResponseError::Cancelled) => Failure::Refused(format!(
            "the owner did not allow remote control ({what} was cancelled)"
        )),
        e => Failure::Failed(format!("the RemoteDesktop portal's {what}: {e}")),
    }
}

/// A started portal session; closed when it ends, or when the call is dropped. The keys it holds
/// down are released before the session closes, on a drop too.
struct Started {
    rd: RemoteDesktop,
    session: Option<Session<RemoteDesktop>>,
    /// The keys pressed and not yet released, in the order they went down.
    held: Vec<Held>,
}

/// A key that is down: an evdev code (a chord's) or a keysym (a character's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    Code(u32),
    Sym(u32),
}

/// Releases `held` in reverse, then closes the session; errors are of no use any more.
async fn release_and_close(rd: &RemoteDesktop, session: Session<RemoteDesktop>, held: Vec<Held>) {
    for h in held.into_iter().rev() {
        let up = KeyState::Released;
        let _ = match h {
            Held::Code(c) => {
                rd.notify_keyboard_keycode(&session, c as i32, up, Default::default())
                    .await
            }
            Held::Sym(k) => {
                rd.notify_keyboard_keysym(&session, k as i32, up, Default::default())
                    .await
            }
        };
    }
    let _ = session.close().await;
}

impl Started {
    fn session(&self) -> &Session<RemoteDesktop> {
        self.session.as_ref().expect("closed only by close()")
    }

    async fn close(mut self) {
        if let Some(s) = self.session.take() {
            let held = std::mem::take(&mut self.held);
            release_and_close(&self.rd, s, held).await;
        }
    }

    /// One key event, acknowledged by the portal (its reply), and after a release the pause
    /// (module doc). A press counts as held from before it is sent, so a drop while it is on its
    /// way still releases it.
    async fn send(&mut self, key: Held, down: bool) -> Result<(), Failure> {
        let state = if down {
            if !self.held.contains(&key) {
                self.held.push(key);
            }
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        let session = self.session.as_ref().expect("closed only by close()");
        match key {
            Held::Code(c) => self
                .rd
                .notify_keyboard_keycode(session, c as i32, state, Default::default())
                .await
                .map_err(|e| failed("NotifyKeyboardKeycode", e))?,
            Held::Sym(k) => self
                .rd
                .notify_keyboard_keysym(session, k as i32, state, Default::default())
                .await
                .map_err(|e| failed("NotifyKeyboardKeysym", e))?,
        }
        if !down {
            self.held.retain(|h| *h != key);
            tokio::time::sleep(RELEASE_PAUSE).await;
        }
        Ok(())
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        let Some(s) = self.session.take() else {
            return;
        };
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        // The proxy cannot be cloned, so the task gets one of its own on the same connection.
        let conn = self.rd.connection().clone();
        let held = std::mem::take(&mut self.held);
        rt.spawn(async move {
            if let Ok(rd) = RemoteDesktop::with_connection(conn).await {
                release_and_close(&rd, s, held).await;
            } else {
                let _ = s.close().await;
            }
        });
    }
}

impl Portal {
    async fn start(&self) -> Result<Started, Failure> {
        let conn = self.bus.get().await.map_err(Failure::Failed)?;
        let rd = RemoteDesktop::with_connection(conn)
            .await
            .map_err(|e| match e {
                ashpd::Error::PortalNotFound(_) => {
                    Failure::Refused("the desktop has no RemoteDesktop portal".into())
                }
                e => failed("connection", e),
            })?;
        let session = tokio::time::timeout(self.timeout, rd.create_session(Default::default()))
            .await
            .map_err(|_| {
                Failure::Failed(format!(
                    "the RemoteDesktop portal did not answer CreateSession within {} s \
                     (dbus.timeout_s)",
                    self.timeout.as_secs()
                ))
            })?
            .map_err(|e| failed("CreateSession", e))?;
        let started = Started {
            rd,
            session: Some(session),
            held: Vec::new(),
        };
        match self.negotiate(&started).await {
            Ok(()) => Ok(started),
            Err(e) => {
                started.close().await;
                Err(e)
            }
        }
    }

    /// `SelectDevices` and `Start` on a created session (module doc), then the token's keeping.
    async fn negotiate(&self, started: &Started) -> Result<(), Failure> {
        let stored = self.token.as_ref().and_then(TokenFile::load);
        let select = SelectDevicesOptions::default()
            .set_devices(Some(DeviceType::Keyboard.into()))
            .set_persist_mode(PersistMode::ExplicitlyRevoked)
            .set_restore_token(stored.as_deref());
        started
            .rd
            .select_devices(started.session(), select)
            .await
            .map_err(|e| failed("SelectDevices", e))?;
        // No bound: with no valid token the desktop asks the owner (module doc).
        let response = started
            .rd
            .start(started.session(), None, Default::default())
            .await
            .map_err(|e| failed("Start", e))?
            .response()
            .map_err(|e| failed("Start", e))?;
        if !response.devices().contains(DeviceType::Keyboard) {
            return Err(Failure::Refused(
                "the owner did not allow keyboard control through the portal".into(),
            ));
        }
        if let (Some(file), Some(token)) = (&self.token, response.restore_token())
            && let Err(e) = file.store(token)
        {
            tracing::warn!(
                "desktop: cannot store the remote-desktop restore token ({}): {e}; the next use \
                 may ask the owner again",
                file.0.display()
            );
        }
        Ok(())
    }

    async fn typing(&self, keysyms: &[u32], guard: &dyn Guard) -> Result<(), Failure> {
        let mut s = self.start().await?;
        let mut result = Ok(());
        for (done, &k) in keysyms.iter().enumerate() {
            result = guard.check(done).await;
            if result.is_ok() {
                result = s.send(Held::Sym(k), true).await;
            }
            if result.is_ok() {
                result = s.send(Held::Sym(k), false).await;
            }
            if result.is_err() {
                break;
            }
        }
        s.close().await;
        result
    }

    async fn pressing(&self, chords: &[Chord], guard: &dyn Guard) -> Result<(), Failure> {
        let mut s = self.start().await?;
        let mut result = Ok(());
        for (done, Chord(codes)) in chords.iter().enumerate() {
            result = guard.check(done).await;
            if result.is_err() {
                break;
            }
            for &c in codes {
                if let Err(e) = s.send(Held::Code(c), true).await {
                    result = Err(e);
                    break;
                }
            }
            // Whatever went down comes up again, in reverse, even after a failure.
            for c in codes.iter().rev() {
                if s.held.contains(&Held::Code(*c))
                    && let Err(e) = s.send(Held::Code(*c), false).await
                {
                    result = result.and(Err(e));
                }
            }
            if result.is_err() {
                break;
            }
        }
        s.close().await;
        result
    }
}

impl Input for Portal {
    fn type_keysyms<'a>(&'a self, keysyms: &'a [u32], guard: Arc<dyn Guard>) -> Fut<'a, ()> {
        Box::pin(async move { self.typing(keysyms, guard.as_ref()).await })
    }

    fn press_chords<'a>(&'a self, chords: &'a [Chord], guard: Arc<dyn Guard>) -> Fut<'a, ()> {
        Box::pin(async move { self.pressing(chords, guard.as_ref()).await })
    }
}
