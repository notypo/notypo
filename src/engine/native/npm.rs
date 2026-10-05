//! npm's completion command receives literal, separate argv words. Its
//! `unescape` removes outer single quotes, so wrapping each word preserves
//! whitespace, empty arguments, backslashes, and embedded quotes without a
//! shell. COMP_POINT uses JavaScript's UTF-16 offsets, not bytes or scalars.
//!
//! Completion calls command metadata callbacks, never the operation itself.
//! npm can fetch registry metadata in those callbacks; force offline mode,
//! disable scripts/logging, and refuse flags that could override the policy.
//! Project script names and local package names are resources, not commands.

use super::{Backend, CompletionError, CompletionItem};
use crate::engine::probe::{self, Budget, Capture, Probe};

// This is an offline-input policy, not a command/option vocabulary. Unknown
// flags may be nopt abbreviations of network/logging settings, so refuse them
// rather than silently change or drop a user's arguments.
const VALUE_FLAGS: &[&str] = &["--prefix", "--location", "--depth", "--omit", "--include"];
const BOOL_FLAGS: &[&str] = &[
    "--global",
    "-g",
    "--json",
    "--parseable",
    "--long",
    "--all",
    "--dry-run",
    "--package-lock-only",
];

fn positionals<'a>(words: &[&'a str]) -> Result<Vec<&'a str>, CompletionError> {
    let mut args = words.iter().copied();
    let mut result = Vec::new();
    while let Some(word) = args.next() {
        if word == "--" {
            result.extend(args);
            break;
        }
        let flag = word.split_once('=').map_or(word, |(flag, _)| flag);
        if VALUE_FLAGS.contains(&flag) {
            if !word.contains('=') {
                args.next();
            }
        } else if word.starts_with('-') && !BOOL_FLAGS.contains(&flag) {
            // Report a failed check instead of an empty authoritative answer:
            // refusing this context says nothing about its command validity.
            return Err(CompletionError::Failed(format!(
                "npm completion refuses {flag}: outside the offline input policy"
            )));
        } else if !word.starts_with('-') {
            result.push(word);
        }
    }
    Ok(result)
}

fn raw(
    backend: &Backend,
    words: &[&str],
    prefix: &str,
    budget: &mut Budget,
) -> Result<String, CompletionError> {
    if cfg!(windows) {
        return Err(CompletionError::Unsupported(
            "native npm completion is unsupported on Windows".into(),
        ));
    }
    if super::identity::npm_entrypoint(&backend.completer) != Some("npm") {
        return Err(CompletionError::Failed(
            "the installed entry point no longer declares npm completion".into(),
        ));
    }
    positionals(words)?;
    if std::iter::once(backend.name.as_str())
        .chain(words.iter().copied())
        .chain(std::iter::once(prefix))
        .any(|word| word.contains(char::is_control))
    {
        return Err(CompletionError::Unsupported(
            "npm completion cannot represent control characters".into(),
        ));
    }
    let mut key = vec!["npm"];
    key.extend_from_slice(words);
    key.push(prefix);
    backend.memoized(&key, || {
        let encoded: Vec<String> = std::iter::once(backend.name.as_str())
            .chain(words.iter().copied())
            .chain(std::iter::once(prefix))
            .map(|word| format!("'{word}'"))
            .collect();
        let line = encoded.join(" ");
        let mut env = super::offline_env(super::Flavor::Npm);
        env.extend([
            ("COMP_LINE".into(), Some(line.clone().into())),
            (
                "COMP_POINT".into(),
                Some(line.encode_utf16().count().to_string().into()),
            ),
            (
                "COMP_CWORD".into(),
                Some((words.len() + 1).to_string().into()),
            ),
        ]);
        // npm.localPrefix is initialized before the completion callback's
        // nopt parse. Supply this scope to startup as a literal config value,
        // while retaining the entire original context in completion words.
        let mut args = vec!["completion".to_owned()];
        let mut context = words.iter().copied();
        while let Some(word) = context.next() {
            if word == "--" {
                break;
            }
            let (flag, attached) = word
                .split_once('=')
                .map_or((word, None), |(flag, value)| (flag, Some(value)));
            if VALUE_FLAGS.contains(&flag) {
                let value = attached.or_else(|| context.next());
                if flag == "--prefix"
                    && let Some(value) = value
                {
                    args.push(format!("--prefix={value}"));
                }
            }
        }
        args.push("--".to_owned());
        args.extend(encoded);
        let output = probe::run(
            &Probe {
                program: &backend.completer,
                args: args.into_iter().map(Into::into).collect(),
                env,
                capture: Capture::Stdout,
            },
            budget,
        )
        .map_err(super::probe_error)?;
        if output.status != Some(0) {
            return Err(CompletionError::Failed(format!(
                "npm completion exited with {:?}",
                output.status
            )));
        }
        if output.truncated {
            return Err(CompletionError::Failed(
                "npm completion output exceeded the probe limit".into(),
            ));
        }
        String::from_utf8(output.data)
            .map_err(|_| CompletionError::Failed("npm completion output is not valid UTF-8".into()))
    })
}

