//! Where Kai's files live (XDG base directories).

use std::path::{Path, PathBuf};

/// The files Kai reads and writes. Built from the environment once; tests build it from a temp dir.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `$XDG_CONFIG_HOME/kai/config.toml`
    pub config_file: PathBuf,
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

/// `$XDG_DATA_HOME/kai/routines` (K28): one folder per routine.
#[cfg_attr(test, allow(dead_code))]
pub fn routines_dir() -> Result<PathBuf, String> {
    Ok(xdg_dir("XDG_DATA_HOME", ".local/share")?
        .join("kai")
        .join("routines"))
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
