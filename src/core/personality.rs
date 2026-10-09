//! WP22 on the core: `personality_set` (PD3) and the tray's Personality radio set the profile of
//! Kai's conversation folder through the voice's link; the list of profiles and each thread's
//! profile (`crate::personality`) follow lmgw's feed.

use std::collections::VecDeque;

use tokio::sync::{mpsc, oneshot};

use super::Core;
use super::calls::CoreCall;
use crate::audio::AudioPort;
use crate::conversation::FolderInfo;
use crate::host::registry::Reply;
use crate::lmgw_link::LmgwLink;
use crate::personality::{self, Target};
use crate::secret::SecretStore;
use crate::voice::VoiceLink;
use crate::voice::link::{FolderPatched, Pending, ProfileList, Refusal};

/// Who asked for a set, and so where its outcome goes.
pub(super) enum Origin {
    Tool(oneshot::Sender<Reply>),
    Tray,
}

/// The step a set's spawned task finished, posted back to the core.
pub(super) enum Step {
    Listed(Result<ProfileList, Refusal>),
    Patched(Box<Result<FolderPatched, Refusal>>),
}

/// The sets of the personality: one runs at a time (its requests are spawned, so the core keeps
/// serving its other work), later ones wait their turn in order, so the last pick is the one
/// that stands.
pub(super) struct Sets {
    next_id: u64,
    running: Option<Running>,
    waiting: VecDeque<(Target, Origin)>,
    tx: mpsc::UnboundedSender<(u64, Step)>,
    rx: mpsc::UnboundedReceiver<(u64, Step)>,
}

struct Running {
    id: u64,
    origin: Origin,
    target: Target,
    folder: i64,
    /// The list and the choice made from it, once the first request answered.
    listed: Option<(ProfileList, Option<i64>)>,
}

impl Default for Sets {
    fn default() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            next_id: 0,
            running: None,
            waiting: VecDeque::new(),
            tx,
            rx,
        }
    }
}

impl Sets {
    /// The next finished step of the running set (pending while none runs).
    pub(super) async fn next(&mut self) -> Option<(u64, Step)> {
        self.rx.recv().await
    }

    fn spawn<T: Send + 'static>(&self, id: u64, req: Pending<T>, step: fn(T) -> Step) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send((id, step(req.await)));
        });
    }
}

impl<S: SecretStore, L: LmgwLink, A: AudioPort, V: VoiceLink> Core<S, L, A, V> {
    pub(super) async fn personality_tool(&mut self, c: CoreCall) {
        match c.call.args.text("name") {
            None => {
                let _ = c.reply.send(Reply::error("personality_set needs name"));
            }
            Some(name) => {
                let target = Target::Name(name.to_owned());
                self.set_personality(target, Origin::Tool(c.reply)).await;
            }
        }
    }

    /// The tray's pick: the same operation; a refusal is said in a notification (the tray has no
    /// answer to carry it).
    pub(super) async fn pick_personality(&mut self, id: Option<i64>) {
        self.set_personality(Target::Id(id), Origin::Tray).await;
    }

    /// Reads lmgw's profiles, resolves `target` against them and writes the choice to Kai's
    /// conversation folder (module doc of `crate::personality`). Both requests are bounded by
    /// `lmgw.timeout_s` and run in spawned tasks; [`Core::on_personality_step`] carries on with
    /// each answer. A set that comes while one runs waits for it.
    async fn set_personality(&mut self, target: Target, origin: Origin) {
        self.sets.waiting.push_back((target, origin));
        self.start_personality().await;
    }

    /// Starts the oldest waiting set if none runs; one that cannot start is answered at once.
    async fn start_personality(&mut self) {
        while self.sets.running.is_none() {
            let Some((target, origin)) = self.sets.waiting.pop_front() else {
                return;
            };
            if !self.paired() {
                self.personality_done(origin, Err("Kai is not paired with lmgw".to_owned()));
                continue;
            }
            let Some(folder) = self.conversation.folder().map(|f| f.id) else {
                let why = "Kai's conversation folder is not known yet (lmgw is not connected, or \
                           the folder is not set up)";
                self.personality_done(origin, Err(why.to_owned()));
                continue;
            };
            self.sets.next_id += 1;
            let id = self.sets.next_id;
            let req = self.voice.link_mut().profiles();
            self.sets.spawn(id, req, Step::Listed);
            self.sets.running = Some(Running {
                id,
                origin,
                target,
                folder,
                listed: None,
            });
        }
    }

