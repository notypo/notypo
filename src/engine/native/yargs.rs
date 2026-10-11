//! yargs answers `app --get-yargs-completions <program> <words> <current>`
//! in every app built on it, whether or not the app calls `.completion()`.
//! The first word after the flag is consumed as the flag's own value, so
//! the program name goes first, as yargs's bash and zsh scripts pass it.
//! An app that doesn't use yargs simply runs, so the protocol comes only
//! from evidence: an installed yargs script for this app (read, never
//! sourced), or, under `trusted_help`, a Node bin whose package depends on
//! yargs and whose `--help` shows yargs's built-in help option.
//!
//! Answering loads the app's module and runs the builders of the commands
//! named in the query; handlers, middleware, and coerce functions don't run
//! (verified with yargs 18.2.0). An app that disables `exitProcess` keeps
//! running after printing, and yargs-parser loads config files named by
//! `.config()` options, so queries carry only command words the app listed.
//!
//! Each query runs twice. Plain answers print names; zsh answers (selected
//! by `ZSH_NAME`) add `:description` to commands and options and escape
//! colons. Pairing the two tells commands and options from choice values
//! and from an app's custom completion output, which both print verbatim.

use super::{Backend, Capabilities, CompletionError, CompletionItem, Flavor, Trust, run_stdout};
use crate::engine::cache::fingerprint;
use crate::engine::probe::Budget;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const FLAG: &str = "--get-yargs-completions";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Protocol {
    /// Files whose change means the evidence no longer describes the app.
    files: Vec<PathBuf>,
    fingerprint: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Command,
    Option,
    /// A choice, or a word from the app's own completion function.
    Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    item: CompletionItem,
    kind: Kind,
}

#[derive(Debug, PartialEq, Eq)]
struct Level {
    entries: Vec<Entry>,
}

impl Level {
    fn has_command(&self, word: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.kind == Kind::Command && entry.item.value == word)
    }

    fn lists_commands(&self) -> bool {
        self.entries.iter().any(|entry| entry.kind == Kind::Command)
    }
}

struct Walk<'a> {
    path: Vec<&'a str>,
    /// A word naming no command was passed: the slot is an argument.
    positional: bool,
}

/// Whether a query may start the app or must reuse this request's answers.
enum Mode<'a> {
    Probe(&'a mut Budget),
    Memo,
}

fn evidence_files(path: &Path) -> Vec<PathBuf> {
    let mut files = vec![path.to_owned()];
    if let Ok(real) = std::fs::canonicalize(path) {
        if let Some((root, _)) = super::identity::npm_manifest(&real) {
            files.push(root.join("package.json"));
            files.extend(yargs_manifest(&root));
        }
        files.push(real);
    }
    files
}

fn evidence_fingerprint(files: &[PathBuf]) -> String {
    fingerprint(files, &["yargs"])
}

/// yargs's manifest as Node resolves it from the package root.
fn yargs_manifest(root: &Path) -> Option<PathBuf> {
    root.ancestors().take(32).find_map(|dir| {
        let manifest = dir.join("node_modules/yargs/package.json");
        let text = std::fs::read_to_string(&manifest).ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        (value.get("name")?.as_str()? == "yargs").then_some(manifest)
    })
}

/// A Node bin whose own package declares yargs as a dependency and can
/// resolve it. Help output then decides whether the root parser is yargs.
fn depends_on_yargs(path: &Path) -> bool {
    let Ok(real) = std::fs::canonicalize(path) else {
        return false;
    };
    let Some((root, manifest)) = super::identity::npm_manifest(&real) else {
        return false;
    };
    let declared = ["dependencies", "optionalDependencies", "peerDependencies"]
        .iter()
        .any(|field| {
            manifest
                .get(field)
                .and_then(|deps| deps.get("yargs"))
                .is_some()
        });
    declared && yargs_manifest(&root).is_some()
}

pub(super) fn backend(
    name: &str,
    path: &Path,
    identity: Option<String>,
    protocol: Protocol,
) -> Backend {
    let mut backend =
        Backend::new(Flavor::Yargs, name, path.to_owned()).with_application(path.to_owned());
    backend.identity = identity;
    backend.bridge = Some(Box::new(super::Bridge::Yargs(protocol)));
    backend
}

fn protocol_for(path: &Path) -> Protocol {
    let files = evidence_files(path);
    Protocol {
        fingerprint: evidence_fingerprint(&files),
        files,
    }
}

