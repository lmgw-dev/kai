//! Firefox's sidebar's requests on the core (kai-firefox design §8.2 "Kai's proxy", §8.3): what
//! `browser.sock` admitted (`crate::browser::sidebar`) goes to the paired lmgw through the voice's
//! link, as the settings window's Personality page does (`super::window`), with Kai's device key.
//! The exchange runs in a spawned task, so the core's loop is never held by it; leaving the gateway
//! ends it with an error the sidebar shows.

use super::Core;
use crate::audio::AudioPort;
use crate::browser::sidebar::Relay;
use crate::lmgw_link::LmgwLink;
use crate::lmgw_link::proxy::Limits;
use crate::secret::SecretStore;
use crate::voice::VoiceLink;

impl<S: SecretStore, L: LmgwLink, A: AudioPort, V: VoiceLink> Core<S, L, A, V> {
    /// One request of the sidebar: refused while Kai is not paired or its body is over
    /// `window.proxy_max_mb`; else sent, its thread marked when it starts a turn
    /// (`crate::browser::sidebar::turns`). It ends with an error when Kai leaves the gateway it
    /// went to (`Core::drop_requests`).
    pub(super) fn sidebar(&mut self, relay: Relay) {
        let Relay { request, reply } = relay;
        if let Some(why) = self.window_unpaired() {
            return reply.refuse(503, "not_paired", &why);
        }
        let limits = Limits::from_config(&self.config.window);
        if let Some(refusal) = limits.refuse_body(request.out.body.len()) {
            return reply.refused(refusal);
        }
        let mark = request
            .turn
            .map(|(thread, start)| self.sidebar_turns.mark(thread, start));
        let ahead = self.config.browser.sidebar_ahead_mb;
        let answer = self.voice.link_mut().sidebar(request.out, limits);
        tokio::spawn(reply.run(answer, ahead, mark, self.sidebar_left.clone()));
    }
}
