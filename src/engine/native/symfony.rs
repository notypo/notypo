//! Symfony Console's completion command, which every Symfony Console 5.4+
//! application has (Composer, Symfony framework `bin/console`, Laravel's
//! `artisan`, drush): `app _complete --no-interaction -sbash -c<index>
//! -i<program> -i<word>...` prints one suggestion per line. Words are the
//! command line's, the program first; `-c` is the index of the word being
//! completed, one past the last word for an empty one (the generated scripts
//! drop empty words). Every version speaks `bash`; zsh and fish (with
//! descriptions) came later, and the API version option changed name (`-S`
//! before 6.2, `-a` since), so neither is sent: the version check only runs
//! when one is given.
//!
//! The command name is the first argument and is resolved as the app runs
//! it: an exact name or alias, or an unambiguous abbreviation of each
//! `:`-separated part (`instal`, `c:c`), and hidden commands run though they
//! aren't listed. Asked about such a word, the app answers with the command
//! it resolves to rather than every command, which tells notypo it is valid.
//! Options are listed by long name (and `--no-` for negatable ones) with no
//! shortcuts; option names are never abbreviated.
//!
//! Answering boots the application: Composer reads its configuration and,
//! unless `--no-plugins` is given, its global and project plugins; a
//! framework console boots the project's kernel. So the bridge needs
//! `trusted_help` as well as `trusted_completers`, is found from the app's
//! own help (a PHP script whose `--help` shows Symfony's global options),
//! passes the `--no-plugins` and `--no-scripts` options the help declares,
//! and keeps Composer offline. A console inside the working directory is
//! project code and follows the workspace trust policy.

use super::{Backend, Capabilities, CompletionError, CompletionItem, Flavor, Trust, run_stdout};
use crate::engine::cache::fingerprint;
use crate::engine::probe::Budget;
use std::ffi::OsString;
use std::path::Path;

