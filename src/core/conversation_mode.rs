//! K27's tools on the core (PD3): `conversation_start` and `conversation_end` reach the voice as
//! one command each, and the answer is the tool's result.

use super::Core;
use super::calls::CoreCall;
use crate::audio::AudioPort;
use crate::host::registry::Reply;
use crate::lmgw_link::LmgwLink;
use crate::secret::SecretStore;
use crate::voice::VoiceLink;

impl<S: SecretStore, L: LmgwLink, A: AudioPort, V: VoiceLink> Core<S, L, A, V> {
    pub(super) async fn conversation_tool(&mut self, c: CoreCall) {
        let reply = match c.call.spec.name {
            "conversation_start" => match self.voice.conversation_start() {
                Ok(text) => Reply::text(text),
                Err(why) => Reply::error(why),
            },
            "conversation_end" => Reply::text(self.voice.conversation_end()),
            other => Reply::error(format!("Kai's conversation area has no tool {other}")),
        };
        let _ = c.reply.send(reply);
    }
}