impl Protocol {
    /// A script yargs generated for this app: its banner, one query line
    /// running this application with the completion flag, and a
    /// registration of yargs's function (or fish command) for the same name.
    pub(super) fn from_script(script: &str, name: &str, path: &Path) -> Option<Self> {
        if cfg!(windows) {
            return None;
        }
        let lines: Vec<&str> = script.lines().map(str::trim).collect();
        if !lines.contains(&"# yargs command completion script") {
            return None;
        }
        let app = lines.iter().find_map(|line| {
            line.strip_prefix("###-begin-")?
                .strip_suffix("-completions-###")
        })?;
        if !lines.contains(&format!("###-end-{app}-completions-###").as_str()) {
            return None;
        }
        let queries: Vec<&str> = lines
            .iter()
            .filter(|line| line.contains(FLAG))
            .copied()
            .collect();
        let [query] = queries.as_slice() else {
            return None;
        };
        // The word before the flag runs the app: a plain path or name.
        let before = query.split_once(&format!(" {FLAG} "))?.0;
        let program = before.rsplit([' ', '(', '<']).next()?;
        if program.is_empty()
            || program.bytes().any(|c| b"\"'`$\\;&|*?[]{}~()".contains(&c))
            || !super::clap::same_application(program, name, path)
        {
            return None;
        }
        let function = format!("_{app}_yargs_completions");
        let registered = lines.iter().any(|line| {
            *line == format!("complete -o bashdefault -o default -F {function} {app}")
                || *line == format!("compdef {function} {app}")
                || line.starts_with(&format!("complete -f -c {app} -a '("))
        });
        (registered && super::clap::same_application(app, name, path)).then(|| protocol_for(path))
    }

    pub fn capabilities(&self, trust: Trust) -> Capabilities {
        Capabilities {
            subcommands: true,
            options: true,
            option_arity: false,
            values: true,
            // Every query uses the offline environment, even with network
            // completion enabled.
            resources: false,
            option_prefix: "-",
            short_options: true,
            // Hidden items and command aliases are not listed, and apps
            // without strict mode accept options they never declared.
            complete_options: false,
            complete_subcommands: false,
            descriptions: true,
            query_dialect: None,
            trust,
        }
    }

    fn raw(
        &self,
        backend: &Backend,
        path: &[&str],
        current: &str,
        zsh: bool,
        mode: &mut Mode<'_>,
    ) -> Result<String, CompletionError> {
        let mode_name = if zsh { "zsh" } else { "plain" };
        let mut key = vec!["yargs", mode_name];
        key.extend_from_slice(path);
        key.push(current);
        let budget = match mode {
            Mode::Probe(budget) => budget,
            Mode::Memo => {
                let key: Vec<String> = key.iter().map(|k| k.to_string()).collect();
                return backend.memo.borrow().get(&key).cloned().unwrap_or_else(|| {
                    Err(CompletionError::Failed(
                        "yargs level was not queried in this request".into(),
                    ))
                });
            }
        };
        if cfg!(windows) {
            return Err(CompletionError::Unsupported(
                "yargs apps run through npm's command shims on Windows, which notypo does not start".into(),
            ));
        }
        if evidence_fingerprint(&self.files) != self.fingerprint {
            return Err(CompletionError::Failed(
                "the app changed after its yargs completion evidence was read".into(),
            ));
        }
        let program = super::app_name(&backend.name);
        let mut args: Vec<OsString> = vec![FLAG.into(), program.into()];
        args.extend(path.iter().map(Into::into));
        args.push(current.into());
        backend.memoized(&key, || {
            let mut env = super::offline_env(Flavor::Yargs);
            if zsh {
                env.extend([
                    ("SHELL".into(), Some("/bin/zsh".into())),
                    ("ZSH_NAME".into(), Some("zsh".into())),
                ]);
            } else {
                env.extend([
                    ("SHELL".into(), Some("/bin/sh".into())),
                    ("ZSH_NAME".into(), None),
                ]);
            }
            run_stdout(&backend.completer, args, env, budget, true)
        })
    }

    fn level(
        &self,
        backend: &Backend,
        path: &[&str],
        current: &str,
        mode: &mut Mode<'_>,
    ) -> Result<Level, CompletionError> {
        let limit = match mode {
            Mode::Probe(budget) => budget.max_candidates,
            Mode::Memo => usize::MAX,
        };
        let plain = self.raw(backend, path, current, false, mode)?;
        // An empty answer has nothing to describe.
        let zsh = if plain.is_empty() {
            String::new()
        } else {
            self.raw(backend, path, current, true, mode)?
        };
        Ok(Level {
            entries: pair(&plain, &zsh, limit)?,
        })
    }

