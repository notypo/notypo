//! Entry points, port of `thefuck/entrypoints/*`.

use crate::args::{self, Args};
use crate::corrector::Corrector;
use crate::engine::safety::{self, Decision, DeclaredEffect};
use crate::engine::{self, CapturedOutput, FailureContext, Outcome, OutputOrigin, Report};
use crate::settings::Settings;
#[cfg(unix)]
use crate::shell_logger;
use crate::shells::{DEFAULT_ALIASES, Shell};
use crate::types::{Command, Context, CorrectedCommand};
use crate::ui::{Choice, Suggestions};
use crate::{difflib, logs, output_readers, ui, utils};
use std::env;
use std::fmt::Write as _;
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const PROG: &str = env!("CARGO_PKG_NAME");

/// Runs the CLI. Invoked as `fuck` (a symlink), it behaves like thefuck's
/// `fuck` entry point and explains how to install the alias.
pub fn run() -> ExitCode {
    crate::terminal::init_output();
    let argv: Vec<String> = env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let invoked_as = argv
        .first()
        .and_then(|a| std::path::Path::new(a).file_stem())
        .and_then(|a| a.to_str())
        .unwrap_or(PROG);
    if invoked_as == "fuck" {
        not_configured();
        return ExitCode::SUCCESS;
    }
    main(argv.get(1..).unwrap_or_default())
}

/// `entrypoints.main.main`.
pub fn main(argv: &[String]) -> ExitCode {
    let args = match args::parse(argv, &utils::get_alias()) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{}{PROG}: error: {e}", args::usage(PROG));
            return ExitCode::from(2);
        }
    };
    if args.help {
        eprint!("{}", args::help(PROG));
    } else if args.version {
        eprintln!("{PROG} {}", env!("CARGO_PKG_VERSION"));
    } else if let Some(alias) = &args.alias {
        // Must come before the TF_HISTORY check (thefuck issue #921).
        print_alias(&args, alias);
    } else if args.force_command.is_some()
        || !args.command.is_empty()
        || env::var_os("TF_HISTORY").is_some()
    {
        return fix_command(&args);
    } else if let Some(path) = &args.shell_logger {
        #[cfg(unix)]
        return match shell_logger::log_shell(path.as_ref()) {
            Ok(status) => {
                use std::os::unix::process::ExitStatusExt;
                ExitCode::from(
                    status
                        .code()
                        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
                        .clamp(0, 255) as u8,
                )
            }
            Err(error) => {
                logs::warn(&format!("Can't start shell logger: {error}"));
                ExitCode::FAILURE
            }
        };
        #[cfg(windows)]
        {
            let _ = path;
            logs::warn("Shell logging requires a Unix PTY");
            return ExitCode::FAILURE;
        }
    } else {
        eprint!("{}", args::usage(PROG));
    }
    ExitCode::SUCCESS
}

fn init_settings(args: Option<&Args>) -> Settings {
    let settings = Settings::init(args);
    logs::configure(settings.no_colors, settings.debug);
    settings
}

fn current_exe() -> String {
    env::current_exe()
        .ok()
        .and_then(|p| p.into_os_string().into_string().ok())
        .unwrap_or_else(|| PROG.to_owned())
}

/// `entrypoints.alias.print_alias`.
fn print_alias(args: &Args, alias: &str) {
    let settings = init_settings(Some(args));
    let shell = Shell::detect();
    let aliases = if args.default_aliases {
        DEFAULT_ALIASES
    } else {
        std::slice::from_ref(&alias)
    };
    let exe = current_exe();
    let alias = if args.instant_mode {
        shell.instant_mode_aliases(aliases, &exe, settings.alter_history)
    } else {
        shell.app_aliases(aliases, &exe, settings.alter_history)
    };
    println!("{alias}");
}

