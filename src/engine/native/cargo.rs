//! Cargo exposes installed commands, aliases, and extensions through --list.
//! Help is a separate permission: aliases may launch shells, and extensions
//! are separate executables. Never forward failed arguments to a help probe.
//! Project values come from offline --no-deps metadata, never a build or run.

use super::{
    Backend, Capabilities, CompletionError, CompletionItem, Flavor, NativeCompletionBackend, Trust,
};
use crate::engine::{
    docs,
    probe::{self, Budget, Capture, Probe},
};
use crate::shells::Shell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub(super) struct Protocol {
    pub trusted: Vec<String>,
    pub trusted_help: Vec<String>,
    pub shell: Shell,
    fingerprint: String,
    delegates: Rc<RefCell<HashMap<PathBuf, Option<Backend>>>>,
    commands: Rc<RefCell<HashMap<String, Kind>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Builtin,
    Alias,
    External(PathBuf),
}

#[derive(Clone, Debug)]
struct Command {
    item: CompletionItem,
    kind: Kind,
}

#[derive(Default)]
struct Scope<'a> {
    selector: Option<&'a str>,
    configs: Vec<&'a str>,
    command: Option<&'a str>,
    args: &'a [&'a str],
}

// Input grammar for metadata requests; candidates always come from Cargo.
const GLOBAL_VALUES: &[&str] = &["--config", "--color", "-Z", "-C", "--explain"];
const GLOBAL_FLAGS: &[&str] = &[
    "-v",
    "-vv",
    "--verbose",
    "-q",
    "--quiet",
    "--offline",
    "--locked",
    "--frozen",
    "--help",
    "-h",
    "--version",
    "-V",
    "--list",
];

fn scope<'a>(words: &'a [&'a str]) -> Result<Scope<'a>, CompletionError> {
    let mut result = Scope::default();
    let mut i = 0;
    if words.first().is_some_and(|word| word.starts_with('+')) {
        result.selector = Some(words[0]);
        i += 1;
    }
    while let Some(&word) = words.get(i) {
        if word.contains(char::is_control) {
            return Err(CompletionError::Failed(
                "Cargo metadata context contains control characters".into(),
            ));
        }
        let (name, attached) = word
            .split_once('=')
            .map_or((word, None), |(name, value)| (name, Some(value)));
        if GLOBAL_VALUES.contains(&name) {
            let value = attached.or_else(|| {
                i += 1;
                words.get(i).copied()
            });
            if name == "--config"
                && let Some(value) = value
            {
                result.configs.push(value);
            }
            // -C changes the project directory and -Z may change dispatch.
            // Their semantics need their own audit; do not guess the scope.
            if matches!(name, "-C" | "-Z" | "--explain") {
                return Err(CompletionError::Failed(format!(
                    "Cargo metadata does not support context flag {name}"
                )));
            }
        } else if GLOBAL_FLAGS.contains(&name) {
        } else if word.starts_with('-') {
            return Err(CompletionError::Failed(format!(
                "Cargo metadata does not support context flag {name}"
            )));
        } else {
            result.command = Some(word);
            result.args = &words[i + 1..];
            if result.args.iter().any(|word| {
                word.contains(char::is_control)
                    || matches!(word.split('=').next(), Some("--config" | "-C" | "-Z"))
            }) {
                return Err(CompletionError::Unsupported(
                    "Cargo metadata does not support control characters or dispatch flags after the command".into(),
                ));
            }
            break;
        }
        i += 1;
    }
    Ok(result)
}

pub(super) fn root_context(words: &[&str]) -> bool {
    scope(words).is_ok_and(|scope| scope.command.is_none())
}

pub(super) fn resource_value(words: &[&str]) -> bool {
    words.last().is_some_and(|word| {
        [
            "-p",
            "--package",
            "--bin",
            "--example",
            "--test",
            "--bench",
            "-F",
            "--features",
        ]
        .contains(word)
    })
}

pub(super) fn feature_value(words: &[&str]) -> bool {
    matches!(words.last(), Some(&"--features" | &"-F"))
}

pub(super) fn feature_items(
    items: Vec<CompletionItem>,
    typed: &str,
) -> Result<Vec<CompletionItem>, CompletionError> {
    let mut unknown = None;
    let mut offset = 0;
    for component in typed.split_inclusive([',', ' ']) {
        let word = component.trim_end_matches([',', ' ']);
        if !word.is_empty() && !items.iter().any(|item| item.value == word) {
            if unknown.is_some() {
                return Err(CompletionError::Unsupported(
                    "Cargo feature lists with multiple unknown components require separate corrections".into(),
                ));
            }
            unknown = Some(offset..offset + word.len());
        }
        offset += component.len();
    }
    let Some(range) = unknown else {
        return Ok(vec![CompletionItem {
            value: typed.into(),
            takes_value: None,
            description: None,
        }]);
    };
    Ok(items
        .into_iter()
        .map(|mut item| {
            item.value = format!(
                "{}{}{}",
                &typed[..range.start],
                item.value,
                &typed[range.end..]
            );
            item
        })
        .collect())
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"_-".contains(&ch))
}

fn external(name: &str) -> Option<PathBuf> {
    let executable = format!("cargo-{name}{}", std::env::consts::EXE_SUFFIX);
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::utils::expand_user("~/.cargo"));
    let installed = home.join("bin").join(&executable);
    if installed.is_absolute() && installed.is_file() {
        return Some(installed);
    }
    crate::utils::which(&executable)
}

