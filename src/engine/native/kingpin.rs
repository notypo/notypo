//! kingpin's completion flag, also in the fisk fork: every kingpin app
//! accepts a hidden `--completion-bash` as its first argument and prints, one
//! word per line, the subcommands of the deepest command the following words
//! name, or that command's argument hints when it has no subcommands
//! (kingpin forbids mixing them), or every long flag when the last word is
//! `--`. It lists neither aliases, short flags, `--no-` forms, nor hidden
//! items, so its lists are partial.
//!
//! Answering still parses the line: the app's own pre-actions run and every
//! flag's default and environment value is set, and value types that open
//! files create them (verified with kingpin 2.4.0). Queries therefore need
//! `trusted_help` as well, run in a private empty directory, and carry only
//! command words the app listed (or a word whose answer differs from its
//! parent's, for unlisted aliases), never options, their values, or `@file`
//! arguments, which kingpin expands by reading the file.

use super::{Backend, Capabilities, CompletionError, CompletionItem, Flavor, Trust};
use crate::engine::probe::{self, Budget, Capture, Probe};
use std::path::PathBuf;

const LIBRARIES: [&str; 3] = [
    "gopkg.in/alecthomas/kingpin.v2",
    "github.com/alecthomas/kingpin/v2",
    "github.com/choria-io/fisk",
];

const FLAG: &str = "--completion-bash";

pub(super) fn linked(module: &super::go::GoModule) -> bool {
    LIBRARIES.iter().any(|library| module.uses(library))
}

#[derive(Clone, Debug, Default)]
pub(super) struct Protocol {
    pub trusted_help: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct Level {
    words: Vec<CompletionItem>,
}

struct Walk<'a> {
    path: Vec<&'a str>,
    positional: bool,
}

/// An empty directory of notypo's own for the app's relative writes.
fn private_dir() -> Result<PathBuf, CompletionError> {
    let dir = crate::utils::cache_dir().join("kingpin-probes");
    std::fs::create_dir_all(&dir).map_err(|error| {
        CompletionError::Failed(format!("no private directory for kingpin probes: {error}"))
    })?;
    Ok(dir)
}

impl Protocol {
    pub fn allowed(&self, backend: &Backend) -> bool {
        self.trusted_help.iter().any(|trusted| {
            trusted == "*" || *trusted == backend.name || Some(trusted) == backend.identity.as_ref()
        })
    }

    pub fn capabilities(&self, trust: Trust) -> Capabilities {
        Capabilities {
            subcommands: true,
            options: true,
            option_arity: false,
            values: false,
            resources: false,
            option_prefix: "--",
            short_options: false,
            complete_options: false,
            complete_subcommands: false,
            descriptions: false,
            query_dialect: None,
            trust,
        }
    }

    fn raw(
        &self,
        backend: &Backend,
        path: &[&str],
        options: bool,
        budget: &mut Budget,
    ) -> Result<String, CompletionError> {
        let mut args: Vec<&str> = vec![FLAG];
        args.extend_from_slice(path);
        if options {
            args.push("--");
        }
        let mut key = vec!["kingpin"];
        key.extend_from_slice(&args);
        if !self.allowed(backend) {
            return Err(CompletionError::Failed(format!(
                "kingpin runs the app's pre-actions and flag defaults while completing; add {} to trusted_help to allow it",
                backend.name
            )));
        }
        let dir = private_dir()?;
        backend.memoized(&key, || {
            let output = probe::run_in(
                &Probe {
                    program: &backend.completer,
                    args: args.iter().map(Into::into).collect(),
                    env: super::offline_env(Flavor::Kingpin),
                    capture: Capture::Stdout,
                },
                &dir,
                budget,
            )
            .map_err(super::probe_error)?;
            if output.status != Some(0) {
                return Err(CompletionError::Failed(format!(
                    "kingpin completion exited with {:?}",
                    output.status
                )));
            }
            if output.truncated {
                return Err(CompletionError::Failed(
                    "kingpin completion output exceeded the probe limit".into(),
                ));
            }
            String::from_utf8(output.data).map_err(|_| {
                CompletionError::Failed("kingpin completion output is not valid UTF-8".into())
            })
        })
    }

