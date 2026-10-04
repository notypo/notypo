//! Shell integration, port of `thefuck/shells/*`.

use crate::args::PLACEHOLDER;
use crate::{logs, shlex, utils};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

pub const DEFAULT_ALIASES: &[&str] = &["fuck", "typo"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Generic,
    Bash,
    Zsh,
    Fish,
    Tcsh,
    Powershell,
}

/// How to install the alias (`Generic.how_to_configure`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellConfiguration {
    pub content: String,
    pub path: String,
    pub reload: String,
    pub can_configure_automatically: bool,
}

const BUILTIN_COMMANDS: &[&str] = &[
    "alias", "bg", "bind", "break", "builtin", "case", "cd", "command", "compgen", "complete",
    "continue", "declare", "dirs", "disown", "echo", "enable", "eval", "exec", "exit", "export",
    "fc", "fg", "getopts", "hash", "help", "history", "if", "jobs", "kill", "let", "local",
    "logout", "popd", "printf", "pushd", "pwd", "read", "readonly", "return", "set", "shift",
    "shopt", "source", "suspend", "test", "times", "trap", "type", "typeset", "ulimit", "umask",
    "unalias", "unset", "until", "wait", "while",
];

impl Shell {
    pub fn from_name(name: &str) -> Option<Shell> {
        Some(match name {
            "bash" => Shell::Bash,
            "zsh" => Shell::Zsh,
            "fish" => Shell::Fish,
            "csh" | "tcsh" => Shell::Tcsh,
            "powershell" | "pwsh" => Shell::Powershell,
            _ => return None,
        })
    }

    /// `TF_SHELL`, otherwise the first known shell among our ancestors.
    pub fn detect() -> Shell {
        env::var("TF_SHELL")
            .ok()
            .and_then(|name| Shell::from_name(&name))
            .or_else(Shell::from_process_tree)
            .unwrap_or(if cfg!(windows) {
                Shell::Powershell
            } else {
                Shell::Generic
            })
    }

    fn from_process_tree() -> Option<Shell> {
        let mut pid = std::process::id() as i32;
        for _ in 0..64 {
            if pid <= 0 {
                break;
            }
            let (name, parent) = process::name_and_parent(pid)?;
            // os.path.splitext(name)[0], and login shells show up as `-zsh`.
            let name = name.trim_start_matches('-');
            let stem = match name.rsplit_once('.') {
                Some((stem, _)) if !stem.is_empty() => stem,
                _ => name,
            };
            if let Some(shell) = Shell::from_name(stem) {
                return Some(shell);
            }
            pid = parent;
        }
        None
    }

