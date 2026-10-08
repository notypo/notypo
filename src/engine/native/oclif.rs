//! oclif apps (heroku, sf, eas, shopify) describe their commands in
//! `oclif.manifest.json`, which oclif generates when a package is published
//! and reads instead of loading command files, but only when its version
//! matches the package's. Each plugin the app loads has its own: core
//! plugins from `oclif.plugins` (resolved like Node modules) and the user's
//! plugins listed in the app's data directory. Reading them runs nothing.
//!
//! The manifests give command ids, aliases (hidden ones included), flags
//! with their short forms, aliases, `--no-` forms, arity, and choices, and
//! positional arguments with choices. oclif matches names exactly. With the
//! default `:` separator the first word is the whole id (`build:list`);
//! with `topicSeparator: " "` words join while they name a topic or command.
//! When every plugin has a manifest, command and option lists are complete
//! for strict commands. A plugin without one loads its commands from files
//! at runtime, so its commands are unknown here and lists become partial.

use super::{Capabilities, CompletionError, CompletionItem, Trust};
use crate::engine::cache::fingerprint;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_FILE: u64 = 32 * 1024 * 1024;
const MAX_PLUGINS: usize = 128;
/// oclif's id for the one command of a single-command CLI.
const SINGLE: &str = "Symbol(SINGLE_COMMAND_CLI)";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Flag {
    /// `--name`, `-c`, aliases, char aliases, and `--no-name` forms.
    spellings: Vec<String>,
    takes_value: bool,
    choices: Vec<String>,
    description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Command {
    description: Option<String>,
    /// Accepts arguments and flags it doesn't declare.
    strict: bool,
    /// From a just-in-time plugin, which oclif installs when it first runs.
    jit: bool,
    flags: Vec<Flag>,
    /// Each declared positional argument's choices, in order.
    args: Vec<Vec<String>>,
}

impl Command {
    fn flag(&self, word: &str) -> Option<&Flag> {
        let name = attached_name(word);
        self.flags
            .iter()
            .find(|flag| flag.spellings.iter().any(|s| s == name))
    }
}

/// The flag a word names: `--name=value` and `-cvalue` carry their value.
fn attached_name(word: &str) -> &str {
    if word.starts_with("--") {
        word.split_once('=').map_or(word, |(name, _)| name)
    } else if word.len() > 2 && word.starts_with('-') && word.is_char_boundary(2) {
        &word[..2]
    } else {
        word
    }
}

fn value_attached(word: &str) -> bool {
    if word.starts_with("--") {
        word.contains('=')
    } else {
        word.starts_with('-') && word.chars().count() > 2
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Protocol {
    space: bool,
    /// Command ids and aliases, each naming an entry of `commands`.
    ids: BTreeMap<String, usize>,
    commands: Vec<Command>,
    /// Topic ids (every id prefix, and declared topics) and descriptions.
    topics: BTreeMap<String, Option<String>>,
    help: Vec<String>,
    version: Vec<String>,
    single: bool,
    /// Every plugin's commands came from a manifest, and ids aren't
    /// permuted (flexible taxonomy).
    complete: bool,
    files: Vec<PathBuf>,
    fingerprint: String,
}

/// Where the next word goes.
enum Walk<'a> {
    /// The words so far name this topic or command id ("" is the root).
    Level(String),
    /// Inside a command's arguments, after this many positionals.
    Arguments(&'a Command, usize),
}

fn read_json(path: &Path) -> Option<Value> {
    let file = std::fs::File::open(path).ok()?;
    let mut data = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut data).ok()?;
    if data.len() as u64 > MAX_FILE {
        return None;
    }
    serde_json::from_slice(&data).ok()
}

fn text(value: Option<&Value>) -> Option<String> {
    let first = value?.as_str()?.lines().next()?;
    super::clean_description(first)
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|s| !s.is_empty() && !s.contains(char::is_control))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn flag(name: &str, value: &Value) -> Option<Flag> {
    if name.is_empty() || name.contains(char::is_whitespace) {
        return None;
    }
    let takes_value = value.get("type")?.as_str()? == "option";
    let mut spellings = vec![format!("--{name}")];
    let aliases = strings(value.get("aliases"));
    if let Some(char) = value.get("char").and_then(Value::as_str) {
        spellings.push(format!("-{char}"));
    }
    for alias in strings(value.get("charAliases")) {
        spellings.push(format!("-{alias}"));
    }
    for alias in &aliases {
        spellings.push(format!("--{alias}"));
    }
    if !takes_value && value.get("allowNo").and_then(Value::as_bool) == Some(true) {
        spellings.push(format!("--no-{name}"));
        spellings.extend(aliases.iter().map(|alias| format!("--no-{alias}")));
    }
    Some(Flag {
        spellings,
        takes_value,
        choices: strings(value.get("options")),
        description: text(value.get("summary")).or_else(|| text(value.get("description"))),
    })
}

fn command(value: &Value) -> Option<Command> {
    let flags = value
        .get("flags")
        .and_then(Value::as_object)
        .map(|flags| flags.iter().filter_map(|(n, v)| flag(n, v)).collect())
        .unwrap_or_default();
    let args = value
        .get("args")
        .and_then(Value::as_object)
        .map(|args| args.values().map(|a| strings(a.get("options"))).collect())
        .unwrap_or_default();
    Some(Command {
        description: text(value.get("summary")).or_else(|| text(value.get("description"))),
        strict: value.get("strict").and_then(Value::as_bool) != Some(false),
        jit: value.get("pluginType").and_then(Value::as_str) == Some("jit"),
        flags,
        args,
    })
}

/// A package's manifest when it describes this version of the package.
fn manifest(root: &Path, version: &str, files: &mut Vec<PathBuf>) -> Option<Value> {
    let release = |v: &str| v.split('-').next().unwrap_or(v).to_owned();
    for name in ["oclif.manifest.json", ".oclif.manifest.json"] {
        let path = root.join(name);
        files.push(path.clone());
        if !path.is_file() {
            continue;
        }
        let manifest = read_json(&path)?;
        let matches = manifest
            .get("version")
            .and_then(Value::as_str)
            .is_some_and(|v| release(v) == release(version));
        return matches.then_some(manifest);
    }
    None
}

/// `name`'s package directory as Node resolves it from `from`.
fn resolve(from: &Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains(['*', '\\']) || name.split('/').any(|part| part == "..") {
        return None;
    }
    from.ancestors()
        .take(32)
        .map(|dir| dir.join("node_modules").join(name))
        .find(|dir| dir.join("package.json").is_file())
}

struct Loader {
    protocol: Protocol,
    plugins: usize,
}

impl Loader {
    /// Adds a plugin's commands, then its own plugins.
    fn plugin(&mut self, root: &Path, depth: usize) {
        self.plugins += 1;
        let package = root.join("package.json");
        self.protocol.files.push(package.clone());
        let Some(pjson) = read_json(&package) else {
            self.protocol.complete = false;
            return;
        };
        let version = pjson.get("version").and_then(Value::as_str).unwrap_or("");
        match manifest(root, version, &mut self.protocol.files) {
            Some(manifest) => self.commands(&manifest),
            None => self.protocol.complete = false,
        }
        let oclif = pjson.get("oclif");
        if let Some(topics) = oclif
            .and_then(|o| o.get("topics"))
            .and_then(Value::as_object)
        {
            for (id, topic) in topics {
                self.protocol
                    .topics
                    .entry(id.clone())
                    .or_insert_with(|| text(topic.get("description")));
            }
        }
        let nested = oclif.and_then(|o| o.get("plugins"));
        if nested.is_some_and(|plugins| !plugins.is_array()) {
            self.protocol.complete = false;
        }
        for name in strings(nested) {
            self.child(root, &name, depth);
        }
    }

    fn child(&mut self, from: &Path, name: &str, depth: usize) {
        if depth >= 8 || self.plugins >= MAX_PLUGINS {
            self.protocol.complete = false;
            return;
        }
        match resolve(from, name) {
            Some(root) => self.plugin(&root, depth + 1),
            None => self.protocol.complete = false,
        }
    }

    fn commands(&mut self, manifest: &Value) {
        let Some(commands) = manifest.get("commands").and_then(Value::as_object) else {
            self.protocol.complete = false;
            return;
        };
        for (id, value) in commands {
            if id.is_empty() || id.contains(char::is_whitespace) {
                continue;
            }
            let Some(command) = command(value) else {
                self.protocol.complete = false;
                continue;
            };
            let index = self.protocol.commands.len();
            self.protocol.commands.push(command);
            let aliases = ["aliases", "hiddenAliases"]
                .into_iter()
                .flat_map(|field| strings(value.get(field)));
            for name in std::iter::once(id.clone()).chain(aliases) {
                if name.contains(char::is_whitespace) {
                    continue;
                }
                // Every prefix of an id is a topic.
                let mut prefix = String::new();
                for part in name
                    .split(':')
                    .collect::<Vec<_>>()
                    .split_last()
                    .map_or(&[][..], |(_, p)| p)
                {
                    if !prefix.is_empty() {
                        prefix.push(':');
                    }
                    prefix.push_str(part);
                    self.protocol.topics.entry(prefix.clone()).or_insert(None);
                }
                self.protocol.ids.entry(name).or_insert(index);
            }
        }
    }
}

/// oclif's data directory for the app: `<BIN>_DATA_DIR`, else
/// `$XDG_DATA_HOME/<dirname>`, `%LOCALAPPDATA%\<dirname>` on Windows, or
/// `~/.local/share/<dirname>`.
fn data_dir(pjson: &Value, bin: &str) -> Option<PathBuf> {
    let oclif = pjson.get("oclif")?;
    let scoped = |name: &str| {
        format!(
            "{}_DATA_DIR",
            name.replace('@', "").replace(['/', '-'], "_")
        )
        .to_uppercase()
    };
    let mut bins = vec![bin.to_owned()];
    bins.extend(strings(oclif.get("binAliases")));
    if let Some(dir) = bins
        .iter()
        .find_map(|name| std::env::var_os(scoped(name)).filter(|v| !v.is_empty()))
    {
        return Some(PathBuf::from(dir));
    }
    let dirname = oclif
        .get("dirname")
        .and_then(Value::as_str)
        .or_else(|| pjson.get("name").and_then(Value::as_str))?;
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            cfg!(windows)
                .then(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
                .flatten()
        })
        .unwrap_or_else(|| crate::utils::expand_user("~/.local/share"));
    Some(base.join(dirname))
}

/// The oclif app `path` runs, read from its package and manifests. The
/// executable must be one the package declares as a bin.
pub(super) fn discover(path: &Path) -> Option<Protocol> {
    discover_in(path, None)
}

/// [`discover`] with the user's plugins read from `data`, when given.
fn discover_in(path: &Path, data: Option<&Path>) -> Option<Protocol> {
    let real = std::fs::canonicalize(path).ok()?;
    let (root, pjson) = super::identity::npm_manifest(&real)?;
    let oclif = pjson.get("oclif")?.as_object()?;
    let declared = match pjson.get("bin")? {
        Value::String(bin) => vec![bin.as_str()],
        Value::Object(bins) => bins.values().filter_map(Value::as_str).collect(),
        _ => return None,
    };
    if !declared
        .iter()
        .any(|bin| std::fs::canonicalize(root.join(bin)).ok().as_ref() == Some(&real))
    {
        return None;
    }
    let flags = |field: &str, base: &str| {
        let mut flags = vec![base.to_owned()];
        flags.extend(strings(oclif.get(field)));
        flags
    };
    let mut loader = Loader {
        protocol: Protocol {
            space: oclif.get("topicSeparator").and_then(Value::as_str) == Some(" "),
            ids: BTreeMap::new(),
            commands: Vec::new(),
            topics: BTreeMap::new(),
            help: flags("additionalHelpFlags", "--help"),
            version: flags("additionalVersionFlags", "--version"),
            single: oclif
                .get("commands")
                .and_then(|c| c.get("strategy"))
                .and_then(Value::as_str)
                == Some("single"),
            complete: oclif.get("flexibleTaxonomy").and_then(Value::as_bool) != Some(true),
            files: Vec::new(),
            fingerprint: String::new(),
        },
        plugins: 0,
    };
    // Without the root manifest, nothing is known about the app.
    let version = pjson.get("version").and_then(Value::as_str).unwrap_or("");
    manifest(&root, version, &mut Vec::new())?;
    loader.plugin(&root, 0);
    let bin = oclif
        .get("bin")
        .and_then(Value::as_str)
        .or_else(|| pjson.get("name").and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned();
    if let Some(dir) = data.map(Path::to_owned).or_else(|| data_dir(&pjson, &bin)) {
        let user = dir.join("package.json");
        loader.protocol.files.push(user.clone());
        if user.exists() {
            match read_json(&user) {
                Some(user) => {
                    let plugins = user
                        .get("oclif")
                        .and_then(|o| o.get("plugins"))
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    for plugin in plugins {
                        let name = plugin.get("name").and_then(Value::as_str).unwrap_or("");
                        match plugin.get("type").and_then(Value::as_str) {
                            Some("user") => loader.child(&dir, name, 0),
                            Some("link") => match plugin.get("root").and_then(Value::as_str) {
                                Some(root) if loader.plugins < MAX_PLUGINS => {
                                    loader.plugin(Path::new(root), 1);
                                }
                                _ => loader.protocol.complete = false,
                            },
                            _ => {}
                        }
                    }
                }
                None => loader.protocol.complete = false,
            }
        }
    }
    let mut protocol = loader.protocol;
    if protocol.ids.is_empty() {
        return None;
    }
    protocol.fingerprint = fingerprint(&protocol.files, &["oclif"]);
    Some(protocol)
}

impl Protocol {
    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            subcommands: true,
            options: true,
            option_arity: true,
            values: true,
            resources: false,
            option_prefix: "-",
            short_options: true,
            complete_options: self.complete,
            complete_subcommands: self.complete,
            descriptions: true,
            query_dialect: None,
            trust: Trust::Bridge,
        }
    }

    pub fn capabilities_for(&self, words: &[&str]) -> Capabilities {
        let mut capabilities = self.capabilities();
        match self.walk(words) {
            Ok(Walk::Level(prefix)) => {
                // A command that takes arguments can also take a word here.
                if let Some(command) = self.command(&prefix) {
                    capabilities.complete_subcommands &= command.strict && command.args.is_empty();
                    capabilities.complete_options &= command.strict;
                }
            }
            Ok(Walk::Arguments(command, _)) => {
                capabilities.complete_subcommands = false;
                capabilities.complete_options &= command.strict;
            }
            Err(_) => {
                capabilities.complete_subcommands = false;
                capabilities.complete_options = false;
            }
        }
        capabilities
    }

    fn command(&self, id: &str) -> Option<&Command> {
        let id = if self.single && id.is_empty() {
            SINGLE
        } else {
            id
        };
        self.ids.get(id).map(|&i| &self.commands[i])
    }

    fn check(&self) -> Result<(), CompletionError> {
        if fingerprint(&self.files, &["oclif"]) != self.fingerprint {
            return Err(CompletionError::Failed(
                "the app's oclif manifests changed after they were read".into(),
            ));
        }
        Ok(())
    }

    fn walk<'a>(&'a self, words: &[&str]) -> Result<Walk<'a>, CompletionError> {
        let unsupported = |why: &str| Err(CompletionError::Unsupported(why.into()));
        if self.single {
            let command = self.command("").ok_or_else(|| {
                CompletionError::Unsupported("the single command is not described".into())
            })?;
            return self.arguments(command, words);
        }
        let Some((first, rest)) = words.split_first() else {
            return Ok(Walk::Level(String::new()));
        };
        if first.starts_with('-') {
            return unsupported("oclif reads flags before the command as help or version");
        }
        if !self.space || first.contains(':') {
            // The first word is the whole id.
            return match self.command(first) {
                Some(command) => self.arguments(command, rest),
                None => unsupported("this word names a topic or no command"),
            };
        }
        let mut prefix = String::new();
        for (i, word) in words.iter().enumerate() {
            let command = self.command(&prefix).filter(|_| !prefix.is_empty());
            let takes_arguments = command.is_some_and(|c| !c.strict || !c.args.is_empty());
            if word.starts_with('-') || word.contains('=') || takes_arguments {
                return match command {
                    Some(command) => self.arguments(command, &words[i..]),
                    None => unsupported("these words name a topic, not a command"),
                };
            }
            let next = if prefix.is_empty() {
                (*word).to_owned()
            } else {
                format!("{prefix}:{word}")
            };
            if !self.ids.contains_key(&next) && !self.topics.contains_key(&next) {
                return unsupported("these words name no command");
            }
            prefix = next;
        }
        Ok(Walk::Level(prefix))
    }

    /// Counts the positional arguments in `words`, skipping flag values.
    fn arguments<'a>(
        &'a self,
        command: &'a Command,
        words: &[&str],
    ) -> Result<Walk<'a>, CompletionError> {
        let mut positional = 0;
        let mut dashdash = false;
        let mut words = words.iter();
        while let Some(word) = words.next() {
            if dashdash || *word == "-" || !word.starts_with('-') || is_negative_number(word) {
                positional += 1;
                continue;
            }
            if *word == "--" {
                dashdash = true;
                continue;
            }
            if self.help.iter().any(|h| h == word) {
                continue;
            }
            match command.flag(word) {
                Some(flag) => {
                    if flag.takes_value && !value_attached(word) {
                        words.next();
                    }
                }
                None if command.strict => {
                    return Err(CompletionError::Unsupported(
                        "an undeclared flag precedes this word".into(),
                    ));
                }
                None => {}
            }
        }
        Ok(Walk::Arguments(command, positional))
    }

    pub fn complete(
        &self,
        words: &[&str],
        prefix: &str,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        self.check()?;
        let walk = self.walk(words)?;
        let mut items = if prefix.starts_with('-') {
            self.options(&walk, words.is_empty())
        } else {
            match walk {
                Walk::Level(level) => self.level(&level)?,
                Walk::Arguments(command, positional) => self.argument(command, positional)?,
            }
        };
        items.retain(|item| item.value.starts_with(prefix));
        Ok(items)
    }

    fn options(&self, walk: &Walk<'_>, root: bool) -> Vec<CompletionItem> {
        let command = match walk {
            Walk::Level(level) => self
                .command(level)
                .filter(|_| !level.is_empty() || self.single),
            Walk::Arguments(command, _) => Some(*command),
        };
        let mut items: Vec<CompletionItem> = Vec::new();
        let mut push = |value: &str, takes_value: bool, description: Option<String>| {
            if !items.iter().any(|item| item.value == value) {
                items.push(CompletionItem {
                    value: value.to_owned(),
                    takes_value: Some(takes_value),
                    description,
                });
            }
        };
        for flag in command.iter().flat_map(|c| &c.flags) {
            for spelling in &flag.spellings {
                push(spelling, flag.takes_value, flag.description.clone());
            }
        }
        for help in &self.help {
            push(help, false, Some("Show help".into()));
        }
        // Version flags count only as the first word.
        if root {
            for version in &self.version {
                push(version, false, Some("Show version".into()));
            }
        }
        items
    }

    fn level(&self, level: &str) -> Result<Vec<CompletionItem>, CompletionError> {
        let mut items: Vec<CompletionItem> = Vec::new();
        let mut push = |value: &str, description: Option<String>| {
            if !value.is_empty() && !items.iter().any(|item| item.value == value) {
                items.push(CompletionItem {
                    value: value.to_owned(),
                    takes_value: None,
                    description,
                });
            }
        };
        let describe = |id: &str| {
            self.command(id)
                .and_then(|c| c.description.clone())
                .or_else(|| self.topics.get(id).cloned().flatten())
        };
        for id in self.ids.keys().chain(self.topics.keys()) {
            if !self.space {
                // The first word is a whole id, or a topic (oclif shows its help).
                push(id, describe(id));
                continue;
            }
            let rest = if level.is_empty() {
                Some(id.as_str())
            } else {
                id.strip_prefix(level)
                    .and_then(|rest| rest.strip_prefix(':'))
            };
            if let Some(rest) = rest {
                let word = rest.split(':').next().unwrap_or(rest);
                let full = if level.is_empty() {
                    word.to_owned()
                } else {
                    format!("{level}:{word}")
                };
                push(word, describe(&full));
            }
            // A first word containing `:` is taken as a whole id.
            if level.is_empty() && id.contains(':') {
                push(id, describe(id));
            }
        }
        if let Some(command) = self.command(level).filter(|_| !level.is_empty()) {
            if !command.strict {
                return Err(CompletionError::Unsupported(
                    "this command accepts any arguments".into(),
                ));
            }
            if let Some(choices) = command.args.first() {
                if choices.is_empty() {
                    return Err(CompletionError::Unsupported(
                        "this command's argument takes any value".into(),
                    ));
                }
                for choice in choices {
                    push(choice, None);
                }
            }
        }
        Ok(items)
    }

    fn argument(
        &self,
        command: &Command,
        positional: usize,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        match command.args.get(positional) {
            Some(choices) if command.strict && !choices.is_empty() => Ok(choices
                .iter()
                .map(|choice| CompletionItem {
                    value: choice.clone(),
                    takes_value: None,
                    description: None,
                })
                .collect()),
            _ => Err(CompletionError::Unsupported(
                "oclif declares no choices for this argument".into(),
            )),
        }
    }

    /// A flag's declared choices.
    pub fn values(&self, words: &[&str]) -> Result<Vec<CompletionItem>, CompletionError> {
        self.check()?;
        let Some((option, parent)) = words.split_last() else {
            return Ok(Vec::new());
        };
        let command = match self.walk(parent)? {
            Walk::Level(level) => self.command(&level),
            Walk::Arguments(command, _) => Some(command),
        };
        let flag = command.and_then(|c| c.flag(option)).ok_or_else(|| {
            CompletionError::Unsupported(format!("{option} is not a flag of this command"))
        })?;
        if !flag.takes_value || flag.choices.is_empty() {
            return Err(CompletionError::Unsupported(format!(
                "oclif declares no choices for {option}"
            )));
        }
        Ok(flag
            .choices
            .iter()
            .map(|choice| CompletionItem {
                value: choice.clone(),
                takes_value: None,
                description: None,
            })
            .collect())
    }

    /// A command from a just-in-time plugin installs that plugin first.
    pub fn resource(&self, words: &[&str], item: &CompletionItem) -> bool {
        if item.is_option() {
            return false;
        }
        let id = match self.walk(words) {
            Ok(Walk::Level(level)) if self.space && !level.is_empty() => {
                format!("{level}:{}", item.value)
            }
            Ok(Walk::Level(_)) => item.value.clone(),
            _ => return false,
        };
        self.command(&id).is_some_and(|command| command.jit)
    }
}

