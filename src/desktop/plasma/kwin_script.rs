//! KWin scripting from Kai (WP13 (f): no grant needed): a script is written to a file of Kai's own
//! (`$XDG_RUNTIME_DIR/kai/kwin/`, mode 0600, created new under a random name, in a 0700 directory
//! that must be a real directory of ours), loaded with
//! `org.kde.kwin.Scripting.loadScript(path, plugin)`, run, and unloaded. A KWin script cannot
//! return a value to its loader, so it answers by `callDBus` to an object Kai serves on its own
//! connection (its unique name, so it works before and without the well-known name). Only an
//! answer whose sender is the `org.kde.KWin` owner that loaded the script is taken, and only one
//! that carries the run's token: 128 random bits, in the script's `ARGS` (`ARGS.reply.id`), which
//! is also what the run is looked up by. Any session-bus client can have KWin load a script of its
//! own, and KWin is then the sender of whatever that script says; without the token it cannot
//! answer for Kai's run. What remains: a process of the same user that is not sandboxed can read
//! the 0600 script file (or ptrace Kai) and learn the token. That process can do all that Kai can
//! do to the session anyway; a sandboxed client (Flatpak) cannot reach the runtime directory.
//!
//! At its first run an instance also sweeps the leftovers of Kai processes that died with a
//! script loaded: a `kai-<pid>-<hex>.js` file whose pid is not alive is unloaded from KWin and
//! removed. Nothing else in the directory is touched.
//!
//! Each run waits at most `dbus.timeout_s` for each D-Bus call and for the script's answer, said
//! in the error when it passes. A run dropped part way (the call was cancelled) still unloads its
//! script and removes its file, in a task of its own.
//!
//! **One run at a time** in the process ([`RUNS`], held from `loadScript` until the script is
//! unloaded, by the drop's task too): KWin numbers a loaded script by the length of its list
//! (`/Scripting/Script<n>`), so a load while another run unloads can be given a number that is in
//! use, and the `run` call then reaches no object (`No such object path`). Seen live once the
//! input tools read the focus by a script before every character while a window action ran.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kai_desktop::caps::Failure;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::host::registry::SessionBus;

const KWIN: &str = "org.kde.KWin";
const IFACE: &str = "dev.rootshell.kai.KWinScript";

/// Prepended to every script: `ARGS` (the call's arguments and where to answer), `reply`, and the
/// helpers the scripts share.
const PRELUDE: &str = r#"const ARGS = __ARGS__;
function reply(o) { callDBus(ARGS.reply.service, ARGS.reply.path, ARGS.reply.iface, "Result", ARGS.reply.id, JSON.stringify(o)); }
function bare(id) { return id.toString().replace(/^\{|\}$/g, ""); }
function byId(id) { for (const w of workspace.windowList()) if (bare(w.internalId) === id) return w; return null; }
"#;

/// Appended: the script's `run()` answers its value, or the error it threw.
const EPILOGUE: &str = r#"
try { reply({ ok: true, value: run() }); } catch (e) { reply({ ok: false, error: String(e) }); }
"#;

/// Answers on their way back, by the run's token: the KWin owner that must send it, and where it
/// goes.
type Pending = Mutex<HashMap<String, (String, oneshot::Sender<String>)>>;

struct Inner {
    pending: Pending,
    /// The leftovers of dead processes are swept at the first run.
    swept: tokio::sync::OnceCell<()>,
    /// The receiving object, served on the connection at its first run.
    served: tokio::sync::OnceCell<String>,
}

/// One instance's object path (two instances on one connection, as in tests, never collide).
static INSTANCES: AtomicU64 = AtomicU64::new(0);

/// One run at a time (module doc). Each run is bounded by `dbus.timeout_s` per step, so a wait for
/// it is too.
static RUNS: std::sync::LazyLock<Arc<tokio::sync::Mutex<()>>> =
    std::sync::LazyLock::new(Arc::default);

