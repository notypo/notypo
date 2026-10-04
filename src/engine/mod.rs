//! The structured correction pipeline.
//!
//! ```text
//! failure context → lossless parse → effective program → suspicious tokens
//!   → candidates: native completion first, installed executables,
//!     error-output hints → ranking → safety gate → outcome
//! ```
//!
//! The installed app owns its command knowledge. When an app has a native
//! completer, the engine asks it what is valid at each level of the command
//! (an empty prefix enumerates the level) and only proposes words the app
//! reports, so new commands and extensions work without a notypo release.
//! Discovery never reruns the failed command. Legacy rules stay available
//! as a fallback in [`crate::app`].

pub mod cache;
pub mod diagnosis;
pub mod docs;
pub mod native;
pub mod parser;
pub mod probe;
pub mod providers;
pub mod ranking;
pub mod safety;

use self::diagnosis::{OutputDiagnosis, ProblemKind};
use self::parser::{Script, Span};
use self::probe::Budget;
use self::providers::CandidateProvider;
use self::ranking::ScoreBreakdown;
use self::safety::{Decision, SafetyAssessment};
use crate::shells::Shell;
use crate::types::Context;
use std::path::PathBuf;

/// Where a piece of evidence came from. Each source can be disabled with the
/// `disabled_sources` setting, using [`Source::setting_name`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    NativeCompletion,
    Executables,
    Stderr,
    History,
    Filesystem,
    Help,
    ManPage,
    LegacyRule,
}

