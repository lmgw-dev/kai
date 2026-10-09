//! Live checks only: compiled with the cargo feature `live-test`, which no default or release
//! build has (a release build with it does not compile). The live check builds it into its own
//! target directory (`target/live-test`), so `target/debug/kai`, the binary `just install-dev`
//! registers as the `lmgw-pair:` handler and `just run` starts, is never a live-test build. With
//! `KAI_LIVE_LINK=1` in the environment of such a build, Kai runs unattended against a dev lmgw
//! (`scripts/live-check-link.sh`):
//!
//! - a pairing link that reaches the owner's confirmation is answered yes at once, with no dialog,
//!   only when it is one the live script minted: its URL and the SHA-256 of its key are listed in
//!   `KAI_LIVE_LINK_ACCEPT` (review KAI-2 K2-2). Any other link, from any process of the session
//!   or a click in the owner's browser during the run, gets the owner's dialog as in every build;
//! - desktop notifications are written to the log instead of being sent;
//! - no global shortcuts are bound (the portal would name the caller's app scope, not Kai's).
//!
//! The switch is the process's own environment at its start, and such a Kai refuses to start with
//! a configuration outside a `target` directory, so it never runs on the owner's own.

#[cfg(not(debug_assertions))]
compile_error!("the live-test feature is for debug builds only, never a release build");

/// The environment switch.
pub const ENV: &str = "KAI_LIVE_LINK";
/// The links the live script minted: whitespace-separated `<url>#<sha256 of the key, hex>`.
pub const ACCEPT: &str = "KAI_LIVE_LINK_ACCEPT";

pub fn active() -> bool {
    std::env::var(ENV).is_ok_and(|v| v == "1")
}

/// The link for `url` (normalized, as the link parsed) with `key` is one the live script minted.
pub fn accepts(url: &str, key: &str) -> bool {
    active() && listed(&std::env::var(ACCEPT).unwrap_or_default(), url, key)
}

fn listed(list: &str, url: &str, key: &str) -> bool {
    let want = format!("{url}#{}", sha256_hex(key));
    list.split_whitespace().any(|entry| entry == want)
}

fn sha256_hex(text: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A live-test Kai runs only on a configuration under a `target` directory (the live check's).
pub fn config_home_ok() -> Result<(), String> {
    let home = std::env::var_os("XDG_CONFIG_HOME")
        .ok_or("XDG_CONFIG_HOME is not set (a live-test build needs the live check's own)")?;
    let path = std::path::Path::new(&home);
    if path.components().any(|c| c.as_os_str() == "target") {
        Ok(())
    } else {
        Err(format!(
            "XDG_CONFIG_HOME={} is outside a target directory: a live-test build runs only on the \
             live check's own configuration",
            path.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_listed_url_and_key_are_accepted() {
        let key = "lmgw-device-0123";
        // `printf %s lmgw-device-0123 | sha256sum`
        let entry = format!("http://127.0.0.1:8897#{}", sha256_hex(key));
        assert_eq!(sha256_hex("abc").len(), 64);
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let list = format!("http://localhost:8897#{} {entry}", sha256_hex("other"));
        assert!(listed(&list, "http://127.0.0.1:8897", key));
        assert!(!listed(&list, "http://127.0.0.1:8897", "lmgw-device-0124"));
        assert!(!listed(&list, "http://127.0.0.1:8001", key));
        assert!(!listed(&list, "http://localhost:8897", key));
        assert!(!listed("", "http://127.0.0.1:8897", key));
    }
}
