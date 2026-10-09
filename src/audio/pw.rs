//! The PipeWire loop thread (§2): the connection, the registry (devices and defaults), the two
//! streams, and app AEC. Everything PipeWire lives here; the rest of Kai talks to it through
//! [`Cmd`]s and hears back through `AudioEvent`s.
//!
//! The streams run with `RT_PROCESS`: their process callbacks run in PipeWire's data thread and
//! only move samples through the heads (lock-free rings, no allocation, no locks, no I/O). All
//! other callbacks (registry, metadata, core, stream state) only queue an [`Ev`]; the loop below
//! handles the queue between iterations, so no handler runs inside another.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::CString;
use std::path::PathBuf;
use std::process::Child;
use std::rc::Rc;
use std::time::Instant;

use pipewire as pw;
use pw::spa;
use pw::stream::{Stream, StreamFlags, StreamListener, StreamRc, StreamState};
use pw::types::ObjectType;
use tokio::sync::mpsc::UnboundedSender;

use super::aec::{self, AecAction, AecDriver, Picks};
use super::capture::CaptureHead;
use super::devices::{DeviceList, Graph, json_of_name};
use super::echo::{self, EchoMode, EchoStatus};
use super::playback::{Clock, PlayHead};
use super::{AudioEvent, AudioSettings, RATE};

pub enum Cmd {
    SetEcho(EchoMode),
    SetInput(Option<String>),
    SetOutput(Option<String>),
    /// Opens the playback stream with id `.1`, replacing any open one.
    OpenPlayback(Box<PlayHead>, u64),
    /// Closes the playback stream if it is still the one with this id (a later open replaced it
    /// otherwise).
    ClosePlayback(u64),
    OpenCapture(Box<CaptureHead>, u64),
    CloseCapture(u64),
    /// Restore, close, and answer when done.
    Quit(std::sync::mpsc::Sender<()>),
}

enum Ev {
    Cmd(Cmd),
    Node {
        id: u32,
        class: Option<String>,
        name: Option<String>,
        description: Option<String>,
        nick: Option<String>,
    },
    DefaultMetadata(Box<pw::registry::GlobalObject<pw::properties::PropertiesBox>>),
    Remove(u32),
    Meta {
        subject: u32,
        key: Option<String>,
        value: Option<String>,
    },
    Done(i32),
    CoreError {
        id: u32,
        res: i32,
        message: String,
    },
    Stream {
        which: Which,
        state: String,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    Playback,
    Capture,
}

impl Which {
    fn name(self) -> &'static str {
        match self {
            Which::Playback => "playback",
            Which::Capture => "capture",
        }
    }
}

type Queue = Rc<RefCell<VecDeque<Ev>>>;

fn push(q: &Queue, ev: Ev) {
    q.borrow_mut().push_back(ev);
}

/// The connection's objects. Fields drop in declaration order: every listener before the object
/// it listens on (an object's destruction cleans its hook list), the core last.
struct Conn {
    metadata: Option<(pw::metadata::MetadataListener, pw::metadata::Metadata)>,
    _registry_listener: pw::registry::Listener,
    registry: pw::registry::RegistryRc,
    _core_listener: pw::core::Listener,
    core: pw::core::CoreRc,
}

/// An open stream. Fields drop in declaration order: the listener (and the head inside its process
/// callback) before the stream.
struct Slot {
    _listener: StreamListener<()>,
    stream: StreamRc,
    /// The id the open came with.
    id: u64,
    /// The `target.object` it was connected with.
    target: Option<String>,
    /// Marked when the slot goes, so the head's owner knows.
    on_close: Box<dyn Fn()>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let _ = self.stream.disconnect();
        (self.on_close)();
    }
}

struct Thread {
    s: AudioSettings,
    events: UnboundedSender<AudioEvent>,
    q: Queue,
    conn: Option<Conn>,
    graph: Graph,
    /// The initial roundtrip after connecting.
    init_seq: Option<i32>,
    /// The roundtrip after binding the default metadata (its properties are in once it returns).
    meta_seq: Option<i32>,
    defaults_ready: bool,
    aec: AecDriver,
    aec_sync: Option<i32>,
    child: Option<Child>,
    playback: Option<Slot>,
    capture: Option<Slot>,
    quitting: Option<std::sync::mpsc::Sender<()>>,
    retry_at: Option<Instant>,
    last_devices: Option<DeviceList>,
    last_echo: Option<EchoStatus>,
    last_link: Option<Option<String>>,
}

