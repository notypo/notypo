use super::{one, output_contains_lower};
use crate::types::{Command, Rule};
use crate::utils::{self, capture, get_close_matches};
use std::path::{Path, PathBuf};
use std::{env, fs};

pub(super) const RULES: &[Rule] = &[
    sudo_rule!("cd_correction", cd_missing, cd_correction),
    Rule::new(
        "cd_cs",
        |c| c.part(0) == "cs",
        |c| one(c.script.replacen("cs", "cd", 1)),
    )
    .priority(900),
    sudo_rule!("cd_mkdir", cd_missing, cd_mkdir),
    Rule::new("cd_parent", |c| c.script == "cd..", |_| one("cd ..")),
    Rule::new(
        "chmod_x",
        |c| {
            c.script.starts_with("./")
                && c.output_lower().contains("permission denied")
                && Path::new(c.part(0)).exists()
                && !utils::has_execute_permission(Path::new(c.part(0)))
        },
        |c| {
            one(c.shell().and_(&[
                &format!("chmod +x {}", c.part(0).trim_start_matches("./")),
                &c.script,
            ]))
        },
    ),
    Rule::new(
        "cat_dir",
        |c| {
            c.is_app_at_least(&["cat"], 1)
                && c.output().starts_with("cat: ")
                && Path::new(c.part(1)).is_dir()
        },
        |c| one(c.script.replacen("cat", "ls", 1)),
    ),
    Rule::new(
        "cp_create_destination",
        |c| {
            c.is_app(&["cp", "mv"])
                && (c.output().contains("No such file or directory")
                    || c.output().starts_with("cp: directory")
                        && c.output().trim_end().ends_with("does not exist"))
        },
        |c| {
            c.script_parts().last().map_or_else(Vec::new, |dest| {
                one(c.shell().and_(&[&format!("mkdir -p {dest}"), &c.script]))
            })
        },
    ),
    sudo_rule!(
        "cp_omitting_directory",
        |c| c.is_app(&["cp"])
            && output_contains_lower(c, &["omitting directory", "is a directory"]),
        |c| one(regex!(r"^cp").replace(&c.script, "cp -a").into_owned())
    ),
    sudo_rule!(
        "ln_no_hard_link",
        |c| c.part(0) == "ln" && c.output().ends_with("hard link not allowed for directory"),
        |c| one(c.script.replacen("ln ", "ln -s ", 1))
    ),
    sudo_rule!(
        "ln_s_order",
        |c| c.part(0) == "ln"
            && (c.has_part("-s") || c.has_part("--symbolic"))
            && c.output().contains("File exists")
            && ln_destination(c).is_some(),
        |c| move_to_end(c, ln_destination(c))
    ),
    sudo_rule!(
        "mkdir_p",
        |c| c.script.contains("mkdir") && c.output().contains("No such file or directory"),
        |c| one(regex!(r"\bmkdir (.*)")
            .replace_all(&c.script, "mkdir -p $1")
            .into_owned())
    ),
    sudo_rule!(
        "rm_dir",
        |c| c.script.contains("rm") && c.output_lower().contains("is a directory"),
        |c| one(regex!(r"\brm (.*)")
            .replace_all(
                &c.script,
                if c.script.contains("hdfs") {
                    "rm -r $1"
                } else {
                    "rm -rf $1"
                }
            )
            .into_owned())
    ),
    sudo_rule!(
        "rm_root",
        |c| c.has_part("rm")
            && c.has_part("/")
            && !c.script.contains("--no-preserve-root")
            && c.output().contains("--no-preserve-root"),
        |c| one(format!("{} --no-preserve-root", c.script))
    )
    .disabled_by_default(),
    Rule::new(
        "touch",
        |c| c.is_app(&["touch"]) && touch_directory(c).is_some(),
        |c| {
            touch_directory(c).map_or_else(Vec::new, |dir| {
                one(c.shell().and_(&[&format!("mkdir -p {dir}"), &c.script]))
            })
        },
    ),
    Rule::new(
        "no_such_file",
        |c| missing_file(c).is_some(),
        |c| {
            missing_file(c).map_or_else(Vec::new, |file| {
                let dir = file.rsplit_once('/').map_or("", |(dir, _)| dir);
                one(c.shell().and_(&[&format!("mkdir -p {dir}"), &c.script]))
            })
        },
    ),
    Rule::new(
        "dirty_untar",
        |c| {
            c.is_app(&["tar"])
                && !c.script.contains("-C")
                && (c.script.contains("--extract") || c.part(1).contains('x'))
                && tar_file(c).is_some()
        },
        |c| {
            tar_file(c).map_or_else(Vec::new, |(_, dir)| {
                let dir = c.shell().quote(dir);
                one(c.shell().and_(&[
                    &format!("mkdir -p {dir}"),
                    &format!("{} -C {dir}", c.script),
                ]))
            })
        },
    )
    .side_effect(clean_tar),
    Rule::new(
        "dirty_unzip",
        |c| {
            c.is_app(&["unzip"])
                && !c.script.contains("-d")
                && zip_file(c)
                    .is_some_and(|file| archive_members("unzip", &["-Z1", &file]).len() > 1)
        },
        |c| {
            zip_file(c).map_or_else(Vec::new, |file| {
                one(format!(
                    "{} -d {}",
                    c.script,
                    c.shell().quote(file.strip_suffix(".zip").unwrap_or(&file))
                ))
            })
        },
    )
    .requires_output(false)
    .side_effect(clean_zip),
    Rule::new(
        "open",
        |c| {
            c.is_app(&["open", "xdg-open", "gnome-open", "kde-open"])
                && (is_url(c) || missing_open_file(c))
        },
        open_fix,
    ),
    Rule::new(
        "grep_arguments_order",
        |c| {
            c.is_app(&["grep", "egrep"])
                && c.output().contains(": No such file or directory")
                && actual_file(c).is_some()
        },
        |c| move_to_end(c, actual_file(c)),
    ),
    Rule::new(
        "grep_recursive",
        |c| c.is_app(&["grep"]) && c.output_lower().contains("is a directory"),
        |c| one(format!("grep -r {}", c.script.get(5..).unwrap_or_default())),
    ),
    Rule::new(
        "ls_all",
        |c| c.is_app(&["ls"]) && c.output().trim().is_empty(),
        |c| {
            one(std::iter::once("ls")
                .chain(std::iter::once("-A"))
                .chain(c.script_parts()[1..].iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "))
        },
    ),
    Rule::new(
        "ls_lah",
        |c| c.is_app(&["ls"]) && !c.script.contains("ls -"),
        |c| {
            one(std::iter::once("ls -lah")
                .chain(c.script_parts()[1..].iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "))
        },
    ),
    Rule::new("man", |c| c.is_app_at_least(&["man"], 1), man_fix),
    Rule::new(
        "man_no_space",
        |c| c.script.starts_with("man") && c.output_lower().contains("command not found"),
        |c| one(format!("man {}", &c.script[3..])),
    )
    .priority(2000),
    Rule::new(
        "sed_unterminated_s",
        |c| c.is_app(&["sed"]) && c.output().contains("unterminated `s' command"),
        |c| {
            let mut parts = c.script_parts().to_vec();
            for part in &mut parts {
                if (part.starts_with("s/") || part.starts_with("-es/")) && !part.ends_with('/') {
                    part.push('/');
                }
            }
            one(parts
                .iter()
                .map(|part| c.shell().quote(part))
                .collect::<Vec<_>>()
                .join(" "))
        },
    ),
];

fn cd_missing(c: &Command) -> bool {
    c.is_app_at_least(&["cd"], 1)
        && c.script.starts_with("cd ")
        && output_contains_lower(
            c,
            &[
                "no such file or directory",
                "cd: can't cd to",
                "does not exist",
            ],
        )
}

fn cd_mkdir(c: &Command) -> Vec<String> {
    let Some(dest) = c.script.strip_prefix("cd ") else {
        return Vec::new();
    };
    one(c
        .shell()
        .and_(&[&format!("mkdir -p {dest}"), &format!("cd {dest}")]))
}

fn cd_correction(c: &Command) -> Vec<String> {
    let dest = c.part(1).strip_suffix('/').unwrap_or(c.part(1));
    let mut cwd = if dest.starts_with('/') {
        PathBuf::from("/")
    } else {
        env::current_dir().unwrap_or_default()
    };
    for dir in dest.split('/').filter(|d| !d.is_empty()) {
        match dir {
            "." => continue,
            ".." => {
                cwd.pop();
            }
            dir => {
                let Ok(entries) = fs::read_dir(&cwd) else {
                    return cd_mkdir(c);
                };
                let dirs: Vec<_> = entries
                    .flatten()
                    .filter(|entry| entry.path().is_dir())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect();
                let Some(best) = get_close_matches(dir, &dirs, 3, 0.6).into_iter().next() else {
                    return cd_mkdir(c);
                };
                cwd.push(best);
            }
        }
    }
    one(format!("cd \"{}\"", cwd.display()))
}

fn ln_destination<'c>(c: &'c Command) -> Option<&'c str> {
    c.script_parts()
        .iter()
        .find(|part| {
            !["ln", "-s", "--symbolic"].contains(&part.as_str()) && Path::new(part).exists()
        })
        .map(String::as_str)
}

fn actual_file<'c>(c: &'c Command) -> Option<&'c str> {
    c.script_parts()
        .iter()
        .skip(1)
        .find(|part| Path::new(part).exists())
        .map(String::as_str)
}

