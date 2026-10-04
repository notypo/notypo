use super::{one, output_contains};
use crate::types::{Command, Rule};
use crate::utils::{
    capture, capture_all, closest_match, get_all_matched_commands, get_close_matches,
    replace_argument, replace_command, run_stdout,
};
use std::path::Path;

pub(super) const RULES: &[Rule] = &[
    git_rule!(
        "git_add",
        |c| pathspec(c).is_some_and(|file| Path::new(file).exists()),
        |c| pathspec(c).map_or_else(Vec::new, |file| one(c
            .shell()
            .and_(&[&format!("git add -- {file}"), &c.script])))
    ),
    git_rule!(
        "git_add_force",
        |c| c.has_part("add")
            && c.output()
                .contains("Use -f if you really want to add them."),
        |c| replace(c, "add", "add --force")
    ),
    git_rule!(
        "git_bisect_usage",
        |c| c.has_part("bisect") && c.output().contains("usage: git bisect"),
        bisect_fix
    ),
    git_rule!(
        "git_branch_0flag",
        |c| c.part(1) == "branch" && zero_flag(c).is_some(),
        branch_zero_fix
    ),
    git_rule!(
        "git_branch_delete",
        |c| c.script.contains("branch -d")
            && c.output().contains("If you are sure you want to delete it"),
        |c| replace(c, "-d", "-D")
    ),
    git_rule!(
        "git_branch_delete_checked_out",
        |c| (c.script.contains("branch -d") || c.script.contains("branch -D"))
            && c.output().contains("error: Cannot delete branch '")
            && c.output().contains("' checked out at '"),
        |c| one(c.shell().and_(&[
            "git checkout master",
            &replace_argument(&c.script, "-d", "-D")
        ]))
    ),
    git_rule!(
        "git_branch_exists",
        |c| existing_branch(c).is_some(),
        branch_exists_fix
    ),
    git_rule!(
        "git_branch_list",
        |c| c
            .script_parts()
            .get(1..)
            .is_some_and(|p| p == ["branch", "list"]),
        |c| one(c.shell().and_(&["git branch --delete list", "git branch"]))
    ),
    git_rule!(
        "git_checkout",
        |c| c
            .output()
            .contains("did not match any file(s) known to git")
            && !c.output().contains("Did you forget to 'git add'?"),
        checkout_fix
    ),
    git_rule!(
        "git_clone_git_clone",
        |c| c.script.contains(" git clone ") && c.output().contains("fatal: Too many arguments."),
        |c| one(c.script.replacen(" git clone ", " ", 1))
    ),
    Rule::new("git_clone_missing", clone_missing, |c| {
        one(format!("git clone {}", c.script))
    }),
    git_rule!(
        "git_commit_add",
        |c| c.has_part("commit") && c.output().contains("no changes added to commit"),
        |c| ["commit -a", "commit -p"]
            .into_iter()
            .map(|fix| replace_argument(&c.script, "commit", fix))
            .collect()
    ),
    git_rule!("git_commit_amend", |c| c.has_part("commit"), |_| one(
        "git commit --amend"
    )),
    git_rule!("git_commit_reset", |c| c.has_part("commit"), |_| one(
        "git reset HEAD~"
    )),
    git_rule!(
        "git_diff_no_index",
        |c| c.script.contains("diff")
            && !c.script.contains("--no-index")
            && c.script_parts()
                .iter()
                .skip(2)
                .filter(|arg| !arg.starts_with('-'))
                .count()
                == 2,
        |c| replace(c, "diff", "diff --no-index")
    ),
    git_rule!(
        "git_diff_staged",
        |c| c.script.contains("diff") && !c.script.contains("--staged"),
        |c| replace(c, "diff", "diff --staged")
    ),
    git_rule!(
        "git_fix_stash",
        |c| c.part(1) == "stash" && c.output().contains("usage:"),
        stash_fix
    ),
    git_rule!(
        "git_flag_after_filename",
        |c| bad_flag(c).is_some(),
        flag_after_filename
    ),
    git_rule!(
        "git_help_aliased",
        |c| c.script.contains("help") && c.output().contains(" is aliased to "),
        |c| {
            c.output()
                .splitn(3, '`')
                .nth(2)
                .and_then(|s| s.split('\'').next())
                .and_then(|s| s.split(' ').next())
                .map_or_else(Vec::new, |alias| one(format!("git help {alias}")))
        }
    ),
    git_rule!("git_hook_bypass", |c| hooked_command(c).is_some(), |c| {
        hooked_command(c).map_or_else(Vec::new, |cmd| {
            replace(c, cmd, &format!("{cmd} --no-verify"))
        })
    })
    .priority(1100)
    .requires_output(false),
    git_rule!(
        "git_lfs_mistype",
        |c| c.script.contains("lfs") && c.output().contains("Did you mean this?"),
        |c| {
            capture(
                regex!(r#"Error: unknown command "([^"]*)" for "git-lfs""#),
                c.output(),
            )
            .map_or_else(Vec::new, |broken| {
                replace_command(
                    c,
                    broken,
                    &get_all_matched_commands(c.output(), &["Did you mean", " for usage."]),
                )
            })
        }
    ),
    git_rule!(
        "git_main_master",
        |c| output_contains(c, &["'master'", "'main'"]),
        |c| one(if c.output().contains("'master'") {
            c.script.replace("master", "main")
        } else {
            c.script.replace("main", "master")
        })
    )
    .priority(1200),
    git_rule!(
        "git_merge",
        |c| c.script.contains("merge")
            && c.output().contains(" - not something we can merge")
            && c.output().contains("Did you mean this?"),
        |c| {
            let unknown = capture(
                regex!(r"merge: (.+) - not something we can merge"),
                c.output(),
            );
            let remote = capture(regex!(r"Did you mean this\?\n\t([^\n]+)"), c.output());
            unknown
                .zip(remote)
                .map_or_else(Vec::new, |(from, to)| replace(c, from, to))
        }
    ),
    git_rule!(
        "git_merge_unrelated",
        |c| c.script.contains("merge")
            && c.output()
                .contains("fatal: refusing to merge unrelated histories"),
        |c| one(format!("{} --allow-unrelated-histories", c.script))
    ),
    git_rule!(
        "git_not_command",
        |c| c
            .output()
            .contains(" is not a git command. See 'git --help'.")
            && output_contains(c, &["The most similar command", "Did you mean"]),
        |c| {
            capture(regex!(r"git: '([^']*)' is not a git command"), c.output()).map_or_else(
                Vec::new,
                |broken| {
                    replace_command(
                        c,
                        broken,
                        &get_all_matched_commands(
                            c.output(),
                            &["The most similar command", "Did you mean"],
                        ),
                    )
                },
            )
        }
    ),
    git_rule!(
        "git_pull",
        |c| c.script.contains("pull") && c.output().contains("set-upstream"),
        pull_fix
    ),
    git_rule!(
        "git_pull_clone",
        |c| c.output().contains("fatal: Not a git repository")
            && c.output().contains(
                "Stopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set)."
            ),
        |c| replace(c, "pull", "clone")
    ),
    git_rule!(
        "git_pull_uncommitted_changes",
        |c| c.script.contains("pull")
            && output_contains(
                c,
                &["You have unstaged changes", "contains uncommitted changes"]
            ),
        |c| one(c.shell().and_(&["git stash", "git pull", "git stash pop"]))
    ),
    git_rule!(
        "git_push",
        |c| c.has_part("push") && c.output().contains("git push --set-upstream"),
        push_fix
    ),
    git_rule!(
        "git_push_different_branch_names",
        |c| c.script.contains("push")
            && c.output()
                .contains("The upstream branch of your current branch does not match"),
        |c| capture(regex!(r"(?m)^ +(git push [^\s]+ [^\s]+)"), c.output())
            .map_or_else(Vec::new, one)
    ),
    git_rule!(
        "git_push_force",
        |c| rejected_push(c)
            && c.output()
                .contains("Updates were rejected because the tip of your current branch is behind"),
        |c| replace(c, "push", "push --force-with-lease")
    )
    .disabled_by_default(),
    git_rule!(
        "git_push_pull",
        |c| rejected_push(c)
            && output_contains(
                c,
                &[
                    "Updates were rejected because the tip of your current branch is behind",
                    "Updates were rejected because the remote contains work that you do"
                ]
            ),
        |c| one(c
            .shell()
            .and_(&[&replace_argument(&c.script, "push", "pull"), &c.script]))
    ),
    git_rule!(
        "git_push_without_commits",
        |c| regex!(r"src refspec \w+ does not match any").is_match(c.output()),
        |c| one(c
            .shell()
            .and_(&["git commit -m \"Initial commit\"", &c.script]))
    ),
    git_rule!(
        "git_rebase_merge_dir",
        |c| c.script.contains(" rebase")
            && c.output()
                .contains("It seems that there is already a rebase-merge directory")
            && c.output()
                .contains("I wonder if you are in the middle of another rebase"),
        rebase_merge_fix
    ),
    git_rule!(
        "git_rebase_no_changes",
        |c| c.has_part("rebase")
            && c.has_part("--continue")
            && c.output()
                .contains("No changes - did you forget to use 'git add'?"),
        |_| one("git rebase --skip")
    ),
    git_rule!(
        "git_remote_delete",
        |c| c.script.contains("remote delete"),
        |c| one(c.script.replacen("delete", "remove", 1))
    ),
    git_rule!(
        "git_remote_seturl_add",
        |c| c.script.contains("set-url") && c.output().contains("fatal: No such remote"),
        |c| replace(c, "set-url", "add")
    ),
    git_rule!(
        "git_rm_local_modifications",
        |c| c.script.contains(" rm ")
            && c.output()
                .contains("error: the following file has local modifications")
            && c.output()
                .contains("use --cached to keep the file, or -f to force removal"),
        |c| insert_after(c, "rm", &["--cached", "-f"])
    ),
    git_rule!(
        "git_rm_recursive",
        |c| c.script.contains(" rm ")
            && c.output().contains("fatal: not removing '")
            && c.output().contains("' recursively without -r"),
        |c| insert_after(c, "rm", &["-r"])
    ),
    git_rule!(
        "git_rm_staged",
        |c| c.script.contains(" rm ")
            && c.output()
                .contains("error: the following file has changes staged in the index")
            && c.output()
                .contains("use --cached to keep the file, or -f to force removal"),
        |c| insert_after(c, "rm", &["--cached", "-f"])
    ),
    git_rule!("git_stash", |c| c.output().contains("or stash them"), |c| {
        one(c.shell().and_(&["git stash", &c.script]))
    }),
    git_rule!(
        "git_stash_pop",
        |c| c.script.contains("stash")
            && c.script.contains("pop")
            && c.output().contains(
                "Your local changes to the following files would be overwritten by merge"
            ),
        |c| one(c
            .shell()
            .and_(&["git add --update", "git stash pop", "git reset ."]))
    )
    .priority(900),
    git_rule!(
        "git_tag_force",
        |c| c.has_part("tag") && c.output().contains("already exists"),
        |c| replace(c, "tag", "tag --force")
    ),
    git_rule!(
        "git_two_dashes",
        |c| c.output().contains("error: did you mean `")
            && c.output().contains("` (with two dashes ?)"),
        |c| {
            c.output()
                .split('`')
                .nth(1)
                .and_then(|to| to.strip_prefix('-').map(|from| (from, to)))
                .map_or_else(Vec::new, |(from, to)| replace(c, from, to))
        }
    ),
];

fn replace(c: &Command, from: &str, to: &str) -> Vec<String> {
    one(replace_argument(&c.script, from, to))
}
fn pathspec<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(
        regex!(r"error: pathspec '([^']*)' did not match any file\(s\) known to git"),
        c.output(),
    )
}
fn zero_flag<'c>(c: &'c Command) -> Option<&'c str> {
    c.script_parts()
        .iter()
        .find(|p| p.chars().count() == 2 && p.starts_with('0'))
        .map(String::as_str)
}
fn existing_branch<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(
        regex!(r"fatal: A branch named '(.+)' already exists."),
        c.output(),
    )
}
fn bad_flag<'a>(c: &Command<'a>) -> Option<&'a str> {
    capture(
        regex!(r"fatal: bad flag '(.*?)' used after filename"),
        c.output(),
    )
    .or_else(|| {
        capture(
            regex!(r"fatal: option '(.*?)' must come before non-option arguments"),
            c.output(),
        )
    })
}
fn hooked_command(c: &Command) -> Option<&'static str> {
    ["am", "commit", "push"]
        .into_iter()
        .find(|cmd| c.has_part(cmd))
}
fn rejected_push(c: &Command) -> bool {
    c.script.contains("push")
        && c.output().contains("! [rejected]")
        && c.output().contains("failed to push some refs to")
}

