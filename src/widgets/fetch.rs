//! The one HTTP fetch of the overlays (K19): `overlay_image`'s picture and `overlay_weather`'s data,
//! nothing else. The rules, all visible in `[overlay]` or here:
//!
//! - **http and https only**; the answer's status must be 2xx;
//! - **the body streams against `overlay.image_max_mb`**, a `Content-Length` over it is refused
//!   before a byte is read, and the error names the key;
//! - **redirects** are followed only to the same origin (same host and port; `http` may go to
//!   `https`), at most [`MAX_REDIRECTS`]; any other target is refused and named by its origin, so
//!   the caller can use that address itself;
//! - **private addresses**: loopback, link-local and private (LAN) addresses are fetched like any
//!   other (the owner's decision, 2026-10-06) unless `overlay.fetch_private_addresses = false`;
//!   then a name is resolved first, every address it has is checked, the connection is pinned to
//!   what was checked (a second lookup cannot differ), and the refusal names the key and is logged;
//! - **a deadline**, `overlay.fetch_timeout_s`, for the whole request, named in its error;
//! - errors name the status or the size, never the body.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use url::Url;

/// How many redirects one fetch follows.
pub const MAX_REDIRECTS: usize = 5;

/// The fetch rules, from `[overlay]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub max_bytes: u64,
    pub max_mb: u64,
    pub private_addresses: bool,
    pub timeout: Duration,
    pub timeout_s: u64,
}

impl Policy {
    pub fn of(o: &crate::config::OverlayConfig) -> Self {
        Self {
            max_bytes: o.image_max_mb.saturating_mul(1024 * 1024),
            max_mb: o.image_max_mb,
            private_addresses: o.fetch_private_addresses,
            timeout: Duration::from_secs(o.fetch_timeout_s),
            timeout_s: o.fetch_timeout_s,
        }
    }
}

/// What came back.
#[derive(Debug)]
pub struct Fetched {
    /// The `Content-Type` header's media type, lowercase, without parameters.
    pub media_type: Option<String>,
    pub bytes: Vec<u8>,
}

/// Addresses a fetch must not reach with `overlay.fetch_private_addresses = false`: loopback,
/// link-local, private (LAN), unspecified, shared (CGNAT `100.64.0.0/10`), benchmarking
/// (`198.18.0.0/15`), `0.0.0.0/8`, reserved and broadcast (`240.0.0.0/4`), multicast, IPv6
/// unique-local, site-local (`fec0::/10`), and the IPv6 forms that carry an IPv4 address
/// (mapped, NAT64 `64:ff9b::/96`, 6to4 `2002::/16`), judged by the address they carry.
pub fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            a.is_loopback()
                || a.is_link_local()
                || a.is_private()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.is_multicast()
                || o[0] == 0 // 0.0.0.0/8
                || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10
                || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18.0.0/15
                || o[0] >= 240 // 240.0.0.0/4
        }
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_private(IpAddr::V4(v4));
            }
            let s = a.segments();
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                // NAT64: the IPv4 address is the last 32 bits.
                let v4 = std::net::Ipv4Addr::new(
                    (s[6] >> 8) as u8,
                    s[6] as u8,
                    (s[7] >> 8) as u8,
                    s[7] as u8,
                );
                return is_private(IpAddr::V4(v4));
            }
            if s[0] == 0x2002 {
                // 6to4: the IPv4 address follows the prefix.
                let v4 = std::net::Ipv4Addr::new(
                    (s[1] >> 8) as u8,
                    s[1] as u8,
                    (s[2] >> 8) as u8,
                    s[2] as u8,
                );
                return is_private(IpAddr::V4(v4));
            }
            a.is_loopback()
                || a.is_unspecified()
                || a.is_multicast() // ff00::/8
                || (s[0] & 0xffc0) == 0xfe80 // link-local
                || (s[0] & 0xffc0) == 0xfec0 // site-local
                || (s[0] & 0xfe00) == 0xfc00 // unique local
        }
    }
}

fn origin(u: &Url) -> String {
    u.origin().ascii_serialization()
}

/// Whether a redirect from `from` to `to` stays on the same origin (`http` to `https` of the same
/// host and its default port counts).
fn same_origin(from: &Url, to: &Url) -> bool {
    if from.origin() == to.origin() {
        return true;
    }
    from.scheme() == "http"
        && to.scheme() == "https"
        && from.host_str() == to.host_str()
        && from.port_or_known_default() == Some(80)
        && to.port_or_known_default() == Some(443)
}

