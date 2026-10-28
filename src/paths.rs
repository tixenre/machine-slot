//! Where things live. Following the XDG base directories:
//! - state (queue, locks): `$MACHINE_SLOT_STATE`, else `$XDG_STATE_HOME/machine-slot`,
//!   else `~/.local/state/machine-slot`;
//! - data (the record, times, log): `$MACHINE_SLOT_DIR`, else `data_dir` in the
//!   config file, else `$XDG_DATA_HOME/machine-slot`, else `~/.local/share/machine-slot`;
//! - config: `$XDG_CONFIG_HOME/machine-slot/config`, else `~/.config/machine-slot/config`,
//!   lines of `key = value` (`#` starts a comment).

use std::path::PathBuf;

pub fn home() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h),
        _ => {
            eprintln!("machine-slot: HOME is not set");
            std::process::exit(2);
        }
    }
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(fallback))
}

/// `~/x` and `$HOME/x` in config values.
fn expand(v: &str) -> PathBuf {
    if let Some(rest) = v.strip_prefix("~/").or_else(|| v.strip_prefix("$HOME/")) {
        home().join(rest)
    } else if v == "~" {
        home()
    } else {
        PathBuf::from(v)
    }
}

pub fn config_file() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("machine-slot/config")
}

pub fn config(key: &str) -> Option<String> {
    let text = std::fs::read_to_string(config_file()).ok()?;
    parse_config(&text, key)
}

fn parse_config(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.split('#').next()?.trim();
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_string())
    })
}

pub fn state() -> PathBuf {
    std::env::var_os("MACHINE_SLOT_STATE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| xdg("XDG_STATE_HOME", ".local/state").join("machine-slot"))
}

pub fn data() -> PathBuf {
    if let Some(d) = std::env::var_os("MACHINE_SLOT_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(d) = config("data_dir") {
        return expand(&d);
    }
    xdg("XDG_DATA_HOME", ".local/share").join("machine-slot")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_lines() {
        let text = "# machine-slot\n data_dir = \"~/notes/slots\"  # mine\nother=1\n";
        assert_eq!(
            parse_config(text, "data_dir").as_deref(),
            Some("~/notes/slots")
        );
        assert_eq!(parse_config(text, "other").as_deref(), Some("1"));
        assert_eq!(parse_config(text, "missing"), None);
    }
}
