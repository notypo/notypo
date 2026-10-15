//! Arch Linux helpers: `pkgfile` lookups and the preferred pacman wrapper.

use crate::utils::which;
use crate::{engine::legacy, types::Command};

/// Packages providing `command`'s executable, via bounded `pkgfile -b -v`.
pub fn get_pkgfile(context: &Command, command: &str) -> Vec<String> {
    let command_trimmed = command.trim();
    let command_trimmed = command_trimmed
        .strip_prefix("sudo ")
        .unwrap_or(command_trimmed);
    let executable = command_trimmed.split(' ').next().unwrap_or_default();
    legacy::stdout(context, "pkgfile", &["-b", "-v", executable])
        .map(|out| {
            out.lines()
                .filter_map(|l| l.split_whitespace().next())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
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
