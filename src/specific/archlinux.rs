//! Arch Linux helpers: `pkgfile` lookups and the preferred pacman wrapper.

use crate::utils::which;
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};

/// Packages providing `command`'s executable, via `pkgfile -b -v` (memoized).
pub fn get_pkgfile(command: &str) -> Vec<String> {
    static MEMO: LazyLock<Mutex<HashMap<String, Vec<String>>>> = LazyLock::new(Default::default);
    if let Some(hit) = MEMO.lock().ok().and_then(|m| m.get(command).cloned()) {
        return hit;
    }
    let command_trimmed = command.trim();
    let command_trimmed = command_trimmed
        .strip_prefix("sudo ")
        .unwrap_or(command_trimmed);
    let executable = command_trimmed.split(' ').next().unwrap_or_default();
    let packages: Vec<String> = Command::new("pkgfile")
        .args(["-b", "-v", executable])
        .stderr(Stdio::null())
        .output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| l.split_whitespace().next())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if let Ok(mut m) = MEMO.lock() {
        m.insert(command.to_owned(), packages.clone());
    }
    packages
}

/// `archlinux_env()`: (enabled by default, pacman command).
pub fn archlinux_env() -> (bool, Option<&'static str>) {
    let pacman = if which("yay").is_some() {
        "yay"
    } else if which("pikaur").is_some() {
        "pikaur"
    } else if which("yaourt").is_some() {
        "yaourt"
    } else if which("pacman").is_some() {
        "sudo pacman"
    } else {
        return (false, None);
    };
    (which("pkgfile").is_some(), Some(pacman))
}