fn bisect_fix(c: &Command) -> Vec<String> {
    let broken = capture(regex!(r"git bisect ([^ $]*).*"), &c.script);
    let usage = capture(regex!(r"usage: git bisect \[([^\]]+)\]"), c.output());
    broken.zip(usage).map_or_else(Vec::new, |(broken, usage)| {
        replace_command(c, broken, &usage.split('|').collect::<Vec<_>>())
    })
}

fn branch_zero_fix(c: &Command) -> Vec<String> {
    let Some(flag) = zero_flag(c) else {
        return Vec::new();
    };
    let fixed = c.script.replace(flag, &flag.replace('0', "-"));
    one(
        if c.output().contains("A branch named '") && c.output().contains("' already exists.") {
            c.shell().and_(&[&format!("git branch -D {flag}"), &fixed])
        } else {
            fixed
        },
    )
}

fn branch_exists_fix(c: &Command) -> Vec<String> {
    let Some(branch) = existing_branch(c) else {
        return Vec::new();
    };
    let branch = branch.replace('\'', "\\'");
    let mut fixes = Vec::new();
    for flag in ["-d", "-D"] {
        for create in ["branch", "checkout -b"] {
            fixes.push(c.shell().and_(&[
                &format!("git branch {flag} {branch}"),
                &format!("git {create} {branch}"),
            ]));
        }
    }
    fixes.push(format!("git checkout {branch}"));
    fixes
}

