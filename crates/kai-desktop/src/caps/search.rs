//! The Search capability (K23): `find_files` and `recent_files` (WP15). On Plasma the file index
//! is Baloo's (`src/desktop/plasma/baloo.rs`); the recently used files are the freedesktop list
//! (`src/desktop/fdo/recent.rs`), which every provider reads.

use super::Fut;

/// What a file search found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Found {
    /// The files' paths, in the index's order.
    pub paths: Vec<String>,
    /// The index holds more matches than the limit asked for.
    pub more: bool,
    /// What the result must say about the index (off, stale, only some folders): one short
    /// sentence each.
    pub notes: Vec<String>,
}

/// One recently used file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Recent {
    /// A path for a local file, else the URL.
    pub location: String,
    /// When it was last used (modified or visited), as the list writes it (ISO 8601, UTC).
    pub used: String,
    /// The apps that used it, by name.
    pub apps: Vec<String>,
}

/// The recently used files, and how many entries the list marks private (skipped, said).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Recents {
    pub entries: Vec<Recent>,
    /// Entries marked `<bookmark:private/>` by their writer: not listed.
    pub private_skipped: usize,
}

pub trait Search: Send + Sync {
    /// This provider has a file index (`find_files` is listed).
    fn indexes(&self) -> bool;

    /// The files matching `query`, at most `limit` of them (0: all the index has).
    fn find<'a>(&'a self, query: &'a str, limit: usize) -> Fut<'a, Found>;

    /// The recently used files, newest first.
    fn recent(&self) -> Fut<'_, Recents>;
}