    fn walk<'a>(
        &self,
        backend: &Backend,
        words: &[&'a str],
        mode: &mut Mode<'_>,
    ) -> Result<Walk<'a>, CompletionError> {
        let mut path: Vec<&str> = Vec::new();
        let mut level = self.level(backend, &path, "", mode)?;
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
                    && words
                        .get(i)
                        .is_some_and(|next| !next.starts_with('-') && !level.has_command(next))
                {
                    i += 1;
                }
                continue;
            }
            if level.has_command(word) {
                path.push(word);
                level = self.level(backend, &path, "", mode)?;
                continue;
            }
            if level.lists_commands() {
                // Aliases and hidden commands are unlisted; a word naming
                // no command gets its parent's answer again.
                let mut child = path.clone();
                child.push(word);
                let answer = self.level(backend, &child, "", mode)?;
                if answer != level {
                    path = child;
                    level = answer;
                    continue;
                }
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
                "yargs completion context contains control characters".into(),
            ));
        }
        let mut mode = Mode::Probe(budget);
        let walk = self.walk(backend, words, &mut mode)?;
        let entries = if prefix.starts_with('-') {
            // With `--`, yargs prints a short alias as `--v`; `-` keeps
            // each option's own dashes.
            let level = self.level(backend, &walk.path, "-", &mut mode)?;
            level
                .entries
                .into_iter()
                .filter(|entry| entry.kind == Kind::Option)
                .collect::<Vec<_>>()
        } else if walk.positional {
            return Err(CompletionError::Unsupported(
                "yargs is not asked about words after an argument".into(),
            ));
        } else {
            let level = self.level(backend, &walk.path, "", &mut mode)?;
            let words: Vec<Entry> = level
                .entries
                .into_iter()
                .filter(|entry| entry.kind != Kind::Option)
                .collect();
            if words.is_empty() {
                return Err(CompletionError::Unsupported(
                    "this yargs command lists no subcommands or argument choices".into(),
                ));
            }
            words
        };
        Ok(entries
            .into_iter()
            .map(|entry| entry.item)
            .filter(|item| item.value.starts_with(prefix))
            .collect())
    }

    /// An option's declared choices. Without choices yargs answers with
    /// the level's commands and options instead.
    pub fn values(
        &self,
        backend: &Backend,
        words: &[&str],
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let Some((&option, parent)) = words.split_last() else {
            return Ok(Vec::new());
        };
        if !option.starts_with('-') || option == "-" {
            return Ok(Vec::new());
        }
        if option.contains('=') || words.iter().any(|word| word.contains(char::is_control)) {
            return Err(CompletionError::Unsupported(
                "yargs completes values only after a separate option word".into(),
            ));
        }
        let mut mode = Mode::Probe(budget);
        let walk = self.walk(backend, parent, &mut mode)?;
        if walk.positional {
            return Err(CompletionError::Unsupported(
                "yargs is not asked about words after an argument".into(),
            ));
        }
        let mut query = walk.path.clone();
        query.push(option);
        let answer = self.level(backend, &query, "", &mut mode)?;
        let level = self.level(backend, &walk.path, "", &mut mode)?;
        if answer == level || answer.entries.iter().any(|entry| entry.kind != Kind::Value) {
            return Err(CompletionError::Unsupported(format!(
                "yargs lists no choices for {option}"
            )));
        }
        Ok(answer.entries.into_iter().map(|entry| entry.item).collect())
    }

    /// Aliases and hidden commands are unlisted: a word naming one gets a
    /// different answer than its parent's.
    pub fn confirms(
        &self,
        backend: &Backend,
        words: &[&str],
        typed: &str,
        budget: &mut Budget,
    ) -> Option<bool> {
        if typed.is_empty()
            || typed.starts_with('-')
            || typed.contains(char::is_control)
            || words.iter().any(|word| word.contains(char::is_control))
        {
            return None;
        }
        let mut mode = Mode::Probe(budget);
        let walk = self.walk(backend, words, &mut mode).ok()?;
        if walk.positional {
            return None;
        }
        let level = self.level(backend, &walk.path, "", &mut mode).ok()?;
        if !level.lists_commands() {
            return None;
        }
        let mut child = walk.path.clone();
        child.push(typed);
        let answer = self.level(backend, &child, "", &mut mode).ok()?;
        // An unchanged answer is inconclusive: two levels can look alike.
        (answer != level).then_some(true)
    }

    /// Only commands the app listed are vocabulary. Choices can't be told
    /// from an app's own completion output, so changing one needs approval.
    /// Judged from this request's answers only.
    pub fn resource(&self, backend: &Backend, words: &[&str], item: &CompletionItem) -> bool {
        if item.is_option() {
            return false;
        }
        let mut mode = Mode::Memo;
        match self.walk(backend, words, &mut mode) {
            Ok(walk) if !walk.positional => self
                .level(backend, &walk.path, "", &mut mode)
                .map_or(true, |level| !level.has_command(&item.value)),
            _ => true,
        }
    }
}

/// zsh's escaping of a name.
fn escaped(name: &str) -> String {
    name.replace(':', "\\:")
}

