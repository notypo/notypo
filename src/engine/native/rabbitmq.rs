//! RabbitMQ's CLI tools (rabbitmqctl, rabbitmq-diagnostics, rabbitmq-plugins,
//! rabbitmq-queues, rabbitmq-streams, rabbitmq-upgrade) share one Elixir
//! command framework. Each tool is a `run_escript` shell script beside
//! `rabbitmq-env`, starting the escript of the same name in `../escript`;
//! Homebrew adds a one-line `exec` wrapper that puts Erlang on PATH. These
//! files identify the tool without running it.
//!
//! The framework's `autocomplete [prefix]` command (in the ctl, diagnostics,
//! plugins, and queues scopes, so not in rabbitmq-streams or rabbitmq-upgrade)
//! takes a single word: a command-name prefix, answered with the matching
//! commands, or after `--` an option prefix, answered with every global
//! switch. It never lists a command's own switches, and an empty prefix
//! answers nothing (verified with 4.3.6). Commands therefore come from
//! `help`, which prints the root usage and every command in the tool's scope
//! (core and enabled plugins) with its description; `help <command>` prints
//! that command's usage, whose bracketed options state their arity, and its
//! documented arguments and options. Usage is written by hand and can omit
//! a switch, so option lists stay partial. Commands named in an aliases
//! file are valid but never listed.
//!
//! Every invocation sources the installation's `rabbitmq-env.conf` and starts
//! an Erlang VM that loads enabled plugins' modules, so the bridge needs
//! `trusted_help` as well as `trusted_completers`. Neither command contacts a
//! node, but the VM creates `$HOME/.erlang.cookie` when it is missing, and a
//! crash writes `erl_crash.dump` to the working directory: probes run in a
//! private directory that is also their HOME. Only words the app listed as
//! commands are ever passed to `help`.

use super::{Backend, Capabilities, CompletionError, CompletionItem, Flavor, Trust};
use crate::engine::probe::{self, Budget, Capture, Probe};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Protocol {
    pub trusted_help: Vec<String>,
    /// The tool's escript name, which its help prints in usage lines.
    tool: String,
}

/// What one `help` answer documents.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Help {
    commands: Vec<CompletionItem>,
    options: Vec<CompletionItem>,
}

/// Where the words before the slot leave the command line.
#[derive(Debug, PartialEq, Eq)]
enum Position<'a> {
    /// No command yet: the slot is a command or a global option.
    Root,
    /// After `command`, with `arguments` positional words following it.
    Command { name: &'a str, arguments: usize },
    /// After `--`, the parser takes nothing more as options.
    Literal,
}

/// The RabbitMQ CLI tool `path` runs, from its installed scripts.
pub(super) fn tool(path: &Path) -> Option<String> {
    let real = std::fs::canonicalize(path).ok()?;
    let script = homebrew_target(&real).unwrap_or(real);
    let tool = script.file_name()?.to_str()?.to_owned();
    let text = super::head(&script, 16 * 1024);
    let shebang = text.lines().next()?;
    let starter = format!("run_escript \"${{ESCRIPT_DIR:?must be defined}}\"/{tool}");
    let starts = text.lines().any(|line| {
        line.trim_start()
            .strip_prefix(&starter)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
    });
    if !(shebang.starts_with("#!/bin/sh") || shebang.starts_with("#!/bin/bash"))
        || !text
            .lines()
            .any(|line| line.trim() == ". \"${0%/*}\"/rabbitmq-env")
        || !starts
    {
        return None;
    }
    let dir = script.parent()?;
    let env = super::head(&dir.join("rabbitmq-env"), 64 * 1024);
    if !env.contains("run_escript()") || !env.contains("ESCRIPT_DIR=") {
        return None;
    }
    let escript = super::head(&dir.parent()?.join("escript").join(&tool), 512);
    let mut lines = escript.lines();
    let header = lines.next()?;
    (header.starts_with("#!")
        && header.contains("escript")
        && lines
            .take(2)
            .any(|line| line.starts_with("%%!") && line.contains("-escript main Elixir.")))
    .then_some(tool)
}