pub fn run(s: AudioSettings, events: UnboundedSender<AudioEvent>, rx: pw::channel::Receiver<Cmd>) {
    pw::init();
    let ml = match pw::main_loop::MainLoopRc::new(None) {
        Ok(ml) => ml,
        Err(e) => {
            let _ = events.send(AudioEvent::Link(Some(format!(
                "cannot start the PipeWire loop: {e}"
            ))));
            return;
        }
    };
    let ctx = match pw::context::ContextRc::new(&ml, None) {
        Ok(ctx) => ctx,
        Err(e) => {
            let _ = events.send(AudioEvent::Link(Some(format!(
                "cannot create a PipeWire context: {e}"
            ))));
            return;
        }
    };
    let q: Queue = Rc::default();
    let _rx = rx.attach(ml.loop_(), {
        let q = q.clone();
        move |cmd| push(&q, Ev::Cmd(cmd))
    });
    let mut t = Thread {
        aec: AecDriver::new(s.aec_ready_timeout, s.aec_restore_timeout),
        s,
        events,
        q,
        conn: None,
        graph: Graph::default(),
        init_seq: None,
        meta_seq: None,
        defaults_ready: false,
        aec_sync: None,
        child: None,
        playback: None,
        capture: None,
        quitting: None,
        retry_at: None,
        last_devices: None,
        last_echo: None,
        last_link: None,
    };
    loop {
        if t.conn.is_none()
            && t.quitting.is_none()
            && t.retry_at.is_none_or(|at| Instant::now() >= at)
        {
            t.connect(&ctx);
        }
        loop {
            let ev = t.q.borrow_mut().pop_front();
            match ev {
                Some(ev) => t.handle(ev),
                None => break,
            }
        }
        t.tick(Instant::now());
        if t.finished() {
            break;
        }
        let timeout = match t.next_wakeup() {
            Some(at) => pw::loop_::Timeout::Finite(at.saturating_duration_since(Instant::now())),
            None => pw::loop_::Timeout::Infinite,
        };
        ml.loop_().iterate(timeout);
    }
    t.playback = None;
    t.capture = None;
    t.kill_child();
    t.conn = None;
    tracing::debug!("the PipeWire thread ends");
    if let Some(ack) = t.quitting.take() {
        let _ = ack.send(());
    }
}

impl Thread {
    fn connect(&mut self, ctx: &pw::context::ContextRc) {
        let core = match ctx.connect_rc(None) {
            Ok(core) => core,
            Err(e) => {
                self.retry_at = Some(Instant::now() + self.s.retry);
                self.link(Some(format!(
                    "PipeWire is not reachable ({e}); retrying every {} s (audio.retry_s)",
                    self.s.retry.as_secs()
                )));
                return;
            }
        };
        let registry = match core.get_registry_rc() {
            Ok(r) => r,
            Err(e) => {
                self.retry_at = Some(Instant::now() + self.s.retry);
                self.link(Some(format!("PipeWire's registry failed: {e}")));
                return;
            }
        };
        let q = self.q.clone();
        let q2 = self.q.clone();
        let core_listener = core
            .add_listener_local()
            .done(move |id, seq| {
                if id == pw::core::PW_ID_CORE {
                    push(&q, Ev::Done(seq.seq()));
                }
            })
            .error(move |id, _seq, res, message| {
                push(
                    &q2,
                    Ev::CoreError {
                        id,
                        res,
                        message: message.to_owned(),
                    },
                );
            })
            .register();
        let q = self.q.clone();
        let q2 = self.q.clone();
        let registry_listener = registry
            .add_listener_local()
            .global(move |g| {
                let get = |k: &str| g.props.and_then(|p| p.get(k)).map(str::to_owned);
                match g.type_ {
                    ObjectType::Node => push(
                        &q,
                        Ev::Node {
                            id: g.id,
                            class: get("media.class"),
                            name: get("node.name"),
                            description: get("node.description"),
                            nick: get("node.nick"),
                        },
                    ),
                    ObjectType::Metadata if get("metadata.name").as_deref() == Some("default") => {
                        push(&q, Ev::DefaultMetadata(Box::new(g.to_owned())))
                    }
                    _ => {}
                }
            })
            .global_remove(move |id| push(&q2, Ev::Remove(id)))
            .register();
        self.init_seq = core.sync(0).ok().map(|s| s.seq());
        self.conn = Some(Conn {
            core,
            registry,
            _core_listener: core_listener,
            _registry_listener: registry_listener,
            metadata: None,
        });
        self.retry_at = None;
        self.link(None);
        tracing::info!("connected to PipeWire");
    }

