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

/// A position in a command that needs a valid word.
#[derive(Clone, Copy, Debug)]
pub struct Slot<'s> {
    pub role: TokenRole,
    /// The program the slot belongs to; empty for the executable itself.
    pub program: &'s str,
    pub path: Option<&'s PathBuf>,
    /// The words between the program and the slot, as corrected so far.
    pub context: &'s [String],
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
    backends: HashMap<String, Option<native::Backend>>,
    caches: HashMap<String, Option<CompletionCache>>,
    memo: Memo,
    notes: Vec<String>,
}

impl<'c> NativeCompletion<'c> {
    pub fn new(trusted: &'c [String]) -> Self {
        NativeCompletion {
            trusted,
            backends: HashMap::new(),
            caches: HashMap::new(),
            memo: HashMap::new(),
            notes: Vec::new(),
        }
    }

    /// Writes new answers to the disk cache.
    pub fn persist(&mut self) {
        for cache in self.caches.values_mut().flatten() {
            cache.save();
        }
    }

    fn cache(&mut self, program: &str, backend: &native::Backend) -> Option<&mut CompletionCache> {
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
    pub fn backend(&mut self, program: &str, path: Option<&PathBuf>) -> Option<&native::Backend> {
        if !self.backends.contains_key(program) {
            let found = match path {
                None => None,
                Some(path) => match native::discover(program, path, self.trusted) {
                    Discovery::Found(backend) => {
                        self.note(format!(
                            "{program}: native completion via {} ({} protocol); option values and resource names are not queried offline",
                            backend.completer.display(),
                            backend.id()
                        ));
                        Some(backend)
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
        self.backends[program].as_ref()
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
        let Some(backend) = self.backend(slot.program, slot.path).cloned() else {
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
                    .then(|| self.cache(slot.program, &backend))
                    .flatten()
                    .and_then(|cache| cache.get(&disk_key).map(<[_]>::to_vec));
                match from_disk {
                    Some(items) => (Ok(items), true),
                    None => {
                        let words: Vec<&str> = slot.context.iter().map(String::as_str).collect();
                        let result = backend.complete(&words, prefix, budget);
                        if let Ok(items) = &result
                            && let Some(cache) = self.cache(slot.program, &backend)
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
                cached,
            }),
            // The protocol has no data for this position (gcloud's static
            // tree and positionals): nothing here is a subcommand.
            Err(CompletionError::Unsupported(_)) => Answer::Words(Vocabulary {
                words: Vec::new(),
                authoritative: true,
                via,
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
            cached: false,
        })
    }
}