fn commands(text: &str, limit: usize) -> Result<Vec<Command>, CompletionError> {
    let mut lines = text.lines();
    if lines.next() != Some("Installed Commands:") {
        return Err(CompletionError::Failed(
            "Cargo did not return an installed-command list".into(),
        ));
    }
    let mut result = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with("    ") {
            return Err(CompletionError::Failed(
                "malformed Cargo command-list row".into(),
            ));
        }
        let row = line.trim_start();
        let (name, description) = row
            .split_once(char::is_whitespace)
            .map_or((row, ""), |(name, rest)| (name, rest.trim_start()));
        if !safe_name(name)
            || result
                .iter()
                .any(|command: &Command| command.item.value == name)
        {
            return Err(CompletionError::Failed(
                "invalid or duplicate Cargo command name".into(),
            ));
        }
        if result.len() >= limit {
            return Err(CompletionError::Failed(
                "Cargo command list exceeded the candidate limit".into(),
            ));
        }
        let kind = if description.starts_with("alias: ") {
            Kind::Alias
        } else if Path::new(description).is_absolute()
            || description.starts_with("./")
            || description.starts_with("../")
            || description.contains(std::path::MAIN_SEPARATOR)
                && description.ends_with(&format!("cargo-{name}{}", std::env::consts::EXE_SUFFIX))
            || description.contains(std::path::MAIN_SEPARATOR)
                && !description.contains(char::is_whitespace)
        {
            Kind::External(PathBuf::from(description))
        } else {
            Kind::Builtin
        };
        result.push(Command {
            item: CompletionItem {
                value: name.into(),
                takes_value: None,
                description: super::clean_description(description),
            },
            kind,
        });
    }
    if result.is_empty() {
        return Err(CompletionError::Failed(
            "Cargo command list is empty".into(),
        ));
    }
    Ok(result)
}

impl Protocol {
    pub fn new(path: &Path) -> Self {
        Self {
            trusted: Vec::new(),
            trusted_help: Vec::new(),
            shell: Shell::Bash,
            fingerprint: super::super::cache::fingerprint(&[path.to_owned()], &["cargo"]),
            delegates: Default::default(),
            commands: Default::default(),
        }
    }

    fn help_allowed(&self, name: &str) -> bool {
        self.trusted_help
            .iter()
            .any(|trusted| trusted == "*" || trusted == name || trusted == "rust:cargo")
    }

    pub fn feature_value(&self, words: &[&str]) -> bool {
        feature_value(words)
            && scope(words)
                .ok()
                .and_then(|scope| scope.command)
                .is_some_and(|command| self.commands.borrow().get(command) == Some(&Kind::Builtin))
    }

    pub fn capabilities(&self, name: &str) -> Capabilities {
        let help = self.help_allowed(name);
        Capabilities {
            subcommands: true,
            options: help || !self.trusted.is_empty(),
            option_arity: help,
            values: help || !self.trusted.is_empty(),
            resources: false,
            option_prefix: "-",
            short_options: help,
            complete_options: false,
            complete_subcommands: true,
            descriptions: true,
            query_dialect: None,
            trust: Trust::UserTrusted,
        }
    }

    fn checked(
        &self,
        backend: &Backend,
    ) -> Result<super::identity::CargoInstallation, CompletionError> {
        if self.fingerprint
            != super::super::cache::fingerprint(
                std::slice::from_ref(&backend.completer),
                &["cargo"],
            )
        {
            return Err(CompletionError::Failed(
                "Cargo executable changed after discovery".into(),
            ));
        }
        super::identity::cargo_installation(&backend.completer).ok_or_else(|| {
            CompletionError::Failed("Cargo installation binding changed after discovery".into())
        })
    }

