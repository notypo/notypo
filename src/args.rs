//! Command-line parsing, port of `thefuck/argument_parser.py`.
//!
//! Hand-rolled instead of clap because the semantics are argparse's:
//! unique-prefix long options, clustered short flags, an optional value for
//! `--alias`, and the `THEFUCK_ARGUMENT_PLACEHOLDER` trick used by the shell
//! aliases to separate our own flags from the broken command.

use std::fmt::Write as _;

pub const PLACEHOLDER: &str = "THEFUCK_ARGUMENT_PLACEHOLDER";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub version: bool,
    pub alias: Option<String>,
    /// A bare `--alias` installs both default correction functions.
    pub default_aliases: bool,
    pub shell_logger: Option<String>,
    pub instant_mode: bool,
    pub help: bool,
    pub yes: bool,
    pub repeat: bool,
    pub debug: bool,
    pub force_command: Option<String>,
    pub command: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Opt {
    Version,
    Alias,
    ShellLogger,
    InstantMode,
    Help,
    Yes,
    Repeat,
    Debug,
    ForceCommand,
}

impl Opt {
    const LONG: &[(&str, Opt)] = &[
        ("version", Opt::Version),
        ("alias", Opt::Alias),
        ("shell-logger", Opt::ShellLogger),
        ("enable-experimental-instant-mode", Opt::InstantMode),
        ("help", Opt::Help),
        ("yes", Opt::Yes),
        ("yeah", Opt::Yes),
        ("hard", Opt::Yes),
        ("repeat", Opt::Repeat),
        ("debug", Opt::Debug),
        ("force-command", Opt::ForceCommand),
    ];

    fn short(c: char) -> Option<Opt> {
        Some(match c {
            'v' => Opt::Version,
            'a' => Opt::Alias,
            'l' => Opt::ShellLogger,
            'h' => Opt::Help,
            'y' => Opt::Yes,
            'r' => Opt::Repeat,
            'd' => Opt::Debug,
            _ => return None,
        })
    }

    fn long(name: &str) -> Result<Opt, String> {
        if let Some(&(_, opt)) = Self::LONG.iter().find(|(n, _)| *n == name) {
            return Ok(opt);
        }
        let hits: Vec<&str> = Self::LONG
            .iter()
            .filter(|(n, _)| n.starts_with(name))
            .map(|(n, _)| *n)
            .collect();
        match hits.as_slice() {
            [one] => Self::long(one),
            [] => Err(format!("unrecognized arguments: --{name}")),
            many => {
                let names: Vec<String> = many.iter().map(|n| format!("--{n}")).collect();
                Err(format!(
                    "ambiguous option: --{name} could match {}",
                    names.join(", ")
                ))
            }
        }
    }

    fn display(self) -> &'static str {
        match self {
            Opt::Version => "-v/--version",
            Opt::Alias => "-a/--alias",
            Opt::ShellLogger => "-l/--shell-logger",
            Opt::InstantMode => "--enable-experimental-instant-mode",
            Opt::Help => "-h/--help",
            Opt::Yes => "-y/--yes/--yeah/--hard",
            Opt::Repeat => "-r/--repeat",
            Opt::Debug => "-d/--debug",
            Opt::ForceCommand => "--force-command",
        }
    }

    fn takes_value(self) -> bool {
        matches!(self, Opt::Alias | Opt::ShellLogger | Opt::ForceCommand)
    }
}

/// `Parser._prepare_arguments`: moves our own flags (everything after the
/// placeholder) in front of the broken command and inserts `--`.
pub fn prepare<S: AsRef<str>>(argv: &[S]) -> Vec<String> {
    let argv: Vec<String> = argv.iter().map(|s| s.as_ref().to_owned()).collect();
    if let Some(i) = argv.iter().position(|a| a == PLACEHOLDER) {
        let mut v = argv[i + 1..].to_vec();
        v.push("--".into());
        v.extend_from_slice(&argv[..i]);
        v
    } else if argv
        .first()
        .is_some_and(|a| !a.starts_with('-') && a != "--")
    {
        std::iter::once("--".to_owned()).chain(argv).collect()
    } else {
        argv
    }
}

fn looks_like_negative_number(s: &str) -> bool {
    let rest = &s[1..];
    !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
        && rest.chars().any(|c| c.is_ascii_digit())
}

fn is_option(s: &str) -> bool {
    s.len() > 1 && s.starts_with('-') && !looks_like_negative_number(s)
}

