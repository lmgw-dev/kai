//! The core's side of the hosted server (PD2, PD5): started and stopped where the feed is, its
//! events followed, the tooltip's tools lines.
//!
//! **PD5.** The Kai folder's tool defaults are the owner's: Kai never writes its label into them.
//! While the link is up and the folder's defaults lack the label, the tooltip and one notification
//! name the step, and Open in lmgw opens the folder's settings (§4's no-model pattern).

use tokio::sync::mpsc;

use super::Core;
use crate::audio::AudioPort;
use crate::conversation::FolderInfo;
use crate::host::{HostEvent, HostStatus, ToolHost, tools_line};
use crate::lmgw_link::{Credentials, LmgwLink};
use crate::secret::SecretStore;
use crate::tray::model::{Link, Pairing};
use crate::voice::VoiceLink;

/// The core's hold on the host.
pub(super) struct Hosting {
    host: Option<Box<dyn ToolHost>>,
    pub(super) events: mpsc::UnboundedReceiver<HostEvent>,
    status: Option<HostStatus>,
    /// The `[policy]` settings no tool has, said in the tooltip.
    policy_notes: Vec<String>,
    /// PD5's notification went out for this label and folder: once per run of the link (a
    /// `host_start`) and again only when the label or the folder changes, not at each reconnect.
    step_said: Option<(String, Option<i64>)>,
}

impl Hosting {
    /// The host, for the feed's `hello` (`ToolHost::feed_up`).
    pub(super) fn host_mut(&mut self) -> Option<&mut Box<dyn ToolHost>> {
        self.host.as_mut()
    }
}

impl Default for Hosting {
    fn default() -> Self {
        Self {
            host: None,
            events: mpsc::unbounded_channel().1,
            status: None,
            policy_notes: Vec::new(),
            step_said: None,
        }
    }
}

impl<S: SecretStore, L: LmgwLink, A: AudioPort, V: VoiceLink> Core<S, L, A, V> {
    /// The hosted server (`crate::host::HostLink`); `policy_notes` are its `[policy]` settings
    /// that name no tool.
    pub fn set_host(&mut self, host: Box<dyn ToolHost>, policy_notes: Vec<String>) {
        self.hosting.host = Some(host);
        self.hosting.policy_notes = policy_notes
            .into_iter()
            .map(|k| format!("{k}: Kai offers no such tool; the setting does nothing"))
            .collect();
    }

    /// Beside the feed's start: each start gets its own event channel.
    pub(super) fn host_start(&mut self, creds: Credentials) {
        let Some(host) = self.hosting.host.as_mut() else {
            return;
        };
        let (tx, rx) = mpsc::unbounded_channel();
        self.hosting.events = rx;
        self.hosting.status = Some(HostStatus::Connecting);
        self.hosting.step_said = None;
        host.start(creds, tx);
        self.show_tools();
    }

    /// Beside the feed's stop (revocation, quit).
    pub(super) fn host_stop(&mut self) {
        if let Some(host) = self.hosting.host.as_mut() {
            host.stop();
        }
        self.hosting.events = mpsc::unbounded_channel().1;
        self.hosting.status = None;
        self.show_tools();
    }

    pub(super) async fn on_host_event(&mut self, e: HostEvent) {
        match e {
            HostEvent::Status(s) => self.hosting.status = Some(s),
            HostEvent::Revoked {
                reason,
                loss: Some(loss),
            } => return self.revoke(reason, loss).await,
            HostEvent::Revoked { reason, loss: None } => {
                // A kind this build does not know: lmgw's sentence is said (once per reason, as
                // the feed's), and the link tries again after its backoff. Whether the key still
                // opens is the feed's to find: its own `revoked` or a 401 decides.
                if matches!(self.model.pairing, Pairing::Paired { .. }) {
                    self.say_unknown(&reason);
                }
            }
        }
        self.show_tools();
    }

