//! Port of `thefuck/corrector.py`.
//!
//! Like the Python generator chain this is lazy: rules are tried in
//! priority order and evaluation stops at the first one that matches. The
//! remaining rules only run if the user asks for other suggestions (arrow
//! keys), so `-y` or accepting the first fix never pays for them.

use crate::logs;
use crate::rules::RULES;
use crate::types::{Command, CorrectedCommand, Rule};

pub struct Corrector<'c, 'a> {
    command: &'c Command<'a>,
    rules: Vec<(&'static Rule, i64)>,
    next: usize,
    /// Remaining suggestions of the rule that produced the first one.
    leftover: Vec<CorrectedCommand>,
}

impl<'c, 'a> Corrector<'c, 'a> {
    pub fn new(command: &'c Command<'a>) -> Self {
        Self::with_rules(command, &RULES)
    }

    pub fn with_rules(command: &'c Command<'a>, rules: &'static [Rule]) -> Self {
        let settings = command.settings();
        let mut rules: Vec<(&'static Rule, i64)> = rules
            .iter()
            .map(|r| (r, settings.priority_for(r.name, r.priority)))
            .collect();
        // Python loads `sorted(glob('*.py'))`, then stable-sorts by priority.
        rules.sort_by(|(a, pa), (b, pb)| pa.cmp(pb).then_with(|| a.name.cmp(b.name)));
        Corrector {
            command,
            rules,
            next: 0,
            leftover: Vec::new(),
        }
    }

    /// Suggestions of the next enabled rule that matches.
    fn next_rule(&mut self) -> Option<Vec<CorrectedCommand>> {
        let settings = self.command.settings();
        while let Some(&(rule, priority)) = self.rules.get(self.next) {
            self.next += 1;
            if !settings.is_rule_enabled(rule.name, rule.enabled_by_default) {
                continue;
            }
            if !logs::debug_time(format_args!("Trying rule: {};", rule.name), || {
                rule.is_match(self.command)
            }) {
                continue;
            }
            let scripts = (rule.get_new_command)(self.command);
            return Some(
                scripts
                    .into_iter()
                    .enumerate()
                    .map(|(n, script)| CorrectedCommand {
                        script,
                        side_effect: rule.side_effect,
                        priority: (n as i64 + 1) * priority,
                    })
                    .collect(),
            );
        }
        None
    }

    /// The first suggestion: what `-y` runs and the UI shows first.
    pub fn first(&mut self) -> Option<CorrectedCommand> {
        loop {
            let mut suggestions = self.next_rule()?;
            if !suggestions.is_empty() {
                let first = suggestions.remove(0);
                self.leftover = suggestions;
                return Some(first);
            }
        }
    }

    /// `organize_commands` for everything after `first`: the remaining
    /// suggestions sorted by priority, without duplicates.
    pub fn rest(&mut self, first: &CorrectedCommand) -> Vec<CorrectedCommand> {
        let mut all = std::mem::take(&mut self.leftover);
        while let Some(more) = self.next_rule() {
            all.extend(more);
        }
        all.sort_by_key(|c| c.priority);
        let mut unique: Vec<CorrectedCommand> = Vec::with_capacity(all.len());
        for c in all {
            if c != *first && !unique.contains(&c) {
                unique.push(c);
            }
        }
        logs::debug(format_args!(
            "Corrected commands: {}",
            std::iter::once(first)
                .chain(&unique)
                .map(|c| format!("{c:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        unique
    }
}

/// `get_corrected_commands`: every suggestion, organized.
pub fn get_corrected_commands(command: &Command) -> Vec<CorrectedCommand> {
    let mut corrector = Corrector::new(command);
    let Some(first) = corrector.first() else {
        return Vec::new();
    };
    let rest = corrector.rest(&first);
    std::iter::once(first).chain(rest).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::shells::Shell;
    use crate::types::Context;

    fn first_of(ctx: &Context, script: &str, output: &str) -> Option<String> {
        let cmd = Command::new(script, Some(output), ctx);
        Corrector::new(&cmd).first().map(|c| c.script)
    }

    #[test]
    fn rest_is_lazy_and_organized() {
        let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
        let cmd = Command::new(
            "mkdir a/b",
            Some("mkdir: a: No such file or directory\nPermission denied"),
            &ctx,
        );
        let mut corrector = Corrector::new(&cmd);
        let first = corrector.first().unwrap();
        assert_eq!(first.script, "mkdir -p a/b");
        assert!(
            corrector.next < corrector.rules.len(),
            "later rules must not run yet"
        );
        let rest: Vec<String> = corrector
            .rest(&first)
            .into_iter()
            .map(|c| c.script)
            .collect();
        assert!(rest.contains(&"sudo mkdir a/b".to_owned()));
        assert!(!rest.contains(&first.script));
    }

    #[test]
    fn exclusions_and_priorities() {
        let output = "mkdir: a: No such file or directory\nPermission denied";
        let s = Settings {
            exclude_rules: vec!["mkdir_p".into()],
            ..Settings::default()
        };
        let ctx = Context::new(s, Shell::Bash, "fuck".into());
        assert_eq!(
            first_of(&ctx, "mkdir a/b", output).as_deref(),
            Some("sudo mkdir a/b")
        );

        let s = Settings {
            priority: [("sudo".to_owned(), 10)].into(),
            ..Settings::default()
        };
        let ctx = Context::new(s, Shell::Bash, "fuck".into());
        assert_eq!(
            first_of(&ctx, "mkdir a/b", output).as_deref(),
            Some("sudo mkdir a/b")
        );
    }

    #[test]
    fn no_output_means_no_output_rules() {
        let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
        let cmd = Command::new("cd..", None, &ctx);
        assert!(
            Corrector::new(&cmd)
                .first()
                .is_none_or(|c| c.script != "cd ..")
        );
    }
}