/// `_get_raw_command`: explicit arguments, or the newest `TF_HISTORY`
/// entry that isn't (a typo of) our own alias. The flag tells whether the
/// exit status the shell function captured belongs to that command: only
/// when nothing but this correction call ran after it.
fn raw_command(args: &Args, ctx: &Context) -> (Vec<String>, bool) {
    if let Some(command) = &args.force_command {
        return (vec![command.clone()], false);
    }
    // Zsh's `fc -ln -10` omits a multiline paste while it is executing.
    // The shell function supplies that event separately, with real newlines.
    if ctx.shell == Shell::Zsh
        && let Ok(current) = env::var("NOTYPO_CURRENT_COMMAND")
        && let Some(command) = command_before_alias(&current, ctx)
    {
        return (vec![command], true);
    }
    let history = env::var("TF_HISTORY").unwrap_or_default();
    if history.is_empty() {
        return (args.command.clone(), false);
    }
    let is_executable = |name: &str| {
        // Our alias is never among the executables (it's filtered out), so
        // skip listing $PATH for the common `fuck` line.
        !(name == ctx.alias && ["fuck", "thefuck"].contains(&name))
            && ctx.executables().iter().any(|e| e == name)
    };
    let mut later_calls = 0;
    let mut skipped = false;
    for line in history
        .split('\n')
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        if is_alias_invocation(line, ctx) {
            later_calls += 1;
        } else if difflib::ratio(&ctx.alias, line) < 0.5 || is_executable(line) {
            return (vec![line.to_owned()], later_calls <= 1 && !skipped);
        } else {
            skipped = true;
        }
    }
    (Vec::new(), false)
}

fn is_alias_invocation(script: &str, ctx: &Context) -> bool {
    ctx.shell
        .split_command(script)
        .first()
        .is_some_and(|name| name == &ctx.alias || DEFAULT_ALIASES.contains(&name.as_str()))
}

/// Selects the line immediately before the correction call in a pasted block.
/// Stop at the call: later lines in the paste have not executed yet.
fn command_before_alias(current: &str, ctx: &Context) -> Option<String> {
    let mut previous = None;
    for line in current.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if is_alias_invocation(line, ctx) {
            return previous.map(str::to_owned);
        }
        previous = Some(line);
    }
    None
}

/// `entrypoints.fix_command.fix_command`.
pub fn fix_command(args: &Args) -> ExitCode {
    let started = Instant::now();
    let settings = init_settings(Some(args));
    logs::debug(format_args!("Run with settings: {settings:#?}"));
    let ctx = Context::new(settings, Shell::detect(), utils::get_alias());

    let (raw, status_applies) = raw_command(args, &ctx);
    let script = utils::format_raw_script(&raw);
    if script.is_empty() {
        logs::debug("Empty command, nothing to do");
        return ExitCode::SUCCESS;
    }
    let expanded = ctx.shell.from_shell(&script);
    let code = fix_with_engine(args, &ctx, &script, &expanded, status_applies);
    logs::debug(format_args!("Total took: {:?}", started.elapsed()));
    code
}

/// Captured output only (the command is rerun just when
/// `replay_for_diagnosis` allows it and the safety gate agrees), then the
/// engine's candidates and the legacy rules' suggestions, all gated.
fn fix_with_engine(
    args: &Args,
    ctx: &Context,
    script: &str,
    expanded: &str,
    status_applies: bool,
) -> ExitCode {
    let (exit_status, pipe_status) = if status_applies {
        engine::status_from_env()
    } else {
        (None, None)
    };
    let mut output = match output_readers::captured_output(script, &ctx.settings) {
        Some((text, origin)) => CapturedOutput::Combined { text, origin },
        None => CapturedOutput::Unknown,
    };
    let inspecting = args.explain || args.json;
    if output == CapturedOutput::Unknown && ctx.settings.replay_for_diagnosis && !inspecting {
        let gate = safety::assess_replay(expanded);
        if gate.decision == Decision::Allow {
            if let Some(text) = output_readers::get_output(script, expanded, &ctx.settings) {
                output = CapturedOutput::Combined {
                    text,
                    origin: OutputOrigin::Replay,
                };
            }
        } else {
            logs::debug(format_args!(
                "Not replaying the command: {}",
                gate.reasons.join("; ")
            ));
        }
    }
    let failure = FailureContext {
        source: expanded.to_owned(),
        cwd: env::current_dir().ok(),
        exit_status,
        pipe_status,
        output,
    };
    let report = logs::debug_time("Structured engine", || engine::correct(&failure, ctx));
    for note in &report.notes {
        logs::debug(note);
    }
    logs::debug(format_args!(
        "Outcome: {} ({} probes)",
        report.outcome.describe(),
        report.probes
    ));
    if args.json {
        println!("{}", report_json(&failure, &report));
        return ExitCode::SUCCESS;
    }
    if args.explain {
        eprint!("{}", explain(&failure, &report));
        return ExitCode::SUCCESS;
    }
    let command = Command::new(expanded, failure.output.diagnostic_text(), ctx);
    let mut suggestions = EngineSuggestions::new(&report, &command);
    match ui::select_command(
        &mut suggestions,
        ctx.settings.require_confirmation,
        &ctx.alias,
    ) {
        Some(selected) => match revalidate(ctx, &report, &selected) {
            Ok(()) => {
                run_corrected(&command, &selected);
                ExitCode::SUCCESS
            }
            Err(why) => {
                logs::failed(&format!("Not running `{}`: {why}", selected.script));
                ExitCode::FAILURE
            }
        },
        None => ExitCode::FAILURE,
    }
}