    pub fn friendly_name(self) -> &'static str {
        match self {
            Shell::Generic => "Generic Shell",
            Shell::Bash => "Bash",
            Shell::Zsh => "ZSH",
            Shell::Fish => "Fish Shell",
            Shell::Tcsh => "Tcsh",
            Shell::Powershell => "PowerShell",
        }
    }

    pub fn and_(self, commands: &[&str]) -> String {
        match self {
            Shell::Fish => commands.join("; and "),
            Shell::Powershell => commands
                .iter()
                .map(|c| format!("({c})"))
                .collect::<Vec<_>>()
                .join(" -and "),
            _ => commands.join(" && "),
        }
    }

    pub fn or_(self, commands: &[&str]) -> String {
        match self {
            Shell::Fish => commands.join("; or "),
            _ => commands.join(" || "),
        }
    }

    pub fn quote(self, s: &str) -> String {
        if self == Shell::Powershell
            && !s
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-\\".contains(c))
        {
            return format!("'{}'", s.replace('\'', "''"));
        }
        shlex::quote(s).into_owned()
    }

    pub fn split_command(self, command: &str) -> Vec<String> {
        if self == Shell::Powershell {
            return powershell::split(command);
        }
        shlex::split_command(command)
            .into_iter()
            .map(|c| c.into_owned())
            .collect()
    }

    /// The function users `eval` from their shell rc file.
    pub fn app_alias(self, name: &str, exe: &str, alter_history: bool) -> String {
        let exe = shlex::quote(exe);
        match self {
            Shell::Bash => format!(
                "
            function {name} () {{
                export TF_SHELL=bash;
                export TF_ALIAS={name};
                export TF_SHELL_ALIASES=$(alias);
                export TF_HISTORY=$(fc -ln -10);
                TF_CMD=$(
                    {exe} {PLACEHOLDER} \"$@\"
                ) && eval \"$TF_CMD\";
                unset TF_HISTORY;
                {}
            }}
        ",
                if alter_history {
                    "history -s $TF_CMD;"
                } else {
                    ""
                }
            ),
            Shell::Zsh => format!(
                "
            {name} () {{
                local -x NOTYPO_CURRENT_COMMAND=\"${{history[$HISTCMD]-}}\";
                export TF_SHELL=zsh;
                export TF_ALIAS={name};
                TF_SHELL_ALIASES=$(alias);
                export TF_SHELL_ALIASES;
                TF_HISTORY=\"$(fc -ln -10)\";
                export TF_HISTORY;
                TF_CMD=$(
                    {exe} {PLACEHOLDER} $@
                ) && eval $TF_CMD;
                unset TF_HISTORY;
                {}
            }}
        ",
                if alter_history {
                    "test -n \"$TF_CMD\" && print -s $TF_CMD"
                } else {
                    ""
                }
            ),
            Shell::Fish => format!(
                "function {name} -d \"Correct your previous console command\"\n  \
                 set -l fucked_up_command $history[1]\n  \
                 env TF_SHELL=fish TF_ALIAS={name} {exe} $fucked_up_command {PLACEHOLDER} $argv | read -l unfucked_command\n  \
                 if [ \"$unfucked_command\" != \"\" ]\n    \
                 eval $unfucked_command\n{}  \
                 end\nend",
                if alter_history {
                    "    builtin history delete --exact --case-sensitive -- $fucked_up_command\n    builtin history merge\n"
                } else {
                    ""
                }
            ),
            Shell::Tcsh => format!(
                "alias {name} 'setenv TF_SHELL tcsh && setenv TF_ALIAS {name} && \
                 set fucked_cmd=`history -h 2 | head -n 1` && eval `{exe} ${{fucked_cmd}}`'"
            ),
            Shell::Powershell => format!(
                "function {name} {{\n    $history = (Get-History -Count 1).CommandLine;\n    \
                 if (-not [string]::IsNullOrWhiteSpace($history)) {{\n        \
                 $env:TF_SHELL = 'powershell';\n        \
                 $env:TF_ALIAS = '{name}';\n        \
                 $fuck = $(& {exe} $args $history);\n        \
                 if (-not [string]::IsNullOrWhiteSpace($fuck)) {{\n            \
                 if ($fuck.StartsWith(\"echo\")) {{ $fuck = $fuck.Substring(5); }}\n            \
                 else {{ iex \"$fuck\"; }}\n        }}\n    }}\n    [Console]::ResetColor() \n}}\n"
            ),
            Shell::Generic => {
                format!("alias {name}='eval \"$(TF_ALIAS={name} {exe} \"$(fc -ln -1)\")\"'")
            }
        }
    }

    /// Each function has its own TF_ALIAS so history selection and repeat work
    /// under either name.
    pub fn app_aliases(self, names: &[&str], exe: &str, alter_history: bool) -> String {
        names
            .iter()
            .map(|name| self.app_alias(name, exe, alter_history))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Starts a recorded shell, or marks prompts inside an already recorded one.
    pub fn instant_mode_alias(self, name: &str, exe: &str, alter_history: bool) -> String {
        self.instant_mode_aliases(&[name], exe, alter_history)
    }

    /// Installs all names while marking the prompt or starting the logger once.
    pub fn instant_mode_aliases(self, names: &[&str], exe: &str, alter_history: bool) -> String {
        if !matches!(self, Shell::Bash | Shell::Zsh) {
            logs::warn("Instant mode not supported by your shell");
            return self.app_aliases(names, exe, alter_history);
        }
        if env::var("THEFUCK_INSTANT_MODE").is_ok_and(|v| v.eq_ignore_ascii_case("true")) {
            let mark = format!("{}{}", logs::USER_COMMAND_MARK, "\x08".repeat(10));
            let mark = if self == Shell::Zsh {
                format!("%{{{mark}%}}")
            } else {
                format!("\\[{mark}\\]")
            };
            return format!(
                "export PS1=\"{mark}$PS1\";\n{}",
                self.app_aliases(names, exe, alter_history)
            );
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = env::temp_dir().join(format!(
            "notypo-script-log-{}-{timestamp}",
            std::process::id()
        ));
        let path = self.quote(&path.to_string_lossy());
        format!(
            "export THEFUCK_INSTANT_MODE=True;\nexport THEFUCK_OUTPUT_LOG={path};\n{} --shell-logger {path};\nrm -f {path};\nexit",
            self.quote(exe)
        )
    }

    /// Shell aliases (and fish functions), memoized for the process.
    pub fn get_aliases(self) -> &'static HashMap<String, String> {
        static ALIASES: OnceLock<HashMap<String, String>> = OnceLock::new();
        ALIASES.get_or_init(|| match self {
            Shell::Bash | Shell::Zsh => {
                parse_aliases(self, &env::var("TF_SHELL_ALIASES").unwrap_or_default())
            }
            Shell::Fish => fish::aliases(),
            Shell::Tcsh => utils::run_stdout("tcsh", &["-ic", "alias"])
                .map(|out| {
                    out.split('\n')
                        .filter_map(|l| l.split_once('\t'))
                        .map(|(n, v)| (n.into(), v.into()))
                        .collect()
                })
                .unwrap_or_default(),
            Shell::Generic | Shell::Powershell => HashMap::new(),
        })
    }

    /// `from_shell`: prepares the command before running it (alias expansion).
    pub fn from_shell(self, script: &str) -> String {
        let aliases = self.get_aliases();
        let binary = script.split(' ').next().unwrap_or_default();
        let Some(expansion) = aliases.get(binary) else {
            return script.to_owned();
        };
        if self == Shell::Fish && expansion == binary {
            // A fish function: run it through fish itself.
            return format!("fish -ic \"{}\"", script.replace('"', "\\\""));
        }
        script.replacen(binary, expansion, 1)
    }

    pub fn history_file_name(self) -> Option<PathBuf> {
        let histfile = |default: &str| {
            Some(env::var("HISTFILE").map_or_else(|_| utils::expand_user(default), PathBuf::from))
        };
        match self {
            Shell::Bash => histfile("~/.bash_history"),
            Shell::Zsh => histfile("~/.zsh_history"),
            Shell::Tcsh => histfile("~/.history"),
            Shell::Fish => Some(utils::expand_user("~/.config/fish/fish_history")),
            Shell::Generic | Shell::Powershell => None,
        }
    }

    fn history_line(self, command: &str) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        match self {
            Shell::Bash => format!("{command}\n"),
            Shell::Zsh => format!(": {now}:0;{command}\n"),
            Shell::Fish => format!("- cmd: {command}\n   when: {now}\n"),
            Shell::Tcsh => format!("#+{now}\n{command}\n"),
            Shell::Generic | Shell::Powershell => String::new(),
        }
    }

    pub fn script_from_history(self, line: &str) -> String {
        match self {
            // EXTENDED_HISTORY lines are `: <start>:<elapsed>;<command>`.
            // thefuck drops every line without `;`, losing the whole history
            // in zsh's default plain format; we keep those lines.
            Shell::Zsh => match line
                .strip_prefix(": ")
                .and_then(|rest| rest.split_once(';'))
            {
                Some((stamp, command)) if stamp.chars().all(|c| c.is_ascii_digit() || c == ':') => {
                    command.to_owned()
                }
                _ => line.to_owned(),
            },
            Shell::Fish => line
                .split_once("- cmd: ")
                .map_or_else(String::new, |(_, cmd)| cmd.to_owned()),
            _ => line.to_owned(),
        }
    }

    /// Prepared, non-empty history entries, oldest first.
    pub fn get_history(self, limit: Option<usize>) -> Vec<String> {
        let Some(file) = self.history_file_name() else {
            return Vec::new();
        };
        let Ok(bytes) = fs::read(file) else {
            return Vec::new();
        };
        // errors='ignore': drop invalid UTF-8 sequences.
        let text: String = bytes.utf8_chunks().map(|chunk| chunk.valid()).collect();
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let lines = match limit {
            Some(n) if n > 0 => &lines[lines.len().saturating_sub(n)..],
            _ => &lines[..],
        };
        lines
            .iter()
            .map(|line| utils::py_strip(&self.script_from_history(line)).to_owned())
            .filter(|line| !line.is_empty())
            .collect()
    }

    /// Adds the fixed command to history where the alias can't (only fish).
    pub fn put_to_history(self, command: &str) {
        if self != Shell::Fish {
            return;
        }
        let Some(file) = self.history_file_name().filter(|f| f.is_file()) else {
            return;
        };
        let result = fs::OpenOptions::new()
            .append(true)
            .open(file)
            .and_then(|mut f| {
                std::io::Write::write_all(&mut f, self.history_line(command).as_bytes())
            });
        if let Err(e) = result {
            logs::warn(&format!("Can't update history: {e}"));
        }
    }

    pub fn get_builtin_commands(self) -> &'static [&'static str] {
        BUILTIN_COMMANDS
    }

    fn version(self) -> Option<String> {
        let run = |prog: &str, args: &[&str]| utils::run_stdout(prog, args);
        match self {
            Shell::Bash => run("bash", &["-c", "echo $BASH_VERSION"]).map(|s| s.trim().to_owned()),
            Shell::Zsh => run("zsh", &["-c", "echo $ZSH_VERSION"]).map(|s| s.trim().to_owned()),
            Shell::Fish => run("fish", &["--version"])
                .and_then(|s| s.split_whitespace().last().map(str::to_owned)),
            Shell::Tcsh => run("tcsh", &["--version"])
                .and_then(|s| s.split_whitespace().nth(1).map(str::to_owned)),
            Shell::Powershell => run("pwsh", &["--version"])
                .and_then(|s| s.split_whitespace().last().map(str::to_owned)),
            Shell::Generic => Some(String::new()),
        }
    }

    /// "<name> <version>", e.g. `ZSH 5.9`.
    pub fn info(self) -> String {
        format!(
            "{} {}",
            self.friendly_name(),
            self.version().unwrap_or_default()
        )
        .trim_end()
        .to_owned()
    }

    pub fn how_to_configure(self, exe: &str) -> Option<ShellConfiguration> {
        let config = |content: String, path: &str, reload: &str| {
            Some(ShellConfiguration {
                content,
                path: path.to_owned(),
                reload: reload.to_owned(),
                can_configure_automatically: utils::expand_user(path).exists(),
            })
        };
        match self {
            Shell::Bash => config(
                format!("eval \"$({exe} --alias)\""),
                "~/.bashrc",
                "source ~/.bashrc",
            ),
            Shell::Zsh => config(
                format!("eval $({exe} --alias)"),
                "~/.zshrc",
                "source ~/.zshrc",
            ),
            Shell::Fish => config(
                format!("{exe} --alias | source"),
                "~/.config/fish/config.fish",
                "fish",
            ),
            Shell::Tcsh => config(format!("eval `{exe} --alias`"), "~/.tcshrc", "tcsh"),
            Shell::Powershell => Some(ShellConfiguration {
                content: format!("iex \"$({exe} --alias)\""),
                path: "$profile".into(),
                reload: ". $profile".into(),
                can_configure_automatically: false,
            }),
            Shell::Generic => None,
        }
    }
}

