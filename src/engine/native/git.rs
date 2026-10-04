//! git's own completion entry points, the ones git-completion.bash uses:
//! `git --list-cmds=...` for commands and aliases, `git -h` for global
//! options, and `git <builtin> [<subcommand>] --git-completion-helper` for a
//! builtin's options and subcommands.
//!
//! The helper only runs for builtins (an alias or a `git-*` program could do
//! anything), inside a neutral repository in notypo's cache directory, so the
//! user's repository configuration and hooks are never involved.

use super::{Backend, CompletionError, CompletionItem, offline_env, run_stdout};
use crate::engine::probe::Budget;
use std::ffi::OsString;
use std::path::PathBuf;

pub(super) fn complete(
    backend: &Backend,
    words: &[&str],
    prefix: &str,
    budget: &mut Budget,
) -> Result<Vec<CompletionItem>, CompletionError> {
    let globals = global_options(backend, budget)?;
    // Skip global options (and the values of `-C <path>` style ones).
    let mut i = 0;
    while let Some(word) = words.get(i).filter(|w| w.starts_with('-')) {
        let name = word.split('=').next().unwrap_or(word);
        let separate = !word.contains('=')
            && globals
                .iter()
                .any(|o| o.value == name && o.takes_value == Some(true));
        i += if separate { 2 } else { 1 };
    }
    let Some(&command) = words.get(i) else {
        return if prefix.starts_with('-') {
            Ok(globals)
        } else {
            Ok(lines(&list(
                backend,
                "main,others,alias,nohelpers",
                budget,
            )?))
        };
    };
    let builtins = list(backend, "builtins", budget)?;
    if !builtins.lines().any(|b| b.trim() == command) {
        return Err(CompletionError::Unsupported(format!(
            "git {command} is not a builtin, so its arguments are not probed"
        )));
    }
    let top = helper(backend, &[command], budget)?;
    let subcommands: Vec<&str> = top
        .split_whitespace()
        .filter(|w| !w.starts_with('-'))
        .collect();
    let rest = &words[i + 1..];
    let sub = rest
        .iter()
        .find(|w| !w.starts_with('-') && subcommands.contains(w));
    if prefix.starts_with('-') {
        let tokens = match sub {
            Some(sub) => helper(backend, &[command, sub], budget)?,
            None => top,
        };
        return Ok(options(&tokens));
    }
    if sub.is_none() && rest.iter().all(|w| w.starts_with('-')) {
        return Ok(lines(&subcommands.join("\n")));
    }
    Ok(Vec::new())
}

fn base_env() -> Vec<(OsString, Option<OsString>)> {
    let mut env = offline_env(super::Flavor::Git);
    env.extend(
        [
            ("GIT_PAGER", "cat"),
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GIT_OPTIONAL_LOCKS", "0"),
        ]
        .map(|(k, v)| (OsString::from(k), Some(OsString::from(v)))),
    );
    env
}

/// `git --list-cmds=<kinds>`, read in the current directory so aliases from
/// the repository's configuration are included (config is only read).
fn list(backend: &Backend, kinds: &str, budget: &mut Budget) -> Result<String, CompletionError> {
    backend.memoized(&["--list-cmds", kinds], || {
        run_stdout(
            &backend.completer,
            vec![format!("--list-cmds={kinds}").into()],
            base_env(),
            budget,
            true,
        )
    })
}

/// Global options from the usage `git -h` prints, such as `-C <path>`.
fn global_options(
    backend: &Backend,
    budget: &mut Budget,
) -> Result<Vec<CompletionItem>, CompletionError> {
    // `git -h` exits with 129 after printing its usage.
    let usage = backend.memoized(&["-h"], || {
        run_stdout(
            &backend.completer,
            vec!["-h".into()],
            base_env(),
            budget,
            false,
        )
    })?;
    let mut items = Vec::new();
    let usage = usage.split("\n\n").next().unwrap_or_default();
    for group in regex!(r"\[(-[^\[\]]*(?:\[[^\]]*\][^\[\]]*)?)\]").captures_iter(usage) {
        for alternative in group[1].split(" | ") {
            let alternative = alternative.trim();
            let name_end = alternative
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .unwrap_or(alternative.len());
            let (name, rest) = alternative.split_at(name_end);
            if name.len() < 2 {
                continue;
            }
            // `-C <path>` and `--git-dir=<path>` accept a separate value;
            // `--exec-path[=<path>]` only an attached one.
            let takes_value = rest.starts_with(" <") || rest.starts_with("=<");
            items.push(CompletionItem {
                value: name.to_owned(),
                takes_value: Some(takes_value),
                description: None,
            });
        }
    }
    if items.is_empty() {
        return Err(CompletionError::Failed("git -h printed no options".into()));
    }
    Ok(items)
}

