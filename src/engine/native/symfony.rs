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
//! Answering boots the application: Composer reads its configuration and
//! its global and project plugins; a framework console boots the project's
//! kernel. So the bridge needs
//! `trusted_help` as well as `trusted_completers`, is found from the app's
//! own help (a PHP script whose `--help` shows Symfony's global options),
//! passes `--no-scripts` when the help declares it, and keeps Composer
//! offline. Plugins remain available under that explicit trust. A console
//! inside the working directory is project code and follows workspace trust.

use super::{Backend, Capabilities, CompletionError, CompletionItem, Flavor, Trust, run_stdout};
use crate::engine::cache::fingerprint;
use crate::engine::probe::Budget;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Disable script execution when the application's help declares the flag.
/// Plugin and command initialization still follow the user's trust policy.
const GUARDS: [&str; 1] = ["--no-scripts"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Protocol {
    guards: Vec<&'static str>,
    global_options: Vec<CompletionItem>,
    /// The app may have been replaced since its help was read.
    application_fingerprint: String,
    /// `php artisan` uses the resolved interpreter, not the shebang.
    interpreter: Option<PathBuf>,
}

fn app_fingerprint(path: &Path, interpreter: Option<&Path>) -> String {
    let mut files = vec![path.to_owned()];
    files.extend(std::fs::canonicalize(path).ok());
    if let Some(interpreter) = interpreter {
        files.push(interpreter.to_owned());
        files.extend(std::fs::canonicalize(interpreter).ok());
    }
    fingerprint(&files, &["symfony-console"])
}

pub(super) fn is_php_file(path: &Path) -> bool {
    is_php_script(path) || super::head(path, 256).starts_with("<?php")
}

fn invoke(
    path: &Path,
    interpreter: Option<&Path>,
    mut args: Vec<OsString>,
    budget: &mut Budget,
) -> Result<String, CompletionError> {
    let program = if let Some(interpreter) = interpreter {
        args.insert(0, path.as_os_str().to_owned());
        interpreter
    } else {
        path
    };
    run_stdout(
        program,
        args,
        super::offline_env(Flavor::Symfony),
        budget,
        true,
    )
}

/// A script PHP runs: `#!/usr/bin/env php` or a direct interpreter path.
pub(super) fn is_php_script(path: &Path) -> bool {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let head = super::head(&real, 256);
    head.lines().next().is_some_and(|shebang| {
        shebang.starts_with("#!")
            && shebang
                .split_whitespace()
                .any(super::is_php_interpreter_name)
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

/// Help-based discovery for a trusted PHP script. The hidden command's JSON
/// definition proves the completion protocol exists before we invoke it.
pub(super) fn discover_from_help(
    path: &Path,
    budget: &mut Budget,
) -> Result<Option<Protocol>, CompletionError> {
    discover_with_interpreter(path, None, budget)
}

pub(super) fn discover_with_interpreter(
    path: &Path,
    interpreter: Option<&Path>,
    budget: &mut Budget,
) -> Result<Option<Protocol>, CompletionError> {
    if !(if interpreter.is_some() {
        is_php_file(path)
    } else {
        is_php_script(path)
    }) {
        return Ok(None);
    }
    let before = app_fingerprint(path, interpreter);
    let help = invoke(
        path,
        interpreter,
        vec!["--help".into(), "--no-interaction".into()],
        budget,
    )?;
    let Some(mut protocol) = Protocol::from_help(&help, path) else {
        return Ok(None);
    };
    protocol.interpreter = interpreter.map(Path::to_owned);
    protocol.application_fingerprint = app_fingerprint(path, interpreter);
    let mut args: Vec<OsString> = protocol.guards.iter().map(OsString::from).collect();
    args.extend(["help", "_complete", "--format=json", "--no-interaction"].map(OsString::from));
    let schema = invoke(path, interpreter, args, budget)?;
    if !completion_schema(&schema) {
        return Err(CompletionError::Unsupported(
            "the PHP app has no Symfony completion command definition".into(),
        ));
    }
    if protocol.application_fingerprint != before {
        return Err(CompletionError::Failed(
            "the application changed during Symfony Console discovery".into(),
        ));
    }
    Ok(Some(protocol))
}

fn completion_schema(text: &str) -> bool {
    let Ok(schema) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    schema["name"] == "_complete"
        && schema["hidden"] == true
        && ["shell", "input", "current"].into_iter().all(|name| {
            let option = &schema["definition"]["options"][name];
            option["name"] == format!("--{name}")
                && option["accept_value"] == true
                && option["is_multiple"] == (name == "input")
        })
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
        if !option(
            "-h, --help",
            "When no command is given display help for the list command",
        ) || !option(
            "-n, --no-interaction",
            "Do not ask any interactive question",
        ) {
            return None;
        }
        let guards = GUARDS
            .into_iter()
            .filter(|guard| {
                help.lines().any(|line| {
                    line.split_whitespace()
                        .next()
                        .is_some_and(|word| word == *guard)
                })
            })
            .collect();
        Some(Self {
            guards,
            global_options: crate::engine::docs::options(help),
            application_fingerprint: app_fingerprint(path, None),
            interpreter: None,
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
        if self.command(words).is_some() {
            capabilities.subcommands = false;
        }
        capabilities
    }

    /// Values of the application's documented global options aren't commands.
    fn command<'a>(&self, words: &[&'a str]) -> Option<&'a str> {
        let mut words = words.iter().copied();
        while let Some(word) = words.next() {
            if word == "--" {
                return words.next();
            }
            if !word.starts_with('-') {
                return Some(word);
            }
            if self
                .global_options
                .iter()
                .any(|option| option.value == word && option.takes_value == Some(true))
            {
                words.next();
            }
        }
        None
    }

    fn query(
        &self,
        backend: &Backend,
        words: &[&str],
        current: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        if app_fingerprint(&backend.completer, self.interpreter.as_deref())
            != self.application_fingerprint
        {
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
            invoke(
                &backend.completer,
                self.interpreter.as_deref(),
                args.iter().map(OsString::from).collect(),
                budget,
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
        if items.iter().any(CompletionItem::is_option) {
            // Completion lists names without arity. Symfony's JSON help
            // describes binding, without executing the operation. It only
            // annotates names the native completer actually returned.
            let options = self
                .command(words)
                .and_then(|command| self.command_options(backend, command, budget).ok())
                .unwrap_or_else(|| self.global_options.clone());
            for item in &mut items {
                if let Some(option) = options.iter().find(|option| option.value == item.value) {
                    item.takes_value = option.takes_value;
                }
            }
        }
        Ok(items)
    }

    fn command_options(
        &self,
        backend: &Backend,
        command: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let mut args: Vec<&str> = self.guards.clone();
        args.extend(["help", "--format=json", "--no-interaction", "--", command]);
        let key: Vec<_> = std::iter::once("symfony-help")
            .chain(args.iter().copied())
            .collect();
        let text = backend.memoized(&key, || {
            invoke(
                &backend.completer,
                self.interpreter.as_deref(),
                args.iter().map(OsString::from).collect(),
                budget,
            )
        })?;
        let description: serde_json::Value = serde_json::from_str(&text)
            .map_err(|_| CompletionError::Failed("Symfony command help is not JSON".into()))?;
        let options = description["definition"]["options"]
            .as_object()
            .ok_or_else(|| {
                CompletionError::Failed("Symfony command help has no option definitions".into())
            })?;
        if options.len() > budget.max_candidates {
            return Err(super::over_limit());
        }
        Ok(options
            .values()
            .filter_map(|option| {
                Some(CompletionItem {
                    value: option["name"].as_str()?.to_owned(),
                    takes_value: Some(option["accept_value"].as_bool()?),
                    description: None,
                })
            })
            .collect())
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
        let options = self
            .command(parent)
            .and_then(|command| self.command_options(backend, command, budget).ok())
            .unwrap_or_else(|| self.global_options.clone());
        let arity = options
            .iter()
            .find(|item| item.value == *option)
            .and_then(|item| item.takes_value);
        if arity == Some(false) {
            return Err(CompletionError::Unsupported(format!(
                "{option} takes no value"
            )));
        }
        let values = self.query(backend, words, "", budget)?;
        // The parent's answer can fail where the option's doesn't (Composer
        // lists installed packages only inside a project): then they differ.
        if arity.is_none() && self.query(backend, parent, "", budget).ok().as_ref() == Some(&values)
        {
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
        if self.command(words).is_some() || typed.starts_with('-') {
            return None;
        }
        let all = self.query(backend, words, "", budget).ok()?;
        // With the typed word as the current one.
        let answer = self.query(backend, words, typed, budget).ok()?;
        Some(!answer.is_empty() && answer.len() < all.len())
    }

    /// Values come from application callbacks and may name resources.
    pub fn resource(&self, words: &[&str], item: &CompletionItem) -> bool {
        // The wire format doesn't distinguish enum choices from callback
        // results (packages, paths, services), so values need confirmation.
        !item.is_option() && self.command(words).is_some()
    }
}

/// One literal suggestion per line. Spaces, quotes, and shell operators
/// are valid value bytes; the edit layer quotes them for the user's shell.
fn parse(text: &str, limit: usize) -> Result<Vec<CompletionItem>, CompletionError> {
    let mut items: Vec<CompletionItem> = Vec::new();
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.contains(char::is_control) {
            return Err(CompletionError::Failed(
                "Symfony completion contains control characters".into(),
            ));
        }
        if line.is_empty() || items.iter().any(|item| item.value == line) {
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
        assert_eq!(protocol.guards, ["--no-scripts"]);
        let console = "Options:\n  -h, --help            Display help for the given command. When no command is given display help for the list command\n  -n, --no-interaction  Do not ask any interactive question\n  -e, --env=ENV         The Environment name. [default: \"dev\"]\n";
        assert!(
            Protocol::from_help(console, path)
                .unwrap()
                .guards
                .is_empty()
        );
        for other in [
            "usage: tool [-h]\n  -h, --help  show this help message and exit\n",
            "  -h, --help  Display help for the given command. When no command is given display help for the list command\n",
            "Mentions --no-plugins and -n, --no-interaction Do not ask any interactive question\n",
        ] {
            assert_eq!(Protocol::from_help(other, path), None, "{other}");
        }
    }

    #[test]
    fn completion_needs_the_hidden_commands_own_schema() {
        let schema = serde_json::json!({"name":"_complete", "hidden":true, "definition":{"options":{
            "shell":{"name":"--shell", "accept_value":true, "is_multiple":false},
            "input":{"name":"--input", "accept_value":true, "is_multiple":true},
            "current":{"name":"--current", "accept_value":true, "is_multiple":false}
        }}});
        assert!(completion_schema(&schema.to_string()));
        for (field, value) in [
            ("name", serde_json::json!("list")),
            ("hidden", serde_json::json!(false)),
        ] {
            let mut wrong = schema.clone();
            wrong[field] = value;
            assert!(!completion_schema(&wrong.to_string()));
        }
        let mut wrong = schema.clone();
        wrong["definition"]["options"]["input"]["is_multiple"] = serde_json::json!(false);
        assert!(!completion_schema(&wrong.to_string()));
        for wrong in ["", "not JSON", "{}", "{\"name\":\"_complete\"}"] {
            assert!(!completion_schema(wrong));
        }
    }

    #[test]
    fn answers_are_one_word_per_line() {
        let items = parse("install\ni\n--dev\n--no-dev\n\ninstall\nnot one word\n", 16).unwrap();
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(
            values,
            ["install", "i", "--dev", "--no-dev", "not one word"]
        );
        assert!(parse("a\nb\nc\n", 2).is_err());
        assert!(parse("safe\nbad\x1b\n", 16).is_err());
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

    use super::super::{NativeCompletionBackend, tests::Dir};
    use std::time::Duration;

    const HELP: &str = "Options:\n  -h, --help  Display help for the given command. When no command is given display help for the list command\n  -n, --no-interaction  Do not ask any interactive question\n  -d, --working-dir=DIR  Working directory\n      --no-scripts  Disable scripts\n";

    // A protocol fixture, independent of whether PHP is installed. Records
    // every argv element, requires completion mode, and models app upgrades.
    const APP: &str = r#"#!/bin/sh
root=$(dirname "$0")
[ "$HTTPS_PROXY" = http://127.0.0.1:9 ] || exit 8
[ "$COMPOSER_DISABLE_NETWORK" = 1 ] || exit 8
[ -z "$SYMFONY_COMPLETION_DEBUG" ] || exit 8
printf '<%s>' "$@" >> "$root/queries"
printf '\n' >> "$root/queries"
if [ "$1" = --no-scripts ] && [ "$2" = help ]; then
  printf '%s\n' '{"definition":{"options":{"format":{"name":"--format","accept_value":true},"no-cache":{"name":"--no-cache","accept_value":false},"verbose":{"name":"--verbose","accept_value":false}}}}'
  exit 0
fi
[ "$1" = --no-scripts ] && [ "$2" = _complete ] && [ "$3" = --no-interaction ] && [ "$4" = -sbash ] || { touch "$root/operation-marker"; exit 9; }
if [ -f "$root/mode" ]; then
  case "$(cat "$root/mode")" in
    failed) exit 2;;
    invalid) printf '\377'; exit 0;;
    oversized) printf 'one\ntwo\nthree\n'; exit 0;;
  esac
fi
case "$*" in
  *'-i--format') printf 'json\nyaml\n';;
  *'-i--') printf -- '--format\n--no-cache\n--verbose\n';;
  *'-icache:clear') printf 'my route\nregion;literal\n';;
  *'-ic:c'|*'-icc') printf 'cache:clear\n';;
  *) printf 'help\nlist\ncache:clear\n'; [ ! -f "$root/upgrade" ] || printf 'extension:inspect\n';;
