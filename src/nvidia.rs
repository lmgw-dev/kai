//! The NVIDIA rule (§2): WebKitGTK on NVIDIA under Wayland crashes with "Error 71 (Protocol error)"
//! under the default DMABuf renderer unless `__NV_DISABLE_EXPLICIT_SYNC=1` is set. That variable
//! avoids the crash and keeps zero-copy GPU rendering.
//!
//! Kai never sets `WEBKIT_DISABLE_DMABUF_RENDERER` (software rendering),
//! `WEBKIT_DMABUF_RENDERER_FORCE_SHM` (a GPU readback per frame) or `GDK_BACKEND=x11` (XWayland,
//! mis-renders on NVIDIA).

use std::ffi::OsString;
use std::path::Path;

pub const VAR: &str = "__NV_DISABLE_EXPLICIT_SYNC";

/// What [`apply`] did, for the log line written once tracing is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The variable was already in the environment; an explicit value always wins.
    KeptExplicit,
    /// NVIDIA is present and the variable was unset: set to `1`.
    Set,
    /// No NVIDIA driver: nothing to do.
    NotNvidia,
}

/// The pure decision: `current` is the variable's value now, `nvidia` whether the driver is present.
pub fn decide(current: Option<&OsString>, nvidia: bool) -> Decision {
    match (current, nvidia) {
        (Some(_), _) => Decision::KeptExplicit,
        (None, true) => Decision::Set,
        (None, false) => Decision::NotNvidia,
    }
}

/// The driver is present when `/dev/nvidiactl` or `/sys/module/nvidia` exists.
pub fn nvidia_present() -> bool {
    Path::new("/dev/nvidiactl").exists() || Path::new("/sys/module/nvidia").exists()
}

/// Applies the rule to the process environment. Must run first in `main()`, before GTK and before
/// any thread exists (setting a variable is only sound while the process is single-threaded).
pub fn apply() -> Decision {
    let decision = decide(std::env::var_os(VAR).as_ref(), nvidia_present());
    if decision == Decision::Set {
        // SAFETY: called as the first statement of main(), before any other thread is started.
        unsafe { std::env::set_var(VAR, "1") };
    }
    decision
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_without_the_variable_sets_it() {
        assert_eq!(decide(None, true), Decision::Set);
    }

    #[test]
    fn an_explicit_value_always_wins() {
        for value in ["0", "1", ""] {
            let v = OsString::from(value);
            assert_eq!(
                decide(Some(&v), true),
                Decision::KeptExplicit,
                "value {value:?}"
            );
            assert_eq!(
                decide(Some(&v), false),
                Decision::KeptExplicit,
                "value {value:?}"
            );
        }
    }

    #[test]
    fn no_nvidia_does_nothing() {
        assert_eq!(decide(None, false), Decision::NotNvidia);
    }
}