fn move_to_end(c: &Command, part: Option<&str>) -> Vec<String> {
    let Some(part) = part else { return Vec::new() };
    let mut parts = c.script_parts().to_vec();
    if let Some(index) = parts.iter().position(|p| p == part) {
        let part = parts.remove(index);
        parts.push(part);
    }
    one(parts.join(" "))
}

fn touch_directory<'a>(c: &Command<'a>) -> Option<&'a str> {
    c.output()
        .contains("No such file or directory")
        .then(|| capture(regex!(r"touch: (?:cannot touch ')?(.+)/.+'?:"), c.output()))
        .flatten()
}

fn missing_file<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(
        regex!(
            r"(?:mv: cannot move '[^']*' to|cp: cannot create regular file) '([^']*)': (?:No such file or directory|Not a directory)"
        ),
        c.output(),
    )
}

fn tar_file<'c>(c: &'c Command) -> Option<(&'c str, &'c str)> {
    const EXTENSIONS: &[&str] = &[
        ".tar",
        ".tar.Z",
        ".tar.bz2",
        ".tar.gz",
        ".tar.lz",
        ".tar.lzma",
        ".tar.xz",
        ".taz",
        ".tb2",
        ".tbz",
        ".tbz2",
        ".tgz",
        ".tlz",
        ".txz",
        ".tz",
    ];
    c.script_parts().iter().find_map(|file| {
        EXTENSIONS
            .iter()
            .find_map(|ext| file.strip_suffix(ext).map(|dir| (file.as_str(), dir)))
    })
}