impl Source {
    pub fn setting_name(self) -> &'static str {
        match self {
            Source::NativeCompletion => "native",
            Source::Executables => "executables",
            Source::Stderr => "stderr",
            Source::History => "history",
            Source::Filesystem => "filesystem",
            Source::Help => "help",
            Source::ManPage => "man",
            Source::LegacyRule => "legacy",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenRole {
    Executable,
    Subcommand,
    OptionName,
    /// An option's value, such as a region or an output format.
    OptionValue,
    Path,
    /// A missing space: `cd..` is `cd ..`, `gitstatus` is `git status`.
    Split,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    pub source: Source,
    pub detail: String,
}

/// A replacement of one word (or an option's name) of the original command.
#[derive(Clone, Debug, PartialEq)]
pub struct TokenEdit {
    pub command: usize,
    pub word: usize,
    pub span: Span,
    pub role: TokenRole,
    pub from: String,
    pub to: String,
    /// Who vouched for `to`, such as `aws completion`.
    pub via: String,
    /// What the app says `to` does, when its completion describes it.
    pub description: Option<String>,
    /// `to` names a resource (an instance, a bucket) found by a networked
    /// lookup, so the edit changes what the command acts on.
    pub resource: bool,
    pub score: ScoreBreakdown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub script: String,
    pub edits: Vec<TokenEdit>,
    pub evidence: Vec<Evidence>,
    /// A ranking signal, not a probability.
    pub score: f64,
    pub safety: SafetyAssessment,
    /// Only structural evidence: the shell never reported the failure.
    pub weak: bool,
    /// A corrected program's own completer accepted the following word.
    pub confirmed: bool,
}

impl Candidate {
    /// One line describing each change, for the selection prompt.
    pub fn reason(&self) -> String {
        self.edits
            .iter()
            .map(|e| match &e.description {
                Some(description) => format!("{} → {} ({}: {description})", e.from, e.to, e.via),
                None => format!("{} → {} ({})", e.from, e.to, e.via),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// What is known about the failed command. Unknown facts stay `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FailureContext {
    pub source: String,
    pub cwd: Option<PathBuf>,
    pub exit_status: Option<i32>,
    /// Exit statuses of the last pipeline's stages, when the shell gave them.
    pub pipe_status: Option<Vec<i32>>,
    pub output: CapturedOutput,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CapturedOutput {
    #[default]
    Unknown,
    /// stdout and stderr interleaved, as a terminal log records them.
    Combined {
        text: String,
        origin: OutputOrigin,
    },
    Separate {
        stdout: String,
        stderr: String,
    },
}

impl CapturedOutput {
    /// The text error diagnostics are read from.
    pub fn diagnostic_text(&self) -> Option<&str> {
        match self {
            CapturedOutput::Unknown => None,
            CapturedOutput::Combined { text, .. } => Some(text),
            CapturedOutput::Separate { stderr, .. } => Some(stderr),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputOrigin {
    ShellLogger,
    InstantModeLog,
    /// The command was rerun because `replay_for_diagnosis` allowed it.
    Replay,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// The first candidate is clearly best.
    Suggestion(Vec<Candidate>),
    /// Candidates are too close (or too weakly supported) to choose alone.
    Ambiguous(Vec<Candidate>),
    /// Every candidate was refused by the safety gate.
    Unsafe(Vec<Candidate>),
    UnsupportedSyntax(String),
    /// Something looks wrong, but nothing valid is close enough.
    InsufficientEvidence(String),
    NoCorrection(String),
}

impl Outcome {
    pub fn candidates(&self) -> &[Candidate] {
        match self {
            Outcome::Suggestion(c) | Outcome::Ambiguous(c) => c,
            _ => &[],
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Outcome::Suggestion(c) => format!("suggestion: {}", c[0].script),
            Outcome::Ambiguous(c) if c.len() == 1 => {
                format!("needs confirmation: {} (weak evidence)", c[0].script)
            }
            Outcome::Ambiguous(c) => format!("{} close candidates; choose one", c.len()),
            Outcome::Unsafe(c) => format!(
                "refused: {}",
                c.first()
                    .map_or(String::new(), |c| c.safety.reasons.join("; "))
            ),
            Outcome::UnsupportedSyntax(why) => format!("unsupported syntax: {why}"),
            Outcome::InsufficientEvidence(why) => format!("insufficient evidence: {why}"),
            Outcome::NoCorrection(why) => format!("no correction: {why}"),
        }
    }
}

#[derive(Debug)]
pub struct Report {
    pub outcome: Outcome,
    /// Tokens found to be wrong, with the evidence against each.
    pub suspicions: Vec<Suspicion>,
    /// Capability limits, discovery results, and probe failures.
    pub notes: Vec<String>,
    pub probes: usize,
}

/// Reads the status the shell function captured before it ran anything.
pub fn status_from_env() -> (Option<i32>, Option<Vec<i32>>) {
    let status = std::env::var("NOTYPO_EXIT_STATUS")
        .ok()
        .and_then(|s| s.trim().parse().ok());
    let pipe: Option<Vec<i32>> = std::env::var("NOTYPO_PIPESTATUS").ok().and_then(|s| {
        s.split_whitespace()
            .map(|n| n.parse().ok())
            .collect::<Option<Vec<i32>>>()
            .filter(|v| !v.is_empty())
    });
    (status, pipe)
}

/// Runs the pipeline for one failed command.
pub fn correct(failure: &FailureContext, ctx: &Context) -> Report {
    correct_with_backends(failure, ctx, Vec::new())
}

/// [`correct`], answering native completion for the named programs from
/// the given backends instead of discovering them (tests, corpora, and
/// embedders with their own protocol bridges).
pub fn correct_with_backends(
    failure: &FailureContext,
    ctx: &Context,
    backends: Vec<(String, std::rc::Rc<dyn native::NativeCompletionBackend>)>,
) -> Report {
    let mut run = Run::new(failure, ctx);
    if let Some(native) = run.native.take() {
        run.native = Some(backends.into_iter().fold(native, |n, (program, backend)| {
            n.with_backend(&program, backend)
        }));
    }
    let outcome = run.execute();
    if let Some(native) = run.native.as_mut() {
        native.persist();
    }
    Report {
        outcome,
        suspicions: run.suspicions,
        probes: run.budget.spawned(),
        notes: run.notes,
    }
}

/// Rechecks, right before the shell runs a chosen candidate, the facts its
/// edits relied on: programs it introduced still resolve to executable
/// files, and paths it introduced still exist (as directories for `cd`).
/// This narrows, but can't close, the window between discovery and
/// execution; nothing here proves the command harmless.
pub fn revalidate(
    candidate: &Candidate,
    failure: &FailureContext,
    ctx: &Context,
) -> Result<(), String> {
    let dialect = parser::Dialect::for_shell(ctx.shell).unwrap_or_default();
    let original = parser::parse_with_dialect(&failure.source, dialect);
    let cwd = failure.cwd.clone().or_else(|| std::env::current_dir().ok());
    for edit in &candidate.edits {
        let words = original
            .commands
            .get(edit.command)
            .map_or(&[][..], |c| &c.words[..]);
        let program = diagnosis::effective_program(words);
        let introduced = match edit.role {
            TokenRole::Executable => Some(edit.to.as_str()),
            // `gitstatus` → `git status` introduces `git`.
            TokenRole::Split if program == Some(edit.word) => edit.to.split(' ').next(),
            _ => None,
        };
        if let Some(name) = introduced
            && !ctx.shell.get_builtin_commands().contains(&name)
            && !ctx.shell.get_aliases().contains_key(name)
            && !ctx.shell.get_functions().contains(name)
        {
            let installed = if name.contains('/') {
                cwd.as_ref()
                    .is_some_and(|cwd| crate::utils::is_executable_file(&cwd.join(name)))
            } else {
                // `which` is memoized for the run; stat the file it found again.
                ctx.which(name)
                    .is_some_and(|path| crate::utils::is_executable_file(&path))
            };
            if !installed {
                return Err(format!("{name} is no longer installed"));
            }
        }
        if edit.role == TokenRole::Path
            && let Some(cwd) = &cwd
        {
            let path = cwd.join(&edit.to);
            let directory = program
                .and_then(|p| words[p].literal())
                .is_some_and(|name| matches!(name, "cd" | "pushd"));
            let present = if directory {
                path.is_dir()
            } else {
                std::fs::symlink_metadata(&path).is_ok()
            };
            if !present {
                return Err(format!("{} no longer exists", edit.to));
            }
            // `./scirpt.sh` → `./script.sh` names the program itself.
            if program == Some(edit.word) && !crate::utils::is_executable_file(&path) {
                return Err(format!("{} is no longer executable", edit.to));
            }
        }
    }
    Ok(())
}

/// How many competing interpretations are followed.
const BEAM: usize = 3;
/// Alternatives this far behind the best replacement are not followed.
const BRANCH_WINDOW: f64 = 0.2;
/// Discount for an interpretation that left a later invalid token unfixed.
const UNRESOLVED_PENALTY: f64 = 0.8;
/// Discount per later occurrence when a hint names a repeated token.
const LATER_OCCURRENCE_PENALTY: f64 = 0.03;

/// How strongly each kind of evidence says a token is wrong. Independent
/// pieces combine as `1 - Π(1 - p)`.
const APP_REJECTS: f64 = 0.9;
/// The app's list is known to be partial (git's option helper).
const APP_OMITS: f64 = 0.5;
const NOT_ON_PATH: f64 = 0.6;
const EXIT_127: f64 = 0.8;
const OUTPUT_NAMES_IT: f64 = 0.85;
const PATH_MISSING: f64 = 0.6;
/// Ranking bonus for a corrected program that accepts the rest of the line.
const CONTEXT_FITS: f64 = 0.1;
/// A missing space ranks just below an equally close spelling fix.
const SPLIT_DISCOUNT: f64 = 0.97;
/// ...and further below when nothing confirms the word it splits off.
const UNCONFIRMED_SPLIT: f64 = 0.9;
const COMMAND_FAILED: f64 = 0.5;
/// Weaker suspicions can't be repaired without the user confirming.
const STRONG_SUSPICION: f64 = 0.75;

fn combine(evidence: &[f64]) -> f64 {
    1.0 - evidence.iter().map(|p| 1.0 - p).product::<f64>()
}

/// A token the engine believes is wrong, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Suspicion {
    pub command: usize,
    pub word: usize,
    pub role: TokenRole,
    pub token: String,
    /// Combined strength of the evidence, in `[0, 1]`.
    pub score: f64,
    pub evidence: Vec<Evidence>,
}

struct Run<'a> {
    failure: &'a FailureContext,
    ctx: &'a Context,
    budget: Budget,
    diagnosis: OutputDiagnosis,
    notes: Vec<String>,
    native: Option<providers::NativeCompletion<'a>>,
    executables: Option<providers::Executables<'a>>,
    hints: Option<providers::ErrorHints>,
    history: Option<providers::History>,
    man: Option<providers::ManPages>,
    help: Option<providers::HelpText<'a>>,
    filesystem: bool,
    suspicions: Vec<Suspicion>,
    /// Some words could not be checked (a failed probe, an exhausted budget).
    unchecked: bool,
    /// Words some source confirmed or rejected.
    checked: usize,
}

/// One interpretation of a command being repaired.
#[derive(Clone, Debug)]
struct State {
    program: String,
    path: Option<PathBuf>,
    /// The words after the program, as corrected so far.
    context: Vec<String>,
    /// The subcommands among them.
    command_path: Vec<String>,
    /// A corrected program's completer accepted the word after it.
    confirmed: bool,
    edits: Vec<TokenEdit>,
    score: f64,
    evidence: Vec<Evidence>,
    weak: bool,
}

impl State {
    fn new(program: &str, path: Option<PathBuf>) -> State {
        State {
            program: program.to_owned(),
            path,
            context: Vec::new(),
            command_path: Vec::new(),
            confirmed: false,
            edits: Vec::new(),
            score: 1.0,
            evidence: Vec::new(),
            weak: false,
        }
    }
}

enum Resolution {
    Executable(PathBuf),
    /// An alias, builtin, or function: the shell knows it.
    ShellDefined,
    ExplicitPath,
    Missing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    /// The app may still expect a subcommand or group here.
    Commands,
    /// Only arguments follow; words are no longer checked.
    Arguments,
}

/// How a word missing from a vocabulary was handled.
enum Branching {
    /// A partial list doesn't hold anything close: no reason to doubt it.
    NotSuspicious,
    /// The word is wrong, but nothing listed is close.
    Unresolved,
    /// Interpretations with a close replacement, and whether it takes a value.
    Branches(Vec<(State, Option<bool>)>),
}

/// What the app said about one word of the walk.
enum Check {
    Valid {
        takes_value: Option<bool>,
        /// The app itself (not documentation or history) listed the word.
        confirmed: bool,
    },
    Invalid(providers::Vocabulary),
    /// Nothing here is a subcommand: arguments follow.
    Arguments,
    Unknown,
}

impl<'a> Run<'a> {
    fn new(failure: &'a FailureContext, ctx: &'a Context) -> Self {
        let settings = &ctx.settings;
        let enabled = |source: Source| settings.is_source_enabled(source.setting_name());
        let mut notes = Vec::new();
        for source in [
            Source::NativeCompletion,
            Source::Executables,
            Source::Stderr,
            Source::History,
            Source::Filesystem,
            Source::ManPage,
        ] {
            if !enabled(source) {
                notes.push(format!(
                    "the {} source is disabled by settings",
                    source.setting_name()
                ));
            }
        }
        let diagnosis = match failure.output.diagnostic_text() {
            Some(text) if enabled(Source::Stderr) => diagnosis::diagnose_output(text),
            _ => OutputDiagnosis::default(),
        };
        Run {
            failure,
            ctx,
            budget: Budget::for_timeout(settings.probe_timeout),
            notes,
            native: enabled(Source::NativeCompletion).then(|| {
                providers::NativeCompletion::new(
                    &settings.trusted_completers,
                    settings.network_completion,
                )
                .with_shell(ctx.shell)
            }),
            executables: enabled(Source::Executables).then_some(providers::Executables { ctx }),
            hints: enabled(Source::Stderr).then(|| providers::ErrorHints {
                suggestions: diagnosis.suggestions.clone(),
            }),
            history: enabled(Source::History).then(|| {
                let own = [ctx.alias.as_str(), "fuck", "typo", "notypo", "thefuck"];
                providers::History::with_dialect(
                    ctx.recent_history(),
                    parser::Dialect::for_shell(ctx.shell).unwrap_or_default(),
                    |line| {
                        line == failure.source
                            || line
                                .split_whitespace()
                                .next()
                                .is_some_and(|first| own.contains(&first))
                    },
                )
            }),
            man: enabled(Source::ManPage).then(|| providers::ManPages::new(ctx.which("man"))),
            help: (enabled(Source::Help) && !settings.trusted_help.is_empty())
                .then(|| providers::HelpText::new(&settings.trusted_help)),
            filesystem: enabled(Source::Filesystem),
            diagnosis,
            suspicions: Vec::new(),
            unchecked: false,
            checked: 0,
        }
    }

    fn note(&mut self, note: String) {
        if !self.notes.contains(&note) {
            self.notes.push(note);
        }
    }

    fn collect_notes(&mut self) {
        let mut notes = self.native.as_mut().map(|n| n.notes()).unwrap_or_default();
        if let Some(executables) = self.executables.as_mut() {
            notes.extend(executables.notes());
        }
        if let Some(help) = self.help.as_mut() {
            notes.extend(help.notes());
        }
        for note in notes {
            self.note(note);
        }
    }

    fn suspect(
        &mut self,
        index: usize,
        word: usize,
        role: TokenRole,
        token: &str,
        evidence: Vec<(f64, Evidence)>,
    ) -> f64 {
        let score = combine(&evidence.iter().map(|(p, _)| *p).collect::<Vec<_>>());
        let evidence = evidence.into_iter().map(|(_, e)| e).collect();
        match self
            .suspicions
            .iter_mut()
            .find(|s| s.command == index && s.word == word && s.token == token)
        {
            Some(existing) => existing.score = existing.score.max(score),
            None => self.suspicions.push(Suspicion {
                command: index,
                word,
                role,
                token: token.to_owned(),
                score,
                evidence,
            }),
        }
        score
    }

    fn execute(&mut self) -> Outcome {
        let shell = self.ctx.shell;
        let Some(dialect) = parser::Dialect::for_shell(shell) else {
            return Outcome::UnsupportedSyntax(format!(
                "{} command lines are not parsed yet",
                shell.friendly_name()
            ));
        };
        let script = parser::parse_with_dialect(&self.failure.source, dialect);
        if let Some(diagnostic) = script.diagnostics.first() {
            return Outcome::UnsupportedSyntax(format!(
                "{:?} at `{}`",
                diagnostic.kind,
                diagnostic.span.of(&script.source)
            ));
        }

        let mut states = Vec::new();
        for index in self.failed_commands(&script) {
            states.extend(self.repair(&script, index));
        }
        self.collect_notes();
        let mut candidates: Vec<Candidate> = Vec::new();
        for state in states {
            let Some(candidate) = self.build(&script, state) else {
                continue;
            };
            match candidates.iter_mut().find(|c| c.script == candidate.script) {
                Some(existing) => {
                    for evidence in candidate.evidence {
                        if !existing.evidence.contains(&evidence) {
                            existing.evidence.push(evidence);
                        }
                    }
                    if candidate.score > existing.score {
                        existing.score = candidate.score;
                        existing.edits = candidate.edits;
                        existing.weak = candidate.weak;
                        existing.confirmed = candidate.confirmed;
                    }
                }
                None => candidates.push(candidate),
            }
        }
        candidates.sort_by(|a, b| b.score.total_cmp(&a.score));

        if candidates.is_empty() {
            return if !self.suspicions.is_empty() {
                Outcome::InsufficientEvidence(
                    "a token looks wrong, but no valid word is close to it".into(),
                )
            } else if self.unchecked {
                Outcome::InsufficientEvidence(
                    "some words could not be checked against the app".into(),
                )
            } else if let Some(failure) = self.diagnosis.operational {
                Outcome::NoCorrection(format!(
                    "the output points to {}, which a spelling change can't fix",
                    failure.describe()
                ))
            } else if self.checked == 0 {
                Outcome::NoCorrection("no source could check these words".into())
            } else {
                Outcome::NoCorrection("every checked token is valid".into())
            };
        }
        let (refused, mut allowed): (Vec<_>, Vec<_>) = candidates
            .into_iter()
            .map(|mut c| {
                c.safety = safety::assess(&script, &c.script, &[]);
                for edit in c.edits.iter().filter(|e| e.resource) {
                    c.safety.flag(
                        Decision::Confirm,
                        format!(
                            "acts on the resource `{}` instead of `{}`",
                            edit.to, edit.from
                        ),
                    );
                }
                c
            })
            .partition(|c| c.safety.decision == Decision::Refuse);
        for candidate in &refused {
            self.note(format!(
                "refused {}: {}",
                candidate.script,
                candidate.safety.reasons.join("; ")
            ));
        }
        if allowed.is_empty() {
            return Outcome::Unsafe(refused);
        }
        allowed.truncate(BEAM * 2);
        let scores: Vec<f64> = allowed.iter().map(|c| c.score).collect();
        let tied = allowed
            .get(1)
            .is_some_and(|second| equally_supported(&allowed[0], second));
        if ranking::is_decisive(&scores) && !allowed[0].weak && !tied {
            Outcome::Suggestion(allowed)
        } else {
            Outcome::Ambiguous(allowed)
        }
    }

    /// The commands worth repairing. A single pipeline whose stage statuses
    /// single out one failure narrows it; otherwise every command is checked.
    fn failed_commands(&self, script: &Script) -> Vec<usize> {
        let all: Vec<usize> = (0..script.commands.len()).collect();
        // Stage statuses only map onto simple commands outside compounds.
        let one_pipeline = script.compounds.is_empty()
            && script
                .commands
                .iter()
                .skip(1)
                .all(|c| c.connector.is_some_and(parser::Connector::is_pipe));
        match &self.failure.pipe_status {
            Some(status) if one_pipeline && status.len() == all.len() => {
                let failed: Vec<usize> = all.iter().copied().filter(|&i| status[i] != 0).collect();
                if failed.len() == 1 { failed } else { all }
            }
            _ => all,
        }
    }

    /// The shell's exit status says the program wasn't found: 127 from
    /// POSIX shells and fish. tcsh exits 1 instead; it has no functions, and
    /// its aliases and builtins are known, so a failed name that resolves to
    /// none of them was missing too.
    fn reported_missing(&self) -> bool {
        match self.ctx.shell {
            Shell::Tcsh => self.failure.exit_status.is_some_and(|s| s != 0),
            _ => self.failure.exit_status == Some(127),
        }
    }

    fn resolve(&self, name: &str) -> Resolution {
        if name.contains('/') {
            return Resolution::ExplicitPath;
        }
        if self.ctx.shell.get_aliases().contains_key(name)
            || self.ctx.shell.get_functions().contains(name)
            || self.ctx.shell.get_builtin_commands().contains(&name)
        {
            return Resolution::ShellDefined;
        }
        match self.ctx.which(name) {
            Some(path) => Resolution::Executable(path),
            // The shell ran something under this name (a function, say).
            None if self.failure.exit_status.is_some() && !self.reported_missing() => {
                Resolution::ShellDefined
            }
            None => Resolution::Missing,
        }
    }

    fn repair(&mut self, script: &Script, index: usize) -> Vec<State> {
        let words = &script.commands[index].words;
        let Some((p, runner)) = diagnosis::effective_program_via(words) else {
            return Vec::new();
        };
        let Some(name) = words[p].literal() else {
            return Vec::new();
        };
        if let Some(ecosystem) = runner {
            return match self.resolve_package(name, ecosystem) {
                Some(start) => self.walk(script, index, p, start),
                None => Vec::new(),
            }
            .into_iter()
            .filter(|s| !s.edits.is_empty())
            .collect();
        }
        let starts = match self.resolve(name) {
            Resolution::Executable(path) => vec![State::new(name, Some(path))],
            Resolution::ShellDefined => {
                return if matches!(name, "cd" | "pushd") {
                    self.path_candidates(script, index, p, Some(p + 1))
                } else {
                    Vec::new()
                };
            }
            Resolution::ExplicitPath => return self.path_candidates(script, index, p, Some(p)),
            Resolution::Missing => {
                let starts = self.executable_candidates(script, index, p, name);
                let mut finished = Vec::new();
                for start in starts {
                    finished.extend(self.walk(script, index, p, start));
                }
                finished.retain(|s| !s.edits.is_empty());
                return finished;
            }
        };
        let mut finished = Vec::new();
        for start in starts {
            let native = self.has_native(&start);
            let original = start.edits.is_empty();
            finished.extend(self.walk(script, index, p, start.clone()));
            if !native && original {
                finished.extend(self.hint_candidates(script, index, p, start));
            }
        }
        finished.extend(self.path_candidates(script, index, p, None));
        finished.retain(|s| !s.edits.is_empty());
        finished
    }

    /// The installed program a package runner would run, without fetching
    /// anything: on `$PATH`, or (for Node) in a `node_modules/.bin` above
    /// the working directory, which is used but never probed.
    fn resolve_package(&mut self, spec: &str, ecosystem: diagnosis::Ecosystem) -> Option<State> {
        if spec.starts_with('@') || spec.contains('/') {
            self.note(format!(
                "{spec}: scoped or path packages are not resolved to a program"
            ));
            return None;
        }
        let name = spec.split('@').next().unwrap_or(spec);
        if let Some(path) = self.ctx.which(name) {
            return Some(State::new(name, Some(path)));
        }
        if ecosystem == diagnosis::Ecosystem::Node
            && let Some(cwd) = self
                .failure
                .cwd
                .clone()
                .or_else(|| std::env::current_dir().ok())
            && cwd
                .ancestors()
                .take(16)
                .any(|dir| dir.join("node_modules/.bin").join(name).is_file())
        {
            self.note(format!(
                "{name}: a local package binary; its completion is not probed automatically"
            ));
            return Some(State::new(name, None));
        }
        self.note(format!(
            "{name} is not installed; the package runner would fetch it, so its name is not checked"
        ));
        None
    }

    fn has_native(&mut self, state: &State) -> bool {
        self.native
            .as_mut()
            .is_some_and(|n| n.backend(&state.program, state.path.as_ref()).is_some())
    }

    /// Repairs words naming paths that don't exist: the target of `cd`
    /// (`only`), an explicit program path (`only`), or, with `only` unset,
    /// paths the output reports missing and, after a known failure, other
    /// words containing `/`.
    fn path_candidates(
        &mut self,
        script: &Script,
        index: usize,
        p: usize,
        only: Option<usize>,
    ) -> Vec<State> {
        if !self.filesystem {
            return Vec::new();
        }
        let Some(cwd) = self
            .failure
            .cwd
            .clone()
            .or_else(|| std::env::current_dir().ok())
        else {
            return Vec::new();
        };
        let words = &script.commands[index].words;
        let program = words[p].literal().unwrap_or_default();
        let cd = matches!(program, "cd" | "pushd") && only == Some(p + 1);
        if cd && std::env::var_os("CDPATH").is_some_and(|v| !v.is_empty()) {
            self.note("cd targets are not repaired while CDPATH is set".into());
            return Vec::new();
        }
        let failed = self.failure.exit_status.is_some_and(|s| s != 0)
            || self.failure.output.diagnostic_text().is_some();
        let targets: Vec<usize> = match only {
            Some(i) if cd => words
                .iter()
                .enumerate()
                .skip(i)
                .find(|(_, w)| w.literal().is_some_and(|t| !t.starts_with('-')))
                .map(|(i, _)| i)
                .into_iter()
                .collect(),
            Some(i) => vec![i],
            None => (p + 1..words.len())
                .filter(|&i| {
                    words[i].literal().is_some_and(|t| {
                        !t.starts_with('-')
                            && !t.contains("://")
                            && (self.diagnosis.mentions(ProblemKind::MissingPath, t)
                                || failed && t.contains('/') && !t.contains('='))
                    })
                })
                .collect(),
        };
        let mut states = Vec::new();
        for i in targets {
            let Some(typed) = words[i].literal() else {
                continue;
            };
            let here = cwd.join(typed);
            let exists = if cd {
                here.is_dir()
            } else {
                std::fs::symlink_metadata(&here).is_ok()
            };
            if exists {
                continue;
            }
            let mut evidence = vec![(
                PATH_MISSING,
                Evidence {
                    source: Source::Filesystem,
                    detail: format!("`{typed}` does not exist"),
                },
            )];
            if self.diagnosis.mentions(ProblemKind::MissingPath, typed) {
                evidence.push((
                    OUTPUT_NAMES_IT,
                    Evidence {
                        source: Source::Stderr,
                        detail: format!("the output says `{typed}` is missing"),
                    },
                ));
            } else if self.failure.exit_status.is_some_and(|s| s != 0) {
                evidence.push((
                    COMMAND_FAILED,
                    Evidence {
                        source: Source::Filesystem,
                        detail: "the command failed".into(),
                    },
                ));
            }
            let suspicion = self.suspect(index, i, TokenRole::Path, typed, evidence.clone());
            for (to, score) in providers::repair_path(typed, &cwd, cd) {
                let mut state = State::new(program, None);
                state.weak = suspicion < STRONG_SUSPICION;
                state.score = score.total;
                state
                    .evidence
                    .extend(evidence.iter().map(|(_, e)| e.clone()));
                state.edits.push(TokenEdit {
                    command: index,
                    word: i,
                    span: words[i].span,
                    role: TokenRole::Path,
                    from: typed.to_owned(),
                    to,
                    via: "filesystem".into(),
                    description: None,
                    resource: false,
                    score,
                });
                states.push(state);
            }
        }
        states
    }

    fn executable_candidates(
        &mut self,
        script: &Script,
        index: usize,
        p: usize,
        name: &str,
    ) -> Vec<State> {
        let mut evidence = vec![(
            NOT_ON_PATH,
            Evidence {
                source: Source::Executables,
                detail: format!("`{name}` is not an installed executable, alias, or builtin"),
            },
        )];
        if self.reported_missing() {
            let status = self.failure.exit_status.unwrap_or_default();
            evidence.push((
                EXIT_127,
                Evidence {
                    source: Source::Executables,
                    detail: format!("the shell exited with {status} (command not found)"),
                },
            ));
        }
        if self.diagnosis.mentions(ProblemKind::CommandNotFound, name) {
            evidence.push((
                OUTPUT_NAMES_IT,
                Evidence {
                    source: Source::Stderr,
                    detail: format!("the output says `{name}` was not found"),
                },
            ));
        }
        let suspicion = self.suspect(index, p, TokenRole::Executable, name, evidence.clone());
        let Some(provider) = self.executables.as_mut() else {
            return Vec::new();
        };
        let slot = providers::Slot {
            role: TokenRole::Executable,
            program: "",
            path: None,
            context: &[],
            command_path: &[],
            typed: name,
            fresh: false,
        };
        let providers::Answer::Words(vocabulary) = provider.vocabulary(&slot, &mut self.budget)
        else {
            return Vec::new();
        };
        let history = self.history.as_ref();
        let uses = |word: &str| history.map_or(0, |h| h.program_uses(word));
        let ranked = within_window(ranking::rank_tokens(
            name,
            vocabulary.words.iter().map(|w| w.value.as_str()),
            &self.diagnosis.suggestions,
            &uses,
            BEAM,
        ));
        if ranked.is_empty() {
            self.note(format!("no installed executable is close to `{name}`"));
        }
        let word = &script.commands[index].words[p];
        let splits =
            self.split_candidates(index, p, word.span, name, &vocabulary, &evidence, suspicion);
        ranked
            .into_iter()
            .map(|(to, score)| {
                let mut state = State::new(to, self.ctx.which(to));
                state.weak = suspicion < STRONG_SUSPICION;
                state.score = score.total;
                state
                    .evidence
                    .extend(evidence.iter().map(|(_, e)| e.clone()));
                state.edits.push(TokenEdit {
                    command: index,
                    word: p,
                    span: word.span,
                    role: TokenRole::Executable,
                    from: name.to_owned(),
                    to: to.to_owned(),
                    via: vocabulary.via.clone(),
                    description: None,
                    resource: false,
                    score,
                });
                state
            })
            .chain(splits)
            .collect()
    }

    /// `name` as a known program followed by the rest of the word: `cd..`
    /// is `cd ..`, `gitstatus` is `git status`. The longest programs that
    /// fit are tried, and the program's completer judges the rest.
    #[allow(clippy::too_many_arguments)]
    fn split_candidates(
        &mut self,
        index: usize,
        p: usize,
        span: Span,
        name: &str,
        vocabulary: &providers::Vocabulary,
        evidence: &[(f64, Evidence)],
        suspicion: f64,
    ) -> Vec<State> {
        let mut heads: Vec<usize> = name
            .char_indices()
            .map(|(i, _)| i)
            .filter(|&i| i >= 2 && vocabulary.contains(&name[..i]))
            .collect();
        heads.reverse();
        let mut states = Vec::new();
        for i in heads.into_iter().take(2) {
            let (head, tail) = name.split_at(i);
            // `cd..`, `cd/tmp`, `cd~`, `ls-la`: punctuation after a program.
            let wordy = !tail.starts_with(['.', '/', '~', '-']);
            // `lsx` is likelier `ls` with a stray key than `ls x`.
            if wordy && tail.chars().count() < 2 {
                continue;
            }
            let joined = format!("{head} {tail}");
            let uses = self.history.as_ref().map_or(0, |h| h.program_uses(head));
            let score = ranking::score_token(name, &joined, false, uses);
            let mut state = State::new(head, self.ctx.which(head));
            state.weak = suspicion < STRONG_SUSPICION;
            // An equally close spelling fix is the likelier mistake.
            state.score = score.total * SPLIT_DISCOUNT;
            state
                .evidence
                .extend(evidence.iter().map(|(_, e)| e.clone()));
            state.evidence.push(Evidence {
                source: Source::Executables,
                detail: format!("`{head}` is installed and `{name}` starts with it"),
            });
            if !tail.starts_with('-') {
                match self.check(&state, TokenRole::Subcommand, tail) {
                    Check::Valid {
                        confirmed: true, ..
                    } => {
                        state.score = (state.score + CONTEXT_FITS).min(1.0);
                        state.confirmed = true;
                        state.command_path.push(tail.to_owned());
                        state.evidence.push(Evidence {
                            source: Source::NativeCompletion,
                            detail: format!("{head} completion lists `{tail}`"),
                        });
                    }
                    Check::Invalid(v) if v.authoritative => state.score *= UNRESOLVED_PENALTY,
                    // Nothing vouches for a word after the program.
                    _ if wordy => state.score *= UNCONFIRMED_SPLIT,
                    _ => {}
                }
            }
            state.context.push(tail.to_owned());
            state.edits.push(TokenEdit {
                command: index,
                word: p,
                span,
                role: TokenRole::Split,
                from: name.to_owned(),
                to: joined,
                via: vocabulary.via.clone(),
                description: None,
                resource: false,
                score,
            });
            states.push(state);
        }
        states
    }

    /// Asks the app's completer about one word of the walk, or, for apps
    /// without one, what documentation and history know.
    fn check(&mut self, state: &State, role: TokenRole, typed: &str) -> Check {
        let mut check = if self.has_native(state) {
            self.native_check(state, role, typed)
        } else {
            self.fallback_check(state, role, typed)
        };
        if role == TokenRole::OptionName
            && let Check::Invalid(vocabulary) = &check
            && let Some(resolved) = short_cluster(typed, vocabulary)
        {
            check = resolved;
        }
        if matches!(check, Check::Valid { .. } | Check::Invalid(_)) {
            self.checked += 1;
        }
        check
    }

    fn native_check(&mut self, state: &State, role: TokenRole, typed: &str) -> Check {
        let Some(native) = self.native.as_mut() else {
            return Check::Unknown;
        };
        let mut slot = providers::Slot {
            role,
            program: &state.program,
            path: state.path.as_ref(),
            context: &state.context,
            command_path: &state.command_path,
            typed,
            fresh: false,
        };
        let mut answer = native.vocabulary(&slot, &mut self.budget);
        // A cached list may predate an upgrade: confirm with the app before
        // calling a word invalid or a level free of subcommands.
        if let providers::Answer::Words(vocabulary) = &answer
            && vocabulary.cached
            && (vocabulary.words.is_empty() || !vocabulary.contains(typed))
        {
            slot.fresh = true;
            answer = native.vocabulary(&slot, &mut self.budget);
        }
        match answer {
            providers::Answer::Words(vocabulary) => {
                if let Some(word) = vocabulary.words.iter().find(|w| w.value == typed) {
                    Check::Valid {
                        takes_value: word.takes_value,
                        confirmed: vocabulary.authoritative,
                    }
                } else if vocabulary.words.is_empty() {
                    if role == TokenRole::OptionName {
                        Check::Unknown
                    } else {
                        Check::Arguments
                    }
                } else {
                    Check::Invalid(vocabulary)
                }
            }
            providers::Answer::NotApplicable | providers::Answer::Failed(_) => Check::Unknown,
        }
    }

    /// `--help` output, man pages, and history for an app without a native
    /// completer: documented command levels and options, never authoritative.
    fn fallback_check(&mut self, state: &State, role: TokenRole, typed: &str) -> Check {
        let slot = providers::Slot {
            role,
            program: &state.program,
            path: state.path.as_ref(),
            context: &state.context,
            command_path: &state.command_path,
            typed,
            fresh: false,
        };
        let answers = [
            self.help
                .as_mut()
                .map(|p| p.vocabulary(&slot, &mut self.budget)),
            self.man
                .as_mut()
                .map(|p| p.vocabulary(&slot, &mut self.budget)),
            self.history
                .as_mut()
                .map(|p| p.vocabulary(&slot, &mut self.budget)),
        ];
        let mut merged = providers::Vocabulary {
            words: Vec::new(),
            authoritative: false,
            via: String::new(),
            source: Source::ManPage,
            cached: false,
            resources: Vec::new(),
        };
        let mut vias = Vec::new();
        for answer in answers.into_iter().flatten() {
            let providers::Answer::Words(vocabulary) = answer else {
                continue;
            };
            if vias.is_empty() {
                merged.source = vocabulary.source;
            }
            vias.push(vocabulary.via);
            for word in vocabulary.words {
                match merged.words.iter_mut().find(|w| w.value == word.value) {
                    Some(known) if known.takes_value.is_none() => {
                        known.takes_value = word.takes_value
                    }
                    Some(_) => {}
                    None => merged.words.push(word),
                }
            }
        }
        merged.via = vias.join(" and ");
        if merged.words.is_empty() {
            return if role == TokenRole::Subcommand {
                Check::Arguments
            } else {
                Check::Unknown
            };
        }
        match merged.words.iter().find(|w| w.value == typed) {
            Some(word) => Check::Valid {
                takes_value: word.takes_value,
                confirmed: false,
            },
            None => Check::Invalid(merged),
        }
    }

    /// Checks the words after the program against the app's completer (or,
    /// without one, documentation and history), from the first level down,
    /// branching on close alternatives.
    fn walk(&mut self, script: &Script, index: usize, p: usize, start: State) -> Vec<State> {
        let src = &script.source;
        let words = &script.commands[index].words;
        let mut done = Vec::new();
        // (interpretation, next word, level, the previous option takes a value)
        let mut frontier = vec![(start, p + 1, Level::Commands, false)];
        while let Some((mut state, mut i, mut level, mut pending_value)) = frontier.pop() {
            while i < words.len() {
                if self.budget.exhausted() {
                    self.note("probe budget exhausted; later words were not checked".into());
                    self.unchecked = true;
                    break;
                }
                let word = &words[i];
                let Some(text) = word.literal() else {
                    // An expansion: its meaning is unknown, so words after it
                    // can no longer be checked as subcommands.
                    if !pending_value {
                        level = Level::Arguments;
                    }
                    pending_value = false;
                    state.context.push("x".into());
                    i += 1;
                    continue;
                };
                if pending_value {
                    pending_value = false;
                    if let Some(vocabulary) = self.check_value(&state, &[], text)
                        && let Branching::Branches(mut branches) = self.branch(
                            &state,
                            (index, i),
                            TokenRole::OptionValue,
                            (text, ""),
                            word.span,
                            &vocabulary,
                        )
                    {
                        let (first, _) = branches.remove(0);
                        for (other, _) in branches.into_iter().rev() {
                            frontier.push((other, i + 1, level, false));
                        }
                        state = first;
                        i += 1;
                        continue;
                    }
                    state.context.push(text.to_owned());
                    i += 1;
                    continue;
                }
                if text == "--" {
                    break;
                }
                let is_option = text.len() > 1 && text.starts_with('-');
                if !is_option && level == Level::Arguments {
                    state.context.push(text.to_owned());
                    i += 1;
                    continue;
                }
                let (role, typed) = if is_option {
                    (
                        TokenRole::OptionName,
                        text.split('=').next().unwrap_or(text),
                    )
                } else {
                    (TokenRole::Subcommand, text)
                };
                let vocabulary = match self.check(&state, role, typed) {
                    Check::Valid {
                        takes_value,
                        confirmed,
                    } => {
                        // A misspelled program whose own completer accepts
                        // the next word fits the whole command.
                        if confirmed
                            && !is_option
                            && state.command_path.is_empty()
                            && state.edits.iter().any(|e| e.role == TokenRole::Executable)
                        {
                            state.score = (state.score + CONTEXT_FITS).min(1.0);
                            state.confirmed = true;
                            state.evidence.push(Evidence {
                                source: Source::NativeCompletion,
                                detail: format!("{} completion lists `{text}`", state.program),
                            });
                        }
                        // `--output=jsn`: check the attached value too.
                        if let Some(value) =
                            text.strip_prefix(typed).and_then(|t| t.strip_prefix('='))
                            && takes_value != Some(false)
                            && let Some(span) = attached_value_span(word, src, typed)
                            && let Some(vocabulary) = self.check_value(&state, &[typed], value)
                            && let Branching::Branches(mut branches) = self.branch(
                                &state,
                                (index, i),
                                TokenRole::OptionValue,
                                (value, ""),
                                span,
                                &vocabulary,
                            )
                        {
                            let joined = |mut s: State| {
                                let value = s.context.pop().unwrap_or_default();
                                s.context.push(format!("{typed}={value}"));
                                s
                            };
                            let (first, _) = branches.remove(0);
                            for (other, _) in branches.into_iter().rev() {
                                frontier.push((joined(other), i + 1, level, false));
                            }
                            state = joined(first);
                            i += 1;
                            continue;
                        }
                        pending_value =
                            is_option && !text.contains('=') && takes_value != Some(false);
                        if !is_option {
                            state.command_path.push(text.to_owned());
                        }
                        state.context.push(text.to_owned());
                        i += 1;
                        continue;
                    }
                    Check::Arguments => {
                        level = Level::Arguments;
                        state.context.push(text.to_owned());
                        i += 1;
                        continue;
                    }
                    Check::Unknown if is_option => {
                        pending_value = !text.contains('=');
                        state.context.push(text.to_owned());
                        i += 1;
                        continue;
                    }
                    Check::Unknown => {
                        self.unchecked = true;
                        break;
                    }
                    Check::Invalid(_) if is_option && word.option_name_span(src).is_none() => {
                        // A quoted option name can't be edited in place.
                        pending_value = !text.contains('=');
                        state.context.push(text.to_owned());
                        i += 1;
                        continue;
                    }
                    Check::Invalid(vocabulary) => vocabulary,
                };
                let span = match role {
                    TokenRole::OptionName => word.option_name_span(src).unwrap_or(word.span),
                    _ => word.span,
                };
                match self.branch(
                    &state,
                    (index, i),
                    role,
                    (typed, &text[typed.len()..]),
                    span,
                    &vocabulary,
                ) {
                    Branching::NotSuspicious => {
                        // A partial list says nothing about a word far from it.
                        pending_value = is_option && !text.contains('=');
                        state.context.push(text.to_owned());
                        i += 1;
                    }
                    Branching::Unresolved => {
                        state.score *= UNRESOLVED_PENALTY;
                        break;
                    }
                    Branching::Branches(mut branches) => {
                        let pending = |takes_value: Option<bool>| {
                            is_option && !text.contains('=') && takes_value != Some(false)
                        };
                        let (first, takes_value) = branches.remove(0);
                        for (other, takes_value) in branches.into_iter().rev() {
                            frontier.push((other, i + 1, level, pending(takes_value)));
                        }
                        state = first;
                        pending_value = pending(takes_value);
                        i += 1;
                    }
                }
            }
            done.push(state);
        }
        done
    }

    /// The values the app lists for the option ending the context (plus
    /// `extra` words), when `typed` is not among them. Values that look like
    /// paths or URIs are never judged.
    fn check_value(
        &mut self,
        state: &State,
        extra: &[&str],
        typed: &str,
    ) -> Option<providers::Vocabulary> {
        if typed.is_empty() || typed.contains('/') {
            return None;
        }
        let mut context = state.context.clone();
        context.extend(extra.iter().map(|w| w.to_string()));
        let vocabulary = if self.has_native(state) {
            self.native.as_mut()?.values(
                &state.program,
                state.path.as_ref(),
                &context,
                &mut self.budget,
            )?
        } else {
            let slot = providers::Slot {
                role: TokenRole::OptionValue,
                program: &state.program,
                path: state.path.as_ref(),
                context: &context,
                command_path: &state.command_path,
                typed,
                fresh: false,
            };
            match self.help.as_mut()?.vocabulary(&slot, &mut self.budget) {
                providers::Answer::Words(vocabulary) => vocabulary,
                _ => return None,
            }
        };
        (!vocabulary.contains(typed)).then_some(vocabulary)
    }

    /// `typed` (followed by `suffix`, such as `=value`) is not listed in
    /// `vocabulary`: records the suspicion and returns the closest listed
    /// words as new interpretations, with whether each takes a value.
    fn branch(
        &mut self,
        state: &State,
        (index, i): (usize, usize),
        role: TokenRole,
        (typed, suffix): (&str, &str),
        span: Span,
        vocabulary: &providers::Vocabulary,
    ) -> Branching {
        let history = self.history.as_ref();
        let uses =
            |word: &str| history.map_or(0, |h| h.uses(&state.program, &state.command_path, word));
        let ranked = within_window(ranking::rank_tokens_as(
            comparison(role),
            typed,
            vocabulary.words.iter().map(|v| v.value.as_str()),
            &self.diagnosis.suggestions,
            &uses,
            BEAM,
        ));
        if ranked.is_empty() && !vocabulary.authoritative {
            return Branching::NotSuspicious;
        }
        let kind = if role == TokenRole::OptionName {
            ProblemKind::UnknownOption
        } else {
            ProblemKind::UnknownCommand
        };
        let mut evidence = vec![(
            if vocabulary.authoritative {
                APP_REJECTS
            } else {
                APP_OMITS
            },
            Evidence {
                source: vocabulary.source,
                detail: format!(
                    "`{typed}` is not listed after `{}` by {}{}",
                    [state.program.as_str()]
                        .into_iter()
                        .chain(state.context.iter().map(String::as_str))
                        .collect::<Vec<_>>()
                        .join(" "),
                    vocabulary.via,
                    if vocabulary.authoritative {
                        ""
                    } else {
                        " (a partial list)"
                    }
                ),
            },
        )];
        if self.diagnosis.mentions(kind, typed) {
            evidence.push((
                OUTPUT_NAMES_IT,
                Evidence {
                    source: Source::Stderr,
                    detail: format!("the error output names `{typed}`"),
                },
            ));
        }
        let suspicion = self.suspect(index, i, role, typed, evidence.clone());
        if ranked.is_empty() {
            self.note(format!(
                "{}: `{typed}` is not valid after `{}`, and nothing valid is close",
                state.program,
                state.context.join(" ")
            ));
            return Branching::Unresolved;
        }
        let branches = ranked
            .into_iter()
            .map(|(to, score)| {
                let mut next = state.clone();
                let listed = vocabulary.words.iter().find(|v| v.value == to);
                let takes_value = listed.and_then(|v| v.takes_value);
                next.context.push(format!("{to}{suffix}"));
                if role == TokenRole::Subcommand {
                    next.command_path.push(to.to_owned());
                }
                if score.history > 0.0 {
                    next.evidence.push(Evidence {
                        source: Source::History,
                        detail: format!("you have used `{to}` here before"),
                    });
                }
                next.score *= score.total;
                next.weak |= suspicion < STRONG_SUSPICION;
                next.evidence
                    .extend(evidence.iter().map(|(_, e)| e.clone()));
                next.edits.push(TokenEdit {
                    command: index,
                    word: i,
                    span,
                    role,
                    from: typed.to_owned(),
                    to: to.to_owned(),
                    via: vocabulary.via.clone(),
                    description: listed.and_then(|v| v.description.clone()),
                    resource: vocabulary.resources.iter().any(|r| r == to),
                    score,
                });
                (next, takes_value)
            })
            .collect();
        Branching::Branches(branches)
    }

    /// Replacements offered by the error output, for apps without a native
    /// completer. Each occurrence of the named token is its own candidate.
    fn hint_candidates(
        &mut self,
        script: &Script,
        index: usize,
        p: usize,
        start: State,
    ) -> Vec<State> {
        if self.hints.is_none() {
            return Vec::new();
        }
        let src = &script.source;
        let words = &script.commands[index].words;
        let problems: Vec<_> = self
            .diagnosis
            .problems
            .iter()
            .filter(|problem| {
                matches!(
                    problem.kind,
                    ProblemKind::UnknownCommand | ProblemKind::UnknownOption
                )
            })
            .cloned()
            .collect();
        let mut states = Vec::new();
        for problem in problems {
            let option = problem.kind == ProblemKind::UnknownOption;
            let role = if option {
                TokenRole::OptionName
            } else {
                TokenRole::Subcommand
            };
            let positions: Vec<usize> = (p + 1..words.len())
                .filter(|&i| {
                    let Some(text) = words[i].literal() else {
                        return false;
                    };
                    if option {
                        text.split('=').next().map(|n| n.trim_start_matches('-'))
                            == Some(problem.token.trim_start_matches('-'))
                            && words[i].option_name_span(src).is_some()
                    } else {
                        text == problem.token
                    }
                })
                .collect();
            for &i in &positions {
                let evidence = vec![(
                    OUTPUT_NAMES_IT,
                    Evidence {
                        source: Source::Stderr,
                        detail: format!("the error output rejects `{}`", problem.token),
                    },
                )];
                self.suspect(index, i, role, &problem.token, evidence);
            }
            let slot = providers::Slot {
                role,
                program: &start.program,
                path: start.path.as_ref(),
                context: &[],
                command_path: &[],
                typed: &problem.token,
                fresh: false,
            };
            let Some(providers::Answer::Words(vocabulary)) = self
                .hints
                .as_mut()
                .map(|hints| hints.vocabulary(&slot, &mut self.budget))
            else {
                continue;
            };
            let ranked = ranking::rank_tokens_as(
                comparison(role),
                &problem.token,
                vocabulary.words.iter().map(|w| w.value.as_str()),
                &self.diagnosis.suggestions,
                &|_: &str| 0,
                BEAM,
            );
            for (n, &i) in positions.iter().enumerate() {
                let word = &words[i];
                for (to, score) in &ranked {
                    let mut state = start.clone();
                    state.score = score.total * (1.0 - LATER_OCCURRENCE_PENALTY * n as f64);
                    state.evidence.push(Evidence {
                        source: Source::Stderr,
                        detail: format!(
                            "the error output rejects `{}` and suggests `{to}`",
                            problem.token
                        ),
                    });
                    state.edits.push(TokenEdit {
                        command: index,
                        word: i,
                        span: if option {
                            word.option_name_span(src).unwrap_or(word.span)
                        } else {
                            word.span
                        },
                        role,
                        from: problem.token.clone(),
                        to: (*to).to_owned(),
                        via: vocabulary.via.clone(),
                        description: None,
                        resource: false,
                        score: score.clone(),
                    });
                    states.push(state);
                }
            }
        }
        states
    }

    /// Applies a state's edits and checks that only the intended words
    /// changed: same commands, operators, redirections, and other words.
    fn build(&mut self, script: &Script, state: State) -> Option<Candidate> {
        if state.edits.is_empty() {
            return None;
        }
        let edits: Vec<parser::Edit> = state
            .edits
            .iter()
            .map(|e| parser::Edit {
                span: e.span,
                replacement: if e.role == TokenRole::Split {
                    e.to.split(' ')
                        .map(|word| parser::quote_word_with_dialect(word, script.dialect))
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    parser::quote_word_with_dialect(&e.to, script.dialect)
                },
            })
            .collect();
        let source = parser::apply_edits(&script.source, &edits)?;
        if let Err(why) = check_structure(script, &source, &state.edits) {
            self.note(format!("rejected {source}: {why}"));
            return None;
        }
        Some(Candidate {
            script: source,
            edits: state.edits,
            evidence: state.evidence,
            score: state.score,
            safety: SafetyAssessment {
                decision: Decision::Allow,
                reasons: Vec::new(),
            },
            weak: state.weak,
            confirmed: state.confirmed,
        })
    }
}

/// One dash before several characters (`-la`, `-cf`, `-j4`, `-I/usr/lib`)
/// is a cluster of short options, or one with its value attached, when its
/// first letter is a listed short option. Otherwise only a multi-letter
/// option (`find -type`, `--verbose`) can have been meant: replacing a
/// cluster with a single short option would silently drop the rest of it.
fn short_cluster(typed: &str, vocabulary: &providers::Vocabulary) -> Option<Check> {
    let letters = typed
        .strip_prefix('-')
        .filter(|rest| !rest.starts_with('-'))?;
    // `-x` alone is an ordinary short option.
    letters.chars().nth(1)?;
    let short = |c: char| {
        vocabulary
            .words
            .iter()
            .find(|w| w.value.strip_prefix('-') == Some(c.encode_utf8(&mut [0; 4])))
    };
    if letters.chars().next().and_then(short).is_some() {
        let takes_value = match letters.chars().map(short).collect::<Option<Vec<_>>>() {
            // The last option of a cluster may take the next word.
            Some(options) => options.last().and_then(|o| o.takes_value),
            // `-j4`, `-oout`: the value is attached.
            None => Some(false),
        };
        return Some(Check::Valid {
            takes_value,
            confirmed: false,
        });
    }
    let mut longer = vocabulary.clone();
    longer
        .words
        .retain(|w| w.value.trim_start_matches('-').chars().nth(1).is_some());
    Some(if longer.words.is_empty() {
        Check::Unknown
    } else {
        Check::Invalid(longer)
    })
}

/// The entire value suffix of `--name=value`, including its quotes/escapes.
/// The parsed word must be literal, and the option name must be plain.
/// Replacing the whole suffix lets the selected dialect quote it safely.
fn attached_value_span(word: &parser::Word, src: &str, name: &str) -> Option<Span> {
    let name_span = word.option_name_span(src)?;
    let raw = &src[name_span.end..word.span.end];
    let value = raw.strip_prefix('=')?;
    (name_span.of(src) == name
        && !value.is_empty()
        && word
            .literal()
            .is_some_and(|text| text.starts_with(&format!("{name}="))))
    .then(|| Span::new(name_span.end + 1, word.span.end))
}

/// Two repairs of the same words that are equally many edits away and
/// equally backed by hints and history: only word length would separate
/// them (`mal` → `mail` or `man`), which is no reason to choose alone.
fn equally_supported(a: &Candidate, b: &Candidate) -> bool {
    a.confirmed == b.confirmed
        && a.edits.len() == b.edits.len()
        && a.edits.iter().zip(&b.edits).all(|(x, y)| {
            (x.command, x.word, x.role) == (y.command, y.word, y.role)
                && x.score.distance == y.score.distance
                && x.score.first_letter == y.score.first_letter
                && x.score.hint == y.score.hint
                && x.score.history == y.score.history
        })
}

/// How spellings in a role compare: option names without their dashes.
fn comparison(role: TokenRole) -> ranking::Comparison {
    if role == TokenRole::OptionName {
        ranking::Comparison::OptionName
    } else {
        ranking::Comparison::Word
    }
}

/// Drops alternatives too far behind the best one to be worth probing.
fn within_window<T>(mut ranked: Vec<(T, ScoreBreakdown)>) -> Vec<(T, ScoreBreakdown)> {
    if let Some(best) = ranked.first().map(|(_, s)| s.total) {
        ranked.retain(|(_, s)| s.total >= best - BRANCH_WINDOW);
    }
    ranked
}

/// Reparses an edited command line and verifies that the edits changed
/// exactly the intended words and nothing else.
fn check_structure(old: &Script, source: &str, edits: &[TokenEdit]) -> Result<(), String> {
    let new = parser::parse_with_dialect(source, old.dialect);
    if !new.is_fully_supported() {
        return Err("the edit produced syntax that can't be analyzed".into());
    }
    if new.commands.len() != old.commands.len() {
        return Err("the number of commands changed".into());
    }
    if new.compound_shapes() != old.compound_shapes() {
        return Err("the compound command structure changed".into());
    }
    let literal = |script: &Script, w: &parser::Word| {
        w.literal()
            .map_or_else(|| format!("\0{}", w.span.of(&script.source)), str::to_owned)
    };
    for (n, (a, b)) in old.commands.iter().zip(&new.commands).enumerate() {
        if a.connector != b.connector
            || a.assignments.len() != b.assignments.len()
            || a.redirections.len() != b.redirections.len()
        {
            return Err("the command structure changed".into());
        }
        for (x, y) in a.assignments.iter().zip(&b.assignments) {
            if literal(old, x) != literal(&new, y) {
                return Err("an assignment changed".into());
            }
        }
        for (x, y) in a.redirections.iter().zip(&b.redirections) {
            let target =
                |s: &Script, r: &parser::Redirection| r.target.as_ref().map(|t| literal(s, t));
            if x.operator.of(&old.source) != y.operator.of(&new.source)
                || target(old, x) != target(&new, y)
            {
                return Err("a redirection changed".into());
            }
        }
        let mut expected: Vec<String> = Vec::new();
        for (i, x) in a.words.iter().enumerate() {
            let word = match edits.iter().find(|e| e.command == n && e.word == i) {
                Some(edit) if edit.role == TokenRole::Split => {
                    expected.extend(edit.to.split(' ').map(str::to_owned));
                    continue;
                }
                Some(edit) if edit.role == TokenRole::OptionName => {
                    let old_text = literal(old, x);
                    format!(
                        "{}{}",
                        edit.to,
                        &old_text[edit.from.len().min(old_text.len())..]
                    )
                }
                // `--output=jsn` → `--output=json`: the value is the suffix.
                Some(edit)
                    if edit.role == TokenRole::OptionValue && literal(old, x) != edit.from =>
                {
                    let old_text = literal(old, x);
                    format!(
                        "{}{}",
                        &old_text[..old_text.len().saturating_sub(edit.from.len())],
                        edit.to
                    )
                }
                Some(edit) => edit.to.clone(),
                None => literal(old, x),
            };
            expected.push(word);
        }
        let actual: Vec<String> = b.words.iter().map(|w| literal(&new, w)).collect();
        if actual != expected {
            return Err(format!("the words would read `{}`", actual.join(" ")));
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::native::tests::{Dir, FAKE_GIT};
    use super::*;
    use crate::settings::Settings;
    use std::fs;

    /// An aws_completer double that answers from a `tree` file of
    /// `<context glob>|<word>` lines, so tests can change the app's
    /// command surface between requests.
    const TREE_COMPLETER: &str = r#"#!/bin/sh
# awscli test double
line=${COMP_LINE#aws }
prefix=${line##* }
context=${line% *}
[ "$context" = "$line" ] && context=
# Like the real completer, global options don't change the level.
case $context in
  "--region "*) rest=${context#--region }
    case $rest in *" "*) context=${rest#* };; *) context=;; esac;;
esac
lists="$(dirname "$0")/tree"
[ -r "$lists" ] || exit 2
# Resource names need credentials: offline queries get none.
[ "$AWS_CONFIG_FILE" != /dev/null ] && [ -f "$(dirname "$0")/resources" ] &&
  lists="$lists $(dirname "$0")/resources"
cat $lists | while IFS='|' read -r pattern word; do
  case $context in
    $pattern) case $word in "$prefix"*) printf '%s\n' "$word";; esac;;
  esac
done
"#;

    const TREE: &str = "|ec2\n|s3\n|sts\n|--region\n|--profile\n\
ec2|describe-instances\nec2|describe-instance-status\nec2|run-instances\nec2|terminate-instances\n\
ec2 *-instances*|--instance-ids\nec2 *-instances*|--region\nec2 *-instances*|--dry-run\n\
ec2 *-instances*|--output\n\
ec2 *-instances* --region|eu-west-1\nec2 *-instances* --region|eu-west-2\nec2 *-instances* --region|us-east-1\n\
ec2 *-instances* --output|json\nec2 *-instances* --output|table\nec2 *-instances* --output|text\n";

    struct Fake {
        dir: Dir,
        aws: PathBuf,
    }

    fn fake_aws() -> Fake {
        let dir = Dir::new("engine");
        let aws = dir.script("bin/aws", "#!/bin/sh\ntouch \"$(dirname \"$0\")/ran\"\n");
        dir.script("bin/aws_completer", TREE_COMPLETER);
        dir.script("bin/git", FAKE_GIT);
        fs::write(dir.0.join("bin/tree"), TREE).unwrap();
        Fake { dir, aws }
    }

    fn context(fake: &Fake) -> Context {
        context_with(fake, Settings::default())
    }

    fn context_with(fake: &Fake, settings: Settings) -> Context {
        Context::new(settings, Shell::Bash, "fuck".into())
            .with_executables(&["aws", "git", "grep", "gzip", "ls"])
            .with_which("aws", Some(fake.aws.to_str().unwrap()))
            .with_which("git", Some(fake.dir.0.join("bin/git").to_str().unwrap()))
            .with_which("ls", Some("/bin/ls"))
            .with_which("gti", None)
            .with_which("jq", Some("/usr/bin/jq"))
            .with_which("man", None)
            .with_history::<&str>(&[])
    }

    fn failure(source: &str) -> FailureContext {
        FailureContext {
            source: source.into(),
            ..FailureContext::default()
        }
    }

    fn scripts(outcome: &Outcome) -> Vec<&str> {
        outcome
            .candidates()
            .iter()
            .map(|c| c.script.as_str())
            .collect()
    }

    #[test]
    fn native_completion_repairs_operations_and_options() {
        let fake = fake_aws();
        let ctx = context(&fake);
        let report = correct(
            &failure("aws ec2 describ-instances --regoin eu-west-1"),
            &ctx,
        );
        let Outcome::Suggestion(candidates) = &report.outcome else {
            panic!("{:?} {:?}", report.outcome, report.notes);
        };
        assert_eq!(
            candidates[0].script,
            "aws ec2 describe-instances --region eu-west-1"
        );
        assert_eq!(candidates[0].edits.len(), 2);
        assert!(
            candidates[0]
                .evidence
                .iter()
                .all(|e| e.source == Source::NativeCompletion)
        );
        assert_eq!(candidates[0].safety.decision, Decision::Allow);
        assert!(
            !fake.dir.0.join("bin/ran").exists(),
            "discovery must not run the app's failed operation"
        );
        assert!(report.probes <= 4, "{} probes", report.probes);
    }

    #[test]
    fn only_the_diagnosed_token_changes() {
        let fake = fake_aws();
        let ctx = context(&fake);
        let source = "AWS_PAGER='' aws ec2 describ-instances --region=eu-west-1 | jq '.x y' > \"out file\" # note";
        let report = correct(&failure(source), &ctx);
        assert_eq!(
            scripts(&report.outcome)[0],
            "AWS_PAGER='' aws ec2 describe-instances --region=eu-west-1 | jq '.x y' > \"out file\" # note"
        );
    }

    #[test]
    fn commands_inside_compound_commands_are_repaired_in_place() {
        let fake = fake_aws();
        let ctx = context(&fake);
        for (source, expected) in [
            (
                "for r in a 'b c'; do aws ec2 describ-instances; done > log",
                "for r in a 'b c'; do aws ec2 describe-instances; done > log",
            ),
            (
                "{ aws ec2 describ-instances; } | jq .",
                "{ aws ec2 describe-instances; } | jq .",
            ),
            (
                "f() { aws ec2 describ-instances; } # keep",
                "f() { aws ec2 describe-instances; } # keep",
            ),
            (
                "! aws ec2 describ-instances",
                "! aws ec2 describe-instances",
            ),
        ] {
            let report = correct(&failure(source), &ctx);
            let Outcome::Suggestion(candidates) = &report.outcome else {
                panic!("{source}: {:?} {:?}", report.outcome, report.notes);
            };
            assert_eq!(candidates[0].script, expected);
            assert_eq!(candidates[0].safety.decision, Decision::Allow, "{source}");
        }
        assert!(!fake.dir.0.join("bin/ran").exists());
        let mut f = failure("if true; then gti status; fi");
        f.exit_status = Some(127);
        assert_eq!(
            scripts(&correct(&f, &ctx).outcome).first().copied(),
            Some("if true; then git status; fi")
        );

        let old = parser::parse("for x in a; do git sttus; done");
        let edit = |to: &str, word| TokenEdit {
            command: 0,
            word,
            role: TokenRole::Subcommand,
            span: old.commands[0].words[word].span,
            from: "sttus".into(),
            to: to.into(),
            via: "test".into(),
            description: None,
            resource: false,
            score: ranking::score_token("sttus", to, false, 0),
        };
        assert!(
            check_structure(
                &old,
                "for x in a; do git status; done",
                &[edit("status", 1)]
            )
            .is_ok()
        );
        assert!(
            check_structure(
                &old,
                "for x in b; do git status; done",
                &[edit("status", 1)]
            )
            .is_err(),
            "a loop's items are not part of the edit"
        );
        assert!(
            check_structure(&old, "while x; do git status; done", &[edit("status", 1)]).is_err()
        );
    }

    #[test]
    fn tcsh_lines_are_repaired_with_tcsh_syntax() {
        let fake = fake_aws();
        let ctx = Context::new(Settings::default(), Shell::Tcsh, "fuck".into())
            .with_executables(&["aws", "git", "grep", "gzip", "ls"])
            .with_which("aws", Some(fake.aws.to_str().unwrap()))
            .with_which("git", Some(fake.dir.0.join("bin/git").to_str().unwrap()))
            .with_which("gti", None)
            .with_which("man", None)
            .with_history::<&str>(&[]);
        let case = |source: &str, status: i32| {
            let mut f = failure(source);
            f.exit_status = Some(status);
            correct(&f, &ctx).outcome
        };
        // tcsh exits 1, not 127, for a command it can't find.
        let outcome = case("gti status 'a b' >& log", 1);
        let Outcome::Suggestion(candidates) = &outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(candidates[0].script, "git status 'a b' >& log");
        assert!(
            candidates[0]
                .evidence
                .iter()
                .any(|e| e.detail.contains("exited with 1"))
        );
        assert_eq!(
            scripts(&case("aws ec2 describ-instances |& grep x", 255))[0],
            "aws ec2 describe-instances |& grep x"
        );
        assert!(matches!(
            case("gti status x!y", 1),
            Outcome::UnsupportedSyntax(_)
        ));
        assert!(matches!(case("gti status", 0), Outcome::NoCorrection(_)));
        assert!(!fake.dir.0.join("bin/ran").exists());
    }

    #[test]
    fn new_operations_are_used_without_code_changes() {
        let fake = fake_aws();
        let ctx = context(&fake);
        let source = "aws ec2 describe-capacity-blocks";
        assert!(matches!(
            correct(&failure(source), &ctx).outcome,
            Outcome::Suggestion(_) | Outcome::Ambiguous(_) | Outcome::InsufficientEvidence(_)
        ));
        // The installed CLI gains an operation: the same request now validates.
        let tree = fs::read_to_string(fake.dir.0.join("bin/tree")).unwrap();
        fs::write(
            fake.dir.0.join("bin/tree"),
            format!("{tree}ec2|describe-capacity-blocks\n"),
        )
        .unwrap();
        let report = correct(&failure(source), &ctx);
        assert!(
            matches!(report.outcome, Outcome::NoCorrection(_)),
            "{:?}",
            report.outcome
        );
        let report = correct(&failure("aws ec2 describe-capacity-block"), &ctx);
        assert_eq!(scripts(&report.outcome)[0], source);
    }

    #[test]
    fn git_commands_options_and_subcommands_come_from_git() {
        let fake = fake_aws();
        let ctx = context(&fake);
        for (typo, fixed, decisive) in [
            ("git sttus", "git status", true),
            ("git stash pp", "git stash pop", true),
            // git's option lists are partial: without output, ask.
            (
                "git -C repo stash pop --idex",
                "git -C repo stash pop --index",
                false,
            ),
            (
                "git commit --amnd -m 'msg here'",
                "git commit --amend -m 'msg here'",
                false,
            ),
        ] {
            let report = correct(&failure(typo), &ctx);
            assert_eq!(
                matches!(report.outcome, Outcome::Suggestion(_)),
                decisive,
                "{typo}: {:?} {:?}",
                report.outcome,
                report.notes
            );
            assert_eq!(scripts(&report.outcome)[0], fixed);
        }
        for valid in [
            "git commit -m x -a",
            "git co main",
            "git --no-pager status --short",
        ] {
            let report = correct(&failure(valid), &ctx);
            assert!(
                matches!(report.outcome, Outcome::NoCorrection(_)),
                "{valid}: {:?}",
                report.outcome
            );
        }
        assert!(!fake.dir.0.join("bin/ran").exists());
    }

    #[test]
    fn man_pages_supply_options_for_apps_without_completion() {
        let fake = fake_aws();
        let man = fake.dir.script(
            "bin/man",
            "#!/bin/sh\n[ \"$1\" = ls ] || exit 1\nprintf 'LS(1)\\n     -\\b-a\\ba      all\\n     -\\b--\\b-c\\bco\\bol\\blo\\bor\\br[=when]   colorize\\n'\n",
        );
        let ctx = context(&fake).with_which("man", Some(man.to_str().unwrap()));
        let mut f = failure("ls --colro=auto -a");
        f.exit_status = Some(2);
        let report = correct(&f, &ctx);
        assert!(
            matches!(report.outcome, Outcome::Ambiguous(_)),
            "{:?}",
            report.outcome
        );
        assert_eq!(scripts(&report.outcome)[0], "ls --color=auto -a");
        assert!(
            report.outcome.candidates()[0]
                .evidence
                .iter()
                .any(|e| e.source == Source::ManPage)
        );
        f.output = CapturedOutput::Combined {
            text: "ls: unrecognized option '--colro=auto'".into(),
            origin: OutputOrigin::ShellLogger,
        };
        assert!(matches!(correct(&f, &ctx).outcome, Outcome::Suggestion(_)));
        assert!(
            matches!(
                correct(&failure("ls --all-of-it"), &ctx).outcome,
                Outcome::NoCorrection(_)
            ),
            "an unlisted option far from every documented one is left alone"
        );
    }

    #[test]
    fn history_ranks_and_supplies_first_level_words() {
        let fake = fake_aws();
        let cargo = fake.dir.script("bin/cargo", "#!/bin/sh\n");
        let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into())
            .with_which("cargo", Some(cargo.to_str().unwrap()))
            .with_which("go", Some(cargo.to_str().unwrap()))
            .with_which("gt", None)
            .with_which("man", None)
            .with_executables(&["git", "gzip", "go", "cargo"])
            .with_history(&[
                "cargo test",
                "cargo build --release",
                "go build",
                "go test",
                "go vet",
                "fuck",
            ]);
        let mut f = failure("cargo tset");
        f.exit_status = Some(101);
        let report = correct(&f, &ctx);
        assert_eq!(
            scripts(&report.outcome)[0],
            "cargo test",
            "{:?}",
            report.notes
        );
        assert!(
            matches!(report.outcome, Outcome::Ambiguous(_)),
            "history alone is weak"
        );
        let report = correct(&failure("cargo build --relase"), &ctx);
        assert_eq!(scripts(&report.outcome)[0], "cargo build --release");
        assert!(matches!(
            correct(&failure("cargo add serde"), &ctx).outcome,
            Outcome::NoCorrection(_)
        ));
        // `gt` is as close to git as to go; history prefers what was used.
        let mut f = failure("gt vet");
        f.exit_status = Some(127);
        let report = correct(&f, &ctx);
        assert_eq!(
            scripts(&report.outcome)[0],
            "go vet",
            "{:?}",
            report.outcome
        );
    }

    #[test]
    fn missing_paths_are_repaired_from_the_filesystem() {
        let fake = fake_aws();
        let root = fake.dir.0.join("work");
        fs::create_dir_all(root.join("src/engine")).unwrap();
        fs::write(root.join("src/engine/parser.rs"), "").unwrap();
        fs::write(root.join("notes.txt"), "").unwrap();
        fake.dir.script("work/script.sh", "#!/bin/sh\n");
        let ctx = context(&fake).with_which("cat", Some("/bin/cat"));
        let case = |source: &str, status: Option<i32>, output: Option<&str>| {
            let mut f = failure(source);
            f.cwd = Some(root.clone());
            f.exit_status = status;
            if let Some(text) = output {
                f.output = CapturedOutput::Combined {
                    text: text.into(),
                    origin: OutputOrigin::ShellLogger,
                };
            }
            correct(&f, &ctx).outcome
        };
        let outcome = case("cd sr/engin", Some(1), None);
        assert!(matches!(outcome, Outcome::Suggestion(_)), "{outcome:?}");
        assert_eq!(scripts(&outcome)[0], "cd src/engine");
        let outcome = case(
            "cat notse.txt",
            Some(1),
            Some("cat: notse.txt: No such file or directory"),
        );
        assert_eq!(scripts(&outcome)[0], "cat notes.txt");
        let outcome = case("cat src/engine/parsr.rs", Some(1), None);
        assert_eq!(scripts(&outcome)[0], "cat src/engine/parser.rs");
        let outcome = case("./scirpt.sh --x", Some(127), None);
        assert_eq!(scripts(&outcome)[0], "./script.sh --x");
        assert!(
            matches!(
                case("cat src/engine/parsr.rs", None, None),
                Outcome::NoCorrection(_)
            ),
            "an argument path is only questioned after a known failure"
        );
        assert!(matches!(
            case("cd src", Some(1), None),
            Outcome::NoCorrection(_)
        ));
    }

    #[test]
    fn chosen_corrections_are_rechecked_before_they_run() {
        let fake = fake_aws();
        let root = fake.dir.0.join("work");
        fs::create_dir_all(root.join("src/engine")).unwrap();
        fs::write(root.join("src/notes.txt"), "").unwrap();
        let script = fake.dir.script("work/script.sh", "#!/bin/sh\n");
        let ctx = context(&fake).with_which("cat", Some("/bin/cat"));
        let chosen: Vec<(FailureContext, Candidate)> = [
            ("cd sr/engin", 1, "cd src/engine"),
            ("cat src/notse.txt", 1, "cat src/notes.txt"),
            ("./scirpt.sh --x", 127, "./script.sh --x"),
            ("gti status", 127, "git status"),
            ("gitstatus", 127, "git status"),
        ]
        .into_iter()
        .map(|(source, status, expected)| {
            let mut f = failure(source);
            f.cwd = Some(root.clone());
            f.exit_status = Some(status);
            let report = correct(&f, &ctx);
            let candidate = report
                .outcome
                .candidates()
                .iter()
                .find(|c| c.script == expected)
                .unwrap_or_else(|| panic!("{source}: {:?}", report.outcome))
                .clone();
            (f, candidate)
        })
        .collect();
        for (f, candidate) in &chosen {
            assert_eq!(revalidate(candidate, f, &ctx), Ok(()), "{}", f.source);
        }

        // The world changes between discovery and execution.
        fs::remove_dir(root.join("src/engine")).unwrap();
        fs::write(root.join("src/engine"), "").unwrap();
        fs::remove_file(root.join("src/notes.txt")).unwrap();
        fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        fs::remove_file(fake.dir.0.join("bin/git")).unwrap();
        let errors: Vec<String> = chosen
            .iter()
            .map(|(f, candidate)| revalidate(candidate, f, &ctx).unwrap_err())
            .collect();
        assert_eq!(
            errors,
            [
                "src/engine no longer exists",
                "src/notes.txt no longer exists",
                "./script.sh is no longer executable",
                "git is no longer installed",
                "git is no longer installed",
            ]
        );
    }

    #[test]
    fn trusted_help_output_supplies_subcommands() {
        let fake = fake_aws();
        let tool = fake.dir.script(
            "bin/tool",
            "#!/bin/sh\ncase \"$*\" in '--help'|'build --help') ;; *) touch \"$(dirname \"$0\")/ran\"; exit 1;; esac\nprintf 'Usage: tool <command>\\n\\nCommands:\\n  build    Build it\\n  deploy   Ship it\\n\\nOptions:\\n  -v, --verbose  Talk\\n' >&2\n",
        );
        let settings = Settings {
            trusted_help: vec!["tool".into()],
            ..Settings::default()
        };
        let ctx = Context::new(settings, Shell::Bash, "fuck".into())
            .with_which("tool", Some(tool.to_str().unwrap()))
            .with_which("man", None)
            .with_history::<&str>(&[]);
        let report = correct(&failure("tool biuld --verbos"), &ctx);
        assert_eq!(scripts(&report.outcome)[0], "tool build --verbose");
        assert!(!fake.dir.0.join("bin/ran").exists(), "only --help ran");
        let untrusted = context(&fake).with_which("tool", Some(tool.to_str().unwrap()));
        assert!(
            correct(&failure("tool biuld"), &untrusted)
                .outcome
                .candidates()
                .is_empty()
        );
    }

    const NESTED_HELP: &str = r#"#!/bin/sh
root=$(dirname "$0")
printf '[%s]' "$@" >> "$root/queries"
printf '\n' >> "$root/queries"
case "$*" in
  '--help')
    printf 'Commands:\n  cluster    Manage clusters\n  status     Show status\n\nOptions:\n  --profile <PROFILE>  Select profile\n  --format {json,yaml}  Global format\n';;
  'cluster --help')
    printf 'Commands:\n  nodes      Manage nodes\n  backups    Manage backups\n\nOptions:\n  --region <REGION>  Select region\n';;
  'cluster nodes --help')
    printf 'Commands:\n  list    List nodes\n  show    Show a node\n';;
  'cluster nodes list --help')
    printf 'Options:\n  --verbose          Talk\n  -o, --format <FMT>  Format [possible values: text, binary]\n  -t, --target <ID>   Target\n';;
  *) touch "$root/ran"; exit 1;;
esac
"#;

    #[test]
    fn help_follows_documented_nested_commands_without_forwarding_arguments() {
        let fake = fake_aws();
        let tool = fake.dir.script("bin/tool", NESTED_HELP);
        let settings = Settings {
            trusted_help: vec!["tool".into()],
            ..Settings::default()
        };
        let ctx = Context::new(settings, Shell::Bash, "fuck".into())
            .with_which("tool", Some(tool.to_str().unwrap()))
            .with_which("man", None)
            .with_history::<&str>(&[]);
        let source = "tool --profile 'private account' cluster ndose list --verbos | cat > 'out file' # keep";
        let report = correct(&failure(source), &ctx);
        assert_eq!(
            scripts(&report.outcome)[0],
            source
                .replace("ndose", "nodes")
                .replace("--verbos", "--verbose")
        );
        assert!(
            matches!(report.outcome, Outcome::Ambiguous(_)),
            "help is partial: {:?}",
            report.outcome
        );
        assert!(
            report.outcome.candidates()[0]
                .edits
                .iter()
                .all(|e| e.via.contains("--help"))
        );
        assert_eq!(
            fs::read_to_string(fake.dir.0.join("bin/queries")).unwrap(),
            "[--help]\n[cluster][--help]\n[cluster][nodes][--help]\n[cluster][nodes][list][--help]\n"
        );
        assert!(!fake.dir.0.join("bin/ran").exists());
        // An option value and positional words must not become command paths.
        let report = correct(&failure("tool cluster nodes list -t verbos"), &ctx);
        assert!(
            matches!(report.outcome, Outcome::NoCorrection(_)),
            "{:?}",
            report.outcome
        );
        assert!(!fake.dir.0.join("bin/ran").exists());
    }

    #[test]
    fn short_option_clusters_and_attached_values_are_never_cut_down() {
        let fake = fake_aws();
        let tool = fake.dir.script(
            "bin/archiver",
            "#!/bin/sh\n[ \"$1\" = --help ] || { touch \"$(dirname \"$0\")/ran\"; exit 1; }\nprintf 'Usage: archiver [options] files\\n\\nOptions:\\n  -c            Create\\n  -x            Extract\\n  -v            Verbose\\n  -z            Compress\\n  -f FILE       Archive file\\n  -j N          Jobs\\n  -C DIR        Change directory\\n  -name PATTERN Match names\\n  --verbose     Talk\\n  --zstd        Use zstd\\n'\n",
        );
        let settings = Settings {
            trusted_help: vec!["archiver".into()],
            ..Settings::default()
        };
        let ctx = Context::new(settings, Shell::Bash, "fuck".into())
            .with_which("archiver", Some(tool.to_str().unwrap()))
            .with_which("man", None)
            .with_history::<&str>(&[]);
        for (source, expected) in [
            // `f` ends the cluster and takes `out.tar`.
            (
                "archiver --ztsd -cf out.tar src",
                "archiver --zstd -cf out.tar src",
            ),
            (
                "archiver -xvzf out.tar --verbsoe",
                "archiver -xvzf out.tar --verbose",
            ),
            ("archiver -j4 --verbsoe", "archiver -j4 --verbose"),
            ("archiver -Csrc --verbsoe", "archiver -Csrc --verbose"),
            // A single-dash long option is still repaired as a whole.
            ("archiver -nmae '*.rs'", "archiver -name '*.rs'"),
        ] {
            let report = correct(&failure(source), &ctx);
            assert_eq!(
                scripts(&report.outcome).first().copied(),
                Some(expected),
                "{source}: {:?}",
                report.outcome
            );
            assert!(
                report
                    .outcome
                    .candidates()
                    .iter()
                    .flat_map(|c| &c.edits)
                    .all(|e| e.to.trim_start_matches('-').chars().nth(1).is_some()),
                "{source}: no cluster becomes a single short option"
            );
        }
        assert!(!fake.dir.0.join("bin/ran").exists());
    }

    #[test]
    fn help_option_values_belong_to_the_leaf_declaration_and_short_alias() {
        let fake = fake_aws();
        let tool = fake.dir.script("bin/tool", NESTED_HELP);
        let settings = Settings {
            trusted_help: vec!["tool".into()],
            ..Settings::default()
        };
        let ctx = Context::new(settings, Shell::Bash, "fuck".into())
            .with_which("tool", Some(tool.to_str().unwrap()))
            .with_which("man", None)
            .with_history::<&str>(&[]);
        for (source, expected) in [
            ("tool --format jsno", "tool --format json"),
            (
                "tool cluster nodes list --format txet",
                "tool cluster nodes list --format text",
            ),
            (
                "tool cluster nodes list --format=txet",
                "tool cluster nodes list --format=text",
            ),
            (
                "tool cluster nodes list -o txet",
                "tool cluster nodes list -o text",
            ),
            (
                "tool cluster nodes list --format 'txet' --target status",
                "tool cluster nodes list --format text --target status",
            ),
        ] {
            let report = correct(&failure(source), &ctx);
            assert_eq!(
                scripts(&report.outcome)[0],
                expected,
                "{source}: {:?}",
                report.outcome
            );
            assert!(
                matches!(report.outcome, Outcome::Ambiguous(_)),
                "{source}: {:?}",
                report.outcome
            );
            assert_eq!(
                report.outcome.candidates()[0].edits[0].role,
                TokenRole::OptionValue
            );
        }
        for valid in [
            "tool cluster nodes list --format binary",
            "tool cluster nodes list --target txet",
        ] {
            assert!(
                matches!(
                    correct(&failure(valid), &ctx).outcome,
                    Outcome::NoCorrection(_)
                ),
                "{valid}"
            );
        }
        assert!(!fake.dir.0.join("bin/ran").exists());
    }

    #[test]
    fn package_runners_resolve_installed_apps_only() {
        let fake = fake_aws();
        let ctx = context(&fake)
            .with_which("cdkk", None)
            .with_which("cdk", None);
        let report = correct(&failure("npx aws ec2 describ-instances"), &ctx);
        assert_eq!(
            scripts(&report.outcome)[0],
            "npx aws ec2 describe-instances"
        );
        let report = correct(&failure("npx cdkk deploy"), &ctx);
        assert!(
            report.outcome.candidates().is_empty(),
            "{:?}",
            report.outcome
        );
        assert!(report.notes.iter().any(|n| n.contains("not installed")));
    }

    #[test]
    fn partial_option_lists_are_weak_evidence() {
        let fake = fake_aws();
        let ctx = context(&fake);
        // git's helper omits `log`'s revision options: unlisted is not wrong.
        let report = correct(&failure("git commit --graph"), &ctx);
        assert!(
            matches!(report.outcome, Outcome::NoCorrection(_)),
            "{:?}",
            report.outcome
        );
        assert!(report.suspicions.is_empty());
        let mut f = failure("git commit --amen");
        let report = correct(&f, &ctx);
        assert!(
            matches!(report.outcome, Outcome::Ambiguous(_)),
            "{:?}",
            report.outcome
        );
        assert_eq!(scripts(&report.outcome)[0], "git commit --amend");
        f.output = CapturedOutput::Combined {
            text: "error: unknown option `amen'".into(),
            origin: OutputOrigin::ShellLogger,
        };
        let report = correct(&f, &ctx);
        assert!(
            matches!(report.outcome, Outcome::Suggestion(_)),
            "{:?}",
            report.outcome
        );
    }

    #[test]
    fn missing_executables_need_shell_evidence_to_be_decisive() {
        let fake = fake_aws();
        let ctx = context(&fake);
        let mut f = failure("gti status");
        let weak = correct(&f, &ctx);
        assert!(
            matches!(weak.outcome, Outcome::Ambiguous(_)),
            "{:?}",
            weak.outcome
        );
        assert_eq!(scripts(&weak.outcome)[0], "git status");
        assert!(weak.suspicions[0].score < STRONG_SUSPICION);
        f.exit_status = Some(127);
        let strong = correct(&f, &ctx);
        assert_eq!(strong.suspicions.len(), 1);
        assert_eq!(strong.suspicions[0].role, TokenRole::Executable);
        assert!(strong.suspicions[0].score > STRONG_SUSPICION);
        assert!(
            matches!(strong.outcome, Outcome::Suggestion(_)),
            "{:?}",
            strong.outcome
        );
        assert_eq!(scripts(&strong.outcome)[0], "git status");
        f.exit_status = Some(1);
        assert!(matches!(
            correct(&f, &ctx).outcome,
            Outcome::NoCorrection(_)
        ));
    }

    #[test]
    fn a_program_that_accepts_the_rest_of_the_line_ranks_first() {
        let fake = fake_aws();
        let gtr = fake.dir.script("bin/gtr", "#!/bin/sh\n");
        let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into())
            .with_executables(&["gtr", "git"])
            .with_which("gtr", Some(gtr.to_str().unwrap()))
            .with_which("git", Some(fake.dir.0.join("bin/git").to_str().unwrap()))
            .with_which("gti", None)
            .with_which("man", None)
            .with_history::<&str>(&[]);
        let mut f = failure("gti status");
        f.exit_status = Some(127);
        let report = correct(&f, &ctx);
        assert_eq!(scripts(&report.outcome), ["git status", "gtr status"]);
        assert!(
            matches!(report.outcome, Outcome::Suggestion(_)),
            "{:?}",
            report.outcome
        );
        let mut f = failure("gti frobnicate");
        f.exit_status = Some(127);
        assert!(
            matches!(correct(&f, &ctx).outcome, Outcome::Ambiguous(_)),
            "without context agreement, equally close names are a tie"
        );
    }

    #[test]
    fn missing_spaces_after_programs_are_inserted() {
        let fake = fake_aws();
        let ctx = context(&fake)
            .with_which("cd..", None)
            .with_which("gitstatus", None)
            .with_which("awsec2", None);
        for (typo, fixed) in [
            ("cd..", "cd .."),
            ("gitstatus --short", "git status --short"),
            ("awsec2 describ-instances", "aws ec2 describe-instances"),
        ] {
            let mut f = failure(typo);
            f.exit_status = Some(127);
            let report = correct(&f, &ctx);
            assert!(
                matches!(report.outcome, Outcome::Suggestion(_)),
                "{typo}: {:?}",
                report.outcome
            );
            assert_eq!(scripts(&report.outcome)[0], fixed);
        }
    }

    #[test]
    fn misspelled_app_names_continue_into_native_completion() {
        let fake = fake_aws();
        let ctx = context(&fake).with_which("asw", None);
        let mut f = failure("asw ec2 describ-instances");
        f.exit_status = Some(127);
        let report = correct(&f, &ctx);
        assert_eq!(scripts(&report.outcome)[0], "aws ec2 describe-instances");
    }

    #[test]
    fn error_output_hints_help_apps_without_native_completion() {
        let fake = fake_aws();
        let ctx = context(&fake);
        let mut f = failure("ls --colro=auto");
        f.output = CapturedOutput::Combined {
            text: "ls: unrecognized option '--colro=auto'\nTry 'ls --help' for more information."
                .into(),
            origin: OutputOrigin::ShellLogger,
        };
        assert!(matches!(
            correct(&f, &ctx).outcome,
            Outcome::InsufficientEvidence(_)
        ));
        f.output = CapturedOutput::Combined {
            text: "ls: unrecognized option '--colro=auto'\nDid you mean --color?".into(),
            origin: OutputOrigin::ShellLogger,
        };
        let report = correct(&f, &ctx);
        assert_eq!(scripts(&report.outcome), ["ls --color=auto"]);
        let mut f = failure("git sttus sttus");
        f.output = CapturedOutput::Combined {
            text: "git: 'sttus' is not a git command. See 'git --help'.\n\nThe most similar command is\n\tstatus\n".into(),
            origin: OutputOrigin::ShellLogger,
        };
        let report = correct(&f, &ctx);
        assert_eq!(scripts(&report.outcome)[0], "git status sttus");
    }

    #[test]
    fn printed_hints_cannot_inject_commands() {
        let fake = fake_aws();
        let ctx = context(&fake);
        let mut f = failure("git sttus");
        f.output = CapturedOutput::Combined {
            text: "git: 'sttus' is not a git command.\nDid you mean 'sttus;rm'?".into(),
            origin: OutputOrigin::ShellLogger,
        };
        let report = correct(&f, &ctx);
        for script in scripts(&report.outcome) {
            assert_eq!(parser::parse(script).commands.len(), 1, "{script}");
        }
    }

    #[test]
    fn risky_and_unsupported_cases_have_explicit_outcomes() {
        let fake = fake_aws();
        let ctx = context(&fake);
        let report = correct(&failure("aws ec2 terminat-instances"), &ctx);
        let candidate = &report.outcome.candidates()[0];
        assert_eq!(candidate.script, "aws ec2 terminate-instances");
        assert_eq!(candidate.safety.decision, Decision::Confirm);
        assert!(matches!(
            correct(&failure("[[ -n x ]] && gti status"), &ctx).outcome,
            Outcome::UnsupportedSyntax(_)
        ));
        let unsupported = Context::new(Settings::default(), Shell::Powershell, "fuck".into());
        assert!(matches!(
            correct(&failure("gti status"), &unsupported).outcome,
            Outcome::UnsupportedSyntax(_)
        ));
        let mut f = failure("aws sts get-caller-identity");
        f.output = CapturedOutput::Combined {
            text: "Unable to locate credentials.".into(),
            origin: OutputOrigin::ShellLogger,
        };
        let tree = fs::read_to_string(fake.dir.0.join("bin/tree")).unwrap();
        fs::write(
            fake.dir.0.join("bin/tree"),
            format!("{tree}sts|get-caller-identity\n"),
        )
        .unwrap();
        let report = correct(&f, &ctx);
        assert!(
            matches!(&report.outcome, Outcome::NoCorrection(why) if why.contains("authentication")),
            "{:?}",
            report.outcome
        );
    }

    /// An argcomplete app (gcloud or az flavored) answering on fd 8 from a
    /// tree file, which tests rewrite to install new groups or extensions.
    fn fake_argcomplete(app: &str) -> (Dir, PathBuf) {
        let dir = Dir::new(app);
        let marker = if app == "az" {
            "# azure.cli test double"
        } else {
            ""
        };
        let path = dir.script(
            &format!("sdk/bin/{app}"),
            &format!(
                "#!/bin/sh\n{marker}\n[ \"$_ARGCOMPLETE\" = 1 ] || {{ touch \"$(dirname \"$0\")/ran\"; exit 0; }}\n\
line=${{COMP_LINE#{app} }}\nprefix=${{line##* }}\ncontext=${{line% *}}\n[ \"$context\" = \"$line\" ] && context=\n\
while IFS='|' read -r pattern word; do\n  case $context in\n    $pattern) case $word in \"$prefix\"*) printf '%s\\v' \"$word\" >&8;; esac;;\n  esac\n\
done < \"$(dirname \"$0\")/tree\"\n"
            ),
        );
        fs::create_dir_all(dir.0.join("sdk/lib/googlecloudsdk")).unwrap();
        fs::write(
            dir.0.join("sdk/bin/tree"),
            "|storage\n|vm\nstorage|account\nstorage|blob\nstorage account|list\nstorage account|create\n\
storage account *|--resource-group=\nstorage account *|--verbose\n",
        )
        .unwrap();
        (dir, path)
    }

    #[test]
    fn argcomplete_apps_reflect_newly_installed_groups() {
        for app in ["gcloud", "az"] {
            let (dir, path) = fake_argcomplete(app);
            let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into())
                .with_which(app, Some(path.to_str().unwrap()))
                .with_history::<&str>(&[]);
            let report = correct(
                &failure(&format!("{app} storage acount list --resorce-group=rg")),
                &ctx,
            );
            assert_eq!(
                scripts(&report.outcome)[0],
                format!("{app} storage account list --resource-group=rg"),
                "{app}: {:?}",
                report.notes
            );
            // An extension adds a group: it is accepted and repaired at once.
            let typo = format!("{app} containerap up");
            assert!(!matches!(
                correct(&failure(&typo), &ctx).outcome,
                Outcome::Suggestion(_)
            ));
            let tree = fs::read_to_string(dir.0.join("sdk/bin/tree")).unwrap();
            fs::write(
                dir.0.join("sdk/bin/tree"),
                format!("{tree}|containerapp\ncontainerapp|up\n"),
            )
            .unwrap();
            let report = correct(&failure(&typo), &ctx);
            assert_eq!(
                scripts(&report.outcome)[0],
                format!("{app} containerapp up"),
                "{app}: {:?}",
                report.notes
            );
            assert!(
                !dir.0.join("sdk/bin/ran").exists(),
                "{app} ran an operation"
            );
        }
    }

    #[test]
    fn every_protocol_follows_commands_added_by_an_upgrade() {
        use super::native::{Backend, Flavor};
        let dir = Dir::new("upgrades");
        // Each fake answers from `words` next to it, in its own protocol.
        let cobra = dir.script(
            "cobra/app",
            "#!/bin/sh\n[ \"$1\" = __complete ] || exit 1\ncat \"$(dirname \"$0\")/words\"\necho :4\n",
        );
        let posener = dir.script(
            "posener/app",
            "#!/bin/sh\n[ -n \"$COMP_LINE\" ] || exit 1\ncat \"$(dirname \"$0\")/words\"\n",
        );
        let git = dir.script(
            "git/app",
            "#!/bin/sh\ncase \"$*\" in\n  --list-cmds=main*) cat \"$(dirname \"$0\")/words\";;\n  --list-cmds=builtins) ;;\n  -h) printf 'usage: git [--version]\\n'; exit 129;;\n  *) exit 1;;\nesac\n",
        );
        for (flavor, path) in [
            (Flavor::Cobra, cobra),
            (Flavor::Posener, posener),
            (Flavor::Git, git),
        ] {
            let words = path.parent().unwrap().join("words");
            fs::write(&words, "build\ndeploy\n").unwrap();
            let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into())
                .with_which("app", Some(path.to_str().unwrap()))
                .with_which("man", None)
                .with_history::<&str>(&[]);
            let run = |source: &str| {
                let backend: std::rc::Rc<dyn native::NativeCompletionBackend> =
                    std::rc::Rc::new(Backend::new(flavor, "app", path.clone()));
                correct_with_backends(&failure(source), &ctx, vec![("app".into(), backend)])
            };
            assert!(
                !matches!(run("app releese").outcome, Outcome::Suggestion(_)),
                "{flavor:?}: nothing close yet"
            );
            fs::write(&words, "build\ndeploy\nrelease\n").unwrap();
            let report = run("app releese");
            assert_eq!(
                scripts(&report.outcome),
                ["app release"],
                "{flavor:?}: {:?}",
                report.notes
            );
            assert!(matches!(
                run("app release").outcome,
                Outcome::NoCorrection(_)
            ));
        }
    }

