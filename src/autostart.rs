//! "Start with Plasma" (§13): writes or removes `$XDG_CONFIG_HOME/autostart/dev.rootshell.kai.desktop`.
//! Plasma runs it as `app-dev.rootshell.kai@autostart.service`, which gives the portal Kai's app id
//! (K12, WP0 (g)). Switched on after the first pairing.

use std::path::Path;

pub fn is_on(file: &Path) -> bool {
    file.exists()
}

/// Switches the entry on or off. `exe` is the binary the entry starts (this one).
pub fn set(file: &Path, exe: &Path, on: bool) -> Result<(), String> {
    let err = |what: &str, e: std::io::Error| format!("cannot {what} {}: {e}", file.display());
    if on {
        let dir = file
            .parent()
            .ok_or_else(|| format!("{} has no directory", file.display()))?;
        std::fs::create_dir_all(dir).map_err(|e| err("create the directory of", e))?;
        std::fs::write(file, entry(exe)).map_err(|e| err("write", e))
    } else {
        match std::fs::remove_file(file) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(err("remove", e)),
        }
    }
}

fn entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Kai\nComment=Kai, the desktop client of lmgw\n\
         Exec={}\nIcon={}\nTerminal=false\n",
        exec_quote(&exe.to_string_lossy()),
        crate::APP_ID
    )
}

/// Quotes a program path for an `Exec` key (Desktop Entry spec, "The Exec key"): reserved
/// characters need double quotes, inside which `"`, `` ` ``, `$` and `\` are backslash-escaped;
/// the value is then a string, so each backslash is doubled again; `%` is written `%%`.
pub fn exec_quote(path: &str) -> String {
    const RESERVED: &[char] = &[
        ' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(',
        ')', '`',
    ];
    let path = path.replace('%', "%%");
    if !path.contains(RESERVED) {
        return path;
    }
    let mut quoted = String::from("\"");
    for c in path.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted.replace('\\', "\\\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_writes_the_entry_and_off_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir
            .path()
            .join("autostart")
            .join("dev.rootshell.kai.desktop");
        assert!(!is_on(&file));
        set(&file, Path::new("/usr/bin/kai"), true).unwrap();
        assert!(is_on(&file));
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("[Desktop Entry]\n"), "{text}");
        assert!(
            text.contains("\nExec=/usr/bin/kai\n") && text.contains("\nIcon=dev.rootshell.kai\n"),
            "{text}"
        );
        set(&file, Path::new("/usr/bin/kai"), false).unwrap();
        assert!(!is_on(&file));
        set(&file, Path::new("/usr/bin/kai"), false).unwrap();
    }

    #[test]
    fn exec_paths_are_quoted() {
        assert_eq!(exec_quote("/usr/bin/kai"), "/usr/bin/kai");
        assert_eq!(exec_quote("/home/a b/kai"), "\"/home/a b/kai\"");
        assert_eq!(exec_quote("/x/$y/kai"), "\"/x/\\\\$y/kai\"");
        assert_eq!(exec_quote("/x/100%/kai"), "/x/100%%/kai");
    }
}