/// Rechecks, right before the shell runs it, that the programs a selected
/// engine correction introduced still resolve. This narrows (but can't
/// close) the window between discovery and execution.
fn revalidate(ctx: &Context, report: &Report, selected: &Choice) -> Result<(), String> {
    let Some(candidate) = report
        .outcome
        .candidates()
        .iter()
        .find(|c| c.script == selected.script)
    else {
        return Ok(());
    };
    for edit in &candidate.edits {
        if edit.role != engine::TokenRole::Executable
            || ctx.shell.get_builtin_commands().contains(&edit.to.as_str())
            || ctx.shell.get_aliases().contains_key(&edit.to)
            || ctx.shell.get_functions().contains(&edit.to)
        {
            continue;
        }
        // `which` is memoized for the run; stat the file it found again.
        if !ctx
            .which(&edit.to)
            .is_some_and(|path| utils::is_executable_file(&path))
        {
            return Err(format!("{} is no longer installed", edit.to));
        }
    }
    Ok(())
}

/// Engine candidates and legacy rule suggestions, each with the reason it
/// is offered and what needs approval. A decisive engine suggestion comes
/// first; otherwise a matching rule does (rules encode specific fixes the
/// engine can't always see without output), followed by the engine's.
/// Rules are evaluated only when needed.
struct EngineSuggestions<'c, 'a> {
    native: Vec<Choice>,
    /// The engine's first candidate was chosen on its own evidence.
    decisive: bool,
    /// A rule's suggestion was offered first.
    led_by_legacy: bool,
    legacy: Option<Corrector<'c, 'a>>,
    /// The legacy suggestion already taken from the corrector.
    legacy_first: Option<CorrectedCommand>,
    legacy_done: bool,
    pending: Vec<Choice>,
    original: engine::parser::Script,
}

impl<'c, 'a> EngineSuggestions<'c, 'a> {
    fn new(report: &Report, command: &'c Command<'a>) -> Self {
        let candidates = report.outcome.candidates();
        let ambiguous = matches!(report.outcome, Outcome::Ambiguous(_));
        let native = candidates
            .iter()
            .enumerate()
            .map(|(n, c)| {
                let mut concerns = Vec::new();
                if c.safety.decision == Decision::Confirm {
                    concerns.extend(c.safety.reasons.iter().cloned());
                }
                if n == 0 && ambiguous {
                    concerns.push(if c.weak {
                        "the shell did not report the command as missing".into()
                    } else if candidates.len() > 1 {
                        format!("{} close alternatives", candidates.len() - 1)
                    } else {
                        "low confidence".into()
                    });
                }
                Choice {
                    script: c.script.clone(),
                    side_effect: None,
                    reason: Some(c.reason()),
                    concerns,
                }
            })
            .collect();
        let legacy = command
            .settings()
            .is_source_enabled(engine::Source::LegacyRule.setting_name())
            .then(|| Corrector::new(command));
        EngineSuggestions {
            native,
            decisive: matches!(report.outcome, Outcome::Suggestion(_)),
            led_by_legacy: false,
            legacy,
            legacy_first: None,
            legacy_done: false,
            pending: Vec::new(),
            original: engine::parser::parse(&command.script),
        }
    }