    #[test]
    fn shell_handlers_follow_upgrades_and_fingerprint_the_app_and_registration() {
        use super::native::{Backend, Flavor};
        let dir = Dir::new("shell-upgrades");
        for (shell, flavor) in [
            (Shell::Bash, Flavor::BashFunction),
            (Shell::Fish, Flavor::FishScript),
        ] {
            let name = if shell == Shell::Fish { "fish" } else { "bash" };
            let Some(executable) = crate::utils::which(name) else {
                continue;
            };
            let app = dir.script(
                &format!("{name}/app"),
                &format!(
                    "#!/bin/sh\nprintf ran > {}\n",
                    crate::shlex::quote(dir.0.join("operation-marker").to_str().unwrap())
                ),
            );
            let words = app.parent().unwrap().join("words");
            fs::write(&words, "build\ndeploy\n").unwrap();
            let body = if shell == Shell::Fish {
                format!(
                    "complete -c app -f -a '(cat {})'\n",
                    parser::quote_word_with_dialect(words.to_str().unwrap(), parser::Dialect::Fish)
                )
            } else {
                format!(
                    "_app() {{ COMPREPLY=(); while IFS= read -r word; do COMPREPLY+=(\"$word\"); done < {}; }}\ncomplete -F _app app\n",
                    crate::shlex::quote(words.to_str().unwrap())
                )
            };
            let script = dir.script(&format!("{name}/completion"), &body);
            let backend = || {
                Backend::new(flavor, "app", executable.clone())
                    .with_helper(script.clone())
                    .with_application(app.clone())
            };
            let ctx = Context::new(Settings::default(), shell, "fuck".into())
                .with_which("app", Some(app.to_str().unwrap()))
                .with_which("man", None)
                .with_history::<&str>(&[]);
            let run = |source: &str| {
                correct_with_backends(
                    &failure(source),
                    &ctx,
                    vec![("app".into(), std::rc::Rc::new(backend()))],
                )
            };
            assert!(scripts(&run("app releese").outcome).is_empty(), "{shell:?}");
            fs::write(&words, "build\ndeploy\nrelease\n").unwrap();
            let report = run("app releese");
            assert_eq!(
                scripts(&report.outcome),
                ["app release"],
                "{shell:?}: {:?}",
                report.notes
            );
            assert!(
                matches!(report.outcome, Outcome::Ambiguous(_)),
                "handwritten scripts give partial evidence"
            );
            assert!(matches!(
                run("app release").outcome,
                Outcome::NoCorrection(_)
            ));
            let original = backend().cache_identity();
            fs::write(
                &app,
                format!(
                    "#!/bin/sh\n# upgraded installation\nprintf ran > {}\n",
                    crate::shlex::quote(dir.0.join("operation-marker").to_str().unwrap())
                ),
            )
            .unwrap();
            let upgraded = backend().cache_identity();
            assert_ne!(
                original, upgraded,
                "{shell:?}: app identity matters even when a shell probes it"
            );
            fs::write(&script, format!("{body}\n# changed registration\n")).unwrap();
            assert_ne!(
                upgraded,
                backend().cache_identity(),
                "{shell:?}: registration identity matters"
            );
            assert!(!dir.0.join("operation-marker").exists());
        }
    }