pub(super) fn complete(
    backend: &Backend,
    words: &[&str],
    prefix: &str,
    budget: &mut Budget,
) -> Result<Vec<CompletionItem>, CompletionError> {
    let answer = raw(backend, words, prefix, budget)?;
    // Learn root command names once per request, so a global flag/value isn't
    // mistaken for a script/package resource context.
    if !words.is_empty() {
        raw(backend, &[], "", budget)?;
    }
    let mut items = Vec::new();
    for value in answer.lines() {
        if items.len() >= budget.max_candidates {
            break;
        }
        if value.is_empty() || value.contains(char::is_control) || !value.starts_with(prefix) {
            continue;
        }
        if !items
            .iter()
            .any(|item: &CompletionItem| item.value == value)
        {
            items.push(CompletionItem {
                value: value.to_owned(),
                takes_value: None,
                description: None,
            });
        }
    }
    Ok(items)
}

pub(super) fn resources(backend: &Backend, words: &[&str]) -> bool {
    let Ok(args) = positionals(words) else {
        return false;
    };
    let Some(command) = args.first() else {
        return false;
    };
    let memo = backend.memo.borrow();
    let key = vec!["npm".to_owned(), String::new()];
    let Some(Ok(root)) = memo.get(&key) else {
        return false;
    };
    root.lines().any(|name| name == *command)
        // `config`'s first list is fixed subcommands; deeper lists are setting
        // names. Other callbacks may return scripts, packages, or API data.
        && !(args.len() == 1 && matches!(*command, "config" | "c"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::{Discovery, Flavor, NativeCompletionBackend, discover, tests::Dir};
    use super::*;
    use std::time::Duration;

    const APP: &str = r#"#!/bin/sh
root=$(dirname "$0")
[ "$1" = completion ] || { touch "$root/operation-marker"; exit 9; }
shift
while [ "${1#--prefix=}" != "$1" ]; do shift; done
[ "$1" = -- ] || { touch "$root/operation-marker"; exit 9; }
shift
[ "$npm_config_offline" = true ] && [ "$npm_config_ignore_scripts" = true ] || exit 8
[ "$npm_config_logs_max" = 0 ] && [ "$npm_config_timing" = false ] || exit 8
case "$npm_config_userconfig" in */disabled-user.npmrc) ;; *) exit 8;; esac
case "$npm_config_globalconfig" in */disabled-global.npmrc) ;; *) exit 8;; esac
[ "$HTTPS_PROXY" = http://127.0.0.1:9 ] || exit 8
[ -z "$COMP_FISH" ] && [ -z "$NPM_CONFIG_OFFLINE" ] || exit 8
printf '%s|%s|%s\n' "$COMP_CWORD" "$COMP_POINT" "$COMP_LINE" >> "$root/queries"
for arg in "$@"; do printf '<%s>' "$arg" >> "$root/argv"; done
printf '\n' >> "$root/argv"
if [ -f "$root/result-mode" ]; then
  case "$(cat "$root/result-mode")" in
    failed) printf 'publish\n'; exit 2;;
    utf8) printf '\377'; exit 0;;
    oversized) printf '%8192s' x; exit 0;;
    hanging) (sleep 1; touch "$root/delayed-marker") & wait; exit 0;;
  esac
