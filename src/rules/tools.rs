use super::{one, output_contains, output_contains_lower};
use crate::types::{Command, Rule};
use crate::utils::{
    self, capture, capture_all, get_all_matched_commands, get_close_matches, get_closest,
    replace_argument, replace_command, run_output, run_stdout,
};
use std::path::{Path, PathBuf};

pub(super) const RULES: &[Rule] = &[
    Rule::new(
        "adb_unknown_command",
        |c| c.is_app(&["adb"]) && c.output().starts_with("Android Debug Bridge version"),
        adb_fix,
    ),
    Rule::new(
        "ag_literal",
        |c| c.is_app(&["ag"]) && c.output().ends_with("run ag with -Q\n"),
        |c| one(c.script.replacen("ag", "ag -Q", 1)),
    ),
    Rule::new(
        "aws_cli",
        |c| {
            c.is_app(&["aws"])
                && c.output().contains("usage:")
                && c.output().contains("maybe you meant:")
        },
        |c| {
            capture(
                regex!(r"Invalid choice: '(.*)', maybe you meant:"),
                c.output(),
            )
            .map_or_else(Vec::new, |broken| {
                capture_all(regex!(r"(?m)^\s*\*\s(.*)"), c.output())
                    .into_iter()
                    .map(|fix| replace_argument(&c.script, broken, fix))
                    .collect()
            })
        },
    ),
    Rule::new(
        "az_cli",
        |c| {
            c.is_app(&["az"])
                && c.output().contains("is not in the")
                && c.output().contains("command group")
        },
        |c| {
            capture(
                regex!(r"az(?:.*): '(.*)' is not in the '.*' command group."),
                c.output(),
            )
            .map_or_else(Vec::new, |broken| {
                capture_all(
                    regex!(r"(?m)^The most similar choice to '.*' is:\n\s*(.*)$"),
                    c.output(),
                )
                .into_iter()
                .map(|fix| replace_argument(&c.script, broken, fix))
                .collect()
            })
        },
    ),
    Rule::new(
        "composer_not_command",
        |c| {
            c.is_app(&["composer"])
                && (output_contains_lower(c, &["did you mean this?", "did you mean one of these?"])
                    || c.has_part("install") && c.output_lower().contains("composer require"))
        },
        composer_fix,
    ),
    Rule::new(
        "conda_mistype",
        |c| c.is_app(&["conda"]) && c.output().contains("Did you mean 'conda"),
        |c| {
            let commands = capture_all(regex!(r"'conda ([^']*)'"), c.output());
            match commands.as_slice() {
                [broken, fix, ..] => replace_command(c, broken, &[*fix]),
                _ => Vec::new(),
            }
        },
    ),
    Rule::new(
        "cpp11",
        |c| {
            c.is_app(&["g++", "clang++"])
                && output_contains(
                    c,
                    &[
                        "This file requires compiler and library support for the ISO C++ 2011 standard.",
                        "-Wc++11-extensions",
                    ],
                )
        },
        |c| one(format!("{} -std=c++11", c.script)),
    ),
    Rule::new(
        "django_south_ghost",
        |c| {
            c.script.contains("manage.py")
                && c.script.contains("migrate")
                && c.output().contains("or pass --delete-ghost-migrations")
        },
        |c| one(format!("{} --delete-ghost-migrations", c.script)),
    ),
    Rule::new(
        "django_south_merge",
        |c| {
            c.script.contains("manage.py")
                && c.script.contains("migrate")
                && c.output()
                    .contains("--merge: will just attempt the migration")
        },
        |c| one(format!("{} --merge", c.script)),
    ),
    Rule::new(
        "docker_image_being_used_by_container",
        |c| {
            c.is_app(&["docker"])
                && c.output()
                    .contains("image is being used by running container")
        },
        |c| {
            c.output()
                .split_whitespace()
                .next_back()
                .map_or_else(Vec::new, |id| {
                    one(c
                        .shell()
                        .and_(&[&format!("docker container rm -f {id}"), &c.script]))
                })
        },
    ),
    Rule::new(
        "docker_login",
        |c| {
            c.is_app(&["docker"])
                && c.output().contains("access denied")
                && c.output().contains("may require 'docker login'")
        },
        |c| one(c.shell().and_(&["docker login", &c.script])),
    ),
    sudo_rule!(
        "docker_not_command",
        |c| c.is_app(&["docker"])
            && output_contains(c, &["is not a docker command", "Usage:\tdocker"]),
        docker_fix
    ),
    Rule::new(
        "fab_command_not_found",
        |c| c.is_app(&["fab"]) && c.output().contains("Warning: Command(s) not found:"),
        fab_fix,
    ),
    Rule::new(
        "gem_unknown_command",
        |c| {
            c.is_app(&["gem"])
                && c.output()
                    .contains("ERROR:  While executing gem ... (Gem::CommandLineError)")
                && c.output().contains("Unknown command")
        },
        |c| {
            capture(regex!(r"Unknown command (.*)$"), c.output()).map_or_else(Vec::new, |broken| {
                let commands = program_commands("gem", &[], || {
                    run_stdout("gem", &["help", "commands"])
                        .unwrap_or_default()
                        .lines()
                        .filter(|line| line.starts_with("    "))
                        .filter_map(|line| line.trim().split(' ').next())
                        .map(str::to_owned)
                        .collect()
                });
                replace_command(c, broken, &commands)
            })
        },
    ),
    Rule::new(
        "go_run",
        |c| c.is_app(&["go"]) && c.script.starts_with("go run ") && !c.script.ends_with(".go"),
        |c| one(format!("{}.go", c.script)),
    ),
    Rule::new(
        "go_unknown_command",
        |c| c.is_app_at_least(&["go"], 1) && c.output().contains("unknown command"),
        |c| {
            let commands = program_commands("go", &[], || {
                let (_, help) = run_output("go", &[]).unwrap_or_default();
                help.lines()
                    .map(str::trim)
                    .skip_while(|line| *line != "The commands are:")
                    .skip(2)
                    .take_while(|line| !line.is_empty())
                    .filter_map(|line| line.split(' ').next())
                    .map(str::to_owned)
                    .collect()
            });
            get_closest(c.part(1), &commands).map_or_else(Vec::new, |fix| {
                one(replace_argument(&c.script, c.part(1), &fix))
            })
        },
    ),
    Rule::new(
        "gradle_no_task",
        |c| c.is_app(&["gradle", "gradlew"]) && gradle_task(c).is_some(),
        gradle_fix,
    ),
    Rule::new(
        "gradle_wrapper",
        |c| {
            c.is_app(&["gradle"])
                && c.ctx().which(c.part(0)).is_none()
                && c.output().contains("not found")
                && Path::new("gradlew").is_file()
        },
        |c| one(format!("./gradlew {}", c.script_parts()[1..].join(" "))),
    ),
    Rule::new(
        "grunt_task_not_found",
        |c| c.is_app(&["grunt"]) && grunt_task(c).is_some(),
        grunt_fix,
    ),
    Rule::new(
        "gulp_not_task",
        |c| c.is_app(&["gulp"]) && c.output().contains("is not in your gulpfile"),
        |c| {
            capture(regex!(r"Task '(\w+)' is not in your gulpfile"), c.output()).map_or_else(
                Vec::new,
                |broken| {
                    let commands =
                        utils::cached("gulp-tasks", &[PathBuf::from("gulpfile.js")], || {
                            run_stdout("gulp", &["--tasks-simple"])
                                .unwrap_or_default()
                                .lines()
                                .map(str::to_owned)
                                .collect()
                        });
                    replace_command(c, broken, &commands)
                },
            )
        },
    ),
    sudo_rule!(
        "has_exists_script",
        |c| !c.part(0).is_empty()
            && Path::new(c.part(0)).exists()
            && c.output().contains("command not found"),
        |c| one(format!("./{}", c.script))
    ),
    Rule::new(
        "heroku_multiple_apps",
        |c| {
            c.is_app(&["heroku"])
                && c.output()
                    .contains("https://devcenter.heroku.com/articles/multiple-environments")
        },
        |c| {
            capture_all(regex!(r"([^ ]*) \([^)]*\)"), c.output())
                .into_iter()
                .map(|app| format!("{} --app {app}", c.script))
                .collect()
        },
    ),
    Rule::new(
        "heroku_not_command",
        |c| c.is_app(&["heroku"]) && c.output().contains("Run heroku _ to run"),
        |c| capture(regex!(r"Run heroku _ to run ([^.]*)"), c.output()).map_or_else(Vec::new, one),
    ),
    sudo_rule!(
        "hostscli",
        |c| c.is_app(&["hostscli"])
            && output_contains(
                c,
                &[
                    "Error: No such command",
                    "hostscli.errors.WebsiteImportError"
                ]
            ),
        |c| {
            if c.output().contains("hostscli.errors.WebsiteImportError") {
                return one("hostscli websites");
            }
            capture(regex!(r#"Error: No such command "([^"]*)""#), c.output()).map_or_else(
                Vec::new,
                |broken| {
                    replace_command(
                        c,
                        broken,
                        &["block", "unblock", "websites", "block_all", "unblock_all"],
                    )
                },
            )
        }
    ),
    Rule::new(
        "java",
        |c| c.is_app(&["java"]) && c.script.ends_with(".java"),
        |c| one(c.script.strip_suffix(".java").unwrap_or(&c.script)),
    ),
    Rule::new(
        "javac",
        |c| c.is_app(&["javac"]) && !c.script.ends_with(".java"),
        |c| one(format!("{}.java", c.script)),
    ),
    sudo_rule!(
        "lein_not_task",
        |c| c.is_app(&["lein"])
            && c.output().contains("is not a task. See 'lein help'")
            && c.output().contains("Did you mean this?"),
        |c| {
            capture(regex!(r"'([^']*)' is not a task"), c.output()).map_or_else(
                Vec::new,
                |broken| {
                    replace_command(
                        c,
                        broken,
                        &get_all_matched_commands(c.output(), &["Did you mean this?"]),
                    )
                },
            )
        }
    ),
    Rule::new(
        "mercurial",
        |c| {
            c.is_app_at_least(&["hg"], 1)
                && (c.output().contains("hg: unknown command")
                    && c.output().contains("(did you mean one of ")
                    || c.output().contains("hg: command '")
                        && c.output().contains("' is ambiguous:"))
        },
        mercurial_fix,
    ),
    Rule::new(
        "mvn_no_command",
        |c| {
            c.is_app(&["mvn"])
                && c.output()
                    .contains("No goals have been specified for this build")
        },
        |c| {
            ["clean package", "clean install"]
                .into_iter()
                .map(|goals| format!("{} {goals}", c.script))
                .collect()
        },
    ),
    Rule::new(
        "mvn_unknown_lifecycle_phase",
        |c| c.is_app(&["mvn"]) && mvn_lifecycle(c).is_some() && mvn_phases(c).is_some(),
        |c| {
            mvn_lifecycle(c)
                .zip(mvn_phases(c))
                .map_or_else(Vec::new, |(broken, options)| {
                    let options =
                        get_close_matches(broken, &options.split(", ").collect::<Vec<_>>(), 3, 0.6);
                    replace_command(c, broken, &options)
                })
        },
    ),
    Rule::new(
        "omnienv_no_such_command",
        |c| c.is_app_at_least(ENV_APPS, 1) && c.output().contains("env: no such command "),
        omnienv_fix,
    )
    .enabled_by_default(|| ENV_APPS.iter().any(|app| utils::which(app).is_some())),
    Rule::new(
        "php_s",
        |c| {
            c.is_app_at_least(&["php"], 2)
                && c.has_part("-s")
                && c.script_parts().last().is_some_and(|part| part != "-s")
        },
        |c| one(replace_argument(&c.script, "-s", "-S")),
    ),
    Rule::new(
        "prove_recursively",
        |c| {
            c.is_app(&["prove"])
                && c.output().contains("NOTESTS")
                && !c.script_parts().iter().skip(1).any(|p| {
                    p == "--recurse"
                        || p.starts_with('-') && !p.starts_with("--") && p.contains('r')
                })
                && c.script_parts()
                    .iter()
                    .skip(1)
                    .any(|p| !p.starts_with('-') && Path::new(p).is_dir())
        },
        |c| {
            let mut parts = c.script_parts().to_vec();
            parts.insert(1, "-r".into());
            one(parts.join(" "))
        },
    ),
    Rule::new(
        "rails_migrations_pending",
        |c| {
            c.output()
                .contains("Migrations are pending. To resolve this issue, run:")
        },
        |c| {
            capture(regex!(r"To resolve this issue, run:\s+(.*?)\n"), c.output())
                .map_or_else(Vec::new, |script| one(c.shell().and_(&[script, &c.script])))
        },
    ),
    Rule::new(
        "react_native_command_unrecognized",
        |c| c.is_app(&["react-native"]) && react_command(c).is_some(),
        |c| {
            react_command(c).map_or_else(Vec::new, |broken| {
                let commands = utils::cached(
                    "react-native-commands",
                    &[PathBuf::from("package.json")],
                    || {
                        run_stdout("react-native", &["--help"])
                            .unwrap_or_default()
                            .lines()
                            .map(str::trim)
                            .skip_while(|line| !line.contains("Commands:"))
                            .skip(1)
                            .filter_map(|line| line.split_whitespace().next())
                            .map(str::to_owned)
                            .collect()
                    },
                );
                replace_command(c, broken, &commands)
            })
        },
    ),
    sudo_rule!(
        "systemctl",
        |c| c.is_app(&["systemctl"])
            && c.output().contains("Unknown operation '")
            && c.script_parts().len() == 3,
        |c| {
            let mut parts = c.script_parts().to_vec();
            if parts.len() >= 2 {
                let len = parts.len();
                parts.swap(len - 1, len - 2);
            }
            one(parts.join(" "))
        }
    ),
    Rule::new(
        "terraform_init",
        |c| {
            c.is_app(&["terraform"])
                && output_contains_lower(
                    c,
                    &[
                        "this module is not yet installed",
                        "initialization required",
                    ],
                )
        },
        |c| one(c.shell().and_(&["terraform init", &c.script])),
    ),
    Rule::new(
        "terraform_no_command",
        |c| {
            c.is_app(&["terraform"])
                && terraform_mistake(c).is_some()
                && terraform_suggestion(c).is_some()
        },
        |c| {
            terraform_mistake(c)
                .zip(terraform_suggestion(c))
                .map_or_else(Vec::new, |(from, to)| one(c.script.replace(from, to)))
        },
    ),
    Rule::new(
        "tmux",
        |c| {
            c.is_app(&["tmux"])
                && c.output().contains("ambiguous command:")
                && c.output().contains("could be:")
        },
        |c| {
            regex!(r"^ambiguous command: (.*), could be: (.*)")
                .captures(c.output())
                .map_or_else(Vec::new, |caps| {
                    replace_command(
                        c,
                        &caps[1],
                        &caps[2].split(',').map(str::trim).collect::<Vec<_>>(),
                    )
                })
        },
    ),
    Rule::new(
        "tsuru_login",
        |c| {
            c.is_app(&["tsuru"])
                && c.output().contains("not authenticated")
                && c.output().contains("session has expired")
        },
        |c| one(c.shell().and_(&["tsuru login", &c.script])),
    ),
    Rule::new(
        "tsuru_not_command",
        |c| {
            c.is_app(&["tsuru"])
                && c.output()
                    .contains(" is not a tsuru command. See \"tsuru help\".")
                && c.output().contains("\nDid you mean?\n\t")
        },
        |c| {
            capture(
                regex!(r#"tsuru: "([^"]*)" is not a tsuru command"#),
                c.output(),
            )
            .map_or_else(Vec::new, |broken| {
                replace_command(
                    c,
                    broken,
                    &get_all_matched_commands(c.output(), &["Did you mean"]),
                )
            })
        },
    ),
    Rule::new(
        "vagrant_up",
        |c| c.is_app(&["vagrant"]) && c.output_lower().contains("run `vagrant up`"),
        |c| {
            let all = c.shell().and_(&["vagrant up", &c.script]);
            if c.part(2).is_empty() {
                one(all)
            } else {
                vec![
                    c.shell()
                        .and_(&[&format!("vagrant up {}", c.part(2)), &c.script]),
                    all,
                ]
            }
        },
    ),
    Rule::new("whois", |c| c.is_app_at_least(&["whois"], 1), whois_fix),
    Rule::new(
        "workon_doesnt_exists",
        |c| c.is_app_at_least(&["workon"], 1) && !virtualenvs().iter().any(|env| env == c.part(1)),
        |c| {
            let mut fixes = replace_command(c, c.part(1), &virtualenvs());
            fixes.push(format!("mkvirtualenv {}", c.part(1)));
            fixes
        },
    ),
];

const ENV_APPS: &[&str] = &["goenv", "nodenv", "pyenv", "rbenv"];

fn program_commands(
    program: &str,
    extra_deps: &[PathBuf],
    compute: impl FnOnce() -> Vec<String>,
) -> Vec<String> {
    let mut deps: Vec<_> = utils::which(program).into_iter().collect();
    deps.extend_from_slice(extra_deps);
    utils::cached(&format!("{program}-commands"), &deps, compute)
}

fn adb_fix(c: &Command) -> Vec<String> {
    const COMMANDS: &[&str] = &[
        "backup",
        "bugreport",
        "connect",
        "devices",
        "disable-verity",
        "disconnect",
        "enable-verity",
        "emu",
        "forward",
        "get-devpath",
        "get-serialno",
        "get-state",
        "install",
        "install-multiple",
        "jdwp",
        "keygen",
        "kill-server",
        "logcat",
        "pull",
        "push",
        "reboot",
        "reconnect",
        "restore",
        "reverse",
        "root",
        "run-as",
        "shell",
        "sideload",
        "start-server",
        "sync",
        "tcpip",
        "uninstall",
        "unroot",
        "usb",
        "wait-for",
    ];
    c.script_parts()
        .iter()
        .enumerate()
        .skip(1)
        .find(|(index, arg)| {
            !arg.starts_with('-') && !["-s", "-H", "-P", "-L"].contains(&c.part(index - 1))
        })
        .and_then(|(_, broken)| {
            get_closest(broken, COMMANDS).map(|fix| replace_argument(&c.script, broken, &fix))
        })
        .into_iter()
        .collect()
}

fn composer_fix(c: &Command) -> Vec<String> {
    if c.has_part("install") && c.output_lower().contains("composer require") {
        return one(replace_argument(&c.script, "install", "require"));
    }
    let broken = capture(regex!(r#"Command "([^"]*)" is not defined"#), c.output());
    let fix = capture(
        regex!(r"Did you mean (?:this|one of these)\?[^\n]*\n\s*([^\n]*)"),
        c.output(),
    );
    broken.zip(fix).map_or_else(Vec::new, |(from, to)| {
        one(replace_argument(&c.script, from, to.trim()))
    })
}

fn listed_commands<'a>(text: &'a str, heading: &str) -> Vec<&'a str> {
    text.lines()
        .skip_while(|line| !line.starts_with(heading))
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| line.trim().split(' ').next())
        .collect()
}

fn docker_fix(c: &Command) -> Vec<String> {
    if c.output().contains("Usage:") && c.script_parts().len() > 2 {
        return replace_command(c, c.part(2), &listed_commands(c.output(), "Commands:"));
    }
    let Some(broken) = capture(
        regex!(r"docker: '(\w+)' is not a docker command."),
        c.output(),
    ) else {
        return Vec::new();
    };
    let commands = program_commands("docker", &[], || {
        let (stdout, stderr) = run_output("docker", &[]).unwrap_or_default();
        let help = if stdout.is_empty() { stderr } else { stdout };
        listed_commands(&help, "Management Commands:")
            .into_iter()
            .chain(listed_commands(&help, "Commands:"))
            .map(str::to_owned)
            .collect()
    });
    replace_command(c, broken, &commands)
}

fn between<'a>(text: &'a str, start: &str, end: Option<&str>) -> Vec<&'a str> {
    text.split('\n')
        .skip_while(|line| !line.contains(start))
        .skip(1)
        .take_while(|line| !end.is_some_and(|end| line.contains(end)))
        .filter(|line| !line.is_empty())
        .filter_map(|line| line.trim().split(' ').next())
        .collect()
}

