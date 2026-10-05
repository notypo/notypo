//! .NET SDK completion parses a line and prints completion labels without
//! invoking its command. Its callbacks can evaluate MSBuild projects; the
//! provider applies workspace trust before any SDK probe. Recent SDKs also
//! expose their parser schema, supplying arity and descriptions without
//! invoking a command's help handler or an external dotnet-* tool.

use super::{Backend, Capabilities, CompletionError, CompletionItem, Flavor, Trust};
use crate::engine::probe::{self, Budget, Capture, Probe};
use serde_json::Value;
use std::cell::RefCell;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub(super) struct Installation {
    sdks: Vec<(String, bool)>,
    files: Vec<PathBuf>,
}

fn read(path: &Path, limit: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path).ok()?.take(limit as u64 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= limit).then_some(bytes)
}

/// A registered SDK host, with its own dependency manifest and completion
/// implementation. Merely naming an unrelated program dotnet proves nothing.
pub(super) fn installation(path: &Path) -> Option<Installation> {
    let real = fs::canonicalize(path).ok()?;
    if real.file_name()?.to_str()? != format!("dotnet{}", std::env::consts::EXE_SUFFIX) {
        return None;
    }
    let root = real.parent()?;
    let mut files = vec![path.to_owned(), real.clone(), root.join("sdk")];
    let mut sdks = Vec::new();
    for entry in fs::read_dir(root.join("sdk")).ok()?.filter_map(Result::ok).take(128) {
        let version = entry.file_name().to_str()?.to_owned();
        let directory = entry.path();
        let receipt = read(&directory.join(".version"), 4096)?;
        if std::str::from_utf8(&receipt).ok()?.lines().nth(1) != Some(&version) {
            continue;
        }
        let deps: Value = serde_json::from_slice(&read(&directory.join("dotnet.deps.json"), 1024 * 1024)?).ok()?;
        if !deps.get("libraries")?.as_object()?.contains_key(&format!("dotnet.deps.json/{version}")) {
            continue;
        }
        let assembly = read(&directory.join("dotnet.dll"), 32 * 1024 * 1024)?;
        let has = |symbol: &[u8]| assembly.windows(symbol.len()).any(|window| window == symbol);
        if !has(b"CompleteCommand") || !has(b"GetCompletions") {
            continue;
        }
        sdks.push((version, has(b"PrintCliSchemaAction")));
        files.extend([directory.join(".version"), directory.join("dotnet.deps.json"), directory.join("dotnet.dll")]);
    }
    (!sdks.is_empty()).then_some(Installation { sdks, files })
}

#[derive(Clone, Debug)]
pub(super) struct Protocol {
    binding: String,
    installation: Installation,
    schema: Rc<RefCell<Option<Value>>>,
}

struct Scope<'a> {
    command: &'a Value,
    ancestors: Vec<&'a Value>,
    pending: Option<&'a Value>,
    arguments: bool,
}

fn option<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get("options")?.as_object()?.iter().find_map(|(key, value)| {
        (key == name || value.get("aliases").and_then(Value::as_array).is_some_and(|aliases| aliases.iter().any(|alias| alias.as_str() == Some(name)))).then_some(value)
    })
}

fn command<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get("subcommands")?.as_object()?.iter().find_map(|(key, value)| {
        (key == name || value.get("aliases").and_then(Value::as_array).is_some_and(|aliases| aliases.iter().any(|alias| alias.as_str() == Some(name)))).then_some(value)
    })
}

fn arity(option: &Value) -> Option<bool> {
    option.get("arity")?.get("maximum")?.as_u64().map(|max| max != 0)
}

fn scope<'a>(schema: &'a Value, words: &[&str]) -> Scope<'a> {
    let mut result = Scope { command: schema, ancestors: Vec::new(), pending: None, arguments: false };
    for word in words {
        if result.pending.take().is_some() {
            continue;
        }
        if *word == "--" {
            result.arguments = true;
            break;
        }
        let (name, attached) = word.split_once(['=', ':']).map_or((*word, false), |(name, _)| (name, true));
        let flag = option(result.command, name).or_else(|| result.ancestors.iter().rev().find_map(|parent| option(parent, name).filter(|flag| flag["recursive"] == true)));
        if let Some(flag) = flag {
            if !attached && arity(flag) == Some(true) {
                result.pending = Some(flag);
            }
        } else if let Some(child) = command(result.command, word) {
            result.ancestors.push(result.command);
            result.command = child;
        } else if !word.starts_with('-') {
            result.arguments = true;
        }
    }
    result
}

/// System.CommandLine's string tokenizer strips all double quotes and has
/// no escape for literal ones. Its cursor indexes UTF-16 code units.
fn line(words: &[&str], prefix: &str) -> Result<String, CompletionError> {
    let mut result = "dotnet".to_owned();
    for word in words.iter().copied().chain(std::iter::once(prefix)) {
        if word.contains('"') || word.contains(char::is_control) || word.starts_with('@') || word.starts_with('[') {
            return Err(CompletionError::Unsupported("dotnet completion cannot preserve quotes, control characters, response files, or parser directives in this context".into()));
        }
        result.push(' ');
        if word.is_empty() && word == prefix {
            continue;
        }
        if word.is_empty() || word.contains(char::is_whitespace) {
            result.push('"'); result.push_str(word); result.push('"');
        } else {
            result.push_str(word);
        }
    }
    Ok(result)
}

impl Protocol {
    pub fn new(path: &Path) -> Option<Self> {
        let installation = installation(path)?;
        Some(Self { binding: super::super::cache::fingerprint(&installation.files, &["dotnet"]), installation, schema: Default::default() })
    }

