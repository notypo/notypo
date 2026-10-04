//! Locating the suspicious token: error-output diagnostics, operational
//! failures, and the effective program behind wrappers such as `sudo`.
//!
//! The patterns describe common diagnostic formats (argparse, getopt,
//! cobra, clap, shells), not the commands of any particular app. Printed
//! suggestions are untrusted text: they only ever raise the score of a
//! replacement the engine already considers, or become a quoted word.

use super::parser::Word;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    CommandNotFound,
    UnknownCommand,
    UnknownOption,
    MissingPath,
}

/// A token the output says is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub kind: ProblemKind,
    pub token: String,
}

/// Failures that a spelling change can't fix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationalFailure {
    Permission,
    Authentication,
    Network,
}

impl OperationalFailure {
    pub fn describe(self) -> &'static str {
        match self {
            OperationalFailure::Permission => "a permission problem",
            OperationalFailure::Authentication => "an authentication problem",
            OperationalFailure::Network => "a network problem",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputDiagnosis {
    pub problems: Vec<Problem>,
    /// Words from "did you mean" style hints.
    pub suggestions: Vec<String>,
    pub operational: Option<OperationalFailure>,
}

impl OutputDiagnosis {
    /// Whether the output names `token` as a problem of `kind`. Options are
    /// compared without dashes: git prints ``unknown option `amend'``.
    pub fn mentions(&self, kind: ProblemKind, token: &str) -> bool {
        let bare = |t: &str| {
            if kind == ProblemKind::UnknownOption {
                t.trim_start_matches('-').to_owned()
            } else {
                t.to_owned()
            }
        };
        self.problems
            .iter()
            .any(|p| p.kind == kind && bare(&p.token) == bare(token))
    }
}

pub fn diagnose_output(output: &str) -> OutputDiagnosis {
    let mut diagnosis = OutputDiagnosis::default();
    let patterns: [(ProblemKind, &regex::Regex); 20] = [
        (
            ProblemKind::CommandNotFound,
            regex!(r"(?m)(?:^|: )(?:line \d+: |\d+: )?([^\s:'`]+): (?:command )?not found\s*$"),
        ),
        (
            ProblemKind::CommandNotFound,
            regex!(r"command not found: (\S+)"),
        ),
        (
            ProblemKind::CommandNotFound,
            regex!(r"(?m)^fish: Unknown command:? '?([^'\s]+)"),
        ),
        (
            ProblemKind::CommandNotFound,
            regex!(r"'([^']+)' is not recognized as an internal or external command"),
        ),
        (
            ProblemKind::CommandNotFound,
            regex!(r"The term '([^']+)' is not recognized"),
        ),
        (
            ProblemKind::UnknownCommand,
            regex!(r#"(?i)invalid choice:? ['"‘]([^'"’]+)['"’]"#),
        ),
        (
            ProblemKind::UnknownCommand,
            regex!(r#"["'‘]([^"'’\s]+)["'’] is not an? [\w .-]+ command"#),
        ),
        (
            ProblemKind::UnknownCommand,
            regex!(
                r#"(?i)(?:unknown|unrecognized|no such) (?:sub)?command:? ["'`]?([^"'`\s]+)["'`]?"#
            ),
        ),
        (
            ProblemKind::UnknownCommand,
            regex!(r"'([^']+)' is not in the '[^']+' command group"),
        ),
        (
            ProblemKind::UnknownOption,
            regex!(r#"(?i)unrecognized option:? ['"`‘]?(-[^'"`’\s=]+)"#),
        ),
        (
            ProblemKind::UnknownOption,
            regex!(r#"(?i)unknown (?:option|flag|argument):? ['"`]?(-{1,2}[^'"`\s=:,]+)"#),
        ),
        (
            ProblemKind::UnknownOption,
            regex!(r"(?i)unknown options?: (-[^\s,=]+)"),
        ),
        (
            ProblemKind::UnknownOption,
            regex!(r"(?i)unknown (?:option|switch) `([^'\s=]+)'"),
        ),
        (
            ProblemKind::UnknownOption,
            regex!(r"(?i)unrecognized argument: (-[^\s=]+)"),
        ),
        (
            ProblemKind::UnknownOption,
            regex!(r"(?i)unrecognized arguments?: (-[^\s=]+)"),
        ),
        (
            ProblemKind::UnknownOption,
            regex!(r"(?i)no such option:? (-[^\s=]+)"),
        ),
        (
            ProblemKind::UnknownOption,
            regex!(r"(?i)unexpected argument '(-[^'\s=]+)"),
        ),
        (
            ProblemKind::UnknownOption,
            regex!(r"flag provided but not defined: (-[^\s=]+)"),
        ),
        (
            ProblemKind::MissingPath,
            regex!(r"(?m)([^\s:'`]+): No such file or directory"),
        ),
        (ProblemKind::MissingPath, regex!(r"cannot access '([^']+)'")),
    ];
    for (kind, pattern) in patterns {
        for caps in pattern.captures_iter(output) {
            let token = caps[1].to_owned();
            if !diagnosis.mentions(kind, &token) {
                diagnosis.problems.push(Problem { kind, token });
            }
        }
    }
    diagnosis.suggestions = suggestions(output);
    diagnosis.operational = operational(output);
    diagnosis
}

/// Words offered by hints such as `Did you mean 'status'?` or a header
/// followed by indented alternatives.
fn suggestions(output: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut add = |text: &str| {
        for word in text.split(|c: char| c.is_whitespace() || c == ',') {
            let word = word.trim_matches(|c: char| "'\"`‘’“”?.;:()".contains(c));
            if !word.is_empty() && !words.iter().any(|w| w == word) {
                words.push(word.to_owned());
            }
        }
    };
    let header = regex!(
        r"(?i)(did you mean|most similar (?:command|choice)|maybe you meant|similar (?:sub)?commands? (?:exists?|are|is)|possible alternatives?)"
    );
    let mut in_list = false;
    for line in output.lines() {
        if let Some(m) = header.find(line) {
            // Inline: `Did you mean 'status'?` or `... exists: 'status'`. In
            // `most similar choice to 'acount' is:` the typo precedes "is".
            let rest = &line[m.end()..];
            let rest = rest
                .split_once(" is")
                .or_else(|| rest.split_once(" are"))
                .map_or(rest, |(_, after)| after)
                .trim_start_matches(|c: char| !c.is_alphanumeric() && c != '-');
            let rest = ["one of these", "these", "this"]
                .into_iter()
                .find_map(|filler| rest.strip_prefix(filler))
                .unwrap_or(rest);
            add(rest);
            in_list = true;
        } else if in_list && line.starts_with(char::is_whitespace) && !line.trim().is_empty() {
            add(line);
        } else if !line.trim().is_empty() {
            in_list = false;
        }
    }
    words
}

fn operational(output: &str) -> Option<OperationalFailure> {
    if regex!(
        r"(?i)unable to locate credentials|expiredtoken|token (?:has )?expired|credentials? (?:have|has) expired|please run ['`]?(?:az|gcloud auth) login|not logged in|unauthenticated|authentication failed|invalid ?credentials|reauthentication"
    )
    .is_match(output)
    {
        Some(OperationalFailure::Authentication)
    } else if regex!(
        r"(?i)permission denied|access ?denied|unauthorizedoperation|not authorized|forbidden|operation not permitted"
    )
    .is_match(output)
    {
        Some(OperationalFailure::Permission)
    } else if regex!(
        r"(?i)could not connect|connection (?:refused|timed out|reset)|could not resolve host|name or service not known|network is unreachable|temporary failure in name resolution|endpointconnectionerror|no route to host"
    )
    .is_match(output)
    {
        Some(OperationalFailure::Network)
    } else {
        None
    }
}

/// Wrappers that run another program, with their options that take a value.
const WRAPPERS: &[(&str, &[&str])] = &[
    (
        "sudo",
        &[
            "-u",
            "-g",
            "-h",
            "-p",
            "-C",
            "-D",
            "-r",
            "-t",
            "-U",
            "-T",
            "-R",
            "--user",
            "--group",
            "--host",
            "--prompt",
            "--close-from",
            "--chdir",
            "--role",
            "--type",
            "--other-user",
            "--command-timeout",
            "--chroot",
        ],
    ),
    ("doas", &["-u", "-C"]),
    (
        "env",
        &[
            "-u",
            "--unset",
            "-C",
            "--chdir",
            "-S",
            "--split-string",
            "-P",
        ],
    ),
    ("nohup", &[]),
    ("time", &["-f", "-o", "--format", "--output"]),
    ("command", &[]),
    ("builtin", &[]),
    ("exec", &["-a"]),
    ("nice", &["-n", "--adjustment"]),
    ("ionice", &["-c", "-n", "--class", "--classdata"]),
    (
        "stdbuf",
        &["-i", "-o", "-e", "--input", "--output", "--error"],
    ),
    ("timeout", &["-s", "-k", "--signal", "--kill-after"]),
    ("noglob", &[]),
    ("nocorrect", &[]),
];

/// Package runners: the program they run comes from a package, which may
/// have to be downloaded. Options listed take a value; `-c`/`--call` run a
/// shell string instead of a program.
const RUNNERS: &[(&[&str], Ecosystem, &[&str])] = &[
    (
        &["npx"],
        Ecosystem::Node,
        &["-p", "--package", "--registry", "--cache"],
    ),
    (&["pnpx"], Ecosystem::Node, &[]),
    (&["bunx"], Ecosystem::Node, &["-p", "--package"]),
    (
        &["npm", "exec"],
        Ecosystem::Node,
        &["-p", "--package", "--registry", "-w", "--workspace"],
    ),
    (&["npm", "x"], Ecosystem::Node, &["-p", "--package"]),
    (
        &["pnpm", "exec"],
        Ecosystem::Node,
        &["-C", "--dir", "--filter"],
    ),
    (&["pnpm", "dlx"], Ecosystem::Node, &["--package"]),
    (&["yarn", "dlx"], Ecosystem::Node, &["-p", "--package"]),
    (&["yarn", "exec"], Ecosystem::Node, &[]),
    (&["bun", "x"], Ecosystem::Node, &["-p", "--package"]),
    (
        &["uvx"],
        Ecosystem::Python,
        &["--from", "--with", "-p", "--python", "--index-url"],
    ),
    (
        &["pipx", "run"],
        Ecosystem::Python,
        &["--spec", "--python", "--index-url"],
    ),
    (
        &["uv", "run"],
        Ecosystem::Python,
        &["--with", "-p", "--python", "--project", "--directory"],
    ),
    (&["poetry", "run"], Ecosystem::Python, &[]),
    (&["bundle", "exec"], Ecosystem::Ruby, &["--gemfile"]),
];

/// Where a package runner finds the program it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ecosystem {
    Node,
    Python,
    Ruby,
}

/// Index of the word naming the program that actually runs, skipping
/// wrappers and their options; `None` when nothing is executed (`sudo -l`,
/// `command -v git`) or a wrapper is the last word.
pub fn effective_program(words: &[Word]) -> Option<usize> {
    effective_program_via(words).map(|(i, _)| i)
}

/// [`effective_program`], and the package runner it is run through.
pub fn effective_program_via(words: &[Word]) -> Option<(usize, Option<Ecosystem>)> {
    let mut i = 0;
    let mut runner = None;
    loop {
        let name = words.get(i)?.literal()?;
        let base = name.rsplit('/').next().unwrap_or(name);
        let next = words.get(i + 1).and_then(Word::literal);
        if let Some((prefix, ecosystem, with_value)) = RUNNERS.iter().find(|(prefix, _, _)| {
            prefix[0] == base && (prefix.len() == 1 || Some(prefix[1]) == next)
        }) {
            i += prefix.len();
            runner = Some(*ecosystem);
            while let Some(arg) = words.get(i).map(Word::literal) {
                let arg = arg?;
                if arg == "--" {
                    i += 1;
                    break;
                }
                if !arg.starts_with('-') || arg.len() < 2 {
                    break;
                }
                if matches!(arg, "-c" | "--call") || arg.starts_with("--call=") {
                    return None;
                }
                i += if with_value.contains(&arg) { 2 } else { 1 };
            }
            continue;
        }
        let Some((wrapper, with_value)) = WRAPPERS.iter().find(|(w, _)| *w == base) else {
            return Some((i, runner));
        };
        i += 1;
        let mut positional_duration = *wrapper == "timeout";
        while let Some(word) = words.get(i) {
            let arg = word.literal()?;
            if arg == "--" {
                i += 1;
                break;
            }
            if *wrapper == "env" && !arg.starts_with('-') && arg.contains('=') {
                i += 1;
                continue;
            }
            if arg.starts_with('-') && arg.len() > 1 {
                if *wrapper == "command" && matches!(arg, "-v" | "-V") {
                    return None;
                }
                if matches!(*wrapper, "sudo" | "doas")
                    && matches!(arg, "-l" | "-v" | "-k" | "-K" | "--list" | "--validate")
                {
                    return None;
                }
                i += 1;
                if with_value.contains(&arg) {
                    i += 1;
                }
                continue;
            }
            if positional_duration {
                positional_duration = false;
                i += 1;
                continue;
            }
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::parser::parse;

    fn program(src: &str) -> Option<String> {
        let script = parse(src);
        let words = &script.commands[0].words;
        effective_program(words).map(|i| words[i].literal().unwrap().to_owned())
    }

    #[test]
    fn wrappers_and_their_options_are_skipped() {
        assert_eq!(program("git status").as_deref(), Some("git"));
        assert_eq!(program("sudo -u root -E aws s3 ls").as_deref(), Some("aws"));
        assert_eq!(program("sudo --user=root gti").as_deref(), Some("gti"));
        assert_eq!(program("env -i A=1 B=2 az vm list").as_deref(), Some("az"));
        assert_eq!(
            program("timeout -s KILL 5 gcloud info").as_deref(),
            Some("gcloud")
        );
        assert_eq!(program("nice -n 5 time -p make").as_deref(), Some("make"));
        assert_eq!(program("sudo -- ls").as_deref(), Some("ls"));
        assert_eq!(program("/usr/bin/sudo ls").as_deref(), Some("ls"));
        assert_eq!(program("sudo"), None);
        assert_eq!(program("sudo -l"), None);
        assert_eq!(program("command -v git"), None);
        assert_eq!(program("sudo $CMD"), None);
    }

    #[test]
    fn package_runners_are_looked_through() {
        let via = |src: &str| {
            let script = parse(src);
            let words = &script.commands[0].words;
            effective_program_via(words).map(|(i, e)| (words[i].literal().unwrap().to_owned(), e))
        };
        assert_eq!(
            via("npx cdk deploy"),
            Some(("cdk".into(), Some(Ecosystem::Node)))
        );
        assert_eq!(
            via("npx -p aws-cdk@2 cdk diff"),
            Some(("cdk".into(), Some(Ecosystem::Node)))
        );
        assert_eq!(
            via("pnpm dlx create-vite app"),
            Some(("create-vite".into(), Some(Ecosystem::Node)))
        );
        assert_eq!(
            via("uvx ruff check"),
            Some(("ruff".into(), Some(Ecosystem::Python)))
        );
        assert_eq!(
            via("sudo bundle exec rake db:migrate"),
            Some(("rake".into(), Some(Ecosystem::Ruby)))
        );
        assert_eq!(via("npm install"), Some(("npm".into(), None)));
        assert_eq!(via("npx -c 'echo hi'"), None);
    }

    #[test]
    fn diagnoses_common_error_formats() {
        let cases = [
            (
                "bash: gti: command not found",
                ProblemKind::CommandNotFound,
                "gti",
            ),
            (
                "zsh: command not found: gti",
                ProblemKind::CommandNotFound,
                "gti",
            ),
            ("sh: 1: gti: not found", ProblemKind::CommandNotFound, "gti"),
            (
                "aws: [ERROR]: argument operation: Found invalid choice 'describ-instances'",
                ProblemKind::UnknownCommand,
                "describ-instances",
            ),
            (
                "ERROR: (gcloud.compute) Invalid choice: 'instnaces'.",
                ProblemKind::UnknownCommand,
                "instnaces",
            ),
            (
                "az storage: 'acount' is not in the 'az storage' command group.",
                ProblemKind::UnknownCommand,
                "acount",
            ),
            (
                "git: 'sttus' is not a git command. See 'git --help'.",
                ProblemKind::UnknownCommand,
                "sttus",
            ),
            (
                "Error: unknown command \"gt\" for \"kubectl\"",
                ProblemKind::UnknownCommand,
                "gt",
            ),
            (
                "error: no such command: `biuld`",
                ProblemKind::UnknownCommand,
                "biuld",
            ),
            (
                "ls: unrecognized option '--colro=auto'",
                ProblemKind::UnknownOption,
                "--colro",
            ),
            (
                "Unknown options: --regoin, eu-west-1",
                ProblemKind::UnknownOption,
                "--regoin",
            ),
            (
                "Error: unknown flag: --nmespace",
                ProblemKind::UnknownOption,
                "--nmespace",
            ),
            (
                "error: unexpected argument '--relese' found",
                ProblemKind::UnknownOption,
                "--relese",
            ),
            (
                "error: unknown option `amen'",
                ProblemKind::UnknownOption,
                "--amen",
            ),
            (
                "fatal: unrecognized argument: --grpah",
                ProblemKind::UnknownOption,
                "--grpah",
            ),
            (
                "cat: fiel.txt: No such file or directory",
                ProblemKind::MissingPath,
                "fiel.txt",
            ),
        ];
        for (output, kind, token) in cases {
            let diagnosis = diagnose_output(output);
            assert!(
                diagnosis.mentions(kind, token),
                "{output:?}: {:?}",
                diagnosis.problems
            );
        }
        assert!(diagnose_output("all good").problems.is_empty());
    }

    #[test]
    fn extracts_suggestions() {
        let git = "git: 'sttus' is not a git command. See 'git --help'.\n\nThe most similar command is\n\tstatus\n";
        assert_eq!(diagnose_output(git).suggestions, ["status"]);
        let gcloud = "ERROR: (gcloud.compute) Invalid choice: 'instnaces'.\nMaybe you meant:\n  gcloud compute instances\n";
        assert!(
            diagnose_output(gcloud)
                .suggestions
                .contains(&"instances".to_owned())
        );
        let az = "The most similar choice to 'acount' is:\n\taccount\n";
        assert_eq!(diagnose_output(az).suggestions, ["account"]);
        let clap = "  tip: a similar subcommand exists: 'build'\n";
        assert_eq!(diagnose_output(clap).suggestions, ["build"]);
        let cargo = "Did you mean `build`?";
        assert_eq!(diagnose_output(cargo).suggestions, ["build"]);
    }

    #[test]
    fn recognizes_operational_failures() {
        for (output, expected) in [
            (
                "An error occurred (ExpiredToken) when calling",
                OperationalFailure::Authentication,
            ),
            (
                "Unable to locate credentials.",
                OperationalFailure::Authentication,
            ),
            (
                "Please run 'az login' to setup account.",
                OperationalFailure::Authentication,
            ),
            (
                "An error occurred (AccessDenied) when calling",
                OperationalFailure::Permission,
            ),
            (
                "mkdir: /x: Permission denied",
                OperationalFailure::Permission,
            ),
            (
                "Could not connect to the endpoint URL",
                OperationalFailure::Network,
            ),
        ] {
            assert_eq!(
                diagnose_output(output).operational,
                Some(expected),
                "{output}"
            );
        }
    }
}