/// Global options that keep a project's own code from running, passed when
/// the app's help declares them (Composer's).
const GUARDS: [&str; 2] = ["--no-plugins", "--no-scripts"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Protocol {
    guards: Vec<&'static str>,
    /// The app may have been replaced since its help was read.
    application_fingerprint: String,
}

fn app_fingerprint(path: &Path) -> String {
    let mut files = vec![path.to_owned()];
    files.extend(std::fs::canonicalize(path).ok());
    fingerprint(&files, &["symfony-console"])
}

/// A script PHP runs: `#!/usr/bin/env php` or a direct interpreter path.
pub(super) fn is_php_script(path: &Path) -> bool {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let head = super::head(&real, 256);
    head.lines().next().is_some_and(|shebang| {
        shebang.starts_with("#!")
            && shebang
                .split_whitespace()
                .any(|word| word.rsplit('/').next().is_some_and(|name| name.starts_with("php")))
    })
}

pub(super) fn backend(
    name: &str,
    path: &Path,
    identity: Option<String>,
    protocol: Protocol,
) -> Backend {
    let mut backend =
        Backend::new(Flavor::Symfony, name, path.to_owned()).with_application(path.to_owned());
    backend.identity = identity;
    backend.bridge = Some(Box::new(super::Bridge::Symfony(protocol)));
    backend
}

/// Help-based discovery for a trusted PHP script. Only `--help` runs.
pub(super) fn discover_from_help(
    path: &Path,
    budget: &mut Budget,
) -> Result<Option<Protocol>, CompletionError> {
    if !is_php_script(path) {
        return Ok(None);
    }
    let before = app_fingerprint(path);
    let help = run_stdout(
        path,
        vec!["--help".into(), "--no-interaction".into()],
        super::offline_env(Flavor::Symfony),
        budget,
        true,
    )?;
    let Some(protocol) = Protocol::from_help(&help, path) else {
        return Ok(None);
    };
    if protocol.application_fingerprint != before {
        return Err(CompletionError::Failed(
            "the application changed during Symfony Console discovery".into(),
        ));
    }
    Ok(Some(protocol))
}

impl Protocol {
    /// Symfony Console's help shows its own global options: the help
    /// option's description (5.0 onwards) and `--no-interaction`'s.
    pub(super) fn from_help(help: &str, path: &Path) -> Option<Self> {
        let option = |name: &str, description: &str| {
            help.lines().any(|line| {
                let line = line.trim_start();
                line.starts_with(name) && line.trim_end().ends_with(description)
            })
        };
        if !option("-h, --help", "When no command is given display help for the list command")
            || !option("-n, --no-interaction", "Do not ask any interactive question")
        {
            return None;
        }
        let guards = GUARDS
            .into_iter()
            .filter(|guard| {
                help.lines().any(|line| {
                    line.trim_start()
                        .split_whitespace()
                        .next()
                        .is_some_and(|word| word == *guard)
                })
            })
            .collect();
        Some(Self {
            guards,
            application_fingerprint: app_fingerprint(path),
        })
    }

    pub fn capabilities(&self, trust: Trust) -> Capabilities {
        Capabilities {
            subcommands: true,
            options: true,
            option_arity: false,
            values: true,
            resources: false,
            option_prefix: "--",
            // Shortcuts are never listed.
            short_options: false,
            complete_options: true,
            // Hidden commands and abbreviations run without being listed.
            complete_subcommands: false,
            descriptions: false,
            query_dialect: None,
            trust,
        }
    }

    pub fn capabilities_for(&self, words: &[&str], trust: Trust) -> Capabilities {
        let mut capabilities = self.capabilities(trust);
        // After the command, words are its arguments: there are no nested
        // commands (namespaces are part of the name, `cache:clear`).
        if words.iter().any(|word| !word.starts_with('-')) {
            capabilities.subcommands = false;
        }
        capabilities
    }

    fn query(
        &self,
        backend: &Backend,
        words: &[&str],
        current: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        if app_fingerprint(&backend.completer) != self.application_fingerprint {
            return Err(CompletionError::Failed(
                "the app changed after its Symfony Console help was read".into(),
            ));
        }
        // Empty words would be dropped, shifting the index; newlines would
        // split the answer.
        if words
            .iter()
            .any(|word| word.is_empty() || word.contains(char::is_control))
            || current.contains(char::is_control)
        {
            return Err(CompletionError::Unsupported(
                "the command line has empty words or control characters".into(),
            ));
        }
        let mut args: Vec<String> = self.guards.iter().map(|g| (*g).to_owned()).collect();
        args.extend([
            "_complete".into(),
            "--no-interaction".into(),
            "-sbash".into(),
            format!("-c{}", words.len() + 1),
            format!("-i{}", backend.name),
        ]);
        args.extend(words.iter().map(|word| format!("-i{word}")));
        if !current.is_empty() {
            args.push(format!("-i{current}"));
        }
        let key: Vec<&str> = std::iter::once("symfony")
            .chain(args.iter().map(String::as_str))
            .collect();
        let text = backend.memoized(&key, || {
            run_stdout(
                &backend.completer,
                args.iter().map(OsString::from).collect(),
                super::offline_env(Flavor::Symfony),
                budget,
                true,
            )
        })?;
        parse(&text, budget.max_candidates)
    }

    pub fn complete(
        &self,
        backend: &Backend,
        words: &[&str],
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let mut items = self.query(backend, words, prefix, budget)?;
        items.retain(|item| item.value.starts_with(prefix));
        Ok(items)
    }

    /// An option's values, when it takes one. After a flag the slot is the
    /// next argument, so its answer repeats the parent's.
    pub fn values(
        &self,
        backend: &Backend,
        words: &[&str],
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let Some((option, parent)) = words.split_last() else {
            return Ok(Vec::new());
        };
        if !option.starts_with('-') {
            return Ok(Vec::new());
        }
        let values = self.query(backend, words, "", budget)?;
        // The parent's answer can fail where the option's doesn't (Composer
        // lists installed packages only inside a project): then they differ.
        if self.query(backend, parent, "", budget).ok().as_ref() == Some(&values) {
            return Err(CompletionError::Unsupported(format!(
                "{option} takes no value that the app completes"
            )));
        }
        Ok(values)
    }

    /// Whether an unlisted command word is one the app runs: asked about it,
    /// the app answers with the command it resolves to (an abbreviation,
    /// alias, or hidden command), not with every command.
    pub fn confirms(
        &self,
        backend: &Backend,
        words: &[&str],
        typed: &str,
        budget: &mut Budget,
    ) -> Option<bool> {
        if words.iter().any(|word| !word.starts_with('-')) || typed.starts_with('-') {
            return None;
        }
        let all = self.query(backend, words, "", budget).ok()?;
        // With the typed word as the current one.
        let answer = self.query(backend, words, typed, budget).ok()?;
        Some(!answer.is_empty() && answer.len() < all.len())
    }

    /// A command's argument values come from its own callbacks: package,
    /// service, or route names. Option values are the app's choices.
    pub fn resource(&self, words: &[&str], item: &CompletionItem) -> bool {
        let option_value = words.last().is_some_and(|word| word.starts_with('-'));
        !item.is_option() && !option_value && words.iter().any(|word| !word.starts_with('-'))
    }
}

/// One suggestion per line. A line with whitespace is a value the command
/// line couldn't hold unquoted, or a message: it is left out.
fn parse(text: &str, limit: usize) -> Result<Vec<CompletionItem>, CompletionError> {
    let mut items: Vec<CompletionItem> = Vec::new();
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty()
            || line.contains(char::is_whitespace)
            || line.contains(char::is_control)
            || items.iter().any(|item| item.value == line)
        {
            continue;
        }
        if items.len() >= limit {
            return Err(super::over_limit());
        }
        items.push(CompletionItem {
            value: line.to_owned(),
            takes_value: None,
            description: None,
        });
    }
    Ok(items)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn help_signatures_and_guards_come_from_the_apps_own_help() {
        let path = Path::new("/nonexistent/composer");
        let composer = "Usage:\n  list [options] [--] [<namespace>]\n\nOptions:\n  -h, --help                     Display help for the given command. When no command is given display help for the list command\n  -n, --no-interaction           Do not ask any interactive question\n      --no-plugins               Whether to disable plugins.\n      --no-scripts               Skips the execution of all scripts defined in composer.json file.\n";
        let protocol = Protocol::from_help(composer, path).unwrap();
        assert_eq!(protocol.guards, ["--no-plugins", "--no-scripts"]);
        let console = "Options:\n  -h, --help            Display help for the given command. When no command is given display help for the list command\n  -n, --no-interaction  Do not ask any interactive question\n  -e, --env=ENV         The Environment name. [default: \"dev\"]\n";
        assert!(Protocol::from_help(console, path).unwrap().guards.is_empty());
        for other in [
            "usage: tool [-h]\n  -h, --help  show this help message and exit\n",
            "  -h, --help  Display help for the given command. When no command is given display help for the list command\n",
            "Mentions --no-plugins and -n, --no-interaction Do not ask any interactive question\n",
        ] {
            assert_eq!(Protocol::from_help(other, path), None, "{other}");
        }
    }

    #[test]
    fn answers_are_one_word_per_line() {
        let items = parse("install\ni\n--dev\n--no-dev\n\ninstall\nnot one word\n", 16).unwrap();
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(values, ["install", "i", "--dev", "--no-dev"]);
        assert!(parse("a\nb\nc\n", 2).is_err());
    }

    #[test]
    fn php_scripts_are_recognized_by_their_interpreter() {
        let dir = super::super::tests::Dir::new("symfony-php");
        let env = dir.script("composer", "#!/usr/bin/env php\n<?php\n");
        let direct = dir.script("console", "#!/opt/homebrew/bin/php\n<?php\n");
        let python = dir.script("tool", "#!/usr/bin/env python3\n");
        let phpstan = dir.script("phpstan", "#!/bin/sh\nexec phpstan.phar\n");
        assert!(is_php_script(&env));
        assert!(is_php_script(&direct));
        assert!(!is_php_script(&python));
        assert!(!is_php_script(&phpstan));
    }
}
