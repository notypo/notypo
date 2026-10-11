//! pip's completion mode exits before normal command dispatch. Its input is
//! whitespace-split COMP_WORDS, not a shell command: quoting does not preserve
//! embedded whitespace or empty arguments. Refuse those contexts rather than
//! change their meaning. Never pass the failed operation as process arguments.
//!
//! Root commands are space-separated; deeper answers are one item per line
//! and can contain filesystem or installed-distribution names. Learn the root
//! commands from pip itself to distinguish these formats without a catalog.

use super::{Backend, CompletionError, CompletionItem};
use crate::engine::probe::{self, Budget, Capture, Probe};

fn raw(
    backend: &Backend,
    words: &[&str],
    prefix: &str,
    budget: &mut Budget,
) -> Result<String, CompletionError> {
    if backend.name.is_empty()
        || backend.name.contains(char::is_whitespace)
        || words
            .iter()
            .any(|word| word.is_empty() || word.contains(char::is_whitespace))
        || prefix.contains(char::is_whitespace)
    {
        return Err(CompletionError::Unsupported(
            "pip's COMP_WORDS protocol cannot preserve whitespace or empty arguments".into(),
        ));
    }
    let mut key = vec!["pip"];
    key.extend_from_slice(words);
    key.push(prefix);
    backend.memoized(&key, || {
        let mut command = vec![backend.name.as_str()];
        command.extend_from_slice(words);
        command.push(prefix);
        let mut env = super::offline_env(super::Flavor::Pip);
        env.extend([
            ("PIP_AUTO_COMPLETE".into(), Some("1".into())),
            ("COMP_WORDS".into(), Some(command.join(" ").into())),
            (
                "COMP_CWORD".into(),
                Some((words.len() + 1).to_string().into()),
            ),
        ]);
        let output = probe::run(
            &Probe {
                program: &backend.completer,
                args: Vec::new(),
                env,
                capture: Capture::Stdout,
            },
            budget,
        )
        .map_err(super::probe_error)?;
        // pip deliberately exits 1 on every completed request, including an
        // empty answer. Zero is not evidence that completion mode was entered.
        if output.status != Some(1) {
            return Err(CompletionError::Failed(format!(
                "pip completion exited with {:?}, expected 1",
                output.status
            )));
        }
        if output.truncated {
            return Err(CompletionError::Failed(
                "pip completion output exceeded the probe limit".into(),
            ));
        }
        String::from_utf8(output.data)
            .map_err(|_| CompletionError::Failed("pip completion output is not valid UTF-8".into()))
    })
}

pub(super) fn complete(
    backend: &Backend,
    words: &[&str],
    prefix: &str,
    budget: &mut Budget,
) -> Result<Vec<CompletionItem>, CompletionError> {
    // Reject an unrepresentable request before even enumerating the root.
    let answer = raw(backend, words, prefix, budget)?;
    let root = if words.is_empty() {
        None
    } else {
        Some(raw(backend, &[], "", budget)?)
    };
    let nested = root.as_deref().is_some_and(|root| {
        words
            .iter()
            .any(|word| root.split_whitespace().any(|cmd| cmd == *word))
    });
    let entries: Vec<&str> = if nested {
        answer.lines().collect()
    } else {
        answer.split_whitespace().collect()
    };
    let mut items = Vec::new();
    for entry in entries {
        if items.len() >= budget.max_candidates {
            break;
        }
        if entry.is_empty() || entry.contains(char::is_control) {
            continue;
        }
        let (value, takes_value) = match entry.strip_suffix('=') {
            Some(name) if name.starts_with('-') => (name, Some(true)),
            _ if nested && entry.starts_with("--") => (entry, Some(false)),
            _ => (entry, None),
        };
        if value.starts_with(prefix)
            && !items
                .iter()
                .any(|item: &CompletionItem| item.value == value)
        {
            items.push(CompletionItem {
                value: value.to_owned(),
                takes_value,
                description: None,
            });
        }
    }
    Ok(items)
}