/// Parses `alias` output from bash (`alias ll='ls -l'`) or zsh (`ll='ls -l'`).
pub fn parse_aliases(shell: Shell, raw: &str) -> HashMap<String, String> {
    raw.split('\n')
        .filter(|line| !line.is_empty() && line.contains('='))
        .filter_map(|line| {
            let line = if shell == Shell::Bash {
                line.replacen("alias ", "", 1)
            } else {
                line.to_owned()
            };
            let (name, value) = line.split_once('=')?;
            let quoted = value.len() >= 2
                && (value.starts_with('"') && value.ends_with('"')
                    || value.starts_with('\'') && value.ends_with('\''));
            let value = if quoted {
                &value[1..value.len() - 1]
            } else {
                value
            };
            Some((name.to_owned(), value.to_owned()))
        })
        .collect()
}

mod fish {
    use super::*;

    fn overridden_aliases() -> Vec<String> {
        let raw = env::var("THEFUCK_OVERRIDDEN_ALIASES")
            .or_else(|_| env::var("TF_OVERRIDDEN_ALIASES"))
            .unwrap_or_default();
        let mut names: Vec<String> = ["cd", "grep", "ls", "man", "open"].map(String::from).into();
        names.extend(raw.split(',').map(|a| a.trim().to_owned()));
        names.sort();
        names.dedup();
        names
    }