    /// A spawned request of the running set answered. An answer of a set that was dropped (the
    /// pairing moved to another gateway) is ignored.
    pub(super) async fn on_personality_step(&mut self, id: u64, step: Step) {
        if self.sets.running.as_ref().is_none_or(|r| r.id != id) {
            return;
        }
        match step {
            Step::Listed(Err(e)) => {
                self.finish_personality(Err(format!("cannot read lmgw's profiles: {e}")))
            }
            Step::Listed(Ok(list)) => {
                self.personality.loaded(&list);
                let run = self.sets.running.as_mut().expect("checked above");
                match personality::resolve(&list, &run.target) {
                    Err(why) => self.finish_personality(Err(why)),
                    Ok(chosen) => {
                        let req = self.voice.link_mut().set_folder_profile(run.folder, chosen);
                        run.listed = Some((list, chosen));
                        self.sets.spawn(id, req, |r| Step::Patched(Box::new(r)));
                    }
                }
            }
            Step::Patched(patched) => match *patched {
                Err(e) => {
                    self.finish_personality(Err(format!("lmgw did not take the profile: {e}")))
                }
                Ok(patched) => self.personality_patched(patched).await,
            },
        }
        self.start_personality().await;
    }

    async fn personality_patched(&mut self, patched: FolderPatched) {
        let Some((list, chosen)) = self.sets.running.as_mut().and_then(|r| r.listed.take()) else {
            return;
        };
        let text = personality::answer(&list, chosen, &patched);
        // What the feed will say in a moment: the folder as it is, and the thread that took it.
        let acts = self.conversation.changed(FolderInfo::from(&patched.folder));
        self.apply(acts).await;
        if let Some(a) = &patched.applied
            && a.fields.iter().any(|f| f == "profile_id")
        {
            let now = chosen.or(self.personality.global());
            self.personality.thread_is(a.thread_id, now);
        }
        self.finish_personality(Ok(text));
    }

    /// The running set is over: its outcome goes to whoever asked.
    fn finish_personality(&mut self, outcome: Result<String, String>) {
        if let Some(run) = self.sets.running.take() {
            self.personality_done(run.origin, outcome);
        }
    }

    fn personality_done(&mut self, origin: Origin, outcome: Result<String, String>) {
        match origin {
            Origin::Tool(reply) => {
                let _ = reply.send(match outcome {
                    Ok(text) => Reply::text(text),
                    Err(why) => Reply::error(why),
                });
            }
            Origin::Tray => {
                match outcome {
                    Ok(text) => tracing::info!("personality: {text}"),
                    Err(why) => {
                        tracing::warn!("personality: {why}");
                        self.notify("Kai: the personality did not change", why);
                    }
                }
                self.sync_personality();
                self.publish();
            }
        }
    }

    /// The pairing moved to another gateway: its folder is not the sets' any more.
    pub(super) fn drop_personality_sets(&mut self) {
        let why = "the pairing changed before lmgw answered".to_owned();
        let running = self.sets.running.take().map(|r| r.origin);
        let waiting: Vec<Origin> = self.sets.waiting.drain(..).map(|(_, o)| o).collect();
        for origin in running.into_iter().chain(waiting) {
            self.personality_done(origin, Err(why.clone()));
        }
    }

    /// Asks for the profile list (at `hello` and after a `resync`).
    pub(super) fn ask_profiles(&mut self) {
        self.profiles_req = Some(self.voice.link_mut().profiles());
    }

    pub(super) fn on_profiles_answer(&mut self, answer: Result<ProfileList, Refusal>) {
        match answer {
            Ok(list) => self.personality.loaded(&list),
            // An older lmgw has no profiles: no submenu, no complaint.
            Err(e) => tracing::info!("personality: no profile list: {e}"),
        }
    }

    /// The tray's Personality radio from what is known of the conversation's folder and thread.
    pub(super) fn sync_personality(&mut self) {
        let folder = self.conversation.folder().and_then(|f| f.profile_id);
        self.model.personality = self.personality.menu(folder, self.voice.thread_id());
    }
}