    fn raw(
        &self,
        backend: &Backend,
        path: &Path,
        args: &[String],
        help: bool,
        budget: &mut Budget,
    ) -> Result<String, CompletionError> {
        self.checked(backend)?;
        let mut key = vec!["cargo".to_owned(), path.to_string_lossy().into_owned()];
        // Aliases and extension dispatch can change without a Cargo upgrade.
        // Refresh request-local command/help answers when their local config
        // or executable directories change as well.
        let mut files = Vec::new();
        if let Ok(cwd) = std::env::current_dir() {
            files.push(cwd.clone());
            for ancestor in cwd.ancestors() {
                files.extend([
                    ancestor.join(".cargo/config"),
                    ancestor.join(".cargo/config.toml"),
                ]);
            }
        }
        let home = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::utils::expand_user("~/.cargo"));
        files.extend([
            home.join("config"),
            home.join("config.toml"),
            home.join("bin"),
        ]);
        if let Some(paths) = std::env::var_os("PATH") {
            files.extend(std::env::split_paths(&paths));
        }
        files.extend(
            args.iter()
                .filter_map(|arg| arg.strip_prefix("--config="))
                .map(PathBuf::from),
        );
        key.push(super::super::cache::fingerprint(&files, &["cargo context"]));
        key.extend_from_slice(args);
        let key: Vec<&str> = key.iter().map(String::as_str).collect();
        backend.memoized(&key, || {
            let mut env = super::offline_env(Flavor::Cargo);
            env.push(("PAGER".into(), Some("cat".into())));
            let output = probe::run(
                &Probe {
                    program: path,
                    args: args.iter().map(Into::into).collect(),
                    env,
                    capture: if help {
                        Capture::Combined
                    } else {
                        Capture::Stdout
                    },
                },
                budget,
            )
            .map_err(super::probe_error)?;
            if output.status != Some(0) {
                return Err(CompletionError::Failed(format!(
                    "Cargo metadata exited with {:?}",
                    output.status
                )));
            }
            if output.truncated {
                return Err(CompletionError::Failed(
                    "Cargo metadata output exceeded the probe limit".into(),
                ));
            }
            String::from_utf8(output.data).map_err(|_| {
                CompletionError::Failed("Cargo metadata output is not valid UTF-8".into())
            })
        })
    }

    fn args(&self, backend: &Backend, scope: &Scope<'_>) -> Result<Vec<String>, CompletionError> {
        let installation = self.checked(backend)?;
        let mut args = Vec::new();
        if let Some(selector) = scope.selector {
            if installation.rustup.is_none() || !selector.strip_prefix('+').is_some_and(safe_name) {
                return Err(CompletionError::Failed(
                    "Cargo toolchain selector is unsupported for this installation".into(),
                ));
            }
            args.push(selector.into());
        }
        for config in &scope.configs {
            if config.contains(char::is_control) {
                return Err(CompletionError::Failed(
                    "Cargo config argument contains control characters".into(),
                ));
            }
            args.push(format!("--config={config}"));
        }
        args.extend(["--offline".into(), "--color=never".into()]);
        Ok(args)
    }

    fn list(
        &self,
        backend: &Backend,
        scope: &Scope<'_>,
        budget: &mut Budget,
    ) -> Result<Vec<Command>, CompletionError> {
        let mut args = self.args(backend, scope)?;
        args.extend(["--list".into(), "--verbose".into()]);
        let commands = commands(
            &self.raw(backend, &backend.completer, &args, false, budget)?,
            budget.max_candidates,
        )?;
        *self.commands.borrow_mut() = commands
            .iter()
            .map(|command| (command.item.value.clone(), command.kind.clone()))
            .collect();
        Ok(commands)
    }

    fn toolchains(
        &self,
        backend: &Backend,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let Some(rustup) = self.checked(backend)?.rustup else {
            return Ok(Vec::new());
        };
        let listing = self.raw(
            backend,
            &rustup,
            &["toolchain".into(), "list".into()],
            false,
            budget,
        )?;
        let show = self.raw(backend, &rustup, &["show".into()], false, budget)?;
        let host = show
            .lines()
            .find_map(|line| line.strip_prefix("Default host: "))
            .filter(|host| safe_name(host));
        let mut items: Vec<CompletionItem> = Vec::new();
        for line in listing.lines() {
            let Some(name) = line
                .split_whitespace()
                .next()
                .filter(|name| safe_name(name))
            else {
                continue;
            };
            let short = host.and_then(|host| name.strip_suffix(&format!("-{host}")));
            for name in std::iter::once(name).chain(short) {
                let value = format!("+{name}");
                if !items.iter().any(|item| item.value == value) {
                    items.push(CompletionItem {
                        value,
                        takes_value: None,
                        description: Some("Installed Rust toolchain".into()),
                    });
                }
            }
        }
        if items.len() > budget.max_candidates {
            return Err(CompletionError::Failed(
                "installed toolchain list exceeded the candidate limit".into(),
            ));
        }
        Ok(items)
    }

    fn help(
        &self,
        backend: &Backend,
        scope: &Scope<'_>,
        budget: &mut Budget,
    ) -> Result<String, CompletionError> {
        if !self.help_allowed(&backend.name) {
            return Err(CompletionError::Unsupported(
                "Cargo option/nested metadata requires trusted_help".into(),
            ));
        }
        let args = self.args(backend, scope)?;
        let mut path = Vec::new();
        if let Some(command) = scope.command {
            let listed = self.list(backend, scope, budget)?;
            let Some(entry) = listed.iter().find(|entry| entry.item.value == command) else {
                return Err(CompletionError::Failed(
                    "Cargo did not list the requested command".into(),
                ));
            };
            match &entry.kind {
                Kind::Alias => {
                    return Err(CompletionError::Failed(
                        "Cargo aliases are listed but never executed for help".into(),
                    ));
                }
                Kind::External(_) => {
                    let name = format!("cargo-{command}");
                    if !self
                        .trusted_help
                        .iter()
                        .any(|trusted| trusted == "*" || *trusted == name)
                    {
                        return Err(CompletionError::Failed(format!(
                            "Cargo extension help requires trusted_help for {name}"
                        )));
                    }
                }
                Kind::Builtin => {
                    // Some bundled commands delegate despite having builtin
                    // descriptions. A same-name plugin may also be shadowed
                    // by a builtin; always ask Cargo, never borrow its tree.
                    if external(command).is_some()
                        && !self
                            .trusted_help
                            .iter()
                            .any(|trusted| trusted == "*" || *trusted == format!("cargo-{command}"))
                    {
                        return Err(CompletionError::Failed(format!(
                            "Cargo help may invoke cargo-{command}; its trusted_help permission is required"
                        )));
                    }
                }
            }
            path.push(command.to_owned());
        }
        // Only names advertised by each parent's help may extend the path.
        let mut text = self.help_text(backend, &args, &path, budget)?;
        for word in scope.args {
            if word.starts_with('-')
                || !docs::subcommands(&text)
                    .iter()
                    .any(|item| item.value == *word)
            {
                break;
            }
            path.push((*word).into());
            text = self.help_text(backend, &args, &path, budget)?;
        }
        Ok(text)
    }

    fn help_text(
        &self,
        backend: &Backend,
        scope: &[String],
        path: &[String],
        budget: &mut Budget,
    ) -> Result<String, CompletionError> {
        let mut args = scope.to_vec();
        args.extend_from_slice(path);
        args.push("--help".into());
        self.raw(backend, &backend.completer, &args, true, budget)
    }

    fn delegate_backend(
        &self,
        backend: &Backend,
        scope: &Scope<'_>,
        entry: &Command,
        budget: &mut Budget,
    ) -> Result<Option<Backend>, CompletionError> {
        let Kind::External(path) = &entry.kind else {
            return Ok(None);
        };
        if !path.is_absolute() {
            return Err(CompletionError::Failed(
                "Cargo extension path is not absolute".into(),
            ));
        }
        if std::fs::canonicalize(path)
            .ok()
            .and_then(|path| path.file_name().map(|name| name == "rustup"))
            .unwrap_or(false)
        {
            return Ok(None);
        }
        if scope.selector.is_some() || !scope.configs.is_empty() {
            // A direct plugin process cannot reproduce rustup/Cargo config's
            // environment or toolchain binding yet.
            return Ok(None);
        }
        if !self.delegates.borrow().contains_key(path) {
            let name = format!("cargo-{}", entry.item.value);
            let discovered = super::discover_for_shell_with_budget(
                &name,
                path,
                &self.trusted,
                &self.trusted_help,
                self.shell,
                budget,
            );
            let found = match discovered {
                super::Discovery::Found(backend) if backend.flavor != Flavor::Cargo => {
                    Some(backend)
                }
                _ => None,
            };
            self.delegates.borrow_mut().insert(path.clone(), found);
        }
        let found = self.delegates.borrow().get(path).cloned().flatten();
        let Some(delegate) = found else {
            return Ok(None);
        };
        self.checked(backend)?;
        Ok(Some(delegate))
    }

    fn delegate(
        &self,
        backend: &Backend,
        scope: &Scope<'_>,
        entry: &Command,
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Option<Vec<CompletionItem>>, CompletionError> {
        let Some(delegate) = self.delegate_backend(backend, scope, entry, budget)? else {
            return Ok(None);
        };
        // Cargo's external-command convention includes the command's name as
        // its first argument. Preserve it in the plugin's completion context.
        let mut words = vec![entry.item.value.as_str()];
        words.extend_from_slice(scope.args);
        Ok(Some(delegate.complete(&words, prefix, budget)?))
    }

    pub fn complete(
        &self,
        backend: &Backend,
        words: &[&str],
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let scope = scope(words)?;
        if let Some(command) = scope.command {
            let entries = self.list(backend, &scope, budget)?;
            if let Some(entry) = entries.iter().find(|entry| entry.item.value == command)
                && let Some(items) = self.delegate(backend, &scope, entry, prefix, budget)?
            {
                return Ok(items);
            }
        }
        let mut items = if prefix.starts_with('-') {
            docs::options(&self.help(backend, &scope, budget)?)
        } else if scope.command.is_none() {
            let mut items: Vec<_> = self
                .list(backend, &scope, budget)?
                .into_iter()
                .map(|entry| entry.item)
                .collect();
            if words.is_empty() {
                items.extend(self.toolchains(backend, budget)?);
            }
            if items.len() > budget.max_candidates {
                return Err(CompletionError::Failed(
                    "Cargo commands and toolchains exceeded the candidate limit".into(),
                ));
            }
            items
        } else {
            docs::subcommands(&self.help(backend, &scope, budget)?)
        };
        items.retain(|item| item.value.starts_with(prefix));
        items.truncate(budget.max_candidates);
        Ok(items)
    }

    pub fn values(
        &self,
        backend: &Backend,
        words: &[&str],
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let Some(option) = words.last().copied() else {
            return Ok(Vec::new());
        };
        let scope = scope(words)?;
        if let Some(command) = scope.command {
            let entries = self.list(backend, &scope, budget)?;
            if let Some(entry) = entries.iter().find(|entry| entry.item.value == command)
                && let Some(delegate) = self.delegate_backend(backend, &scope, entry, budget)?
            {
                let mut words = vec![command];
                words.extend_from_slice(scope.args);
                return delegate.complete_offline_values(&words, budget);
            }
        }
        let help = self.help(backend, &scope, budget)?;
        let options = docs::options(&help);
        if !options.iter().any(|item| item.value == option) {
            return Ok(Vec::new());
        }
        if resource_value(words) {
            return self.project_values(backend, &scope, &options, option, budget);
        }
        Ok(docs::option_values(&help, option))
    }

    fn project_values(
        &self,
        backend: &Backend,
        scope: &Scope<'_>,
        options: &[CompletionItem],
        option: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let entries = self.list(backend, scope, budget)?;
        if !entries
            .iter()
            .any(|entry| entry.item.value == "metadata" && entry.kind == Kind::Builtin)
        {
            return Err(CompletionError::Failed(
                "Cargo metadata command is unavailable or shadowed".into(),
            ));
        }
        let mut args = self.args(backend, scope)?;
        args.extend([
            "metadata".into(),
            "--offline".into(),
            "--locked".into(),
            "--no-deps".into(),
            "--format-version=1".into(),
        ]);
        let mut selected = Vec::new();
        let mut workspace = false;
        let mut iter = scope.args.iter().copied();
        while let Some(word) = iter.next() {
            if word == "--" {
                break;
            }
            let (name, value) = word
                .split_once('=')
                .map_or((word, None), |(name, value)| (name, Some(value)));
            if name == "--workspace" {
                workspace = true;
            } else if name == "--exclude" {
                return Err(CompletionError::Unsupported(
                    "Cargo metadata does not yet reproduce package exclusions".into(),
                ));
            } else if ["--manifest-path", "-p", "--package"].contains(&name) {
                let value = value.or_else(|| iter.next());
                if let Some(value) = value {
                    if name == "--manifest-path" {
                        args.push(format!("--manifest-path={value}"));
                    } else {
                        selected.push(value);
                    }
                }
            } else if value.is_none()
                && options
                    .iter()
                    .any(|item| item.value == name && item.takes_value == Some(true))
            {
                iter.next();
            }
        }
        let text = self.raw(backend, &backend.completer, &args, false, budget)?;
        project_items(&text, option, &selected, workspace, budget.max_candidates)
    }
}