    /// Fish functions and aliases (cached on disk: running fish is slow).
    pub fn aliases() -> HashMap<String, String> {
        let overridden = overridden_aliases();
        let config = PathBuf::from("~/.config/fish/config.fish");
        let functions = utils::cached(
            "fish-functions",
            &[config.clone(), PathBuf::from("~/.config/fish/functions")],
            || {
                utils::run_stdout("fish", &["-ic", "functions"])
                    .map(|s| s.trim().split('\n').map(str::to_owned).collect())
                    .unwrap_or_default()
            },
        );
        let raw_aliases = utils::cached("fish-aliases", &[config], || {
            utils::run_stdout("fish", &["-ic", "alias"])
                .map(|s| s.trim().split('\n').map(str::to_owned).collect())
                .unwrap_or_default()
        });
        let mut aliases: HashMap<String, String> = functions
            .into_iter()
            .filter(|f| !f.is_empty() && !overridden.contains(f))
            .map(|f| (f.clone(), f))
            .collect();
        for line in raw_aliases.iter().filter(|l| !l.is_empty()) {
            let line = line.replacen("alias ", "", 1);
            let Some((name, value)) = line.split_once(' ').or_else(|| line.split_once('=')) else {
                continue;
            };
            if !overridden.iter().any(|o| o == name) {
                aliases.insert(name.to_owned(), value.to_owned());
            }
        }
        aliases
    }
}