/// GETs `url` under `policy`; `tool` and the error's words are the caller's. The whole fetch (name
/// lookups, every hop, the body) shares one `overlay.fetch_timeout_s`.
pub async fn get(tool: &str, url: &str, policy: &Policy) -> Result<Fetched, String> {
    match tokio::time::timeout(policy.timeout, get_within(tool, url, policy)).await {
        Ok(r) => r,
        Err(_) => Err(format!(
            "{tool}: the fetch (lookup, redirects and body together) took longer than overlay.fetch_timeout_s = {}",
            policy.timeout_s
        )),
    }
}

async fn get_within(tool: &str, url: &str, policy: &Policy) -> Result<Fetched, String> {
    let mut url = Url::parse(url).map_err(|e| format!("{tool}: not a URL ({e})"))?;
    let mut hops = 0;
    loop {
        let reply = hop(tool, &url, policy).await?;
        let (status, location) = (reply.status(), reply.headers().get("location").cloned());
        if status.is_redirection() {
            let Some(loc) = location.and_then(|l| l.to_str().ok().map(str::to_owned)) else {
                return Err(format!(
                    "{tool}: {} answered {status} without a Location",
                    origin(&url)
                ));
            };
            let next = url.join(&loc).map_err(|_| {
                format!(
                    "{tool}: {} redirected to an address Kai cannot read",
                    origin(&url)
                )
            })?;
            if !same_origin(&url, &next) {
                return Err(format!(
                    "{tool}: {} redirected to {}; Kai follows redirects within one origin only (K19), use that address",
                    origin(&url),
                    origin(&next)
                ));
            }
            hops += 1;
            if hops > MAX_REDIRECTS {
                return Err(format!(
                    "{tool}: more than {MAX_REDIRECTS} redirects from {}",
                    origin(&url)
                ));
            }
            url = next;
            continue;
        }
        if !status.is_success() {
            return Err(format!("{tool}: {} answered {status}", origin(&url)));
        }
        return read(tool, &url, reply, policy).await;
    }
}

/// One request to `url`, redirects not followed.
// A picture on the open web a tool asked for, never lmgw: device link D15's rule (`clippy.toml`)
// is lmgw's connections'.
#[allow(clippy::disallowed_methods)]
async fn hop(tool: &str, url: &Url, policy: &Policy) -> Result<reqwest::Response, String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "{tool}: only http and https addresses are fetched, not {}:",
            url.scheme()
        ));
    }
    let Some(host) = url.host_str() else {
        return Err(format!("{tool}: the address has no host"));
    };
    let port = url.port_or_known_default().unwrap_or(80);
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        // Proxy variables in the environment would send the request elsewhere than the checked address.
        .no_proxy()
        .timeout(policy.timeout);
    if !policy.private_addresses {
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.trim_matches(['[', ']']), port))
            .await
            .map_err(|e| format!("{tool}: cannot resolve {host} ({e})"))?
            .collect();
        if addrs.is_empty() {
            return Err(format!("{tool}: {host} has no address"));
        }
        if let Some(a) = addrs.iter().find(|a| is_private(a.ip())) {
            tracing::warn!(host, "widgets: a fetch to a private address was refused");
            return Err(format!(
                "{tool}: {host} is a private address ({}); overlay.fetch_private_addresses is false",
                a.ip()
            ));
        }
        builder = builder.resolve_to_addrs(host, &addrs);
    }
    let client = builder
        .build()
        .map_err(|e| format!("{tool}: cannot set up the request ({e})"))?;
    client.get(url.clone()).send().await.map_err(|e| {
        if e.is_timeout() {
            format!(
                "{tool}: no answer from {} within overlay.fetch_timeout_s = {}",
                origin(url),
                policy.timeout_s
            )
        } else {
            // reqwest's text can hold the URL; the origin is what the caller may see.
            format!("{tool}: cannot reach {} ({})", origin(url), e.without_url())
        }
    })
}

/// The body, against the size bound.
async fn read(
    tool: &str,
    url: &Url,
    mut reply: reqwest::Response,
    policy: &Policy,
) -> Result<Fetched, String> {
    let too_big = |size: Option<u64>| {
        let size = size
            .map(|s| format!(" ({} MB)", s.div_ceil(1024 * 1024)))
            .unwrap_or_default();
        format!(
            "{tool}: the answer from {} is larger than overlay.image_max_mb = {}{size}",
            origin(url),
            policy.max_mb
        )
    };
    if let Some(len) = reply.content_length().filter(|l| *l > policy.max_bytes) {
        return Err(too_big(Some(len)));
    }
    let media_type = reply
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        });
    let mut bytes = Vec::new();
    loop {
        let chunk = reply.chunk().await.map_err(|e| {
            if e.is_timeout() {
                format!(
                    "{tool}: {} stopped sending within overlay.fetch_timeout_s = {}",
                    origin(url),
                    policy.timeout_s
                )
            } else {
                format!(
                    "{tool}: the answer from {} broke off ({})",
                    origin(url),
                    e.without_url()
                )
            }
        })?;
        let Some(chunk) = chunk else { break };
        if (bytes.len() + chunk.len()) as u64 > policy.max_bytes {
            return Err(too_big(None));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Fetched { media_type, bytes })
}