fn zip_file(c: &Command) -> Option<String> {
    c.script_parts()
        .iter()
        .skip(1)
        .find(|p| !p.starts_with('-'))
        .map(|p| {
            if p.ends_with(".zip") {
                p.clone()
            } else {
                format!("{p}.zip")
            }
        })
}

fn archive_members(program: &str, args: &[&str]) -> Vec<String> {
    utils::run_stdout(program, args)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn clean_archive(members: Vec<String>) {
    let Ok(cwd) = env::current_dir().and_then(fs::canonicalize) else {
        return;
    };
    for member in members {
        let file = cwd.join(member);
        if file
            .parent()
            .and_then(|p| fs::canonicalize(p).ok())
            .is_some_and(|p| p.starts_with(&cwd))
        {
            let _ = fs::remove_file(file);
        }
    }
}

fn clean_tar(c: &Command, _: &str) {
    if let Some((file, _)) = tar_file(c) {
        clean_archive(archive_members("tar", &["-tf", file]));
    }
}
fn clean_zip(c: &Command, _: &str) {
    if let Some(file) = zip_file(c) {
        clean_archive(archive_members("unzip", &["-Z1", &file]));
    }
}

fn is_url(c: &Command) -> bool {
    [
        ".com", ".edu", ".info", ".io", ".ly", ".me", ".net", ".org", ".se", "www.",
    ]
    .iter()
    .any(|suffix| c.script.contains(suffix))
}
fn missing_open_file(c: &Command) -> bool {
    c.output().trim().starts_with("The file ") && c.output().trim().ends_with(" does not exist.")
}
fn open_fix(c: &Command) -> Vec<String> {
    if is_url(c) {
        return one(c.script.replace("open ", "open http://"));
    }
    c.script.split_once(' ').map_or_else(Vec::new, |(_, arg)| {
        ["touch", "mkdir"]
            .into_iter()
            .map(|program| c.shell().and_(&[&format!("{program} {arg}"), &c.script]))
            .collect()
    })
}

fn man_fix(c: &Command) -> Vec<String> {
    if c.script.contains('3') {
        return one(c.script.replace('3', "2"));
    }
    if c.script.contains('2') {
        return one(c.script.replace('2', "3"));
    }
    let Some(arg) = c.script_parts().last() else {
        return Vec::new();
    };
    let help = format!("{arg} --help");
    if c.output().trim() == format!("No manual entry for {arg}") {
        return one(help);
    }
    vec![
        format!("{} 3 {}", c.part(0), c.script_parts()[1..].join("")),
        format!("{} 2 {}", c.part(0), c.script_parts()[1..].join("")),
        help,
    ]
}
