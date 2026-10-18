//! The device's key in the Secret Service (§2; lmgw's device link design §6.2), served on Plasma by
//! `ksecretd` (WP0 (h)).
//!
//! One item per gateway, attributes `{application: dev.rootshell.kai, lmgw: <the gateway's CA
//! fingerprint, sha256:…>, kind: device-tls-key}`, in the default collection, holding the device's
//! P-256 key as PKCS#8 PEM (`crate::pairing::TlsKey`). Load and delete look only there and take only
//! that kind of item (review KAI-2 K2-16): the Secret Service's search matches every item that
//! carries *at least* the given attributes, in every collection. A pairing from before TLS left an
//! item keyed by its URL with `kind: device-key` (or, from an earlier build, no `kind`): it opens
//! nothing on lmgw now, and [`SecretStore::delete_v1`] removes it once Kai is paired again. Kai
//! never locks or unlocks the wallet: it reads the collection's `Locked` property first, and on a
//! locked wallet it reports [`Lookup::Locked`] / [`Stored::Locked`] without searching or writing,
//! so no unlock prompt can appear. The core shows "wallet locked: unlock KWallet" and checks again
//! every `wallet.retry_s` (§2: "Kai retries when the wallet opens").

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use secret_service::{EncryptionType, SecretService};
use tokio::sync::Mutex;

use crate::pairing::TlsKey;

pub const ATTR_APPLICATION: &str = "application";
pub const ATTR_LMGW: &str = "lmgw";
pub const ATTR_KIND: &str = "kind";
/// The `kind` of the device's key item (the device link).
pub const KIND_TLS_KEY: &str = "device-tls-key";
/// The `kind` of a bearer device key's item, from a pairing before TLS.
pub const KIND_V1_KEY: &str = "device-key";

/// The result of reading the key for one gateway.
#[derive(Debug, Clone)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub enum Lookup {
    Found(TlsKey),
    NotFound,
    Locked,
}

/// The result of storing a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stored {
    Stored,
    Locked,
}

/// The result of deleting Kai's items for one gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removed {
    /// How many items went (0: there was none).
    Items(usize),
    /// The wallet is locked: nothing was deleted and no unlock was asked for.
    Locked,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SecretError(pub String);

/// Where the key lives. [`SecretServiceStore`] in Kai; [`MemoryStore`] in tests. `gateway` is the
/// `fp` of the gateway's device CA (`crate::pairing::Paired::fp`).
pub trait SecretStore: Send + Sync + 'static {
    /// Whether the default collection is locked (read without asking for an unlock).
    fn locked(&self) -> impl Future<Output = Result<bool, SecretError>> + Send;
    fn load(&self, gateway: &str) -> impl Future<Output = Result<Lookup, SecretError>> + Send;
    /// Stores `key` for `gateway` under `label` (what KWallet shows), replacing Kai's earlier key
    /// for it.
    fn store(
        &self,
        gateway: &str,
        label: &str,
        key: &TlsKey,
    ) -> impl Future<Output = Result<Stored, SecretError>> + Send;
    /// Deletes Kai's own key items for `gateway` in the default collection (attributes
    /// `application`, `lmgw` and `kind: device-tls-key`), and nothing else.
    fn delete(&self, gateway: &str) -> impl Future<Output = Result<Removed, SecretError>> + Send;
    /// Deletes the bearer key a pairing before TLS kept for `url` (`kind: device-key`, or an
    /// earlier build's item with only `application` and `lmgw`), and nothing else.
    fn delete_v1(&self, url: &str) -> impl Future<Output = Result<Removed, SecretError>> + Send;
}

/// What a search for `lmgw`'s items asks (any kind; [`which`] sorts them out).
fn attributes(lmgw: &str) -> HashMap<&str, &str> {
    HashMap::from([(ATTR_APPLICATION, crate::APP_ID), (ATTR_LMGW, lmgw)])
}

/// What Kai's key item for `gateway` carries.
fn key_attributes(gateway: &str) -> HashMap<&str, &str> {
    let mut a = attributes(gateway);
    a.insert(ATTR_KIND, KIND_TLS_KEY);
    a
}

