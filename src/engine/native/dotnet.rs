//! .NET SDK completion parses a line and prints completion labels without
//! invoking its command. Its callbacks can evaluate MSBuild projects; the
//! provider applies workspace trust before any SDK probe. Recent SDKs also
//! expose their parser schema, supplying arity and descriptions without
//! invoking a command's help handler or an external dotnet-* tool.

use super::{Backend, Capabilities, CompletionError, CompletionItem, Flavor, Trust};
use crate::engine::probe::{self, Budget, Capture, Probe};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Debug)]
pub(super) struct Installation {
    files: Vec<PathBuf>,
}

fn read(path: &Path, limit: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
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
    let mut verified = false;
    for entry in fs::read_dir(root.join("sdk"))
        .ok()?
        .filter_map(Result::ok)
        .take(128)
    {
        let Some(version) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if let Some((_, sdk_files)) = sdk(&entry.path(), &version) {
            verified = true;
            files.extend(sdk_files);
        }
    }
    verified.then_some(Installation { files })
}

/// Prove the selected SDK too: global.json can select an SDK from another
/// root, including one with the same version as a different installation.
fn sdk(directory: &Path, version: &str) -> Option<(bool, Vec<PathBuf>)> {
    if directory.file_name()?.to_str()? != version {
        return None;
    }
    let receipt = read(&directory.join(".version"), 4096)?;
    if std::str::from_utf8(&receipt).ok()?.lines().nth(1) != Some(version) {
        return None;
    }
    let deps: Value =
        serde_json::from_slice(&read(&directory.join("dotnet.deps.json"), 1024 * 1024)?).ok()?;
    if !deps
        .get("libraries")?
        .as_object()?
        .contains_key(&format!("dotnet.deps.json/{version}"))
    {
        return None;
    }
    let assembly = read(&directory.join("dotnet.dll"), 32 * 1024 * 1024)?;
    static SYMBOLS: OnceLock<regex::bytes::Regex> = OnceLock::new();
    let symbols = SYMBOLS.get_or_init(|| {
        regex::bytes::Regex::new("(?-u:CompleteCommand|GetCompletions|PrintCliSchemaAction)")
            .expect("fixed SDK symbol pattern")
    });
    let (mut complete, mut completions, mut schema) = (false, false, false);
    for symbol in symbols.find_iter(&assembly) {
        match symbol.as_bytes() {
            b"CompleteCommand" => complete = true,
            b"GetCompletions" => completions = true,
            b"PrintCliSchemaAction" => schema = true,
            _ => unreachable!(),
        }
        if complete && completions && schema {
            break;
        }
    }
    (complete && completions).then(|| {
        (
            schema,
            vec![
                directory.join(".version"),
                directory.join("dotnet.deps.json"),
                directory.join("dotnet.dll"),
            ],
        )
    })
}

#[derive(Clone, Debug)]
pub(super) struct Protocol {
    binding: String,
    installation: Installation,
    schema: Rc<RefCell<Option<Schema>>>,
    home: Rc<RefCell<Option<Rc<ProbeHome>>>>,
    hives: Rc<RefCell<HashMap<(PathBuf, bool), PathBuf>>>,
    original_home: PathBuf,
    pub trusted_workspaces: Vec<String>,
    pub cwd: Option<PathBuf>,
}

/// SDK startup and template discovery write state. Preserve the user's
/// installed templates by copying their hive, and confine new writes to an
/// owned, request-local directory. Never persist raw lines or schema answers.
#[derive(Debug)]
struct ProbeHome(PathBuf);

impl Drop for ProbeHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_hive(source: &Path, destination: &Path) -> Result<(), CompletionError> {
    let mut pending = vec![(source.to_owned(), destination.to_owned())];
    let mut entries = 0;
    let mut bytes = 0_u64;
    while let Some((source, destination)) = pending.pop() {
        let metadata = fs::metadata(&source).map_err(|error| {
            CompletionError::Failed(format!("cannot inspect dotnet template state: {error}"))
        })?;
        entries += 1;
        bytes = bytes.saturating_add(if metadata.is_file() {
            metadata.len()
        } else {
            0
        });
        if entries > 4096 || bytes > 64 * 1024 * 1024 {
            return Err(CompletionError::Unsupported(
                "dotnet template state exceeds the probe snapshot limit".into(),
            ));
        }
        if metadata.is_dir() {
            fs::create_dir_all(&destination).map_err(|error| {
                CompletionError::Failed(format!("cannot prepare dotnet template state: {error}"))
            })?;
            for entry in fs::read_dir(source).map_err(|error| {
                CompletionError::Failed(format!("cannot read dotnet template state: {error}"))
            })? {
                let entry = entry.map_err(|error| {
                    CompletionError::Failed(format!("cannot read dotnet template entry: {error}"))
                })?;
                pending.push((entry.path(), destination.join(entry.file_name())));
            }
        } else if metadata.is_file() {
            fs::copy(source, destination).map_err(|error| {
                CompletionError::Failed(format!("cannot snapshot dotnet template state: {error}"))
            })?;
        } else {
            return Err(CompletionError::Unsupported(
                "dotnet template state contains a non-file entry".into(),
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct Schema {
    context: String,
    value: Option<Rc<Value>>,
    files: Vec<PathBuf>,
    binding: String,
}

fn context_key(cwd: &Path) -> String {
    let mut files: Vec<_> = cwd
        .ancestors()
        .flat_map(|dir| {
            [
                "global.json",
                "Directory.Build.props",
                "Directory.Build.targets",
                "Directory.Packages.props",
            ]
            .map(|file| dir.join(file))
        })
        .collect();
    for directory in cwd.ancestors() {
        if let Ok(entries) = fs::read_dir(directory) {
            files.extend(
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|extension| {
                                let extension = extension.to_ascii_lowercase();
                                extension.ends_with("proj")
                                    || ["sln", "slnx"].contains(&extension.as_str())
                            })
                    }),
            );
        }
    }
    files.sort();
    super::super::cache::fingerprint(&files, &[&cwd.to_string_lossy()])
}

struct Scope<'a> {
    command: &'a Value,
    ancestors: Vec<&'a Value>,
    pending: Option<&'a Value>,
    arguments: bool,
}

fn option<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get("options")?
        .as_object()?
        .iter()
        .find_map(|(key, value)| {
            (key == name
                || value
                    .get("aliases")
                    .and_then(Value::as_array)
                    .is_some_and(|aliases| {
                        aliases.iter().any(|alias| alias.as_str() == Some(name))
                    }))
            .then_some(value)
        })
}