fn project_items(
    text: &str,
    option: &str,
    selected: &[&str],
    workspace: bool,
    limit: usize,
) -> Result<Vec<CompletionItem>, CompletionError> {
    let data: serde_json::Value = serde_json::from_str(text)
        .map_err(|_| CompletionError::Failed("Cargo metadata is not valid JSON".into()))?;
    let members = data
        .get("workspace_members")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            CompletionError::Failed("Cargo metadata has no workspace member list".into())
        })?;
    let packages = data
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| CompletionError::Failed("Cargo metadata has no package list".into()))?;
    let defaults = match data.get("workspace_default_members") {
        Some(value) => value.as_array().ok_or_else(|| {
            CompletionError::Failed("Cargo metadata has an invalid default member list".into())
        })?,
        None => members,
    };
    let mut items: Vec<CompletionItem> = Vec::new();
    let mut add = |value: &str| -> Result<(), CompletionError> {
        if value.is_empty() || value.contains(char::is_control) {
            return Err(CompletionError::Failed(
                "Cargo metadata contains an invalid resource name".into(),
            ));
        }
        if !items.iter().any(|item| item.value == value) {
            if items.len() >= limit {
                return Err(CompletionError::Failed(
                    "Cargo metadata exceeded the candidate limit".into(),
                ));
            }
            items.push(CompletionItem {
                value: value.into(),
                takes_value: None,
                description: None,
            });
        }
        Ok(())
    };
    for package in packages {
        let id = package
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| CompletionError::Failed("Cargo package metadata has no id".into()))?;
        if !members.iter().any(|member| member.as_str() == Some(id)) {
            continue;
        }
        let name = package
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| CompletionError::Failed("Cargo package metadata has no name".into()))?;
        if matches!(option, "-p" | "--package") {
            add(name)?;
            continue;
        }
        if !selected.is_empty()
            && !selected
                .iter()
                .any(|selected| *selected == name || *selected == id)
        {
            continue;
        }
        if selected.is_empty()
            && !workspace
            && !defaults.iter().any(|member| member.as_str() == Some(id))
        {
            continue;
        }
        if matches!(option, "--features" | "-F") {
            let features = package
                .get("features")
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| {
                    CompletionError::Failed("Cargo package metadata has no features".into())
                })?;
            for feature in features.keys() {
                add(feature)?;
                add(&format!("{name}/{feature}"))?;
            }
        } else {
            let kind = option.trim_start_matches('-');
            let targets = package
                .get("targets")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    CompletionError::Failed("Cargo package metadata has no targets".into())
                })?;
            for target in targets {
                let kinds = target
                    .get("kind")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| {
                        CompletionError::Failed("Cargo target metadata has no kind list".into())
                    })?;
                if kinds.iter().any(|entry| entry.as_str() == Some(kind)) {
                    let name = target
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| {
                            CompletionError::Failed("Cargo target metadata has no name".into())
                        })?;
                    add(name)?;
                }
            }
        }
    }
    Ok(items)
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::{Discovery, discover, discover_for_shell_with_budget, tests::Dir};
    use super::*;
    use std::time::Duration;

    const APP: &str = r#"#!/bin/sh
