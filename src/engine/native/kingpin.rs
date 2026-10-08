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
//!
//! Argument hints may open repositories or connect to a server, even at a
//! command group: kingpin implicitly descends into default commands before
//! completing. Its help flag does not reliably disable that descent for
//! custom applications. Commands therefore come from context-sensitive help,
//! whose renderer reparses without defaults. Completion is asked only for
//! option names with a final `--`, never for arguments or option values; help
//! removes flags belonging only to implicitly selected default commands.
//! Including `--help` also suppresses the library's default value setters;
//! application pre-actions still run under the user's explicit trust.

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
    scope: Scope,
}

#[derive(Debug, PartialEq, Eq)]
struct Scope {
    usage: String,
    has_commands: bool,
    commands: Vec<CompletionItem>,
    options: Vec<CompletionItem>,
}

impl Scope {
    fn parse(text: &str) -> Result<Self, CompletionError> {
        let text = crate::engine::docs::strip_formatting(text);
        let lines: Vec<_> = text.lines().collect();
        let start = lines
            .iter()
            .position(|line| line.trim_start().to_ascii_lowercase().starts_with("usage:"))
            .ok_or_else(|| CompletionError::Failed("kingpin help has no usage summary".into()))?;
        let mut usage = lines[start].trim().to_owned();
        for line in &lines[start + 1..] {
            if line.trim().is_empty() || !line.starts_with(char::is_whitespace) {
                break;
            }
            usage.push(' ');
            usage.push_str(line.trim());
        }
        let usage = usage.split_whitespace().collect::<Vec<_>>().join(" ");
        let heading = lines.iter().position(|line| {
            let line = line.trim().to_ascii_lowercase();
            matches!(line.as_str(), "commands:" | "subcommands:")
                || (line.starts_with("commands (") && line.ends_with("):"))
        });
        let has_commands =
            usage.split_whitespace().any(|word| word == "<command>") && heading.is_some();
        let mut commands: Vec<CompletionItem> = Vec::new();
        if has_commands {
            let path: Vec<_> = usage
                .split_whitespace()
                .skip(2)
                .take_while(|word| !word.starts_with(['[', '<', '-']))
                .collect();
            let mut indentation = None;
            let mut tree_parent = None;
            for line in &lines[heading.unwrap() + 1..] {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if matches!(
                    trimmed,
                    "Flags:" | "Global Flags:" | "Args:" | "Command Aliases:"
                ) {
                    break;
                }
                let indent = line.len() - line.trim_start().len();
                let words: Vec<_> = trimmed.split_whitespace().collect();
                // Compact templates print a repeated parent row followed by
                // indented children, instead of a full path on every row.
                if !path.is_empty() && words == path {
                    tree_parent = Some(indent);
                    indentation = None;
                    continue;
                }
                let command = if let Some(parent) = tree_parent {
                    if indent <= parent || indent != *indentation.get_or_insert(indent) {
                        continue;
                    }
                    words.first()
                } else {
                    if !words.starts_with(&path) || indent != *indentation.get_or_insert(indent) {
                        continue;
                    }
                    words.get(path.len())
                };
                let Some(command) = command else {
                    continue;
                };
                let command = command.trim_end_matches('*');
                if command.is_empty()
                    || command.starts_with(['[', '<', '-'])
                    || command.contains(char::is_control)
                    || commands.iter().any(|item| item.value == command)
                {
                    continue;
                }
                commands.push(CompletionItem {
                    value: command.to_owned(),
                    takes_value: None,
                    description: None,
                });
            }
        }
        let options = crate::engine::docs::options(&text.replace("--[no-]", "--"));
        Ok(Self {
            usage,
            has_commands,
            commands,
            options,
        })
    }
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
            trusted == "*"
                || *trusted == backend.name
                || *trusted == super::app_name(&backend.name)
                || Some(trusted) == backend.identity.as_ref()
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
        budget: &mut Budget,
    ) -> Result<String, CompletionError> {
        let mut args: Vec<&str> = vec![FLAG, "--help"];
        args.extend_from_slice(path);
        args.push("--");
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

    fn scope(
        &self,
        backend: &Backend,
        path: &[&str],
        budget: &mut Budget,
    ) -> Result<Scope, CompletionError> {
        if !self.allowed(backend) {
            return Err(CompletionError::Failed(format!(
                "kingpin help needs {} in trusted_help",
                backend.name
            )));
        }
        let mut args = vec!["--help"];
        args.extend_from_slice(path);
        let mut key = vec!["kingpin-help"];
        key.extend_from_slice(&args);
        let dir = private_dir()?;
        let text = backend.memoized(&key, || {
            let output = probe::run_in(
                &Probe {
                    program: &backend.completer,
                    args: args.iter().map(Into::into).collect(),
                    env: super::offline_env(Flavor::Kingpin),
                    capture: Capture::Combined,
                },
                &dir,
                budget,
            )
            .map_err(super::probe_error)?;
            if output.status != Some(0) {
                return Err(CompletionError::Failed(format!(
                    "kingpin help exited with {:?}",
                    output.status
                )));
            }
            if output.truncated {
                return Err(CompletionError::Failed(
                    "kingpin help output exceeded the probe limit".into(),
                ));
            }
            String::from_utf8(output.data).map_err(|_| {
                CompletionError::Failed("kingpin help output is not valid UTF-8".into())
            })
        })?;
        Scope::parse(&text)
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
        let scope = self.scope(backend, path, budget)?;
        if scope.commands.len() > budget.max_candidates {
            return Err(super::over_limit());
        }
        Ok(Level {
            words: scope.commands.clone(),
            scope,
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
            if !level.scope.has_commands {
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
            // Aliases and hidden commands are not listed. Help resolves
            // them without invoking the command's argument hints.
            let answer = self.level(backend, &child, budget)?;
            if answer.scope.usage != level.scope.usage {
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
            let scope = self.scope(backend, &walk.path, budget)?;
            Self::parse(&self.raw(backend, &walk.path, budget)?, limit)?
                .into_iter()
                .filter(|item| {
                    scope
                        .options
                        .iter()
                        .any(|option| item.value == option.value)
                })
                .collect()
        } else if walk.positional {
            return Err(CompletionError::Unsupported(
                "kingpin lists nothing after an argument it does not know".into(),
            ));
        } else {
            let level = self.level(backend, &walk.path, budget)?;
            if !level.scope.has_commands {
                return Err(CompletionError::Unsupported(
                    "kingpin argument hints may open a repository or contact a server".into(),
                ));
            }
            level.words
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
        if !level.scope.has_commands {
            return None;
        }
        let mut child = walk.path.clone();
        child.push(typed);
        let answer = self.scope(backend, &child, budget).ok()?;
        Some(answer.usage != level.scope.usage)
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

    /// Models context-sensitive help and completion separately. A command
    /// leaf's argument completer writes a marker, even without arguments.
    const APP: &str = r#"#!/bin/sh
root=$(dirname "$0")
[ "$HTTPS_PROXY" = http://127.0.0.1:9 ] && [ "$KUBECONFIG" = /dev/null ] || exit 8
printf '<%s>' "$@" >> "$root/queries"
printf '|%s\n' "$PWD" >> "$root/queries"
touch default-value-file
if [ -f "$root/mode" ]; then
  case "$(cat "$root/mode")" in
    usage) printf 'not a help document\n'; exit 0;;
    failed) exit 1;;
  esac
fi
case "$1" in
  --help)
    shift
    case "$*" in
      deploy|d) printf 'usage: fixture deploy [<flags>] <command> [<args> ...]\n\nSubcommands:\n  deploy status <target>\n  deploy rollback\n';;
      'deploy status'|'d status'|'deploy s'|'d s') printf 'usage: fixture deploy status [<flags>] <target>\n\nArgs:\n  <target> Target\n';;
      'deploy rollback'|'d rollback') printf 'usage: fixture deploy rollback [<flags>]\n';;
      'db:migrate') printf 'usage: fixture db:migrate [<flags>]\n';;
      *) printf 'usage: fixture [<flags>] <command> [<args> ...]\n\nCommands:\n  help\n  deploy\n  db:migrate\n';;
    esac
    printf '\nFlags:\n  --help                 Help\n  --config=CONFIG        Config file\n  --log-file=LOG-FILE     Log file\n'
    case "$*" in
      deploy|d) printf '  --region=REGION        Region\n';;
      'deploy status'|'d status'|'deploy s'|'d s') printf '  --output=OUTPUT        Output\n';;
    esac
    exit 0;;
  --completion-bash)
    shift
    [ "$1" = --help ] || { touch "$root/default-command-marker"; exit 9; }
    shift;;
  *) touch "$root/operation-marker"; exit 9;;