/// What an item found is to Kai, by its attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    /// The device's key (`kind: device-tls-key`).
    Key,
    /// A bearer key from before TLS: `kind: device-key`, or only `application` and `lmgw` (and
    /// the Secret Service's own `xdg:` attributes), an earlier build's.
    V1,
    /// Anything else (another kind, or more attributes): not Kai's key.
    Other,
}

fn which(attrs: &HashMap<String, String>) -> Which {
    match attrs.get(ATTR_KIND).map(String::as_str) {
        Some(KIND_TLS_KEY) => Which::Key,
        Some(KIND_V1_KEY) => Which::V1,
        Some(_) => Which::Other,
        None if attrs
            .keys()
            .all(|k| k == ATTR_APPLICATION || k == ATTR_LMGW || k.starts_with("xdg:")) =>
        {
            Which::V1
        }
        None => Which::Other,
    }
}

/// The Secret Service over D-Bus. The session is opened on first use and opened again after an
/// error, so a `ksecretd` that starts after Kai is picked up by the next retry.
pub struct SecretServiceStore {
    conn: zbus::Connection,
    timeout: Duration,
    service: Mutex<Option<SecretService<'static>>>,
}

impl SecretServiceStore {
    /// `timeout` is `wallet.timeout_s`; it bounds each whole operation and is named in its error.
    pub fn new(conn: zbus::Connection, timeout: Duration) -> Self {
        Self {
            conn,
            timeout,
            service: Mutex::new(None),
        }
    }

    async fn bounded<T>(
        &self,
        what: &str,
        f: impl Future<Output = Result<T, SecretError>>,
    ) -> Result<T, SecretError> {
        match tokio::time::timeout(self.timeout, f).await {
            Ok(r) => r,
            Err(_) => Err(SecretError(format!(
                "the Secret Service did not answer {what} within {} s (wallet.timeout_s)",
                self.timeout.as_secs()
            ))),
        }
    }

    /// The session, opened first when needed. Callers drop it after an error, so the next call
    /// opens a fresh one (ksecretd may have restarted).
    async fn session_guard(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<SecretService<'static>>>, SecretError> {
        let mut guard = self.service.lock().await;
        if guard.is_none() {
            let ss = SecretService::connect_with_existing(EncryptionType::Dh, self.conn.clone())
                .await
                .map_err(|e| SecretError(format!("cannot open a Secret Service session: {e}")))?;
            *guard = Some(ss);
        }
        Ok(guard)
    }
}

/// Runs `$call` on the store's session, bounded by `wallet.timeout_s` (named as `$what`); an
/// error drops the session, so the next call opens a fresh one.
macro_rules! with_session {
    ($store:expr, $what:expr, |$ss:ident| $call:expr) => {
        $store
            .bounded($what, async {
                let mut guard = $store.session_guard().await?;
                let $ss = guard.as_ref().expect("opened");
                let result = $call.await;
                if result.is_err() {
                    *guard = None;
                }
                result
            })
            .await
    };
}

fn ss_err(context: &str) -> impl Fn(secret_service::Error) -> SecretError + '_ {
    move |e| match e {
        secret_service::Error::NoResult => SecretError(
            "the Secret Service has no default collection: create a wallet in KWallet".into(),
        ),
        other => SecretError(format!("{context}: {other}")),
    }
}

impl SecretStore for SecretServiceStore {
    async fn locked(&self) -> Result<bool, SecretError> {
        with_session!(self, "whether the wallet is locked", |ss| locked_with(ss))
    }

    async fn load(&self, gateway: &str) -> Result<Lookup, SecretError> {
        with_session!(self, "a lookup", |ss| load_with(ss, gateway))
    }

    async fn store(&self, gateway: &str, label: &str, key: &TlsKey) -> Result<Stored, SecretError> {
        with_session!(self, "a store", |ss| store_with(ss, gateway, label, key))
    }

    async fn delete(&self, gateway: &str) -> Result<Removed, SecretError> {
        with_session!(self, "a delete", |ss| delete_with(ss, gateway, Which::Key))
    }

    async fn delete_v1(&self, url: &str) -> Result<Removed, SecretError> {
        with_session!(self, "a delete", |ss| delete_with(ss, url, Which::V1))
    }
}