fn branches() -> Vec<String> {
    run_stdout("git", &["branch", "-a", "--no-color", "--no-column"])
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.contains("->"))
        .map(|line| {
            let line = line.trim_start_matches('*').trim();
            line.strip_prefix("remotes/")
                .and_then(|remote| remote.split_once('/').map(|(_, branch)| branch))
                .unwrap_or(line)
                .to_owned()
        })
        .collect()
}

fn checkout_fix(c: &Command) -> Vec<String> {
    let Some(file) = pathspec(c) else {
        return Vec::new();
    };
    let mut fixes: Vec<_> = closest_match(file, &branches(), 0.6)
        .into_iter()
        .map(|branch| replace_argument(&c.script, file, &branch))
        .collect();
    if c.part(1) == "checkout" {
        fixes.push(replace_argument(&c.script, "checkout", "checkout -b"));
    }
    if fixes.is_empty() {
        fixes.push(c.shell().and_(&[&format!("git branch {file}"), &c.script]));
    }
    fixes
}

fn clone_missing(c: &Command) -> bool {
    if c.script_parts().len() != 1
        || c.ctx().which(c.part(0)).is_some()
        || !output_contains(
            c,
            &[
                "No such file or directory",
                "not found",
                "is not recognised as",
            ],
        )
    {
        return false;
    }
    let script = c.script.as_str();
    if let Some(rest) = script
        .strip_prefix("http://")
        .or_else(|| script.strip_prefix("https://"))
    {
        return !rest.split('/').next().unwrap_or_default().is_empty();
    }
    // scp-style git URLs have an SSH user and a colon; reject other schemes.
    (script.starts_with("ssh://") || !script.contains("://"))
        && script.contains('@')
        && script.contains(':')
}