root=$(dirname "$(dirname "$0")")
[ "$RUSTUP_AUTO_INSTALL" = 0 ] && [ "$CARGO_NET_OFFLINE" = true ] || exit 8
[ "$CARGO_HTTP_PROXY" = http://127.0.0.1:9 ] && [ -z "$CARGO_COMPLETE" ] || exit 8
printf '%s:' "$(basename "$0")" >> "$root/queries"
for arg in "$@"; do printf '<%s>' "$arg" >> "$root/queries"; done
printf '\n' >> "$root/queries"
if [ "$(basename "$0")" = rustup ]; then
  case "$*" in
    'toolchain list') printf 'stable-test-host (default)\nnightly-test-host\n'; exit;;
    show) printf 'Default host: test-host\n'; exit;;
  esac
  touch "$root/operation-marker"; exit 9
fi
case "$1" in +*) shift;; esac
while [ "$#" -gt 0 ]; do
  case "$1" in --config=*|--offline|--color=never) shift;; *) break;; esac
done
if [ -f "$root/result-mode" ]; then
  case "$(cat "$root/result-mode")" in
    failed) printf 'Installed Commands:\n    build\n'; exit 101;;
    utf8) printf '\377'; exit 0;;
    oversized) printf '%8192s' x; exit 0;;
    hanging) (sleep 1; touch "$root/delayed-marker") & wait; exit 0;;
  esac
fi
case "$*" in
  '--list --verbose') cat "$root/commands";;
  --help) cat "$root/root-help";;
  'build --help'|'metadata --help'|'version --help') cat "$root/build-help";;
  'report --help') printf 'Commands:\n  future-incompatibilities  Show reports\n';;
  'report future-incompatibilities --help') printf 'Options:\n  --id <ID>  Choose report\n';;
  'metadata --offline --locked --no-deps --format-version=1'*) cat "$root/metadata.json";;
  *) touch "$root/operation-marker"; exit 9;;