fn command<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get("subcommands")?
        .as_object()?
        .iter()
        .find_map(|(key, value)| {
            (key == name
                || value
                    .get("aliases")
                    .and_then(Value::as_array)
                    .is_some_and(|aliases| {
                        aliases.iter().any(|alias| alias.as_str() == Some(name))
                    }))
            .then_some(value)
        })
}

fn arity(option: &Value) -> Option<bool> {
    let arity = option.get("arity")?;
    let maximum = arity.get("maximum")?.as_u64()?;
    if maximum == 0 {
        Some(false)
    } else if arity.get("minimum")?.as_u64()? > 0 {
        Some(true)
    } else {
        None
    } // Optional values cannot be represented as required ones.
}

pub(super) fn option_separator(typed: &str) -> Option<char> {
    let name = typed.split(['=', ':']).next()?;
    (name.len() > 1
        && name.starts_with(['-', '/'])
        && name[1..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_?".contains(&byte)))
    .then_some(if typed.contains('=') {
        '='
    } else if typed.contains(':') {
        ':'
    } else {
        '='
    })
}

fn scoped_option<'a>(scope: &Scope<'a>, name: &str) -> Option<&'a Value> {
    option(scope.command, name).or_else(|| {
        scope
            .ancestors
            .iter()
            .rev()
            .find_map(|parent| option(parent, name).filter(|flag| flag["recursive"] == true))
    })
}

fn item(value: &str, metadata: &Value) -> CompletionItem {
    CompletionItem {
        value: value.into(),
        takes_value: arity(metadata),
        description: metadata["description"]
            .as_str()
            .and_then(super::clean_description),
    }
}

