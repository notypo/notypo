use crate::utils::{run_stdout, which};
use std::sync::OnceLock;

pub fn brew_available() -> bool {
    which("brew").is_some()
}

/// `brew --prefix`, memoized.
pub fn get_brew_path_prefix() -> Option<&'static str> {
    static PREFIX: OnceLock<Option<String>> = OnceLock::new();
    PREFIX
        .get_or_init(|| {
            run_stdout("brew", &["--prefix"])
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        })
        .as_deref()
}