/// Homebrew's wrapper: a shebang, then `VAR="..." exec "<script>" "$@"`.
fn homebrew_target(wrapper: &Path) -> Option<PathBuf> {
    let text = super::head(wrapper, 1024);
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let [shebang, exec] = lines[..] else {
        return None;
    };
    if !matches!(shebang, "#!/bin/bash" | "#!/bin/sh") {
        return None;
    }
    let pattern = regex!(
        r#"^(?:[A-Z_][A-Z0-9_]*="[^"$`\\]*(?:\$\{PATH\})?[^"$`\\]*"\s+)*exec\s+"([^"$`\\]+)"\s+"\$@"$"#
    );
    let target = PathBuf::from(pattern.captures(exec.trim())?.get(1)?.as_str());
    (target.is_absolute() && target.file_name() == wrapper.file_name()).then_some(target)
}

pub(super) fn backend(name: &str, path: &Path, tool: String) -> Backend {
    let mut backend = Backend::new(Flavor::RabbitMQ, name, path.to_owned());
    backend.identity = Some(format!("rabbitmq:{tool}"));
    backend.bridge = Some(Box::new(super::Bridge::RabbitMQ(Protocol {
        trusted_help: Vec::new(),
        tool,
    })));
    backend
}

/// An empty directory of notypo's own, used as the VM's HOME and cwd.
fn private_dir() -> Result<PathBuf, CompletionError> {
    let dir = crate::utils::cache_dir().join("rabbitmq-probes");
    std::fs::create_dir_all(&dir).map_err(|error| {
        CompletionError::Failed(format!("no private directory for RabbitMQ probes: {error}"))
    })?;
    Ok(dir)
}

fn is_command_name(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_lowercase())
        && word
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn is_option_name(word: &str) -> bool {
    let name = word
        .strip_prefix("--")
        .or_else(|| word.strip_prefix('-'))
        .unwrap_or_default();
    name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Adds `option`, keeping the first stated arity and description.
fn merge(items: &mut Vec<CompletionItem>, option: CompletionItem) {
    match items.iter_mut().find(|item| item.value == option.value) {
        Some(item) => {
            if item.takes_value.is_none() {
                item.takes_value = option.takes_value;
            }
            if item.description.is_none() {
                item.description = option.description;
            }
        }
        None => items.push(option),
    }
}

/// The usage lines that follow the `Usage` heading and name `tool`.
fn usage_lines<'a>(lines: &[&'a str], tool: &str) -> Vec<&'a str> {
    let Some(start) = lines.iter().position(|line| line.trim() == "Usage") else {
        return Vec::new();
    };
    lines[start + 1..]
        .iter()
        .skip_while(|line| line.trim().is_empty())
        .take_while(|line| !line.trim().is_empty())
        .filter(|line| line.split_whitespace().next() == Some(tool))
        .copied()
        .collect()
}

/// Options in a usage line. A placeholder (`<vhost>`, `=<true|false>`) after
/// an option or `|`-joined aliases means they take a value; anything else
/// after them (a bracket, a word, the end) means they don't.
fn usage_options(line: &str) -> Vec<CompletionItem> {
    #[derive(PartialEq)]
    enum Token<'a> {
        Word(&'a str),
        Placeholder,
        Bar,
        Boundary,
    }
    let mut tokens = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        match c {
            '<' => {
                for (_, c) in chars.by_ref() {
                    if c == '>' {
                        break;
                    }
                }
                tokens.push(Token::Placeholder);
            }
            '|' => tokens.push(Token::Bar),
            '[' | ']' | '(' | ')' => tokens.push(Token::Boundary),
            c if c.is_whitespace() || c == '"' || c == ',' || c == '=' => {}
            _ => {
                let mut end = start + c.len_utf8();
                while let Some(&(i, c)) = chars.peek() {
                    if c.is_whitespace() || "<|[]()\",=".contains(c) {
                        break;
                    }
                    end = i + c.len_utf8();
                    chars.next();
                }
                tokens.push(Token::Word(&line[start..end]));
            }
        }
    }
    let mut options = Vec::new();
    let mut group: Vec<&str> = Vec::new();
    let mut joined = false;
    let mut flush = |group: &mut Vec<&str>, takes_value: bool| {
        for name in group.drain(..) {
            merge(
                &mut options,
                CompletionItem {
                    value: name.to_owned(),
                    takes_value: Some(takes_value),
                    description: None,
                },
            );
        }
    };
    for token in tokens {
        match token {
            Token::Word(word) if word.starts_with('-') && is_option_name(word) => {
                if !joined {
                    flush(&mut group, false);
                }
                group.push(word);
                joined = false;
            }
            Token::Bar => joined = !group.is_empty(),
            Token::Placeholder => {
                flush(&mut group, true);
                joined = false;
            }
            Token::Word(_) | Token::Boundary => {
                flush(&mut group, false);
                joined = false;
            }
        }
    }
    flush(&mut group, false);
    options
}