/// Process name / parent lookup for shell detection (psutil in Python).
mod process {
    #[cfg(target_os = "macos")]
    pub fn name_and_parent(pid: i32) -> Option<(String, i32)> {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: the buffer is exactly one `proc_bsdinfo`.
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if written != size {
            return None;
        }
        // SAFETY: the kernel filled the struct.
        let info = unsafe { info.assume_init() };
        let name = if info.pbi_name[0] != 0 {
            &info.pbi_name[..]
        } else {
            &info.pbi_comm[..]
        };
        let bytes: Vec<u8> = name
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        Some((
            String::from_utf8_lossy(&bytes).into_owned(),
            info.pbi_ppid as i32,
        ))
    }

    #[cfg(target_os = "linux")]
    pub fn name_and_parent(pid: i32) -> Option<(String, i32)> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (open, close) = (stat.find('(')?, stat.rfind(')')?);
        let parent = stat.get(close + 2..)?.split(' ').nth(1)?.parse().ok()?;
        Some((stat[open + 1..close].to_owned(), parent))
    }

    #[cfg(windows)]
    pub fn name_and_parent(pid: i32) -> Option<(String, i32)> {
        crate::platform::windows::process_entry(pid as u32)
            .map(|(name, parent)| (name, parent as i32))
    }

    #[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
    pub fn name_and_parent(pid: i32) -> Option<(String, i32)> {
        let output = crate::utils::run_stdout(
            "ps",
            &["-p", &pid.to_string(), "-o", "comm=", "-o", "ppid="],
        )?;
        let (name, parent) = output.trim().rsplit_once(char::is_whitespace)?;
        Some((
            name.trim().rsplit('/').next()?.to_owned(),
            parent.trim().parse().ok()?,
        ))
    }
}

