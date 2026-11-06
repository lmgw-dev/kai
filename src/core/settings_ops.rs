//! K30's tools and the configuration window's operations on the core (PD3, WP21b): the owner's
//! configuration operations (`crate::settings`) run here, so the core's copy of the configuration
//! and the host's policy follow what was written (and who looks at screenshots,
//! `delegate.screenshots`, and the conversation's folder name, `conversation.folder_name`).

use super::Core;
use super::calls::CoreCall;
use crate::audio::AudioPort;
use crate::config::Config;
use crate::host::policy::Policy;
use crate::host::registry::Reply;
use crate::lmgw_link::LmgwLink;
use crate::secret::SecretStore;
use crate::settings;
use crate::tray::model::{Link, Pairing};
use crate::voice::VoiceLink;

impl<S: SecretStore, L: LmgwLink, A: AudioPort, V: VoiceLink> Core<S, L, A, V> {
    pub(super) async fn config_tool(&mut self, c: CoreCall) {
        let reply = match settings::tools::request(c.call.spec.name, &c.call.args) {
            Err(why) => Reply::error(why),
            Ok(request) => match self.apply_setting(request, settings::Source::Tool).await {
                Ok(text) => Reply::text(text),
                Err(refusal) => Reply::error(refusal.to_string()),
            },
        };
        let _ = c.reply.send(reply);
    }

    /// One configuration operation, for the tools and (WP21b) the window: the file is written, the
    /// core takes the configuration as it now reads, and a changed `[policy]` reaches the host.
    pub(crate) async fn apply_setting(
        &mut self,
        request: settings::Request,
        source: settings::Source,
    ) -> Result<String, settings::Refusal> {
        Ok(settings::tools::render(
            &self.apply_request_from(request, source).await?,
        ))
    }

    /// The same operation with the structured answer (the owner's own UI).
    pub(crate) async fn apply_request(
        &mut self,
        request: settings::Request,
    ) -> Result<settings::Response, settings::Refusal> {
        self.apply_request_from(request, settings::Source::Owner)
            .await
    }

    /// The window's operation: a refusal names the setting by its label, not its key
    /// ([`settings::catalog::in_words`]).
    pub(crate) async fn apply_window_request(
        &mut self,
        request: settings::Request,
    ) -> Result<settings::Response, settings::Refusal> {
        self.apply_request(request)
            .await
            .map_err(|r| settings::Refusal::plain(settings::catalog::in_words(&r)))
    }

    /// The confirmation's "always allow" box: the tool runs unasked for trusted callers from now
    /// on, written as the settings window would. A refusal (the file does not load, it cannot be
    /// written) is said, since the owner expects no dialog next time.
    pub(super) async fn allow_always(&mut self, tool: &str) {
        let request = settings::Request::ToolState {
            tool: tool.to_owned(),
            state: Some(crate::host::settings::ToolState::AllowTrusted),
        };
        match self.apply_request(request).await {
            Ok(_) => tracing::info!(
                tool,
                "policy.tools: allow_trusted, from the confirmation's \"always allow\""
            ),
            Err(refusal) => {
                tracing::warn!(tool, "\"always allow\" was not saved: {refusal}");
                self.notify(
                    "Kai: \"always allow\" was not saved",
                    format!(
                        "{tool} is still asked about: {}",
                        settings::catalog::in_words(&refusal)
                    ),
                );
            }
        }
    }

    async fn apply_request_from(
        &mut self,
        request: settings::Request,
        source: settings::Source,
    ) -> Result<settings::Response, settings::Refusal> {
        let outcome = settings::apply_from(&self.paths.config_file, request, source)?;
        if let Some(config) = outcome.config {
            self.take_config(config).await;
        }
        Ok(outcome.response)
    }

    async fn take_config(&mut self, config: Config) {
        let policy_changed = Policy::of(&config) != Policy::of(&self.config);
        let screenshots_changed = config.delegate.screenshots != self.config.delegate.screenshots;
        let renamed = config.conversation.folder_name != self.config.conversation.folder_name;
        self.config = config;
        if renamed {
            self.switch_conversation().await;
        }
        self.jobs.set_keep(self.config.jobs.keep_ended());
        if policy_changed && let Some(host) = self.hosting.host_mut() {
            host.set_policy(Policy::of(&self.config));
        }
        if screenshots_changed && let Some(host) = self.hosting.host_mut() {
            host.set_screenshots(self.config.delegate.screenshots);
        }
    }

    /// `conversation.folder_name` changed (the window's Conversation choice, a tool, the file
    /// through an operation): the conversation leaves the old folder and looks the new name up
    /// now, as a start would; its remembered id goes from the file (`Conversation::rename`). A
    /// lookup or create of the old name still on its way is dropped. The session ends without a
    /// report, and listening, if it was on, resumes once the new folder is ready.
    async fn switch_conversation(&mut self) {
        let feed_up = matches!(
            self.model.pairing,
            Pairing::Paired {
                link: Link::Up { .. },
                ..
            }
        );
        let name = self.config.conversation.folder_name.clone();
        // Only a name that changed drops the lookup on its way: a rename to the same name (spaces
        // aside) leaves it, which its answer then ends.
        if let Some(acts) = self.conversation.rename(name, feed_up) {
            self.folder_req = None;
            self.apply(acts).await;
        }
    }
}