    /// The safety gate for a legacy rule's whole-command replacement.
    fn gate(&self, corrected: CorrectedCommand) -> Option<Choice> {
        let effects: Vec<DeclaredEffect> = corrected
            .side_effect
            .map(|_| DeclaredEffect::for_rule(corrected.rule))
            .into_iter()
            .collect();
        let assessment = safety::assess(&self.original, &corrected.script, &effects);
        if assessment.decision == Decision::Refuse {
            logs::debug(format_args!(
                "Refused {}: {}",
                corrected.script,
                assessment.reasons.join("; ")
            ));
            return None;
        }
        Some(Choice {
            reason: Some(format!("legacy rule {}", corrected.rule)),
            concerns: if assessment.decision == Decision::Confirm {
                assessment.reasons
            } else {
                Vec::new()
            },
            script: corrected.script,
            side_effect: corrected.side_effect,
        })
    }

    /// Every legacy suggestion not taken yet.
    fn legacy_rest(&mut self) -> Vec<CorrectedCommand> {
        if self.legacy_done {
            return Vec::new();
        }
        self.legacy_done = true;
        let Some(legacy) = self.legacy.as_mut() else {
            return Vec::new();
        };
        match self.legacy_first.take() {
            Some(first) => legacy.rest(&first),
            None => match legacy.first() {
                Some(first) => {
                    let rest = legacy.rest(&first);
                    std::iter::once(first).chain(rest).collect()
                }
                None => Vec::new(),
            },
        }
    }
}

impl EngineSuggestions<'_, '_> {
    /// The first legacy suggestion the safety gate doesn't refuse.
    fn legacy_first(&mut self) -> Option<Choice> {
        let first = self.legacy.as_mut()?.first()?;
        self.legacy_first = Some(first.clone());
        if let Some(choice) = self.gate(first) {
            return Some(choice);
        }
        let mut acceptable = self
            .legacy_rest()
            .into_iter()
            .filter_map(|c| self.gate(c))
            .collect::<Vec<_>>()
            .into_iter();
        let choice = acceptable.next()?;
        self.pending = acceptable.collect();
        Some(choice)
    }
}

impl Suggestions for EngineSuggestions<'_, '_> {
    fn first(&mut self) -> Option<Choice> {
        if self.decisive
            && let Some(first) = self.native.first()
        {
            return Some(first.clone());
        }
        if let Some(choice) = self.legacy_first() {
            self.led_by_legacy = true;
            return Some(choice);
        }
        self.native.first().cloned()
    }

    fn rest(&mut self, first: &Choice) -> Vec<Choice> {
        let skip = usize::from(!self.led_by_legacy);
        let mut all: Vec<Choice> = self.native.iter().skip(skip).cloned().collect();
        all.append(&mut self.pending);
        let legacy: Vec<Choice> = self
            .legacy_rest()
            .into_iter()
            .filter_map(|c| self.gate(c))
            .collect();
        all.extend(legacy);
        let mut unique: Vec<Choice> = Vec::with_capacity(all.len());
        for choice in all {
            if choice != *first && !unique.contains(&choice) {
                unique.push(choice);
            }
        }
        unique
    }
}