    /// The label lmgw lists Kai's tools under, as the feed's `hello` said it.
    fn hosts_label(&self) -> Option<&str> {
        match &self.model.pairing {
            Pairing::Paired {
                link: Link::Up { hosts_label, .. },
                ..
            } => hosts_label.as_deref(),
            _ => None,
        }
    }

    /// PD5: the label the Kai folder's tool defaults lack while the link is up, if they do.
    pub(super) fn host_step(&self) -> Option<String> {
        missing_label(
            matches!(self.hosting.status, Some(HostStatus::Linked { .. })),
            self.hosts_label(),
            self.conversation.folder(),
        )
    }

    /// The tooltip's tools lines, and PD5's notification once.
    pub(super) fn show_tools(&mut self) {
        let mut lines = Vec::new();
        if let Some(s) = &self.hosting.status {
            lines.push(tools_line(s, self.hosts_label()));
        }
        lines.extend(self.hosting.policy_notes.iter().cloned());
        lines.extend(crate::host::policy::Policy::new(&self.config.policy).open_notes());
        if let Some(label) = self.host_step() {
            let (folder, folder_id) = self
                .conversation
                .folder()
                .map(|f| (f.name.clone(), Some(f.id)))
                .unwrap_or_default();
            let step = format!(
                "the folder '{folder}' does not give its threads Kai's tools: add '{label}' to its \
                 tools in its settings (Open in lmgw opens them)"
            );
            lines.push(step.clone());
            if should_say(&mut self.hosting.step_said, &label, folder_id) {
                self.notify("Kai: its tools are not in the conversation yet", step);
            }
        }
        if self.model.tools != lines {
            self.model.tools = lines;
            self.publish();
        }
    }
}

/// PD5's notification is due: not yet said for this label and folder in this run of the link
/// (marked said when it is).
fn should_say(said: &mut Option<(String, Option<i64>)>, label: &str, folder: Option<i64>) -> bool {
    let due = said
        .as_ref()
        .is_none_or(|(l, f)| l != label || *f != folder);
    if due {
        *said = Some((label.to_owned(), folder));
    }
    due
}

/// PD5's condition: the link is up, lmgw named the label, the folder is known, and its tool
/// defaults lack the label (lmgw compares labels case-insensitively).
fn missing_label(linked: bool, label: Option<&str>, folder: Option<&FolderInfo>) -> Option<String> {
    let label = label.filter(|_| linked)?;
    let has = folder?
        .tool_labels
        .iter()
        .any(|l| l.eq_ignore_ascii_case(label));
    (!has).then(|| label.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_step_is_named_only_while_linked_and_the_folder_lacks_the_label() {
        let lacking = FolderInfo {
            id: 3,
            name: "Kai".into(),
            tool_labels: vec!["kb".into()],
            ..FolderInfo::default()
        };
        let having = FolderInfo {
            tool_labels: vec!["kb".into(), "KAI".into()],
            ..lacking.clone()
        };
        assert_eq!(
            missing_label(true, Some("kai"), Some(&lacking)),
            Some("kai".into())
        );
        assert_eq!(missing_label(true, Some("kai"), Some(&having)), None);
        assert_eq!(missing_label(false, Some("kai"), Some(&lacking)), None);
        assert_eq!(missing_label(true, None, Some(&lacking)), None);
        assert_eq!(missing_label(true, Some("kai"), None), None);
    }

    /// PD5's one notification: not again at each reconnect (the link's statuses do not reset it),
    /// again for another label or folder, and in the next run of the link (`host_start` resets it).
    #[test]
    fn the_step_is_notified_once_per_label_and_folder() {
        let mut said = None;
        assert!(should_say(&mut said, "kai", Some(3)));
        assert!(!should_say(&mut said, "kai", Some(3)), "a reconnect");
        assert!(should_say(&mut said, "desk", Some(3)), "another label");
        assert!(should_say(&mut said, "desk", Some(4)), "another folder");
        assert!(!should_say(&mut said, "desk", Some(4)));
        said = None; // host_start
        assert!(should_say(&mut said, "desk", Some(4)));
    }
}