/// The root `help`: its usage line names the tool and a `<command>`, then
/// grouped commands with descriptions, then the tool's closing hint.
fn parse_root(text: &str, tool: &str, limit: usize) -> Result<Help, CompletionError> {
    let not_help = || CompletionError::Failed(format!("{tool} help is not RabbitMQ CLI help"));
    let lines: Vec<&str> = text.lines().collect();
    let usage = usage_lines(&lines, tool);
    let footer = format!("Use '{tool} help <command>'");
    let start = lines
        .iter()
        .position(|line| line.trim() == "Available commands:")
        .ok_or_else(not_help)?;
    if usage.len() != 1
        || !usage[0].split_whitespace().any(|word| word == "<command>")
        || !lines[start..]
            .iter()
            .any(|line| line.trim_start().starts_with(&footer))
    {
        return Err(not_help());
    }
    let mut commands: Vec<CompletionItem> = Vec::new();
    for line in &lines[start + 1..] {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with(&footer) {
            break;
        }
        if !line.starts_with(char::is_whitespace) {
            // A group heading: `Help:`, `Stream plugin:`.
            if !trimmed.ends_with(':') || trimmed.contains(char::is_control) {
                return Err(not_help());
            }
            continue;
        }
        let (name, description) = trimmed
            .split_once(char::is_whitespace)
            .unwrap_or((trimmed, ""));
        if !is_command_name(name) {
            return Err(not_help());
        }
        if commands.iter().any(|item| item.value == name) {
            continue;
        }
        if commands.len() >= limit {
            return Err(super::over_limit());
        }
        commands.push(CompletionItem {
            value: name.to_owned(),
            takes_value: None,
            description: super::clean_description(description),
        });
    }
    if !commands.iter().any(|item| item.value == "help") {
        return Err(not_help());
    }
    Ok(Help {
        commands,
        options: usage_options(usage[0]),
    })
}

/// `help <command>`: options from its usage line, then those documented
/// under `Arguments and Options` (a placeholder there means a value; its
/// absence says nothing, since `--vhost` is listed bare).
fn parse_command(
    text: &str,
    tool: &str,
    command: &str,
    limit: usize,
) -> Result<Vec<CompletionItem>, CompletionError> {
    let lines: Vec<&str> = text.lines().collect();
    let usage = usage_lines(&lines, tool);
    if usage.is_empty()
        || !usage
            .iter()
            .all(|line| line.split_whitespace().any(|word| word == command))
    {
        return Err(CompletionError::Failed(format!(
            "{tool} help {command} has no usage line for it"
        )));
    }
    let mut options = Vec::new();
    for line in &usage {
        for option in usage_options(line) {
            merge(&mut options, option);
        }
    }
    if let Some(start) = lines
        .iter()
        .position(|line| line.trim() == "Arguments and Options")
    {
        let mut entries = lines[start + 1..].iter().peekable();
        while let Some(line) = entries.next() {
            if line.trim().is_empty() || line.starts_with(char::is_whitespace) {
                continue;
            }
            if !line.starts_with(['-', '<']) {
                break;
            }
            let mut words = line.split_whitespace();
            let Some(name) = words.next().filter(|name| is_option_name(name)) else {
                continue;
            };
            let description = entries
                .peek()
                .filter(|next| next.starts_with('\t'))
                .and_then(|next| super::clean_description(next));
            merge(
                &mut options,
                CompletionItem {
                    value: name.to_owned(),
                    takes_value: words
                        .next()
                        .is_some_and(|word| word.starts_with('<'))
                        .then_some(true),
                    description,
                },
            );
        }
    }
    if options.len() > limit {
        return Err(super::over_limit());
    }
    Ok(options)
}

