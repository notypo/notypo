//! go-flags enters completion before setting arguments, defaults, or
//! environment-backed options and before dispatching Commander.Execute.
//! `GO_FLAGS_COMPLETION=1` prints labels verbatim; `verbose` adds padded
//! descriptions when there is more than one label. Query both formats to
//! retain spaces and `  # ` inside a label without guessing a delimiter.
//!
//! Completion can still call default MarshalFlag methods and value
//! Completer callbacks. A custom CompletionHandler returns to the caller
//! instead of exiting, so code after Parse can run too. This generic bridge
//! requires both trust settings; it is not an audit or a sandbox. Native
//! callbacks run in the failed command's directory with literal arguments.

use super::{Backend, Capabilities, CompletionError, CompletionItem, Flavor, Trust};
use crate::engine::cache::fingerprint;
use crate::engine::probe::{self, Budget, Capture, Probe};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(super) const LIBRARY: &str = "github.com/jessevdk/go-flags";

#[derive(Clone, Debug, Default)]
pub(super) struct Protocol {
    pub trusted_help: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Slash options with `:` values. Discovery sets it for Windows builds
    /// without the forceposix tag.
    pub windows: bool,
    installation: Option<String>,
}

fn installation(path: &Path) -> String {
    let mut files = vec![path.to_owned()];
    files.extend(std::fs::canonicalize(path).ok());
    fingerprint(&files, &["go-flags"])
}

impl Protocol {
    pub fn identify(&mut self, path: &Path) {
        self.installation = Some(installation(path));
    }

    pub fn allowed(&self, backend: &Backend) -> bool {
        super::identity::is_trusted(
            &self.trusted_help,
            &backend.name,
            backend.identity.as_deref(),
        )
    }

    pub fn capabilities(&self, trust: Trust) -> Capabilities {
        Capabilities {
            subcommands: true,
            options: true,
            option_arity: false,
            values: true,
            // Every query uses the offline environment, even with network
            // completion enabled. The wire format supplies no value kind.
            resources: false,
            option_prefix: if self.windows { "/" } else { "-" },
            // Short aliases of long options and hidden items are omitted.
            short_options: false,
            complete_options: false,
            complete_subcommands: false,
            descriptions: true,
            query_dialect: None,
            trust,
        }
    }

    pub fn is_option(&self, word: &str) -> bool {
        word.starts_with('-') || (self.windows && word.starts_with('/'))
    }

    pub fn is_short_option(&self, word: &str) -> bool {
        if self.windows && word.starts_with('/') {
            word.chars().count() == 2
        } else {
            word.len() > 1 && word.starts_with('-') && !word.starts_with("--")
        }
    }

    pub fn option_separator(&self, word: &str) -> Option<char> {
        (self.windows && word.len() > 1 && word.starts_with('/')).then_some(':')
    }

