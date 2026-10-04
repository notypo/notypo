//! Portable regression cases translated from the upstream rule tests.

use notypo::rules::RULES;
use notypo::settings::Settings;
use notypo::shells::Shell;
use notypo::types::{Command, Context};
use std::collections::HashSet;

enum Expectation {
    Matches(bool),
    Commands(&'static [&'static str]),
}

struct Case {
    rule: &'static str,
    script: &'static str,
    output: &'static str,
    expected: Expectation,
}

const CASES: &[Case] = &include!("data/rules.rs");

#[test]
fn upstream_rule_regressions() {
    let ctx =
        Context::new(Settings::default(), Shell::Bash, "fuck".into()).with_history::<&str>(&[]);
    let mut failures = Vec::new();
    for (index, case) in CASES.iter().enumerate() {
        let rule = RULES
            .iter()
            .find(|rule| rule.name == case.rule)
            .expect("registered rule");
        let command = Command::new(case.script, Some(case.output), &ctx);
        match case.expected {
            Expectation::Matches(expected) => {
                let actual = (rule.matches)(&command);
                if actual != expected {
                    failures.push(format!(
                        "case {index}: {} {:?}: match returned {actual}, expected {expected}",
                        case.rule, case.script
                    ));
                }
            }
            Expectation::Commands(expected) => {
                let actual = (rule.get_new_command)(&command);
                if actual != expected {
                    failures.push(format!(
                        "case {index}: {} {:?}: commands {actual:?}, expected {expected:?}",
                        case.rule, case.script
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn registry_is_complete_and_rule_names_are_unique() {
    assert_eq!(RULES.len(), 169);
    let names: HashSet<_> = RULES.iter().map(|rule| rule.name).collect();
    assert_eq!(names.len(), RULES.len());
    for name in ["rm_root", "git_push_force"] {
        let rule = RULES.iter().find(|rule| rule.name == name).unwrap();
        assert!(!(rule.enabled_by_default)());
    }
    for name in [
        "wrong_hyphen_before_subcommand",
        "dirty_unzip",
        "git_hook_bypass",
    ] {
        assert!(
            !RULES
                .iter()
                .find(|rule| rule.name == name)
                .unwrap()
                .requires_output
        );
    }
}

#[test]
fn malformed_commands_do_not_panic_in_rules() {
    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into())
        .with_executables(&["git", "ls", "cargo"])
        .with_history::<&str>(&[]);
    for script in [
        "", " ", "'", "sudo", "git", "git ''", "sudo ''", "λ", "\u{a0}",
    ] {
        let command = Command::new(script, Some(""), &ctx);
        for rule in RULES.iter() {
            if rule.is_match(&command) {
                let _ = (rule.get_new_command)(&command);
            }
        }
    }
}