/// Runs KWin scripts (module doc).
#[derive(Clone)]
pub struct KwinScript {
    bus: SessionBus,
    dir: PathBuf,
    timeout: Duration,
    path: String,
    inner: Arc<Inner>,
}

/// The object a script's `callDBus` reaches.
struct Receiver {
    inner: Arc<Inner>,
}

#[zbus::interface(name = "dev.rootshell.kai.KWinScript")]
impl Receiver {
    /// A script's answer: its run's token and its JSON.
    async fn result(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        id: String,
        json: String,
    ) {
        let sender = header.sender().map(|s| s.as_str().to_owned());
        let mut pending = self.inner.pending.lock().expect("never poisoned");
        match pending.get(&id) {
            Some((owner, _)) if sender.as_deref() == Some(owner.as_str()) => {
                if let Some((_, tx)) = pending.remove(&id) {
                    let _ = tx.send(json);
                }
            }
            Some(_) => tracing::warn!(
                "desktop: a KWin script answer came from a sender that is not KWin; ignored"
            ),
            None => tracing::debug!(
                "desktop: a KWin script answer with no run to go with (a token that is not \
                 one's, or the run is over); ignored"
            ),
        }
    }
}

/// Unloads the script and removes its file when the run ends, however it ends.
struct Cleanup {
    bus: Option<zbus::Connection>,
    plugin: String,
    file: PathBuf,
    id: String,
    inner: Arc<Inner>,
    timeout: Duration,
    /// This run's turn ([`RUNS`]), given up once the script is unloaded.
    turn: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl Cleanup {
    async fn unload(bus: zbus::Connection, plugin: String, timeout: Duration) {
        let body = (plugin.as_str(),);
        let call = bus.call_method(
            Some(KWIN),
            "/Scripting",
            Some("org.kde.kwin.Scripting"),
            "unloadScript",
            &body,
        );
        match tokio::time::timeout(timeout, call).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => tracing::warn!("desktop: KWin did not unload Kai's script {plugin}: {e}"),
            Err(_) => tracing::warn!(
                "desktop: KWin did not unload Kai's script {plugin} within {} s (dbus.timeout_s)",
                timeout.as_secs()
            ),
        }
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
        self.inner
            .pending
            .lock()
            .expect("never poisoned")
            .remove(&self.id);
        if let Some(bus) = self.bus.take()
            && let Ok(rt) = tokio::runtime::Handle::try_current()
        {
            let (plugin, timeout, turn) = (
                std::mem::take(&mut self.plugin),
                self.timeout,
                self.turn.take(),
            );
            rt.spawn(async move {
                Self::unload(bus, plugin, timeout).await;
                drop(turn);
            });
        }
    }
}

impl KwinScript {
    /// Scripts' files go to `dir` (Kai's runtime directory's `kwin`).
    pub fn new(bus: SessionBus, dir: PathBuf, timeout: Duration) -> Self {
        let n = INSTANCES.fetch_add(1, Ordering::Relaxed);
        Self {
            bus,
            dir,
            timeout,
            path: format!("/dev/rootshell/kai/KWinScript/{n}"),
            inner: Arc::new(Inner {
                pending: Mutex::default(),
                swept: tokio::sync::OnceCell::new(),
                served: tokio::sync::OnceCell::new(),
            }),
        }
    }

    fn late(&self, what: &str) -> Failure {
        Failure::Failed(format!(
            "{what} within {} s (dbus.timeout_s)",
            self.timeout.as_secs()
        ))
    }

