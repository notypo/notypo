//! Port of `thefuck/types.py`, plus the per-run [`Context`] that replaces
//! the Python version's module-level `settings` / `shell` globals and its
//! `@memoize` caches.

use crate::settings::Settings;
use crate::shells::{PowerShellCommand, Shell};
use crate::{path_index, utils};
use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;

/// Everything a rule may consult besides the command itself.
#[derive(Debug)]
pub struct Context {
    pub settings: Settings,
    pub shell: Shell,
    /// `get_alias()`: `TF_ALIAS`, or `fuck`.
    pub alias: String,
    executables: OnceLock<Vec<String>>,
    history: OnceLock<Vec<String>>,
    recent_history: OnceLock<Vec<String>>,
    which_overrides: HashMap<String, Option<PathBuf>>,
    /// Answer `which` only from this listing (`/listing/bin/<name>`).
    which_listing: Option<HashSet<String>>,
    /// Shell aliases in place of the parent shell's (tests).
    alias_overrides: Option<HashMap<String, String>>,
    /// PowerShell's command resolutions in place of the session's (tests).
    powershell_overrides: Option<HashMap<String, PowerShellCommand>>,
}

/// Names `get_all_executables` never offers: our own entry points.
const ENTRY_POINTS: &[&str] = &["thefuck", "fuck", "typo", "notypo"];

impl Context {
    pub fn new(settings: Settings, shell: Shell, alias: String) -> Context {
        Context {
            settings,
            shell,
            alias,
            executables: OnceLock::new(),
            history: OnceLock::new(),
            recent_history: OnceLock::new(),
            which_overrides: HashMap::new(),
            which_listing: None,
            alias_overrides: None,
            powershell_overrides: None,
        }
    }

    /// Fixes the shell's aliases (tests).
    pub fn with_aliases(mut self, pairs: &[(&str, &str)]) -> Context {
        self.alias_overrides = Some(
            pairs
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
        );
        self
    }

    /// Fixes how the PowerShell session resolved command names (tests):
    /// lines of `name<TAB>kind<TAB>target kind<TAB>target`.
    pub fn with_powershell_commands(mut self, report: &str) -> Context {
        self.powershell_overrides = Some(crate::shells::parse_powershell_commands(report));
        self
    }

    /// How the parent PowerShell session resolved the failed line's command
    /// names, keyed by lowercased name; empty for other shells.
    pub fn powershell_commands(&self) -> &HashMap<String, PowerShellCommand> {
        self.powershell_overrides
            .as_ref()
            .unwrap_or_else(|| self.shell.get_powershell_commands())
    }

    /// The parent shell's aliases (`TF_SHELL_ALIASES`, or tcsh's own list).
    pub fn aliases(&self) -> &HashMap<String, String> {
        self.alias_overrides
            .as_ref()
            .unwrap_or_else(|| self.shell.get_aliases())
    }

    /// Fixes the executables list instead of reading `$PATH` (tests).
    pub fn with_executables<S: AsRef<str>>(self, names: &[S]) -> Context {
        let _ = self
            .executables
            .set(names.iter().map(|s| s.as_ref().to_owned()).collect());
        self
    }

    /// Fixes the shell history (tests).
    pub fn with_history<S: AsRef<str>>(self, lines: &[S]) -> Context {
        let _ = self
            .history
            .set(lines.iter().map(|s| s.as_ref().to_owned()).collect());
        self
    }

    /// Pins the answer of `which(name)` (tests).
    pub fn with_which(mut self, name: &str, path: Option<&str>) -> Context {
        self.which_overrides
            .insert(name.to_owned(), path.map(PathBuf::from));
        self
    }

    /// Makes `$PATH` exactly `names`: `which` and the executables list
    /// answer from it alone (tests and corpora independent of the machine).
    pub fn with_path_listing<S: AsRef<str>>(mut self, names: &[S]) -> Context {
        self.which_listing = Some(names.iter().map(|n| n.as_ref().to_owned()).collect());
        self.with_executables(names)
    }

    pub fn which(&self, name: &str) -> Option<PathBuf> {
        match (self.which_overrides.get(name), &self.which_listing) {
            (Some(answer), _) => answer.clone(),
            (None, Some(listing)) => listing
                .contains(name)
                .then(|| PathBuf::from("/listing/bin").join(name)),
            (None, None) => utils::which(name),
        }
    }