/// `autocomplete -- --`: every global switch, one per line.
fn parse_switches(text: &str, limit: usize) -> Result<Vec<CompletionItem>, CompletionError> {
    let mut items: Vec<CompletionItem> = Vec::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        if !line.starts_with("--") || !is_option_name(line) {
            return Err(CompletionError::Failed(
                "RabbitMQ autocomplete returned a line that is not an option".into(),
            ));
        }
        if items.len() >= limit {
            return Err(super::over_limit());
        }
        merge(
            &mut items,
            CompletionItem {
                value: line.to_owned(),
                takes_value: None,
                description: None,
            },
        );
    }
    if items.is_empty() {
        return Err(CompletionError::Failed(
            "RabbitMQ autocomplete listed no global options".into(),
        ));
    }
    Ok(items)
}

/// An aliases file makes unlisted command names valid.
fn aliases(words: &[&str]) -> bool {
    std::env::var_os("RABBITMQ_CLI_ALIASES_FILE").is_some_and(|file| !file.is_empty())
        || words
            .iter()
            .any(|word| *word == "--aliases-file" || word.starts_with("--aliases-file="))
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
            // Short aliases are documented in prose tables only.
            short_options: false,
            complete_options: false,
            complete_subcommands: true,
            descriptions: true,
            query_dialect: None,
            trust,
        }
    }

    pub fn capabilities_for(&self, words: &[&str], trust: Trust) -> Capabilities {
        let mut capabilities = self.capabilities(trust);
        capabilities.complete_subcommands = !aliases(words);
        capabilities
    }

    /// Runs the tool with `args` in the private directory; stdout only.
    fn run(
        &self,
        backend: &Backend,
        args: &[&str],
        budget: &mut Budget,
    ) -> Result<String, CompletionError> {
        if !self.allowed(backend) {
            return Err(CompletionError::Failed(format!(
                "RabbitMQ help starts the Erlang VM and loads plugins; add {} to trusted_help to allow it",
                backend.name
            )));
        }
        let key: Vec<&str> = std::iter::once("rabbitmq")
            .chain(args.iter().copied())
            .collect();
        backend.memoized(&key, || {
            let dir = private_dir()?;
            let mut env = super::offline_env(Flavor::RabbitMQ);
            env.push((OsString::from("HOME"), Some(dir.clone().into_os_string())));
            let output = probe::run_in(
                &Probe {
                    program: &backend.completer,
                    args: args.iter().map(OsString::from).collect(),
                    env,
                    capture: Capture::Stdout,
                },
                &dir,
                budget,
            )
            .map_err(super::probe_error)?;
            if output.status != Some(0) {
                return Err(CompletionError::Failed(format!(
                    "{} {} exited with {:?}",
                    self.tool,
                    args.join(" "),
                    output.status
                )));
            }
            if output.truncated {
                return Err(CompletionError::Failed(
                    "RabbitMQ help output exceeded the probe limit".into(),
                ));
            }
            String::from_utf8(output.data).map_err(|_| {
                CompletionError::Failed("RabbitMQ help output is not valid UTF-8".into())
            })
        })
    }

    fn root(&self, backend: &Backend, budget: &mut Budget) -> Result<Help, CompletionError> {
        let limit = budget.max_candidates;
        parse_root(&self.run(backend, &["help"], budget)?, &self.tool, limit)
    }

    /// Global switches: the native answer when the tool has `autocomplete`,
    /// with arity from the root usage line.
    fn global_options(
        &self,
        backend: &Backend,
        root: &Help,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let mut options = root.options.clone();
        if root
            .commands
            .iter()
            .any(|item| item.value == "autocomplete")
        {
            let limit = budget.max_candidates;
            let text = self.run(backend, &["autocomplete", "--", "--"], budget)?;
            for option in parse_switches(&text, limit)? {
                merge(&mut options, option);
            }
        }
        Ok(options)
    }

    fn command_options(
        &self,
        backend: &Backend,
        command: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let limit = budget.max_candidates;
        let text = self.run(backend, &["help", command], budget)?;
        parse_command(&text, &self.tool, command, limit)
    }

    /// Finds the command, skipping global options and their values. An option
    /// of unknown arity takes the next word unless the app lists it as a
    /// command (`--silent list_queues`, `--vhost / list_queues`).
    fn position<'a>(root: &Help, words: &[&'a str]) -> Position<'a> {
        let listed = |word: &str| root.commands.iter().any(|item| item.value == word);
        let mut i = 0;
        while let Some(&word) = words.get(i) {
            i += 1;
            if word == "--" {
                return Position::Literal;
            }
            if word.len() > 1 && word.starts_with('-') {
                if word.contains('=') {
                    continue;
                }
                let next = words.get(i).copied();
                let takes_value = root
                    .options
                    .iter()
                    .find(|option| option.value == word)
                    .and_then(|option| option.takes_value)
                    .unwrap_or_else(|| {
                        next.is_some_and(|next| !next.starts_with('-') && !listed(next))
                    });
                if takes_value {
                    i += 1;
                }
                continue;
            }
            return Position::Command {
                name: word,
                arguments: words[i..]
                    .iter()
                    .filter(|word| !word.starts_with('-'))
                    .count(),
            };
        }
        Position::Root
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
                "the command line contains control characters".into(),
            ));
        }
        let root = self.root(backend, budget)?;
        let option = prefix.starts_with('-');
        let mut items = match Self::position(&root, words) {
            Position::Literal => {
                return Err(CompletionError::Unsupported(
                    "RabbitMQ takes words after `--` as arguments".into(),
                ));
            }
            Position::Root if option => self.global_options(backend, &root, budget)?,
            Position::Root => root.commands,
            Position::Command { name, .. } if option => {
                if !root.commands.iter().any(|item| item.value == name) {
                    return Err(CompletionError::Unsupported(format!(
                        "{name} is not a listed {} command",
                        self.tool
                    )));
                }
                let mut options = self.global_options(backend, &root, budget)?;
                for option in self.command_options(backend, name, budget)? {
                    merge(&mut options, option);
                }
                options
            }
            // `help <command>` names a command; other arguments are values.
            Position::Command {
                name: "help",
                arguments: 0,
            } => root.commands,
            Position::Command { .. } => {
                return Err(CompletionError::Unsupported(
                    "RabbitMQ command arguments are not completed".into(),
                ));
            }
        };
        items.retain(|item| item.value.starts_with(prefix));
        Ok(items)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::{
        Discovery, NativeCompletionBackend, discover, discover_for_shell_with_budget, tests::Dir,
    };
    use super::*;
    use crate::shells::Shell;
    use std::time::Duration;

    const ENV: &str = r#"#!/bin/sh -e