#[cfg(test)]
pub mod stand_in {
    //! A loopback HTTP server for the fetch tests: no real network, ever.

    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// One canned answer: the raw bytes after the request.
    pub type Route = (&'static str, Vec<u8>);

    pub struct Server {
        pub port: u16,
        /// The request lines seen.
        pub seen: Arc<Mutex<Vec<String>>>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    /// An HTTP/1.1 response.
    pub fn response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
        for (k, v) in headers {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        let mut out = out.into_bytes();
        out.extend_from_slice(body);
        out
    }

    /// Serves `routes` by the request's path (with its query); anything else is a 404.
    pub async fn serve(routes: Vec<Route>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let routes = routes.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let mut n = 0;
                    while !buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut buf[n..]).await {
                            Ok(0) | Err(_) => return,
                            Ok(m) => n += m,
                        }
                    }
                    let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let path = head
                        .lines()
                        .next()
                        .and_then(|l| l.split(' ').nth(1))
                        .unwrap_or("/")
                        .to_owned();
                    log.lock().unwrap().push(path.clone());
                    let answer = routes
                        .iter()
                        .find(|(p, _)| {
                            path == *p || (p.ends_with('*') && path.starts_with(&p[..p.len() - 1]))
                        })
                        .map(|(_, a)| a.clone())
                        .unwrap_or_else(|| response("404 Not Found", &[], b"not here"));
                    let _ = sock.write_all(&answer).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        Server { port, seen, task }
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::{response, serve};
    use super::*;

    fn policy(max_mb: u64, private: bool) -> Policy {
        Policy {
            max_bytes: max_mb * 1024 * 1024,
            max_mb,
            private_addresses: private,
            timeout: Duration::from_secs(5),
            timeout_s: 5,
        }
    }

    #[tokio::test]
    async fn a_body_comes_back_with_its_media_type() {
        let s = serve(vec![(
            "/a",
            response(
                "200 OK",
                &[("Content-Type", "Image/PNG; charset=x")],
                b"abc",
            ),
        )])
        .await;
        let got = get(
            "overlay_image",
            &format!("http://127.0.0.1:{}/a", s.port),
            &policy(1, true),
        )
        .await
        .unwrap();
        assert_eq!(got.media_type.as_deref(), Some("image/png"));
        assert_eq!(got.bytes, b"abc");
    }

    #[tokio::test]
    async fn status_errors_name_the_status_never_the_body() {
        let s = serve(vec![]).await;
        let e = get(
            "overlay_image",
            &format!("http://127.0.0.1:{}/x?secret=1", s.port),
            &policy(1, true),
        )
        .await
        .unwrap_err();
        assert!(e.contains("404"), "{e}");
        assert!(!e.contains("not here") && !e.contains("secret"), "{e}");
    }

    #[tokio::test]
    async fn the_size_bound_is_named_by_header_and_by_stream() {
        let big = vec![7u8; 1024 * 1024 + 1];
        let s = serve(vec![
            ("/len", response("200 OK", &[], &big)),
            (
                "/chunked",
                [
                    b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec(),
                    big.clone(),
                ]
                .concat(),
            ),
        ])
        .await;
        for p in ["len", "chunked"] {
            let e = get(
                "overlay_image",
                &format!("http://127.0.0.1:{}/{p}", s.port),
                &policy(1, true),
            )
            .await
            .unwrap_err();
            assert!(e.contains("overlay.image_max_mb = 1"), "{p}: {e}");
        }
        let ok = get(
            "overlay_image",
            &format!("http://127.0.0.1:{}/len", s.port),
            &policy(2, true),
        )
        .await;
        assert!(ok.is_ok());
    }

    #[tokio::test]
    async fn redirects_stay_in_one_origin_and_are_counted() {
        let s = serve(vec![
            ("/r1", response("302 Found", &[("Location", "/r2")], b"")),
            ("/r2", response("302 Found", &[("Location", "/ok")], b"")),
            (
                "/ok",
                response("200 OK", &[("Content-Type", "image/png")], b"fine"),
            ),
            (
                "/away",
                response(
                    "302 Found",
                    &[("Location", "http://localhost:1/x?q=1")],
                    b"",
                ),
            ),
            (
                "/loop",
                response("302 Found", &[("Location", "/loop")], b""),
            ),
        ])
        .await;
        let base = format!("http://127.0.0.1:{}", s.port);
        let got = get("overlay_image", &format!("{base}/r1"), &policy(1, true))
            .await
            .unwrap();
        assert_eq!(got.bytes, b"fine");
        let e = get("overlay_image", &format!("{base}/away"), &policy(1, true))
            .await
            .unwrap_err();
        assert!(
            e.contains("http://localhost:1") && e.contains("one origin"),
            "{e}"
        );
        assert!(!e.contains("q=1"), "{e}");
        let e = get("overlay_image", &format!("{base}/loop"), &policy(1, true))
            .await
            .unwrap_err();
        assert!(e.contains("more than 5 redirects"), "{e}");
    }

    #[tokio::test]
    async fn private_addresses_are_refused_with_the_setting_off() {
        let s = serve(vec![("/a", response("200 OK", &[], b"x"))]).await;
        for host in ["127.0.0.1", "localhost"] {
            let e = get(
                "overlay_image",
                &format!("http://{host}:{}/a", s.port),
                &policy(1, false),
            )
            .await
            .unwrap_err();
            assert!(e.contains("overlay.fetch_private_addresses"), "{e}");
        }
        assert!(s.seen.lock().unwrap().is_empty(), "nothing was sent");
    }

    #[tokio::test]
    async fn only_http_and_https_and_real_urls() {
        let e = get("overlay_image", "file:///etc/passwd", &policy(1, true))
            .await
            .unwrap_err();
        assert!(e.contains("only http and https"), "{e}");
        assert!(
            get("overlay_image", "nonsense", &policy(1, true))
                .await
                .unwrap_err()
                .contains("not a URL")
        );
    }

    #[tokio::test]
    async fn proxy_variables_do_not_redirect_the_fetch() {
        // A proxy that would see the request; the fetch must not use it.
        let proxy = serve(vec![("*", response("200 OK", &[], b"via proxy"))]).await;
        let target = serve(vec![("/a", response("200 OK", &[], b"direct"))]).await;
        // SAFETY: only this test touches these names; set before the client is built.
        unsafe {
            std::env::set_var("HTTP_PROXY", format!("http://127.0.0.1:{}", proxy.port));
            std::env::set_var("http_proxy", format!("http://127.0.0.1:{}", proxy.port));
            std::env::set_var("ALL_PROXY", format!("http://127.0.0.1:{}", proxy.port));
        }
        let got = get(
            "overlay_image",
            &format!("http://127.0.0.1:{}/a", target.port),
            &policy(1, true),
        )
        .await;
        unsafe {
            std::env::remove_var("HTTP_PROXY");
            std::env::remove_var("http_proxy");
            std::env::remove_var("ALL_PROXY");
        }
        assert_eq!(got.unwrap().bytes, b"direct");
        assert!(proxy.seen.lock().unwrap().is_empty(), "the proxy saw it");
    }

    #[tokio::test]
    async fn the_deadline_covers_all_hops_together() {
        // Each hop answers after 600 ms; the budget is 900 ms, so no single hop times out.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = l.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    let _ = sock
                        .write_all(&response("302 Found", &[("Location", "/next")], b""))
                        .await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        let mut p = policy(1, true);
        p.timeout = Duration::from_millis(900);
        p.timeout_s = 9;
        let e = get("overlay_image", &format!("http://127.0.0.1:{port}/a"), &p)
            .await
            .unwrap_err();
        task.abort();
        assert!(e.contains("overlay.fetch_timeout_s = 9"), "{e}");
        assert!(e.contains("together"), "{e}");
    }

    #[test]
    fn the_private_ranges() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.200.1",
            "169.254.1.1",
            "::1",
            "fe80::1",
            "fd12:3456::1",
            "::ffff:10.0.0.1",
            "0.0.0.0",
            "0.1.2.3",
            "100.64.0.1",
            "100.127.255.254",
            "198.18.0.1",
            "198.19.255.255",
            "255.255.255.255",
            "224.0.0.1",
            "240.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b::a00:1",
            "2002:7f00:1::",
            "2002:c0a8:101::1",
            "fec0::1",
            "ff02::1",
        ] {
            assert!(is_private(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "8.8.8.8",
            "172.32.0.1",
            "2606:4700::1111",
            "::ffff:8.8.8.8",
            "100.63.255.255",
            "100.128.0.1",
            "198.17.255.255",
            "198.20.0.1",
            "64:ff9b::808:808",
            "2002:808:808::1",
        ] {
            assert!(!is_private(ip.parse().unwrap()), "{ip}");
        }
    }
}
