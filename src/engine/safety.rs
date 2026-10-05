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

mod operations;

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
const FORCE_SHORT: &[&str] = &["rm", "cp", "mv", "ln", "chmod", "chown", "chgrp", "git"];
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
const REMOTE: &[&str] = &[
    "ssh",
    "scp",
    "sftp",
    "rsync",
    "mosh",
    "autossh",
    "sshuttle",
    "openvpn",
    "openconnect",
];
/// Programs that write to, move, or change the files named as arguments.
const WRITERS: &[&str] = &[
    "cp", "mv", "ln", "install", "rsync", "scp", "tee", "touch", "chmod", "chown", "chgrp", "tar",
    "unzip", "patch", "sed", "truncate",
];
const INTERPRETERS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "fish", "python", "python3", "perl", "ruby", "node",
];
const DOWNLOADERS: &[&str] = &["curl", "wget", "fetch"];

/// PowerShell cmdlets and their aliases (compared in lowercase, as
/// PowerShell resolves them) that delete, stop, or reformat things.
const POWERSHELL_DESTRUCTIVE: &[&str] = &[
    "remove-item",
    "ri",
    "del",
    "erase",
    "rd",
    "clear-content",
    "clc",
    "clear-item",
    "cli",
    "clear-itemproperty",
    "clp",
    "clear-recyclebin",
    "stop-process",
    "spps",
    "stop-computer",
    "restart-computer",
    "stop-service",
    "spsv",
    "restart-service",
    "format-volume",
    "clear-disk",
    "initialize-disk",
    "remove-partition",
    "set-executionpolicy",
];
/// Verbs whose cmdlets change or remove state (`Remove-AzVM`, `Stop-Job`).
const POWERSHELL_RISKY_VERBS: &[&str] = &[
    "remove",
    "clear",
    "stop",
    "uninstall",
    "unregister",
    "disable",
    "reset",
    "restart",
    "dismount",
    "revoke",
    "suspend",
    "block",
];
/// Verbs whose cmdlets only read, where `-Force` and `-Recurse` widen what
/// is read (`Get-ChildItem -Recurse -Force` lists hidden files recursively).
const POWERSHELL_READ_VERBS: &[&str] = &[
    "get",
    "find",
    "search",
    "select",
    "show",
    "test",
    "measure",
    "compare",
    "resolve",
    "read",
    "group",
    "sort",
    "convertto",
    "convertfrom",
];
/// Aliases of such cmdlets.
const POWERSHELL_READERS: &[&str] = &[
    "gci", "dir", "ls", "gc", "cat", "type", "gi", "gp", "gm", "gps", "ps", "gsv", "gal", "gcm",
    "sls", "select", "sort", "measure", "compare", "diff",
];
/// Cmdlets that run code, start processes, or reach other machines.
const POWERSHELL_CODE: &[&str] = &[
    "invoke-expression",
    "iex",
    "invoke-command",
    "icm",
    "enter-pssession",
    "etsn",
    "new-pssession",
    "nsn",
    "start-job",
    "sajb",
    "start-process",
    "saps",
    "start",
    "invoke-item",
    "ii",
    "invoke-restmethod",
    "irm",
    "invoke-webrequest",
    "iwr",
];
/// Cmdlets that write files or items at the paths they are given.
const POWERSHELL_WRITERS: &[&str] = &[
    "set-content",
    "sc",
    "add-content",
    "ac",
    "out-file",
    "tee-object",
    "tee",
    "copy-item",
    "cpi",
    "copy",
    "move-item",
    "mi",
    "move",
    "rename-item",
    "rni",
    "ren",
    "new-item",
    "ni",
    "set-item",
    "si",
    "export-csv",
    "epcsv",
    "export-clixml",
];

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
            } else if script.dialect == parser::Dialect::PowerShell {
                // Backticks are escapes here; `$(...)` and `@(...)` run code.
                raw.contains("$(") || raw.contains("@(")
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
    assess_resolved(original, candidate, effects, |_| None)
}