    async fn call<B>(
        &self,
        bus: &zbus::Connection,
        path: &str,
        iface: &str,
        method: &str,
        body: &B,
    ) -> Result<zbus::Message, Failure>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        let call = bus.call_method(Some(KWIN), path, Some(iface), method, body);
        match tokio::time::timeout(self.timeout, call).await {
            Ok(Ok(m)) => Ok(m),
            Ok(Err(e)) => Err(Failure::Failed(format!("KWin refused {method}: {e}"))),
            Err(_) => Err(self.late(&format!("KWin did not answer {method}"))),
        }
    }

    /// Runs `body` (JavaScript defining `function run()`, whose value is the answer) with `args`
    /// as `ARGS`, and returns its value. A script that throws is [`Failure::Failed`] with its
    /// error.
    pub async fn run(&self, body: &str, args: Value) -> Result<Value, Failure> {
        if !self.dir.is_absolute() {
            return Err(Failure::Failed(
                "Kai has no runtime directory for its KWin scripts (XDG_RUNTIME_DIR)".into(),
            ));
        }
        let bus = self.bus.get().await.map_err(Failure::Failed)?;
        let inner = self.inner.clone();
        let path = self.path.clone();
        let conn = bus.clone();
        self.inner
            .served
            .get_or_try_init(|| async move {
                conn.object_server()
                    .at(path.as_str(), Receiver { inner })
                    .await
                    .map(|_| path)
            })
            .await
            .map_err(|e| Failure::Failed(format!("Kai cannot receive KWin's answers: {e}")))?;
        let owner = {
            let dbus = zbus::fdo::DBusProxy::new(&bus)
                .await
                .map_err(|e| Failure::Failed(e.to_string()))?;
            match tokio::time::timeout(
                self.timeout,
                dbus.get_name_owner(KWIN.try_into().expect("a bus name")),
            )
            .await
            {
                Ok(Ok(o)) => o.to_string(),
                Ok(Err(_)) => return Err(Failure::Failed("KWin is not running".into())),
                Err(_) => return Err(self.late("the session bus did not answer")),
            }
        };
        self.inner
            .swept
            .get_or_init(|| sweep_leftovers(&bus, &self.dir, self.timeout))
            .await;
        let random = |n| {
            random_hex(n).map_err(|e| Failure::Failed(format!("cannot draw random bytes: {e}")))
        };
        // The run's secret, and a name that cannot be guessed to plant something at it.
        let id = random(16)?;
        let plugin = format!("kai-{}-{}", std::process::id(), random(8)?);
        let file = self.dir.join(format!("{plugin}.js"));
        let unique = bus
            .unique_name()
            .map(|n| n.to_string())
            .ok_or_else(|| Failure::Failed("Kai's bus connection has no name".into()))?;
        let mut args = args;
        if let Some(obj) = args.as_object_mut() {
            obj.insert(
                "reply".into(),
                json!({"service": unique, "path": self.path, "iface": IFACE, "id": id}),
            );
        }
        // JSON is a JavaScript literal, but for U+2028 and U+2029 before ES2019.
        let literal = args
            .to_string()
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029");
        let source = format!("{}{body}{EPILOGUE}", PRELUDE.replace("__ARGS__", &literal));
        write_private(&self.dir, &file, &source)
            .map_err(|e| Failure::Failed(format!("cannot write Kai's KWin script: {e}")))?;
        let (tx, rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .expect("never poisoned")
            .insert(id.clone(), (owner, tx));
        let turn = RUNS.clone().lock_owned().await;
        let mut cleanup = Cleanup {
            bus: None,
            plugin: plugin.clone(),
            file: file.clone(),
            id,
            inner: self.inner.clone(),
            timeout: self.timeout,
            turn: Some(turn),
        };
        let loaded = self
            .call(
                &bus,
                "/Scripting",
                "org.kde.kwin.Scripting",
                "loadScript",
                &(file.to_string_lossy().as_ref(), plugin.as_str()),
            )
            .await?;
        // Unloading a plugin that did not load is harmless; one that did must go.
        cleanup.bus = Some(bus.clone());
        let script: i32 = loaded
            .body()
            .deserialize()
            .map_err(|e| Failure::Failed(format!("KWin's loadScript answered oddly: {e}")))?;
        if script < 0 {
            return Err(Failure::Failed(format!(
                "KWin did not load Kai's script (loadScript answered {script})"
            )));
        }
        self.call(
            &bus,
            &format!("/Scripting/Script{script}"),
            "org.kde.kwin.Script",
            "run",
            &(),
        )
        .await?;
        let answer = match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(a)) => a,
            Ok(Err(_)) => return Err(Failure::Failed("Kai's KWin script was dropped".into())),
            Err(_) => return Err(self.late("Kai's KWin script did not answer")),
        };
        // Unloaded now rather than by the guard's task, so the next run finds it gone.
        if let Some(bus) = cleanup.bus.take() {
            Cleanup::unload(bus, plugin, self.timeout).await;
        }
        drop(cleanup);
        let answer: Value = serde_json::from_str(&answer)
            .map_err(|e| Failure::Failed(format!("Kai's KWin script answered no JSON: {e}")))?;
        if answer["ok"].as_bool() == Some(true) {
            Ok(answer["value"].clone())
        } else {
            Err(Failure::Failed(format!(
                "KWin: {}",
                answer["error"].as_str().unwrap_or("the script failed")
            )))
        }
    }
}

