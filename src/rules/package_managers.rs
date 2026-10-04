use super::{one, output_contains};
use crate::specific::{self, apt::apt_available, archlinux, brew, npm};
use crate::types::{Command, Rule};
use crate::utils::{
    self, capture, capture_all, get_closest, replace_argument, replace_command, run_stdout,
};
use std::fs;
use std::path::Path;

pub(super) const RULES: &[Rule] = &[
    Rule::new("apt_get", |c| output_contains(c, &["not found", "not installed"]) && c.ctx().which(executable(c)).is_none() && apt_package(c).is_some(),
        |c| apt_package(c).map_or_else(Vec::new, |package| one(c.shell().and_(&[&format!("sudo apt-get install {package}"), &c.script])))).enabled_by_default(apt_available),
    Rule::new("apt_get_search", |c| c.is_app(&["apt-get"]) && c.script.starts_with("apt-get search"),
        |c| one(c.script.replacen("apt-get", "apt-cache", 1))).enabled_by_default(apt_available),
    sudo_rule!("apt_invalid_operation", |c| c.is_app(&["apt", "apt-get", "apt-cache"]) && c.output().contains("E: Invalid operation"), apt_operation_fix).enabled_by_default(apt_available),
    sudo_rule!("apt_list_upgradable", |c| c.is_app(&["apt"]) && c.output().contains("apt list --upgradable"), |_| one("apt list --upgradable")).enabled_by_default(apt_available),
    sudo_rule!("apt_upgrade", |c| c.is_app(&["apt"]) && c.script == "apt list --upgradable" && c.output().trim().split('\n').count() > 1,
        |_| one("apt upgrade")).enabled_by_default(apt_available),
    Rule::new("brew_cask_dependency", |c| c.is_app(&["brew"]) && c.has_part("install") && c.output().contains("brew cask install"), |c| {
        let mut commands: Vec<_> = c.output().lines().map(str::trim).filter(|line| line.starts_with("brew cask install")).collect();
        commands.push(&c.script);
        one(c.shell().and_(&commands))
    }).enabled_by_default(brew::brew_available),
    Rule::new("brew_install", |c| c.is_app_at_least(&["brew"], 2) && c.script.contains("install") && c.output().contains("No available formula") && c.output().contains("Did you mean"),
        |c| capture(regex!(r#"Warning: No available formula with the name "(?:[^"]+)". Did you mean (.+)\?"#), c.output())
            .map_or_else(Vec::new, |options| options.replace(" or ", ", ").split(", ").map(|formula| format!("brew install {formula}")).collect())).enabled_by_default(brew::brew_available),
    Rule::new("brew_link", |c| c.is_app_at_least(&["brew"], 2) && ["ln", "link"].contains(&c.part(1)) && c.output().contains("brew link --overwrite --dry-run"),
        |c| brew_insert(c, "link", &["--overwrite", "--dry-run"])),
    Rule::new("brew_reinstall", |c| c.is_app_at_least(&["brew"], 2) && c.script.contains("install")
        && regex!(r"Warning: .+ is already installed and up-to-date").is_match(c.output())
        && regex!(r"To reinstall [^\n]+, run `brew reinstall [^`]+`").is_match(c.output()), |c| one(c.script.replace("install", "reinstall"))),
    Rule::new("brew_uninstall", |c| c.is_app_at_least(&["brew"], 2) && ["uninstall", "rm", "remove"].contains(&c.part(1)) && c.output().contains("brew uninstall --force"),
        |c| brew_insert(c, "uninstall", &["--force"])),
    Rule::new("brew_unknown_command", |c| c.script.contains("brew") && brew_unknown(c).is_some(),
        |c| brew_unknown(c).map_or_else(Vec::new, |broken| replace_command(c, broken, &brew_commands()))).enabled_by_default(brew::brew_available),
    Rule::new("brew_update_formula", |c| c.is_app_at_least(&["brew"], 2) && c.script.contains("update") && c.output().contains("Error: This command updates brew itself") && c.output().contains("Use `brew upgrade"),
        |c| one(c.script.replace("update", "upgrade"))),
    Rule::new("cargo", |c| c.script == "cargo", |_| one("cargo build")),
    Rule::new("cargo_no_command", |c| c.is_app_at_least(&["cargo"], 1) && c.output_lower().contains("no such subcommand") && c.output().contains("Did you mean"),
        |c| capture(regex!(r"Did you mean `([^`]*)`"), c.output()).map_or_else(Vec::new, |fix| one(replace_argument(&c.script, c.part(1), fix)))),
    Rule::new("choco_install", |c| c.is_app(&["choco", "cinst"]) && (c.script.starts_with("choco install") || c.has_part("cinst")) && c.output().contains("Installing the following packages"), |c| {
        c.script_parts().iter().find(|part| !["choco", "cinst", "install"].contains(&part.as_str()) && !part.starts_with('-') && !part.contains(['=', '/']))
            .map_or_else(Vec::new, |package| one(c.script.replace(package, &format!("{package}.install"))))
    }).enabled_by_default(|| utils::which("choco").is_some() || utils::which("cinst").is_some()),
    sudo_rule!("dnf_no_such_command", |c| c.is_app(&["dnf"]) && c.output_lower().contains("no such command"), |c| {
        capture(regex!(r"No such command: (.*)\."), c.output()).map_or_else(Vec::new, |broken| {
            let help = run_stdout("dnf", &["--help"]).unwrap_or_default();
            replace_command(c, broken, &capture_all(regex!(r"(?m)^([a-z-]+) +"), &help))
        })
    }).enabled_by_default(specific::dnf_available),
    Rule::new("npm_missing_script", |c| c.is_app(&["npm"]) && c.script_parts().iter().any(|p| p.starts_with("ru")) && c.output().contains("npm ERR! missing script: "),
        |c| capture(regex!(r".*missing script: (.*)\n"), c.output()).map_or_else(Vec::new, |broken| replace_command(c, broken, npm::get_scripts()))).enabled_by_default(npm::npm_available),
    Rule::new("npm_run_script", |c| c.is_app_at_least(&["npm"], 1) && c.output().contains("Usage: npm <command>")
        && !c.script_parts().iter().any(|p| p.starts_with("ru")) && npm::get_scripts().iter().any(|script| script == c.part(1)), |c| {
        let mut parts = c.script_parts().to_vec(); parts.insert(1, "run-script".into()); one(parts.join(" "))
    }).enabled_by_default(npm::npm_available),
    sudo_rule!("npm_wrong_command", |c| c.is_app(&["npm"]) && c.output().contains("where <command> is one of:") && npm_wrong(c).is_some(), npm_wrong_fix).enabled_by_default(npm::npm_available),
    Rule::new("pacman", |c| c.output().contains("not found") && !archlinux::get_pkgfile(&c.script).is_empty(), |c| {
        archlinux::archlinux_env().1.map_or_else(Vec::new, |pacman| archlinux::get_pkgfile(&c.script).into_iter()
            .map(|package| c.shell().and_(&[&format!("{pacman} -S {package}"), &c.script])).collect())
    }).enabled_by_default(arch_enabled),
    sudo_rule!("pacman_invalid_option", |c| c.is_app(&["pacman"]) && c.output().starts_with("error: invalid option '-") && regex!(r" -[surqfdvt]").is_match(&c.script),
        |c| capture(regex!(r" -[dfqrstuv]"), &c.script).map_or_else(Vec::new, |option| one(c.script.replace(option, &option.to_uppercase())))).enabled_by_default(arch_enabled),
    Rule::new("pacman_not_found", |c| (c.is_app(&["pacman", "yay", "pikaur", "yaourt"]) || c.part(0) == "sudo" && c.part(1) == "pacman") && c.output().contains("error: target not found:"),
        |c| c.script_parts().last().map_or_else(Vec::new, |package| replace_command(c, package, &archlinux::get_pkgfile(package)))).enabled_by_default(arch_enabled),
    Rule::new("pip_install", |c| crate::specific::sudo::sudo_support_match(c, |c| c.is_app(&["pip"]) && c.script.contains("pip install") && c.output().contains("Permission denied")),
        |c| one(if !c.script.contains("--user") { c.script.replace(" install ", " install --user ") } else { format!("sudo {}", c.script.replace(" --user", "")) })),
    sudo_rule!("pip_unknown_command", |c| c.is_app(&["pip", "pip2", "pip3"]) && c.output().contains("unknown command") && c.output().contains("maybe you meant"), |c| {
        let broken = capture(regex!(r#"ERROR: unknown command "([^"]+)""#), c.output());
        let fix = capture(regex!(r#"maybe you meant "([^"]+)""#), c.output());
        broken.zip(fix).map_or_else(Vec::new, |(from, to)| one(replace_argument(&c.script, from, to)))
    }),
    Rule::new("yarn_alias", |c| c.is_app_at_least(&["yarn"], 1) && c.output().contains("Did you mean"),
        |c| capture(regex!(r#"Did you mean [`"](?:yarn )?([^`"]*)[`"]"#), c.output()).map_or_else(Vec::new, |fix| one(replace_argument(&c.script, c.part(1), fix)))),
    Rule::new("yarn_command_not_found", |c| c.is_app(&["yarn"]) && yarn_unknown(c).is_some(), yarn_unknown_fix),
    Rule::new("yarn_command_replaced", |c| c.is_app_at_least(&["yarn"], 1) && yarn_replacement(c).is_some(), |c| yarn_replacement(c).map_or_else(Vec::new, one)),
    Rule::new("yarn_help", |c| c.is_app_at_least(&["yarn"], 2) && c.part(1) == "help" && c.output().contains("for documentation about this command."),
        |c| capture(regex!(r"Visit ([^ ]*) for documentation about this command."), c.output()).map_or_else(Vec::new, |url| one(utils::open_command(url)))),
    sudo_rule!("yum_invalid_operation", |c| c.is_app(&["yum"]) && c.output().contains("No such command: "), yum_operation_fix).enabled_by_default(specific::yum_available),
    Rule::new("nixos_cmd_not_found", |c| nix_package(c).is_some(), |c| nix_package(c).map_or_else(Vec::new,
        |package| one(c.shell().and_(&[&format!("nix-env -iA {package}"), &c.script])))).enabled_by_default(specific::nix_available),
];

fn arch_enabled() -> bool {
    archlinux::archlinux_env().0
}
fn executable<'c>(c: &'c Command) -> &'c str {
    if c.part(0) == "sudo" {
        c.part(1)
    } else {
        c.part(0)
    }
}
fn brew_unknown<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(regex!(r"Error: Unknown command: ([a-z]+)"), c.output())
}
fn yarn_unknown<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(regex!(r#"error Command "(.*)" not found."#), c.output())
}
fn yarn_replacement<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(regex!(r#"Run "(.*)" instead"#), c.output())
}
fn nix_package<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(regex!(r"nix-env -iA ([^\s]*)"), c.output())
}

/// Use command-not-found's recommendation, or apt-file's native package
/// index, without importing the distribution's Python CommandNotFound module.
fn apt_package(c: &Command) -> Option<String> {
    if let Some(package) = capture(
        regex!(r"(?:sudo )?apt(?:-get)? install ([a-zA-Z0-9][a-zA-Z0-9.+:-]*)"),
        c.output(),
    ) {
        return Some(package.into());
    }
    let executable = executable(c);
    if executable.is_empty() || utils::which("apt-file").is_none() {
        return None;
    }
    let pattern = format!(r"/bin/{}$", regex::escape(executable));
    run_stdout("apt-file", &["search", "--regexp", &pattern])?
        .lines()
        .find_map(|line| line.split_once(": ").map(|(package, _)| package.to_owned()))
}

fn apt_operation_fix(c: &Command) -> Vec<String> {
    let Some(broken) = c.output().split_whitespace().next_back() else {
        return Vec::new();
    };
    if broken == "uninstall" {
        return one(c.script.replace("uninstall", "remove"));
    }
    let help = run_stdout(c.part(0), &["--help"]).unwrap_or_default();
    let mut listing = false;
    let mut operations = Vec::new();
    for line in help.lines().map(str::trim) {
        if listing {
            if line.is_empty() && c.part(0) != "apt" {
                break;
            }
            if let Some(operation) = line.split_whitespace().next() {
                operations.push(operation);
            }
        } else if line.starts_with("Basic commands:")
            || line.starts_with("Most used commands:")
            || line.starts_with("Commands:")
        {
            listing = true;
        }
    }
    replace_command(c, broken, &operations)
}

fn brew_insert(c: &Command, subcommand: &str, flags: &[&str]) -> Vec<String> {
    let mut parts = c.script_parts().to_vec();
    let Some(part) = parts.get_mut(1) else {
        return Vec::new();
    };
    *part = subcommand.into();
    parts.splice(2..2, flags.iter().map(|s| (*s).to_owned()));
    one(parts.join(" "))
}

fn brew_commands() -> Vec<String> {
    let fallback = || {
        [
            "info",
            "home",
            "options",
            "install",
            "uninstall",
            "search",
            "list",
            "update",
            "upgrade",
            "pin",
            "unpin",
            "doctor",
            "create",
            "edit",
            "cask",
        ]
        .map(str::to_owned)
        .to_vec()
    };
    let Some(prefix) = brew::get_brew_path_prefix() else {
        return fallback();
    };
    let root = Path::new(prefix).join("Homebrew/Library");
    let Ok(entries) = fs::read_dir(root.join("Homebrew/cmd")) else {
        return fallback();
    };
    let mut commands: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".rb")
                .or_else(|| name.strip_suffix(".sh"))
                .map(str::to_owned)
        })
        .collect();
    if let Ok(users) = fs::read_dir(root.join("Taps")) {
        for user in users.flatten().filter(|e| e.path().is_dir()) {
            let Ok(taps) = fs::read_dir(user.path()) else {
                continue;
            };
            for tap in taps.flatten().filter(|e| {
                e.path().is_dir() && e.file_name().to_string_lossy().starts_with("homebrew-")
            }) {
                let Ok(entries) = fs::read_dir(tap.path().join("cmd")) else {
                    continue;
                };
                commands.extend(entries.flatten().filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    name.strip_prefix("brew-")
                        .and_then(|n| n.strip_suffix(".rb"))
                        .map(str::to_owned)
                }));
            }
        }
    }
    commands
}

fn npm_wrong<'c>(c: &'c Command) -> Option<&'c str> {
    c.script_parts()
        .iter()
        .skip(1)
        .find(|p| !p.starts_with('-'))
        .map(String::as_str)
}
fn npm_wrong_fix(c: &Command) -> Vec<String> {
    let Some(broken) = npm_wrong(c) else {
        return Vec::new();
    };
    let options: Vec<_> = c
        .output()
        .split('\n')
        .skip_while(|line| !line.starts_with("where <command> is one of:"))
        .skip(1)
        .take_while(|line| !line.is_empty())
        .flat_map(|line| line.split(", "))
        .map(str::trim)
        .filter(|cmd| !cmd.is_empty())
        .collect();
    get_closest(broken, &options).map_or_else(Vec::new, |fix| {
        one(replace_argument(&c.script, broken, &fix))
    })
}

