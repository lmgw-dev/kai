//! Where Kai's files live (XDG base directories).

use std::path::{Path, PathBuf};

/// The files Kai reads and writes. Built from the environment once; tests build it from a temp dir.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `$XDG_CONFIG_HOME/kai/config.toml`
    pub config_file: PathBuf,
    /// `$XDG_CONFIG_HOME/kai/device-link.json`: the pairing's public data (`crate::pairing::Paired`).
    pub device_link_file: PathBuf,
    /// `$XDG_CONFIG_HOME/autostart/dev.rootshell.kai.desktop` ("Start with Plasma", §13)
    pub autostart_file: PathBuf,
    /// `$XDG_STATE_HOME/kai`: Kai's state (the AEC record, the portal's restore token).
    pub state_dir: PathBuf,
    /// `$XDG_STATE_HOME/kai/aec.json`: the default devices app AEC flipped (K20).
    pub aec_file: PathBuf,
    /// `$XDG_RUNTIME_DIR/kai`: the echo-cancel child's generated configuration.
    pub runtime_dir: PathBuf,
}

impl Paths {
    pub fn from_env() -> Result<Self, String> {
        let config_home = xdg_dir("XDG_CONFIG_HOME", ".config")?;
        let state = xdg_dir("XDG_STATE_HOME", ".local/state")?.join("kai");
        Ok(Self {
            runtime_dir: crate::audio::runtime_dir_for(&state),
            aec_file: state.join("aec.json"),
            state_dir: state,
            ..Self::under_config_home(&config_home)
        })
    }

    /// Everything under one directory (the tests).
    pub fn under_config_home(config_home: &Path) -> Self {
        Self {
            config_file: config_home.join("kai").join("config.toml"),
            device_link_file: config_home.join("kai").join("device-link.json"),
            autostart_file: config_home
                .join("autostart")
                .join(format!("{}.desktop", crate::APP_ID)),
            aec_file: config_home.join("state").join("kai").join("aec.json"),
            state_dir: config_home.join("state").join("kai"),
            runtime_dir: config_home.join("runtime").join("kai"),
        }
    }
}

/// The routines' CLI socket (K28): `$KAI_CLI_SOCKET` when it is an absolute path (the tests' and
/// the live checks' own, never the running Kai's), else `cli.sock` in Kai's runtime folder
/// (`$XDG_RUNTIME_DIR/kai`). A routine's script gets the path in `KAI_CLI_SOCKET`.
pub fn cli_socket(runtime_dir: &Path) -> PathBuf {
    std::env::var_os("KAI_CLI_SOCKET")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| runtime_dir.join("cli.sock"))
}

/// Kai for Firefox's socket (kai-firefox design §4.4): `$KAI_BROWSER_SOCKET` when it is an absolute
/// path (the tests' and the live checks' own, never the running Kai's), else `browser.sock` in
/// Kai's runtime folder (`$XDG_RUNTIME_DIR/kai`), where `kai-browser-host` looks for it too.
pub fn browser_socket(runtime_dir: &Path) -> PathBuf {
    std::env::var_os("KAI_BROWSER_SOCKET")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| runtime_dir.join("browser.sock"))
}

/// `$XDG_DATA_HOME/kai/routines` (K28): one folder per routine.
#[cfg_attr(test, allow(dead_code))]
pub fn routines_dir() -> Result<PathBuf, String> {
    Ok(xdg_dir("XDG_DATA_HOME", ".local/share")?
        .join("kai")
        .join("routines"))
}

/// Makes Kai's runtime folder `dir` private: created mode 0700 where it is missing (its missing
/// parents too), and one that exists and is this user's real directory made 0700 (an older Kai's
/// echo-cancel child could have made it with the umask's mode). Another owner's folder or a symlink
/// is left as it is; what is put in it then says so when it fails.
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: getuid has no preconditions and cannot fail.
    let me = unsafe { libc::getuid() };
    if meta.file_type().is_dir() && meta.uid() == me && meta.mode() & 0o777 != 0o700 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// `$VAR` when it is set to an absolute path (the XDG spec ignores relative values), else
/// `$HOME/<fallback>`.
fn xdg_dir(var: &str, fallback: &str) -> Result<PathBuf, String> {
    if let Some(v) = std::env::var_os(var) {
        let p = PathBuf::from(v);
        if p.is_absolute() {
            return Ok(p);
        }
    }
    let home = std::env::var_os("HOME").ok_or_else(|| format!("neither {var} nor HOME is set"))?;
    Ok(PathBuf::from(home).join(fallback))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_runtime_folder_is_made_private() {
        let tmp = crate::desktop::tests::bus::temp_dir();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let made = tmp.path().join("run/kai");
        super::private_dir(&made).unwrap();
        assert_eq!(mode(&made), 0o700);
        assert_eq!(mode(made.parent().unwrap()), 0o700, "a missing parent too");
        // One made open before (an older Kai's create_dir_all) is tightened.
        std::fs::set_permissions(&made, std::fs::Permissions::from_mode(0o755)).unwrap();
        super::private_dir(&made).unwrap();
        assert_eq!(mode(&made), 0o700);
        // A symlink is not followed.
        let target = tmp.path().join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        super::private_dir(&link).unwrap();
        assert_eq!(mode(&target), 0o755);
    }
}