mod powershell {
    /// PowerShell quotes use backticks for escaping, and backslashes in paths
    /// are literal. A doubled apostrophe belongs to a single quoted string.
    pub(super) fn split(command: &str) -> Vec<String> {
        let mut chars = command.chars().peekable();
        let mut quote = None;
        let mut parts = Vec::new();
        let mut part = String::new();
        let mut started = false;
        while let Some(ch) = chars.next() {
            match ch {
                '`' if quote != Some('\'') => {
                    let Some(next) = chars.next() else {
                        return command.split(' ').map(str::to_owned).collect();
                    };
                    part.push(next);
                    started = true;
                }
                '\'' | '"' if quote.is_none() => {
                    quote = Some(ch);
                    started = true;
                }
                ch if quote == Some(ch) => {
                    if ch == '\'' && chars.peek() == Some(&'\'') {
                        chars.next();
                        part.push('\'');
                    } else {
                        quote = None;
                    }
                }
                ch if ch.is_whitespace() && quote.is_none() => {
                    if started {
                        parts.push(std::mem::take(&mut part));
                        started = false;
                    }
                }
                ch => {
                    part.push(ch);
                    started = true;
                }
            }
        }
        if quote.is_some() {
            return command.split(' ').map(str::to_owned).collect();
        }
        if started {
            parts.push(part);
        }
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powershell_preserves_windows_paths_and_quote_rules() {
        assert_eq!(
            Shell::Powershell.split_command(r#"git -C C:\work\repo commit -m 'it''s fixed'"#),
            ["git", "-C", r"C:\work\repo", "commit", "-m", "it's fixed"]
        );
        assert_eq!(
            Shell::Powershell.split_command(r#"echo "a`"b" "" a` b"#),
            ["echo", "a\"b", "", "a b"]
        );
        assert_eq!(Shell::Powershell.quote("it's fixed"), "'it''s fixed'");
        let alias = Shell::Powershell.app_alias("fix", r"C:\Program Files\notypo.exe", true);
        assert!(alias.contains(r"$(& 'C:\Program Files\notypo.exe'"));
        assert!(alias.contains("$env:TF_ALIAS = 'fix'"));
    }

    #[test]
    fn parses_bash_and_zsh_aliases() {
        let bash = parse_aliases(
            Shell::Bash,
            "alias fuck='eval $(thefuck $(fc -ln -1))'\nalias l='ls -CF'\nalias la='ls -A'\nalias ll='ls -alF'",
        );
        assert_eq!(bash["l"], "ls -CF");
        assert_eq!(bash["fuck"], "eval $(thefuck $(fc -ln -1))");
        let zsh = parse_aliases(Shell::Zsh, "l='ls -CF'\nla='ls -A'\ngst=git status");
        assert_eq!(zsh["gst"], "git status");
        assert_eq!(zsh.len(), 3);
    }

    #[test]
    fn and_or() {
        assert_eq!(Shell::Bash.and_(&["ls", "cd"]), "ls && cd");
        assert_eq!(Shell::Fish.and_(&["ls", "cd"]), "ls; and cd");
        assert_eq!(Shell::Fish.or_(&["ls", "cd"]), "ls; or cd");
        assert_eq!(Shell::Powershell.and_(&["ls", "cd"]), "(ls) -and (cd)");
        assert_eq!(Shell::Zsh.or_(&["ls", "cd"]), "ls || cd");
    }

    #[test]
    fn history_line_parsing() {
        assert_eq!(
            Shell::Zsh.script_from_history(": 1700000000:0;git status"),
            "git status"
        );
        assert_eq!(Shell::Zsh.script_from_history("git log"), "git log");
        assert_eq!(
            Shell::Zsh.script_from_history(": 1700000000:12;echo a;b"),
            "echo a;b"
        );
        assert_eq!(Shell::Fish.script_from_history("- cmd: ls -la"), "ls -la");
        assert_eq!(Shell::Fish.script_from_history("   when: 1"), "");
        assert_eq!(Shell::Bash.script_from_history("ls"), "ls");
    }

    #[test]
    fn aliases_embed_placeholder_and_exe() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            let alias = shell.app_alias("fuck", "/opt/my bin/notypo", true);
            assert!(alias.contains(PLACEHOLDER), "{shell:?}");
            assert!(alias.contains("'/opt/my bin/notypo'"), "{shell:?}");
        }
        assert!(
            Shell::Bash
                .app_alias("fuck", "notypo", true)
                .contains("history -s $TF_CMD;")
        );
        assert!(
            !Shell::Bash
                .app_alias("fuck", "notypo", false)
                .contains("history -s")
        );
    }

    #[test]
    fn configuration() {
        assert_eq!(
            Shell::Zsh.how_to_configure("notypo").unwrap().content,
            "eval $(notypo --alias)"
        );
        assert!(Shell::Generic.how_to_configure("notypo").is_none());
        assert!(
            !Shell::Powershell
                .how_to_configure("notypo")
                .unwrap()
                .can_configure_automatically
        );
    }
}