/// The output of `<args> --git-completion-helper`, run in the neutral
/// probe repository.
fn helper(
    backend: &Backend,
    args: &[&str],
    budget: &mut Budget,
) -> Result<String, CompletionError> {
    let mut key: Vec<&str> = args.to_vec();
    key.push("--git-completion-helper");
    backend.memoized(&key, || {
        let repo = probe_repository(backend, budget)?;
        let mut argv: Vec<OsString> = vec![
            "-C".into(),
            repo.into_os_string(),
            "-c".into(),
            "core.fsmonitor=false".into(),
        ];
        argv.extend(args.iter().map(OsString::from));
        argv.push("--git-completion-helper".into());
        run_stdout(&backend.completer, argv, base_env(), budget, true).map_err(|_| {
            CompletionError::Unsupported(format!("git {} has no completion helper", args.join(" ")))
        })
    })
}

/// An empty repository owned by notypo, created once.
fn probe_repository(backend: &Backend, budget: &mut Budget) -> Result<PathBuf, CompletionError> {
    let repo = crate::utils::cache_dir().join("git-probe");
    if !repo.join(".git/HEAD").is_file() {
        let _ = std::fs::create_dir_all(&repo);
        run_stdout(
            &backend.completer,
            vec![
                "init".into(),
                "-q".into(),
                "--template=".into(),
                repo.clone().into_os_string(),
            ],
            base_env(),
            budget,
            true,
        )?;
    }
    Ok(repo)
}

fn lines(text: &str) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = Vec::new();
    for word in text.split_whitespace() {
        if !items.iter().any(|i| i.value == word) {
            items.push(CompletionItem {
                value: word.to_owned(),
                takes_value: None,
                description: None,
            });
        }
    }
    items
}

/// Options from helper output: `--message=` takes a value, `--` separates
/// the negated forms, and words are subcommands.
fn options(tokens: &str) -> Vec<CompletionItem> {
    tokens
        .split_whitespace()
        .filter(|t| t.starts_with('-') && *t != "--")
        .map(|t| match t.strip_suffix('=') {
            Some(name) => CompletionItem {
                value: name.to_owned(),
                takes_value: Some(true),
                description: None,
            },
            None => CompletionItem {
                value: t.to_owned(),
                takes_value: Some(false),
                description: None,
            },
        })
        .collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::tests::{Dir, FAKE_GIT};
    use super::super::{Discovery, NativeCompletionBackend, discover};
    use super::*;
    use std::time::Duration;

    fn backend() -> (Dir, Backend) {
        let dir = Dir::new("git");
        let git = dir.script("bin/git", FAKE_GIT);
        let Discovery::Found(backend) = discover("git", &git, &[]) else {
            panic!("git is a built-in bridge");
        };
        (dir, backend)
    }

    fn values(items: Vec<CompletionItem>) -> Vec<String> {
        items.into_iter().map(|i| i.value).collect()
    }

    #[test]
    fn commands_options_and_subcommands_come_from_git() {
        let (dir, backend) = backend();
        let mut budget = Budget::new(Duration::from_secs(20), Duration::from_secs(5), 32);
        let top = values(backend.complete(&[], "", &mut budget).unwrap());
        assert!(top.contains(&"status".into()) && top.contains(&"co".into()));
        let globals = backend.complete(&[], "-", &mut budget).unwrap();
        assert!(
            globals
                .iter()
                .any(|g| g.value == "-C" && g.takes_value == Some(true))
        );
        assert!(
            globals
                .iter()
                .any(|g| g.value == "--no-pager" && g.takes_value == Some(false))
        );
        assert_eq!(
            values(
                backend
                    .complete(&["-C", "x", "status"], "--", &mut budget)
                    .unwrap()
            ),
            ["--short", "--branch", "--porcelain", "--no-short"]
        );
        assert_eq!(
            values(backend.complete(&["stash"], "", &mut budget).unwrap()),
            ["apply", "clear", "drop", "pop", "push"]
        );
        assert_eq!(
            values(
                backend
                    .complete(&["stash", "pop"], "-", &mut budget)
                    .unwrap()
            ),
            ["--index", "--quiet", "--no-index"]
        );
        let commit = backend.complete(&["commit"], "-", &mut budget).unwrap();
        assert_eq!(commit[0].takes_value, Some(true));
        assert!(
            backend
                .complete(&["commit", "-m", "x"], "", &mut budget)
                .unwrap()
                .is_empty()
        );
        // `co` is an alias: git never runs it to learn its arguments.
        assert!(matches!(
            backend.complete(&["co"], "", &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
        assert!(!dir.0.join("bin/ran").exists());
        let calls = std::fs::read_to_string(dir.0.join("bin/calls")).unwrap();
        assert!(!calls.lines().any(|c| c.starts_with("co")), "{calls}");
    }
}