fn stash_fix(c: &Command) -> Vec<String> {
    if let Some(fix) = closest_match(
        c.part(2),
        &[
            "apply", "branch", "clear", "drop", "list", "pop", "save", "show",
        ],
        0.6,
    ) {
        return replace(c, c.part(2), &fix);
    }
    let mut parts = c.script_parts().to_vec();
    parts.insert(2.min(parts.len()), "save".into());
    one(parts.join(" "))
}

fn flag_after_filename(c: &Command) -> Vec<String> {
    let Some(flag) = bad_flag(c) else {
        return Vec::new();
    };
    let mut parts = c.script_parts().to_vec();
    let Some(bad_index) = parts.iter().position(|p| p == flag) else {
        return Vec::new();
    };
    let Some(file_index) = parts[..bad_index].iter().rposition(|p| !p.starts_with('-')) else {
        return Vec::new();
    };
    parts.swap(bad_index, file_index);
    one(parts.join(" "))
}

fn pull_fix(c: &Command) -> Vec<String> {
    let Some(line) = c.output().split('\n').rev().nth(2) else {
        return Vec::new();
    };
    let line = line.trim();
    let branch = line.split(' ').next_back().unwrap_or_default();
    let upstream = line
        .replace("<remote>", "origin")
        .replace("<branch>", branch);
    one(c.shell().and_(&[&upstream, &c.script]))
}

fn push_fix(c: &Command) -> Vec<String> {
    let mut parts = c.script_parts().to_vec();
    if let Some(index) = parts
        .iter()
        .position(|p| p == "--set-upstream")
        .or_else(|| parts.iter().position(|p| p == "-u"))
    {
        parts.remove(index);
        if index < parts.len() {
            parts.remove(index);
        }
    } else if let Some(index) = parts.iter().position(|p| p == "push") {
        while parts.len() > index + 1 && parts.last().is_some_and(|p| !p.starts_with('-')) {
            parts.pop();
        }
    }
    let Some(arguments) = capture_all(regex!(r"git push (.*)"), c.output())
        .into_iter()
        .next_back()
    else {
        return Vec::new();
    };
    one(replace_argument(
        &parts.join(" "),
        "push",
        &format!("push {}", arguments.replace('\'', "\\'").trim()),
    ))
}

fn rebase_merge_fix(c: &Command) -> Vec<String> {
    let mut options = vec![
        "git rebase --continue",
        "git rebase --abort",
        "git rebase --skip",
    ];
    if let Some(line) = c.output().split('\n').rev().nth(3) {
        options.push(line.trim());
    }
    get_close_matches(&c.script, &options, 4, 0.0)
}

fn insert_after(c: &Command, word: &str, flags: &[&str]) -> Vec<String> {
    let Some(index) = c.script_parts().iter().position(|p| p == word) else {
        return Vec::new();
    };
    flags
        .iter()
        .map(|flag| {
            let mut parts = c.script_parts().to_vec();
            parts.insert(index + 1, (*flag).into());
            parts.join(" ")
        })
        .collect()
}