/// Preserve package-manager policy for versioned and renamed executables.
/// Identity is read from the resolved file, never inferred from spelling.
pub fn assess_with_context(
    original: &Script,
    candidate: &str,
    effects: &[DeclaredEffect],
    context: &crate::types::Context,
) -> SafetyAssessment {
    let resolve = |program: &str| {
        let path = context.which(program)?;
        super::probe::is_trusted_location(&path)
            .then(|| super::native::package_manager_name(&path))
            .flatten()
    };
    let mut gate = assess_resolved(original, candidate, effects, resolve);
    // An alias hides what runs (`rmf` for `rm -rf`): judge the expansion too,
    // against the original expanded the same way.
    use super::aliases::{Expansion, expand};
    let aliases = context.aliases();
    let dialect = original.dialect;
    let expanded_candidate = match expand(candidate, dialect, aliases) {
        Expansion::Unchanged => return gate,
        Expansion::Unanalyzable(name) => {
            gate.flag(
                Decision::Confirm,
                format!("runs the alias {name}, whose expansion notypo can't analyze"),
            );
            return gate;
        }
        Expansion::Expanded(text) => text,
    };
    let expanded_original = match expand(&original.source, dialect, aliases) {
        Expansion::Expanded(text) => parser::parse_with_dialect(&text, dialect),
        _ => original.clone(),
    };
    let expanded = assess_resolved(&expanded_original, &expanded_candidate, effects, resolve);
    for reason in expanded.reasons {
        if !gate.reasons.contains(&reason) {
            gate.flag(expanded.decision, format!("{reason} (through an alias)"));
        }
    }
    gate
}