/// The `--explain` report: what was known, what each source found, and why
/// candidates were ranked and gated as they were.
fn explain(failure: &FailureContext, report: &Report) -> String {
    let mut out = String::new();
    let unknown = || "unknown".to_owned();
    let _ = writeln!(out, "command:     {}", failure.source);
    let _ = writeln!(
        out,
        "exit status: {}",
        failure.exit_status.map_or_else(unknown, |s| s.to_string())
    );
    if let Some(pipe) = &failure.pipe_status {
        let _ = writeln!(out, "pipe status: {pipe:?}");
    }
    let _ = writeln!(
        out,
        "output:      {}",
        match &failure.output {
            CapturedOutput::Unknown => "not captured (the command is not rerun)".to_owned(),
            CapturedOutput::Combined { origin, .. } =>
                format!("{origin:?} (stdout and stderr combined)"),
            CapturedOutput::Separate { .. } => "stdout and stderr".to_owned(),
        }
    );
    for suspicion in &report.suspicions {
        let _ = writeln!(
            out,
            "suspect:     {:?} `{}` (strength {:.2})",
            suspicion.role, suspicion.token, suspicion.score
        );
        for evidence in &suspicion.evidence {
            let _ = writeln!(
                out,
                "   evidence ({}): {}",
                evidence.source.setting_name(),
                evidence.detail
            );
        }
    }
    let _ = writeln!(out, "outcome:     {}", report.outcome.describe());
    let listed = match &report.outcome {
        Outcome::Unsafe(c) => c.as_slice(),
        other => other.candidates(),
    };
    for (n, candidate) in listed.iter().enumerate() {
        let _ = writeln!(
            out,
            "{}. {}  [score {:.2}, {:?}]",
            n + 1,
            candidate.script,
            candidate.score,
            candidate.safety.decision
        );
        for edit in &candidate.edits {
            let _ = writeln!(
                out,
                "   {:?} `{}` → `{}` via {} (similarity {:.2}, distance {}, hint {:.2})",
                edit.role,
                edit.from,
                edit.to,
                edit.via,
                edit.score.similarity,
                edit.score.distance,
                edit.score.hint
            );
        }
        for evidence in &candidate.evidence {
            let _ = writeln!(
                out,
                "   evidence ({}): {}",
                evidence.source.setting_name(),
                evidence.detail
            );
        }
        for reason in &candidate.safety.reasons {
            let _ = writeln!(out, "   safety: {reason}");
        }
    }
    for note in &report.notes {
        let _ = writeln!(out, "note: {note}");
    }
    let _ = writeln!(out, "probes:      {}", report.probes);
    out
}

/// The `--json` report. Field names are part of the CLI's interface.
fn report_json(failure: &FailureContext, report: &Report) -> serde_json::Value {
    use serde_json::json;
    let (kind, listed) = match &report.outcome {
        Outcome::Suggestion(c) => ("suggestion", c.as_slice()),
        Outcome::Ambiguous(c) => ("ambiguous", c.as_slice()),
        Outcome::Unsafe(c) => ("unsafe", c.as_slice()),
        Outcome::UnsupportedSyntax(_) => ("unsupported_syntax", &[][..]),
        Outcome::InsufficientEvidence(_) => ("insufficient_evidence", &[][..]),
        Outcome::NoCorrection(_) => ("no_correction", &[][..]),
    };
    let evidence = |e: &[engine::Evidence]| -> Vec<serde_json::Value> {
        e.iter()
            .map(|e| json!({"source": e.source.setting_name(), "detail": e.detail}))
            .collect()
    };
    json!({
        "command": failure.source,
        "exit_status": failure.exit_status,
        "pipe_status": failure.pipe_status,
        "output": match &failure.output {
            CapturedOutput::Unknown => serde_json::Value::Null,
            CapturedOutput::Combined { origin, .. } => json!({"streams": "combined", "origin": format!("{origin:?}")}),
            CapturedOutput::Separate { .. } => json!({"streams": "separate"}),
        },
        "outcome": {"kind": kind, "description": report.outcome.describe()},
        "candidates": listed.iter().map(|c| json!({
            "command": c.script,
            "score": c.score,
            "weak": c.weak,
            "safety": {
                "decision": format!("{:?}", c.safety.decision).to_lowercase(),
                "reasons": c.safety.reasons,
            },
            "edits": c.edits.iter().map(|e| json!({
                "role": format!("{:?}", e.role),
                "from": e.from,
                "to": e.to,
                "via": e.via,
                "start": e.span.start,
                "end": e.span.end,
                "similarity": e.score.similarity,
                "distance": e.score.distance,
            })).collect::<Vec<_>>(),
            "evidence": evidence(&c.evidence),
        })).collect::<Vec<_>>(),
        "suspicions": report.suspicions.iter().map(|s| json!({
            "role": format!("{:?}", s.role),
            "token": s.token,
            "strength": s.score,
            "evidence": evidence(&s.evidence),
        })).collect::<Vec<_>>(),
        "notes": report.notes,
        "probes": report.probes,
    })
}