esac
query=
for word in "$@"; do
  [ -z "$word" ] || query="${query:+$query }$word"
done
case "$query" in
  '--') printf -- '--help\n--config\n--log-file\n--default-leaf-only';;
  'deploy --'|'d --') printf -- '--region\n--help\n--config\n--log-file';;
  'deploy status --'|'d status --'|'deploy s --'|'d s --') printf -- '--output\n--help\n--config\n--log-file';;
  'deploy'|'d') printf 'status\nrollback';;
  'deploy status'|'d status'|'deploy s'|'d s') touch "$root/hint-marker"; printf 'prod\nstaging';;
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
        // `d` is not listed, but its help names the deploy group.
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
                .all(|line| line.starts_with("<--completion-bash><--help>")
                    || line.starts_with("<--help>")),
            "{queries}"
        );
        assert!(
            !queries.contains("<--config>") && !queries.contains("@args-file"),
            "{queries}"
        );
        assert!(
            queries
                .lines()
                .filter(|line| line.starts_with("<--completion-bash>"))
                .all(|line| line.contains("<-->|")),
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
        assert!(!dir.0.join("bin/hint-marker").exists());
        assert!(!dir.0.join("bin/default-command-marker").exists());
        let capabilities = backend.capabilities();
        assert!(!capabilities.complete_subcommands && !capabilities.short_options);
        assert_eq!(capabilities.option_prefix, "--");
        assert_eq!(backend.cache_identity(), None);
        assert_eq!(
            values(&backend.complete(&[], "--", &mut budget).unwrap()),
            ["--help", "--config", "--log-file"]
        );
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
        for (mode, why) in [("usage", "no usage"), ("failed", "exited")] {
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
    fn leaf_aliases_and_options_never_query_argument_hints() {
        let dir = Dir::new("kingpin-leaf-hints");
        let backend = backend(&dir, &["fixture"]);
        let mut budget = budget();
        assert_eq!(backend.confirms(&["deploy"], "s", &mut budget), Some(true));
        for words in [
            &["deploy", "status"][..],
            &["deploy", "s"],
            &["deploy", "status", "prod"],
        ] {
            assert!(matches!(
                backend.complete(words, "", &mut budget),
                Err(CompletionError::Unsupported(_))
            ));
            assert!(
                values(&backend.complete(words, "--", &mut budget).unwrap()).contains(&"--output")
            );
        }
        assert!(!dir.0.join("bin/hint-marker").exists());
        assert!(!queries(&dir).contains("<prod>"));
        assert!(!dir.0.join("bin/operation-marker").exists());
        assert!(!dir.0.join("bin/default-command-marker").exists());
    }

    #[test]
    fn usage_and_command_sections_both_identify_a_group() {
        let group = Scope::parse(
            "\x1b[1musage:\x1b[0m fixture deploy [<flags>]\n    <command> [<args> ...]\n\nSubcommands:\n  deploy status\n",
        )
        .unwrap();
        assert!(group.has_commands);
        assert_eq!(
            group.usage,
            "usage: fixture deploy [<flags>] <command> [<args> ...]"
        );
        for text in [
            "usage: fixture show <resource>\n\nSubcommands:\nA description only.\n",
            "usage: fixture help <command>\n\nArgs:\n<command> Command to describe\n",
        ] {
            assert!(!Scope::parse(text).unwrap().has_commands);
        }
        assert!(Scope::parse("Commands:\n  deploy status\n").is_err());
        let root = Scope::parse("usage: kopia [<flags>] <command> [<args> ...]\n\nCommands (use --help-full to list all commands):\nsnapshot restore* [<flags>] <object>\n    Restore a snapshot\nsnapshot list\n    List snapshots\nrepository status\n    Repository status\n").unwrap();
        assert_eq!(values(&root.commands), ["snapshot", "repository"]);
        let alias = Scope::parse("usage: fixture deploy <command> [<args> ...]\n\nSubcommands:\n  deploy status [<flags>]  Show status\n  deploy rollback        Roll back\n\nGlobal Flags:\n  --help   Help\n").unwrap();
        assert_eq!(values(&alias.commands), ["status", "rollback"]);
        let compact = Scope::parse("usage: kopia snapshot fix <command> [<args> ...]\n\nSubcommands:\n  snapshot fix\n      invalid-files [<flags>]\n      remove-files [<flags>]\n").unwrap();
        assert_eq!(values(&compact.commands), ["invalid-files", "remove-files"]);
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