fn assess_resolved(
    original: &Script,
    candidate: &str,
    effects: &[DeclaredEffect],
    resolve: impl Fn(&str) -> Option<&'static str>,
) -> SafetyAssessment {
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
        // PowerShell resolves commands without regard to case, with `\` or
        // a module name (`Microsoft.PowerShell.Management\Remove-Item`).
        let lowered;
        let program = if new.dialect == parser::Dialect::PowerShell {
            lowered = base(all[p])
                .rsplit('\\')
                .next()
                .unwrap_or_default()
                .to_lowercase();
            resolve(&lowered).unwrap_or(&lowered)
        } else {
            resolve(all[p]).unwrap_or_else(|| base(all[p]))
        };
        let args = &all[p + 1..];
        let verbs = operations::verbs(program, args);
        if new.dialect == parser::Dialect::PowerShell {
            if POWERSHELL_DESTRUCTIVE.contains(&program)
                || program
                    .split_once('-')
                    .is_some_and(|(verb, _)| POWERSHELL_RISKY_VERBS.contains(&verb))
            {
                gate.flag(
                    Decision::Confirm,
                    format!("{program} deletes, stops, or changes things"),
                );
            }
            if POWERSHELL_CODE.contains(&program) {
                gate.flag(
                    Decision::Confirm,
                    format!("{program} runs code, starts processes, or reaches other machines"),
                );
            }
            let reads = POWERSHELL_READERS.contains(&program)
                || program
                    .split_once('-')
                    .is_some_and(|(verb, _)| POWERSHELL_READ_VERBS.contains(&verb));
            for flag in args.iter().filter(|a| {
                let name = a.split(':').next().unwrap_or_default().to_lowercase();
                !reads && (name == "-force" || name == "-recurse")
            }) {
                gate.flag(Decision::Confirm, format!("uses {flag}"));
            }
        }

        if DESTRUCTIVE_PROGRAMS.contains(&program) || program.starts_with("mkfs") {
            gate.flag(
                Decision::Confirm,
                format!("{program} deletes or stops things"),
            );
        }
        if let Some(reason) = operations::risk(program, args) {
            gate.flag(Decision::Confirm, format!("{program} {reason}"));
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
        if (PACKAGE_MANAGERS.contains(&program)
            && (verbs.first().is_some_and(|v| PACKAGE_VERBS.contains(v))
                || program == "pacman"
                    && args.iter().any(|a| {
                        a.starts_with("-S") || a.starts_with("-R") || a.starts_with("-U")
                    })))
            || verbs.first().is_some_and(|word| {
                [
                    "plugin",
                    "plugins",
                    "extension",
                    "extensions",
                    "component",
                    "components",
                ]
                .contains(word)
            }) && verbs
                .get(1)
                .is_some_and(|word| PACKAGE_VERBS.contains(word))
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
        if changed
            && (WRITERS.contains(&program)
                || new.dialect == parser::Dialect::PowerShell
                    && POWERSHELL_WRITERS.contains(&program))
        {
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
    assess_replay_using(command, dialect, None)
}

pub fn assess_replay_with_context(
    command: &str,
    dialect: parser::Dialect,
    context: &crate::types::Context,
) -> SafetyAssessment {
    assess_replay_using(command, dialect, Some(context))
}

fn assess_replay_using(
    command: &str,
    dialect: parser::Dialect,
    context: Option<&crate::types::Context>,
) -> SafetyAssessment {
    let script = parser::parse_with_dialect(command, dialect);
    let mut gate = match context {
        Some(context) => assess_with_context(&script, command, &[], context),
        None => assess(&script, command, &[]),
    };
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

    #[cfg(unix)]
    #[test]
    fn package_metadata_preserves_policy_for_versioned_and_renamed_launchers() {
        use crate::engine::native::tests::Dir;
        use crate::settings::Settings;
        use crate::shells::Shell;
        use crate::types::Context;

        let dir = Dir::new("package-risk-identity");
        let pip = dir.script(
            "package-tool",
            "#!/usr/bin/env python3\nfrom pip._internal.cli.main import main\n",
        );
        let npm = dir.script("node_modules/npm/bin/renamed", "#!/usr/bin/env node\n");
        let npx = dir.script("node_modules/npm/bin/runner", "#!/usr/bin/env node\n");
        dir.script(
            "node_modules/npm/package.json",
            r#"{"name":"npm","bin":{"npm":"bin/renamed","npx":"bin/runner"}}"#,
        );
        let unrelated = dir.script("unrelated", "#!/bin/sh\n# An unrelated application\n");
        dir.script("rust/lib/rustlib/manifest-cargo-test", "file:bin/cargo\n");
        let cargo = dir.script("rust/bin/cargo", "#!/bin/sh\n");
        for shell in [Shell::Bash, Shell::Fish, Shell::Tcsh] {
            let ctx = Context::new(Settings::default(), shell, "fuck".into())
                .with_which("package-tool", Some(pip.to_str().unwrap()))
                .with_which("pip3.14", Some(pip.to_str().unwrap()))
                .with_which("js-packages", Some(npm.to_str().unwrap()))
                .with_which("js-runner", Some(npx.to_str().unwrap()))
                .with_which("rust-packages", Some(cargo.to_str().unwrap()))
                .with_which("unrelated", Some(unrelated.to_str().unwrap()));
            let dialect = parser::Dialect::for_shell(shell).unwrap();
            for source in [
                "package-tool install example",
                "pip3.14 --cache-dir /tmp install example",
                "sudo -n package-tool install example",
                "npx package-tool install example",
                "js-packages install example",
            ] {
                let script = parser::parse_with_dialect(source, dialect);
                let gate = assess_with_context(&script, source, &[], &ctx);
                assert_eq!(gate.decision, Decision::Confirm, "{shell:?}: {source}");
                assert!(
                    gate.reasons
                        .iter()
                        .any(|reason| reason.contains("changes installed packages")),
                    "{gate:?}"
                );
                assert_eq!(
                    assess_replay_with_context(source, dialect, &ctx).decision,
                    Decision::Confirm
                );
            }
            for source in [
                "package-tool show install",
                "pip3.14 --cache-dir uninstall list",
                "js-packages view install",
                "unrelated install example",
                "rust-packages version",
            ] {
                let script = parser::parse_with_dialect(source, dialect);
                assert_eq!(
                    assess_with_context(&script, source, &[], &ctx).decision,
                    Decision::Allow,
                    "{shell:?}: {source}"
                );
            }
            let source = "package-tool --log command.log list";
            let script = parser::parse_with_dialect(source, dialect);
            let gate = assess_with_context(&script, source, &[], &ctx);
            assert_eq!(gate.decision, Decision::Confirm);
            assert!(
                gate.reasons
                    .iter()
                    .any(|reason| reason.contains("writes to a log file"))
            );
            assert_eq!(
                assess_replay_with_context(source, dialect, &ctx).decision,
                Decision::Confirm
            );
            for source in [
                "js-packages run build",
                "js-packages --prefix project publish",
                "js-packages ic",
                "js-runner unknown-package",
                "rust-packages build --bin example",
                "rust-packages test",
                "env X=1 rust-packages publish",
                "sudo -n rust-packages danger",
            ] {
                let script = parser::parse_with_dialect(source, dialect);
                assert_eq!(
                    assess_with_context(&script, source, &[], &ctx).decision,
                    Decision::Confirm,
                    "{shell:?}: {source}"
                );
                assert_eq!(
                    assess_replay_with_context(source, dialect, &ctx).decision,
                    Decision::Confirm,
                    "{shell:?}: {source}"
                );
            }
        }
        dir.script("node_modules/npm/package.json", r#"{"name":"unrelated"}"#);
        assert_eq!(
            super::super::native::package_manager_name(&npm),
            None,
            "a manifest change must not keep the old identity"
        );
    }

    fn check(original: &str, candidate: &str) -> SafetyAssessment {
        assess(&parser::parse(original), candidate, &[])
    }

    #[test]
    fn spelling_fixes_are_allowed() {
        for (original, candidate) in [
            ("git sttus", "git status"),
            ("sofka plugin lsit install", "sofka plugin list install"),
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
            (
                "sofka plugin instal example",
                "sofka plugin install example",
            ),
            (
                "gh extension upgarde example",
                "gh extension upgrade example",
            ),
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
    fn powershell_cmdlets_and_aliases_that_change_things_need_approval() {
        let dialect = parser::Dialect::PowerShell;
        let gate = |original: &str, candidate: &str| {
            assess(
                &parser::parse_with_dialect(original, dialect),
                candidate,
                &[],
            )
        };
        for (original, candidate, why) in [
            ("Remove-Itme ./build", "Remove-Item ./build", "deletes"),
            ("REMOVE-ITEM ./buidl", "REMOVE-ITEM ./build", "deletes"),
            ("rd ./buidl", "rd ./build", "deletes"),
            (
                "Microsoft.PowerShell.Management\\Remove-Item ./buidl",
                "Microsoft.PowerShell.Management\\Remove-Item ./build",
                "deletes",
            ),
            ("Stop-Proces -Name x", "Stop-Process -Name x", "stops"),
            ("Remove-AzVMx -Name x", "Remove-AzVM -Name x", "changes"),
            ("iexx ./script.ps1", "iex ./script.ps1", "runs code"),
            (
                "Copy-Item ./a ./b -Recurs",
                "Copy-Item ./a ./b -Recurse",
                "-Recurse",
            ),
            ("git clean -Forc", "git clean -Force", "-Force"),
            (
                "Set-Content ./notes.txt x",
                "Set-Content ./other.txt x",
                "now targets",
            ),
            ("git sttus > out.txt", "git status > other.txt", "writes to"),
            ("echo $(gti x)", "echo $(git x)", "substitution"),
        ] {
            let assessment = gate(original, candidate);
            assert_eq!(
                assessment.decision,
                Decision::Confirm,
                "{candidate}: {assessment:?}"
            );
            assert!(
                assessment.reasons.iter().any(|r| r.contains(why)),
                "{candidate}: {assessment:?}"
            );
        }
        for (original, candidate) in [
            ("git sttus", "git status"),
            ("Get-ChildItme", "Get-ChildItem"),
            // Reading cmdlets only widen what they read.
            (
                "Get-ChildItem -Recurs -Forc",
                "Get-ChildItem -Recurse -Force",
            ),
            ("gci -Recurs", "gci -Recurse"),
            ("echo a`nb", "echo a`nb"),
            (
                "git commit -m 'it''s fixed' --amnd",
                "git commit -m 'it''s fixed' --amend",
            ),
        ] {
            assert_eq!(
                gate(original, candidate).decision,
                Decision::Allow,
                "{candidate}"
            );
        }
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