RABBITMQ_HOME="${0%/*}/.."
ESCRIPT_DIR="${RABBITMQ_HOME}/escript"
run_escript()
{
    escript="${1:?escript must be defined}"
    shift
    shift
    sh "$escript.body" "$@"
}
"#;

    const ESCRIPT: &str = "#!/usr/bin/env escript\n%% This is an -*- erlang -*- file\n%%! -escript main Elixir.RabbitMQCtl -hidden\nPK\n";

    /// Help and autocomplete answers modeled on 4.3.6. Anything else is an
    /// operation and leaves a marker.
    const BODY: &str = r#"root=$(cd "$(dirname "$0")/.." && pwd)
[ "$HTTPS_PROXY" = http://127.0.0.1:9 ] || exit 8
printf '<%s>' "$@" >> "$root/queries"
printf '|%s|%s\n' "$HOME" "$PWD" >> "$root/queries"
if [ -f "$root/mode" ]; then
  case "$(cat "$root/mode")" in
    other) printf 'Usage: something else\n'; exit 0;;
    failed) exit 64;;
  esac
fi
case "$*" in
  help)
    printf '\nUsage\n\nrabbitmqctl [--node <node>] [--timeout <timeout>] [--longnames] [--quiet] <command> [<command options>]\n\nAvailable commands:\n\nHelp:\n\n   autocomplete     Provides command name autocomplete variants\n   help             Displays usage information for a command\n\nQueues:\n\n   delete_queue     Deletes a queue\n   list_queues      Lists queues and their properties\n   trace_on         \n\nStream plugin:\n\n   list_stream_connections   Lists stream connections\n\nUse '"'"'rabbitmqctl help <command>'"'"' to learn more about a specific command\n';;
  'autocomplete -- --') printf -- '--node\n--quiet\n--silent\n--vhost\n--formatter\n--longnames\n--timeout\n';;
  'help delete_queue')
    printf '\nUsage\n\nrabbitmqctl [--node <node>] [--longnames] [--quiet] delete_queue [--vhost <vhost>] <queue_name> [--if-empty|-e] [--if-unused|-u] [--duration|-d <seconds>] [--format=<json | erlang>] [--all | --offset] [--timeout <timeout>]\n\nDeletes a queue.\n\n\nArguments and Options\n\n<queue_name>\n\tname of the queue to delete\n\n--if-empty\n\tdelete the queue if it is empty\n\n--force\n\tdelete the queue even if it is protected\n\n--max-age <max-age>\n\tretention\n\n\nRelevant Doc Guides\n\n * https://rabbitmq.com/docs/queues/\n';;
  'help list_queues')
    printf '\nUsage\n\nrabbitmqctl [--node <node>] [--longnames] [--quiet] list_queues [--vhost <vhost>] [--online] [<column>, ...]\n\nLists queues.\n';;
  *) touch "$root/operation-marker"; exit 0;;