    #[test]
    fn native_text_cannot_inject_operators_or_substitutions() {
        use super::native::{Backend, Flavor};
        let dir = Dir::new("native-injection");
        for shell in [Shell::Bash, Shell::Fish] {
            let ctx = Context::new(Settings::default(), shell, "fuck".into())
                .with_which("app", Some("/usr/bin/app"))
                .with_which("man", None)
                .with_history::<&str>(&[]);
            for (typed, value) in [
                ("buidl;touch_marker", "build;touch_marker"),
                ("buidl$(touch_marker)", "build$(touch_marker)"),
                ("buidl(touch_marker)", "build(touch_marker)"),
                ("buidl'quoted'", "build'quoted'"),
            ] {
                let app = dir.script(
                    "app",
                    &format!(
                        "#!/bin/sh\nprintf '%s\\n' {} ':4'\n",
                        crate::shlex::quote(value)
                    ),
                );
                let backend = Backend::new(Flavor::Cobra, "app", app);
                let dialect = parser::Dialect::for_shell(shell).unwrap();
                let source = format!("app {}", parser::quote_word_with_dialect(typed, dialect));
                let report = correct_with_backends(
                    &failure(&source),
                    &ctx,
                    vec![("app".into(), std::rc::Rc::new(backend))],
                );
                let candidates = scripts(&report.outcome);
                assert_eq!(
                    candidates.len(),
                    1,
                    "{shell:?}: {value}: {:?}",
                    report.notes
                );
                let parsed = parser::parse_with_dialect(candidates[0], dialect);
                assert!(parsed.is_fully_supported());
                assert_eq!(parsed.commands.len(), 1);
                assert_eq!(parsed.commands[0].words[1].literal(), Some(value));
                assert!(parsed.commands[0].redirections.is_empty());
                assert_eq!(
                    safety::assess(
                        &parser::parse_with_dialect(&source, dialect),
                        candidates[0],
                        &[]
                    )
                    .decision,
                    Decision::Allow
                );
            }
        }
    }