fn scope<'a>(schema: &'a Value, words: &[&str]) -> Scope<'a> {
    let mut result = Scope {
        command: schema,
        ancestors: Vec::new(),
        pending: None,
        arguments: false,
    };
    for word in words {
        if let Some(flag) = result.pending.take() {
            let optional = flag["arity"]["minimum"] == 0;
            if !optional || (!word.starts_with('-') && !word.starts_with('/')) {
                continue;
            }
        }
        if *word == "--" {
            result.arguments = true;
            break;
        }
        let (flag, attached) = scoped_option(&result, word)
            .map(|flag| (Some(flag), false))
            .or_else(|| {
                // Colons can belong to an option name (--debug:custom-hive)
                // as well as attach its value. Prefer the longest known name.
                word.char_indices().rev().find_map(|(index, character)| {
                    matches!(character, '=' | ':')
                        .then(|| scoped_option(&result, &word[..index]))
                        .flatten()
                        .map(|flag| (Some(flag), true))
                })
            })
            .unwrap_or((None, false));
        if let Some(flag) = flag {
            if !attached && flag["arity"]["maximum"].as_u64().is_none_or(|max| max != 0) {
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
    for (index, word) in words
        .iter()
        .copied()
        .chain(std::iter::once(prefix))
        .enumerate()
    {
        if word.contains('"')
            || word.contains(char::is_control)
            || word.starts_with('@')
            || word.starts_with('[')
        {
            return Err(CompletionError::Unsupported("dotnet completion cannot preserve quotes, control characters, response files, or parser directives in this context".into()));
        }
        result.push(' ');
        if word.is_empty() && index == words.len() {
            continue;
        }
        if word.is_empty() || word.contains(char::is_whitespace) {
            result.push('"');
            result.push_str(word);
            result.push('"');
        } else {
            result.push_str(word);
        }
    }
    Ok(result)
}

impl Protocol {
    fn template_start(&self, words: &[&str]) -> Option<usize> {
        let index = words
            .iter()
            .position(|word| *word == "--" || !word.starts_with(['-', '/']))?;
        let schema = self.schema.borrow();
        let alias = schema
            .as_ref()
            .and_then(|schema| schema.value.as_ref())
            .and_then(|schema| schema["subcommands"]["new"]["aliases"].as_array())
            .is_some_and(|aliases| {
                aliases
                    .iter()
                    .any(|alias| alias.as_str() == Some(words[index]))
            });
        (words[index] == "new" || alias).then_some(index)
    }

    pub fn new(path: &Path) -> Option<Self> {
        let installation = installation(path)?;
        Some(Self {
            binding: super::super::cache::fingerprint(&installation.files, &["dotnet"]),
            installation,
            schema: Default::default(),
            home: Default::default(),
            hives: Default::default(),
            original_home: std::env::var_os("DOTNET_CLI_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| crate::utils::expand_user("~")),
            trusted_workspaces: Vec::new(),
            cwd: None,
        })
    }

    fn directory(&self) -> Result<PathBuf, CompletionError> {
        let directory = self
            .cwd
            .clone()
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
            .and_then(fs::canonicalize);
        directory.map_err(|error| {
            CompletionError::Failed(format!("no dotnet working directory: {error}"))
        })
    }

    fn home(&self) -> Result<Rc<ProbeHome>, CompletionError> {
        if let Some(home) = self.home.borrow().as_ref() {
            return Ok(home.clone());
        }
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let parent = crate::utils::cache_dir().join("dotnet-probes");
        fs::create_dir_all(&parent).map_err(|error| {
            CompletionError::Failed(format!("cannot prepare dotnet probe state: {error}"))
        })?;
        let parent = fs::canonicalize(parent).map_err(|error| {
            CompletionError::Failed(format!("cannot resolve dotnet probe state: {error}"))
        })?;
        let path = parent.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).map_err(|error| {
            CompletionError::Failed(format!("cannot create dotnet probe home: {error}"))
        })?;
        let home = Rc::new(ProbeHome(path));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&home.0, fs::Permissions::from_mode(0o700)).map_err(|error| {
                CompletionError::Failed(format!("cannot protect dotnet probe state: {error}"))
            })?;
        }
        *self.home.borrow_mut() = Some(home.clone());
        Ok(home)
    }

    fn snapshot(&self, source: &Path, default: bool) -> Result<PathBuf, CompletionError> {
        let source = self.directory()?.join(source);
        let source = fs::canonicalize(&source).unwrap_or(source);
        let key = (source.clone(), default);
        if let Some(destination) = self.hives.borrow().get(&key) {
            return Ok(destination.clone());
        }
        let home = self.home()?;
        let destination = if default {
            home.0.join(".templateengine")
        } else {
            home.0
                .join("hives")
                .join(self.hives.borrow().len().to_string())
        };
        if source.exists() {
            copy_hive(&source, &destination)?;
        } else {
            fs::create_dir_all(&destination).map_err(|error| {
                CompletionError::Failed(format!("cannot prepare dotnet template hive: {error}"))
            })?;
        }
        self.hives.borrow_mut().insert(key, destination.clone());
        Ok(destination)
    }

    /// These selectors affect completion itself, rather than the operation's
    /// eventual output. Check them before even bootstrapping the SDK schema.
    fn template_projects(&self, words: &[&str]) -> Result<(), CompletionError> {
        let Some(start) = self.template_start(words) else {
            return Ok(());
        };
        let cwd = self.directory()?;
        let mut index = start + 1;
        while index < words.len() {
            let word = words[index];
            if word == "--" {
                break;
            }
            let (name, attached) = word
                .split_once(['=', ':'])
                .map_or((word, None), |(name, value)| (name, Some(value)));
            if matches!(name, "--output" | "-o" | "--project") {
                let value = attached.or_else(|| words.get(index + 1).copied());
                if attached.is_none() && value.is_some() {
                    index += 1;
                }
                if let Some(value) = value.filter(|value| !value.is_empty()) {
                    let path = cwd.join(value);
                    let path = fs::canonicalize(&path).unwrap_or(path);
                    let directory = if name == "--project" {
                        path.parent().unwrap_or(&cwd)
                    } else {
                        &path
                    };
                    let directory = directory
                        .ancestors()
                        .find_map(|ancestor| fs::canonicalize(ancestor).ok())
                        .ok_or_else(|| {
                            CompletionError::Failed(
                                "cannot resolve dotnet template project directory".into(),
                            )
                        })?;
                    // Explicit --project can name any MSBuild file, regardless
                    // of its extension. Output paths also search upward.
                    let reason = if name == "--project"
                        && path.is_file()
                        && !crate::workspace::is_trusted(&directory, &self.trusted_workspaces)
                    {
                        Some(format!(
                            "dotnet completion would evaluate {}; add {} to trusted_workspaces",
                            path.display(),
                            directory.display()
                        ))
                    } else {
                        crate::workspace::untrusted_project(
                            "dotnet",
                            &directory,
                            &self.trusted_workspaces,
                        )
                    };
                    if let Some(reason) = reason {
                        return Err(CompletionError::Unsupported(reason));
                    }
                }
            }
            index += 1;
        }
        Ok(())
    }

    fn template_words(&self, words: &[&str]) -> Result<Vec<String>, CompletionError> {
        let mut result: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
        let Some(start) = self.template_start(words) else {
            return Ok(result);
        };
        let mut custom = false;
        let mut index = start + 1;
        while index < result.len() {
            if result[index] == "--" {
                break;
            }
            let word = &result[index];
            if word
                .strip_prefix("--debug:custom-hive")
                .and_then(|suffix| suffix.strip_prefix(['=', ':']))
                .is_some()
            {
                return Err(CompletionError::Unsupported("dotnet does not accept an attached value for --debug:custom-hive; use a separate argument".into()));
            } else if word == "--debug:custom-hive"
                && let Some(value) = result.get(index + 1).filter(|value| !value.is_empty())
            {
                let destination = self.snapshot(Path::new(value), false)?;
                result[index + 1] = destination.to_string_lossy().into_owned();
                custom = true;
                index += 1;
            }
            index += 1;
        }
        if !custom {
            self.snapshot(&self.original_home.join(".templateengine"), true)?;
        }
        Ok(result)
    }

    fn checked(&self, backend: &Backend) -> Result<(), CompletionError> {
        let cwd = self.directory()?;
        if let Some(why) =
            crate::workspace::untrusted_project("dotnet", &cwd, &self.trusted_workspaces).or_else(
                || {
                    crate::workspace::untrusted_project(
                        &backend.completer.to_string_lossy(),
                        &cwd,
                        &self.trusted_workspaces,
                    )
                },
            )
        {
            return Err(CompletionError::Unsupported(why));
        }
        if self.binding != super::super::cache::fingerprint(&self.installation.files, &["dotnet"]) {
            return Err(CompletionError::Failed(
                "dotnet installation changed after discovery".into(),
            ));
        }
        if self.schema.borrow().as_ref().is_some_and(|schema| {
            schema.binding != super::super::cache::fingerprint(&schema.files, &["dotnet-sdk"])
        }) {
            return Err(CompletionError::Failed(
                "dotnet selected SDK changed after discovery".into(),
            ));
        }
        if !backend.completer.exists() {
            return Err(CompletionError::Failed("dotnet host is unavailable".into()));
        }
        Ok(())
    }

    fn raw(
        &self,
        backend: &Backend,
        args: &[String],
        budget: &mut Budget,
    ) -> Result<String, CompletionError> {
        self.checked(backend)?;
        let mut context = vec!["dotnet".to_owned()];
        let directory = self.directory()?;
        context.push(context_key(&directory));
        context.extend_from_slice(args);
        let key: Vec<_> = context.iter().map(String::as_str).collect();
        backend.memoized(&key, || {
            let home = self.home()?;
            let mut env = super::offline_env(Flavor::Dotnet);
            if args.first().is_some_and(|arg| arg == "--info") {
                env.push(("DOTNET_CLI_UI_LANGUAGE".into(), Some("en-US".into())));
            }
            env.push((
                "DOTNET_CLI_HOME".into(),
                Some(home.0.clone().into_os_string()),
            ));
            let output = probe::run_in(
                &Probe {
                    program: &backend.completer,
                    args: args.iter().map(Into::into).collect(),
                    env,
                    capture: Capture::Stdout,
                },
                &directory,
                budget,
            )
            .map_err(super::probe_error)?;
            if output.status != Some(0) || output.truncated {
                return Err(CompletionError::Failed(format!(
                    "dotnet completion failed (status {:?}, truncated {})",
                    output.status, output.truncated
                )));
            }
            String::from_utf8(output.data).map_err(|_| {
                CompletionError::Failed("dotnet completion output is not UTF-8".into())
            })
        })
    }

    fn schema(
        &self,
        backend: &Backend,
        budget: &mut Budget,
    ) -> Result<Option<Rc<Value>>, CompletionError> {
        self.checked(backend)?;
        let context = context_key(&self.directory()?);
        if let Some(schema) = self
            .schema
            .borrow()
            .as_ref()
            .filter(|schema| schema.context == context)
        {
            return Ok(schema.value.clone());
        }
        let version = self.raw(backend, &["--version".into()], budget)?;
        let info = self.raw(backend, &["--info".into()], budget)?;
        let mut bases = info
            .lines()
            .filter_map(|line| line.trim().strip_prefix("Base Path:").map(str::trim));
        let directory = bases.next().and_then(|path| fs::canonicalize(path).ok());
        let Some((has_schema, files)) = directory
            .filter(|_| bases.next().is_none())
            .and_then(|directory| sdk(&directory, version.trim()))
        else {
            return Err(CompletionError::Unsupported(
                "dotnet selected an SDK whose completion implementation was not verified".into(),
            ));
        };
        let binding = super::super::cache::fingerprint(&files, &["dotnet-sdk"]);
        if !has_schema {
            *self.schema.borrow_mut() = Some(Schema {
                context,
                value: None,
                files,
                binding,
            });
            return Ok(None);
        }
        let schema: Value =
            serde_json::from_str(&self.raw(backend, &["--cli-schema".into()], budget)?).map_err(
                |_| CompletionError::Failed("dotnet parser schema is not valid JSON".into()),
            )?;
        if schema["name"] != "dotnet"
            || !schema["subcommands"].is_object()
            || !schema["options"].is_object()
        {
            return Err(CompletionError::Failed(
                "dotnet returned an unsupported parser schema".into(),
            ));
        }
        let schema = Rc::new(schema);
        *self.schema.borrow_mut() = Some(Schema {
            context,
            value: Some(schema.clone()),
            files,
            binding,
        });
        Ok(Some(schema))
    }

    pub fn capabilities(&self, trust: Trust) -> Capabilities {
        let metadata = self
            .schema
            .borrow()
            .as_ref()
            .is_some_and(|schema| schema.value.is_some());
        Capabilities {
            subcommands: true,
            options: true,
            option_arity: metadata,
            values: true,
            resources: false,
            option_prefix: "-",
            short_options: true,
            complete_options: false,
            complete_subcommands: false,
            descriptions: metadata,
            query_dialect: None,
            trust,
        }
    }

    pub fn resources(&self, words: &[&str]) -> bool {
        self.schema
            .borrow()
            .as_ref()
            .and_then(|schema| schema.value.as_ref())
            .is_none_or(|schema| {
                let scope = scope(schema, words);
                if scope
                    .pending
                    .is_some_and(|flag| flag["valueType"] == "System.Boolean")
                {
                    return false;
                }
                scope.pending.is_some()
                    || scope.arguments
                    || (!scope.ancestors.is_empty()
                        && scope
                            .command
                            .get("arguments")
                            .and_then(Value::as_object)
                            .is_some_and(|args| !args.is_empty()))
            })
    }

    pub fn confirms(&self, words: &[&str], typed: &str) -> Option<bool> {
        let schema = self.schema.borrow();
        let schema = schema.as_ref()?.value.as_ref()?;
        let scope = scope(schema, words);
        (!scope.arguments && scope.pending.is_none())
            .then(|| {
                command(scope.command, typed).is_some() || option(scope.command, typed).is_some()
            })
            .filter(|present| *present)
    }

    pub fn is_option(&self, words: &[&str], typed: &str) -> bool {
        self.schema
            .borrow()
            .as_ref()
            .and_then(|schema| schema.value.as_ref())
            .is_some_and(|schema| scoped_option(&scope(schema, words), typed).is_some())
    }

    pub fn option_syntax(&self, typed: &str) -> Option<(usize, char)> {
        let schema = self.schema.borrow();
        let mut nodes: Vec<&Value> = schema
            .as_ref()
            .and_then(|schema| schema.value.as_deref())
            .into_iter()
            .collect();
        let mut longest: Option<(usize, char)> = None;
        let mut namespace = false;
        while let Some(node) = nodes.pop() {
            if let Some(options) = node["options"].as_object() {
                for (name, metadata) in options {
                    for name in std::iter::once(name.as_str()).chain(
                        metadata["aliases"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str),
                    ) {
                        if let Some((stem, _)) = name.split_once(':') {
                            namespace |= typed.starts_with(&format!("{stem}:"));
                        }
                        if let Some(suffix) = typed.strip_prefix(name)
                            && (suffix.is_empty() || suffix.starts_with(['=', ':']))
                            && longest.is_none_or(|(length, _)| name.len() > length)
                        {
                            longest =
                                Some((name.len(), if suffix.starts_with(':') { ':' } else { '=' }));
                        }
                    }
                }
            }
            if let Some(commands) = node["subcommands"].as_object() {
                nodes.extend(commands.values());
            }
        }
        longest.or_else(|| {
            let separator = if namespace {
                '='
            } else {
                option_separator(typed)?
            };
            Some((typed.find(separator).unwrap_or(typed.len()), separator))
        })
    }

    pub fn resolve_option(&self, words: &[&str], typed: &str) -> Option<CompletionItem> {
        let schema = self.schema.borrow();
        let schema = schema.as_ref()?.value.as_ref()?;
        let scope = scope(schema, words);
        scoped_option(&scope, typed).map(|metadata| item(typed, metadata))
    }

    pub fn prepare_values(
        &self,
        words: &[&str],
        typed: &str,
        mut items: Vec<CompletionItem>,
    ) -> Vec<CompletionItem> {
        let schema = self.schema.borrow();
        let boolean = schema
            .as_ref()
            .and_then(|schema| schema.value.as_ref())
            .and_then(|schema| scope(schema, words).pending)
            .is_some_and(|flag| flag["valueType"] == "System.Boolean");
        // Boolean parsing ignores case. Preserve a valid spelling rather
        // than repairing it to the completer's True/False display labels.
        if boolean
            && !items.iter().any(|item| item.value == typed)
            && let Some(mut alias) = items
                .iter()
                .find(|item| item.value.eq_ignore_ascii_case(typed))
                .cloned()
        {
            alias.value = typed.into();
            items.push(alias);
        }
        items
    }

    pub fn requires_value(&self, words: &[&str], typed: &str, next: Option<&str>) -> Option<bool> {
        let schema = self.schema.borrow();
        let schema = schema.as_ref()?.value.as_ref()?;
        let scope = scope(schema, words);
        let flag = scoped_option(&scope, typed)?;
        if let Some(required) = arity(flag) {
            return Some(required);
        }
        (flag["arity"]["minimum"] == 0 && flag["arity"]["maximum"].as_u64().is_some())
            .then(|| next.is_some_and(|next| self.option_syntax(next).is_none() && next != "--"))
    }

    pub fn complete(
        &self,
        backend: &Backend,
        words: &[&str],
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        line(words, prefix)?;
        self.template_projects(words)?;
        let schema = self.schema(backend, budget)?;
        // Schema discovery can reveal a template-command alias that was not
        // known at preflight. Its selectors require the same policy.
        self.template_projects(words)?;
        let mut wire = self.template_words(words)?;
        // System.CommandLine's completer returns no flags when the preceding
        // optional Boolean has no explicit value. A bare Boolean option means
        // true; spell that implicit value only in the query so it can locate
        // the following option position. The user's script is never expanded.
        if (option_separator(prefix).is_some() || prefix == "-" || prefix == "/")
            && let Some(flag) = schema
                .as_ref()
                .and_then(|schema| scope(schema, words).pending)
            && flag["valueType"] == "System.Boolean"
            && flag["arity"]["minimum"] == 0
            && flag["arity"]["maximum"] == 1
        {
            wire.push("true".into());
        }
        let wire: Vec<_> = wire.iter().map(String::as_str).collect();
        let line = line(&wire, prefix)?;
        let text = self.raw(
            backend,
            &[
                "complete".into(),
                "--position".into(),
                line.encode_utf16().count().to_string(),
                line,
            ],
            budget,
        )?;
        let scope = schema.as_ref().map(|schema| scope(schema, words));
        let mut result: Vec<CompletionItem> = Vec::new();
        for (index, value) in text.lines().filter(|value| !value.is_empty()).enumerate() {
            if value.contains(char::is_control) {
                return Err(CompletionError::Failed(
                    "dotnet returned an invalid completion label".into(),
                ));
            }
            if index >= budget.max_candidates {
                return Err(CompletionError::Failed(
                    "dotnet completion exceeded the candidate limit".into(),
                ));
            }
            if !value.starts_with(prefix) {
                continue;
            }
            let metadata = scope.as_ref().and_then(|scope| {
                option(scope.command, value)
                    .or_else(|| {
                        scope.ancestors.iter().rev().find_map(|parent| {
                            option(parent, value).filter(|item| item["recursive"] == true)
                        })
                    })
                    .or_else(|| command(scope.command, value))
            });
            result.push(CompletionItem {
                value: value.into(),
                takes_value: metadata
                    .filter(|_| value.starts_with(['-', '/']))
                    .and_then(arity),
                description: metadata
                    .and_then(|item| item["description"].as_str())
                    .and_then(super::clean_description),
            });
        }
        // Completion hides some flags and aliases that the SDK's parser
        // explicitly declares. Include that app-owned metadata in option
        // positions, without treating the list as exhaustive (MSBuild flags
        // and dynamically discovered template parameters remain separate).
        if prefix.starts_with(['-', '/'])
            && let Some(scope) = scope.as_ref()
            && scope
                .pending
                .is_none_or(|flag| flag["arity"]["minimum"] == 0)
        {
            for (node, inherited) in std::iter::once((scope.command, false))
                .chain(scope.ancestors.iter().rev().map(|node| (*node, true)))
            {
                if let Some(options) = node["options"].as_object() {
                    for (name, metadata) in options {
                        if inherited && metadata["recursive"] != true {
                            continue;
                        }
                        for name in std::iter::once(name.as_str()).chain(
                            metadata["aliases"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str),
                        ) {
                            if name.starts_with(prefix)
                                && !result.iter().any(|item| item.value == name)
                            {
                                if name.contains(char::is_control)
                                    || result.len() >= budget.max_candidates
                                {
                                    return Err(CompletionError::Failed("dotnet parser options exceeded the candidate limit or contained an invalid label".into()));
                                }
                                result.push(item(name, metadata));
                            }
                        }
                    }
                }
            }
        }
        Ok(result)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::{Discovery, NativeCompletionBackend, discover, tests::Dir};
    use super::*;
    use std::time::Duration;

    const SCHEMA: &str = r#"{
      "name":"dotnet","options":{
        "--help":{"aliases":["-h","/h"],"recursive":true,"arity":{"minimum":0,"maximum":0},"description":"Show help"}
      },"arguments":{"subcommand":{"hidden":true}},"subcommands":{
        "build":{"aliases":["b"],"description":"Build projects","arguments":{"project":{}},"options":{
          "--configuration":{"aliases":["-c"],"arity":{"minimum":1,"maximum":1},"description":"Project configuration"},
          "--self-contained":{"valueType":"System.Boolean","arity":{"minimum":0,"maximum":1}}
        },"subcommands":{}},
        "nuget":{"options":{},"arguments":{},"subcommands":{
          "locals":{"options":{},"arguments":{"folders":{}},"subcommands":{}}
        }},
        "new":{"aliases":["templates"],"options":{
          "--debug:custom-hive":{"hidden":true,"recursive":true,"arity":{"minimum":1,"maximum":1}},
          "--debug:ephemeral-hive":{"hidden":true,"aliases":["--debug:virtual-hive"],"recursive":true,"valueType":"System.Boolean","arity":{"minimum":0,"maximum":1}},
          "--output":{"aliases":["-o"],"arity":{"minimum":1,"maximum":1}},
          "--project":{"arity":{"minimum":1,"maximum":1}}
        },"arguments":{"template":{}},"subcommands":{}}
      }
    }"#;

    const APP: &str = r#"#!/bin/sh
root=$(dirname "$0")
[ "$HTTPS_PROXY" = http://127.0.0.1:9 ] || exit 8
[ "$DOTNET_CLI_TELEMETRY_OPTOUT" = 1 ] || exit 8
[ "$DOTNET_GENERATE_ASPNET_CERTIFICATE" = false ] || exit 8
[ "$DOTNET_CLI_HOME" != "$root" ] || exit 8
printf '<%s>' "$@" >> "$root/calls"
printf '\n' >> "$root/calls"
printf '%s' "$DOTNET_CLI_HOME" > "$root/probe-home"
printf '%s' "$PWD" > "$root/probe-cwd"
case "$1" in
  --version) if [ -f "$root/selected" ]; then cat "$root/selected"; else printf '10.0.401\n'; fi;;
  --info)
    selected=$(cat "$root/selected" 2>/dev/null || printf '10.0.401')
    base=$(cat "$root/selected-path" 2>/dev/null || printf '%s/sdk/%s/' "$root" "$selected")
    printf 'Base Path: %s\n' "$base";;
  --cli-schema) cat "$root/schema";;
  complete)
    [ "$2" = --position ] && [ "$#" = 4 ] || exit 8
    if [ -f "$root/mode" ]; then
      case "$(cat "$root/mode")" in
        failed) exit 2;;
        invalid) printf 'bad\001label\n'; exit 0;;
      esac
    fi
    case "$4" in
      'dotnet build -') printf -- '--configuration\n--self-contained\n--help\n-c\n-h\n/h\n';;
      'dotnet build --configuration ') printf 'Debug\nRelease\n';;
      'dotnet nuget ') printf 'locals\n--help\n';;
      'dotnet nuget locals ') printf 'all\nhttp-cache\n';;
      *) printf 'build\nnuget\n--help\n';;
    esac;;
  *) touch "$root/operation-marker"; exit 9;;
