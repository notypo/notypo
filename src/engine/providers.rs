//! Candidate providers: each knows the valid words for some positions.
//!
//! The engine asks providers about a [`Slot`] (a position that needs a valid
//! word) and ranks what they return. An authoritative answer comes from the
//! system itself (the app's completer, the `$PATH` listing): words outside
//! it are invalid, and other sources may only raise scores within it.

use super::cache::CompletionCache;
use super::native::{self, CompletionError, CompletionItem, Discovery, NativeCompletionBackend};
use super::probe::Budget;
use super::{Source, TokenRole};
use crate::types::Context;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

/// A position in a command that needs a valid word.
#[derive(Clone, Copy, Debug)]
pub struct Slot<'s> {
    pub role: TokenRole,
    /// The program the slot belongs to; empty for the executable itself.
    pub program: &'s str,
    pub path: Option<&'s PathBuf>,
    /// The words between the program and the slot, as corrected so far.
    pub context: &'s [String],
    /// The subcommands among them (`stash` in `git -C x stash pop`).
    pub command_path: &'s [String],
    pub typed: &'s str,
    /// Ask the app itself, not a cached answer.
    pub fresh: bool,
}

/// Valid words one provider knows for a slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vocabulary {
    pub words: Vec<CompletionItem>,
    /// The system itself answered, so a word outside the list is invalid.
    pub authoritative: bool,
    /// Who answered, such as `aws completion`.
    pub via: String,
    pub source: Source,
    /// The answer came from the on-disk cache and may be out of date.
    pub cached: bool,
}

impl Vocabulary {
    pub fn contains(&self, word: &str) -> bool {
        self.words.iter().any(|w| w.value == word)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    Words(Vocabulary),
    /// The provider has nothing to say about this slot.
    NotApplicable,
    /// The provider should know, but failed.
    Failed(String),
}

pub trait CandidateProvider {
    fn source(&self) -> Source;
    fn vocabulary(&mut self, slot: &Slot, budget: &mut Budget) -> Answer;
    /// Capability limits and failures worth reporting.
    fn notes(&mut self) -> Vec<String> {
        Vec::new()
    }
}

/// Program names: `$PATH` executables, shell aliases, and builtins.
pub struct Executables<'c> {
    pub ctx: &'c Context,
}

impl CandidateProvider for Executables<'_> {
    fn source(&self) -> Source {
        Source::Executables
    }

    fn vocabulary(&mut self, slot: &Slot, _: &mut Budget) -> Answer {
        if slot.role != TokenRole::Executable {
            return Answer::NotApplicable;
        }
        let words = self
            .ctx
            .executables()
            .iter()
            .map(String::as_str)
            .chain(self.ctx.shell.get_builtin_commands().iter().copied())
            .map(|w| CompletionItem {
                value: w.to_owned(),
                takes_value: None,
            })
            .collect();
        Answer::Words(Vocabulary {
            words,
            authoritative: true,
            via: "installed executables".into(),
            source: Source::Executables,
            cached: false,
        })
    }
}

/// Completer answers by (program, context words, prefix) within one run,
/// and whether each came from the disk cache.
type Memo =
    HashMap<(String, Vec<String>, String), (Result<Vec<CompletionItem>, CompletionError>, bool)>;

/// The installed app's own completer, for its subcommands and options.
pub struct NativeCompletion<'c> {
    pub trusted: &'c [String],
    /// Value queries may look up resources over the network.
    pub network: bool,
    backends: HashMap<String, Option<Rc<dyn NativeCompletionBackend>>>,
    caches: HashMap<String, Option<CompletionCache>>,
    memo: Memo,
    notes: Vec<String>,
}

impl<'c> NativeCompletion<'c> {
    pub fn new(trusted: &'c [String], network: bool) -> Self {
        NativeCompletion {
            trusted,
            network,
            backends: HashMap::new(),
            caches: HashMap::new(),
            memo: HashMap::new(),
            notes: Vec::new(),
        }
    }