    fn disconnected(&mut self, why: String) {
        tracing::warn!("PipeWire connection lost: {why}");
        self.playback = None;
        self.capture = None;
        self.conn = None;
        self.graph = Graph::default();
        self.defaults_ready = false;
        self.init_seq = None;
        self.meta_seq = None;
        self.aec_sync = None;
        // The canceller went with the daemon; the defaults are restored on the next connect.
        self.kill_child();
        self.aec = AecDriver::new(self.s.aec_ready_timeout, self.s.aec_restore_timeout);
        self.retry_at = Some(Instant::now() + self.s.retry);
        self.link(Some(format!(
            "PipeWire went away ({why}); retrying every {} s (audio.retry_s)",
            self.s.retry.as_secs()
        )));
        self.publish();
    }

    fn handle(&mut self, ev: Ev) {
        match ev {
            Ev::Cmd(cmd) => self.command(cmd),
            Ev::Node {
                id,
                class,
                name,
                description,
                nick,
            } => {
                self.graph.add_node(
                    id,
                    class.as_deref(),
                    name.as_deref(),
                    description.as_deref(),
                    nick.as_deref(),
                );
                self.graph_changed();
            }
            Ev::Remove(id) => {
                if self.graph.remove(id) {
                    self.graph_changed();
                }
            }
            Ev::DefaultMetadata(global) => self.bind_metadata(&global),
            Ev::Meta {
                subject,
                key,
                value,
            } => {
                if subject != 0 {
                    return;
                }
                match key {
                    None => self.graph.clear_defaults(),
                    Some(k) => {
                        if !self.graph.set_default(&k, value.as_deref()) {
                            return;
                        }
                    }
                }
                if self.defaults_ready {
                    self.graph_changed();
                }
            }
            Ev::Done(seq) => self.done(seq),
            Ev::CoreError { id, res, message } => {
                if id == pw::core::PW_ID_CORE && res == -libc::EPIPE {
                    self.disconnected(message);
                } else {
                    tracing::warn!(id, res, "PipeWire error: {message}");
                }
            }
            Ev::Stream {
                which,
                state,
                error,
            } => match error {
                Some(e) => {
                    let line = format!("the {} stream failed: {e}", which.name());
                    tracing::warn!("{line}");
                    let _ = self.events.send(AudioEvent::Notice(line));
                }
                None => tracing::debug!(stream = which.name(), %state, "stream state"),
            },
        }
    }

    fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::SetEcho(mode) => {
                if mode == self.s.echo_mode {
                    return;
                }
                let was = self.s.echo_mode;
                self.s.echo_mode = mode;
                tracing::info!(from = was.key(), to = mode.key(), "echo mode");
                if was == EchoMode::App {
                    let acts = self.aec.stop(&self.graph.aec_view(), Instant::now(), false);
                    self.run_aec(acts);
                }
                self.maybe_start_aec();
                self.publish();
                self.retarget();
            }
            Cmd::SetInput(pick) => {
                self.s.input = pick;
                self.repin_aec("microphone");
                self.publish();
                self.retarget();
            }
            Cmd::SetOutput(pick) => {
                self.s.output = pick;
                self.repin_aec("speaker");
                self.publish();
                self.retarget();
            }
            Cmd::OpenPlayback(head, id) => {
                self.playback = None;
                let shared = head.shared();
                match self.open_playback(*head, id) {
                    Ok(slot) => self.playback = Some(slot),
                    Err(e) => {
                        shared.close();
                        self.notice(format!("cannot open the playback stream: {e}"));
                    }
                }
            }
            Cmd::ClosePlayback(id) => {
                if self.playback.as_ref().is_some_and(|s| s.id == id) {
                    self.playback = None;
                }
            }
            Cmd::OpenCapture(head, id) => {
                self.capture = None;
                let shared = head.shared();
                match self.open_capture(*head, id) {
                    Ok(slot) => self.capture = Some(slot),
                    Err(e) => {
                        shared.close();
                        self.notice(format!("cannot open the capture stream: {e}"));
                    }
                }
            }
            Cmd::CloseCapture(id) => {
                if self.capture.as_ref().is_some_and(|s| s.id == id) {
                    self.capture = None;
                }
            }
            Cmd::Quit(ack) => {
                tracing::debug!("the audio thread is asked to quit");
                self.quitting = Some(ack);
                self.playback = None;
                self.capture = None;
                if self.conn.is_some() {
                    let acts = self.aec.stop(&self.graph.aec_view(), Instant::now(), true);
                    self.run_aec(acts);
                }
            }
        }
    }

    fn bind_metadata(
        &mut self,
        global: &pw::registry::GlobalObject<pw::properties::PropertiesBox>,
    ) {
        let Some(conn) = &mut self.conn else { return };
        if conn.metadata.is_some() {
            return;
        }
        let metadata: pw::metadata::Metadata = match conn.registry.bind(global) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("cannot bind PipeWire's default metadata: {e}");
                return;
            }
        };
        let q = self.q.clone();
        let listener = metadata
            .add_listener_local()
            .property(move |subject, key, _type, value| {
                push(
                    &q,
                    Ev::Meta {
                        subject,
                        key: key.map(str::to_owned),
                        value: value.map(str::to_owned),
                    },
                );
                0
            })
            .register();
        conn.metadata = Some((listener, metadata));
        self.meta_seq = conn.core.sync(0).ok().map(|s| s.seq());
    }

    fn done(&mut self, seq: i32) {
        if Some(seq) == self.init_seq {
            self.init_seq = None;
            tracing::debug!(nodes = ?self.graph.devices(), "PipeWire's initial graph");
            if self.meta_seq.is_none() && !self.defaults_ready {
                tracing::warn!("PipeWire has no default metadata (is WirePlumber running?)");
                self.publish();
                self.maybe_start_aec();
            }
        }
        if Some(seq) == self.meta_seq {
            self.meta_seq = None;
            self.defaults_ready = true;
            let file = aec::load(&self.s.aec_file);
            let acts = self
                .aec
                .recover(file, &self.graph.aec_view(), Instant::now());
            self.run_aec(acts);
            self.maybe_start_aec();
            self.publish();
        }
        if Some(seq) == self.aec_sync {
            self.aec_sync = None;
            let acts = self.aec.confirmed();
            self.run_aec(acts);
            self.maybe_start_aec();
            self.publish();
            self.retarget();
        }
    }

    fn graph_changed(&mut self) {
        let acts = self.aec.on_graph(&self.graph.aec_view(), Instant::now());
        self.run_aec(acts);
        self.publish();
        self.retarget();
    }

    /// The devices app AEC is pinned to: Kai's picks the graph has (a pick that is not there would
    /// make the module wait for it silently), else the defaults.
    fn picks(&self) -> Picks {
        let present = |p: &Option<String>| p.clone().filter(|n| self.graph.has_node(n));
        Picks {
            input: present(&self.s.input),
            output: present(&self.s.output),
        }
    }

    fn maybe_start_aec(&mut self) {
        if self.s.echo_mode != EchoMode::App || self.quitting.is_some() || self.conn.is_none() {
            return;
        }
        if !self.defaults_ready && self.init_seq.is_some() {
            return; // still connecting
        }
        let acts = self.aec.start(
            &self.graph.aec_view(),
            &self.picks(),
            aec::plugin_check(),
            Instant::now(),
        );
        self.run_aec(acts);
    }

    /// K-9: a microphone or speaker picked while app AEC runs is applied by restarting the
    /// canceller (restore, confirm, start pinned to the new devices); the tooltip says it starts
    /// meanwhile.
    fn repin_aec(&mut self, which: &str) {
        if self.s.echo_mode != EchoMode::App || !self.aec.repin_needed(&self.picks()) {
            return;
        }
        tracing::info!("app AEC: restarting the canceller for the new {which}");
        let acts = self.aec.stop(&self.graph.aec_view(), Instant::now(), false);
        self.run_aec(acts);
        self.maybe_start_aec();
    }

    fn run_aec(&mut self, actions: Vec<AecAction>) {
        for a in actions {
            if let Err(e) = self.exec(&a) {
                tracing::warn!(?a, "app AEC: {e}");
                let more = self
                    .aec
                    .failed(&a, e, &self.graph.aec_view(), Instant::now());
                self.run_aec(more);
                break;
            }
        }
        if let Some(note) = self.aec.note.take() {
            self.notice(format!("echo cancellation: {note}"));
        }
        self.publish();
        self.retarget();
    }

    fn exec(&mut self, a: &AecAction) -> Result<(), String> {
        match a {
            AecAction::Save(saved) => {
                aec::save(&self.s.aec_file, saved)?;
                tracing::info!(file = %self.s.aec_file.display(), ?saved, "app AEC: saved the default devices");
                Ok(())
            }
            AecAction::Remove => aec::remove(&self.s.aec_file),
            AecAction::SetAside => {
                let bad = self.s.aec_file.with_extension("json.bad");
                std::fs::rename(&self.s.aec_file, &bad).map_err(|e| {
                    format!(
                        "cannot set {} aside as {}: {e}",
                        self.s.aec_file.display(),
                        bad.display()
                    )
                })?;
                tracing::warn!(file = %bad.display(), "app AEC: an unreadable aec.json was set aside");
                Ok(())
            }
            AecAction::Spawn { conf } => {
                std::fs::create_dir_all(&self.s.runtime_dir)
                    .map_err(|e| format!("cannot create {}: {e}", self.s.runtime_dir.display()))?;
                let path = self.s.runtime_dir.join("aec.conf");
                std::fs::write(&path, conf)
                    .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                let child = aec::spawn_child(&path)
                    .map_err(|e| format!("cannot start `pipewire -c {}`: {e}", path.display()))?;
                tracing::info!(pid = child.id(), "app AEC: started the echo-cancel child");
                self.child = Some(child);
                Ok(())
            }
            AecAction::Kill => {
                self.kill_child();
                Ok(())
            }
            AecAction::SetDefault { key, value } => {
                let (_, metadata) = self
                    .conn
                    .as_ref()
                    .and_then(|c| c.metadata.as_ref())
                    .ok_or("PipeWire has no default metadata (is WirePlumber running?)")?;
                let json = value.as_deref().map(json_of_name);
                metadata.set_property(
                    0,
                    key.configured(),
                    json.as_ref().map(|_| "Spa:String:JSON"),
                    json.as_deref(),
                );
                tracing::info!(key = key.configured(), value = ?value, "app AEC: set a default");
                Ok(())
            }
            AecAction::Sync => {
                let core = &self.conn.as_ref().ok_or("not connected to PipeWire")?.core;
                let seq = core
                    .sync(0)
                    .map_err(|e| format!("PipeWire roundtrip failed: {e}"))?;
                self.aec_sync = Some(seq.seq());
                Ok(())
            }
        }
    }

    fn kill_child(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
            tracing::info!(pid = c.id(), "app AEC: stopped the echo-cancel child");
        }
    }

    fn tick(&mut self, now: Instant) {
        if let Some(c) = &mut self.child
            && let Ok(Some(status)) = c.try_wait()
        {
            self.child = None;
            let acts = self
                .aec
                .child_exited(&status.to_string(), &self.graph.aec_view(), now);
            self.run_aec(acts);
        }
        let acts = self.aec.tick(now, &self.graph.aec_view());
        if !acts.is_empty() {
            self.run_aec(acts);
        }
        if self.quitting.is_some() && self.conn.is_none() && self.aec.busy() {
            // Nothing can be restored without PipeWire; aec.json stays for the next start.
            self.aec = AecDriver::new(self.s.aec_ready_timeout, self.s.aec_restore_timeout);
        }
    }

    fn finished(&self) -> bool {
        self.quitting.is_some() && !self.aec.busy()
    }

    /// No polling: the child's exit removes its nodes, and that registry event wakes the loop.
    fn next_wakeup(&self) -> Option<Instant> {
        [self.aec.deadline(), self.retry_at]
            .into_iter()
            .flatten()
            .min()
    }

    /// The `target.object` each stream should have now.
    fn targets(&self) -> (Option<String>, Option<String>) {
        if self.aec.routes() {
            (Some(aec::SOURCE.into()), Some(aec::SINK.into()))
        } else {
            (self.s.input.clone(), self.s.output.clone())
        }
    }

    fn retarget(&mut self) {
        let (input, output) = self.targets();
        for (slot, target, dir) in [
            (&mut self.capture, input, spa::utils::Direction::Input),
            (&mut self.playback, output, spa::utils::Direction::Output),
        ] {
            let Some(slot) = slot else { continue };
            if slot.target == target {
                continue;
            }
            tracing::info!(stream = %slot.stream.name(), from = ?slot.target, to = ?target, "moving a stream");
            set_target(&slot.stream, target.as_deref());
            let _ = slot.stream.disconnect();
            if let Err(e) = connect(&slot.stream, dir) {
                let line = format!("cannot reconnect {}: {e}", slot.stream.name());
                tracing::warn!("{line}");
                let _ = self.events.send(AudioEvent::Notice(line));
            }
            slot.target = target;
        }
    }

    fn open_playback(&mut self, mut head: PlayHead, id: u64) -> Result<Slot, String> {
        let shared = head.shared();
        let (_, target) = self.targets();
        let stream = self.new_stream(Which::Playback, target.as_deref())?;
        let q = self.q.clone();
        let listener = stream
            .add_local_listener_with_user_data(())
            .state_changed(move |_, _, _old, new| stream_state(&q, Which::Playback, new))
            .process(move |stream, _| playback_process(stream, &mut head))
            .register()
            .map_err(|e| e.to_string())?;
        let slot = Slot {
            stream,
            _listener: listener,
            id,
            target,
            on_close: Box::new(move || shared.close()),
        };
        connect(&slot.stream, spa::utils::Direction::Output)?;
        Ok(slot)
    }

    fn open_capture(&mut self, mut head: CaptureHead, id: u64) -> Result<Slot, String> {
        let shared = head.shared();
        let (target, _) = self.targets();
        let stream = self.new_stream(Which::Capture, target.as_deref())?;
        let q = self.q.clone();
        let listener = stream
            .add_local_listener_with_user_data(())
            .state_changed(move |_, _, _old, new| stream_state(&q, Which::Capture, new))
            .process(move |stream, _| capture_process(stream, &mut head))
            .register()
            .map_err(|e| e.to_string())?;
        let slot = Slot {
            stream,
            _listener: listener,
            id,
            target,
            on_close: Box::new(move || shared.close()),
        };
        connect(&slot.stream, spa::utils::Direction::Input)?;
        Ok(slot)
    }

    fn new_stream(&self, which: Which, target: Option<&str>) -> Result<StreamRc, String> {
        let conn = self.conn.as_ref().ok_or("not connected to PipeWire")?;
        let name = format!("kai.{}", which.name());
        let mut props = pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => match which { Which::Playback => "Playback", Which::Capture => "Capture" },
            *pw::keys::MEDIA_ROLE => "Communication",
            *pw::keys::NODE_NAME => name.as_str(),
            *pw::keys::NODE_DESCRIPTION => match which { Which::Playback => "Kai (voice)", Which::Capture => "Kai (microphone)" },
            *pw::keys::APP_NAME => "Kai",
            *pw::keys::APP_ID => crate::APP_ID,
            *pw::keys::APP_ICON_NAME => crate::APP_ID,
        };
        if let Some(t) = target {
            props.insert(*pw::keys::TARGET_OBJECT, t);
        }
        #[cfg(test)]
        if let Ok(v) = std::env::var("KAI_TEST_NODE_ASYNC") {
            props.insert("node.async", v);
        }
        StreamRc::new(conn.core.clone(), &name, props).map_err(|e| e.to_string())
    }

    fn link(&mut self, note: Option<String>) {
        if self.last_link.as_ref() != Some(&note) {
            if let Some(n) = &note {
                tracing::warn!("{n}");
            }
            let _ = self.events.send(AudioEvent::Link(note.clone()));
            self.last_link = Some(note);
        }
    }

    fn notice(&self, line: String) {
        tracing::info!("{line}");
        let _ = self.events.send(AudioEvent::Notice(line));
    }

    /// Sends the device lists and the echo status when they changed.
    fn publish(&mut self) {
        let devices = self.graph.devices();
        if self.last_devices.as_ref() != Some(&devices) {
            let _ = self.events.send(AudioEvent::Devices(devices.clone()));
            self.last_devices = Some(devices.clone());
        }
        let pick = self.s.output.as_deref().map(|name| {
            let label = DeviceList::label_of(&devices.outputs, name).unwrap_or(name);
            (name, label)
        });
        let status = echo::status(
            self.s.echo_mode,
            &self.aec.phase(),
            pick,
            devices.default_output.as_deref(),
        );
        if self.last_echo.as_ref() != Some(&status) {
            tracing::info!(chosen = status.chosen.key(), effective = status.effective.key(), note = ?status.note, "echo");
            let _ = self.events.send(AudioEvent::Echo(status.clone()));
            self.last_echo = Some(status);
        }
    }
}