    fn parse(text: &str, limit: usize) -> Result<Vec<CompletionItem>, CompletionError> {
        let mut items: Vec<CompletionItem> = Vec::new();
        for word in text.lines().filter(|line| !line.is_empty()) {
            // Usage or error text means the request was not answered.
            if word.contains(char::is_whitespace) || word.contains(char::is_control) || word == "--"
            {
                return Err(CompletionError::Failed(
                    "kingpin completion returned a line that is not one word".into(),
                ));
            }
            if items.iter().any(|item| item.value == word) {
                continue;
            }
            if items.len() >= limit {
                return Err(super::over_limit());
            }
            items.push(CompletionItem {
                value: word.to_owned(),
                takes_value: None,
                description: None,
            });
        }
        Ok(items)
    }

    fn level(
        &self,
        backend: &Backend,
        path: &[&str],
        budget: &mut Budget,
    ) -> Result<Level, CompletionError> {
        let items = Self::parse(
            &self.raw(backend, path, false, budget)?,
            budget.max_candidates,
        )?;
        Ok(Level {
            words: items.into_iter().filter(|item| !item.is_option()).collect(),
        })
    }

    fn walk<'a>(
        &self,
        backend: &Backend,
        words: &[&'a str],
        budget: &mut Budget,
    ) -> Result<Walk<'a>, CompletionError> {
        let mut path: Vec<&str> = Vec::new();
        let mut level = self.level(backend, &path, budget)?;
        let mut i = 0;
        while let Some(&word) = words.get(i) {
            i += 1;
            if word == "--" {
                return Ok(Walk {
                    path,
                    positional: true,
                });
            }
            if word.starts_with('-') && word != "-" {
                // Only `--name=value` states its value. Otherwise take the
                // next word as the value unless the app lists it as a command.
                if !word.contains('=')
                    && words.get(i).is_some_and(|next| {
                        !next.starts_with('-')
                            && !level.words.iter().any(|item| item.value == *next)
                    })
                {
                    i += 1;
                }
                continue;
            }
            if word.starts_with('@') {
                return Ok(Walk {
                    path,
                    positional: true,
                });
            }
            let listed = level.words.iter().any(|item| item.value == word);
            let mut child = path.clone();
            child.push(word);
            if listed {
                path = child;
                level = self.level(backend, &path, budget)?;
                continue;
            }
            // Aliases and hidden commands are not listed. A word naming no
            // command gets its parent's answer again.
            let answer = self.level(backend, &child, budget)?;
            if answer != level {
                path = child;
                level = answer;
                continue;
            }
            return Ok(Walk {
                path,
                positional: true,
            });
        }
        Ok(Walk {
            path,
            positional: false,
        })
    }

    pub fn complete(
        &self,
        backend: &Backend,
        words: &[&str],
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        if words
            .iter()
            .chain([&prefix])
            .any(|word| word.contains(char::is_control))
        {
            return Err(CompletionError::Unsupported(
                "kingpin completion context contains control characters".into(),
            ));
        }
        let walk = self.walk(backend, words, budget)?;
        let mut items = if prefix.starts_with('-') {
            let limit = budget.max_candidates;
            Self::parse(&self.raw(backend, &walk.path, true, budget)?, limit)?
                .into_iter()
                .filter(CompletionItem::is_option)
                .collect()
        } else if walk.positional {
            return Err(CompletionError::Unsupported(
                "kingpin lists nothing after an argument it does not know".into(),
            ));
        } else {
            self.level(backend, &walk.path, budget)?.words
        };
        items.retain(|item| item.value.starts_with(prefix));
        Ok(items)
    }

    /// Whether `typed`, missing from a partial list after `words`, names a
    /// command kingpin does not list (an alias or a hidden command).
    pub fn confirms(
        &self,
        backend: &Backend,
        words: &[&str],
        typed: &str,
        budget: &mut Budget,
    ) -> Option<bool> {
        if typed.is_empty()
            || typed.starts_with(['-', '@'])
            || typed.contains(char::is_control)
            || words.iter().any(|word| word.contains(char::is_control))
        {
            return None;
        }
        let walk = self.walk(backend, words, budget).ok()?;
        if walk.positional {
            return None;
        }
        let level = self.level(backend, &walk.path, budget).ok()?;
        let mut child = walk.path.clone();
        child.push(typed);
        let answer = self.level(backend, &child, budget).ok()?;
        Some(answer != level)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::{
        Backend, Discovery, NativeCompletionBackend, discover, discover_for_shell_with_budget, go,
        tests::Dir,
    };
    use super::*;
    use crate::shells::Shell;
    use std::time::Duration;

    const MODULE: &str = "example.com/kingpin-fixture";

    /// Answers like kingpin 2.4.0 did for a fixture app: the flag first,
    /// commands per level (aliases unlisted), `help` at the root, every long
    /// flag after `--`, and an unknown word falling back to its parent.
    const APP: &str = r#"#!/bin/sh
root=$(dirname "$0")
[ "$HTTPS_PROXY" = http://127.0.0.1:9 ] && [ "$KUBECONFIG" = /dev/null ] || exit 8
printf '<%s>' "$@" >> "$root/queries"
printf '|%s\n' "$PWD" >> "$root/queries"
[ "$1" = --completion-bash ] || { touch "$root/operation-marker"; exit 9; }
shift
touch default-value-file
if [ -f "$root/mode" ]; then
  case "$(cat "$root/mode")" in
    usage) printf 'usage: fixture [<flags>] <command>\n'; exit 0;;
    failed) exit 1;;
  esac