/// `n` random bytes as hex, from the system's source.
fn random_hex(n: usize) -> std::io::Result<String> {
    use std::io::Read;
    let mut bytes = vec![0u8; n];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Makes sure `dir` is a real directory of ours that nobody else can enter, creating it 0700 when
/// it is not there. A symlink, a directory of someone else's or one open to group or others is an
/// error: Kai neither follows nor changes it.
fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let bad = |what: &str| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} {what}", dir.display()),
        )
    };
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: geteuid has no arguments and cannot fail.
    if !meta.file_type().is_dir() || meta.file_type().is_symlink() {
        Err(bad("is not a real directory"))
    } else if meta.uid() != unsafe { libc::geteuid() } {
        Err(bad("belongs to someone else"))
    } else if meta.mode() & 0o077 != 0 {
        Err(bad("is open to other users (it must be mode 0700)"))
    } else {
        Ok(())
    }
}

/// Writes `text` to a new `file` in `dir` (checked by [`private_dir`]), mode 0600. A file that is
/// already there, whatever it is, makes this fail.
fn write_private(dir: &std::path::Path, file: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    private_dir(dir)?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(file)?;
    f.write_all(text.as_bytes())
}

/// [`write_private`] as the tests reach it.
#[cfg(test)]
pub fn write_private_for_test(dir: &std::path::Path, name: &str) -> std::io::Result<()> {
    write_private(dir, &dir.join(name), "// x")
}

/// `kai-<pid>-<hex>.js`: the pid of the process that wrote it.
fn leftover_pid(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("kai-")?.strip_suffix(".js")?;
    let (pid, hex) = rest.split_once('-')?;
    let hex_ok = !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit());
    let pid_ok = !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit());
    (hex_ok && pid_ok).then(|| pid.parse().ok()).flatten()
}

fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return true;
    };
    // SAFETY: signal 0 sends nothing; it only asks whether the process exists.
    unsafe {
        libc::kill(pid, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
}

/// Unloads from KWin, and removes, the scripts of Kai processes that are gone. Only regular files
/// of ours named like Kai's own, with a pid that is not alive, are touched.
async fn sweep_leftovers(bus: &zbus::Connection, dir: &std::path::Path, timeout: Duration) {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(pid) = leftover_pid(&name) else {
            continue;
        };
        // SAFETY: geteuid has no arguments and cannot fail.
        let ours = unsafe { libc::geteuid() };
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !meta.file_type().is_file() || meta.uid() != ours || pid_alive(pid) {
            continue;
        }
        let plugin = name.trim_end_matches(".js").to_owned();
        tracing::info!("desktop: unloading the KWin script {plugin} a dead Kai left behind");
        Cleanup::unload(bus.clone(), plugin, timeout).await;
        let _ = std::fs::remove_file(entry.path());
    }
}