esac
"#;

    /// The real layout: Homebrew's wrapper, the `run_escript` script, its
    /// environment, and the escript.
    fn install(dir: &Dir) -> PathBuf {
        dir.script(
            "libexec/rabbitmqctl",
            "#!/bin/sh\nset -e\n. \"${0%/*}\"/rabbitmq-env\nrun_escript \"${ESCRIPT_DIR:?must be defined}\"/rabbitmqctl 'noinput' \"$@\"\n",
        );
        dir.script("libexec/rabbitmq-env", ENV);
        dir.script("escript/rabbitmqctl", ESCRIPT);
        dir.script("escript/rabbitmqctl.body", BODY);
        dir.script(
            "sbin/rabbitmqctl",
            &format!(
                "#!/bin/bash\nPATH=\"/nonexistent/erlang/bin:${{PATH}}\" exec \"{}\"  \"$@\"\n",
                dir.0.join("libexec/rabbitmqctl").display()
            ),
        )
    }

    fn backend(dir: &Dir, help: &[&str]) -> Backend {
        let path = install(dir);
        let mut backend = super::backend("rabbitmqctl", &path, "rabbitmqctl".into());
        if let Some(super::super::Bridge::RabbitMQ(protocol)) = backend.bridge.as_deref_mut() {
            protocol.trusted_help = help.iter().map(|h| h.to_string()).collect();
        }
        backend
    }

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(20), Duration::from_secs(10), 16)
    }

    fn values(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.value.as_str()).collect()
    }

    fn queries(dir: &Dir) -> String {
        std::fs::read_to_string(dir.0.join("queries")).unwrap_or_default()
    }

    #[test]
    fn identifies_installed_scripts_without_running_them() {
        let dir = Dir::new("rabbitmq-identity");
        let wrapper = install(&dir);
        let direct = dir.0.join("libexec/rabbitmqctl");
        assert_eq!(tool(&wrapper).as_deref(), Some("rabbitmqctl"));
        assert_eq!(tool(&direct).as_deref(), Some("rabbitmqctl"));
        assert_eq!(
            super::super::identity::identify(&wrapper).as_deref(),
            Some("rabbitmq:rabbitmqctl")
        );
        // A renamed wrapper, extra commands, or a missing escript are not it.
        let renamed = dir.script(
            "sbin/rmq",
            &format!("#!/bin/bash\nexec \"{}\" \"$@\"\n", direct.display()),
        );
        assert_eq!(tool(&renamed), None);
        let chained = dir.script(
            "other/rabbitmqctl",
            &format!(
                "#!/bin/bash\ntouch marker\nexec \"{}\" \"$@\"\n",
                direct.display()
            ),
        );
        assert_eq!(tool(&chained), None);
        // The server script beside the tools starts a broker; asked for
        // help, it would run one. It has no escript of its own.
        let server = dir.script(
            "libexec/rabbitmq-server",
            "#!/bin/sh\nset -e\n. \"${0%/*}\"/rabbitmq-env\nexec erl -boot start_sasl \"$@\"\n",
        );
        assert_eq!(tool(&server), None);
        dir.script("escript/rabbitmq-server", ESCRIPT);
        assert_eq!(tool(&server), None);
        std::fs::remove_file(dir.0.join("escript/rabbitmqctl")).unwrap();
        assert_eq!(tool(&wrapper), None);
        assert!(queries(&dir).is_empty());
        assert!(!dir.0.join("operation-marker").exists());
    }

    #[test]
    fn discovery_needs_both_trust_settings() {
        let dir = Dir::new("rabbitmq-discovery");
        let path = install(&dir);
        assert!(matches!(
            discover("rabbitmqctl", &path, &[]),
            Discovery::NotTrusted(why) if why.contains("trusted_completers")
        ));
        let mut budget = budget();
        let trusted = ["rabbitmq:rabbitmqctl".to_owned()];
        assert!(matches!(
            discover_for_shell_with_budget("rabbitmqctl", &path, &trusted, &[], Shell::Bash, &mut budget),
            Discovery::NotTrusted(why) if why.contains("trusted_help")
        ));
        match discover_for_shell_with_budget(
            "rabbitmqctl",
            &path,
            &trusted,
            &["rabbitmqctl".into()],
            Shell::Bash,
            &mut budget,
        ) {
            Discovery::Found(backend) => {
                assert_eq!(backend.flavor, Flavor::RabbitMQ);
                assert_eq!(backend.id(), "rabbitmq");
                assert_eq!(backend.cache_identity(), None);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(budget.spawned(), 0, "discovery reads files only");
        assert!(queries(&dir).is_empty());
    }

    #[test]
    fn commands_options_and_help_arguments_come_from_the_app_in_a_private_home() {
        let dir = Dir::new("rabbitmq-levels");
        let backend = backend(&dir, &["rabbitmqctl"]);
        let mut budget = budget();
        let commands = backend.complete(&[], "", &mut budget).unwrap();
        assert_eq!(
            values(&commands),
            [
                "autocomplete",
                "help",
                "delete_queue",
                "list_queues",
                "trace_on",
                "list_stream_connections"
            ]
        );
        assert_eq!(commands[2].description.as_deref(), Some("Deletes a queue"));
        assert_eq!(commands[4].description, None);
        // Global options skip their values; listed commands end flags of
        // unknown arity.
        for words in [
            &["--node", "rabbit@host"][..],
            &["--silent"],
            &["--vhost", "/"],
            &["-p", "/"],
            &["--quiet"],
        ] {
            assert_eq!(
                values(&backend.complete(words, "", &mut budget).unwrap()).len(),
                6,
                "{words:?}"
            );
        }
        assert_eq!(
            values(&backend.complete(&["help"], "list_", &mut budget).unwrap()),
            ["list_queues", "list_stream_connections"]
        );
        let globals = backend.complete(&[], "--", &mut budget).unwrap();
        assert_eq!(
            values(&globals),
            [
                "--node",
                "--timeout",
                "--longnames",
                "--quiet",
                "--silent",
                "--vhost",
                "--formatter"
            ]
        );
        assert_eq!(globals[0].takes_value, Some(true));
        assert_eq!(globals[3].takes_value, Some(false));
        assert_eq!(globals[4].takes_value, None);
        let options = backend
            .complete(
                &["--quiet", "delete_queue", "--vhost", "/"],
                "--",
                &mut budget,
            )
            .unwrap();
        let arity = |name: &str| {
            options
                .iter()
                .find(|item| item.value == name)
                .unwrap_or_else(|| panic!("{name} in {options:?}"))
                .takes_value
        };
        assert_eq!(arity("--vhost"), Some(true));
        assert_eq!(arity("--if-empty"), Some(false));
        assert_eq!(arity("--duration"), Some(true));
        assert_eq!(arity("--format"), Some(true));
        assert_eq!(arity("--all"), Some(false));
        assert_eq!(arity("--offset"), Some(false));
        assert_eq!(arity("--max-age"), Some(true));
        assert_eq!(arity("--force"), None);
        assert_eq!(arity("--formatter"), None);
        assert!(!options.iter().any(|item| item.value == "-e"));
        assert_eq!(
            options
                .iter()
                .find(|item| item.value == "--force")
                .unwrap()
                .description
                .as_deref(),
            Some("delete the queue even if it is protected")
        );
        for words in [
            &["delete_queue"][..],
            &["delete_queue", "--vhost", "/", "orders"],
            &["help", "list_queues"],
            &["--", "list_queues"],
        ] {
            assert!(
                matches!(
                    backend.complete(words, "", &mut budget),
                    Err(CompletionError::Unsupported(_))
                ),
                "{words:?}"
            );
        }
        // An unlisted command (an alias) is never passed to help.
        assert!(matches!(
            backend.complete(&["lsit_queues"], "--", &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
        let queries = queries(&dir);
        let private =
            std::fs::canonicalize(crate::utils::cache_dir().join("rabbitmq-probes")).unwrap();
        for line in queries.lines() {
            let (args, places) = line.split_once('|').unwrap();
            assert!(
                ["<help>", "<autocomplete><--><-->", "<help><delete_queue>"].contains(&args),
                "{queries}"
            );
            let (home, cwd) = places.split_once('|').unwrap();
            assert_eq!(std::fs::canonicalize(home).unwrap(), private);
            assert_eq!(std::fs::canonicalize(cwd).unwrap(), private);
        }
        assert_eq!(
            queries.lines().count(),
            3,
            "answers are memoized: {queries}"
        );
        assert!(!queries.contains("lsit_queues") && !queries.contains("orders"));
        assert!(!dir.0.join("operation-marker").exists());
    }

    #[test]
    fn untrusted_help_foreign_text_and_failures_answer_nothing() {
        let dir = Dir::new("rabbitmq-gates");
        let untrusted = backend(&dir, &[]);
        let mut budget = budget();
        assert!(matches!(
            untrusted.complete(&[], "", &mut budget),
            Err(CompletionError::Failed(why)) if why.contains("trusted_help")
        ));
        assert!(queries(&dir).is_empty());
        assert!(
            backend(&dir, &["rabbitmq:rabbitmqctl"])
                .complete(&[], "", &mut budget)
                .is_ok()
        );
        for (mode, why) in [("other", "not RabbitMQ CLI help"), ("failed", "exited")] {
            let dir = Dir::new("rabbitmq-failures");
            let backend = backend(&dir, &["rabbitmqctl"]);
            std::fs::write(dir.0.join("mode"), mode).unwrap();
            let result = backend.complete(&[], "", &mut budget);
            assert!(
                matches!(&result, Err(CompletionError::Failed(message)) if message.contains(why)),
                "{mode}: {result:?}"
            );
        }
    }

    #[test]
    fn help_text_is_validated_line_by_line() {
        let root = |body: &str| {
            parse_root(
                &format!(
                    "\nUsage\n\nrabbitmqctl [--quiet] <command>\n\nAvailable commands:\n\nHelp:\n\n   help   Help\n{body}\nUse 'rabbitmqctl help <command>' to learn more\n"
                ),
                "rabbitmqctl",
                10,
            )
        };
        assert!(root("").is_ok());
        for body in [
            "   list;queues   bad\n",
            "   List_Queues   bad\n",
            "Not a heading\n",
            "   a b\n   c d\n   e f\n   g h\n   i j\n   k l\n   m n\n   o p\n   q r\n   s t\n",
        ] {
            assert!(root(body).is_err(), "{body:?}");
        }
        assert!(parse_root("Available commands:\n   help\n", "rabbitmqctl", 10).is_err());
        assert!(parse_switches("--node\n--quiet\n", 10).is_ok());
        for text in ["", "--node\nlist_queues\n", "--no de\n", "--x\x1b\n"] {
            assert!(parse_switches(text, 10).is_err(), "{text:?}");
        }
        // help for another command does not describe this one.
        assert!(
            parse_command(
                "\nUsage\n\nrabbitmqctl list_queues [--online]\n",
                "rabbitmqctl",
                "delete_queue",
                10
            )
            .is_err()
        );
        let options = usage_options(
            "rabbitmqctl [-n <node> --global] add_vhost <vhost> [--description <description> --tags \"<tag1>,<tag2>\" --default-queue-type <quorum|classic|stream>] [--address-family <ipv4 | ipv6>] [--protected-from-deletion=<true|false>] --stream <stream> --all",
        );
        let arity: Vec<_> = options
            .iter()
            .map(|item| (item.value.as_str(), item.takes_value))
            .collect();
        assert_eq!(
            arity,
            [
                ("-n", Some(true)),
                ("--global", Some(false)),
                ("--description", Some(true)),
                ("--tags", Some(true)),
                ("--default-queue-type", Some(true)),
                ("--address-family", Some(true)),
                ("--protected-from-deletion", Some(true)),
                ("--stream", Some(true)),
                ("--all", Some(false)),
            ]
        );
    }
}