async fn locked_with(ss: &SecretService<'_>) -> Result<bool, SecretError> {
    ss.get_default_collection()
        .await
        .map_err(ss_err("reading the default collection"))?
        .is_locked()
        .await
        .map_err(ss_err("reading whether the wallet is locked"))
}

/// The default collection, unless it is locked (checked first: a search, a write or a delete on
/// a locked collection would raise an unlock prompt).
async fn open_collection<'a>(
    ss: &'a SecretService<'_>,
) -> Result<Option<secret_service::Collection<'a>>, SecretError> {
    let collection = ss
        .get_default_collection()
        .await
        .map_err(ss_err("reading the default collection"))?;
    if collection
        .is_locked()
        .await
        .map_err(ss_err("reading whether the wallet is locked"))?
    {
        return Ok(None);
    }
    Ok(Some(collection))
}

async fn load_with(ss: &SecretService<'_>, gateway: &str) -> Result<Lookup, SecretError> {
    let Some(collection) = open_collection(ss).await? else {
        return Ok(Lookup::Locked);
    };
    let found = collection
        .search_items(attributes(gateway))
        .await
        .map_err(ss_err("searching Kai's item"))?;
    let mut keys = Vec::new();
    for item in found {
        let attrs = item
            .get_attributes()
            .await
            .map_err(ss_err("reading Kai's item"))?;
        if which(&attrs) == Which::Key {
            keys.push(item);
        }
    }
    if keys.len() > 1 {
        tracing::warn!(
            count = keys.len(),
            "more than one Kai key item for this gateway; using the first"
        );
    }
    match keys.first() {
        Some(item) => {
            // The secret's bytes are wiped when dropped, read in place (no copy as a String).
            let bytes = zeroize::Zeroizing::new(
                item.get_secret()
                    .await
                    .map_err(ss_err("reading Kai's item"))?,
            );
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| SecretError("Kai's item does not hold text".into()))?;
            let key = TlsKey::from_pem(text).map_err(SecretError)?;
            Ok(Lookup::Found(key))
        }
        None => Ok(Lookup::NotFound),
    }
}

async fn store_with(
    ss: &SecretService<'_>,
    gateway: &str,
    label: &str,
    key: &TlsKey,
) -> Result<Stored, SecretError> {
    let Some(collection) = open_collection(ss).await? else {
        return Ok(Stored::Locked);
    };
    let item = collection
        .create_item(
            label,
            key_attributes(gateway),
            key.to_pem().as_bytes(),
            true,
            "text/plain",
        )
        .await
        .map_err(ss_err("storing Kai's item"))?;
    // A provider that does not replace by attributes leaves the earlier key: it goes now.
    let old = delete_items(&collection, gateway, Which::Key, Some(&item.item_path)).await?;
    if old > 0 {
        tracing::info!(
            items = old,
            "Kai's earlier key for this gateway was deleted"
        );
    }
    Ok(Stored::Stored)
}

async fn delete_with(
    ss: &SecretService<'_>,
    lmgw: &str,
    kind: Which,
) -> Result<Removed, SecretError> {
    let Some(collection) = open_collection(ss).await? else {
        return Ok(Removed::Locked);
    };
    Ok(Removed::Items(
        delete_items(&collection, lmgw, kind, None).await?,
    ))
}

/// Deletes `lmgw`'s items of `kind` in `collection`, all but the one at `keep`. The search matches
/// supersets: each item's own attributes decide.
async fn delete_items(
    collection: &secret_service::Collection<'_>,
    lmgw: &str,
    kind: Which,
    keep: Option<&zbus::zvariant::OwnedObjectPath>,
) -> Result<usize, SecretError> {
    let found = collection
        .search_items(attributes(lmgw))
        .await
        .map_err(ss_err("searching Kai's item"))?;
    let mut n = 0;
    for item in &found {
        if keep.is_some_and(|k| *k == item.item_path) {
            continue;
        }
        let attrs = item
            .get_attributes()
            .await
            .map_err(ss_err("reading Kai's item"))?;
        if which(&attrs) == kind {
            item.delete().await.map_err(ss_err("deleting Kai's item"))?;
            n += 1;
        }
    }
    Ok(n)
}