fn stream_state(q: &Queue, which: Which, new: StreamState) {
    let (state, error) = match &new {
        StreamState::Error(e) => ("error".to_owned(), Some(e.clone())),
        other => (format!("{other:?}"), None),
    };
    push(
        q,
        Ev::Stream {
            which,
            state,
            error,
        },
    );
}

/// The format both streams ask for: S16LE, mono, 24 kHz (§6.1).
fn format_pod() -> Vec<u8> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::S16LE);
    info.set_rate(RATE);
    info.set_channels(1);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
    info.set_position(position);
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: info.into(),
        }),
    )
    .expect("the format serializes")
    .0
    .into_inner()
}

fn connect(stream: &Stream, dir: spa::utils::Direction) -> Result<(), String> {
    let bytes = format_pod();
    let pod = spa::pod::Pod::from_bytes(&bytes).ok_or("bad format pod")?;
    stream
        .connect(
            dir,
            None,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut [pod],
        )
        .map_err(|e| e.to_string())
}

/// Sets or (with `None`) removes the stream's `target.object`; takes effect at the next connect.
fn set_target(stream: &Stream, target: Option<&str>) {
    let key = c"target.object";
    let value = target.and_then(|t| CString::new(t).ok());
    let item = spa::sys::spa_dict_item {
        key: key.as_ptr(),
        value: value.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
    };
    let dict = spa::sys::spa_dict {
        flags: 0,
        n_items: 1,
        items: &item,
    };
    // SAFETY: the dict and its strings outlive the call, which copies them.
    unsafe {
        pw::sys::pw_stream_update_properties(stream.as_raw_ptr(), &dict);
    }
}