fi
case "$*" in
  '--') printf -- '--help\n--config\n--log-file';;
  'deploy --'|'d --') printf -- '--region\n--help\n--config\n--log-file';;
  'deploy'|'d') printf 'status\nrollback';;
  'deploy status'|'d status') ;;
  *) printf 'help\ndeploy\ndb:migrate';;
esac
"#;

    fn backend(dir: &Dir, help: &[&str]) -> Backend {
        let path = dir.script("bin/fixture", APP);
        let mut backend = Backend::new(Flavor::Kingpin, "fixture", path);
        backend.identity = Some(MODULE.into());
        backend.kingpin = Some(Box::new(Protocol {
            trusted_help: help.iter().map(|h| h.to_string()).collect(),
        }));
        backend
    }

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(10), Duration::from_secs(5), 16)
    }

    fn values(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.value.as_str()).collect()
    }

    fn queries(dir: &Dir) -> String {
        std::fs::read_to_string(dir.0.join("bin/queries")).unwrap_or_default()
    }

    #[test]
    fn levels_flags_and_unlisted_aliases_come_from_the_app_in_a_private_directory() {
        let dir = Dir::new("kingpin-levels");
        let backend = backend(&dir, &["fixture"]);
        let mut budget = budget();
        assert_eq!(
            values(&backend.complete(&[], "", &mut budget).unwrap()),
            ["help", "deploy", "db:migrate"]
        );
        assert_eq!(
            values(
                &backend
                    .complete(&["--config", "x", "deploy"], "--", &mut budget)
                    .unwrap()
            ),
            ["--region", "--help", "--config", "--log-file"]
        );
        // `d` is not listed, but its answer differs from the root's.
        assert_eq!(
            values(&backend.complete(&["d"], "", &mut budget).unwrap()),
            ["status", "rollback"]
        );
        assert_eq!(backend.confirms(&[], "d", &mut budget), Some(true));
        assert_eq!(backend.confirms(&[], "nosuch", &mut budget), Some(false));
        for words in [
            &["nosuch"][..],
            &["@args-file"],
            &["deploy", "--", "status"],
        ] {
            assert!(
                matches!(
                    backend.complete(words, "", &mut budget),
                    Err(CompletionError::Unsupported(_))
                ),
                "{words:?}"
            );
        }
        let queries = queries(&dir);
        assert!(
            queries
                .lines()
                .all(|line| line.starts_with("<--completion-bash>")),
            "{queries}"
        );
        assert!(
            !queries.contains("<--config>") && !queries.contains("@args-file"),
            "{queries}"
        );
        let private =
            std::fs::canonicalize(crate::utils::cache_dir().join("kingpin-probes")).unwrap();
        assert!(
            queries
                .lines()
                .all(|line| line.ends_with(&format!("|{}", private.display()))),
            "{queries}"
        );
        assert!(!dir.0.join("bin/operation-marker").exists());
        let capabilities = backend.capabilities();
        assert!(!capabilities.complete_subcommands && !capabilities.short_options);
        assert_eq!(capabilities.option_prefix, "--");
        assert_eq!(backend.cache_identity(), None);
    }

    #[test]
    fn help_trust_usage_text_and_failures_gate_every_answer() {
        let dir = Dir::new("kingpin-gates");
        let untrusted = backend(&dir, &[]);
        let mut budget = budget();
        assert!(matches!(
            untrusted.complete(&[], "", &mut budget),
            Err(CompletionError::Failed(why)) if why.contains("trusted_help")
        ));
        assert!(queries(&dir).is_empty());
        let identity = backend(&dir, &[MODULE]);
        assert!(identity.complete(&[], "", &mut budget).is_ok());
        for (mode, why) in [("usage", "not one word"), ("failed", "exited")] {
            let dir = Dir::new("kingpin-failures");
            let backend = backend(&dir, &["fixture"]);
            dir.script("bin/mode", mode);
            let result = backend.complete(&["deploy"], "", &mut budget);
            assert!(
                matches!(&result, Err(CompletionError::Failed(message)) if message.contains(why)),
                "{mode}: {result:?}"
            );
        }
    }

    #[test]
    fn discovery_needs_both_trust_settings_and_refuses_ambiguous_libraries() {
        let dir = Dir::new("kingpin-discovery");
        let path = dir.0.join("bin/tool");
        std::fs::create_dir_all(dir.0.join("bin")).unwrap();
        let binary =
            |lines: &str| std::fs::write(&path, go::fake_binary_info(MODULE, lines)).unwrap();
        binary("dep\tgithub.com/alecthomas/kingpin/v2\tv2.4.0\th1:x\n");
        assert!(matches!(
            discover("tool", &path, &[]),
            Discovery::NotTrusted(why) if why.contains("kingpin")
        ));
        let mut budget = budget();
        let trusted = [MODULE.to_owned()];
        assert!(matches!(
            discover_for_shell_with_budget("tool", &path, &trusted, &[], Shell::Bash, &mut budget),
            Discovery::NotTrusted(why) if why.contains("trusted_help")
        ));
        assert!(matches!(
            discover_for_shell_with_budget("tool", &path, &trusted, &["tool".into()], Shell::Bash, &mut budget),
            Discovery::Found(backend) if backend.flavor == Flavor::Kingpin
        ));
        for lines in [
            "dep\tgithub.com/spf13/cobra\tv1.10.2\th1:x\ndep\tgopkg.in/alecthomas/kingpin.v2\tv2.2.6\th1:y\n",
            "dep\tgithub.com/urfave/cli/v2\tv2.27.7\th1:x\ndep\tgithub.com/choria-io/fisk\tv0.9.1\th1:y\n",
        ] {
            binary(lines);
            assert!(
                matches!(
                    discover("tool", &path, &["*".into()]),
                    Discovery::Unavailable(_)
                ),
                "{lines}"
            );
        }
        // posener with cobra keeps its earlier choice.
        binary(
            "dep\tgithub.com/posener/complete\tv1.2.3\th1:x\ndep\tgithub.com/spf13/cobra\tv1.10.2\th1:y\n",
        );
        assert!(matches!(
            discover("tool", &path, &["*".into()]),
            Discovery::Found(backend) if backend.flavor == Flavor::Posener
        ));
    }
}
