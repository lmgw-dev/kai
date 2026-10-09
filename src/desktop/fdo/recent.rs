//! The recently used files (`recent_files`): the freedesktop list `$XDG_DATA_HOME/recently-used.xbel`
//! (the desktop bookmark spec), which GTK apps and KDE's KRecentDocument both write. Every entry
//! the list holds, newest first: how many it keeps is the writers' business (their own settings),
//! Kai shows them all.
//!
//! The fdo provider's Search: it has no file index, so `find_files` is not listed where it serves
//! (the plasma provider's Search reads this list too).

use std::path::PathBuf;

use kai_desktop::caps::search::{Found, Recent, Recents, Search};
use kai_desktop::caps::{Failure, Fut};

/// The list's file.
#[derive(Debug, Clone)]
pub struct RecentList {
    pub file: PathBuf,
}

impl RecentList {
    pub fn in_data_home(data_home: &std::path::Path) -> Self {
        Self {
            file: data_home.join("recently-used.xbel"),
        }
    }

    /// The entries, newest first; no file is no entries.
    pub async fn read(&self) -> Result<Recents, Failure> {
        let file = self.file.clone();
        let text = tokio::task::spawn_blocking(move || std::fs::read_to_string(&file))
            .await
            .map_err(|e| Failure::Failed(format!("the recent files' read failed: {e}")))?;
        match text {
            Ok(t) => parse(&t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Recents::default()),
            Err(e) => Err(Failure::Failed(format!(
                "cannot read the recent files list ({}): {e}",
                self.file.display()
            ))),
        }
    }
}

const BOOKMARK_NS: &str = "http://www.freedesktop.org/standards/desktop-bookmarks";

/// The XBEL list's entries, newest first; entries their writer marked private
/// (`<bookmark:private/>`, "do not show to others") are left out and counted.
pub fn parse(text: &str) -> Result<Recents, Failure> {
    let doc = roxmltree::Document::parse(text).map_err(|e| {
        Failure::Failed(format!(
            "the recent files list is not valid XBEL ({e}); its writer may be mid-write"
        ))
    })?;
    let is_private = |b: &roxmltree::Node| {
        b.descendants()
            .any(|n| n.has_tag_name((BOOKMARK_NS, "private")))
    };
    let bookmarks: Vec<roxmltree::Node> = doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("bookmark"))
        .collect();
    let private_skipped = bookmarks.iter().filter(|b| is_private(b)).count();
    let mut out: Vec<Recent> = bookmarks
        .into_iter()
        .filter(|b| !is_private(b))
        .filter_map(|b| {
            let href = b.attribute("href")?;
            let used = [b.attribute("modified"), b.attribute("visited")]
                .into_iter()
                .flatten()
                .max()
                .or_else(|| b.attribute("added"))
                .unwrap_or_default()
                .to_owned();
            let apps = b
                .descendants()
                .filter(|n| n.has_tag_name((BOOKMARK_NS, "application")))
                .filter_map(|n| n.attribute("name"))
                .map(str::to_owned)
                .collect();
            Some(Recent {
                location: location(href),
                used,
                apps,
            })
        })
        .collect();
    // ISO 8601 times of one writer's form compare as text; the newest first.
    out.sort_by(|a, b| b.used.cmp(&a.used));
    Ok(Recents {
        entries: out,
        private_skipped,
    })
}

/// A `file:` URL as its path; any other URL as it is.
fn location(href: &str) -> String {
    match url::Url::parse(href) {
        Ok(u) if u.scheme() == "file" => u
            .to_file_path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| href.to_owned()),
        _ => href.to_owned(),
    }
}

/// The fdo Search: the list only.
pub struct FdoSearch {
    pub recent: RecentList,
}

impl Search for FdoSearch {
    fn indexes(&self) -> bool {
        false
    }

    fn find<'a>(&'a self, _query: &'a str, _limit: usize) -> Fut<'a, Found> {
        Box::pin(async {
            Err(Failure::Refused(
                "this desktop has no file index Kai can search".into(),
            ))
        })
    }

    fn recent(&self) -> Fut<'_, Recents> {
        Box::pin(self.recent.read())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XBEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xbel version="1.0"
      xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks"
      xmlns:mime="http://www.freedesktop.org/standards/shared-mime-info">
  <bookmark href="file:///home/u/old%20notes.txt" added="2026-10-01T08:00:00Z" modified="2026-10-01T08:00:00Z" visited="2026-10-02T09:30:12.123456Z">
    <info><metadata owner="http://freedesktop.org">
      <mime:mime-type type="text/plain"/>
      <bookmark:applications>
        <bookmark:application name="kate" exec="&apos;kate %u&apos;" modified="2026-10-02T09:30:12Z" count="2"/>
      </bookmark:applications>
    </metadata></info>
  </bookmark>
  <bookmark href="file:///home/u/Report%20&amp;%20Q3.pdf" added="2026-10-09T12:02:11Z" modified="2026-10-09T12:02:11.5Z" visited="2026-10-09T12:02:11Z">
    <info><metadata owner="http://freedesktop.org">
      <bookmark:applications>
        <bookmark:application name="okular" exec="&apos;okular %u&apos;" modified="2026-10-09T12:02:11Z" count="1"/>
        <bookmark:application name="Firefox" exec="&apos;firefox %u&apos;" modified="2026-10-08T12:02:11Z" count="1"/>
      </bookmark:applications>
    </metadata></info>
  </bookmark>
  <bookmark href="https://example.org/a" added="2026-10-05T00:00:00Z"/>
</xbel>"#;

    #[test]
    fn the_xbel_list_reads_newest_first_with_its_apps() {
        let r = parse(XBEL).unwrap().entries;
        let got: Vec<(&str, &str, Vec<&str>)> = r
            .iter()
            .map(|r| {
                (
                    r.location.as_str(),
                    r.used.as_str(),
                    r.apps.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (
                    "/home/u/Report & Q3.pdf",
                    "2026-10-09T12:02:11Z",
                    vec!["okular", "Firefox"]
                ),
                ("https://example.org/a", "2026-10-05T00:00:00Z", vec![]),
                (
                    "/home/u/old notes.txt",
                    "2026-10-02T09:30:12.123456Z",
                    vec!["kate"]
                ),
            ]
        );
        assert!(parse("<xbel><bookmark").is_err());
        assert!(parse("<xbel/>").unwrap().entries.is_empty());
    }

    #[test]
    fn private_entries_are_skipped_and_counted() {
        let xbel = r#"<xbel xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks">
  <bookmark href="file:///home/u/open.txt" modified="2026-10-01T08:00:00Z"/>
  <bookmark href="file:///home/u/secret.txt" modified="2026-10-02T08:00:00Z">
    <info><metadata owner="http://freedesktop.org"><bookmark:private/></metadata></info>
  </bookmark>
  <bookmark href="file:///home/u/secret2.txt" modified="2026-10-03T08:00:00Z">
    <info><metadata owner="http://freedesktop.org"><bookmark:private/></metadata></info>
  </bookmark>
</xbel>"#;
        let r = parse(xbel).unwrap();
        assert_eq!(r.private_skipped, 2);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].location, "/home/u/open.txt");
    }
}