/// Parses `argv` (without the program name). `default_alias` is argparse's
/// `const` for a bare `--alias`.
pub fn parse<S: AsRef<str>>(argv: &[S], default_alias: &str) -> Result<Args, String> {
    let argv = prepare(argv);
    let mut args = Args::default();
    let mut exclusive: Option<Opt> = None;
    let mut positional_only = false;
    let mut i = 0;

    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        if positional_only || !is_option(arg) {
            args.command.push(arg.clone());
            continue;
        }
        if arg == "--" {
            positional_only = true;
            continue;
        }

        // Expand the argument into (option, inline value) pairs.
        let mut opts: Vec<(Opt, Option<String>)> = Vec::new();
        if let Some(body) = arg.strip_prefix("--") {
            let (name, inline) = match body.split_once('=') {
                Some((n, v)) => (n, Some(v.to_owned())),
                None => (body, None),
            };
            opts.push((Opt::long(name)?, inline));
        } else {
            let body = &arg[1..];
            for (pos, c) in body.char_indices() {
                let opt = Opt::short(c).ok_or_else(|| format!("unrecognized arguments: {arg}"))?;
                if opt.takes_value() {
                    let rest = &body[pos + c.len_utf8()..];
                    let rest = rest.strip_prefix('=').unwrap_or(rest);
                    opts.push((opt, (!rest.is_empty()).then(|| rest.to_owned())));
                    break;
                }
                opts.push((opt, None));
            }
        }

        for (opt, inline) in opts {
            match opt {
                Opt::Yes | Opt::Repeat => {
                    if let Some(prev) = exclusive.filter(|p| *p != opt) {
                        return Err(format!(
                            "argument {}: not allowed with argument {}",
                            opt.display(),
                            prev.display()
                        ));
                    }
                    exclusive = Some(opt);
                    match opt {
                        Opt::Yes => args.yes = true,
                        _ => args.repeat = true,
                    }
                }
                Opt::Version => args.version = true,
                Opt::InstantMode => args.instant_mode = true,
                Opt::Help => args.help = true,
                Opt::Debug => args.debug = true,
                Opt::Alias => {
                    // nargs='?': consume the next argument unless it's an option.
                    let value = inline.or_else(|| {
                        let next = argv.get(i).filter(|n| !is_option(n) && *n != "--")?;
                        i += 1;
                        Some(next.clone())
                    });
                    args.default_aliases = value.is_none();
                    args.alias = Some(value.unwrap_or_else(|| default_alias.to_owned()));
                }
                Opt::ShellLogger | Opt::ForceCommand => {
                    let value = match inline {
                        Some(v) => v,
                        None => match argv.get(i).filter(|n| !is_option(n)) {
                            Some(next) => {
                                i += 1;
                                next.clone()
                            }
                            None => {
                                return Err(format!(
                                    "argument {}: expected one argument",
                                    opt.display()
                                ));
                            }
                        },
                    };
                    if opt == Opt::ShellLogger {
                        args.shell_logger = Some(value);
                    } else {
                        args.force_command = Some(value);
                    }
                }
            }
        }
    }
    Ok(args)
}

pub fn usage(prog: &str) -> String {
    let pad = " ".repeat(prog.len() + 8);
    format!(
        "usage: {prog} [-v] [-a [ALIAS]] [-l SHELL_LOGGER]\n{pad}[--enable-experimental-instant-mode] [-h] [-y | -r] [-d]\n{pad}[command ...]\n"
    )
}

pub fn help(prog: &str) -> String {
    let mut s = usage(prog);
    let rows: &[(&str, &str)] = &[
        ("-v, --version", "show program's version number and exit"),
        (
            "-a [ALIAS], --alias [ALIAS]",
            "print fuck and typo functions, or a custom alias, for the current shell",
        ),
        (
            "-l SHELL_LOGGER, --shell-logger SHELL_LOGGER",
            "log shell output to the file",
        ),
        (
            "--enable-experimental-instant-mode",
            "enable experimental instant mode, use on your own risk",
        ),
        ("-h, --help", "show this help message and exit"),
        (
            "-y, --yes, --yeah, --hard",
            "execute fixed command without confirmation",
        ),
        ("-r, --repeat", "repeat on failure"),
        ("-d, --debug", "enable debug output"),
    ];
    s.push_str("\npositional arguments:\n  command               command that should be fixed\n\noptions:\n");
    for (flags, text) in rows {
        if flags.len() <= 20 {
            let _ = writeln!(s, "  {flags:<22}{text}");
        } else {
            let _ = writeln!(s, "  {flags}\n                        {text}");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: &[&str]) -> Args {
        parse(v, "fuck").unwrap()
    }

    #[test]
    fn prepare_like_python() {
        assert_eq!(
            prepare(&["git", "brnch", PLACEHOLDER, "-y"]),
            ["-y", "--", "git", "brnch"]
        );
        assert_eq!(prepare(&[PLACEHOLDER]), ["--"]);
        assert_eq!(prepare(&["git", "brnch"]), ["--", "git", "brnch"]);
        assert_eq!(prepare(&["-a"]), ["-a"]);
    }

    #[test]
    fn parses_like_argparse() {
        assert_eq!(p(&[]), Args::default());
        assert_eq!(p(&["-a"]).alias.as_deref(), Some("fuck"));
        assert!(p(&["--alias"]).default_aliases);
        assert_eq!(p(&["-a", "fix"]).alias.as_deref(), Some("fix"));
        assert!(!p(&["--alias", "typo"]).default_aliases);
        assert_eq!(p(&["--alias=fix"]).alias.as_deref(), Some("fix"));
        assert_eq!(p(&["--al", "fix"]).alias.as_deref(), Some("fix"));
        assert_eq!(p(&["ls", "-la"]).command, ["ls", "-la"]);
        let a = p(&["git", "push", PLACEHOLDER, "-yd"]);
        assert!(a.yes && a.debug);
        assert_eq!(a.command, ["git", "push"]);
        assert_eq!(
            p(&["--force-command", "ls -l"]).force_command.as_deref(),
            Some("ls -l")
        );
        assert_eq!(
            p(&["git", "checkout", "--", "f"]).command,
            ["git", "checkout", "--", "f"]
        );
        assert!(p(&["--hard"]).yes);
        assert!(p(&["--rep"]).repeat);
        assert!(p(&["-l", "/tmp/log"]).shell_logger.is_some());
    }

    #[test]
    fn errors_like_argparse() {
        assert!(
            parse(&["-y", "-r"], "fuck")
                .unwrap_err()
                .contains("not allowed with")
        );
        assert!(parse(&["--ye"], "fuck").unwrap_err().contains("ambiguous"));
        assert!(
            parse(&["--nope"], "fuck")
                .unwrap_err()
                .contains("unrecognized")
        );
        assert!(
            parse(&["-l"], "fuck")
                .unwrap_err()
                .contains("expected one argument")
        );
    }
}