fn yarn_unknown_fix(c: &Command) -> Vec<String> {
    let Some(broken) = yarn_unknown(c) else {
        return Vec::new();
    };
    if broken == "require" {
        return one(replace_argument(&c.script, broken, "add"));
    }
    let deps: Vec<_> = utils::which("yarn").into_iter().collect();
    let options = utils::cached("yarn-commands", &deps, || {
        run_stdout("yarn", &["--help"])
            .unwrap_or_default()
            .lines()
            .skip_while(|line| !line.contains("Commands:"))
            .skip(1)
            .filter(|line| line.contains("- "))
            .filter_map(|line| line.trim().split(' ').next_back())
            .map(str::to_owned)
            .collect()
    });
    replace_command(c, broken, &options)
}

fn yum_operation_fix(c: &Command) -> Vec<String> {
    if c.part(1) == "uninstall" {
        return one(c.script.replace("uninstall", "remove"));
    }
    let deps: Vec<_> = utils::which("yum").into_iter().collect();
    let operations = utils::cached("yum-commands", &deps, || {
        run_stdout("yum", &[])
            .unwrap_or_default()
            .lines()
            .skip_while(|line| !line.starts_with("List of Commands:"))
            .skip(2)
            .take_while(|line| !line.trim().is_empty())
            .filter_map(|line| line.trim().split(' ').next())
            .map(str::to_owned)
            .collect()
    });
    replace_command(c, c.part(1), &operations)
}