    fn checked(&self, backend: &Backend) -> Result<(), CompletionError> {
        if self.binding != super::super::cache::fingerprint(&self.installation.files, &["dotnet"]) {
            return Err(CompletionError::Failed("dotnet installation changed after discovery".into()));
        }
        if !backend.completer.exists() {
            return Err(CompletionError::Failed("dotnet host is unavailable".into()));
        }
        Ok(())
    }

    fn raw(&self, backend: &Backend, args: &[String], budget: &mut Budget) -> Result<String, CompletionError> {
        self.checked(backend)?;
        let mut context = vec!["dotnet".to_owned()];
        if let Ok(cwd) = std::env::current_dir() {
            let files: Vec<_> = cwd.ancestors().map(|dir| dir.join("global.json")).collect();
            context.push(super::super::cache::fingerprint(&files, &[cwd.to_str().unwrap_or_default()]));
        }
        context.extend_from_slice(args);
        let key: Vec<_> = context.iter().map(String::as_str).collect();
        backend.memoized(&key, || {
            let home = crate::utils::cache_dir().join("dotnet-probes");
            fs::create_dir_all(&home).map_err(|error| CompletionError::Failed(format!("cannot prepare dotnet probe state: {error}")))?;
            let mut env = super::offline_env(Flavor::Dotnet);
            env.push(("DOTNET_CLI_HOME".into(), Some(home.into_os_string())));
            let output = probe::run(&Probe { program: &backend.completer, args: args.iter().map(Into::into).collect(), env, capture: Capture::Stdout }, budget).map_err(super::probe_error)?;
            if output.status != Some(0) || output.truncated {
                return Err(CompletionError::Failed(format!("dotnet completion failed (status {:?}, truncated {})", output.status, output.truncated)));
            }
            String::from_utf8(output.data).map_err(|_| CompletionError::Failed("dotnet completion output is not UTF-8".into()))
        })
    }

    fn schema(&self, backend: &Backend, budget: &mut Budget) -> Result<Option<Value>, CompletionError> {
        if let Some(schema) = self.schema.borrow().clone() { return Ok(Some(schema)); }
        let version = self.raw(backend, &["--version".into()], budget)?;
        let Some((_, has_schema)) = self.installation.sdks.iter().find(|(sdk, _)| sdk == version.trim()) else {
            return Err(CompletionError::Unsupported("dotnet selected an SDK whose completion implementation was not verified".into()));
        };
        if !has_schema { return Ok(None); }
        let schema: Value = serde_json::from_str(&self.raw(backend, &["--cli-schema".into()], budget)?).map_err(|_| CompletionError::Failed("dotnet parser schema is not valid JSON".into()))?;
        if schema["name"] != "dotnet" || !schema["subcommands"].is_object() || !schema["options"].is_object() {
            return Err(CompletionError::Failed("dotnet returned an unsupported parser schema".into()));
        }
        *self.schema.borrow_mut() = Some(schema.clone());
        Ok(Some(schema))
    }

    pub fn capabilities(&self, trust: Trust) -> Capabilities {
        Capabilities { subcommands: true, options: true, option_arity: self.installation.sdks.iter().any(|(_, schema)| *schema), values: true, resources: false, option_prefix: "-", short_options: true, complete_options: false, complete_subcommands: false, descriptions: true, query_dialect: None, trust }
    }

    pub fn resources(&self, words: &[&str]) -> bool {
        self.schema.borrow().as_ref().is_some_and(|schema| {
            let scope = scope(schema, words);
            scope.pending.is_some() || scope.arguments || scope.command.get("arguments").and_then(Value::as_object).is_some_and(|args| !args.is_empty())
        })
    }

    pub fn confirms(&self, words: &[&str], typed: &str) -> Option<bool> {
        let schema = self.schema.borrow();
        let schema = schema.as_ref()?;
        let scope = scope(schema, words);
        (!scope.arguments && scope.pending.is_none()).then(|| command(scope.command, typed).is_some() || option(scope.command, typed).is_some()).filter(|present| *present)
    }

    pub fn complete(&self, backend: &Backend, words: &[&str], prefix: &str, budget: &mut Budget) -> Result<Vec<CompletionItem>, CompletionError> {
        let line = line(words, prefix)?;
        let schema = self.schema(backend, budget)?;
        let text = self.raw(backend, &["complete".into(), "--position".into(), line.encode_utf16().count().to_string(), line], budget)?;
        let scope = schema.as_ref().map(|schema| scope(schema, words));
        let mut result: Vec<CompletionItem> = Vec::new();
        for value in text.lines().filter(|value| !value.is_empty()) {
            if value.contains(char::is_control) { return Err(CompletionError::Failed("dotnet returned an invalid completion label".into())); }
            if result.len() >= budget.max_candidates { return Err(CompletionError::Failed("dotnet completion exceeded the candidate limit".into())); }
            // Slash aliases are accepted as literal context but the engine's
            // option role currently uses '-'. Do not offer them as commands.
            if value.starts_with('/') || !value.starts_with(prefix) { continue; }
            let metadata = scope.as_ref().and_then(|scope| {
                option(scope.command, value).or_else(|| scope.ancestors.iter().rev().find_map(|parent| option(parent, value).filter(|item| item["recursive"] == true))).or_else(|| command(scope.command, value))
            });
            result.push(CompletionItem { value: value.into(), takes_value: metadata.filter(|_| value.starts_with('-')).and_then(arity), description: metadata.and_then(|item| item["description"].as_str()).and_then(super::clean_description) });
        }
        Ok(result)
    }
}
