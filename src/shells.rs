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

/// `builtins` in tcsh 6.21.
const TCSH_BUILTINS: &[&str] = &[
    ":",
    "@",
    "alias",
    "alloc",
    "bg",
    "bindkey",
    "break",
    "breaksw",
    "builtins",
    "bye",
    "case",
    "cd",
    "chdir",
    "complete",
    "continue",
    "default",
    "dirs",
    "echo",
    "echotc",
    "else",
    "end",
    "endif",
    "endsw",
    "eval",
    "exec",
    "exit",
    "fg",
    "filetest",
    "foreach",
    "glob",
    "goto",
    "hashstat",
    "history",
    "hup",
    "if",
    "jobs",
    "kill",
    "limit",
    "login",
    "logout",
    "ls-F",
    "nice",
    "nohup",
    "notify",
    "onintr",
    "popd",
    "printenv",
    "pushd",
    "rehash",
    "repeat",
    "sched",
    "set",
    "setenv",
    "settc",
    "setty",
    "shift",
    "source",
    "stop",
    "suspend",
    "switch",
    "telltc",
    "termname",
    "time",
    "umask",
    "unalias",
    "uncomplete",
    "unhash",
    "unlimit",
    "unset",
    "unsetenv",
    "wait",
    "watchlog",
    "where",
    "which",
    "while",
];

