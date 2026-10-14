use crate::engine::legacy;
use crate::types::Command;
use crate::utils::which;

pub fn brew_available() -> bool {
    which("brew").is_some()
}

/// `brew --prefix`, under the correction request's trust policy and budget.
pub fn get_brew_path_prefix(command: &Command) -> Option<String> {
    legacy::stdout(command, "brew", &["--prefix"])
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}