esac
"#;

    const ROOT: &str = "Installed Commands:\n    build  Compile the project\n    metadata  Read workspace metadata\n    report  Read reports\n    version  Show Cargo version\n    danger  alias: !touch operation-marker\n";
    const HELP: &str = "Commands:\n  build  Compile the project\n  metadata  Read metadata\nOptions:\n  --color <WHEN>  Color [possible values: auto, always, never]\n  --config <VALUE>  Configuration\n";
    const BUILD_HELP: &str = "Options:\n  --release  Optimized build\n  --color <WHEN>  Color [possible values: auto, always, never]\n  -p, --package [<SPEC>]  Package\n  --bin [<NAME>]  Binary\n  --example [<NAME>]  Example\n  --test [<NAME>]  Test\n  --bench [<NAME>]  Benchmark\n  --manifest-path <PATH>  Manifest\n  -F, --features <FEATURES>  Features\n";
    const METADATA: &str = r#"{"workspace_members":["id:demo","id:other"],"packages":[{"id":"id:demo","name":"demo","features":{"quiet-mode":[],"default":[]},"targets":[{"kind":["bin"],"name":"demo-bin"},{"kind":["example"],"name":"demo-example"},{"kind":["test"],"name":"demo-test"},{"kind":["bench"],"name":"demo-bench"},{"kind":["custom-build"],"name":"build-script-build"}]},{"id":"id:other","name":"other","features":{"other-feature":[]},"targets":[{"kind":["bin"],"name":"other-bin"}]}]}"#;

    fn app(dir: &Dir, proxy: bool) -> PathBuf {
        dir.script("commands", ROOT);
        dir.script("root-help", HELP);
        dir.script("build-help", BUILD_HELP);
        dir.script("metadata.json", METADATA);
        if proxy {
            dir.script("bin/rustup", APP);
            let cargo = dir.0.join("bin/cargo");
            std::os::unix::fs::symlink("rustup", &cargo).unwrap();
            cargo
        } else {
            dir.script("lib/rustlib/manifest-cargo-test-host", "file:bin/cargo\n");
            dir.script("bin/cargo", APP)
        }
    }

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(5), Duration::from_secs(2), 16)
    }

    fn backend(path: &Path, help: bool, budget: &mut Budget) -> Backend {
        let trust = vec!["rust:cargo".into()];
        let help = if help { trust.clone() } else { Vec::new() };
        let Discovery::Found(backend) =
            discover_for_shell_with_budget("cargo", path, &trust, &help, Shell::Bash, budget)
        else {
            panic!()
        };
        backend
    }

    #[test]
    fn receipt_and_proxy_identity_survive_renaming_and_untrusted_lookalikes_do_not_probe() {
        for proxy in [false, true] {
            let dir = Dir::new("cargo-identity");
            let path = app(&dir, proxy);
            assert!(matches!(
                discover("cargo", &path, &[]),
                Discovery::NotTrusted(_)
            ));
            let Discovery::Found(found) = discover("cargo", &path, &["rust:cargo".into()]) else {
                panic!()
            };
            assert_eq!(found.flavor, Flavor::Cargo);
            assert_eq!(found.identity.as_deref(), Some("rust:cargo"));
            if !proxy {
                let renamed = dir.0.join("renamed");
                std::os::unix::fs::symlink(&path, &renamed).unwrap();
                assert!(
                    matches!(discover("rust-packages", &renamed, &["rust:cargo".into()]), Discovery::Found(backend) if backend.flavor == Flavor::Cargo)
                );
            }
            assert!(!dir.0.join("queries").exists());
        }
        let dir = Dir::new("cargo-lookalike");
        let path = dir.script("cargo", "#!/bin/sh\ntouch operation-marker\n");
        assert_eq!(super::super::identity::identify(&path), None);
        assert!(
            !matches!(discover("cargo", &path, &["cargo".into()]), Discovery::Found(backend) if backend.flavor == Flavor::Cargo)
        );
        assert!(!dir.0.join("operation-marker").exists());
    }

    #[test]
    fn lists_installed_words_aliases_toolchains_and_fresh_extensions_without_executing_them() {
        let dir = Dir::new("cargo-list");
        let path = app(&dir, true);
        let extension = dir.script("bin/cargo-private", "#!/bin/sh\ntouch operation-marker\n");
        dir.script(
            "commands",
            &format!("{ROOT}    private  {}\n", extension.display()),
        );
        let mut budget = budget();
        let backend = backend(&path, false, &mut budget);
        let items = backend.complete(&[], "", &mut budget).unwrap();
        assert!(items.iter().any(|item| item.value == "danger"));
        assert!(items.iter().any(|item| item.value == "private"));
        let selector = items.iter().find(|item| item.value == "+stable").unwrap();
        assert!(backend.candidate_is_resource(&[], selector));
        assert!(!backend.candidate_is_resource(&[], &items[0]));
        assert_eq!(budget.spawned(), 3);
        let selected = backend.complete(&["+nightly"], "", &mut budget).unwrap();
        assert!(selected.iter().any(|item| item.value == "build"));
        let count = budget.spawned();
        backend.complete(&["+nightly"], "", &mut budget).unwrap();
        assert_eq!(budget.spawned(), count);
        assert!(
            std::fs::read_to_string(dir.0.join("queries"))
                .unwrap()
                .contains("<+nightly><--offline>")
        );
        assert_eq!(backend.cache_identity(), None);
        assert!(!dir.0.join("operation-marker").exists());
        dir.script(
            "commands",
            &format!(
                "{ROOT}    private  {}\n    future-tool  A newly installed command\n",
                extension.display()
            ),
        );
        let updated = self::backend(&path, false, &mut budget);
        assert!(
            updated
                .complete(&[], "", &mut budget)
                .unwrap()
                .iter()
                .any(|item| item.value == "future-tool")
        );
    }

    #[test]
    fn help_requires_separate_permission_and_never_dispatches_aliases_or_failed_arguments() {
        let dir = Dir::new("cargo-help");
        let path = app(&dir, false);
        let mut budget = budget();
        let untrusted = backend(&path, false, &mut budget);
        assert!(matches!(
            untrusted.complete(&["build"], "-", &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
        assert_eq!(budget.spawned(), 1, "only the command list was queried");
        let backend = backend(&path, true, &mut budget);
        let options = backend
            .complete(
                &["build", "--manifest-path", "$(touch operation-marker)"],
                "-",
                &mut budget,
            )
            .unwrap();
        assert!(options.iter().any(|item| item.value == "--release"));
        assert_eq!(
            options
                .iter()
                .find(|item| item.value == "-p")
                .unwrap()
                .takes_value,
            Some(true)
        );
        assert_eq!(
            backend
                .complete_values(&["build", "--color"], &mut budget)
                .unwrap()
                .iter()
                .map(|item| item.value.as_str())
                .collect::<Vec<_>>(),
            ["auto", "always", "never"]
        );
        assert!(backend.complete(&["danger"], "-", &mut budget).is_err());
        let nested = backend.complete(&["report"], "", &mut budget).unwrap();
        assert_eq!(nested[0].value, "future-incompatibilities");
        assert!(
            backend
                .complete(&["report", "future-incompatibilities"], "-", &mut budget)
                .unwrap()
                .iter()
                .any(|item| item.value == "--id")
        );
        let queries = std::fs::read_to_string(dir.0.join("queries")).unwrap();
        assert!(!queries.contains("operation-marker"), "{queries}");
        assert!(!queries.contains("<danger>"), "{queries}");
        assert!(!dir.0.join("operation-marker").exists());
        let config = dir.script("scope.toml", "first configuration\n");
        let config = config.to_str().unwrap();
        dir.script(
            "commands",
            &format!("{ROOT}    private  Builtin before config change\n"),
        );
        assert!(
            backend
                .complete(&["--config", config], "", &mut budget)
                .unwrap()
                .iter()
                .any(|item| item.value == "private")
        );
        dir.script("scope.toml", "changed configuration containing an alias\n");
        dir.script(
            "commands",
            &format!("{ROOT}    private  alias: !touch operation-marker\n"),
        );
        assert!(
            backend
                .complete(&["--config", config, "private"], "-", &mut budget)
                .is_err()
        );
        let queries = std::fs::read_to_string(dir.0.join("queries")).unwrap();
        assert!(!queries.contains("<private><--help>"), "{queries}");
        assert!(!dir.0.join("operation-marker").exists());
    }

    #[test]
    fn offline_metadata_selects_workspace_packages_targets_and_features_without_a_build() {
        let mut metadata: serde_json::Value = serde_json::from_str(METADATA).unwrap();
        metadata["workspace_default_members"] = serde_json::json!(["id:demo"]);
        let text = metadata.to_string();
        let defaults = project_items(&text, "--bin", &[], false, 32).unwrap();
        assert_eq!(
            defaults
                .iter()
                .map(|item| item.value.as_str())
                .collect::<Vec<_>>(),
            ["demo-bin"]
        );
        assert_eq!(
            project_items(&text, "--bin", &[], true, 32).unwrap().len(),
            2
        );
        assert_eq!(
            project_items(&text, "--package", &[], false, 32)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            project_items(&text, "--bin", &["other"], false, 32).unwrap()[0].value,
            "other-bin"
        );
        let dir = Dir::new("cargo-metadata");
        let path = app(&dir, false);
        let mut budget = budget();
        let backend = backend(&path, true, &mut budget);
        for (option, expected) in [
            ("--package", "demo"),
            ("--bin", "demo-bin"),
            ("--example", "demo-example"),
            ("--test", "demo-test"),
            ("--bench", "demo-bench"),
            ("--features", "quiet-mode"),
        ] {
            let items = backend
                .complete_values(&["build", option], &mut budget)
                .unwrap();
            assert!(
                items.iter().any(|item| item.value == expected),
                "{option}: {items:?}"
            );
            assert!(
                items
                    .iter()
                    .all(|item| backend.candidate_is_resource(&["build", option], item))
            );
            assert!(!items.iter().any(|item| item.value == "build-script-build"));
        }
        let selected = backend
            .complete_values(&["build", "-p", "other", "--bin"], &mut budget)
            .unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|item| item.value.as_str())
                .collect::<Vec<_>>(),
            ["other-bin"]
        );
        assert!(
            backend
                .complete_values(
                    &[
                        "build",
                        "--manifest-path",
                        "$(touch operation-marker)",
                        "--features"
                    ],
                    &mut budget
                )
                .is_ok()
        );
        let queries = std::fs::read_to_string(dir.0.join("queries")).unwrap();
        assert!(queries.contains("<metadata><--offline><--locked><--no-deps><--format-version=1>"));
        assert!(queries.contains("<--manifest-path=$(touch operation-marker)>"));
        assert!(!dir.0.join("operation-marker").exists());
        assert!(!dir.0.join("Cargo.lock").exists());
        assert_eq!(backend.cache_identity(), None);
    }

    #[test]
    fn malformed_lists_metadata_limits_status_and_timeout_fail_closed() {
        assert!(commands("Usage: another program\n", 10).is_err());
        assert!(commands("Installed Commands:\n    build\n    build\n", 10).is_err());
        assert!(commands("Installed Commands:\n    $(touch-marker)\n", 10).is_err());
        assert!(commands(ROOT, 1).is_err());
        assert!(matches!(
            commands(
                "Installed Commands:\n    private  local plugins/cargo-private\n",
                10
            )
            .unwrap()[0]
                .kind,
            Kind::External(_)
        ));
        assert!(project_items("[]", "--bin", &[], false, 10).is_err());
        assert!(project_items(METADATA, "--features", &[], false, 1).is_err());
        let dir = Dir::new("cargo-failures");
        let path = app(&dir, false);
        for (mode, limit, why) in [
            ("failed", 1024, "exited"),
            ("utf8", 1024, "UTF-8"),
            ("oversized", 64, "limit"),
            ("hanging", 1024, "timed out"),
        ] {
            dir.script("result-mode", mode);
            let mut budget = Budget::new(
                Duration::from_secs(3),
                if mode == "hanging" {
                    Duration::from_millis(250)
                } else {
                    Duration::from_secs(2)
                },
                1,
            );
            budget.max_output = limit;
            let backend = backend(&path, false, &mut budget);
            let result = backend.complete(&[], "", &mut budget);
            assert!(
                matches!(&result, Err(CompletionError::Failed(message)) if message.contains(why)),
                "{mode}: {result:?}"
            );
        }
        std::thread::sleep(Duration::from_millis(1050));
        assert!(!dir.0.join("delayed-marker").exists());
        let mut budget = Budget::new(Duration::ZERO, Duration::ZERO, 0);
        let backend = backend(&path, false, &mut budget);
        assert_eq!(
            backend.complete(&[], "", &mut budget),
            Err(CompletionError::BudgetExhausted)
        );
    }

    #[test]
    fn changed_executables_receipts_and_unsupported_scope_never_reach_dispatch() {
        let dir = Dir::new("cargo-revalidation");
        let path = app(&dir, false);
        let mut budget = budget();
        let backend = backend(&path, false, &mut budget);
        for words in [
            vec!["-C", "other"],
            vec!["-Z", "unstable-options"],
            vec!["+missing"],
            vec!["--config", "line\nbreak"],
        ] {
            assert!(backend.complete(&words, "", &mut budget).is_err());
        }
        assert_eq!(budget.spawned(), 0);
        dir.script(
            "lib/rustlib/manifest-cargo-test-host",
            "file:bin/different\n",
        );
        assert!(backend.complete(&[], "", &mut budget).is_err());
        assert_eq!(budget.spawned(), 0);
    }

    #[test]
    fn feature_lists_preserve_known_components_and_require_one_unknown_component() {
        let items = project_items(METADATA, "--features", &[], false, 32).unwrap();
        for (typed, expected) in [
            ("quiet-mdoe", "quiet-mode"),
            ("demo/quiet-mdoe", "demo/quiet-mode"),
            ("default,quiet-mdoe", "default,quiet-mode"),
            (
                "quiet-mdoe,other/other-feature",
                "quiet-mode,other/other-feature",
            ),
            ("default, demo/quiet-mdoe", "default, demo/quiet-mode"),
        ] {
            let choices = feature_items(items.clone(), typed).unwrap();
            assert!(
                choices.iter().any(|item| item.value == expected),
                "{typed}: {choices:?}"
            );
        }
        let valid = "default, demo/quiet-mode,other/other-feature";
        assert_eq!(feature_items(items.clone(), valid).unwrap()[0].value, valid);
        assert!(feature_items(items, "defualt,quiet-mdoe").is_err());
    }

    #[test]
    fn external_native_completion_needs_its_own_trust_and_cannot_reuse_another_scope() {
        let dir = Dir::new("cargo-native-plugin");
        let path = app(&dir, false);
        let plugin = dir.script(
            "bin/cargo-private",
            r#"#!/bin/sh
# PYTHON_ARGCOMPLETE_OK
root=$(dirname "$(dirname "$0")")
[ "$#" = 0 ] && [ "$_ARGCOMPLETE" = 1 ] || { touch "$root/operation-marker"; exit 9; }
printf '<%s>\n' "$COMP_LINE" >> "$root/plugin-queries"
case "$COMP_LINE" in
  *' private --format '*) printf 'json\013yaml' >&8;;
  *' private --'*) printf '%s\013' --format= --offline >&8;;
  *' private '*) printf 'future\013inspect' >&8;;
  *) touch "$root/operation-marker"; exit 9;;