/// Pairs the plain and zsh answers line by line. Each zsh line must be the
/// plain line (a value), its escaped form (a value with colons), or the
/// escaped form followed by `:description` (a command or option).
fn pair(plain: &str, zsh: &str, limit: usize) -> Result<Vec<Entry>, CompletionError> {
    let disagree = || CompletionError::Failed("yargs plain and zsh answers disagree".into());
    for text in [plain, zsh] {
        if !text.is_empty() && !text.ends_with('\n') {
            return Err(CompletionError::Failed(
                "yargs completion returned an unterminated line".into(),
            ));
        }
    }
    let plain: Vec<&str> = plain.split_terminator('\n').collect();
    let zsh: Vec<&str> = zsh.split_terminator('\n').collect();
    if plain.len() != zsh.len() {
        return Err(disagree());
    }
    let mut entries: Vec<Entry> = Vec::new();
    for (&value, &line) in plain.iter().zip(&zsh) {
        if value.is_empty() || value.contains(char::is_control) {
            return Err(CompletionError::Failed(
                "yargs completion returned an empty word or control characters".into(),
            ));
        }
        let escaped = escaped(value);
        let (kind, description) = if line == value || line == escaped {
            let kind = if value.starts_with('-') {
                Kind::Option
            } else {
                Kind::Value
            };
            (kind, None)
        } else {
            let description = line
                .strip_prefix(&escaped)
                .and_then(|rest| rest.strip_prefix(':'))
                .ok_or_else(disagree)?;
            let kind = if value.starts_with('-') {
                Kind::Option
            } else {
                Kind::Command
            };
            (kind, super::clean_description(description))
        };
        // Words with whitespace would need quoting the answer cannot show.
        if value.contains(char::is_whitespace)
            || entries.iter().any(|entry| entry.item.value == value)
        {
            continue;
        }
        if entries.len() >= limit {
            return Err(super::over_limit());
        }
        entries.push(Entry {
            item: CompletionItem {
                value: value.to_owned(),
                takes_value: None,
                description,
            },
            kind,
        });
    }
    Ok(entries)
}

/// Help-based discovery for a trusted Node bin whose package depends on
/// yargs. Only `--help` runs, in the C locale so yargs prints its English
/// strings; the app must show yargs's built-in help option.
pub(super) fn discover_from_help(
    path: &Path,
    budget: &mut Budget,
) -> Result<Option<Protocol>, CompletionError> {
    if cfg!(windows) || !depends_on_yargs(path) {
        return Ok(None);
    }
    let protocol = protocol_for(path);
    let mut env = super::offline_env(Flavor::Yargs);
    env.push(("LC_ALL".into(), Some("C".into())));
    let help = run_stdout(path, vec!["--help".into()], env, budget, true)?;
    if !shows_yargs_help(&help) {
        return Ok(None);
    }
    if evidence_fingerprint(&protocol.files) != protocol.fingerprint {
        return Err(CompletionError::Failed(
            "the application changed during yargs discovery".into(),
        ));
    }
    Ok(Some(protocol))
}