    /// `get_all_executables()`: names in non-excluded `$PATH` directories
    /// (minus our entry points) followed by shell aliases and functions
    /// (minus our alias).
    pub fn executables(&self) -> &[String] {
        self.executables.get_or_init(|| {
            let mut seen = HashSet::new();
            let excluded = &self.settings.excluded_search_path_prefixes;
            let bins = path_index::load()
                .into_iter()
                .filter(|listing| utils::include_path_in_search(&listing.dir, excluded))
                .flat_map(|listing| listing.names)
                .filter(|name| !ENTRY_POINTS.contains(&name.as_str()));
            let aliases = self
                .aliases()
                .keys()
                .chain(self.shell.get_functions())
                .filter(|a| **a != self.alias)
                .cloned();
            bins.chain(aliases)
                .filter(|name| seen.insert(name.clone()))
                .collect()
        })
    }

    pub fn history(&self) -> &[String] {
        self.history
            .get_or_init(|| self.shell.get_history(self.settings.history_limit))
    }

    /// A bounded tail of the history for the structured engine: at most
    /// `history_limit` (default 5000) entries from the file's last megabyte.
    pub fn recent_history(&self) -> &[String] {
        if let Some(fixed) = self.history.get() {
            return fixed;
        }
        self.recent_history.get_or_init(|| {
            self.shell
                .recent_history(self.settings.history_limit.unwrap_or(5000))
        })
    }
}

/// The command that should be fixed.
#[derive(Debug)]
pub struct Command<'a> {
    pub script: String,
    output: Option<&'a str>,
    ctx: &'a Context,
    parts: OnceCell<Vec<String>>,
    output_lower: OnceCell<String>,
}