esac
"#,
        );
        dir.script(
            "commands",
            &format!("{ROOT}    private  {}\n", plugin.display()),
        );
        let mut budget = budget();
        let untrusted = backend(&path, false, &mut budget);
        assert!(untrusted.complete(&["private"], "", &mut budget).is_err());
        assert!(!dir.0.join("plugin-queries").exists());
        let trusted = ["rust:cargo".into(), "cargo-private".into()];
        let Discovery::Found(found) =
            discover_for_shell_with_budget("cargo", &path, &trusted, &[], Shell::Bash, &mut budget)
        else {
            panic!()
        };
        let commands = found.complete(&["private"], "", &mut budget).unwrap();
        assert!(!found.value_syntax_contains_slashes(&["private", "--features"]));
        assert!(found.value_syntax_contains_slashes(&["build", "--features"]));
        assert!(commands.iter().any(|item| item.value == "future"));
        let options = found.complete(&["private"], "--", &mut budget).unwrap();
        assert!(options.iter().any(|item| item.value == "--format"));
        let values = found
            .complete_values(&["private", "--format"], &mut budget)
            .unwrap();
        assert!(values.iter().any(|item| item.value == "json"));
        let previous = std::fs::read_to_string(dir.0.join("plugin-queries")).unwrap();
        assert!(
            found
                .complete_values(
                    &["--config=example.toml", "private", "--format"],
                    &mut budget
                )
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(dir.0.join("plugin-queries")).unwrap(),
            previous
        );
        assert!(!dir.0.join("operation-marker").exists());
    }

    #[test]
    fn rustup_hardlink_proxies_are_identified_without_executing_discovery() {
        let dir = Dir::new("cargo-hardlink");
        let path = app(&dir, true);
        std::fs::remove_file(&path).unwrap();
        std::fs::hard_link(dir.0.join("bin/rustup"), &path).unwrap();
        assert_eq!(
            super::super::identity::identify(&path).as_deref(),
            Some("rust:cargo")
        );
        assert_eq!(
            super::super::identity::identify(&dir.0.join("bin/rustup")),
            None
        );
        assert!(!dir.0.join("queries").exists());
        let mut budget = budget();
        let found = backend(&path, false, &mut budget);
        assert!(
            found
                .complete(&[], "", &mut budget)
                .unwrap()
                .iter()
                .any(|item| item.value == "+stable")
        );
    }
}