/// The playback stream's process callback (PipeWire's data thread).
fn playback_process(stream: &Stream, head: &mut PlayHead) {
    let clock = stream.time().ok().and_then(|t| {
        let r = t.rate();
        Clock::from_time(t.now(), r.num, r.denom, t.delay(), t.queued(), t.buffered())
    });
    // SAFETY: the raw buffer API as the PipeWire docs use it: dequeue, fill data[0] within its
    // maxsize, set the chunk and `size` (frames, for `pw_time.queued`), queue.
    unsafe {
        let b = stream.dequeue_raw_buffer();
        if b.is_null() {
            return;
        }
        let buf = (*b).buffer;
        if buf.is_null() || (*buf).n_datas == 0 || (*(*buf).datas).data.is_null() {
            stream.queue_raw_buffer(b);
            return;
        }
        let d = &mut *(*buf).datas;
        let max = d.maxsize as usize / 2;
        let requested = (*b).requested as usize;
        let n = if requested > 0 {
            requested.min(max)
        } else {
            max
        };
        let out = std::slice::from_raw_parts_mut(d.data as *mut i16, n);
        head.fill(out, clock);
        let chunk = &mut *d.chunk;
        chunk.offset = 0;
        chunk.stride = 2;
        chunk.size = (n * 2) as u32;
        (*b).size = n as u64;
        stream.queue_raw_buffer(b);
    }
}

/// The capture stream's process callback (PipeWire's data thread).
fn capture_process(stream: &Stream, head: &mut CaptureHead) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let datas = buffer.datas_mut();
    let Some(d) = datas.first_mut() else { return };
    let (offset, size) = (d.chunk().offset() as usize, d.chunk().size() as usize);
    let Some(bytes) = d.data() else { return };
    let end = (offset + size).min(bytes.len());
    if offset >= end {
        return;
    }
    let bytes = &bytes[offset..end];
    if !(bytes.as_ptr() as usize).is_multiple_of(2) {
        return;
    }
    // SAFETY: aligned for i16 (checked), length rounded down to whole samples.
    let samples =
        unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const i16, bytes.len() / 2) };
    head.take(samples);
}

pub fn runtime_dir_for(state_dir: &std::path::Path) -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| p.join("kai"))
        .unwrap_or_else(|| state_dir.to_owned())
}