const FISH_BUILTINS: &[&str] = &[
    "!",
    ".",
    ":",
    "[",
    "_",
    "abbr",
    "and",
    "argparse",
    "begin",
    "bg",
    "bind",
    "block",
    "break",
    "breakpoint",
    "builtin",
    "case",
    "cd",
    "command",
    "commandline",
    "complete",
    "contains",
    "continue",
    "count",
    "disown",
    "echo",
    "else",
    "emit",
    "end",
    "eval",
    "exec",
    "exit",
    "false",
    "fg",
    "fish_indent",
    "fish_key_reader",
    "for",
    "function",
    "functions",
    "history",
    "if",
    "jobs",
    "math",
    "not",
    "or",
    "path",
    "printf",
    "pwd",
    "random",
    "read",
    "realpath",
    "return",
    "set",
    "set_color",
    "source",
    "status",
    "string",
    "switch",
    "test",
    "time",
    "true",
    "type",
    "ulimit",
    "wait",
    "while",
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
    /// The generic function names itself `generic`; an ancestor named `sh`
    /// doesn't, since scripts run through `sh` inside other shells.
    pub fn detect() -> Shell {
        env::var("TF_SHELL")
            .ok()
            .and_then(|name| {
                if name == "generic" {
                    Some(Shell::Generic)
                } else {
                    Shell::from_name(&name)
                }
            })
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
        if self == Shell::Fish {
            return crate::engine::parser::quote_word_with_dialect(
                s,
                crate::engine::parser::Dialect::Fish,
            );
        }
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
        if self == Shell::Fish {
            let parsed = crate::engine::parser::parse_with_dialect(
                command,
                crate::engine::parser::Dialect::Fish,
            );
            let text = |w: &crate::engine::parser::Word| {
                w.literal()
                    .map(str::to_owned)
                    .unwrap_or_else(|| w.span.of(command).to_owned())
            };
            // Reserved words and block headers stay in source order, as
            // rules saw them before blocks were parsed.
            let mut parts: Vec<(usize, String)> = parsed
                .commands
                .iter()
                .flat_map(|c| c.assignments.iter().chain(&c.words))
                .chain(parsed.compounds.iter().flat_map(|c| &c.words))
                .map(|w| (w.span.start, text(w)))
                .chain(parsed.compounds.iter().flat_map(|c| {
                    c.keywords
                        .iter()
                        .map(|k| (k.start, k.of(command).to_owned()))
                }))
                .collect();
            parts.sort_by_key(|(start, _)| *start);
            return parts.into_iter().map(|(_, part)| part).collect();
        }
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
                local -x NOTYPO_EXIT_STATUS=$? NOTYPO_PIPESTATUS=\"${{PIPESTATUS[*]}}\";
                local -x NOTYPO_SHELL_FUNCTIONS=\"$(compgen -A function -X '_*')\";
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
                local -x NOTYPO_EXIT_STATUS=$? NOTYPO_PIPESTATUS=\"${{pipestatus[*]}}\";
                local -x NOTYPO_ZSH_FPATH=\"${{(j.:.)fpath}}\";
                local -x NOTYPO_SHELL_FUNCTIONS=\"${{(k)functions[(I)[^_]*]}}\";
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
                 set -l notypo_status $status $pipestatus\n  \
                 set -lx NOTYPO_EXIT_STATUS $notypo_status[1]\n  \
                 set -lx NOTYPO_PIPESTATUS (string join ' ' -- $notypo_status[2..-1])\n  \
                 set -lx NOTYPO_SHELL_FUNCTIONS (functions --names | string match -v '_*')\n  \
                 set -lx NOTYPO_FISH_COMPLETE_PATH (string join ':' -- $fish_complete_path)\n  \
                 set -l notypo_history fish\n  \
                 if set -q fish_history\n    \
                 set notypo_history \"$fish_history\"\n  \
                 end\n  \
                 if set -q fish_private_mode\n    \
                 set notypo_history ''\n  \
                 end\n  \
                 set -lx NOTYPO_FISH_HISTORY_SESSION \"$notypo_history\"\n  \
                 set -l fucked_up_command $history[1]\n  \
                 set -lx NOTYPO_CURRENT_COMMAND \"$fucked_up_command\"\n  \
                 set -lx TF_SHELL fish\n  \
                 set -lx TF_ALIAS {name}\n  \
                 set -l unfucked_command ({exe} {PLACEHOLDER} $argv | string collect)\n  \
                 if [ \"$unfucked_command\" != \"\" ]\n    \
                 eval $unfucked_command\n{}  \
                 end\nend",
                if alter_history {
                    // `history append` was added in fish 4. Older shells
                    // keep their original history instead of writing the
                    // file from Rust or deleting without a replacement.
                    "    if builtin history append -- $unfucked_command 2>/dev/null\n      if test \"$unfucked_command\" != \"$fucked_up_command\"\n        builtin history delete --exact --case-sensitive -- $fucked_up_command\n      end\n    end\n"
                } else {
                    ""
                }
            ),
            // `$status` is read before anything else runs. The previous
            // event travels in the environment as one word: backquotes in
            // double quotes aren't globbed, while text inside backquotes
            // would be lexed again. `\!*` forwards the alias's arguments.
            Shell::Tcsh => {
                let exe = exe.replace('\'', "'\\''");
                format!(
                    "alias {name} 'setenv NOTYPO_EXIT_STATUS $status; \
                     setenv TF_SHELL tcsh; setenv TF_ALIAS {name}; \
                     setenv NOTYPO_CURRENT_COMMAND \"`history -h 2 | head -n 1`\"; \
                     eval \"`{exe} {PLACEHOLDER} \\!*`\"; \
                     unsetenv NOTYPO_EXIT_STATUS NOTYPO_CURRENT_COMMAND'"
                )
            }
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
            // A POSIX function: `$?` is read before anything else runs, the
            // recent history lets notypo skip this call's own line (ksh's
            // `fc -ln -1` includes it), and arguments follow the placeholder
            // instead of being appended to `eval`.
            Shell::Generic => format!(
                "{name} () {{\n    \
                 notypo_status=$?\n    \
                 notypo_cmd=$(NOTYPO_EXIT_STATUS=$notypo_status TF_SHELL=generic TF_ALIAS={name} \
                 TF_HISTORY=\"$(fc -ln -10 2>/dev/null)\" {exe} {PLACEHOLDER} \"$@\") && \
                 eval \"$notypo_cmd\"\n    \
                 unset notypo_status notypo_cmd\n}}"
            ),
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

    /// Names of shell functions the shell function passed along, without the
    /// `_`-prefixed completion helpers. Memoized for the process.
    pub fn get_functions(self) -> &'static std::collections::HashSet<String> {
        static FUNCTIONS: OnceLock<std::collections::HashSet<String>> = OnceLock::new();
        FUNCTIONS.get_or_init(|| match self {
            Shell::Bash | Shell::Zsh | Shell::Fish => env::var("NOTYPO_SHELL_FUNCTIONS")
                .unwrap_or_default()
                .split_whitespace()
                .filter(|name| !DEFAULT_ALIASES.contains(name))
                .map(str::to_owned)
                .collect(),
            _ => Default::default(),
        })
    }

    /// Shell aliases, memoized for the process. Fish functions are supplied
    /// by the active parent shell; discovery must not start an interactive
    /// fish that executes the user's configuration.
    pub fn get_aliases(self) -> &'static HashMap<String, String> {
        static ALIASES: OnceLock<HashMap<String, String>> = OnceLock::new();
        ALIASES.get_or_init(|| match self {
            Shell::Bash | Shell::Zsh => {
                parse_aliases(self, &env::var("TF_SHELL_ALIASES").unwrap_or_default())
            }
            Shell::Tcsh => utils::run_stdout("tcsh", &["-ic", "alias"])
                .map(|out| {
                    out.split('\n')
                        .filter_map(|l| l.split_once('\t'))
                        .map(|(n, v)| (n.into(), v.into()))
                        .collect()
                })
                .unwrap_or_default(),
            Shell::Generic | Shell::Powershell | Shell::Fish => HashMap::new(),
        })
    }

    /// `from_shell`: prepares the command before running it (alias expansion).
    pub fn from_shell(self, script: &str) -> String {
        let aliases = self.get_aliases();
        let binary = script.split(' ').next().unwrap_or_default();
        let Some(expansion) = aliases.get(binary) else {
            return script.to_owned();
        };
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
            Shell::Fish => {
                let session = env::var("NOTYPO_FISH_HISTORY_SESSION")
                    .or_else(|_| env::var("fish_history"))
                    .unwrap_or_else(|_| "fish".into());
                if session.is_empty() || session.contains(['/', '\\']) {
                    return None;
                }
                let session = if session == "default" {
                    "fish"
                } else {
                    &session
                };
                let data = env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| utils::expand_user("~/.local/share"));
                Some(data.join("fish").join(format!("{session}_history")))
            }
            Shell::Generic | Shell::Powershell => None,
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
            Shell::Fish => {
                let Some(raw) = line.strip_prefix("- cmd: ") else {
                    return String::new();
                };
                let mut chars = raw.trim_end_matches(['\r', '\n']).chars().peekable();
                let mut command = String::new();
                while let Some(ch) = chars.next() {
                    if ch == '\\' {
                        match chars.peek() {
                            Some('n') => {
                                chars.next();
                                command.push('\n');
                                continue;
                            }
                            Some('\\') => {
                                chars.next();
                                command.push('\\');
                                continue;
                            }
                            _ => {}
                        }
                    }
                    command.push(ch);
                }
                command
            }
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

    /// The newest `limit` entries, oldest first, reading at most the last
    /// megabyte of the history file.
    pub fn recent_history(self, limit: usize) -> Vec<String> {
        use std::io::{Read, Seek, SeekFrom};
        const TAIL: u64 = 1024 * 1024;
        let Some(mut file) = self
            .history_file_name()
            .and_then(|f| fs::File::open(f).ok())
        else {
            return Vec::new();
        };
        let len = file.metadata().map_or(0, |m| m.len());
        let clipped = len > TAIL && file.seek(SeekFrom::Start(len - TAIL)).is_ok();
        let mut bytes = Vec::new();
        if file.take(TAIL).read_to_end(&mut bytes).is_err() {
            return Vec::new();
        }
        let text: String = bytes.utf8_chunks().map(|chunk| chunk.valid()).collect();
        // The first line of a clipped read is probably partial.
        let text = if clipped {
            text.split_once('\n').map_or("", |(_, rest)| rest)
        } else {
            &text
        };
        let mut lines: Vec<String> = text
            .split_inclusive('\n')
            .map(|line| utils::py_strip(&self.script_from_history(line)).to_owned())
            .filter(|line| !line.is_empty())
            .collect();
        lines.drain(..lines.len().saturating_sub(limit));
        lines
    }

    /// Kept for library compatibility. Generated aliases update history in
    /// the parent session after execution, respecting its private mode.
    pub fn put_to_history(self, _command: &str) {}

    pub fn get_builtin_commands(self) -> &'static [&'static str] {
        match self {
            Shell::Fish => FISH_BUILTINS,
            Shell::Tcsh => TCSH_BUILTINS,
            _ => BUILTIN_COMMANDS,
        }
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
    fn fish_parts_keep_block_words_in_source_order() {
        assert_eq!(
            Shell::Fish.split_command("for f in a 'b c'; git sttus $f; end"),
            ["for", "f", "in", "a", "b c", "git", "sttus", "$f", "end"]
        );
        assert_eq!(
            Shell::Fish.split_command("not git sttus"),
            ["not", "git", "sttus"]
        );
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
        assert_eq!(
            Shell::Fish.script_from_history(r"- cmd: printf 'a\\b'\necho line"),
            "printf 'a\\b'\necho line"
        );
        assert_eq!(Shell::Fish.script_from_history("  - cmd: a path"), "");
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
    fn aliases_capture_the_exit_status_first() {
        for (shell, status) in [
            (
                Shell::Bash,
                "local -x NOTYPO_EXIT_STATUS=$? NOTYPO_PIPESTATUS=\"${PIPESTATUS[*]}\"",
            ),
            (
                Shell::Zsh,
                "local -x NOTYPO_EXIT_STATUS=$? NOTYPO_PIPESTATUS=\"${pipestatus[*]}\"",
            ),
            (Shell::Fish, "set -l notypo_status $status $pipestatus"),
        ] {
            let alias = shell.app_alias("fuck", "notypo", true);
            let body = alias.split_once('\n').unwrap().1.trim_start();
            let body = if shell == Shell::Fish {
                body
            } else {
                alias.split_once("{\n").unwrap().1.trim_start()
            };
            assert!(body.starts_with(status), "{shell:?}: {alias}");
        }
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
