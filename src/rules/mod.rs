//! Built-in correction rules, grouped by the tools they support.
//!
//! The registry keeps the Python rule names and metadata so existing rule
//! selections, exclusions, and priority overrides continue to work.

use crate::types::{Command, Rule};
use std::sync::LazyLock;

macro_rules! sudo_rule {
    ($name:literal, $matches:expr, $fix:expr) => {
        Rule::new(
            $name,
            |command| crate::specific::sudo::sudo_support_match(command, $matches),
            |command| crate::specific::sudo::sudo_support(command, $fix),
        )
    };
}

macro_rules! git_rule {
    ($name:literal, $matches:expr, $fix:expr) => {
        Rule::new(
            $name,
            |command| crate::specific::git::git_support(command, $matches),
            |command| crate::specific::git::git_support(command, $fix),
        )
    };
}

mod filesystem;
mod general;
mod git;
mod language;
mod package_managers;
mod system;
mod tools;

pub static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    [
        general::RULES,
        filesystem::RULES,
        git::RULES,
        package_managers::RULES,
        tools::RULES,
        system::RULES,
        language::RULES,
    ]
    .into_iter()
    .flat_map(|rules| rules.iter().copied())
    .collect()
});

fn one(script: impl Into<String>) -> Vec<String> {
    vec![script.into()]
}

fn output_contains(command: &Command, patterns: &[&str]) -> bool {
    patterns
        .iter()
        .any(|pattern| command.output().contains(pattern))
}

fn output_contains_lower(command: &Command, patterns: &[&str]) -> bool {
    patterns
        .iter()
        .any(|pattern| command.output_lower().contains(pattern))
}
