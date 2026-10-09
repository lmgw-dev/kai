//! Search on Plasma: Baloo's file index for `find_files`, the freedesktop recent list for
//! `recent_files` (`fdo::recent`).
//!
//! - **The query** runs `baloosearch6` (WP13 (g): it reads the index directly; the baloo runner's
//!   D-Bus `Match` silently stops at 10 results, so it is not used), the query as one argument
//!   after `--`, never through a shell, with `-l` one past `search.limit` so the result can say
//!   that the index has more. Its output is one path per line (no NUL mode: see `parse_output`) and an `Elapsed:` line. The run is
//!   bounded by `dbus.timeout_s` (it takes about 15 ms) and killed when the call ends.
//! - **The index's state** is read here, since `baloosearch6` answers from a disabled or stale
//!   index without saying so: `baloofilerc` (`Indexing-Enabled`, `only basic indexing`, the
//!   indexed and excluded folders) and whether the indexer (`org.kde.baloo`) runs. The result
//!   always names the indexed folders, and says when indexing is off, the indexer is not running or
//!   only file names are indexed.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use kai_desktop::caps::search::{Found, Recents, Search};
use kai_desktop::caps::{Failure, Fut};

use crate::desktop::fdo::recent::RecentList;
use crate::desktop::has_owner;
use crate::host::registry::SessionBus;

pub struct Baloo {
    /// `baloosearch6`.
    pub program: PathBuf,
    /// `$XDG_CONFIG_HOME` (`baloofilerc`).
    pub config_home: PathBuf,
    /// `$HOME`, for `$HOME` in the folders and their default.
    pub home: PathBuf,
    pub bus: SessionBus,
    pub timeout: Duration,
    pub recent: RecentList,
}

impl Search for Baloo {
    fn indexes(&self) -> bool {
        true
    }

    fn find<'a>(&'a self, query: &'a str, limit: usize) -> Fut<'a, Found> {
        Box::pin(async move {
            let (paths, more, found_notes) = self.search(query, limit).await?;
            let mut notes = found_notes;
            notes.extend(index_notes(&self.config_home, &self.home));
            if !self.indexer_runs().await {
                notes.push(
                    "Baloo's indexer is not running: files changed since it stopped are missing \
                     or stale"
                        .into(),
                );
            }
            Ok(Found { paths, more, notes })
        })
    }

    fn recent(&self) -> Fut<'_, Recents> {
        Box::pin(self.recent.read())
    }
}

impl Baloo {
    async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<(Vec<String>, bool, Vec<String>), Failure> {
        let mut cmd = tokio::process::Command::new(&self.program);
        // One past the limit says "more"; a limit too big to add one to is no limit.
        let limit = match limit.checked_add(1) {
            Some(past) if limit > 0 => {
                cmd.arg("-l").arg(past.to_string());
                limit
            }
            _ => 0,
        };
        cmd.arg("--")
            .arg(query)
            .env("LC_ALL", "C.UTF-8")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let secs = self.timeout.as_secs();
        let out = match tokio::time::timeout(self.timeout, cmd.output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return Err(Failure::Failed(format!(
                    "cannot run {}: {e}",
                    self.program.display()
                )));
            }
            Err(_) => {
                return Err(Failure::Failed(format!(
                    "baloosearch6 did not answer within {secs} s (dbus.timeout_s)"
                )));
            }
        };
        if !out.status.success() {
            return Err(Failure::Failed(format!(
                "baloosearch6 failed ({})",
                out.status
            )));
        }
        let (mut paths, non_utf8) = parse_output(&out.stdout);
        let more = limit > 0 && paths.len() > limit;
        paths.truncate(if limit > 0 { limit } else { paths.len() });
        let mut notes = Vec::new();
        if non_utf8 > 0 {
            notes.push(format!(
                "{non_utf8} paths are not valid UTF-8; their invalid bytes are shown as \\xNN \
                 escapes, so those paths are not usable as they stand"
            ));
        }
        Ok((paths, more, notes))
    }

    async fn indexer_runs(&self) -> bool {
        match self.bus.get().await {
            Ok(bus) => has_owner(&bus, "org.kde.baloo", self.timeout)
                .await
                .unwrap_or(false),
            Err(_) => false,
        }
    }
}