impl<'a> Command<'a> {
    pub fn new(
        script: impl Into<String>,
        output: Option<&'a str>,
        ctx: &'a Context,
    ) -> Command<'a> {
        Command {
            script: script.into(),
            output,
            ctx,
            parts: OnceCell::new(),
            output_lower: OnceCell::new(),
        }
    }

    /// `command.update(script=...)`: same output, new script.
    pub fn with_script(&self, script: impl Into<String>) -> Command<'a> {
        Command::new(script, self.output, self.ctx)
    }

    pub fn has_output(&self) -> bool {
        self.output.is_some()
    }

    /// The captured output (empty when there is none; rules that need it
    /// only run when it exists).
    pub fn output(&self) -> &'a str {
        self.output.unwrap_or_default()
    }

    /// `command.output.lower()`, computed once.
    pub fn output_lower(&self) -> &str {
        self.output_lower
            .get_or_init(|| self.output().to_lowercase())
    }

    /// `command.script_parts`, computed once.
    pub fn script_parts(&self) -> &[String] {
        self.parts
            .get_or_init(|| self.ctx.shell.split_command(&self.script))
    }

    /// `script_parts[i]`, or `""` when out of range.
    pub fn part(&self, i: usize) -> &str {
        self.script_parts().get(i).map_or("", String::as_str)
    }

    pub fn has_part(&self, part: &str) -> bool {
        self.script_parts().iter().any(|p| p == part)
    }

    pub fn ctx(&self) -> &'a Context {
        self.ctx
    }

    pub fn shell(&self) -> Shell {
        self.ctx.shell
    }

    pub fn settings(&self) -> &'a Settings {
        &self.ctx.settings
    }

    /// `is_app(command, *names)`.
    pub fn is_app(&self, names: &[&str]) -> bool {
        self.is_app_at_least(names, 0)
    }

    /// `is_app(command, *names, at_least=n)`: also requires more than `n` parts.
    pub fn is_app_at_least(&self, names: &[&str], at_least: usize) -> bool {
        let parts = self.script_parts();
        parts.len() > at_least && {
            let first = parts[0].rsplit(['/', '\\']).next().unwrap_or(&parts[0]);
            #[cfg(unix)]
            {
                names.contains(&first)
            }
            #[cfg(windows)]
            {
                let name =
                    utils::windows_executable_name(first).unwrap_or_else(|| first.to_owned());
                names
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(&name))
            }
        }
    }

    /// `get_valid_history_without_current(command)`: history entries that
    /// weren't immediately "fucked", aren't our alias, aren't this command,
    /// and start with a known executable or builtin.
    pub fn valid_history_without_current(&self) -> Vec<String> {
        let ctx = self.ctx;
        let history = ctx.history();
        let executables: HashSet<&str> = ctx
            .executables()
            .iter()
            .map(String::as_str)
            .chain(ctx.shell.get_builtin_commands().iter().copied())
            .collect();
        // `_not_corrected`: drop entries followed by the alias (they were broken).
        let not_corrected = history.iter().enumerate().filter_map(|(i, line)| {
            let fucked = history.get(i + 1).is_some_and(|next| *next == ctx.alias);
            (!fucked || i + 1 == history.len()).then_some(line)
        });
        not_corrected
            .filter(|line| {
                !line.starts_with(ctx.alias.as_str())
                    && **line != self.script
                    && executables.contains(line.split(' ').next().unwrap_or_default())
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
impl Command<'static> {
    /// A command evaluated against a default (bash, default settings) context.
    pub fn test(script: &str, output: &'static str) -> Command<'static> {
        static CTX: std::sync::LazyLock<Context> = std::sync::LazyLock::new(|| {
            Context::new(Settings::default(), Shell::Bash, "fuck".into())
        });
        Command::new(script, Some(output), &CTX)
    }
}

/// Side effects run right before the fixed command is printed.
pub type SideEffect = fn(old: &Command, new_script: &str);

/// A rule, as loaded from `thefuck/rules/<name>.py`.
#[derive(Clone, Copy)]
pub struct Rule {
    pub name: &'static str,
    pub matches: fn(&Command) -> bool,
    pub get_new_command: fn(&Command) -> Vec<String>,
    pub priority: i64,
    /// Evaluated lazily: some rules probe the system (`which('apt-get')`).
    pub enabled_by_default: fn() -> bool,
    pub requires_output: bool,
    pub side_effect: Option<SideEffect>,
}

pub const DEFAULT_PRIORITY: i64 = 1000;

impl Rule {
    pub const fn new(
        name: &'static str,
        matches: fn(&Command) -> bool,
        get_new_command: fn(&Command) -> Vec<String>,
    ) -> Rule {
        Rule {
            name,
            matches,
            get_new_command,
            priority: DEFAULT_PRIORITY,
            enabled_by_default: || true,
            requires_output: true,
            side_effect: None,
        }
    }

    pub const fn priority(mut self, priority: i64) -> Rule {
        self.priority = priority;
        self
    }

    pub const fn enabled_by_default(mut self, enabled: fn() -> bool) -> Rule {
        self.enabled_by_default = enabled;
        self
    }

    pub const fn disabled_by_default(self) -> Rule {
        self.enabled_by_default(|| false)
    }

    pub const fn requires_output(mut self, requires_output: bool) -> Rule {
        self.requires_output = requires_output;
        self
    }

    pub const fn side_effect(mut self, side_effect: SideEffect) -> Rule {
        self.side_effect = Some(side_effect);
        self
    }

    /// `Rule.is_match`.
    pub fn is_match(&self, command: &Command) -> bool {
        (command.has_output() || !self.requires_output) && (self.matches)(command)
    }
}

impl std::fmt::Debug for Rule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rule")
            .field("name", &self.name)
            .field("priority", &self.priority)
            .finish()
    }
}

/// A fixed command proposed by a rule.
#[derive(Clone)]
pub struct CorrectedCommand {
    pub script: String,
    pub side_effect: Option<SideEffect>,
    pub priority: i64,
    /// The rule that proposed it.
    pub rule: &'static str,
}

impl PartialEq for CorrectedCommand {
    /// Ignores `priority` (and the rule), like Python.
    fn eq(&self, other: &Self) -> bool {
        self.script == other.script
            && self.side_effect.map(|f| f as usize) == other.side_effect.map(|f| f as usize)
    }
}

impl std::fmt::Debug for CorrectedCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CorrectedCommand(script={}, side_effect={}, priority={})",
            self.script,
            self.side_effect.is_some(),
            self.priority
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_app_like_python() {
        assert!(Command::test("/usr/bin/git diff", "").is_app(&["git", "hub"]));
        assert!(Command::test("/bin/hdfs dfs -rm foo", "").is_app(&["hdfs"]));
        assert!(Command::test("git diff --staged", "").is_app(&["git", "hub"]));
        assert!(!Command::test("firefox bla", "").is_app(&["git", "hub"]));
        assert!(Command::test("hub push --set-upstream origin foo", "").is_app(&["git", "hub"]));
        assert!(!Command::test("cat", "").is_app_at_least(&["cat"], 1));
        assert!(Command::test("cat file", "").is_app_at_least(&["cat"], 1));
    }

    #[test]
    fn script_parts_and_lowercase() {
        let c = Command::test("git commit -m \"new commit\"", "Permission DENIED");
        assert_eq!(c.script_parts(), ["git", "commit", "-m", "new commit"]);
        assert_eq!(c.output_lower(), "permission denied");
        assert_eq!(c.with_script("ls").output(), "Permission DENIED");
    }

    #[test]
    fn valid_history_skips_fucked_alias_and_current() {
        let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into())
            .with_executables(&["ls", "git", "gist"])
            .with_history(&["ls", "gist x", "fuck", "unknown y", "git log", "gti log"]);
        let cmd = Command::new("gti log", Some(""), &ctx);
        assert_eq!(cmd.valid_history_without_current(), ["ls", "git log"]);
    }
}