    pub fn option_prefix(&self, word: &str) -> &'static str {
        if self.windows && word.starts_with('/') {
            "/"
        } else if self.windows && word.starts_with("--") {
            "--"
        } else {
            "-"
        }
    }

    fn raw(
        &self,
        backend: &Backend,
        args: &[&str],
        mode: &str,
        budget: &mut Budget,
    ) -> Result<String, CompletionError> {
        if !self.allowed(backend) {
            return Err(CompletionError::Failed(format!(
                "go-flags completion can run app callbacks and code after a custom handler; add {} to trusted_help to allow it",
                backend.name
            )));
        }
        if self
            .installation
            .as_ref()
            .is_some_and(|known| *known != installation(&backend.completer))
        {
            return Err(CompletionError::Failed(
                "the go-flags application changed after discovery".into(),
            ));
        }
        let cwd = self
            .cwd
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .ok_or_else(|| CompletionError::Failed("no completion working directory".into()))?;
        let cwd_key = cwd.to_string_lossy();
        let mut key = vec!["go-flags", mode, &cwd_key];
        key.extend_from_slice(args);
        backend.memoized(&key, || {
            let mut env = super::offline_env(Flavor::GoFlags);
            env.push(("GO_FLAGS_COMPLETION".into(), Some(mode.into())));
            let output = probe::run_in(
                &Probe {
                    program: &backend.completer,
                    args: args.iter().map(Into::into).collect(),
                    env,
                    capture: Capture::Stdout,
                },
                &cwd,
                budget,
            )
            .map_err(super::probe_error)?;
            if output.status != Some(0) {
                return Err(CompletionError::Failed(format!(
                    "go-flags completion exited with {:?}",
                    output.status
                )));
            }
            if output.truncated {
                return Err(CompletionError::Failed(
                    "go-flags completion output exceeded the probe limit".into(),
                ));
            }
            String::from_utf8(output.data).map_err(|_| {
                CompletionError::Failed("go-flags completion output is not valid UTF-8".into())
            })
        })
    }

    fn labels(text: &str, limit: usize) -> Result<Vec<&str>, CompletionError> {
        if text.is_empty() {
            return Ok(Vec::new());
        }
        if !text.ends_with('\n') {
            return Err(CompletionError::Failed(
                "go-flags completion returned an unterminated label".into(),
            ));
        }
        let mut distinct = HashSet::new();
        let mut labels = Vec::new();
        for label in text.split_terminator('\n') {
            if label.is_empty() || label.contains(char::is_control) {
                return Err(CompletionError::Failed(
                    "go-flags completion returned an empty label or control characters".into(),
                ));
            }
            distinct.insert(label);
            if distinct.len() > limit {
                return Err(super::over_limit());
            }
            labels.push(label);
        }
        Ok(labels)
    }

    fn decode(
        labels: &[&str],
        verbose: Option<&str>,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let lines: Vec<&str> = verbose
            .map(|text| text.split_terminator('\n').collect())
            .unwrap_or_else(|| labels.to_vec());
        if lines.len() != labels.len() || verbose.is_some_and(|text| !text.ends_with('\n')) {
            return Err(CompletionError::Failed(
                "go-flags plain and verbose answers disagree".into(),
            ));
        }
        let width = labels.iter().map(|label| label.len()).max().unwrap_or(0);
        let mut items: Vec<CompletionItem> = Vec::new();
        for (&label, line) in labels.iter().zip(lines) {
            let description = if line == label {
                None
            } else {
                let prefix = format!("{label}{}  # ", " ".repeat(width - label.len()));
                let description = line.strip_prefix(&prefix).ok_or_else(|| {
                    CompletionError::Failed("go-flags plain and verbose answers disagree".into())
                })?;
                super::clean_description(description)
            };
            if items.iter().any(|item| item.value == label) {
                continue;
            }
            items.push(CompletionItem {
                value: label.to_owned(),
                takes_value: None,
                description,
            });
        }
        Ok(items)
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
                "go-flags completion context contains control characters".into(),
            ));
        }
        let mut args = words.to_vec();
        // Preserve the empty final argument: without it the parser completes
        // the preceding command/option instead of the next slot.
        args.push(prefix);
        let plain = self.raw(backend, &args, "1", budget)?;
        let labels = Self::labels(&plain, budget.max_candidates)?;
        let verbose = if labels.len() > 1 {
            Some(self.raw(backend, &args, "verbose", budget)?)
        } else {
            None
        };
        let mut items = Self::decode(&labels, verbose.as_deref())?;
        if self.windows && matches!(prefix, "-" | "--") {
            // Windows completion prints its canonical slash names even when
            // queried with --. Keep the user's long-option spelling.
            for item in &mut items {
                if let Some(name) = item.value.strip_prefix('/') {
                    if name.chars().count() > 1 {
                        item.value = format!("--{name}");
                    } else {
                        item.value = format!("-{name}");
                    }
                }
            }
        }
        Ok(items)
    }

    pub fn confirms(
        &self,
        backend: &Backend,
        words: &[&str],
        typed: &str,
        budget: &mut Budget,
    ) -> Option<bool> {
        if typed.is_empty() || self.is_option(typed) || typed.contains(char::is_control) {
            return None;
        }
        let parent = self.complete(backend, words, "", budget).ok()?;
        if parent.is_empty() {
            return None;
        }
        let mut child = words.to_vec();
        child.push(typed);
        let answer = self.complete(backend, &child, "", budget).ok()?;
        // go-flags accepts command aliases but lists only canonical names.
        // Unknown command words leave the level unchanged. An unchanged
        // answer is inconclusive, since two real levels can look alike.
        (answer != parent).then_some(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_formats_preserve_spaces_hashes_unicode_and_sanitize_descriptions() {
        let plain = "é\ntwo  # words\nyaml\n";
        let labels = Protocol::labels(plain, 3).unwrap();
        let verbose = "é            # Unicode\ntwo  # words  # A\tformat\nyaml          # YAML\n";
        let items = Protocol::decode(&labels, Some(verbose)).unwrap();
        assert_eq!(items[1].value, "two  # words");
        assert_eq!(items[1].description.as_deref(), Some("A format"));
        assert_eq!(items[0].description.as_deref(), Some("Unicode"));
        assert!(items.iter().all(|item| item.takes_value.is_none()));
        let singleton = Protocol::decode(&["two  # words"], None).unwrap();
        assert_eq!(singleton[0].value, "two  # words");
        assert_eq!(singleton[0].description, None);
    }

    #[test]
    fn malformed_changed_and_oversized_answers_fail_as_a_whole() {
        for text in ["missing-newline", "x\n\n", "x\ty\n", "\x1b[31mx\n"] {
            assert!(Protocol::labels(text, 10).is_err(), "{text:?}");
        }
        assert!(Protocol::labels("x\ny\n", 1).is_err());
        assert!(Protocol::labels("x\nx\n", 1).is_ok());
        for text in ["x\nz\n", "x  # description\n", "x\ny", "x\ny\nz\n"] {
            assert!(
                Protocol::decode(&["x", "y"], Some(text)).is_err(),
                "{text:?}"
            );
        }
    }

    #[test]
    fn windows_slash_syntax_and_forceposix_do_not_treat_paths_as_posix_options() {
        let windows = Protocol {
            windows: true,
            ..Protocol::default()
        };
        assert_eq!(windows.option_separator("/format:json"), Some(':'));
        assert_eq!(windows.option_prefix("--format"), "--");
        assert!(!windows.is_short_option("/format"));
        assert!(windows.is_short_option("/f"));
        let posix = Protocol {
            windows: false,
            ..Protocol::default()
        };
        assert!(!posix.is_option("/file"));
        assert_eq!(posix.option_separator("/file"), None);
    }
}

#[cfg(all(test, unix))]
mod protocol_tests {
    use super::*;
    use crate::engine::native::{
        Discovery, NativeCompletionBackend, discover, discover_for_shell_with_budget, go,
        tests::Dir,
    };
    use crate::shells::Shell;
    use std::time::Duration;

    const MODULE: &str = "example.com/go-flags-fixture";
    const APP: &str = r#"#!/bin/sh
root=$(dirname "$0")
[ "$HTTPS_PROXY" = http://127.0.0.1:9 ] && [ "$KUBECONFIG" = /dev/null ] || exit 8
[ -n "$GO_FLAGS_COMPLETION" ] || { touch "$root/operation-marker"; exit 9; }
printf '%s|' "$GO_FLAGS_COMPLETION" >> "$root/queries"
printf '<%s>' "$@" >> "$root/queries"
printf '|%s\n' "$PWD" >> "$root/queries"
if [ -f "$root/mode" ]; then
  case "$(cat "$root/mode")" in
    failed) exit 1;;
    utf8) printf '\377\n'; exit 0;;
    truncated) printf 'one\ntwo\nthree\n'; exit 0;;
    changed) [ "$GO_FLAGS_COMPLETION" != verbose ] || { printf 'wrong\n'; exit 0; };;
  esac