/// An in-memory store for tests: a lock switch, a failure switch, the keys by gateway and the
/// URLs that still hold a key from before TLS.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore {
    pub locked: std::sync::atomic::AtomicBool,
    pub fail: std::sync::Mutex<Option<String>>,
    pub items: std::sync::Mutex<HashMap<String, (String, TlsKey)>>,
    pub v1: std::sync::Mutex<std::collections::HashSet<String>>,
}

#[cfg(test)]
impl MemoryStore {
    pub fn set_locked(&self, locked: bool) {
        self.locked
            .store(locked, std::sync::atomic::Ordering::SeqCst);
    }
    fn check(&self) -> Result<bool, SecretError> {
        if let Some(msg) = self.fail.lock().unwrap().clone() {
            return Err(SecretError(msg));
        }
        Ok(self.locked.load(std::sync::atomic::Ordering::SeqCst))
    }
}

#[cfg(test)]
impl SecretStore for MemoryStore {
    async fn locked(&self) -> Result<bool, SecretError> {
        self.check()
    }

    async fn load(&self, gateway: &str) -> Result<Lookup, SecretError> {
        if self.check()? {
            return Ok(Lookup::Locked);
        }
        Ok(match self.items.lock().unwrap().get(gateway) {
            Some((_, key)) => Lookup::Found(key.clone()),
            None => Lookup::NotFound,
        })
    }

    async fn store(&self, gateway: &str, label: &str, key: &TlsKey) -> Result<Stored, SecretError> {
        if self.check()? {
            return Ok(Stored::Locked);
        }
        self.items
            .lock()
            .unwrap()
            .insert(gateway.to_owned(), (label.to_owned(), key.clone()));
        Ok(Stored::Stored)
    }

    async fn delete(&self, gateway: &str) -> Result<Removed, SecretError> {
        if self.check()? {
            return Ok(Removed::Locked);
        }
        let gone = self.items.lock().unwrap().remove(gateway).is_some();
        Ok(Removed::Items(usize::from(gone)))
    }

    async fn delete_v1(&self, url: &str) -> Result<Removed, SecretError> {
        if self.check()? {
            return Ok(Removed::Locked);
        }
        let gone = self.v1.lock().unwrap().remove(url);
        Ok(Removed::Items(usize::from(gone)))
    }
}

impl<S: SecretStore> SecretStore for std::sync::Arc<S> {
    fn locked(&self) -> impl Future<Output = Result<bool, SecretError>> + Send {
        (**self).locked()
    }
    fn load(&self, gateway: &str) -> impl Future<Output = Result<Lookup, SecretError>> + Send {
        (**self).load(gateway)
    }
    fn store(
        &self,
        gateway: &str,
        label: &str,
        key: &TlsKey,
    ) -> impl Future<Output = Result<Stored, SecretError>> + Send {
        (**self).store(gateway, label, key)
    }
    fn delete(&self, gateway: &str) -> impl Future<Output = Result<Removed, SecretError>> + Send {
        (**self).delete(gateway)
    }
    fn delete_v1(&self, url: &str) -> impl Future<Output = Result<Removed, SecretError>> + Send {
        (**self).delete_v1(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// Review KAI-2 K2-16: the search matches supersets; only Kai's key item counts as the key, and
    /// only a bearer key's item of exactly its shape as one from before TLS.
    #[test]
    fn only_kais_own_key_item_counts() {
        let base = [("application", crate::APP_ID), ("lmgw", "sha256:00")];
        let with = |extra: &[(&str, &str)]| {
            let mut v = base.to_vec();
            v.extend_from_slice(extra);
            attrs(&v)
        };
        assert_eq!(which(&with(&[("kind", "device-tls-key")])), Which::Key);
        assert_eq!(
            which(&with(&[("kind", "device-tls-key"), ("xdg:schema", "x")])),
            Which::Key
        );
        assert_eq!(which(&with(&[("kind", "device-key")])), Which::V1);
        assert_eq!(which(&with(&[])), Which::V1);
        assert_eq!(which(&with(&[("xdg:schema", "x")])), Which::V1);
        assert_eq!(which(&with(&[("kind", "memory-key")])), Which::Other);
        assert_eq!(which(&with(&[("folder", "copy")])), Which::Other);
        assert_eq!(
            key_attributes("sha256:00").get(ATTR_KIND),
            Some(&KIND_TLS_KEY)
        );
    }
}
