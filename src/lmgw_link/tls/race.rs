//! Device link §6.3's rule for several URLs ([`race`]): each URL in order, the next one started a
//! stagger (`lmgw.url_stagger_ms`) after the previous one if no connection to that one is up yet,
//! at once after a failure; the first answer wins. "Up" is the TCP connection and the TLS handshake
//! that verified lmgw ([`Answered`]): each attempt's HTTP client is its own
//! ([`super::Tls::racing`]), with a layer on reqwest's connector that says when its connection is
//! made, so a gateway that is reached but slow to answer is not raced by the next URL.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use tokio::sync::watch;

/// Says when an attempt's connection is up (TCP, and TLS with lmgw verified). Cheap to clone.
#[derive(Clone)]
pub struct Answered(Arc<watch::Sender<bool>>);

impl Answered {
    /// A fresh one, and what [`race`] watches.
    pub fn new() -> (Self, watch::Receiver<bool>) {
        let (tx, rx) = watch::channel(false);
        (Self(Arc::new(tx)), rx)
    }

    /// The connection is up.
    pub fn up(&self) {
        self.0.send_replace(true);
    }
}

/// The layer on reqwest's connector ([`super::Tls::racing`]) that says [`Answered`] once a
/// connection is made: reqwest's connector returns it after TCP and the TLS handshake.
#[derive(Clone)]
pub(super) struct OnConnect(pub(super) Answered);

impl<S> tower_layer::Layer<S> for OnConnect {
    type Service = OnConnectService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        OnConnectService {
            inner,
            answered: self.0.clone(),
        }
    }
}

#[derive(Clone)]
pub(super) struct OnConnectService<S> {
    inner: S,
    answered: Answered,
}

impl<S, R> tower_service::Service<R> for OnConnectService<S>
where
    S: tower_service::Service<R>,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: R) -> Self::Future {
        let connecting = self.inner.call(req);
        let answered = self.answered.clone();
        Box::pin(async move {
            let made = connecting.await;
            if made.is_ok() {
                answered.up();
            }
            made
        })
    }
}

/// The rule (module doc): `attempt` for each of `urls` in order, with an [`Answered`] it hands to
/// its connection; the next started `stagger` after the previous one unless that one's connection
/// is up or it has ended, at once after a failure; the first that succeeds wins and the rest are
/// dropped. All failed: each URL's error, in the list's order.
pub async fn race<T, E, F, Fut>(
    urls: Vec<String>,
    stagger: Duration,
    mut attempt: F,
) -> Result<(String, T), Vec<(String, E)>>
where
    F: FnMut(&str, Answered) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut waiting = urls.into_iter().enumerate();
    let mut running = FuturesUnordered::new();
    let mut failed: Vec<(usize, String, E)> = Vec::new();
    // The latest attempt's connection, and when the next starts unless it is up first.
    let mut latest: Option<watch::Receiver<bool>> = None;
    let mut start = |running: &mut FuturesUnordered<_>, latest: &mut Option<_>| match waiting.next()
    {
        Some((i, url)) => {
            let (answered, up) = Answered::new();
            let fut = attempt(&url, answered);
            running.push(async move { (i, url, fut.await) });
            *latest = Some(up);
            Some(tokio::time::Instant::now() + stagger)
        }
        None => None,
    };
    let mut next_at = start(&mut running, &mut latest);
    loop {
        let timed = next_at.is_some();
        tokio::select! {
            Some((i, url, r)) = running.next() => match r {
                Ok(t) => return Ok((url, t)),
                Err(e) => {
                    failed.push((i, url, e));
                    next_at = start(&mut running, &mut latest);
                    if next_at.is_none() && running.is_empty() {
                        failed.sort_by_key(|(i, ..)| *i);
                        return Err(failed.into_iter().map(|(_, u, e)| (u, e)).collect());
                    }
                }
            },
            () = tokio::time::sleep_until(next_at.unwrap_or_else(tokio::time::Instant::now)), if timed => {
                next_at = start(&mut running, &mut latest);
            }
            // The latest attempt reached lmgw: the next URL waits for it to fail. An attempt that
            // ended says so through `running`.
            () = async {
                let reached = match latest.as_mut() {
                    Some(up) => up.wait_for(|up| *up).await.is_ok(),
                    None => false,
                };
                if !reached {
                    std::future::pending::<()>().await;
                }
            }, if timed => {
                next_at = None;
            }
        }
    }
}