fn is_negative_number(word: &str) -> bool {
    word.len() > 1 && word[1..].parse::<f64>().is_ok()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::engine::native::{Discovery, NativeCompletionBackend, discover, tests::Dir};

    const ROOT_MANIFEST: &str = r#"{"version": "2.1.0", "commands": {
      "build": {"id": "build", "description": "start a build\nmore", "strict": true,
        "flags": {
          "platform": {"name": "platform", "char": "p", "type": "option", "options": ["android", "ios", "all"]},
          "wait": {"name": "wait", "type": "boolean", "allowNo": true, "description": "Wait"},
          "profile": {"name": "profile", "type": "option", "aliases": ["prof"]},
          "secret": {"name": "secret", "type": "boolean", "hidden": true}
        }, "args": {}},
      "build:list": {"id": "build:list", "strict": true, "flags": {}, "args": {}},
      "account:login": {"id": "account:login", "aliases": ["login"], "flags": {}, "args": {}},
      "go": {"id": "go", "hidden": true, "hiddenAliases": ["g0"], "flags": {}, "args": {}},
      "analytics": {"id": "analytics", "flags": {}, "args": {"STATUS": {"name": "STATUS", "options": ["on", "off"]}}},
      "exec": {"id": "exec", "strict": false, "flags": {}, "args": {}},
      "jit:install": {"id": "jit:install", "pluginType": "jit", "flags": {}, "args": {}}
    }}"#;

    struct App {
        dir: Dir,
        bin: PathBuf,
        package: PathBuf,
    }

    fn app(tag: &str, separator: Option<&str>) -> App {
        let dir = Dir::new(tag);
        let package = dir.0.join("lib/node_modules/fixture-cli");
        let separator =
            separator.map_or(String::new(), |s| format!(r#", "topicSeparator": "{s}""#));
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("package.json"),
            format!(
                r#"{{"name": "fixture-cli", "version": "2.1.0", "bin": {{"fixture": "bin/run"}},
                  "oclif": {{"bin": "fixture", "dirname": "notypo-test-oclif-fixture", "plugins": ["@scope/plugin-help"],
                  "additionalHelpFlags": ["-h"], "additionalVersionFlags": ["-v"],
                  "topics": {{"build": {{"description": "build app binaries"}}}}{separator}}}}}"#
            ),
        )
        .unwrap();
        std::fs::write(package.join("oclif.manifest.json"), ROOT_MANIFEST).unwrap();
        dir.script(
            "lib/node_modules/fixture-cli/bin/run",
            "#!/bin/sh\ntouch \"$0.ran\"\n",
        );
        let plugin = dir.0.join("lib/node_modules/@scope/plugin-help");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("package.json"),
            r#"{"name": "@scope/plugin-help", "version": "1.0.0-beta.1"}"#,
        )
        .unwrap();
        std::fs::write(
            plugin.join("oclif.manifest.json"),
            r#"{"version": "1.0.0", "commands": {"help": {"id": "help", "description": "show help", "flags": {"nested-commands": {"name": "nested-commands", "char": "n", "type": "boolean"}}, "args": {}}}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.0.join("bin")).unwrap();
        let bin = dir.0.join("bin/fixture");
        std::os::unix::fs::symlink("../lib/node_modules/fixture-cli/bin/run", &bin).unwrap();
        App { dir, bin, package }
    }

    fn protocol(app: &App) -> Protocol {
        discover_in(&app.bin, Some(&app.dir.0.join("data"))).unwrap()
    }

    fn values(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.value.as_str()).collect()
    }

    #[test]
    fn colon_ids_aliases_flags_choices_and_arguments_come_from_manifests() {
        let app = app("oclif-colon", None);
        let oclif = protocol(&app);
        assert!(oclif.complete);
        let root = oclif.complete(&[], "").unwrap();
        let names = values(&root);
        for id in [
            "build",
            "build:list",
            "account:login",
            "login",
            "go",
            "g0",
            "help",
            "account",
        ] {
            assert!(names.contains(&id), "{id}: {names:?}");
        }
        assert_eq!(
            root.iter()
                .find(|i| i.value == "build")
                .unwrap()
                .description
                .as_deref(),
            Some("start a build")
        );
        let flags = oclif.complete(&["build"], "-").unwrap();
        let names = values(&flags);
        for flag in [
            "--platform",
            "-p",
            "--wait",
            "--no-wait",
            "--profile",
            "--prof",
            "--secret",
            "--help",
            "-h",
        ] {
            assert!(names.contains(&flag), "{flag}: {names:?}");
        }
        assert!(!names.contains(&"--version"), "only as the first word");
        assert!(values(&oclif.complete(&[], "-").unwrap()).contains(&"-v"));
        assert_eq!(flags[0].takes_value, Some(true));
        assert_eq!(
            values(&oclif.values(&["build", "-p"]).unwrap()),
            ["android", "ios", "all"]
        );
        assert!(matches!(
            oclif.values(&["build", "--profile"]),
            Err(CompletionError::Unsupported(_))
        ));
        // Flag values are skipped when counting positionals.
        assert_eq!(
            values(&oclif.complete(&["analytics"], "").unwrap()),
            ["on", "off"]
        );
        assert!(matches!(
            oclif.complete(&["analytics", "on"], ""),
            Err(CompletionError::Unsupported(_))
        ));
        assert!(matches!(
            oclif.complete(&["exec"], ""),
            Err(CompletionError::Unsupported(_))
        ));
        assert!(!oclif.capabilities_for(&["exec"]).complete_options);
        assert!(oclif.capabilities_for(&["build"]).complete_options);
        assert!(oclif.capabilities_for(&[]).complete_subcommands);
        assert!(matches!(
            oclif.complete(&["build", "list"], ""),
            Err(CompletionError::Unsupported(_))
        ));
        let jit = CompletionItem {
            value: "jit:install".into(),
            takes_value: None,
            description: None,
        };
        assert!(oclif.resource(&[], &jit));
        assert!(!oclif.resource(&[], &root[0]));
        assert!(!app.package.join("bin/run.ran").exists());
    }

    #[test]
    fn spaced_topics_join_words_until_a_command_takes_arguments() {
        let app = app("oclif-space", Some(" "));
        let oclif = protocol(&app);
        let root = values(&oclif.complete(&[], "").unwrap()).join(",");
        assert!(
            root.contains("build") && root.contains("account") && root.contains("build:list"),
            "{root}"
        );
        assert!(!root.split(',').any(|w| w == "list"), "{root}");
        let build = oclif.complete(&["build"], "").unwrap();
        assert_eq!(values(&build), ["list"]);
        assert_eq!(
            values(&oclif.complete(&["account"], "").unwrap()),
            ["login"]
        );
        assert!(values(&oclif.complete(&["build", "list"], "-").unwrap()).contains(&"--help"));
        assert!(values(&oclif.complete(&["build", "--wait"], "-").unwrap()).contains(&"-p"));
        assert_eq!(
            values(&oclif.complete(&["analytics"], "").unwrap()),
            ["on", "off"]
        );
        assert!(matches!(
            oclif.complete(&["acount"], ""),
            Err(CompletionError::Unsupported(_))
        ));
    }

    #[test]
    fn user_and_linked_plugins_join_and_a_plugin_without_a_manifest_makes_lists_partial() {
        let app = app("oclif-plugins", None);
        let data = app.dir.0.join("data");
        let user = data.join("node_modules/fixture-plugin-extra");
        let linked = app.dir.0.join("linked-plugin");
        for (dir, name, id) in [
            (&user, "fixture-plugin-extra", "extra"),
            (&linked, "linked", "linked:run"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(
                dir.join("package.json"),
                format!(r#"{{"name": "{name}", "version": "0.3.0"}}"#),
            )
            .unwrap();
            std::fs::write(
                dir.join("oclif.manifest.json"),
                format!(r#"{{"version": "0.3.0", "commands": {{"{id}": {{"id": "{id}", "flags": {{}}, "args": {{}}}}}}}}"#),
            )
            .unwrap();
        }
        std::fs::write(
            data.join("package.json"),
            format!(
                r#"{{"oclif": {{"plugins": [{{"type": "user", "name": "fixture-plugin-extra"}}, {{"type": "link", "name": "linked", "root": "{}"}}]}}}}"#,
                linked.display()
            ),
        )
        .unwrap();
        let oclif = protocol(&app);
        let root = oclif.complete(&[], "").unwrap();
        assert!(values(&root).contains(&"extra") && values(&root).contains(&"linked:run"));
        assert!(oclif.complete);
        // A stale manifest is ignored by oclif, which then loads files.
        std::fs::write(
            user.join("oclif.manifest.json"),
            r#"{"version": "0.2.0", "commands": {"extra": {"id": "extra"}}}"#,
        )
        .unwrap();
        let oclif = protocol(&app);
        assert!(!values(&oclif.complete(&[], "").unwrap()).contains(&"extra"));
        assert!(!oclif.complete && !oclif.capabilities().complete_subcommands);
        // A manifest changed after reading fails rather than answering.
        std::fs::write(user.join("oclif.manifest.json"), r#"{"version": "0.3.0"}"#).unwrap();
        assert!(matches!(
            oclif.complete(&[], ""),
            Err(CompletionError::Failed(_))
        ));
    }

    #[test]
    fn discovery_needs_the_declared_bin_and_a_matching_root_manifest() {
        let app = app("oclif-discovery", None);
        assert!(matches!(
            discover("fixture", &app.bin, &[]),
            Discovery::Found(backend) if backend.id() == "oclif"
                && backend.capabilities().trust == Trust::Bridge
        ));
        // Another file of the package is not its declared bin.
        let other = app
            .dir
            .script("lib/node_modules/fixture-cli/bin/other", "#!/bin/sh\n");
        assert!(discover_in(&other, None).is_none());
        std::fs::write(
            app.package.join("oclif.manifest.json"),
            ROOT_MANIFEST.replace("2.1.0", "2.0.0"),
        )
        .unwrap();
        assert!(discover_in(&app.bin, None).is_none());
        assert!(!app.package.join("bin/run.ran").exists());
    }

    #[test]
    fn the_engine_decides_repairs_from_complete_manifests() {
        use crate::engine::{self, FailureContext, Outcome};
        use crate::settings::Settings;
        use crate::shells::Shell;
        use crate::types::Context;
        let app = app("oclif-engine", None);
        let oclif = protocol(&app);
        let mut backend =
            super::super::Backend::new(super::super::Flavor::Oclif, "fixture", app.bin.clone());
        backend.trust = Trust::Bridge;
        backend.bridge = Some(Box::new(super::super::Bridge::Oclif(oclif)));
        let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into())
            .with_which("fixture", Some(app.bin.to_str().unwrap()))
            .with_which("man", None)
            .with_history::<&str>(&[]);
        for (source, script) in [
            ("fixture biuld", "fixture build"),
            (
                "fixture build --platfrom ios",
                "fixture build --platform ios",
            ),
            ("fixture build --no-wiat", "fixture build --no-wait"),
            ("fixture buidl:list", "fixture build:list"),
            ("fixture logn", "fixture login"),
        ] {
            let failure = FailureContext {
                source: source.into(),
                ..FailureContext::default()
            };
            let rc: std::rc::Rc<dyn NativeCompletionBackend> = std::rc::Rc::new(backend.clone());
            let report =
                engine::correct_with_backends(&failure, &ctx, vec![("fixture".into(), rc)]);
            assert!(
                matches!(report.outcome, Outcome::Suggestion(_)),
                "{source}: {:?}",
                report.outcome
            );
            assert_eq!(report.outcome.candidates()[0].script, script);
            assert_eq!(report.probes, 0);
        }
        assert!(!app.package.join("bin/run.ran").exists());
    }
}
