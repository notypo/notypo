use super::{one, output_contains_lower};
use crate::types::{Command, Rule};
use crate::utils::{
    capture, capture_all, closest_match, expand_user, get_close_matches, get_closest,
    replace_argument, replace_command,
};
use std::collections::HashMap;

pub(super) const RULES: &[Rule] = &[
    Rule::new("sudo", sudo_match, sudo_fix),
    Rule::new(
        "unsudo",
        |c| {
            c.part(0) == "sudo"
                && c.output_lower()
                    .contains("you cannot perform this operation as root")
        },
        |c| one(c.script_parts()[1..].join(" ")),
    ),
    Rule::new(
        "sudo_command_from_user_path",
        |c| {
            c.is_app(&["sudo"]) && sudo_missing(c).is_some_and(|name| c.ctx().which(name).is_some())
        },
        |c| {
            sudo_missing(c).map_or_else(Vec::new, |name| {
                one(replace_argument(
                    &c.script,
                    name,
                    &format!("env \"PATH=$PATH\" {name}"),
                ))
            })
        },
    ),
    sudo_rule!("no_command", no_command_match, no_command_fix).priority(3000),
    Rule::new(
        "unknown_command",
        |c| unknown_command(c).is_some() && c.output().contains("Did you mean "),
        |c| {
            unknown_command(c).map_or_else(Vec::new, |broken| {
                replace_command(
                    c,
                    broken,
                    &capture_all(regex!(r"Did you mean ([^?]*)"), c.output()),
                )
            })
        },
    ),
    Rule::new(
        "history",
        |c| !get_close_matches(&c.script, &c.valid_history_without_current(), 3, 0.6).is_empty(),
        |c| {
            get_closest(&c.script, &c.valid_history_without_current())
                .into_iter()
                .collect()
        },
    )
    .priority(9999),
    Rule::new(
        "missing_space_before_subcommand",
        |c| !known_executable(c, c.part(0)) && executable_prefix(c).is_some(),
        |c| {
            executable_prefix(c).map_or_else(Vec::new, |exe| {
                one(c.script.replacen(exe, &format!("{exe} "), 1))
            })
        },
    )
    .priority(4000),
    sudo_rule!(
        "wrong_hyphen_before_subcommand",
        |c| !known_executable(c, c.part(0))
            && c.part(0)
                .split_once('-')
                .is_some_and(|(exe, _)| known_executable(c, exe)),
        |c| one(c.script.replacen('-', " ", 1))
    )
    .priority(4500)
    .requires_output(false),
    Rule::new(
        "path_from_history",
        |c| missing_destination(c).is_some(),
        path_from_history,
    )
    .priority(800),
    sudo_rule!(
        "python_command",
        |c| c.part(0).ends_with(".py")
            && (c.output().contains("Permission denied")
                || c.output().contains("command not found")),
        |c| one(format!("python {}", c.script))
    ),
    Rule::new(
        "python_execute",
        |c| c.is_app(&["python"]) && !c.script.ends_with(".py"),
        |c| one(format!("{}.py", c.script)),
    ),
    Rule::new(
        "python_module_error",
        |c| missing_module(c).is_some(),
        |c| {
            missing_module(c).map_or_else(Vec::new, |module| {
                one(c
                    .shell()
                    .and_(&[&format!("pip install {module}"), &c.script]))
            })
        },
    ),
    sudo_rule!(
        "fix_alt_space",
        |c| c.output_lower().contains("command not found") && c.script.contains('\u{a0}'),
        |c| one(c.script.replace('\u{a0}', " "))
    ),
    Rule::new(
        "quotation_marks",
        |c| c.script.contains('\'') && c.script.contains('"'),
        |c| one(c.script.replace('\'', "\"")),
    ),
    Rule::new(
        "remove_shell_prompt_literal",
        |c| {
            c.output().contains("$: command not found") && regex!(r"^\s*\$ \S+").is_match(&c.script)
        },
        |c| one(c.script.trim_start_matches(['$', ' '])),
    ),
    Rule::new(
        "remove_trailing_cedilla",
        |c| c.script.ends_with('ç'),
        |c| one(c.script.strip_suffix('ç').unwrap_or(&c.script)),
    ),
    Rule::new(
        "dry",
        |c| c.script_parts().len() >= 2 && c.part(0) == c.part(1),
        |c| one(c.script_parts()[1..].join(" ")),
    )
    .priority(900),
    Rule::new(
        "test.py",
        |c| c.script == "test.py" && c.output().contains("not found"),
        |_| one("pytest"),
    )
    .priority(900),
    Rule::new(
        "long_form_help",
        |c| help_command(c).is_some() || c.output().contains("--help"),
        |c| {
            one(help_command(c).map_or_else(
                || replace_argument(&c.script, "-h", "--help"),
                str::to_owned,
            ))
        },
    )
    .priority(5000),
    Rule::new("sl_ls", |c| c.script == "sl", |_| one("ls")),
    Rule::new(
        "scm_correction",
        |c| {
            let pattern = match c.part(0) {
                "git" => "fatal: Not a git repository",
                "hg" => "abort: no repository found",
                _ => return false,
            };
            c.output().contains(pattern) && actual_scm().is_some()
        },
        |c| {
            actual_scm().map_or_else(Vec::new, |scm| {
                one(std::iter::once(scm)
                    .chain(c.script_parts()[1..].iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join(" "))
            })
        },
    ),
];

const SUDO_ERRORS: &[&str] = &[
    "permission denied",
    "eacces",
    "pkg: insufficient privileges",
    "you cannot perform this operation unless you are root",
    "non-root users cannot",
    "operation not permitted",
    "not super-user",
    "superuser privilege",
    "root privilege",
    "this command has to be run under the root user.",
    "this operation requires root.",
    "requested operation requires superuser privilege",
    "must be run as root",
    "must run as root",
    "must be superuser",
    "must be root",
    "need to be root",
    "need root",
    "needs to be run as root",
    "only root can ",
    "you don't have access to the history db.",
    "authentication is required",
    "edspermissionerror",
    "you don't have write permissions",
    "use `sudo`",
    "sudorequirederror",
    "error: insufficient privileges",
    "updatedb: can not open a temporary file",
];

fn sudo_match(c: &Command) -> bool {
    !(c.part(0) == "sudo" && !c.has_part("&&")) && output_contains_lower(c, SUDO_ERRORS)
}

fn sudo_fix(c: &Command) -> Vec<String> {
    if c.script.contains("&&") {
        one(format!(
            "sudo sh -c \"{}\"",
            c.script_parts()
                .iter()
                .filter(|p| p.as_str() != "sudo")
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        ))
    } else if c.script.contains('>') {
        one(format!("sudo sh -c \"{}\"", c.script.replace('"', "\\\"")))
    } else {
        one(format!("sudo {}", c.script))
    }
}

fn sudo_missing<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(regex!(r"sudo: (.*): command not found"), c.output())
}
fn unknown_command<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(regex!(r"([^:]*): Unknown command.*"), c.output())
}
fn missing_module<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(
        regex!(r"ModuleNotFoundError: No module named '([^']+)'"),
        c.output(),
    )
}
fn help_command<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(
        regex!(r"(?i)(?:Run|Try) '([^']+)'(?: or '[^']+')? for (?:details|more information)."),
        c.output(),
    )
}