/// `CorrectedCommand.run`: side effect, history, then print for the alias to eval.
fn run_corrected(old: &Command, corrected: &Choice) {
    let ctx = old.ctx();
    if let Some(side_effect) = corrected.side_effect {
        side_effect(old, &corrected.script);
    }
    if ctx.settings.alter_history {
        ctx.shell.put_to_history(&corrected.script);
    }
    let script = if ctx.settings.repeat {
        let debug = if ctx.settings.debug { "--debug " } else { "" };
        let again = format!(
            "{} --repeat {debug}--force-command {}",
            ctx.alias,
            ctx.shell.quote(&corrected.script)
        );
        ctx.shell.or_(&[&corrected.script, &again])
    } else {
        corrected.script.clone()
    };
    print!("{script}");
}

/// `entrypoints.not_configured.main`: explains how to set up the alias on
/// the first run, and sets it up when `fuck` is run again right after.
fn not_configured() {
    init_settings(None);
    let shell = Shell::detect();
    let details = shell.how_to_configure(PROG);
    if let Some(details) = details.as_ref().filter(|d| d.can_configure_automatically) {
        let config = utils::expand_user(&details.path);
        if std::fs::read_to_string(&config).is_ok_and(|text| text.contains(&details.content)) {
            logs::already_configured(details);
            return;
        }
        if tracker::is_second_run(shell) {
            let appended = std::fs::OpenOptions::new()
                .append(true)
                .open(&config)
                .and_then(|mut f| {
                    std::io::Write::write_all(&mut f, format!("\n{}\n", details.content).as_bytes())
                });
            if appended.is_ok() {
                logs::configured_successfully(details);
                return;
            }
        }
        tracker::record_first_run();
    }
    logs::how_to_configure_alias(details.as_ref());
}

/// Remembers the shell that ran an unconfigured `fuck` (thefuck keeps a
/// JSON file in the temp dir for this).
mod tracker {
    use super::*;
    use std::path::PathBuf;

    const CONFIGURATION_TIMEOUT: f64 = 60.0;

    fn path() -> PathBuf {
        let user = env::var("USER").unwrap_or_default();
        env::temp_dir().join(format!("thefuck.last_not_configured_run_{user}"))
    }

    fn now() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64())
    }

    pub fn record_first_run() {
        let _ = std::fs::write(
            path(),
            format!("{} {}", crate::platform::parent_id(), now()),
        );
    }

    pub fn is_second_run(shell: Shell) -> bool {
        let Ok(text) = std::fs::read_to_string(path()) else {
            return false;
        };
        let mut fields = text.split_whitespace();
        let pid = fields.next().and_then(|p| p.parse::<u32>().ok());
        let time = fields
            .next()
            .and_then(|t| t.parse::<f64>().ok())
            .unwrap_or(0.0);
        if pid != Some(crate::platform::parent_id()) {
            return false;
        }
        shell
            .get_history(None)
            .last()
            .is_some_and(|last| last == "fuck")
            || now() - time < CONFIGURATION_TIMEOUT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_failed_command_from_pasted_zsh_event() {
        let ctx = Context::new(Settings::default(), Shell::Zsh, "fuck".into());
        let paste = "eval \"$(./target/release/notypo --alias)\"\ngit sttus\nfuck";
        assert_eq!(
            command_before_alias(paste, &ctx).as_deref(),
            Some("git sttus")
        );
        assert_eq!(
            command_before_alias(
                "  git sttus\n\n# correct the typo\n  fuck --debug --yes\nmkdir future/dir",
                &ctx,
            )
            .as_deref(),
            Some("git sttus"),
            "commands after the alias call have not executed yet"
        );
        assert!(command_before_alias("fuck --debug --yes", &ctx).is_none());
        assert!(command_before_alias("git sttus", &ctx).is_none());
    }
}