/// `baloosearch6`'s output as paths, and how many of them held bytes that are not UTF-8 (shown
/// with `\xNN` escapes). The program has no NUL-separated mode, only lines: every path is
/// absolute, so a line that does not start with `/` is the rest of a path with a newline in it and
/// is joined to the one before. (A newline followed by `/` inside a path cannot be told from two
/// paths; that is the program's output, not something Kai can recover.)
fn parse_output(stdout: &[u8]) -> (Vec<String>, usize) {
    let mut paths: Vec<String> = Vec::new();
    let mut non_utf8 = 0;
    for line in stdout.split(|b| *b == b'\n') {
        if line.is_empty() || line.starts_with(b"Elapsed:") {
            continue;
        }
        let text = match std::str::from_utf8(line) {
            Ok(t) => t.to_owned(),
            Err(_) => {
                non_utf8 += 1;
                line.utf8_chunks()
                    .map(|c| {
                        let mut s = c.valid().to_owned();
                        for b in c.invalid() {
                            s.push_str(&format!("\\x{b:02X}"));
                        }
                        s
                    })
                    .collect()
            }
        };
        match paths.last_mut() {
            Some(last) if !text.starts_with('/') => {
                last.push('\n');
                last.push_str(&text);
            }
            _ => paths.push(text),
        }
    }
    (paths, non_utf8)
}

/// `baloofilerc`'s keys Kai reads, as Baloo's own configuration reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexConfig {
    pub enabled: bool,
    pub basic_only: bool,
    pub folders: Vec<String>,
    pub excluded: Vec<String>,
}

/// `baloofilerc` in `config_home`; its defaults where it or a key is missing (indexing on, the home
/// directory, content indexed).
pub fn index_config(config_home: &Path, home: &Path) -> IndexConfig {
    let text = std::fs::read_to_string(config_home.join("baloofilerc")).unwrap_or_default();
    let mut section = String::new();
    let mut c = IndexConfig {
        enabled: true,
        basic_only: false,
        folders: vec![home.display().to_string()],
        excluded: Vec::new(),
    };
    let home = home.display().to_string();
    let list = |v: &str| -> Vec<String> {
        v.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                let s = s.replace("$HOME", &home);
                if s.len() > 1 {
                    s.trim_end_matches('/').to_owned()
                } else {
                    s
                }
            })
            .collect()
    };
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        // `folders[$e]`: KConfig's expansion marker.
        let key = key.trim().trim_end_matches("[$e]").trim();
        let value = value.trim();
        let yes = value.eq_ignore_ascii_case("true");
        match (section.as_str(), key) {
            ("Basic Settings", "Indexing-Enabled") => c.enabled = yes,
            ("General", "only basic indexing") => c.basic_only = yes,
            ("General", "folders") => c.folders = list(value),
            ("General", "exclude folders") => c.excluded = list(value),
            _ => {}
        }
    }
    c
}

/// What `find_files` says about the index (module doc).
pub fn index_notes(config_home: &Path, home: &Path) -> Vec<String> {
    let c = index_config(config_home, home);
    let mut notes = Vec::new();
    if !c.enabled {
        notes.push(
            "Baloo's file indexing is off (System Settings, File Search): these results come from \
             the index as it was when it was on"
                .into(),
        );
    }
    if c.basic_only {
        notes.push("only file names are indexed, not their content".into());
    }
    let shown = |v: &[String]| {
        v.iter()
            .map(|f| crate::desktop::resolve::shown_text(f).into_owned())
            .collect::<Vec<_>>()
            .join(", ")
    };
    notes.push(format!("indexed folders: {}", shown(&c.folders)));
    if !c.excluded.is_empty() {
        notes.push(format!("excluded: {}", shown(&c.excluded)));
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_keeps_odd_paths_whole_or_says_what_it_escaped() {
        let (p, bad) = parse_output(b"/a/b\n/a/with\nnewline\nElapsed: 3 ms\n/a/\xff\xfeend\n");
        assert_eq!(p, ["/a/b", "/a/with\nnewline", "/a/\\xFF\\xFEend"]);
        assert_eq!(bad, 1);
    }

    #[test]
    fn baloofilerc_reads_as_baloo_reads_it() {
        let dir = crate::desktop::tests::temp_dir();
        let home = Path::new("/home/u");
        assert_eq!(
            index_config(dir.path(), home),
            IndexConfig {
                enabled: true,
                basic_only: false,
                folders: vec!["/home/u".into()],
                excluded: vec![],
            },
            "no file: Baloo's defaults"
        );
        std::fs::write(
            dir.path().join("baloofilerc"),
            "[Basic Settings]\nIndexing-Enabled=false\n\n[General]\n\
             folders[$e]=$HOME/docs/,/data/\nexclude folders[$e]=$HOME/docs/tmp/\n\
             only basic indexing=true\n",
        )
        .unwrap();
        assert_eq!(
            index_config(dir.path(), home),
            IndexConfig {
                enabled: false,
                basic_only: true,
                folders: vec!["/home/u/docs".into(), "/data".into()],
                excluded: vec!["/home/u/docs/tmp".into()],
            }
        );
        assert_eq!(
            index_notes(dir.path(), home),
            [
                "Baloo's file indexing is off (System Settings, File Search): these results come \
                 from the index as it was when it was on",
                "only file names are indexed, not their content",
                "indexed folders: /home/u/docs, /data",
                "excluded: /home/u/docs/tmp",
            ]
        );
    }
}