fn known_executable(c: &Command, executable: &str) -> bool {
    c.ctx().executables().iter().any(|name| name == executable)
}
fn executable_prefix<'c>(c: &'c Command) -> Option<&'c str> {
    c.ctx()
        .executables()
        .iter()
        .find(|exe| exe.len() > 1 && c.part(0).starts_with(exe.as_str()))
        .map(String::as_str)
}

fn no_command_match(c: &Command) -> bool {
    !c.part(0).is_empty()
        && c.ctx().which(c.part(0)).is_none()
        && (c.output().contains("not found") || c.output().contains("is not recognized as"))
        && !get_close_matches(c.part(0), c.ctx().executables(), 3, 0.6).is_empty()
}

fn no_command_fix(c: &Command) -> Vec<String> {
    let history = c.valid_history_without_current();
    let used: Vec<_> = history
        .iter()
        .filter_map(|line| line.split(' ').next())
        .collect();
    let mut names: Vec<_> = closest_match(c.part(0), &used, 0.6).into_iter().collect();
    for name in get_close_matches(c.part(0), c.ctx().executables(), 3, 0.6) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
        .into_iter()
        .map(|name| c.script.replacen(c.part(0), &name, 1))
        .collect()
}

fn missing_destination<'a>(c: &Command<'a>) -> Option<&'a str> {
    [
        regex!(r"no such file or directory: (.*)$"),
        regex!(r"cannot access '(.*)': No such file or directory"),
        regex!(r": (.*): No such file or directory"),
        regex!(r"can't cd to (.*)$"),
    ]
    .into_iter()
    .filter_map(|re| capture(re, c.output()))
    .find(|dest| c.has_part(dest))
}

fn path_from_history(c: &Command) -> Vec<String> {
    let Some(dest) = missing_destination(c) else {
        return Vec::new();
    };
    let mut paths: Vec<(String, usize)> = Vec::new();
    let mut indexes = HashMap::new();
    for line in c.valid_history_without_current() {
        for path in c
            .shell()
            .split_command(&line)
            .into_iter()
            .skip(1)
            .filter(|p| p.starts_with(['/', '~']))
        {
            let path = path.strip_suffix('/').unwrap_or(&path).to_owned();
            let index = *indexes.entry(path.clone()).or_insert_with(|| {
                paths.push((path, 0));
                paths.len() - 1
            });
            paths[index].1 += 1;
        }
    }
    paths.sort_by_key(|path| std::cmp::Reverse(path.1));
    paths
        .into_iter()
        .filter(|(path, _)| path.ends_with(dest) && expand_user(path).exists())
        .map(|(path, _)| replace_argument(&c.script, dest, &path))
        .collect()
}

fn actual_scm() -> Option<&'static str> {
    [(".git", "git"), (".hg", "hg")]
        .into_iter()
        .find_map(|(dir, scm)| std::path::Path::new(dir).is_dir().then_some(scm))
}
