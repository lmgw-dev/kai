//! PD3: the tools that act on the core (conversation mode, a job's announce, the configuration
//! operations, the personality) reach it as one [`Command::Tool`] with a oneshot answer; every
//! other tool runs in its own module, off the core.
//!
//! **Adding a core area:** one line in [`core_tools!`] below, `<contract area> => <method>,`, where
//! the method is the core's `async fn(&mut self, call: CoreCall)` in the area's own file and
//! answers on `call.reply`. The area's tools are then offered through the core's provider, in the
//! order of the lines.
//!
//! A call whose answer nobody waits for any more (lmgw cancelled it, the link dropped) is not run
//! (api §5.4): the core looks before it hands it on.

use std::sync::Arc;

use kai_desktop::contract::Area;
use tokio::sync::{mpsc, oneshot};

use super::{Command, Core};
use crate::audio::AudioPort;
use crate::host::registry::{Call, Entry, Provider, Reply};
use crate::lmgw_link::LmgwLink;
use crate::secret::SecretStore;
use crate::voice::VoiceLink;

/// One call for the core.
pub struct CoreCall {
    /// The contract area of the tool (the routing key).
    pub area: &'static str,
    pub call: Call,
    pub reply: oneshot::Sender<Reply>,
}

impl std::fmt::Debug for CoreCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the arguments.
        f.debug_struct("CoreCall")
            .field("area", &self.area)
            .field("tool", &self.call.spec.name)
            .finish()
    }
}

/// The core's areas: `area => method,`, one line each (module doc).
macro_rules! core_tools {
    ($($(#[$attr:meta])* $area:path => $method:ident,)*) => {
        /// The contract areas the core serves, in order.
        #[allow(clippy::vec_init_then_push)]
        pub fn areas() -> Vec<&'static Area> {
            #[allow(unused_mut)]
            let mut all: Vec<&'static Area> = Vec::new();
            $($(#[$attr])* all.push(&$area);)*
            all
        }

        impl<S: SecretStore, L: LmgwLink, A: AudioPort, V: VoiceLink> Core<S, L, A, V> {
            /// A [`Command::Tool`]: routed to its area's method, unless it was cancelled.
            pub(super) async fn on_tool(&mut self, c: CoreCall) {
                if c.reply.is_closed() {
                    tracing::info!(tool = c.call.spec.name, "core: the call was cancelled before the core ran it");
                    return;
                }
                $($(#[$attr])* if c.area == $area.name {
                    return self.$method(c).await;
                })*
                tracing::error!(area = c.area, tool = c.call.spec.name, "core: no method serves this area");
                let _ = c.reply.send(Reply::error(format!(
                    "Kai's core serves no tools of the area {}",
                    c.area
                )));
            }
        }
    };
}

core_tools! {
    kai_desktop::contract::conversation::AREA => conversation_tool,
    kai_desktop::contract::config::AREA => config_tool,
    kai_desktop::contract::personality::AREA => personality_tool,
    kai_desktop::contract::jobs::AREA => jobs_tool,
    #[cfg(test)]
    tests::PING => test_ping,
}

/// The core's provider: the areas' tools, each call sent to the core as [`Command::Tool`]. `jobs`
/// is the host's job table, which `job_cancel`'s and `job_announce`'s dialog reads.
pub fn provider(cmds: mpsc::UnboundedSender<Command>, jobs: crate::jobs::Jobs) -> CoreTools {
    CoreTools { cmds, jobs }
}

pub struct CoreTools {
    cmds: mpsc::UnboundedSender<Command>,
    jobs: crate::jobs::Jobs,
}

impl Provider for CoreTools {
    fn entries(&self) -> Vec<Entry> {
        areas()
            .into_iter()
            .flat_map(|area| {
                area.tools.iter().map(move |spec| {
                    let cmds = self.cmds.clone();
                    Entry {
                        area: area.name,
                        spec,
                        handler: Arc::new(move |call| {
                            let (reply, answer) = oneshot::channel();
                            let sent = cmds.send(Command::Tool(Box::new(CoreCall {
                                area: area.name,
                                call,
                                reply,
                            })));
                            Box::pin(async move {
                                if sent.is_err() {
                                    return Reply::error("Kai is quitting; the call did not run");
                                }
                                answer.await.unwrap_or_else(|_| {
                                    Reply::error("Kai's core gave no answer (it is quitting)")
                                })
                            })
                        }),
                        prepare: super::jobs::preparer(spec.name, &self.jobs),
                    }
                })
            })
            .collect()
    }
}

#[cfg(test)]
pub(super) mod tests {
    use kai_desktop::contract::ToolSpec;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::host::policy::Caller;
    use crate::host::registry::Args;

    pub const PING: Area = Area {
        name: "ping",
        tools: &[ToolSpec {
            name: "ping",
            description: "Answer pong.",
            ..ToolSpec::READ
        }],
    };

    impl<S: SecretStore, L: LmgwLink, A: AudioPort, V: VoiceLink> Core<S, L, A, V> {
        pub(super) async fn test_ping(&mut self, c: CoreCall) {
            let paired = self.paired();
            let _ = c
                .reply
                .send(Reply::text(format!("pong (paired: {paired})")));
        }
    }

    pub fn call(spec: &'static ToolSpec) -> Call {
        Call::new(spec, Args::default(), Caller::Own, CancellationToken::new())
    }

    /// The provider lists the core's areas and hands each call to the core as one command.
    #[tokio::test]
    async fn a_core_tool_is_one_command_with_its_answer() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let entries = provider(tx, crate::jobs::Jobs::default()).entries();
        let names: Vec<_> = entries.iter().map(|e| (e.area, e.spec.name)).collect();
        let mut expected: Vec<(&str, &str)> = vec![
            ("conversation", "conversation_start"),
            ("conversation", "conversation_end"),
        ];
        expected.extend(
            kai_desktop::contract::config::AREA
                .tools
                .iter()
                .map(|t| ("config", t.name)),
        );
        expected.push(("personality", "personality_set"));
        expected.extend([
            ("jobs", "jobs"),
            ("jobs", "job_cancel"),
            ("jobs", "job_announce"),
        ]);
        expected.push(("ping", "ping"));
        assert_eq!(names, expected);
        let ping = entries.last().unwrap();
        let pending = (ping.handler)(call(&PING.tools[0]));
        let Some(Command::Tool(c)) = rx.recv().await else {
            panic!("no Command::Tool");
        };
        assert_eq!(
            format!("{c:?}"),
            r#"CoreCall { area: "ping", tool: "ping" }"#
        );
        c.reply.send(Reply::text("pong")).unwrap();
        assert_eq!(pending.await, Reply::text("pong"));
        // The core gone: an error the model reads, not a hang.
        drop(rx);
        let r = (ping.handler)(call(&PING.tools[0])).await;
        assert!(r.error);
    }
}