/// yargs's own help option line: `--help  Show help  [boolean]`, with an
/// optional short alias.
fn shows_yargs_help(help: &str) -> bool {
    let line = regex!(r"^\s*(?:-[A-Za-z0-9], )?\s*--help\s+Show help\s+\[boolean\]\s*$");
    help.lines().any(|text| line.is_match(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_answers_separate_commands_options_and_values() {
        let plain = "serve\ndb:migrate\n--verbose\n-v\njson\nz:a\n";
        let zsh = "serve:Start the server\ndb\\:migrate:Run: migrations\n--verbose:Verbose\x1b logging\n-v:\njson\nz\\:a\n";
        let entries = pair(plain, zsh, 10).unwrap();
        let kinds: Vec<(&str, Kind)> = entries
            .iter()
            .map(|entry| (entry.item.value.as_str(), entry.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                ("serve", Kind::Command),
                ("db:migrate", Kind::Command),
                ("--verbose", Kind::Option),
                ("-v", Kind::Option),
                ("json", Kind::Value),
                ("z:a", Kind::Value),
            ]
        );
        assert_eq!(
            entries[1].item.description.as_deref(),
            Some("Run: migrations")
        );
        assert_eq!(
            entries[2].item.description.as_deref(),
            Some("Verbose logging")
        );
        assert_eq!(entries[3].item.description, None);
        assert!(pair("", "", 1).unwrap().is_empty());
    }

    #[test]
    fn mismatched_malformed_and_oversized_answers_fail_as_a_whole() {
        for (plain, zsh) in [
            ("a\n", "b:x\n"),
            ("a\nb\n", "a:x\n"),
            ("a", "a:x"),
            ("a\n\n", "a\n\n"),
            ("a\tb\n", "a\tb\n"),
            ("serve\n", "serve\\:x\n"),
        ] {
            assert!(pair(plain, zsh, 10).is_err(), "{plain:?} {zsh:?}");
        }
        assert!(pair("a\nb\n", "a\nb\n", 1).is_err());
        assert_eq!(pair("a\na\n", "a\na\n", 1).unwrap().len(), 1);
        // Words with spaces (a description that broke a line, or custom
        // output) are left out rather than offered unquoted.
        assert!(pair("two words\n", "two words\n", 1).unwrap().is_empty());
    }

    #[test]
    fn help_signature_needs_yargs_help_option_line() {
        assert!(shows_yargs_help(
            "app <command>\n\nOptions:\n      --help     Show help                     [boolean]\n"
        ));
        assert!(shows_yargs_help(
            "Options:\n  -h, --help  Show help  [boolean]\n"
        ));
        assert!(!shows_yargs_help(
            "Options:\n  -h, --help  display help for command\n"
        ));
        assert!(!shows_yargs_help("--help  Show help\n"));
    }

    #[test]
    fn generated_scripts_bind_this_application() {
        let dir = std::env::temp_dir().join(format!("notypo-yargs-script-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let app = dir.join("ydemo");
        std::fs::write(&app, "#!/usr/bin/env node\n").unwrap();
        for script in [BASH, ZSH, FISH] {
            assert!(
                Protocol::from_script(script, "ydemo", &app).is_some(),
                "{script}"
            );
            assert!(Protocol::from_script(script, "other", Path::new("/usr/bin/other")).is_none());
        }
        for edited in [
            BASH.replace("# yargs command completion script", "# completion"),
            BASH.replace("###-end-ydemo", "###-end-other"),
            BASH.replace(
                "complete -o bashdefault -o default -F _ydemo_yargs_completions ydemo",
                "complete -F _ydemo_yargs_completions other",
            ),
            BASH.replace("< <(ydemo --get", "< <($(evil) --get"),
            format!("{BASH}\nx=$(ydemo --get-yargs-completions)\n"),
        ] {
            assert!(
                Protocol::from_script(&edited, "ydemo", &app).is_none(),
                "{edited}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // As yargs 18.2.0 generated them for an app named ydemo.
    const BASH: &str = r#"###-begin-ydemo-completions-###
#
# yargs command completion script
#
# Installation: ydemo completion >> ~/.bashrc
#    or ydemo completion >> ~/.bash_profile on OSX.
#
_ydemo_yargs_completions()
{
    local cur_word args type_list

    cur_word="${COMP_WORDS[COMP_CWORD]}"
    args=("${COMP_WORDS[@]}")

    # ask yargs to generate completions.
    # see https://stackoverflow.com/a/40944195/7080036 for the spaces-handling awk
    mapfile -t type_list < <(ydemo --get-yargs-completions "${args[@]}")
    mapfile -t COMPREPLY < <(compgen -W "$( printf '%q ' "${type_list[@]}" )" -- "${cur_word}" |
        awk '/ / { print "\""$0"\"" } /^[^ ]+$/ { print $0 }')

    # if no match was found, fall back to filename completion
    if [ ${#COMPREPLY[@]} -eq 0 ]; then
      COMPREPLY=()
    fi

    return 0
}
complete -o bashdefault -o default -F _ydemo_yargs_completions ydemo
###-end-ydemo-completions-###
"#;
    const ZSH: &str = r#"#compdef ydemo
###-begin-ydemo-completions-###
#
# yargs command completion script
#
# Installation: ydemo completion >> ~/.zshrc
#    or ydemo completion >> ~/.zprofile on OSX.
#
_ydemo_yargs_completions()
{
  local reply
  local si=$IFS
  IFS=$'\n' reply=($(COMP_CWORD="$((CURRENT-1))" COMP_LINE="$BUFFER" COMP_POINT="$CURSOR" ydemo --get-yargs-completions "${words[@]}"))
  IFS=$si
  if [[ ${#reply} -gt 0 ]]; then
    _describe 'values' reply
  else
    _default
  fi
}
if [[ "${zsh_eval_context[-1]}" == "loadautofunc" ]]; then
  _ydemo_yargs_completions "$@"
else
  compdef _ydemo_yargs_completions ydemo
fi
###-end-ydemo-completions-###
"#;
    const FISH: &str = r#"###-begin-ydemo-completions-###
#
# yargs command completion script
#
# Installation: ydemo completion > ~/.config/fish/completions/ydemo.fish
#
complete -f -c ydemo -a '(ydemo --get-yargs-completions (commandline -o)[2..-1])'
###-end-ydemo-completions-###
"#;
}

#[cfg(all(test, unix))]
mod protocol_tests {
    use super::*;
    use crate::engine::native::{
        Discovery, NativeCompletionBackend, discover_for_shell_with_budget, tests::Dir,
    };
    use crate::shells::Shell;
    use std::time::Duration;

    /// Answers like yargs 18.2.0 did for the same command tree (serve with
    /// alias s, deploy <env> with choices, db with migrate and seed).
    const APP: &str = r#"#!/bin/sh
root='@ROOT@'
[ "$HTTPS_PROXY" = http://127.0.0.1:9 ] && [ "$NO_UPDATE_NOTIFIER" = 1 ] && [ "$NODE_USE_ENV_PROXY" = 1 ] || exit 8
if [ "$1" = --help ]; then [ "$LC_ALL" = C ] || exit 7; cat "$root/help"; exit 0; fi
[ "$1" = --get-yargs-completions ] && [ "$2" = fixture ] || { touch "$root/operation-marker"; exit 9; }
shift 2
suffix=plain
if [ "$ZSH_NAME" = zsh ] && [ "$SHELL" = /bin/zsh ]; then suffix=zsh; elif [ "$SHELL" != /bin/sh ]; then exit 5; fi
printf '%s|' "$suffix" >> "$root/queries"
printf '<%s>' "$@" >> "$root/queries"
echo >> "$root/queries"
if [ -f "$root/mode" ]; then
  case "$(cat "$root/mode")" in
    failed) exit 1;;
    utf8) printf '\377\n'; exit 0;;
    disagree) [ "$suffix" = plain ] || { printf 'other:x\n'; exit 0; };;
  esac
fi
case "$*" in
  '') answer=root;;
  '-'|'deploy -') answer=options;;
  '--format '|'deploy --format ') answer=formats;;
  'serve '|'s '|'serve -'|'s -'|'serve --host '|'s --host ') answer=serve;;
  'db migrate '|'db migrate -') answer=migrate;;
  'db '*) answer=db;;
  'deploy '|'deploy --verbose ') answer=deploy;;
  *) answer=root;;
esac
cat "$root/answers/$answer.$suffix"
"#;
    const OPTIONS: (&str, &str) = (
        "--help\n--version\n--verbose\n-v\n--format\n--config\n",
        "--help:Show help\n--version:Show version number\n--verbose:Verbose logging\n-v:Verbose logging\n--format:Output format\n--config:Config file\n",
    );
    const HELP: &str = "fixture <command>\n\nCommands:\n  fixture serve [port]  Start the server  [aliases: s]\n\nOptions:\n      --help     Show help                                             [boolean]\n      --version  Show version number                                   [boolean]\n";

    struct Fixture {
        dir: Dir,
        bin: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let dir = Dir::new(tag);
            let package = dir.0.join("lib/node_modules/fixture");
            std::fs::create_dir_all(package.join("node_modules/yargs")).unwrap();
            std::fs::write(
                package.join("package.json"),
                r#"{"name": "fixture", "bin": {"fixture": "bin/fixture.js"}, "dependencies": {"yargs": "^18.2.0"}}"#,
            )
            .unwrap();
            std::fs::write(
                package.join("node_modules/yargs/package.json"),
                r#"{"name": "yargs", "version": "18.2.0"}"#,
            )
            .unwrap();
            let root = dir.0.display().to_string();
            dir.script(
                "lib/node_modules/fixture/bin/fixture.js",
                &APP.replace("@ROOT@", &root),
            );
            std::fs::create_dir_all(dir.0.join("bin")).unwrap();
            let bin = dir.0.join("bin/fixture");
            std::os::unix::fs::symlink("../lib/node_modules/fixture/bin/fixture.js", &bin).unwrap();
            std::fs::write(dir.0.join("help"), HELP).unwrap();
            let options = |extra: (&str, &str)| {
                (
                    format!("{}{}", OPTIONS.0, extra.0),
                    format!("{}{}", OPTIONS.1, extra.1),
                )
            };
            for (name, (plain, zsh)) in [
                (
                    "root",
                    (
                        "serve\ndeploy\ndb\n".to_owned(),
                        "serve:Start the server\ndeploy:Deploy the app\ndb:Database tasks\n"
                            .to_owned(),
                    ),
                ),
                ("options", options(("", ""))),
                (
                    "formats",
                    ("json\nyaml\ntable\n".into(), "json\nyaml\ntable\n".into()),
                ),
                ("serve", options(("--host\n", "--host:Bind host\n"))),
                (
                    "migrate",
                    options(("--dry-run\n", "--dry-run:Show the plan\n")),
                ),
                (
                    "db",
                    (
                        "migrate\nseed\n".into(),
                        "migrate:Run migrations\nseed:Seed data\n".into(),
                    ),
                ),
                (
                    "deploy",
                    options(("staging\nproduction\nqa\n", "staging\nproduction\nqa\n")),
                ),
            ] {
                std::fs::create_dir_all(dir.0.join("answers")).unwrap();
                std::fs::write(dir.0.join(format!("answers/{name}.plain")), plain).unwrap();
                std::fs::write(dir.0.join(format!("answers/{name}.zsh")), zsh).unwrap();
            }
            Fixture { dir, bin }
        }

        fn backend(&self) -> Backend {
            backend(
                "fixture",
                &self.bin,
                Some("npm:fixture".into()),
                protocol_for(&self.bin),
            )
        }

        fn queries(&self) -> String {
            std::fs::read_to_string(self.dir.0.join("queries")).unwrap_or_default()
        }

        fn marker(&self) -> bool {
            self.dir.0.join("operation-marker").exists()
        }
    }

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(10), Duration::from_secs(2), 64)
    }

    fn values(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.value.as_str()).collect()
    }

    #[test]
    fn levels_aliases_options_and_choices_keep_descriptions_without_sending_options() {
        let app = Fixture::new("yargs-levels");
        let backend = app.backend();
        let mut budget = budget();
        let root = backend.complete(&[], "", &mut budget).unwrap();
        assert_eq!(values(&root), ["serve", "deploy", "db"]);
        assert_eq!(root[0].description.as_deref(), Some("Start the server"));
        let options = backend.complete(&[], "-", &mut budget).unwrap();
        assert!(values(&options).contains(&"-v"));
        assert_eq!(options[2].description.as_deref(), Some("Verbose logging"));
        assert_eq!(
            values(
                &backend
                    .complete(&["--config", "app.js", "db"], "", &mut budget)
                    .unwrap()
            ),
            ["migrate", "seed"]
        );
        // `s` is serve's unlisted alias: its answer differs from the root's.
        let nested = backend.complete(&["s"], "--", &mut budget).unwrap();
        assert!(values(&nested).contains(&"--host"));
        assert_eq!(backend.confirms(&[], "s", &mut budget), Some(true));
        assert_eq!(backend.confirms(&[], "sevre", &mut budget), None);
        assert_eq!(
            values(&backend.complete(&["deploy"], "", &mut budget).unwrap()),
            ["staging", "production", "qa"]
        );
        assert!(matches!(
            backend.complete(&["deploy", "staging"], "", &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
        assert!(matches!(
            backend.complete(&["serve"], "", &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
        let queries = app.queries();
        assert!(
            !queries.contains("app.js") && !queries.contains("--config"),
            "{queries}"
        );
        assert!(queries.contains("zsh|<db><>"), "{queries}");
        assert!(!app.marker());
        let capabilities = backend.capabilities();
        assert!(capabilities.descriptions && capabilities.short_options && capabilities.values);
        assert!(!capabilities.complete_options && !capabilities.complete_subcommands);
        assert!(!capabilities.option_arity && !capabilities.resources);
        assert_eq!(backend.cache_identity(), None);
    }

    #[test]
    fn option_choices_are_values_and_resources_while_listed_commands_are_not() {
        let app = Fixture::new("yargs-values");
        let mut backend = app.backend();
        backend.network = true;
        let mut budget = budget();
        let formats = backend
            .complete_values(&["deploy", "--format"], &mut budget)
            .unwrap();
        assert_eq!(values(&formats), ["json", "yaml", "table"]);
        assert!(backend.candidate_is_resource(&["deploy", "--format"], &formats[0]));
        let spawned = budget.spawned();
        assert_eq!(
            backend
                .complete_offline_values(&["deploy", "--format"], &mut budget)
                .unwrap(),
            formats
        );
        assert_eq!(budget.spawned(), spawned);
        for words in [
            &["serve", "--host"][..],
            &["deploy", "--verbose"],
            &["deploy", "--format=json"],
        ] {
            assert!(
                matches!(
                    backend.complete_values(words, &mut budget),
                    Err(CompletionError::Unsupported(_))
                ),
                "{words:?}"
            );
        }
        let root = backend.complete(&[], "", &mut budget).unwrap();
        assert!(!backend.candidate_is_resource(&[], &root[0]));
        let deploy = backend.complete(&["deploy"], "", &mut budget).unwrap();
        assert!(backend.candidate_is_resource(&["deploy"], &deploy[0]));
        assert!(!app.marker());
    }

    #[test]
    fn changed_apps_bad_answers_limits_and_control_characters_fail_closed() {
        let app = Fixture::new("yargs-gates");
        let backend = app.backend();
        let mut budget = budget();
        assert!(matches!(
            backend.complete(&["a\nb"], "", &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
        assert_eq!(budget.spawned(), 0);
        std::fs::write(
            app.dir.0.join("lib/node_modules/fixture/package.json"),
            r#"{"name": "fixture", "version": "2.0.0"}"#,
        )
        .unwrap();
        assert!(matches!(
            backend.complete(&[], "", &mut budget),
            Err(CompletionError::Failed(why)) if why.contains("changed")
        ));
        assert_eq!(budget.spawned(), 0);
        for mode in ["failed", "utf8", "disagree"] {
            let app = Fixture::new("yargs-failure");
            std::fs::write(app.dir.0.join("mode"), mode).unwrap();
            let mut budget = self::budget();
            assert!(
                matches!(
                    app.backend().complete(&[], "", &mut budget),
                    Err(CompletionError::Failed(_))
                ),
                "{mode}"
            );
        }
        let app = Fixture::new("yargs-limit");
        let mut budget = self::budget();
        budget.max_candidates = 2;
        assert!(matches!(
            app.backend().complete(&[], "", &mut budget),
            Err(CompletionError::Failed(why)) if why.contains("limit")
        ));
    }

    #[test]
    fn discovery_needs_a_yargs_dependency_both_trust_settings_and_yargs_help() {
        let app = Fixture::new("yargs-discovery");
        let trusted = ["npm:fixture".to_owned()];
        let mut budget = budget();
        let discover = |completers: &[String], help: &[String], budget: &mut Budget| {
            discover_for_shell_with_budget(
                "fixture",
                &app.bin,
                completers,
                help,
                Shell::Bash,
                budget,
            )
        };
        assert!(!matches!(
            discover(&trusted, &[], &mut budget),
            Discovery::Found(_)
        ));
        assert!(!matches!(
            discover(&[], &trusted, &mut budget),
            Discovery::Found(_)
        ));
        assert_eq!(budget.spawned(), 0);
        assert!(matches!(
            discover(&trusted, &trusted, &mut budget),
            Discovery::Found(backend) if backend.flavor == Flavor::Yargs
                && backend.identity.as_deref() == Some("npm:fixture")
        ));
        assert_eq!(budget.spawned(), 1);
        assert!(!app.marker());
        // Help without yargs's own help option is not yargs's.
        std::fs::write(
            app.dir.0.join("help"),
            "Usage: fixture [options]\n  -h, --help  display help\n",
        )
        .unwrap();
        assert!(!matches!(
            discover(&trusted, &trusted, &mut budget),
            Discovery::Found(Backend {
                flavor: Flavor::Yargs,
                ..
            })
        ));
        // A package that doesn't depend on yargs is never asked for help.
        std::fs::write(app.dir.0.join("help"), HELP).unwrap();
        std::fs::write(
            app.dir.0.join("lib/node_modules/fixture/package.json"),
            r#"{"name": "fixture", "dependencies": {"commander": "^14"}}"#,
        )
        .unwrap();
        assert!(!depends_on_yargs(&app.bin));
        assert!(!app.marker());
    }

    #[test]
    fn the_engine_repairs_commands_aliases_options_and_gates_choice_values() {
        use crate::engine::{self, FailureContext, safety::Decision};
        use crate::settings::Settings;
        use crate::types::Context;
        let app = Fixture::new("yargs-engine");
        let backend = app.backend();
        let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into())
            .with_which("fixture", Some(app.bin.to_str().unwrap()))
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
            report.outcome.candidates().first().cloned().unwrap()
        };
        for (source, script) in [
            ("fixture sevre", "fixture serve"),
            ("fixture db mgirate", "fixture db migrate"),
            ("fixture s --hots x", "fixture s --host x"),
            ("fixture --verbsoe db seed", "fixture --verbose db seed"),
        ] {
            let candidate = first(source);
            assert_eq!(candidate.script, script);
            assert!(
                candidate.edits.iter().all(|edit| !edit.resource),
                "{source}"
            );
            assert_eq!(candidate.safety.decision, Decision::Allow, "{source}");
        }
        for (source, script) in [
            (
                "fixture deploy --format jsno",
                "fixture deploy --format json",
            ),
            ("fixture deploy stagign", "fixture deploy staging"),
        ] {
            let candidate = first(source);
            assert_eq!(candidate.script, script);
            assert!(candidate.edits.iter().all(|edit| edit.resource), "{source}");
            assert_eq!(candidate.safety.decision, Decision::Confirm, "{source}");
        }
        // A partial list: `a` may be a valid argument it omits, and `qa`
        // (half its letters differ) is no evidence against it.
        let backend: std::rc::Rc<dyn NativeCompletionBackend> = std::rc::Rc::new(backend.clone());
        let failure = FailureContext {
            source: "fixture deploy a".into(),
            ..FailureContext::default()
        };
        let report =
            engine::correct_with_backends(&failure, &ctx, vec![("fixture".into(), backend)]);
        assert!(
            report.outcome.candidates().is_empty(),
            "{:?}",
            report.outcome
        );
        assert_eq!(first("fixture deploy q").script, "fixture deploy qa");
        assert!(!app.queries().contains("<x>"));
        assert!(!app.marker());
    }

    #[test]
    fn upgrades_are_answered_by_the_installed_app_on_the_next_request() {
        let app = Fixture::new("yargs-upgrade");
        let before = app.backend();
        let mut budget = budget();
        let listed = before.complete(&[], "", &mut budget).unwrap();
        assert!(!values(&listed).contains(&"rollback"));
        std::fs::write(app.dir.0.join("answers/root.plain"), "serve\nrollback\n").unwrap();
        std::fs::write(
            app.dir.0.join("answers/root.zsh"),
            "serve:Start the server\nrollback:Undo\n",
        )
        .unwrap();
        let mut after = before.clone();
        after.memo = Default::default();
        let listed = after.complete(&[], "", &mut budget).unwrap();
        assert_eq!(listed[1].value, "rollback");
        assert_eq!(listed[1].description.as_deref(), Some("Undo"));
    }
}