fn fab_fix(c: &Command) -> Vec<String> {
    let missing = between(
        c.output(),
        "Warning: Command(s) not found:",
        Some("Available commands:"),
    );
    let available = between(c.output(), "Available commands:", None);
    let mut script = c.script.clone();
    for broken in missing {
        if let Some(fix) = get_closest(broken, &available) {
            script = script.replace(&format!(" {broken}"), &format!(" {fix}"));
        }
    }
    one(script)
}

fn gradle_task<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(
        regex!(r"Task '(.*)' (?:is ambiguous|not found)"),
        c.output(),
    )
}
fn grunt_task<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(regex!(r#"Warning: Task "(.*)" not found."#), c.output())
}
fn react_command<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(regex!(r"Unrecognized command '(.*)'"), c.output())
}
fn mvn_lifecycle<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(
        regex!(r#"\[ERROR\] Unknown lifecycle phase "(.+)""#),
        c.output(),
    )
}
fn mvn_phases<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(
        regex!(r"Available lifecycle phases are: (.+) -> \[Help 1\]"),
        c.output(),
    )
}
fn terraform_mistake<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(
        regex!(r#"Terraform has no command named "([^"]+)"\."#),
        c.output(),
    )
}
fn terraform_suggestion<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(regex!(r#"Did you mean "([^"]+)"\?"#), c.output())
}

fn gradle_fix(c: &Command) -> Vec<String> {
    let Some(broken) = gradle_task(c) else {
        return Vec::new();
    };
    let help = run_stdout(c.part(0), &["tasks"]).unwrap_or_default();
    let mut listing = false;
    let mut tasks = Vec::new();
    for line in help.lines().map(str::trim) {
        if line.starts_with("----") {
            listing = true;
        } else if line.is_empty() {
            listing = false;
        } else if listing
            && !line.starts_with("All tasks runnable from root project")
            && let Some(task) = line.split(' ').next()
        {
            tasks.push(task);
        }
    }
    replace_command(c, broken, &tasks)
}

fn grunt_fix(c: &Command) -> Vec<String> {
    let Some(broken) = grunt_task(c).and_then(|task| task.split(':').next()) else {
        return Vec::new();
    };
    let tasks = utils::cached("grunt-tasks", &[PathBuf::from("Gruntfile.js")], || {
        let help = run_stdout("grunt", &["--help"]).unwrap_or_default();
        let mut listing = false;
        let mut tasks = Vec::new();
        for line in help.lines().map(str::trim) {
            if line.contains("Available tasks") {
                listing = true;
                continue;
            }
            if listing && line.is_empty() {
                break;
            }
            if line.contains("  ")
                && let Some(task) = line.split(' ').next()
            {
                tasks.push(task.to_owned());
            }
        }
        tasks
    });
    get_closest(broken, &tasks).map_or_else(Vec::new, |fix| {
        one(c.script.replace(&format!(" {broken}"), &format!(" {fix}")))
    })
}

fn mercurial_fix(c: &Command) -> Vec<String> {
    let options: Vec<_> = if let Some(options) =
        capture(regex!(r"\n\(did you mean one of ([^?]+)\?\)"), c.output())
    {
        options.split(", ").collect()
    } else if let Some(options) = capture(regex!(r"\n    ([^$]+)$"), c.output()) {
        options.split(' ').collect()
    } else {
        Vec::new()
    };
    let Some(fix) = get_closest(c.part(1), &options) else {
        return Vec::new();
    };
    let mut parts = c.script_parts().to_vec();
    if let Some(part) = parts.get_mut(1) {
        *part = fix;
    }
    one(parts.join(" "))
}

fn omnienv_fix(c: &Command) -> Vec<String> {
    let Some(broken) = capture(regex!(r"env: no such command ['`]([^']*)'"), c.output()) else {
        return Vec::new();
    };
    let common: &[&str] = match broken {
        "list" => &["versions", "install --list"],
        "remove" => &["uninstall"],
        _ => &[],
    };
    let mut fixes: Vec<_> = common
        .iter()
        .map(|fix| replace_argument(&c.script, broken, fix))
        .collect();
    let commands = program_commands(c.part(0), &[], || {
        run_stdout(c.part(0), &["commands"])
            .unwrap_or_default()
            .lines()
            .map(|line| line.trim().to_owned())
            .collect()
    });
    fixes.extend(replace_command(c, broken, &commands));
    fixes
}

fn whois_fix(c: &Command) -> Vec<String> {
    let url = c.part(1);
    if c.script.contains('/') {
        let rest = url
            .split_once("://")
            .map(|(_, rest)| rest)
            .or_else(|| url.strip_prefix("//"))
            .unwrap_or_default();
        return one(format!(
            "whois {}",
            rest.split(['/', '?', '#']).next().unwrap_or_default()
        ));
    }
    if c.script.contains('.') {
        let parts: Vec<_> = url.split('.').collect();
        return (1..parts.len())
            .map(|index| format!("whois {}", parts[index..].join(".")))
            .collect();
    }
    Vec::new()
}

fn virtualenvs() -> Vec<String> {
    std::fs::read_dir(utils::expand_user("~/.virtualenvs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}