pub(super) fn resources(backend: &Backend, words: &[&str]) -> bool {
    let memo = backend.memo.borrow();
    let key = vec!["pip".to_owned(), String::new()];
    let Some(Ok(root)) = memo.get(&key) else {
        return false;
    };
    let command = words
        .iter()
        .find(|word| root.split_whitespace().any(|cmd| cmd == **word));
    // These pip completion branches enumerate installed distributions or
    // local paths. This classifies their answers; it supplies no candidates.
    matches!(command, Some(&"show" | &"uninstall" | &"install"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::tests::Dir;
    use super::super::{Discovery, Flavor, NativeCompletionBackend, discover};
    use super::*;
    use std::time::Duration;

    const APP: &str = r#"#!/bin/sh
root=$(dirname "$0")
[ "$1" = -m ] && [ "$2" = pip ] || exit 8
shift 2
printf '[%s]' "$@" >> "$root/calls"
printf '%s|%s\n' "$COMP_CWORD" "$COMP_WORDS" >> "$root/calls"
[ "$#" = 0 ] && [ "$PIP_AUTO_COMPLETE" = 1 ] || { touch "$root/operation-marker"; exit 8; }
[ "$PIP_NO_INDEX" = 1 ] && [ "$PIP_NO_INPUT" = 1 ] || exit 8
[ "$PIP_DISABLE_PIP_VERSION_CHECK" = 1 ] && [ "$PIP_CONFIG_FILE" = /dev/null ] || exit 8
[ "$HTTPS_PROXY" = http://127.0.0.1:9 ] || exit 8
if [ -f "$root/result-mode" ]; then
  case "$(cat "$root/result-mode")" in
    zero) printf 'install\n'; exit 0;;
    failed) printf 'install\n'; exit 2;;
    utf8) printf '\377'; exit 1;;
    oversized) printf '%8192s' x; exit 1;;
    hanging) (sleep 1; touch "$root/delayed-marker") & wait; exit 1;;
  esac
fi
case "$COMP_WORDS" in
  *' config '*) printf 'debug\nget\nlist\n';;
  *' show '*) printf 'example-package\nreport package\n';;
  *' install -') printf '%s\n' '--dry-run' '--index-url=' '--target=' '-t';;
  *' install '*|*' help '*) ;;
  *' -') printf '%s\n' '--help --timeout -h';;
  *) cat "$root/root-commands";;