fi
case "$*" in
  'list -'|'ls -') result=flags;;
  'nodes list ') exit 0;;
  # Like go-flags, an unknown word keeps its parent level's answer.
  'nodes '*) result=nested;;
  'list --format '|'ls -f '|*'--server two words --format ') result=values;;
  'list '| 'ls ') exit 0;;
  *) result=root;;
esac
suffix=plain
[ "$GO_FLAGS_COMPLETION" != verbose ] || suffix=verbose
cat "$root/$result-$suffix"
exit 0
"#;

    fn backend(dir: &Dir, help: &[&str]) -> Backend {
        let path = dir.script("bin/fixture", APP);
        for (name, plain, verbose) in [
            (
                "root",
                "list\nnodes\n",
                "list   # List resources\nnodes  # Manage nodes\n",
            ),
            ("nested", "list\n", "list\n"),
            (
                "flags",
                "--format\n--verbose\n",
                "--format   # Output format\n--verbose  # Verbose logging\n",
            ),
            (
                "values",
                "json\ntwo words\nyaml\n",
                "json       # Format\ntwo words  # Format\nyaml       # Format\n",
            ),
        ] {
            dir.script(&format!("bin/{name}-plain"), plain);
            dir.script(&format!("bin/{name}-verbose"), verbose);
        }
        let mut backend = Backend::new(Flavor::GoFlags, "fixture", path);
        backend.identity = Some(MODULE.into());
        let protocol = backend.go_flags.as_mut().unwrap();
        protocol.trusted_help = help.iter().map(|word| word.to_string()).collect();
        protocol.cwd = Some(dir.0.clone());
        protocol.windows = false;
        backend
    }

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(10), Duration::from_secs(2), 16)
    }

    #[test]
    fn commands_options_aliases_and_callback_values_keep_native_context_and_descriptions() {
        let dir = Dir::new("go-flags-context");
        let backend = backend(&dir, &[MODULE]);
        let mut budget = budget();
        let items = backend.complete(&[], "", &mut budget).unwrap();
        assert_eq!(items[0].value, "list");
        assert_eq!(items[0].description.as_deref(), Some("List resources"));
        assert_eq!(
            backend.complete(&["nodes"], "", &mut budget).unwrap()[0].value,
            "list"
        );
        assert_eq!(
            backend.complete(&["ls"], "-", &mut budget).unwrap()[0].value,
            "--format"
        );
        assert_eq!(backend.confirms(&[], "ls", &mut budget), Some(true));
        assert_eq!(backend.confirms(&[], "unknown", &mut budget), None);
        let values = backend.complete_values(&["ls", "-f"], &mut budget).unwrap();
        assert_eq!(values[1].value, "two words");
        assert!(backend.candidate_is_resource(&["ls", "-f"], &values[1]));
        assert_eq!(backend.cache_identity(), None);
        let capabilities = backend.capabilities();
        assert!(capabilities.values && capabilities.descriptions);
        assert!(
            !capabilities.option_arity
                && !capabilities.complete_options
                && !capabilities.short_options
        );
        assert!(!capabilities.complete_subcommands && !capabilities.resources);
        assert!(!dir.0.join("bin/operation-marker").exists());
    }

    #[test]
    fn literal_arguments_empty_cursor_cwd_and_request_reuse_survive_without_a_shell() {
        let dir = Dir::new("go-flags-literals");
        let mut backend = backend(&dir, &["fixture"]);
        backend.network = true;
        let mut budget = budget();
        let words = [
            "list",
            "--config",
            "$(touch operation-marker)",
            "--server",
            "two words",
            "--format",
        ];
        let answer = backend.complete_values(&words, &mut budget).unwrap();
        assert_eq!(answer[0].value, "json");
        let spawned = budget.spawned();
        assert_eq!(
            backend
                .complete_offline_values(&words, &mut budget)
                .unwrap(),
            answer
        );
        assert_eq!(budget.spawned(), spawned);
        assert_eq!(spawned, 2);
        let queries = std::fs::read_to_string(dir.0.join("bin/queries")).unwrap();
        assert!(queries.contains("<$(touch operation-marker)><--server><two words><--format><>"));
        // The shell reports the physical directory (macOS's /var is a link).
        let cwd = std::fs::canonicalize(&dir.0).unwrap();
        assert!(
            queries
                .lines()
                .all(|line| line.ends_with(&format!("|{}", cwd.display())))
        );
        assert!(!dir.0.join("operation-marker").exists());
        let mut next_request = backend.clone();
        next_request.memo = Default::default();
        next_request.complete_values(&words, &mut budget).unwrap();
        assert_eq!(budget.spawned(), spawned + 2);
    }

    #[test]
    fn trust_invalid_context_changed_binary_and_probe_limits_fail_before_becoming_answers() {
        let dir = Dir::new("go-flags-gates");
        let mut backend = backend(&dir, &[]);
        let mut budget = budget();
        assert!(
            matches!(backend.complete(&[], "", &mut budget), Err(CompletionError::Failed(why)) if why.contains("trusted_help"))
        );
        assert_eq!(budget.spawned(), 0);
        backend.go_flags.as_mut().unwrap().trusted_help = vec!["*".into()];
        assert!(matches!(
            backend.complete(&["a\nb"], "", &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
        assert_eq!(budget.spawned(), 0);
        backend
            .go_flags
            .as_mut()
            .unwrap()
            .identify(&backend.completer);
        std::fs::write(&backend.completer, "changed application").unwrap();
        assert!(
            matches!(backend.complete(&[], "", &mut budget), Err(CompletionError::Failed(why)) if why.contains("changed after discovery"))
        );
        assert_eq!(budget.spawned(), 0);
        for mode in ["failed", "utf8", "truncated", "changed"] {
            let dir = Dir::new("go-flags-failure");
            let backend = self::backend(&dir, &["fixture"]);
            dir.script("bin/mode", mode);
            let mut budget = self::budget();
            if mode == "truncated" {
                budget.max_output = 4;
            }
            assert!(
                matches!(
                    backend.complete(&[], "", &mut budget),
                    Err(CompletionError::Failed(_))
                ),
                "{mode}"
            );
        }
        let dir = Dir::new("go-flags-count");
        let backend = self::backend(&dir, &["fixture"]);
        let mut budget = self::budget();
        budget.max_candidates = 1;
        assert!(matches!(
            backend.complete(&[], "", &mut budget),
            Err(CompletionError::Failed(_))
        ));
        assert_eq!(budget.spawned(), 1);
        let mut budget = Budget::new(Duration::from_secs(10), Duration::from_secs(2), 0);
        assert_eq!(
            backend.complete(&["nodes"], "", &mut budget),
            Err(CompletionError::BudgetExhausted)
        );
    }

    #[test]
    fn the_engine_repairs_commands_options_and_values_and_gates_unlabeled_words() {
        use crate::engine::{self, FailureContext, safety::Decision};
        use crate::settings::Settings;
        use crate::types::Context;
        let dir = Dir::new("go-flags-engine");
        let backend = backend(&dir, &[MODULE]);
        let path = backend.completer.clone();
        let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into())
            .with_which("fixture", Some(path.to_str().unwrap()))
            .with_which("man", None)
            .with_history::<&str>(&[]);
        let first = |source: &str| {
            let backend: std::rc::Rc<dyn NativeCompletionBackend> =
                std::rc::Rc::new(backend.clone());
            let failure = FailureContext {
                source: source.into(),
                ..FailureContext::default()
            };
            let report =
                engine::correct_with_backends(&failure, &ctx, vec![("fixture".into(), backend)]);
            report.outcome.candidates().first().cloned()
        };
        let option = first("fixture list --fromat json").unwrap();
        assert_eq!(option.script, "fixture list --format json");
        assert!(option.edits.iter().all(|edit| !edit.resource));
        assert_eq!(option.safety.decision, Decision::Allow);
        assert_eq!(
            first("fixture ls --verbsoe").unwrap().script,
            "fixture ls --verbose"
        );
        // Commands, aliases, and callback values share one unlabeled format,
        // so a changed word is treated as a resource and needs approval.
        for (source, script) in [
            ("fixture lsit", "fixture list"),
            ("fixture nodes lisst", "fixture nodes list"),
            ("fixture list --format jsno", "fixture list --format json"),
        ] {
            let candidate = first(source).unwrap();
            assert_eq!(candidate.script, script);
            assert!(candidate.edits.iter().all(|edit| edit.resource));
            assert_eq!(candidate.safety.decision, Decision::Confirm, "{source}");
        }
        assert!(!dir.0.join("bin/operation-marker").exists());
    }

    #[test]
    fn upgrades_are_answered_by_the_installed_app_on_the_next_request() {
        let dir = Dir::new("go-flags-upgrade");
        let before = backend(&dir, &[MODULE]);
        let mut budget = budget();
        let names = |items: Vec<CompletionItem>| -> Vec<String> {
            items.into_iter().map(|item| item.value).collect()
        };
        let listed = names(before.complete(&[], "", &mut budget).unwrap());
        assert!(!listed.contains(&"rollback".to_owned()));
        dir.script("bin/root-plain", "list\nnodes\nrollback\n");
        dir.script(
            "bin/root-verbose",
            "list      # List resources\nnodes     # Manage nodes\nrollback  # Undo\n",
        );
        let mut after = before.clone();
        after.memo = Default::default();
        let listed = after.complete(&[], "", &mut budget).unwrap();
        assert_eq!(listed[2].value, "rollback");
        assert_eq!(listed[2].description.as_deref(), Some("Undo"));
    }

    #[test]
    fn discovery_uses_module_identity_both_trust_settings_and_library_ambiguity() {
        let dir = Dir::new("go-flags-discovery");
        let path = dir.0.join("fixture");
        let binary =
            |lines: &str| std::fs::write(&path, go::fake_binary_info(MODULE, lines)).unwrap();
        binary("dep\tgithub.com/jessevdk/go-flags\tv1.6.1\th1:x\n");
        assert!(
            matches!(discover("fixture", &path, &[]), Discovery::NotTrusted(why) if why.contains("go-flags"))
        );
        let trusted = [MODULE.to_owned()];
        let mut budget = budget();
        assert!(
            matches!(discover_for_shell_with_budget("fixture", &path, &trusted, &[], Shell::Bash, &mut budget), Discovery::NotTrusted(why) if why.contains("trusted_help"))
        );
        assert!(
            matches!(discover_for_shell_with_budget("renamed", &path, &trusted, &trusted, Shell::Bash, &mut budget), Discovery::Found(backend) if backend.flavor == Flavor::GoFlags && backend.identity.as_deref() == Some(MODULE))
        );
        assert_eq!(budget.spawned(), 0);
        for library in [
            "github.com/spf13/cobra",
            "github.com/posener/complete",
            "github.com/urfave/cli/v2",
            "github.com/alecthomas/kingpin/v2",
        ] {
            binary(&format!(
                "dep\tgithub.com/jessevdk/go-flags\tv1.6.1\th1:x\ndep\t{library}\tv2.0.0\th1:x\n"
            ));
            assert!(
                matches!(discover("fixture", &path, &["*".into()]), Discovery::Unavailable(why) if why.contains("go-flags")),
                "{library}"
            );
        }
    }
}