    #[test]
    fn cached_answers_are_reused_but_never_hide_new_commands() {
        let fake = fake_aws();
        let ctx = context(&fake);
        let first = correct(&failure("aws ec2 describe-instances --dry-run"), &ctx);
        assert!(matches!(first.outcome, Outcome::NoCorrection(_)));
        assert!(first.probes > 0);
        let again = correct(&failure("aws ec2 describe-instances --dry-run"), &ctx);
        assert_eq!(again.probes, 0, "valid words are confirmed from the cache");
        let tree = fs::read_to_string(fake.dir.0.join("bin/tree")).unwrap();
        fs::write(
            fake.dir.0.join("bin/tree"),
            format!("{tree}ec2|describe-hosts\n"),
        )
        .unwrap();
        let report = correct(&failure("aws ec2 describe-hosts"), &ctx);
        assert!(
            matches!(report.outcome, Outcome::NoCorrection(_)),
            "a stale cached list must not reject a new command: {:?}",
            report.outcome
        );
        assert_eq!(report.probes, 1);
    }

    #[test]
    fn option_values_are_checked_against_the_apps_list() {
        let fake = fake_aws();
        let ctx = context(&fake);
        let report = correct(
            &failure("aws ec2 describe-instances --region eu-wst-1 --output=jsn"),
            &ctx,
        );
        assert_eq!(
            scripts(&report.outcome)[0],
            "aws ec2 describe-instances --region eu-west-1 --output=json",
            "{:?}",
            report.notes
        );
        assert!(
            matches!(report.outcome, Outcome::Ambiguous(_)),
            "value lists are partial: without output, ask"
        );
        let mut f = failure("aws ec2 describe-instances --output jsn");
        f.output = CapturedOutput::Combined {
            text: "aws: [ERROR]: argument --output: Found invalid choice 'jsn'".into(),
            origin: OutputOrigin::ShellLogger,
        };
        let report = correct(&f, &ctx);
        assert!(
            matches!(report.outcome, Outcome::Suggestion(_)),
            "{:?}",
            report.outcome
        );
        assert_eq!(
            scripts(&report.outcome)[0],
            "aws ec2 describe-instances --output json"
        );
        for valid in [
            "aws ec2 describe-instances --region us-east-1",
            "aws ec2 describe-instances --region ap-south-9 --dry-run",
            "aws ec2 describe-instances --instance-ids i-0abc",
        ] {
            assert!(
                matches!(
                    correct(&failure(valid), &ctx).outcome,
                    Outcome::NoCorrection(_)
                ),
                "{valid}"
            );
        }
    }