esac
"#;

    fn sdk(dir: &Dir, version: &str, schema: bool) {
        dir.script(
            &format!("sdk/{version}/.version"),
            &format!("commit\n{version}\n"),
        );
        dir.script(
            &format!("sdk/{version}/dotnet.deps.json"),
            &format!(r#"{{"libraries":{{"dotnet.deps.json/{version}":{{}}}}}}"#),
        );
        dir.script(
            &format!("sdk/{version}/dotnet.dll"),
            if schema {
                "CompleteCommand GetCompletions PrintCliSchemaAction"
            } else {
                "CompleteCommand GetCompletions"
            },
        );
    }

    fn fixture(dir: &Dir) -> Backend {
        sdk(dir, "10.0.401", true);
        dir.script("schema", SCHEMA);
        let path = dir.script("dotnet", APP);
        let Discovery::Found(mut backend) = discover("dotnet", &path, &["dotnet:sdk".into()])
        else {
            panic!("SDK not discovered");
        };
        backend.dotnet.as_mut().unwrap().original_home = dir.0.join("fixture-home");
        backend
    }

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(20), Duration::from_secs(5), 20)
    }
    fn values(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.value.as_str()).collect()
    }

    #[test]
    fn line_preserves_literals_empty_arguments_and_utf16_cursor() {
        let query = line(
            &[
                "build",
                "",
                "a b😀",
                "it's",
                "$(touch marker)",
                r"C:\path\file",
            ],
            "",
        )
        .unwrap();
        assert_eq!(
            query,
            "dotnet build \"\" \"a b😀\" it's \"$(touch marker)\" C:\\path\\file "
        );
        assert_eq!(query.encode_utf16().count(), query.chars().count() + 1);
        assert_eq!(line(&[], ""), Ok("dotnet ".into()));
        for value in ["a\"b", "line\nfeed", "nul\0", "@args", "[debug]"] {
            assert!(matches!(
                line(&[value], ""),
                Err(CompletionError::Unsupported(_))
            ));
            assert!(matches!(
                line(&[], value),
                Err(CompletionError::Unsupported(_))
            ));
        }
    }

    #[test]
    fn schema_walk_distinguishes_aliases_values_positionals_and_optional_flags() {
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        assert!(scope(&schema, &["b", "-c"]).pending.is_some());
        assert!(
            scope(&schema, &["build", "--configuration=Debug"])
                .pending
                .is_none()
        );
        assert!(
            scope(&schema, &["build", "--configuration:Debug"])
                .pending
                .is_none()
        );
        assert!(
            scope(&schema, &["build", "--self-contained", "--help"])
                .pending
                .is_none()
        );
        assert!(
            scope(&schema, &["build", "--self-contained", "true", "-c"])
                .pending
                .is_some()
        );
        assert!(scope(&schema, &["build", "--", "--help"]).arguments);
        assert!(scope(&schema, &["build", "project.csproj"]).arguments);
        assert_eq!(
            arity(option(command(&schema, "build").unwrap(), "--self-contained").unwrap()),
            None
        );
    }

    #[test]
    fn installed_sdk_identity_requires_receipt_manifest_and_implementation() {
        let dir = Dir::new("dotnet-identity");
        let path = dir.script("dotnet", APP);
        assert_eq!(installation(&path).map(|_| ()), None);
        sdk(&dir, "10.0.401", true);
        // A malformed sibling must not mask a valid registered SDK.
        dir.script("sdk/broken/.version", "garbage");
        assert!(matches!(
            discover("dotnet", &path, &[]),
            Discovery::NotTrusted(_)
        ));
        assert!(
            matches!(discover("dotnet", &path, &["dotnet:sdk".into()]), Discovery::Found(backend) if backend.flavor == Flavor::Dotnet)
        );
        let alias = dir.0.join("renamed-sdk");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert_eq!(
            super::super::identity::package_manager_name(&alias),
            Some("dotnet")
        );
        assert!(matches!(
            discover("renamed-sdk", &alias, &["dotnet:sdk".into()]),
            Discovery::Found(_)
        ));
        assert!(!dir.0.join("calls").exists());
        dir.script("sdk/10.0.401/dotnet.dll", "Unrelated implementation");
        assert!(installation(&path).is_none());
    }

    #[test]
    fn completion_supplies_nested_words_values_arity_and_descriptions_without_actions() {
        let dir = Dir::new("dotnet-complete");
        let backend = fixture(&dir);
        let mut budget = budget();
        let root = backend.complete(&[], "", &mut budget).unwrap();
        assert_eq!(values(&root), ["build", "nuget", "--help"]);
        assert!(!backend.candidates_are_resources(&[]));
        let options = backend.complete(&["build"], "-", &mut budget).unwrap();
        assert_eq!(options[0].takes_value, Some(true));
        assert_eq!(
            options[0].description.as_deref(),
            Some("Project configuration")
        );
        assert_eq!(options[1].takes_value, None);
        assert_eq!(options[2].takes_value, Some(false));
        assert_eq!(backend.confirms(&[], "b", &mut budget), Some(true));
        assert_eq!(backend.confirms(&["build"], "-c", &mut budget), Some(true));
        assert_eq!(
            values(&backend.complete(&["nuget"], "", &mut budget).unwrap()),
            ["locals", "--help"]
        );
        assert_eq!(
            values(
                &backend
                    .complete_values(&["nuget", "locals"], &mut budget)
                    .unwrap()
            ),
            ["all", "http-cache"]
        );
        let choices = backend
            .complete_values(&["build", "--configuration"], &mut budget)
            .unwrap();
        assert_eq!(values(&choices), ["Debug", "Release"]);
        assert!(backend.candidate_is_resource(&["build", "--configuration"], &choices[0]));
        assert!(backend.capabilities().option_arity);
        assert!(!backend.candidates_are_resources(&["build", "--self-contained"]));
        assert!(
            !backend.capabilities().complete_options
                && !backend.capabilities().complete_subcommands
        );
        assert_eq!(backend.cache_identity(), None);
        let before = budget.spawned();
        backend.complete(&[], "", &mut budget).unwrap();
        assert_eq!(budget.spawned(), before);
        let calls = fs::read_to_string(dir.0.join("calls")).unwrap();
        assert!(
            calls.lines().all(|call| call.starts_with("<--version>")
                || call.starts_with("<--info>")
                || call.starts_with("<--cli-schema>")
                || call.starts_with("<complete><--position>")),
            "{calls}"
        );
        assert!(!dir.0.join("operation-marker").exists());
        let home = PathBuf::from(fs::read_to_string(dir.0.join("probe-home")).unwrap());
        assert!(home.is_dir());
        drop(backend);
        assert!(!home.exists());
    }

    #[test]
    fn legacy_sdk_completion_does_not_guess_schema_support() {
        let dir = Dir::new("dotnet-legacy");
        sdk(&dir, "6.0.100", false);
        let backend = fixture(&dir);
        dir.script("selected", "6.0.100\n");
        backend.complete(&[], "", &mut budget()).unwrap();
        assert!(!backend.capabilities().option_arity && !backend.capabilities().descriptions);
        assert!(backend.candidates_are_resources(&[]));
        assert!(
            !fs::read_to_string(dir.0.join("calls"))
                .unwrap()
                .contains("--cli-schema")
        );
    }

    #[test]
    fn selected_sdk_in_another_root_is_verified_and_revalidated_by_path() {
        let host = Dir::new("dotnet-host-root");
        let selected = Dir::new("dotnet-selected-root");
        sdk(&selected, "10.0.401", true);
        let backend = fixture(&host);
        host.script(
            "selected-path",
            selected.0.join("sdk/10.0.401").to_str().unwrap(),
        );
        let mut budget = budget();
        assert!(backend.complete(&[], "", &mut budget).is_ok());
        let before = budget.spawned();
        selected.script("sdk/10.0.401/dotnet.dll", "Unverified changed assembly");
        assert!(
            matches!(backend.complete(&[], "", &mut budget), Err(CompletionError::Failed(why)) if why.contains("selected SDK changed"))
        );
        assert_eq!(budget.spawned(), before);
        let backend = fixture(&host);
        assert!(
            matches!(backend.complete(&[], "", &mut budget), Err(CompletionError::Unsupported(why)) if why.contains("not verified"))
        );
    }

    #[test]
    fn failed_command_directory_controls_probes_and_workspace_policy() {
        let dir = Dir::new("dotnet-command-directory");
        let mut backend = fixture(&dir);
        backend.set_workspace(&[], Some(&dir.0));
        let mut local_budget = budget();
        assert!(
            matches!(backend.complete(&[], "", &mut local_budget), Err(CompletionError::Unsupported(why)) if why.contains("program from this directory"))
        );
        assert_eq!(local_budget.spawned(), 0);
        let project = dir.0.join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("app.csproj"), "<Project />").unwrap();
        backend.set_workspace(&[], Some(&project));
        let mut budget = budget();
        assert!(
            matches!(backend.complete(&[], "", &mut budget), Err(CompletionError::Unsupported(why)) if why.contains("trusted_workspaces"))
        );
        assert_eq!(budget.spawned(), 0);
        assert!(!dir.0.join("calls").exists());
        backend.set_workspace(&[project.display().to_string()], Some(&project));
        assert!(backend.complete(&[], "", &mut budget).is_ok());
        let probed = PathBuf::from(fs::read_to_string(dir.0.join("probe-cwd")).unwrap());
        assert_eq!(
            fs::canonicalize(probed).unwrap(),
            fs::canonicalize(project).unwrap()
        );
        let before = budget.spawned();
        backend.set_workspace(&[], backend.dotnet.as_ref().unwrap().cwd.clone().as_deref());
        assert!(backend.complete(&[], "", &mut budget).is_err());
        assert_eq!(
            budget.spawned(),
            before,
            "a memoized answer must not bypass changed workspace trust"
        );
    }

    #[test]
    fn unsafe_context_changes_and_unverified_selected_sdks_fail_without_action() {
        let dir = Dir::new("dotnet-refusals");
        let backend = fixture(&dir);
        for context in [vec!["@secret"], vec!["[debug]"], vec!["embedded\"quote"]] {
            assert!(matches!(
                backend.complete(&context, "", &mut budget()),
                Err(CompletionError::Unsupported(_))
            ));
        }
        assert!(!dir.0.join("calls").exists());
        dir.script("selected", "99.0.0\n");
        assert!(
            matches!(backend.complete(&[], "", &mut budget()), Err(CompletionError::Unsupported(why)) if why.contains("not verified"))
        );
        assert!(
            !fs::read_to_string(dir.0.join("calls"))
                .unwrap()
                .contains("<complete>")
        );
        dir.script("sdk/10.0.401/dotnet.deps.json", "changed");
        assert!(
            matches!(backend.complete(&[], "", &mut budget()), Err(CompletionError::Failed(why)) if why.contains("changed"))
        );
    }

    #[test]
    fn bad_wire_answers_and_over_limit_results_are_not_partial_successes() {
        for (mode, expected) in [("failed", "status"), ("invalid", "label"), ("", "limit")] {
            let dir = Dir::new("dotnet-wire");
            let backend = fixture(&dir);
            dir.script("mode", mode);
            let mut budget = budget();
            if mode.is_empty() {
                budget.max_candidates = 1;
            }
            let result = backend.complete(&[], "z", &mut budget);
            assert!(
                matches!(&result, Err(CompletionError::Failed(why)) if why.contains(expected)),
                "{mode}: {result:?}"
            );
        }
        let dir = Dir::new("dotnet-bad-schema");
        let backend = fixture(&dir);
        dir.script("schema", "{\"name\":\"unrelated\"}");
        assert!(matches!(
            backend.complete(&[], "", &mut budget()),
            Err(CompletionError::Failed(_))
        ));
    }

    #[test]
    fn template_hive_snapshot_preserves_installed_state_without_writing_original() {
        let dir = Dir::new("dotnet-template-state");
        dir.script("original/dotnetcli/settings.json", "installed templates");
        dir.script("original/packages/template.nupkg", "package content");
        copy_hive(&dir.0.join("original"), &dir.0.join("snapshot")).unwrap();
        assert_eq!(
            fs::read_to_string(dir.0.join("snapshot/dotnetcli/settings.json")).unwrap(),
            "installed templates"
        );
        fs::write(
            dir.0.join("snapshot/dotnetcli/settings.json"),
            "SDK changed the copy",
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(dir.0.join("original/dotnetcli/settings.json")).unwrap(),
            "installed templates"
        );
        std::os::unix::fs::symlink(dir.0.join("original"), dir.0.join("original/loop")).unwrap();
        assert!(copy_hive(&dir.0.join("original"), &dir.0.join("refused")).is_err());
    }

    #[test]
    fn hidden_namespaced_options_and_aliases_use_the_sdks_schema() {
        let dir = Dir::new("dotnet-hidden-options");
        let backend = fixture(&dir);
        let flags = backend.complete(&["new"], "-", &mut budget()).unwrap();
        assert!(values(&flags).contains(&"--debug:custom-hive"));
        assert!(values(&flags).contains(&"--debug:virtual-hive"));
        assert_eq!(backend.option_separator("--debug:custom-hive"), Some('='));
        assert_eq!(backend.option_name_len("--debug:custom-hive"), Some(19));
        let flag = backend
            .resolve_option(&["new"], "--debug:custom-hive", &mut budget())
            .unwrap();
        assert_eq!(flag.takes_value, Some(true));
        assert_eq!(
            backend.option_requires_value(&["new"], "--debug:virtual-hive", Some("--output")),
            Some(false)
        );
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let scope = scope(&schema, &["new", "--debug:custom-hive", "a hive"]);
        assert!(scope.pending.is_none() && !scope.arguments);
        let choices = vec![
            CompletionItem {
                value: "True".into(),
                takes_value: None,
                description: None,
            },
            CompletionItem {
                value: "False".into(),
                takes_value: None,
                description: None,
            },
        ];
        let boolean = ["build", "--self-contained"];
        assert!(
            values(
                &backend
                    .prepare_value_candidates(&boolean, "tRuE", choices.clone())
                    .unwrap()
            )
            .contains(&"tRuE")
        );
        assert_eq!(
            backend
                .prepare_value_candidates(&boolean, "treu", choices.clone())
                .unwrap(),
            choices
        );
        assert_eq!(
            backend
                .dotnet
                .as_ref()
                .unwrap()
                .template_start(&["templates"]),
            Some(0)
        );
    }

    #[test]
    fn custom_hives_are_copied_relative_to_the_command_and_removed_afterward() {
        let host = Dir::new("dotnet-custom-hive-host");
        let cwd = Dir::new("dotnet-custom-hive-cwd");
        cwd.script(
            "selected hive/dotnetcli/settings.json",
            "selected templates",
        );
        let mut backend = fixture(&host);
        backend.set_workspace(&[], Some(&cwd.0));
        let protocol = backend.dotnet.as_ref().unwrap();
        let words = ["new", "--debug:custom-hive", "selected hive", "template"];
        let wire = protocol.template_words(&words).unwrap();
        let snapshot = PathBuf::from(&wire[2]);
        assert_ne!(snapshot, cwd.0.join("selected hive"));
        assert_eq!(
            fs::read_to_string(snapshot.join("dotnetcli/settings.json")).unwrap(),
            "selected templates"
        );
        fs::write(
            snapshot.join("dotnetcli/settings.json"),
            "completion wrote its cache",
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(cwd.0.join("selected hive/dotnetcli/settings.json")).unwrap(),
            "selected templates"
        );
        assert_eq!(protocol.template_words(&words).unwrap(), wire);
        backend.complete(&[], "", &mut budget()).unwrap();
        let alias = protocol
            .template_words(&["templates", "--debug:custom-hive", "selected hive"])
            .unwrap();
        assert_eq!(alias[0], "templates");
        assert_eq!(alias[2], wire[2]);
        let absent = protocol
            .template_words(&["new", "--debug:custom-hive", "absent hive"])
            .unwrap();
        assert!(Path::new(&absent[2]).is_dir());
        assert!(!cwd.0.join("absent hive").exists());
        for attached in [
            "--debug:custom-hive=selected",
            "--debug:custom-hive:selected",
        ] {
            assert!(matches!(
                protocol.template_words(&["new", attached]),
                Err(CompletionError::Unsupported(_))
            ));
        }
        // The same original can serve both the explicit and default hive.
        let default = protocol
            .snapshot(&cwd.0.join("selected hive"), true)
            .unwrap();
        assert_ne!(default, snapshot);
        assert_eq!(
            fs::read_to_string(default.join("dotnetcli/settings.json")).unwrap(),
            "selected templates"
        );
        drop(backend);
        assert!(!snapshot.exists() && !default.exists());
    }

    #[test]
    fn template_output_and_explicit_project_paths_require_their_own_trust() {
        let host = Dir::new("dotnet-project-selector-host");
        let cwd = Dir::new("dotnet-project-selector-cwd");
        let foreign = Dir::new("dotnet-project-selector-foreign");
        foreign.script("app.csproj", "<Project />");
        foreign.script("arbitrary.xml", "<Project />");
        std::os::unix::fs::symlink(foreign.0.join("arbitrary.xml"), cwd.0.join("linked.xml"))
            .unwrap();
        let mut backend = fixture(&host);
        backend.set_workspace(&[cwd.0.display().to_string()], Some(&cwd.0));
        let output = foreign.0.join("not-created/nested").display().to_string();
        let explicit = foreign.0.join("arbitrary.xml").display().to_string();
        let attached = format!("--output={output}");
        for words in [
            vec!["new", "--output", &output],
            vec!["new", "-o", &output],
            vec!["new", &attached],
            vec!["new", "--project", &explicit],
            vec!["new", "--project", "linked.xml"],
        ] {
            let mut budget = budget();
            assert!(
                matches!(backend.complete(&words, "", &mut budget), Err(CompletionError::Unsupported(why)) if why.contains("trusted_workspaces"))
            );
            assert_eq!(budget.spawned(), 0);
        }
        assert!(!host.0.join("calls").exists());
        backend.set_workspace(&[foreign.0.display().to_string()], Some(&cwd.0));
        assert!(
            backend
                .complete(&["new", "--output", &output], "-", &mut budget())
                .is_ok()
        );
        assert!(!foreign.0.join("not-created").exists());
        backend.set_workspace(&[], Some(&cwd.0));
        let mut alias_budget = budget();
        assert!(
            matches!(backend.complete(&["templates", "--output", &output], "-", &mut alias_budget), Err(CompletionError::Unsupported(why)) if why.contains("trusted_workspaces"))
        );
        assert_eq!(alias_budget.spawned(), 0);
        // Cached SDK/schema answers still cannot bypass a changed policy.
        assert!(
            backend
                .complete(&["new", "--output", &output], "-", &mut budget())
                .is_err()
        );
    }
}