esac
"#;

    fn fixture(dir: &Dir) -> Backend {
        let path = dir.script("app", APP);
        backend(
            "fixture",
            &path,
            None,
            Protocol::from_help(HELP, &path).unwrap(),
        )
    }

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(10), Duration::from_secs(2), 16)
    }

    fn values(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.value.as_str()).collect()
    }

    #[test]
    fn literal_context_options_values_and_abbreviations_use_completion_mode() {
        let dir = Dir::new("symfony-context");
        let backend = fixture(&dir);
        let mut budget = budget();
        assert_eq!(
            values(&backend.complete(&[], "", &mut budget).unwrap()),
            ["help", "list", "cache:clear"]
        );
        assert_eq!(
            values(
                &backend
                    .complete(&["cache:clear"], "--", &mut budget)
                    .unwrap()
            ),
            ["--format", "--no-cache", "--verbose"]
        );
        assert_eq!(
            values(
                &backend
                    .complete_values(&["cache:clear", "--format"], &mut budget)
                    .unwrap()
            ),
            ["json", "yaml"]
        );
        assert_eq!(
            values(&backend.complete(&["cache:clear"], "", &mut budget).unwrap()),
            ["my route", "region;literal"]
        );
        assert_eq!(backend.confirms(&[], "c:c", &mut budget), Some(true));
        assert_eq!(backend.confirms(&[], "nosuch", &mut budget), Some(false));
        assert!(
            backend
                .capabilities_for(&["--working-dir", "project"])
                .subcommands
        );
        assert!(
            !backend
                .capabilities_for(&["--working-dir", "project", "cache:clear"])
                .subcommands
        );
        // Every word is data in an -i argument, including option-shaped
        // values and shell expressions; no part of the operation runs.
        let _ = backend
            .complete(
                &["cache:clear", "$(touch operation-marker)", "--no-scripts"],
                "--",
                &mut budget,
            )
            .unwrap();
        let queries = std::fs::read_to_string(dir.0.join("queries")).unwrap();
        assert!(queries.contains("<-c1><-ifixture>"), "{queries}");
        assert!(
            queries.contains("<-c3><-ifixture><-icache:clear><-i--format>"),
            "{queries}"
        );
        assert!(
            queries.contains("<-i$(touch operation-marker)><-i--no-scripts>"),
            "{queries}"
        );
        assert!(!queries.contains("<--no-plugins>"));
        assert!(!dir.0.join("operation-marker").exists());
        assert_eq!(backend.cache_identity(), None);
        assert!(!backend.capabilities().complete_subcommands);
        assert!(!backend.capabilities().short_options);
        let value = CompletionItem {
            value: "json".into(),
            takes_value: None,
            description: None,
        };
        assert!(backend.candidate_is_resource(&["cache:clear", "--format"], &value));
    }

    #[test]
    fn app_and_extension_upgrades_are_visible_on_the_next_request() {
        let dir = Dir::new("symfony-upgrade");
        let first = fixture(&dir);
        let mut budget = budget();
        assert_eq!(first.complete(&[], "extension", &mut budget).unwrap(), []);
        std::fs::write(dir.0.join("upgrade"), "enabled").unwrap();
        // Answers are request-local; installed extensions are not disk cached.
        let next = fixture(&dir);
        assert_eq!(
            values(&next.complete(&[], "extension", &mut budget).unwrap()),
            ["extension:inspect"]
        );
        // Replacing the app after reading its help invalidates this backend.
        std::fs::write(dir.0.join("app"), "changed").unwrap();
        assert!(
            matches!(next.complete(&[], "", &mut budget), Err(CompletionError::Failed(why)) if why.contains("changed"))
        );
    }

    #[test]
    fn failed_malformed_and_over_limit_answers_are_not_vocabulary() {
        for (mode, why) in [
            ("failed", "exited"),
            ("invalid", "UTF-8"),
            ("oversized", "candidate"),
        ] {
            let dir = Dir::new(&format!("symfony-{mode}"));
            let backend = fixture(&dir);
            std::fs::write(dir.0.join("mode"), mode).unwrap();
            let mut budget = budget();
            budget.max_candidates = 2;
            let answer = backend.complete(&[], "", &mut budget);
            assert!(
                matches!(answer, Err(CompletionError::Failed(ref message)) if message.contains(why)),
                "{mode}: {answer:?}"
            );
        }
        let dir = Dir::new("symfony-rejected-context");
        let backend = fixture(&dir);
        for words in [&[""][..], &["line\nbreak"], &["tab\tword"]] {
            assert!(matches!(
                backend.complete(words, "", &mut budget()),
                Err(CompletionError::Unsupported(_))
            ));
        }
        assert!(!dir.0.join("queries").exists());
    }
}