fi
last=
for arg in "$@"; do last=$arg; done
if [ "$last" = "'--'" ]; then printf '%s\n' --prefix --ignore-scripts --dry-run; exit; fi
for arg in "$@"; do
  case "$arg" in
    "'config'") printf 'get\nlist\nset\n'; exit;;
    "'run'") printf 'build\nreport package\n'; exit;;
  esac
done
cat "$root/root-commands"
"#;

    fn app(dir: &Dir) -> std::path::PathBuf {
        dir.script(
            "node_modules/npm/package.json",
            r#"{"name":"npm","bin":{"npm":"bin/npm-cli.js","npx":"bin/npx-cli.js"}}"#,
        );
        dir.script(
            "node_modules/npm/bin/root-commands",
            "config\nrun\npublish\ninstall\n",
        );
        dir.script("node_modules/npm/bin/npm-cli.js", APP)
    }

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(5), Duration::from_secs(2), 16)
    }

    #[test]
    fn verifies_the_declared_npm_bin_and_trust_without_probing_npx_or_lookalikes() {
        let dir = Dir::new("npm-discovery");
        let path = app(&dir);
        assert!(matches!(
            discover("package-tool", &path, &[]),
            Discovery::NotTrusted(_)
        ));
        let Discovery::Found(backend) = discover("package-tool", &path, &["npm:npm".into()]) else {
            panic!()
        };
        assert_eq!(backend.flavor, Flavor::Npm);
        assert_eq!(backend.identity.as_deref(), Some("npm:npm"));
        assert!(backend.capabilities_for(&[]).complete_subcommands);
        assert!(!backend.capabilities_for(&["config"]).complete_subcommands);
        assert!(!backend.capabilities().complete_options);
        assert!(!backend.capabilities().short_options);
        let npx = dir.script(
            "node_modules/npm/bin/npx-cli.js",
            "#!/bin/sh\ntouch operation-marker\n",
        );
        assert_eq!(super::super::package_manager_name(&npx), Some("npx"));
        assert!(matches!(
            discover("npm", &npx, &["*".into()]),
            Discovery::Unavailable(_)
        ));
        let unknown = dir.script(
            "node_modules/npm/bin/other",
            "#!/bin/sh\ntouch operation-marker\n",
        );
        assert!(matches!(
            discover("npm", &unknown, &["*".into()]),
            Discovery::Unavailable(_)
        ));
        assert!(!path.parent().unwrap().join("queries").exists());
    }

    #[test]
    fn preserves_literal_words_utf16_offsets_local_resources_and_request_only_memoization() {
        let dir = Dir::new("npm-input");
        let path = app(&dir);
        let root = path.parent().unwrap().to_owned();
        let mut backend = Backend::new(Flavor::Npm, "package-tool", path.clone());
        backend.network = true;
        let mut budget = budget();
        assert_eq!(
            complete(&backend, &[], "", &mut budget).unwrap()[2].value,
            "publish"
        );
        let items = complete(&backend, &["run"], "", &mut budget).unwrap();
        assert_eq!(items[1].value, "report package");
        assert!(backend.candidates_are_resources(&["run"]));
        let options = complete(&backend, &["run"], "--", &mut budget).unwrap();
        assert_eq!(options[0].value, "--prefix");
        assert!(options.iter().all(|item| item.takes_value.is_none()));
        assert!(!backend.candidates_are_resources(&["config"]));
        assert!(!backend.candidates_are_resources(&["--prefix", "run"]));
        let words = [
            "--prefix",
            "a b😀'c\\ d$(touch operation-marker)",
            "run",
            "",
        ];
        complete(&backend, &words, "", &mut budget).unwrap();
        let argv = std::fs::read_to_string(root.join("argv")).unwrap();
        assert!(
            argv.contains("<'a b😀'c\\ d$(touch operation-marker)'><'run'><''><''>"),
            "{argv}"
        );
        let line = "'package-tool' '--prefix' 'a b😀'c\\ d$(touch operation-marker)' 'run' '' ''";
        assert!(
            std::fs::read_to_string(root.join("queries"))
                .unwrap()
                .contains(&format!("5|{}|{line}", line.encode_utf16().count()))
        );
        let spawned = budget.spawned();
        complete(&backend, &words, "", &mut budget).unwrap();
        assert_eq!(budget.spawned(), spawned);
        assert_eq!(backend.cache_identity(), None);
        assert!(matches!(
            backend.complete_values(&["--prefix"], &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
        dir.script(
            "node_modules/npm/bin/root-commands",
            "config\nrun\npublish\ninstall\nnew-command\n",
        );
        let updated = Backend::new(Flavor::Npm, "package-tool", path);
        assert!(
            complete(&updated, &[], "", &mut budget)
                .unwrap()
                .iter()
                .any(|item| item.value == "new-command")
        );
        assert!(!root.join("operation-marker").exists());
    }

    #[test]
    fn offline_overrides_and_malformed_contexts_abstain_before_any_spawn() {
        let dir = Dir::new("npm-offline");
        let path = app(&dir);
        let backend = Backend::new(Flavor::Npm, "npm", path);
        let mut budget = budget();
        for words in [
            vec!["view", "--offline=false"],
            vec!["view", "--of=false"],
            vec!["--proxy", "http://example.invalid", "view"],
            vec!["--no-ignore-scripts", "run"],
            vec!["--logs-max", "10"],
            vec!["--workspace", "project", "run"],
            vec!["run", "line\nbreak"],
        ] {
            assert!(matches!(
                complete(&backend, &words, "", &mut budget),
                Err(CompletionError::Failed(_) | CompletionError::Unsupported(_))
            ));
        }
        assert_eq!(budget.spawned(), 0);
    }

    #[test]
    fn entry_point_changes_are_rechecked_before_a_query() {
        let dir = Dir::new("npm-revalidation");
        let path = app(&dir);
        let backend = Backend::new(Flavor::Npm, "npm", path);
        dir.script("node_modules/npm/bin/new-cli.js", APP);
        dir.script(
            "node_modules/npm/package.json",
            r#"{"name":"npm","bin":{"npm":"bin/new-cli.js","npx":"bin/npm-cli.js"}}"#,
        );
        let mut budget = budget();
        assert!(matches!(
            complete(&backend, &[], "", &mut budget),
            Err(CompletionError::Failed(_))
        ));
        assert_eq!(budget.spawned(), 0);
    }

    #[test]
    fn failures_output_limits_and_timeout_fail_closed() {
        let dir = Dir::new("npm-failures");
        let path = app(&dir);
        for (mode, limit, reason) in [
            ("failed", 1024, "exited"),
            ("utf8", 1024, "UTF-8"),
            ("oversized", 64, "limit"),
            ("hanging", 1024, "timed out"),
        ] {
            dir.script("node_modules/npm/bin/result-mode", mode);
            let backend = Backend::new(Flavor::Npm, "npm", path.clone());
            let timeout = if mode == "hanging" {
                Duration::from_millis(250)
            } else {
                Duration::from_secs(2)
            };
            let mut budget = Budget::new(Duration::from_secs(3), timeout, 1);
            budget.max_output = limit;
            let result = complete(&backend, &[], "", &mut budget);
            assert!(
                matches!(&result, Err(CompletionError::Failed(why)) if why.contains(reason)),
                "{mode}: {result:?}"
            );
        }
        std::thread::sleep(Duration::from_millis(1050));
        assert!(!path.parent().unwrap().join("delayed-marker").exists());
        let backend = Backend::new(Flavor::Npm, "npm", path);
        let mut budget = Budget::new(Duration::ZERO, Duration::ZERO, 0);
        assert_eq!(
            complete(&backend, &[], "", &mut budget),
            Err(CompletionError::BudgetExhausted)
        );
    }
}
