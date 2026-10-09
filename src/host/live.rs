//! The live-test build's tools (`contract::live`, `just live-check-host`, `just
//! live-check-approvals`, `just live-check-jobs`): `live_echo`, `live_note` and `live_slow` (after
//! its seconds, as a job) answer with their name and text. Never in a
//! default or release build.

use std::sync::Arc;

use super::registry::{Entry, Provider, Reply};

pub struct Live;

impl Provider for Live {
    fn entries(&self) -> Vec<Entry> {
        kai_desktop::contract::live::AREA
            .tools
            .iter()
            .map(|spec| Entry {
                area: kai_desktop::contract::live::AREA.name,
                spec,
                handler: Arc::new(move |call| {
                    let text = call.args.text("text").unwrap_or_default().to_owned();
                    let name = spec.name;
                    // `live_slow`: the seconds it was given first (a job, WP18).
                    let wait = call.args.integer("seconds").unwrap_or(0).max(0) as u64;
                    Box::pin(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                        Reply::text(format!("{name}: {text}"))
                    })
                }),
                prepare: None,
            })
            .collect()
    }
}
