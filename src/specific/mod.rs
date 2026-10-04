//! Helpers shared by groups of rules, port of `thefuck/specific/*`.

pub mod apt;
pub mod archlinux;
pub mod brew;
pub mod git;
pub mod npm;
pub mod sudo;

use crate::utils::which;

pub fn dnf_available() -> bool {
    which("dnf").is_some()
}

pub fn nix_available() -> bool {
    which("nix").is_some()
}

pub fn yum_available() -> bool {
    which("yum").is_some()
}
