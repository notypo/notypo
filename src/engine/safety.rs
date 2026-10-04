//! The safety gate every candidate passes before it is shown or run.
//!
//! The gate compares a corrected command with the original and records why
//! it needs approval. Risk is separate from ranking: a top candidate can
//! still need explicit approval or be refused. Static analysis can't prove
//! an arbitrary command harmless, so syntax the parser can't analyze and
//! opaque declared effects always need a person to approve them.

use super::diagnosis::effective_program;
use super::parser::{self, DiagnosticKind, Script, SimpleCommand};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Decision {
    Allow,
    /// Needs explicit approval, even when confirmation is otherwise off.
    Confirm,
    Refuse,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafetyAssessment {
    pub decision: Decision,
    pub reasons: Vec<String>,
}

impl SafetyAssessment {
    /// Raises the decision to at least `decision`, recording why.
    pub(crate) fn flag(&mut self, decision: Decision, reason: String) {
        self.decision = self.decision.max(decision);
        if !self.reasons.contains(&reason) {
            self.reasons.push(reason);
        }
    }
}

/// Something a candidate's source does besides running the command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclaredEffect {
    /// Deletes the files a previous archive extraction left behind
    /// (`dirty_untar`, `dirty_unzip`).
    RemovesExtractedFiles { rule: &'static str },
    /// Deletes the known_hosts keys an ssh warning pointed at, accepting the
    /// host's new identity (`ssh_known_hosts`).
    RemovesHostKeys { rule: &'static str },
    /// A legacy rule callback whose effect isn't described.
    Opaque { rule: &'static str },
}

impl DeclaredEffect {
    /// The declared effect of a legacy rule's side-effect callback.
    pub fn for_rule(rule: &'static str) -> DeclaredEffect {
        match rule {
            "dirty_untar" | "dirty_unzip" => DeclaredEffect::RemovesExtractedFiles { rule },
            "ssh_known_hosts" => DeclaredEffect::RemovesHostKeys { rule },
            _ => DeclaredEffect::Opaque { rule },
        }
    }

    fn describe(self) -> String {
        match self {
            DeclaredEffect::RemovesExtractedFiles { rule } => {
                format!("rule {rule} also deletes the files the archive extracted here")
            }
            DeclaredEffect::RemovesHostKeys { rule } => format!(
                "rule {rule} also deletes known_hosts keys, accepting the host's new identity"
            ),
            DeclaredEffect::Opaque { rule } => {
                format!("rule {rule} also changes something outside the command")
            }
        }
    }
}

const PRIVILEGE: &[&str] = &["sudo", "doas", "su", "pkexec", "run0"];
const DESTRUCTIVE_PROGRAMS: &[&str] = &[
    "rm", "rmdir", "unlink", "shred", "srm", "dd", "wipefs", "truncate", "fdisk", "sfdisk",
    "parted", "kill", "pkill", "killall", "shutdown", "reboot", "halt", "poweroff",
];
/// Programs whose short `-f`/`-r` flags mean force or recursion.
const FORCE_SHORT: &[&str] = &[
    "rm", "cp", "mv", "ln", "chmod", "chown", "chgrp", "git", "docker", "podman",
];
const FORCE_LONG: &[&str] = &[
    "--force",
    "--hard",
    "--recursive",
    "--force-with-lease",
    "--delete",
    "--purge",
    "--prune",
    "--mirror",
    "--no-preserve-root",
    // Skips the tool's own confirmation (terraform, tofu, pulumi --yes).
    "-auto-approve",
    "--auto-approve",
];
const DESTRUCTIVE_VERBS: &[&str] = &[
    "delete",
    "destroy",
    "terminate",
    "remove",
    "purge",
    "prune",
    "drop",
    "uninstall",
    "wipe",
    "erase",
    "deregister",
    "revoke",
    "rb",
    "rm",
    "rmi",
];
const PACKAGE_MANAGERS: &[&str] = &[
    "apt", "apt-get", "aptitude", "yum", "dnf", "zypper", "pacman", "yay", "paru", "brew", "port",
    "pip", "pip3", "pipx", "npm", "pnpm", "yarn", "gem", "cargo", "snap", "flatpak", "apk",
    "nix-env", "choco", "winget", "scoop", "conda", "mamba", "uv",
];
const PACKAGE_VERBS: &[&str] = &[
    "install",
    "i",
    "add",
    "reinstall",
    "uninstall",
    "remove",
    "rm",
    "erase",
    "purge",
    "autoremove",
    "upgrade",
    "update",
    "dist-upgrade",
    "full-upgrade",
];
const REMOTE: &[&str] = &["ssh", "scp", "sftp", "rsync", "mosh"];
/// Programs that write to, move, or change the files named as arguments.
const WRITERS: &[&str] = &[
    "cp", "mv", "ln", "install", "rsync", "scp", "tee", "touch", "chmod", "chown", "chgrp", "tar",
    "unzip", "patch", "sed", "truncate",
];
const INTERPRETERS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "fish", "python", "python3", "perl", "ruby", "node",
];
const DOWNLOADERS: &[&str] = &["curl", "wget", "fetch"];

fn base(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// Literal words of a command, with opaque words as their source text.
fn words<'s>(command: &'s SimpleCommand, src: &'s str) -> Vec<&'s str> {
    command
        .words
        .iter()
        .map(|w| w.literal().unwrap_or_else(|| w.span.of(src)))
        .collect()
}

fn output_redirections(script: &Script) -> HashSet<(String, String)> {
    script
        .commands
        .iter()
        .flat_map(|c| &c.redirections)
        .chain(script.compounds.iter().flat_map(|c| &c.redirections))
        .filter(|r| r.operator.of(&script.source).contains('>'))
        .map(|r| {
            (
                r.operator
                    .of(&script.source)
                    .trim_start_matches(|c: char| c.is_ascii_digit())
                    .to_owned(),
                r.target
                    .as_ref()
                    .map_or("", |t| t.span.of(&script.source))
                    .to_owned(),
            )
        })
        .collect()
}

fn substitutions(script: &Script) -> HashSet<&str> {
    script
        .commands
        .iter()
        .flat_map(|c| {
            c.assignments
                .iter()
                .chain(&c.words)
                .chain(c.redirections.iter().filter_map(|r| r.target.as_ref()))
        })
        // Loop items, case subjects, and compound redirections expand too.
        .chain(script.compounds.iter().flat_map(|c| {
            c.words
                .iter()
                .chain(c.redirections.iter().filter_map(|r| r.target.as_ref()))
        }))
        .filter(|w| w.literal().is_none())
        .map(|w| w.span.of(&script.source))
        .filter(|raw| {
            if script.dialect == parser::Dialect::Fish {
                raw.contains('(')
            } else {
                ["$(", "`", "<(", ">("].iter().any(|s| raw.contains(s))
            }
        })
        .collect()
}

fn has_privilege(script: &Script) -> bool {
    script.commands.iter().any(|c| {
        let program = effective_program(&c.words).unwrap_or(c.words.len());
        c.words[..program.min(c.words.len())]
            .iter()
            .chain(c.words.get(program))
            .any(|w| w.literal().is_some_and(|v| PRIVILEGE.contains(&base(v))))
    })
}

pub fn assess(original: &Script, candidate: &str, effects: &[DeclaredEffect]) -> SafetyAssessment {
    let mut gate = SafetyAssessment {
        decision: Decision::Allow,
        reasons: Vec::new(),
    };
    if candidate.trim().is_empty() {
        gate.flag(Decision::Refuse, "the correction is empty".into());
        return gate;
    }
    for effect in effects {
        gate.flag(Decision::Confirm, effect.describe());
    }
    if !original.is_fully_supported() {
        gate.flag(
            Decision::Confirm,
            "the original command uses syntax notypo can't analyze".into(),
        );
    }
    let new = parser::parse_with_dialect(candidate, original.dialect);
    if new.commands.iter().any(|command| {
        command.words.first().is_some_and(|w| w.literal().is_none())
            || effective_program(&command.words).is_none()
                && command.words.iter().any(|word| word.literal().is_none())
    }) {
        gate.flag(
            Decision::Confirm,
            "the executable depends on an expansion".into(),
        );
    }
    for diagnostic in &new.diagnostics {
        match diagnostic.kind {
            DiagnosticKind::CompoundCommand | DiagnosticKind::HereDocument => gate.flag(
                Decision::Confirm,
                "uses shell syntax notypo can't analyze".into(),
            ),
            DiagnosticKind::HistorySubstitution | DiagnosticKind::QuotedNewline => gate.flag(
                Decision::Refuse,
                "tcsh would not run this text as written".into(),
            ),
            _ => gate.flag(Decision::Refuse, "the correction is malformed".into()),
        }
    }

    if new.commands.len() > original.commands.len() {
        gate.flag(Decision::Confirm, "adds another command".into());
    }
    if new.compound_shapes() != original.compound_shapes() {
        gate.flag(
            Decision::Confirm,
            "changes the loops, conditionals, groups, or functions around the commands".into(),
        );
    }
    let old_ops: HashSet<_> = original
        .commands
        .iter()
        .filter_map(|c| c.connector)
        .collect();
    for op in new.commands.iter().filter_map(|c| c.connector) {
        if !old_ops.contains(&op) {
            gate.flag(Decision::Confirm, format!("adds the shell operator {op:?}"));
        }
    }
    let old_subs = substitutions(original);
    for sub in substitutions(&new) {
        if !old_subs.contains(sub) {
            gate.flag(Decision::Confirm, format!("adds the substitution {sub}"));
        } else {
            gate.flag(
                Decision::Confirm,
                format!("runs a command substitution with unknown effects: {sub}"),
            );
        }
    }
    let old_writes = output_redirections(original);
    for (op, target) in output_redirections(&new) {
        if !old_writes.contains(&(op.clone(), target.clone())) {
            gate.flag(Decision::Confirm, format!("writes to {target} ({op})"));
        }
    }
    if has_privilege(&new) && !has_privilege(original) {
        gate.flag(Decision::Confirm, "adds elevated privileges".into());
    }

    let src = &new.source;
    for (index, command) in new.commands.iter().enumerate() {
        let all = words(command, src);
        let Some(p) = effective_program(&command.words) else {
            continue;
        };
        let program = base(all[p]);
        let args = &all[p + 1..];
        let verbs: Vec<&str> = args
            .iter()
            .copied()
            .filter(|a| !a.starts_with('-'))
            .take(3)
            .collect();

        if DESTRUCTIVE_PROGRAMS.contains(&program) || program.starts_with("mkfs") {
            gate.flag(
                Decision::Confirm,
                format!("{program} deletes or stops things"),
            );
        }
        if let Some(flag) = args.iter().find(|a| FORCE_LONG.contains(a)) {
            gate.flag(Decision::Confirm, format!("uses {flag}"));
        }
        if FORCE_SHORT.contains(&program)
            && let Some(flag) = args.iter().find(|a| {
                a.len() > 1
                    && a.starts_with('-')
                    && !a.starts_with("--")
                    && a.chars()
                        .skip(1)
                        .any(|c| matches!(c, 'f' | 'r' | 'R' | 'D'))
            })
        {
            gate.flag(Decision::Confirm, format!("uses {program} {flag}"));
        }
        if program == "git" && is_destructive_git(&verbs, args) {
            gate.flag(
                Decision::Confirm,
                format!("git {} can discard or rewrite data", verbs[0]),
            );
        }
        if let Some(verb) = verbs.iter().find(|v| {
            DESTRUCTIVE_VERBS
                .iter()
                .any(|d| **v == *d || v.starts_with(&format!("{d}-")))
        }) {
            gate.flag(
                Decision::Confirm,
                format!("{program} {verb} is a destructive operation"),
            );
        }
        if PACKAGE_MANAGERS.contains(&program)
            && (verbs.first().is_some_and(|v| PACKAGE_VERBS.contains(v))
                || program == "pacman"
                    && args
                        .iter()
                        .any(|a| a.starts_with("-S") || a.starts_with("-R") || a.starts_with("-U")))
        {
            gate.flag(
                Decision::Confirm,
                format!("{program} changes installed packages"),
            );
        }
        let changed = original
            .commands
            .get(index)
            .is_none_or(|old| words(old, &original.source) != all);
        if changed && WRITERS.contains(&program) {
            let before: Vec<&str> = original
                .commands
                .get(index)
                .map(|old| words(old, &original.source))
                .unwrap_or_default();
            for target in args.iter().filter(|a| !a.starts_with('-')) {
                if !before.contains(target) {
                    gate.flag(Decision::Confirm, format!("{program} now targets {target}"));
                }
            }
        }
        if changed && REMOTE.contains(&program) {
            gate.flag(
                Decision::Confirm,
                format!("{program} reaches another host with changed arguments"),
            );
        }
        if changed && matches!(program, "eval" | "source" | ".") {
            gate.flag(Decision::Confirm, format!("{program} runs shell text"));
        }
        if INTERPRETERS.contains(&program) {
            let pipeline = new.pipeline_of(index);
            let downloads = new.commands[pipeline.start..index].iter().any(|c| {
                effective_program(&c.words)
                    .and_then(|p| c.words[p].literal())
                    .is_some_and(|w| DOWNLOADERS.contains(&base(w)))
            });
            if downloads {
                gate.flag(
                    Decision::Confirm,
                    "pipes downloaded content into an interpreter".into(),
                );
            }
        }
    }
    gate
}

/// Whether the failed command itself may be rerun to read its output (only
/// when `replay_for_diagnosis` asks for it). The same policy as for a
/// corrected command applies, and any output redirection would write again.
pub fn assess_replay(command: &str) -> SafetyAssessment {
    assess_replay_with_dialect(command, parser::Dialect::Posix)
}

pub fn assess_replay_with_dialect(command: &str, dialect: parser::Dialect) -> SafetyAssessment {
    let script = parser::parse_with_dialect(command, dialect);
    let mut gate = assess(&script, command, &[]);
    for (op, target) in output_redirections(&script) {
        gate.flag(Decision::Confirm, format!("writes to {target} ({op})"));
    }
    if !script.compounds.is_empty() {
        // A loop or condition may run its commands many times, or never stop.
        gate.flag(Decision::Confirm, "reruns a compound command".into());
    }
    if script.commands.is_empty() {
        gate.flag(Decision::Refuse, "nothing to run".into());
    }
    gate
}

fn is_destructive_git(verbs: &[&str], args: &[&str]) -> bool {
    let has = |flags: &[&str]| args.iter().any(|a| flags.contains(a));
    match verbs.first() {
        Some(&"reset") => has(&["--hard", "--merge", "--keep"]),
        Some(&"clean" | &"rm" | &"restore" | &"filter-branch" | &"filter-repo") => true,
        Some(&"push") => {
            has(&[
                "-f",
                "--force",
                "--force-with-lease",
                "--delete",
                "-d",
                "--mirror",
            ]) || args
                .iter()
                .any(|a| a.starts_with('+') || a.starts_with(':'))
        }
        Some(&"checkout") => has(&["--", ".", "-f", "--force"]),
        Some(&"branch") => has(&["-D", "-d", "--delete"]),
        Some(&"stash") => verbs.get(1).is_some_and(|v| matches!(*v, "drop" | "clear")),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(original: &str, candidate: &str) -> SafetyAssessment {
        assess(&parser::parse(original), candidate, &[])
    }

    #[test]
    fn spelling_fixes_are_allowed() {
        for (original, candidate) in [
            ("git sttus", "git status"),
            ("gti log | less", "git log | less"),
            (
                "aws ec2 describ-instances --regoin eu-west-1",
                "aws ec2 describe-instances --region eu-west-1",
            ),
            ("ls --colro=auto > out.txt", "ls --color=auto > out.txt"),
            ("sudo apt-get updte", "sudo apt-get updtae"),
            ("{ ehco x; } > out", "{ echo x; } > out"),
            (
                "for f in a b; do ctt \"$f\"; done",
                "for f in a b; do cat \"$f\"; done",
            ),
            (
                "if gti diff; then echo y; fi",
                "if git diff; then echo y; fi",
            ),
        ] {
            let gate = check(original, candidate);
            assert_eq!(
                gate.decision,
                Decision::Allow,
                "{candidate}: {:?}",
                gate.reasons
            );
        }
    }

    #[test]
    fn risky_changes_need_approval() {
        for (original, candidate) in [
            ("mkdir /x", "sudo mkdir /x"),
            ("rm-rf build", "rm -rf build"),
            ("git rest --hard", "git reset --hard"),
            ("git psuh -f", "git push -f"),
            ("echo x > a.txt", "echo x > b.txt"),
            ("ls", "ls && rm x"),
            ("ls", "ls $(id)"),
            ("brew instal jq", "brew install jq"),
            ("aws ec2 terminat-instances", "aws ec2 terminate-instances"),
            (
                "gcloud compute instances delet x",
                "gcloud compute instances delete x",
            ),
            ("curl https://x | bsh", "curl https://x | bash"),
            ("ssh hots", "ssh host"),
            ("cp a.txt /tmp/b", "cp a.txt /tmp/c"),
            ("tofu aply -auto-approve", "tofu apply -auto-approve"),
            ("ls", "for x in a; do ls; done"),
            (
                "for f in $(ls); do ctt $f; done",
                "for f in $(ls); do cat $f; done",
            ),
            ("{ ehco x; } > out", "{ echo x; } > other"),
            ("if a; then b; fi", "if a; then b; else c; fi"),
            ("while gti pull; do :; done", "until git pull; do :; done"),
            ("f() { ls; }", "f() { ls; } > log"),
        ] {
            let gate = check(original, candidate);
            assert_eq!(
                gate.decision,
                Decision::Confirm,
                "{original} -> {candidate}: {:?}",
                gate.reasons
            );
            assert!(!gate.reasons.is_empty());
        }
    }

    #[test]
    fn malformed_corrections_and_declared_effects() {
        assert_eq!(check("ls", "ls 'oops").decision, Decision::Refuse);
        assert_eq!(check("ls", "ls &&").decision, Decision::Refuse);
        assert_eq!(check("ls", "  ").decision, Decision::Refuse);
        assert_eq!(
            check("if ls; then git sttus; fi", "git status").decision,
            Decision::Confirm,
            "a legacy replacement cannot erase unknown syntax and then auto-execute"
        );
        let gate = assess(
            &parser::parse("cd foo"),
            "mkdir -p foo && cd foo",
            &[DeclaredEffect::Opaque { rule: "cd_mkdir" }],
        );
        assert_eq!(gate.decision, Decision::Confirm);
        assert!(gate.reasons.iter().any(|r| r.contains("cd_mkdir")));
        let gate = assess(
            &parser::parse("ssh host"),
            "ssh host",
            &[DeclaredEffect::for_rule("ssh_known_hosts")],
        );
        assert_eq!(gate.decision, Decision::Confirm);
        assert!(
            gate.reasons[0].contains("known_hosts"),
            "{:?}",
            gate.reasons
        );
        assert!(matches!(
            DeclaredEffect::for_rule("dirty_unzip"),
            DeclaredEffect::RemovesExtractedFiles { .. }
        ));
        assert_eq!(check("cd..", "cd ..").decision, Decision::Allow);
        assert_eq!(
            check("mvv x y", "mv x y").decision,
            Decision::Allow,
            "the user's own targets are unchanged"
        );
    }

    #[test]
    fn replays_only_harmless_commands() {
        assert_eq!(assess_replay("git sttus").decision, Decision::Allow);
        assert_eq!(
            assess_replay("printf x > marker").decision,
            Decision::Confirm
        );
        assert_eq!(assess_replay("rm -r build").decision, Decision::Confirm);
        assert_eq!(assess_replay("brew instal jq").decision, Decision::Allow);
        for compound in [
            "if x; then y; fi",
            "{ git sttus; }",
            "while true; do git sttus; done",
            "! git sttus",
        ] {
            assert_eq!(assess_replay(compound).decision, Decision::Confirm);
        }
    }

    #[test]
    fn existing_privilege_is_not_new() {
        assert_eq!(
            check("sudo apt-get updte", "sudo apt-get update").decision,
            Decision::Confirm,
            "package changes still need approval"
        );
        let gate = check("sudo sytemctl status x", "sudo systemctl status x");
        assert_eq!(gate.decision, Decision::Allow, "{:?}", gate.reasons);
    }

    #[test]
    fn substitutions_and_expanded_executables_need_approval_in_each_dialect() {
        for (dialect, source, candidate) in [
            (
                parser::Dialect::Posix,
                "gti $(touch marker)",
                "git $(touch marker)",
            ),
            (
                parser::Dialect::Posix,
                "gti `touch marker`",
                "git `touch marker`",
            ),
            (
                parser::Dialect::Fish,
                "gti (touch marker)",
                "git (touch marker)",
            ),
            (
                parser::Dialect::Fish,
                "gti $(touch marker)",
                "git $(touch marker)",
            ),
            (parser::Dialect::Fish, "$app sttus", "$app status"),
            (parser::Dialect::Fish, "env $app sttus", "env $app status"),
            (
                parser::Dialect::Posix,
                "sudo $app sttus",
                "sudo $app status",
            ),
            (
                parser::Dialect::Fish,
                "gti <(touch marker)",
                "git <(touch marker)",
            ),
            (
                parser::Dialect::Posix,
                "gti <$(touch marker)",
                "git <$(touch marker)",
            ),
        ] {
            let original = parser::parse_with_dialect(source, dialect);
            let gate = assess(&original, candidate, &[]);
            assert_eq!(
                gate.decision,
                Decision::Confirm,
                "{candidate}: {:?}",
                gate.reasons
            );
            assert_eq!(
                assess_replay_with_dialect(candidate, dialect).decision,
                Decision::Confirm
            );
        }
        for source in [
            "git '(touch marker)'",
            "git `touch_marker`",
            "git \\(touch_marker\\)",
        ] {
            let parsed = parser::parse_with_dialect(source, parser::Dialect::Fish);
            assert_eq!(
                assess(&parsed, source, &[]).decision,
                Decision::Allow,
                "{source}"
            );
        }
    }
}
