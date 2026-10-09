//! The device key in the Secret Service (§2), served on Plasma by `ksecretd` (WP0 (h)).
//!
//! One item per lmgw base URL, attributes `{application: dev.rootshell.kai, lmgw: <base url>,
//! kind: device-key}`, in the default collection. Load and delete look only there and take only
//! that kind of item (review KAI-2 K2-16): the Secret Service's search matches every item that
//! carries *at least* the given attributes, in every collection, so a later item of Kai's for the
//! same gateway, or a copy the owner made in another wallet folder, would match too. An item from
//! a build before the `kind` attribute (exactly application and lmgw) is read and deleted as the
//! key, and goes when the key for its URL is stored again. Kai never locks or unlocks the wallet: it reads the collection's `Locked`
//! property first, and on a locked wallet it reports [`Lookup::Locked`] / [`Stored::Locked`] without
//! searching or writing, so no unlock prompt can appear. The core shows "wallet locked: unlock
//! KWallet" and checks again every `wallet.retry_s` (§2: "Kai retries when the wallet opens").

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use secret_service::{EncryptionType, SecretService};
use tokio::sync::Mutex;

use crate::pairing::DeviceKey;

pub const ATTR_APPLICATION: &str = "application";
pub const ATTR_LMGW: &str = "lmgw";
pub const ATTR_KIND: &str = "kind";
/// The `kind` of the device key's item.
pub const KIND_DEVICE_KEY: &str = "device-key";

/// The result of reading the key for one base URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Found(DeviceKey),
    NotFound,
    Locked,
}

/// The result of storing a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stored {
    Stored,
    Locked,
}

/// The result of deleting Kai's items for one base URL.
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

/// Where the key lives. [`SecretServiceStore`] in Kai; [`MemoryStore`] in tests.
pub trait SecretStore: Send + Sync + 'static {
    /// Whether the default collection is locked (read without asking for an unlock).
    fn locked(&self) -> impl Future<Output = Result<bool, SecretError>> + Send;
    fn load(&self, url: &str) -> impl Future<Output = Result<Lookup, SecretError>> + Send;
    fn store(
        &self,
        url: &str,
        device_name: &str,
        key: &DeviceKey,
    ) -> impl Future<Output = Result<Stored, SecretError>> + Send;
    /// Deletes Kai's own device-key items for `url` in the default collection (attributes
    /// `application`, `lmgw` and `kind: device-key`), and nothing else.
    fn delete(&self, url: &str) -> impl Future<Output = Result<Removed, SecretError>> + Send;
}

/// What a search for `url`'s items asks (any kind; [`which`] sorts them out).
fn attributes(url: &str) -> HashMap<&str, &str> {
    HashMap::from([(ATTR_APPLICATION, crate::APP_ID), (ATTR_LMGW, url)])
}

/// What Kai's device-key item for `url` carries.
fn key_attributes(url: &str) -> HashMap<&str, &str> {
    let mut a = attributes(url);
    a.insert(ATTR_KIND, KIND_DEVICE_KEY);
    a
}

/// What an item found for a URL is to Kai, by its attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    /// Kai's device key (`kind: device-key`).
    Key,
    /// An item of a build before `kind`: only `application` and `lmgw` (and the Secret Service's
    /// own `xdg:` attributes). Read and deleted as the key.
    Legacy,
    /// Anything else (another kind, or more attributes): not the key.
    Other,
}

