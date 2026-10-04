//! Entry points, port of `thefuck/entrypoints/*`.

use crate::args::{self, Args};
use crate::corrector::Corrector;
use crate::settings::Settings;
#[cfg(unix)]
use crate::shell_logger;
use crate::shells::{DEFAULT_ALIASES, Shell};
use crate::types::{Command, Context, CorrectedCommand};
use crate::{difflib, logs, output_readers, ui, utils};
use std::env;
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
/// entry that isn't (a typo of) our own alias.
fn raw_command(args: &Args, ctx: &Context) -> Vec<String> {
    if let Some(command) = &args.force_command {
        return vec![command.clone()];
    }
    // Zsh's `fc -ln -10` omits a multiline paste while it is executing.
    // The shell function supplies that event separately, with real newlines.
    if ctx.shell == Shell::Zsh
        && let Ok(current) = env::var("NOTYPO_CURRENT_COMMAND")
        && let Some(command) = command_before_alias(&current, ctx)
    {
        return vec![command];
    }
    let history = env::var("TF_HISTORY").unwrap_or_default();
    if history.is_empty() {
        return args.command.clone();
    }
    let is_executable = |name: &str| {
        // Our alias is never among the executables (it's filtered out), so
        // skip listing $PATH for the common `fuck` line.
        !(name == ctx.alias && ["fuck", "thefuck"].contains(&name))
            && ctx.executables().iter().any(|e| e == name)
    };
    history
        .split('\n')
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter(|line| !is_alias_invocation(line, ctx))
        .find(|line| difflib::ratio(&ctx.alias, line) < 0.5 || is_executable(line))
        .map(|line| vec![line.to_owned()])
        .unwrap_or_default()
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

    let script = utils::format_raw_script(&raw_command(args, &ctx));
    if script.is_empty() {
        logs::debug("Empty command, nothing to do");
        return ExitCode::SUCCESS;
    }
    let expanded = ctx.shell.from_shell(&script);
    let output = output_readers::get_output(&script, &expanded, &ctx.settings);
    let command = Command::new(expanded, output.as_deref(), &ctx);

    let mut corrector = Corrector::new(&command);
    let code = match ui::select_command(
        &mut corrector,
        ctx.settings.require_confirmation,
        &ctx.alias,
    ) {
        Some(selected) => {
            run_corrected(&command, &selected);
            ExitCode::SUCCESS
        }
        None => ExitCode::FAILURE,
    };
    logs::debug(format_args!("Total took: {:?}", started.elapsed()));
    code
}

/// `CorrectedCommand.run`: side effect, history, then print for the alias to eval.
fn run_corrected(old: &Command, corrected: &CorrectedCommand) {
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
