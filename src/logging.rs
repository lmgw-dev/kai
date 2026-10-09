//! Tracing to stderr (§2), which the autostart unit's journal collects. Level from `KAI_LOG`
//! (an `EnvFilter` string), default `info`.
//!
//! Argv is never logged (the scheme handler's argv carries a key, K13). Two classes of library are
//! capped whatever `KAI_LOG` says, `KAI_LOG=trace` and `KAI_LOG=tungstenite=trace` included (review
//! KAI-2 K2-1): the cap is a second filter every record must also pass, so no directive of
//! `KAI_LOG`, however specific, can lift it.
//!
//! - **The HTTP and WebSocket stack** at `info`: tungstenite traces a handshake's whole request
//!   head, `Authorization: Bearer <device key>` included, and hyper, h2, reqwest and rustls see
//!   the feed's and the folder requests' headers.
//! - **D-Bus** at `warn`: zbus, ksni and the Secret Service crate see every message, the `Pair`
//!   call's link with its key among them.
//! - **rmcp** at `warn`: its traces carry every MCP message of the tool link whole, a call's
//!   arguments and its result among them (the hosted server logs a call by its tool and caller).
//!
//! Kai's own lines never carry the key, a link or audio: an append to the voice session is logged as
//! its kind and sample count only (`voice::session`), a feed event this build cannot read as its
//! name only (`lmgw_link::feed`).

use std::io::IsTerminal;

use tracing::Subscriber;
use tracing_subscriber::filter::{FilterExt, LevelFilter, Targets};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// The libraries capped whatever `KAI_LOG` says, and their cap (module doc). A target matches by
/// prefix (`hyper` covers `hyper_util`, `rustls` the platform verifier).
pub const CAPPED: [(&str, LevelFilter); 10] = [
    ("tungstenite", LevelFilter::INFO),
    ("tokio_tungstenite", LevelFilter::INFO),
    ("hyper", LevelFilter::INFO),
    ("h2", LevelFilter::INFO),
    ("reqwest", LevelFilter::INFO),
    ("rustls", LevelFilter::INFO),
    ("zbus", LevelFilter::WARN),
    ("ksni", LevelFilter::WARN),
    ("secret_service", LevelFilter::WARN),
    ("rmcp", LevelFilter::WARN),
];

/// The cap: everything else passes, to be filtered by `KAI_LOG`.
fn cap() -> Targets {
    Targets::new()
        .with_default(LevelFilter::TRACE)
        .with_targets(CAPPED)
}

/// Kai's subscriber: `spec` as an `EnvFilter` (an unreadable one is `info`), and the cap on top.
pub fn subscriber<W>(spec: &str, writer: W, ansi: bool) -> impl Subscriber + Send + Sync
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let env = EnvFilter::try_new(spec).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_ansi(ansi)
            .with_target(false)
            .with_filter(env.and(cap())),
    )
}

/// Installs the subscriber for `KAI_LOG`, and the `log` bridge (tungstenite and others log through
/// the `log` crate) at its level, so the cap applies to their records too.
pub fn init() {
    let spec = std::env::var("KAI_LOG").unwrap_or_else(|_| "info".to_owned());
    subscriber(&spec, std::io::stderr, std::io::stderr().is_terminal()).init();
}

/// A writer into memory, for the tests that read the log.
#[cfg(test)]
pub mod test_support {
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone, Default)]
    pub struct Capture(pub Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'w> MakeWriter<'w> for Capture {
        type Writer = Capture;
        fn make_writer(&'w self) -> Self::Writer {
            self.clone()
        }
    }

    impl Capture {
        pub fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::Capture;
    use super::*;

    #[test]
    fn the_cap_holds_against_any_kai_log() {
        for spec in [
            "trace",
            "trace,tungstenite=trace,hyper=trace,zbus=trace",
            "trace,tungstenite::handshake::client=trace,zbus::connection=trace",
        ] {
            let out = Capture::default();
            tracing::subscriber::with_default(subscriber(spec, out.clone(), false), || {
                tracing::trace!(target: "tungstenite::handshake::client", "Request: secret");
                tracing::debug!(target: "hyper_util::client", "headers: secret");
                tracing::info!(target: "zbus::connection", "a message: secret");
                tracing::info!(target: "tungstenite::protocol", "an info line");
                tracing::warn!(target: "zbus::connection", "a warning");
                tracing::trace!(target: "kai::voice", "Kai's own trace");
            });
            let text = out.text();
            assert!(!text.contains("secret"), "{spec}: {text}");
            for want in ["an info line", "a warning", "Kai's own trace"] {
                assert!(text.contains(want), "{spec}: {want} missing in {text}");
            }
        }
    }
}