fn which(attrs: &HashMap<String, String>) -> Which {
    match attrs.get(ATTR_KIND).map(String::as_str) {
        Some(KIND_DEVICE_KEY) => Which::Key,
        Some(_) => Which::Other,
        None if attrs
            .keys()
            .all(|k| k == ATTR_APPLICATION || k == ATTR_LMGW || k.starts_with("xdg:")) =>
        {
            Which::Legacy
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
        self.bounded("whether the wallet is locked", async {
            let mut guard = self.session_guard().await?;
            let result = locked_with(guard.as_ref().expect("opened")).await;
            if result.is_err() {
                *guard = None;
            }
            result
        })
        .await
    }

    async fn load(&self, url: &str) -> Result<Lookup, SecretError> {
        self.bounded("a lookup", async {
            let mut guard = self.session_guard().await?;
            let result = load_with(guard.as_ref().expect("opened"), url).await;
            if result.is_err() {
                *guard = None;
            }
            result
        })
        .await
    }

    async fn store(
        &self,
        url: &str,
        device_name: &str,
        key: &DeviceKey,
    ) -> Result<Stored, SecretError> {
        self.bounded("a store", async {
            let mut guard = self.session_guard().await?;
            let result = store_with(guard.as_ref().expect("opened"), url, device_name, key).await;
            if result.is_err() {
                *guard = None;
            }
            result
        })
        .await
    }

    async fn delete(&self, url: &str) -> Result<Removed, SecretError> {
        self.bounded("a delete", async {
            let mut guard = self.session_guard().await?;
            let result = delete_with(guard.as_ref().expect("opened"), url).await;
            if result.is_err() {
                *guard = None;
            }
            result
        })
        .await
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

async fn load_with(ss: &SecretService<'_>, url: &str) -> Result<Lookup, SecretError> {
    let collection = ss
        .get_default_collection()
        .await
        .map_err(ss_err("reading the default collection"))?;
    if collection
        .is_locked()
        .await
        .map_err(ss_err("reading whether the wallet is locked"))?
    {
        return Ok(Lookup::Locked);
    }
    let found = collection
        .search_items(attributes(url))
        .await
        .map_err(ss_err("searching Kai's item"))?;
    // Kai's own kind first, then an item of an earlier build.
    let mut keys = Vec::new();
    let mut legacy = Vec::new();
    for item in found {
        let attrs = item
            .get_attributes()
            .await
            .map_err(ss_err("reading Kai's item"))?;
        match which(&attrs) {
            Which::Key => keys.push(item),
            Which::Legacy => legacy.push(item),
            Which::Other => {}
        }
    }
    if keys.len() > 1 {
        tracing::warn!(
            count = keys.len(),
            "more than one Kai device-key item for this lmgw URL; using the first"
        );
    }
    match keys.first().or(legacy.first()) {
        Some(item) => {
            let bytes = item
                .get_secret()
                .await
                .map_err(ss_err("reading Kai's item"))?;
            let key = String::from_utf8(bytes)
                .map_err(|_| SecretError("Kai's item does not hold text".into()))?;
            Ok(Lookup::Found(DeviceKey::from_store(key)))
        }
        None => Ok(Lookup::NotFound),
    }
}

async fn store_with(
    ss: &SecretService<'_>,
    url: &str,
    device_name: &str,
    key: &DeviceKey,
) -> Result<Stored, SecretError> {
    let collection = ss
        .get_default_collection()
        .await
        .map_err(ss_err("reading the default collection"))?;
    // Checked first: create_item on a locked collection would raise an unlock prompt.
    if collection
        .is_locked()
        .await
        .map_err(ss_err("reading whether the wallet is locked"))?
    {
        return Ok(Stored::Locked);
    }
    let label = format!("Kai: lmgw device key '{device_name}' ({url})");
    let item = collection
        .create_item(
            &label,
            key_attributes(url),
            key.expose().as_bytes(),
            true,
            "text/plain",
        )
        .await
        .map_err(ss_err("storing Kai's item"))?;
    // An earlier build's item for the URL (no `kind`) is not replaced by the store: it goes now.
    let old = delete_keys(&collection, url, Some(&item.item_path)).await?;
    if old > 0 {
        tracing::info!(
            items = old,
            "an earlier item of Kai's for this URL was deleted"
        );
    }
    Ok(Stored::Stored)
}

async fn delete_with(ss: &SecretService<'_>, url: &str) -> Result<Removed, SecretError> {
    let collection = ss
        .get_default_collection()
        .await
        .map_err(ss_err("reading the default collection"))?;
    // Checked first: deleting from a locked collection would raise an unlock prompt.
    if collection
        .is_locked()
        .await
        .map_err(ss_err("reading whether the wallet is locked"))?
    {
        return Ok(Removed::Locked);
    }
    Ok(Removed::Items(delete_keys(&collection, url, None).await?))
}

/// Deletes `url`'s device-key items in `collection` (and an earlier build's, [`Which::Legacy`]),
/// all but the one at `keep`. The search matches supersets: each item's own attributes decide.
async fn delete_keys(
    collection: &secret_service::Collection<'_>,
    url: &str,
    keep: Option<&zbus::zvariant::OwnedObjectPath>,
) -> Result<usize, SecretError> {
    let found = collection
        .search_items(attributes(url))
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
        if matches!(which(&attrs), Which::Key | Which::Legacy) {
            item.delete().await.map_err(ss_err("deleting Kai's item"))?;
            n += 1;
        }
    }
    Ok(n)
}

/// An in-memory store for tests: a lock switch and a failure switch.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore {
    pub locked: std::sync::atomic::AtomicBool,
    pub fail: std::sync::Mutex<Option<String>>,
    pub items: std::sync::Mutex<HashMap<String, (String, DeviceKey)>>,
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

    async fn load(&self, url: &str) -> Result<Lookup, SecretError> {
        if self.check()? {
            return Ok(Lookup::Locked);
        }
        Ok(match self.items.lock().unwrap().get(url) {
            Some((_, key)) => Lookup::Found(key.clone()),
            None => Lookup::NotFound,
        })
    }

    async fn store(
        &self,
        url: &str,
        device_name: &str,
        key: &DeviceKey,
    ) -> Result<Stored, SecretError> {
        if self.check()? {
            return Ok(Stored::Locked);
        }
        self.items
            .lock()
            .unwrap()
            .insert(url.to_owned(), (device_name.to_owned(), key.clone()));
        Ok(Stored::Stored)
    }

    async fn delete(&self, url: &str) -> Result<Removed, SecretError> {
        if self.check()? {
            return Ok(Removed::Locked);
        }
        let gone = self.items.lock().unwrap().remove(url).is_some();
        Ok(Removed::Items(usize::from(gone)))
    }
}

impl<S: SecretStore> SecretStore for std::sync::Arc<S> {
    fn locked(&self) -> impl Future<Output = Result<bool, SecretError>> + Send {
        (**self).locked()
    }
    fn load(&self, url: &str) -> impl Future<Output = Result<Lookup, SecretError>> + Send {
        (**self).load(url)
    }
    fn store(
        &self,
        url: &str,
        device_name: &str,
        key: &DeviceKey,
    ) -> impl Future<Output = Result<Stored, SecretError>> + Send {
        (**self).store(url, device_name, key)
    }
    fn delete(&self, url: &str) -> impl Future<Output = Result<Removed, SecretError>> + Send {
        (**self).delete(url)
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

    /// Review KAI-2 K2-16: the search matches supersets; only Kai's device key, or an earlier
    /// build's item of exactly its shape, is taken for one.
    #[test]
    fn only_kais_own_key_item_counts() {
        let base = [("application", crate::APP_ID), ("lmgw", "http://h:1")];
        let with = |extra: &[(&str, &str)]| {
            let mut v = base.to_vec();
            v.extend_from_slice(extra);
            attrs(&v)
        };
        assert_eq!(which(&with(&[("kind", "device-key")])), Which::Key);
        assert_eq!(
            which(&with(&[("kind", "device-key"), ("xdg:schema", "x")])),
            Which::Key
        );
        assert_eq!(which(&with(&[])), Which::Legacy);
        assert_eq!(which(&with(&[("xdg:schema", "x")])), Which::Legacy);
        assert_eq!(which(&with(&[("kind", "memory-key")])), Which::Other);
        assert_eq!(which(&with(&[("folder", "copy")])), Which::Other);
        assert_eq!(
            key_attributes("http://h:1").get(ATTR_KIND),
            Some(&KIND_DEVICE_KEY)
        );
    }
}