    /// Values the app lists for the option ending `context`. Never cached
    /// on disk: values can be resources that depend on credentials and
    /// account context.
    pub fn values(
        &mut self,
        program: &str,
        path: Option<&PathBuf>,
        context: &[String],
        budget: &mut Budget,
    ) -> Option<Vocabulary> {
        let backend = self.backend(program, path)?;
        if !backend.capabilities().values {
            return None;
        }
        let key = (program.to_owned(), context.to_vec(), "\0values".to_owned());
        let result = match self.memo.get(&key) {
            Some((hit, _)) => hit.clone(),
            None => {
                let words: Vec<&str> = context.iter().map(String::as_str).collect();
                let result = backend.complete_values(&words, budget);
                self.memo.insert(key, (result.clone(), false));
                result
            }
        };
        let words: Vec<CompletionItem> = result
            .ok()?
            .into_iter()
            .filter(|i| !i.is_option())
            .collect();
        if words.is_empty() || words.iter().any(|w| w.value.contains('/')) {
            return None;
        }
        Some(Vocabulary {
            words,
            // Value lists can lag the service (new regions, formats).
            authoritative: false,
            via: format!(
                "{} {}",
                backend.id(),
                if backend.capabilities().resources {
                    "resource lookup"
                } else {
                    "completion"
                }
            ),
            source: Source::NativeCompletion,
            cached: false,
        })
    }

    /// Writes new answers to the disk cache.
    pub fn persist(&mut self) {
        for cache in self.caches.values_mut().flatten() {
            cache.save();
        }
    }

    /// Uses `backend` for `program` instead of discovering one (tests and
    /// embedders).
    pub fn with_backend(mut self, program: &str, backend: Rc<dyn NativeCompletionBackend>) -> Self {
        self.backends.insert(program.to_owned(), Some(backend));
        self
    }

    fn cache(
        &mut self,
        program: &str,
        backend: &dyn NativeCompletionBackend,
    ) -> Option<&mut CompletionCache> {
        self.caches
            .entry(program.to_owned())
            .or_insert_with(|| {
                let identity = backend.cache_identity()?;
                CompletionCache::open(backend.id(), &identity)
            })
            .as_mut()
    }

    fn note(&mut self, note: String) {
        if !self.notes.contains(&note) {
            self.notes.push(note);
        }
    }

    /// The backend for `program`, discovered once per run.
    pub fn backend(
        &mut self,
        program: &str,
        path: Option<&PathBuf>,
    ) -> Option<Rc<dyn NativeCompletionBackend>> {
        if !self.backends.contains_key(program) {
            let found = match path {
                None => None,
                Some(path) => match native::discover(program, path, self.trusted) {
                    Discovery::Found(mut backend) => {
                        backend.network = self.network;
                        self.note(format!(
                            "{program}: native completion via {} ({} protocol); {}",
                            backend.location(),
                            backend.id(),
                            if backend.network {
                                "resource names may be looked up with your credentials"
                            } else {
                                "resource names are not looked up offline"
                            }
                        ));
                        Some(Rc::new(backend) as Rc<dyn NativeCompletionBackend>)
                    }
                    Discovery::None => {
                        self.note(format!("{program}: no native completion found"));
                        None
                    }
                    Discovery::NotTrusted(why) | Discovery::Unavailable(why) => {
                        self.note(format!("{program}: {why}"));
                        None
                    }
                },
            };
            self.backends.insert(program.to_owned(), found);
        }
        self.backends[program].clone()
    }
}

impl CandidateProvider for NativeCompletion<'_> {
    fn source(&self) -> Source {
        Source::NativeCompletion
    }

    fn vocabulary(&mut self, slot: &Slot, budget: &mut Budget) -> Answer {
        if slot.role == TokenRole::Executable {
            return Answer::NotApplicable;
        }
        let Some(backend) = self.backend(slot.program, slot.path) else {
            return Answer::NotApplicable;
        };
        // An empty prefix enumerates the level; a dash prefix its options.
        let options = slot.role == TokenRole::OptionName;
        let short = slot.typed.len() > 1 && !slot.typed.starts_with("--");
        if options && short && !backend.capabilities().short_options {
            return Answer::NotApplicable;
        }
        let prefix = if options {
            backend.capabilities().option_prefix
        } else {
            ""
        };
        let key = (
            slot.program.to_owned(),
            slot.context.to_vec(),
            prefix.to_owned(),
        );
        let disk_key = format!("{}\x1e{prefix}", slot.context.join("\x1f"));
        let (result, cached) = match self.memo.get(&key) {
            Some((hit, cached)) if !(slot.fresh && *cached) => (hit.clone(), *cached),
            _ => {
                let from_disk = (!slot.fresh)
                    .then(|| self.cache(slot.program, &*backend))
                    .flatten()
                    .and_then(|cache| cache.get(&disk_key).map(<[_]>::to_vec));
                match from_disk {
                    Some(items) => (Ok(items), true),
                    None => {
                        let words: Vec<&str> = slot.context.iter().map(String::as_str).collect();
                        let result = backend.complete(&words, prefix, budget);
                        if let Ok(items) = &result
                            && let Some(cache) = self.cache(slot.program, &*backend)
                        {
                            cache.put(disk_key, items.clone());
                        }
                        (result, false)
                    }
                }
            }
        };
        self.memo.insert(key, (result.clone(), cached));
        let via = format!("{} completion", backend.id());
        match result {
            Ok(items) => Answer::Words(Vocabulary {
                words: items
                    .into_iter()
                    .filter(|i| i.is_option() == options)
                    .collect(),
                authoritative: !options || backend.capabilities().complete_options,
                via,
                source: Source::NativeCompletion,
                cached,
            }),
            // The protocol has no data for this position (gcloud's static
            // tree and positionals): nothing here is a subcommand.
            Err(CompletionError::Unsupported(_)) => Answer::Words(Vocabulary {
                words: Vec::new(),
                authoritative: true,
                via,
                source: Source::NativeCompletion,
                cached: false,
            }),
            Err(error) => {
                self.note(format!(
                    "{} completion after `{}`: {error}",
                    slot.program,
                    slot.context.join(" ")
                ));
                Answer::Failed(error.to_string())
            }
        }
    }

    fn notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }
}