    #[test]
    fn resource_names_are_looked_up_only_with_network_completion_and_need_approval() {
        let fake = fake_aws();
        fs::write(
            fake.dir.0.join("bin/resources"),
            "ec2 *-instances* --instance-ids|i-0abc123def\nec2 *-instances* --instance-ids|i-0fff000aaa\n",
        )
        .unwrap();
        let source = "aws ec2 describe-instances --instance-ids i-0abc123dfe";
        let offline = correct(&failure(source), &context(&fake));
        assert!(
            scripts(&offline.outcome)
                .iter()
                .all(|s| !s.contains("i-0abc123def")),
            "offline, resource names are never looked up: {:?}",
            offline.outcome
        );

        let settings = Settings {
            network_completion: true,
            ..Settings::default()
        };
        let ctx = context_with(&fake, settings);
        let report = correct(&failure(source), &ctx);
        let candidate = report
            .outcome
            .candidates()
            .iter()
            .find(|c| c.script == "aws ec2 describe-instances --instance-ids i-0abc123def")
            .unwrap_or_else(|| panic!("{:?} {:?}", report.outcome, report.notes));
        assert!(candidate.edits[0].resource);
        assert_eq!(candidate.safety.decision, Decision::Confirm);
        assert!(
            candidate
                .safety
                .reasons
                .iter()
                .any(|r| r.contains("resource `i-0abc123def`")),
            "{:?}",
            candidate.safety.reasons
        );

        // The app's fixed values stay ordinary words with lookups on.
        let report = correct(
            &failure("aws ec2 describe-instances --region eu-wst-1"),
            &ctx,
        );
        let candidate = &report.outcome.candidates()[0];
        assert_eq!(
            candidate.script,
            "aws ec2 describe-instances --region eu-west-1"
        );
        assert!(!candidate.edits[0].resource);
        assert_eq!(candidate.safety.decision, Decision::Allow);
        assert!(!fake.dir.0.join("bin/ran").exists());
    }

