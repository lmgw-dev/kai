//! The configuration window's Personality page on the core (WP21b): the facts about Kai's
//! conversation it shows in its header, and its profile-editor requests, sent to the paired lmgw
//! through the voice's link with the device key (`crate::lmgw_link::proxy`). The request runs in a
//! spawned task, so the core's loop is never held by it.

use tokio::sync::oneshot;

use super::Core;
use crate::audio::AudioPort;
use crate::lmgw_link::LmgwLink;
use crate::lmgw_link::proxy::{Forward, Limits, Proxied};
use crate::secret::SecretStore;
use crate::settings::window::profiles::Conversation;
use crate::tray::model::{DISABLED, EXPIRED, Pairing};
use crate::voice::VoiceLink;

/// What the page says while Kai is not paired.
pub(crate) const NOT_PAIRED: &str =
    "Kai is not paired with lmgw: pair it from the tray's \"Pair with lmgw…\"";

impl<S: SecretStore, L: LmgwLink, A: AudioPort, V: VoiceLink> Core<S, L, A, V> {
    /// Why the page cannot reach lmgw now, in the owner's words; `None` while Kai is paired.
    fn window_unpaired(&self) -> Option<String> {
        match &self.model.pairing {
            Pairing::Paired { .. } => None,
            Pairing::Disabled { .. } => Some(format!("Kai cannot reach lmgw: {DISABLED}")),
            Pairing::Expired { .. } => Some(format!("Kai cannot reach lmgw: {EXPIRED}")),
            Pairing::Revoked { .. } => Some(
                "Kai cannot reach lmgw: this device's key was revoked; pair again from the \
                 tray's \"Pair with lmgw…\""
                    .to_owned(),
            ),
            Pairing::Checking | Pairing::WalletLocked { .. } | Pairing::WalletError { .. } => Some(
                "Kai is still reading its pairing from the wallet; open the page again in a moment"
                    .to_owned(),
            ),
            Pairing::NotPaired => Some(NOT_PAIRED.to_owned()),
        }
    }

    pub(super) fn window_conversation(&self) -> Conversation {
        let why = self.window_unpaired();
        let folder = self.conversation.folder();
        let thread = self.voice.thread_id();
        let menu = self
            .personality
            .menu(folder.and_then(|f| f.profile_id), thread);
        Conversation {
            paired: why.is_none(),
            why,
            gateway: self.config.lmgw.url.clone(),
            folder: folder.map(|f| f.name.clone()),
            thread_id: thread,
            model: folder.and_then(|f| f.model.clone()),
            profile: menu.options.get(menu.selected).map(|o| o.label.clone()),
        }
    }

    pub(super) fn window_proxy(&mut self, forward: Forward, reply: oneshot::Sender<Proxied>) {
        if let Some(why) = self.window_unpaired() {
            let _ = reply.send(Proxied::refused(503, "not_paired", &why));
            return;
        }
        let limits = Limits::from_config(&self.config.window);
        if let Some(refusal) = limits.refuse_request(&forward) {
            let _ = reply.send(refusal);
            return;
        }
        let answer = self.voice.link_mut().proxy(forward, limits);
        tokio::spawn(async move {
            let _ = reply.send(answer.await);
        });
    }
}