esac
exit 1
"#;

    fn app(dir: &Dir, name: &str) -> std::path::PathBuf {
        dir.script("python3", APP);
        dir.script("root-commands", "config install show help\n");
        dir.script(
            name,
            &format!(
                "#!/bin/sh\nexec {}/python3 -m pip \"$@\"\n",
                dir.0.display()
            ),
        )
    }

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(5), Duration::from_secs(2), 16)
    }

    #[test]
    fn discovers_versioned_and_renamed_launchers_by_identity_and_explicit_trust() {
        let dir = Dir::new("pip-identity");
        for name in ["pip3.14", "package-tool"] {
            let path = app(&dir, name);
            assert!(matches!(
                discover(name, &path, &[]),
                Discovery::NotTrusted(_)
            ));
            let Discovery::Found(backend) = discover(name, &path, &["python:pip".into()]) else {
                panic!()
            };
            assert_eq!(backend.flavor, Flavor::Pip);
            assert_eq!(backend.identity.as_deref(), Some("python:pip"));
            assert!(backend.capabilities_for(&[]).complete_subcommands);
            assert!(!backend.capabilities_for(&["config"]).complete_subcommands);
            assert!(!backend.capabilities().complete_options);
        }
        let unrelated = dir.script("pip", "#!/bin/sh\ntouch operation-marker\n");
        assert!(!matches!(
            discover("pip", &unrelated, &["pip".into()]),
            Discovery::Found(Backend {
                flavor: Flavor::Pip,
                ..
            })
        ));
        assert!(!dir.0.join("calls").exists());
    }

    #[test]
    fn enumerates_commands_nested_options_and_local_values_without_dispatch_or_disk_cache() {
        let dir = Dir::new("pip-protocol");
        let path = app(&dir, "package-tool");
        let mut backend = Backend::new(Flavor::Pip, "package-tool", path);
        backend.network = true;
        let mut budget = budget();
        assert_eq!(
            complete(&backend, &[], "", &mut budget).unwrap()[1].value,
            "install"
        );
        let options = complete(&backend, &["install"], "-", &mut budget).unwrap();
        assert_eq!(options[0].takes_value, Some(false));
        assert_eq!(options[1].value, "--index-url");
        assert_eq!(options[1].takes_value, Some(true));
        assert_eq!(options[3].value, "-t");
        assert_eq!(
            options[3].takes_value, None,
            "pip marks only long valued options with '='"
        );
        assert!(!backend.capabilities().values);
        assert!(!backend.capabilities().resources);
        assert!(matches!(
            backend.complete_values(&["config", "--keyring-provider"], &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
        let values = complete(&backend, &["show"], "", &mut budget).unwrap();
        assert_eq!(
            values[1].value, "report package",
            "one value, not two commands"
        );
        assert!(backend.candidates_are_resources(&["show"]));
        assert!(!backend.candidates_are_resources(&["config", "show"]));
        assert!(
            complete(&backend, &["help"], "", &mut budget)
                .unwrap()
                .is_empty()
        );
        assert!(
            complete(
                &backend,
                &["install", "--target", "$(touch-marker)"],
                "-",
                &mut budget
            )
            .is_ok()
        );
        let spawned = budget.spawned();
        complete(&backend, &["install"], "-", &mut budget).unwrap();
        assert_eq!(budget.spawned(), spawned);
        assert!(
            std::fs::read_to_string(dir.0.join("calls"))
                .unwrap()
                .contains("$(touch-marker)")
        );
        assert_eq!(backend.cache_identity(), None);
        assert!(!dir.0.join("operation-marker").exists());
    }

    #[test]
    fn unrepresentable_words_are_never_probed_and_new_requests_use_new_command_metadata() {
        let dir = Dir::new("pip-input");
        let path = app(&dir, "pip3");
        let backend = Backend::new(Flavor::Pip, "pip3", path.clone());
        let mut budget = budget();
        for words in [
            vec!["show", "private target"],
            vec!["install", ""],
            vec!["install", "a\tb"],
        ] {
            assert!(matches!(
                complete(&backend, &words, "", &mut budget),
                Err(CompletionError::Unsupported(_))
            ));
        }
        assert_eq!(budget.spawned(), 0);
        complete(&backend, &[], "", &mut budget).unwrap();
        dir.script("root-commands", "config install show help new-command\n");
        let upgraded = Backend::new(Flavor::Pip, "pip3", path);
        assert!(
            complete(&upgraded, &[], "", &mut budget)
                .unwrap()
                .iter()
                .any(|item| item.value == "new-command")
        );
    }

    #[test]
    fn completion_exit_status_limits_and_timeout_fail_closed() {
        let dir = Dir::new("pip-failures");
        let path = app(&dir, "pip3");
        for (mode, limit, reason) in [
            ("zero", 1024, "expected 1"),
            ("failed", 1024, "expected 1"),
            ("utf8", 1024, "UTF-8"),
            ("oversized", 64, "limit"),
            ("hanging", 1024, "timed out"),
        ] {
            dir.script("result-mode", mode);
            let backend = Backend::new(Flavor::Pip, "pip3", path.clone());
            let timeout = if mode == "hanging" {
                Duration::from_millis(250)
            } else {
                Duration::from_secs(2)
            };
            let mut budget = Budget::new(Duration::from_secs(3), timeout, 1);
            budget.max_output = limit;
            let answer = complete(&backend, &[], "", &mut budget);
            assert!(
                matches!(&answer, Err(CompletionError::Failed(why)) if why.contains(reason)),
                "{mode}: {answer:?}"
            );
        }
        std::thread::sleep(Duration::from_millis(1050));
        assert!(!dir.0.join("delayed-marker").exists());
        let backend = Backend::new(Flavor::Pip, "pip3", path);
        let mut budget = Budget::new(Duration::ZERO, Duration::ZERO, 0);
        assert!(matches!(
            complete(&backend, &[], "", &mut budget),
            Err(CompletionError::BudgetExhausted)
        ));
        assert!(!dir.0.join("operation-marker").exists());
    }
}