/// Replacements printed by the failed command ("did you mean ..."). Not
/// authoritative: the text is untrusted, and only becomes a quoted word.
pub struct ErrorHints {
    pub suggestions: Vec<String>,
}

impl CandidateProvider for ErrorHints {
    fn source(&self) -> Source {
        Source::Stderr
    }

    fn vocabulary(&mut self, slot: &Slot, _: &mut Budget) -> Answer {
        let option = slot.role == TokenRole::OptionName;
        let words: Vec<CompletionItem> = self
            .suggestions
            .iter()
            .filter(|s| s.starts_with('-') == option)
            .map(|s| CompletionItem {
                value: s.clone(),
                takes_value: None,
            })
            .collect();
        if words.is_empty() {
            return Answer::NotApplicable;
        }
        Answer::Words(Vocabulary {
            words,
            authoritative: false,
            via: "error output".into(),
            source: Source::Stderr,
            cached: false,
        })
    }
}

/// Whether a word looks like a subcommand rather than a file or value.
fn command_like(word: &str) -> bool {
    !word.is_empty()
        && word.len() <= 40
        && word.starts_with(|c: char| c.is_ascii_lowercase())
        && word
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// What the user typed before, from the bounded tail of the shell history.
/// History records no exit status, so a word counts as used, not as valid.
#[derive(Debug, Default)]
pub struct History {
    programs: HashMap<String, u32>,
    /// (program, command path) → word → uses.
    slots: HashMap<(String, Vec<String>), HashMap<String, u32>>,
}

impl History {
    /// Indexes `lines`, skipping those `skip` rejects (correction calls).
    pub fn new(lines: &[String], skip: impl Fn(&str) -> bool) -> History {
        let mut history = History::default();
        for line in lines.iter().filter(|l| !skip(l)) {
            for command in super::parser::parse(line).commands {
                let Some(p) = super::diagnosis::effective_program(&command.words) else {
                    continue;
                };
                let Some(program) = command.words[p].literal() else {
                    continue;
                };
                *history.programs.entry(program.to_owned()).or_default() += 1;
                let mut path = Vec::new();
                for word in &command.words[p + 1..] {
                    let Some(word) = word.literal() else {
                        break;
                    };
                    let slot = history
                        .slots
                        .entry((program.to_owned(), path.clone()))
                        .or_default();
                    if word.starts_with('-') && word.len() > 1 {
                        let name = word.split('=').next().unwrap_or(word);
                        *slot.entry(name.to_owned()).or_default() += 1;
                        continue;
                    }
                    if !command_like(word) || path.len() >= 2 {
                        break;
                    }
                    *slot.entry(word.to_owned()).or_default() += 1;
                    path.push(word.to_owned());
                }
            }
        }
        history
    }

    pub fn program_uses(&self, program: &str) -> u32 {
        self.programs.get(program).copied().unwrap_or(0)
    }

    pub fn uses(&self, program: &str, path: &[String], word: &str) -> u32 {
        self.slots
            .get(&(program.to_owned(), path.to_vec()))
            .and_then(|words| words.get(word))
            .copied()
            .unwrap_or(0)
    }
}

impl CandidateProvider for History {
    fn source(&self) -> Source {
        Source::History
    }

    fn vocabulary(&mut self, slot: &Slot, _: &mut Budget) -> Answer {
        let options = slot.role == TokenRole::OptionName;
        let words: Vec<CompletionItem> = match slot.role {
            TokenRole::Executable => Vec::new(),
            _ => self
                .slots
                .get(&(slot.program.to_owned(), slot.command_path.to_vec()))
                .map(|words| {
                    let mut words: Vec<(&String, &u32)> = words
                        .iter()
                        .filter(|(w, _)| w.starts_with('-') == options)
                        .collect();
                    words.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
                    words
                        .into_iter()
                        .map(|(w, _)| CompletionItem {
                            value: w.clone(),
                            takes_value: None,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };
        if words.is_empty() {
            return Answer::NotApplicable;
        }
        Answer::Words(Vocabulary {
            words,
            authoritative: false,
            via: "shell history".into(),
            source: Source::History,
            cached: false,
        })
    }
}

/// Options from the program's installed man page, formatted by the system's
/// `man` (the program itself never runs).
pub struct ManPages {
    man: Option<PathBuf>,
    pages: HashMap<String, Vec<CompletionItem>>,
}

impl ManPages {
    /// Uses `man`, the system's man page formatter, when it is installed.
    pub fn new(man: Option<PathBuf>) -> ManPages {
        ManPages {
            man: man.filter(|p| super::probe::is_trusted_location(p)),
            pages: HashMap::new(),
        }
    }

    fn options(&mut self, program: &str, path: &PathBuf, budget: &mut Budget) -> &[CompletionItem] {
        if !self.pages.contains_key(program) {
            let options = self.read(program, path, budget);
            self.pages.insert(program.to_owned(), options);
        }
        &self.pages[program]
    }

    fn read(&self, program: &str, path: &PathBuf, budget: &mut Budget) -> Vec<CompletionItem> {
        let Some(man) = &self.man else {
            return Vec::new();
        };
        let mut files = vec![path.clone(), man.clone()];
        files.extend(std::fs::canonicalize(path).ok());
        let fingerprint = super::cache::fingerprint(&files, &["man", program]);
        let mut cache = CompletionCache::open("man", &fingerprint);
        if let Some(hit) = cache.as_ref().and_then(|c| c.get("options")) {
            return hit.to_vec();
        }
        let set = |k: &str, v: &str| (k.into(), Some(v.into()));
        let output = super::probe::run(
            &super::probe::Probe {
                program: man,
                args: vec![program.into()],
                env: vec![
                    set("MANPAGER", "cat"),
                    set("PAGER", "cat"),
                    set("MANWIDTH", "160"),
                    ("MAN_KEEP_FORMATTING".into(), None),
                ],
                capture: super::probe::Capture::Stdout,
            },
            budget,
        );
        let options = match output {
            Ok(out) if out.status == Some(0) => super::docs::options(
                &super::docs::strip_overstrike(&String::from_utf8_lossy(&out.data)),
            ),
            _ => Vec::new(),
        };
        if let Some(cache) = cache.as_mut() {
            cache.put("options".into(), options.clone());
            cache.save();
        }
        options
    }
}

impl CandidateProvider for ManPages {
    fn source(&self) -> Source {
        Source::ManPage
    }

    fn vocabulary(&mut self, slot: &Slot, budget: &mut Budget) -> Answer {
        let Some(path) = slot.path.filter(|_| slot.role == TokenRole::OptionName) else {
            return Answer::NotApplicable;
        };
        let words = self.options(slot.program, path, budget).to_vec();
        if words.is_empty() {
            return Answer::NotApplicable;
        }
        Answer::Words(Vocabulary {
            words,
            authoritative: false,
            via: "man page".into(),
            source: Source::ManPage,
            cached: false,
        })
    }
}

/// Options and subcommands from `<program> --help`, for programs the user
/// listed in `trusted_help` (running a program can always have effects).
pub struct HelpText<'c> {
    pub trusted: &'c [String],
    texts: HashMap<String, Option<String>>,
}

impl<'c> HelpText<'c> {
    pub fn new(trusted: &'c [String]) -> Self {
        HelpText {
            trusted,
            texts: HashMap::new(),
        }
    }

    fn text(&mut self, program: &str, path: &PathBuf, budget: &mut Budget) -> Option<&str> {
        if !self.texts.contains_key(program) {
            let text = self.read(program, path, budget);
            self.texts.insert(program.to_owned(), text);
        }
        self.texts[program].as_deref()
    }

    fn read(&self, program: &str, path: &PathBuf, budget: &mut Budget) -> Option<String> {
        let trusted = self.trusted.iter().any(|t| t == "*" || t == program);
        if !trusted || !super::probe::is_trusted_location(path) {
            return None;
        }
        let output = super::probe::run(
            &super::probe::Probe {
                program: path,
                args: vec!["--help".into()],
                env: [
                    "HTTP_PROXY",
                    "HTTPS_PROXY",
                    "ALL_PROXY",
                    "http_proxy",
                    "https_proxy",
                ]
                .map(|k| (k.into(), Some("http://127.0.0.1:9".into())))
                .into_iter()
                .chain([("PAGER".into(), Some("cat".into()))])
                .collect(),
                capture: super::probe::Capture::Combined,
            },
            budget,
        )
        .ok()?;
        Some(super::docs::strip_overstrike(&String::from_utf8_lossy(
            &output.data,
        )))
    }
}

impl CandidateProvider for HelpText<'_> {
    fn source(&self) -> Source {
        Source::Help
    }

    fn vocabulary(&mut self, slot: &Slot, budget: &mut Budget) -> Answer {
        let Some(path) = slot.path else {
            return Answer::NotApplicable;
        };
        let role = slot.role;
        if role == TokenRole::Executable
            || (role == TokenRole::Subcommand && !slot.command_path.is_empty())
        {
            return Answer::NotApplicable;
        }
        let Some(text) = self.text(slot.program, path, budget) else {
            return Answer::NotApplicable;
        };
        let words = if role == TokenRole::OptionName {
            super::docs::options(text)
        } else {
            super::docs::subcommands(text)
        };
        if words.is_empty() {
            return Answer::NotApplicable;
        }
        Answer::Words(Vocabulary {
            words,
            authoritative: false,
            via: "--help output".into(),
            source: Source::Help,
            cached: false,
        })
    }
}

/// Replacements for a path that doesn't exist: each missing component is
/// matched against what its parent directory holds (bounded, UTF-8 names
/// only). Returns the repaired paths, best first, with their scores.
pub fn repair_path(
    typed: &str,
    cwd: &std::path::Path,
    directories_only: bool,
) -> Vec<(String, super::ranking::ScoreBreakdown)> {
    const MAX_ENTRIES: usize = 2000;
    let absolute = typed.starts_with('/');
    let trailing = typed.ends_with('/') && typed.len() > 1;
    let parts: Vec<&str> = typed.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Vec::new();
    }
    let mut base = if absolute {
        PathBuf::from("/")
    } else {
        cwd.to_owned()
    };
    let mut built: Vec<String> = Vec::new();
    let mut alternatives: Vec<(Vec<String>, super::ranking::ScoreBreakdown)> = Vec::new();
    let mut total: Option<super::ranking::ScoreBreakdown> = None;
    for (n, part) in parts.iter().enumerate() {
        let last = n + 1 == parts.len();
        let here = base.join(part);
        let exists = if last && directories_only || !last {
            here.is_dir()
        } else {
            std::fs::symlink_metadata(&here).is_ok()
        };
        if exists || matches!(*part, "." | "..") {
            built.push((*part).to_owned());
            base = here;
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&base) else {
            return Vec::new();
        };
        let names: Vec<String> = entries
            .take(MAX_ENTRIES)
            .flatten()
            .filter(|e| !(directories_only || !last) || e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        let none = |_: &str| 0;
        let ranked =
            super::ranking::rank_tokens(part, names.iter().map(String::as_str), &[], &none, 3);
        let Some((best, score)) = ranked.first() else {
            return Vec::new();
        };
        if last {
            alternatives = ranked
                .iter()
                .map(|(name, s)| {
                    let mut path = built.clone();
                    path.push((*name).to_owned());
                    (path, s.clone())
                })
                .collect();
        } else {
            built.push((*best).to_owned());
            base = base.join(best);
            total = Some(match total {
                Some(t) if t.total <= score.total => t,
                _ => score.clone(),
            });
            continue;
        }
    }
    if alternatives.is_empty() {
        // Only intermediate components were repaired.
        let Some(score) = total else {
            return Vec::new();
        };
        alternatives.push((built, score));
    } else if let Some(t) = total {
        for (_, score) in &mut alternatives {
            if t.total < score.total {
                *score = t.clone();
            }
        }
    }
    alternatives
        .into_iter()
        .map(|(path, score)| {
            let mut text = format!("{}{}", if absolute { "/" } else { "" }, path.join("/"));
            if trailing {
                text.push('/');
            }
            (text, score)
        })
        .collect()
}