    #[test]
    fn global_options_before_subcommands_keep_their_values() {
        let fake = fake_aws();
        let ctx = context(&fake);
        let report = correct(
            &failure("aws --region eu-west-1 ec2 describ-instances"),
            &ctx,
        );
        assert_eq!(
            scripts(&report.outcome)[0],
            "aws --region eu-west-1 ec2 describe-instances"
        );
    }

    #[test]
    fn a_broken_completer_is_not_mistaken_for_valid_input() {
        let fake = fake_aws();
        fs::remove_file(fake.dir.0.join("bin/tree")).unwrap();
        let ctx = context(&fake);
        let report = correct(&failure("aws ec2 describ-instances"), &ctx);
        assert!(
            matches!(report.outcome, Outcome::InsufficientEvidence(_)),
            "{:?}",
            report.outcome
        );
        assert!(report.notes.iter().any(|n| n.contains("failed")));
    }

    #[test]
    fn disabled_sources_are_reported() {
        let fake = fake_aws();
        let settings = Settings {
            disabled_sources: vec!["native".into()],
            ..Settings::default()
        };
        let ctx = Context::new(settings, Shell::Bash, "fuck".into())
            .with_which("aws", Some(fake.aws.to_str().unwrap()))
            .with_history::<&str>(&[]);
        let report = correct(&failure("aws ec2 describ-instances"), &ctx);
        assert!(report.outcome.candidates().is_empty());
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("native source is disabled"))
        );
        assert_eq!(report.probes, 0);
    }

    #[test]
    fn pipeline_status_narrows_the_failed_stage() {
        let fake = fake_aws();
        let ctx = context(&fake).with_which("gerp", None);
        let mut f = failure("gti log | gerp x");
        f.pipe_status = Some(vec![0, 127]);
        f.exit_status = Some(127);
        let report = correct(&f, &ctx);
        for candidate in report.outcome.candidates() {
            assert!(
                candidate.script.starts_with("gti log | "),
                "{}",
                candidate.script
            );
        }
    }
}
