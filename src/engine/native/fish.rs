//! Native fish completion scripts. The driver uses argument arrays and
//! `complete --do-complete`; it never evaluates the failed command. Loading
//! a script and its callbacks executes code, so discovery requires the
//! program to be listed in `trusted_completers`.
//!
//! Completions the user's session registered in memory (in `config.fish`,
//! or by `tool completion fish | source`) have no script. The shell
//! integration passes `complete --command` output for the previous
//! command's words and the user functions those lines call (two levels
//! deep, without fish's own helpers), which the driver loads.

use super::{CompletionError, CompletionItem, run_stdout};
use crate::engine::parser::{self, Dialect};
use crate::engine::probe::Budget;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const DRIVER: &str = r#"# Older fish releases store the standard helpers on disk; newer releases
# embed them. Avoid loading user configuration to initialize these paths.
if test -d $argv[4]
    set -g fish_function_path $argv[4]
end
switch $argv[5]
    case file
        source $argv[1] >/dev/null 2>&1; or exit 3
    case text
        printf '%s\n' $argv[1] | source >/dev/null 2>&1; or exit 3
    case '*'
        exit 3
end
set -l app $argv[2]
set -l line $argv[3]
if test (count (complete --command $app)) -eq 0
    exit 5
end
# Suppress default files at command positions. Parameter/file completers
# retain the behavior declared by the installed handler.
complete --command $app --no-files
complete --do-complete -- $line; or exit 4
printf '\x1f%s\n' (complete --command $app)
"#;

/// User/vendor scripts, in fish's documented search order. Explicit paths
/// exported by the parent shell take precedence over standard locations.
pub(super) fn script_for(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\']) {
        return None;
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("NOTYPO_FISH_COMPLETE_PATH")
        .map(|paths| std::env::split_paths(&paths).collect())
        .unwrap_or_default();
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::utils::expand_user("~/.config"));
    dirs.push(config.join("fish/completions"));
    for prefix in ["/opt/homebrew", "/usr/local", ""] {
        dirs.push(PathBuf::from(format!("{prefix}/etc/fish/completions")));
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::utils::expand_user("~/.local/share"));
    dirs.push(data.join("fish/vendor_completions.d"));
    for prefix in ["/opt/homebrew", "/usr/local", "/usr"] {
        dirs.push(PathBuf::from(format!(
            "{prefix}/share/fish/vendor_completions.d"
        )));
        dirs.push(PathBuf::from(format!("{prefix}/share/fish/completions")));
    }
    dirs.into_iter()
        .filter(|d| d.is_absolute())
        .map(|dir| dir.join(format!("{name}.fish")))
        .find(|path| path.is_file())
}

/// fish 4 embeds its own completion scripts instead of installing them.
/// Ask the installed fish for one (`status get-file`, without user
/// configuration) and keep it beside notypo's cache, per fish binary, so the
/// driver can load it like an installed script. Returns fish and the file.
pub(super) fn embedded(name: &str, budget: &mut Budget) -> Option<(PathBuf, PathBuf)> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._+-".contains(&c))
        || name.starts_with(['.', '-'])
    {
        return None;
    }
    let fish =
        crate::utils::which("fish").filter(|f| crate::engine::probe::is_trusted_location(f))?;
    let mut files = vec![fish.clone()];
    files.extend(std::fs::canonicalize(&fish).ok());
    let dir =
        crate::utils::cache_dir()
            .join("fish-embedded")
            .join(crate::engine::cache::fingerprint(
                &files,
                &["fish-embedded"],
            ));
    let script = dir.join(format!("{name}.fish"));
    let missing = dir.join(format!("{name}.missing"));
    if script.is_file() {
        return Some((fish, script));
    }
    if missing.is_file() {
        return None;
    }
    // Older fish releases have no embedded files and reject the subcommand.
    let text = run_stdout(
        &fish,
        vec![
            "--no-config".into(),
            "--private".into(),
            "-c".into(),
            "status get-file completions/$argv[1].fish".into(),
            name.into(),
        ],
        super::offline_env(super::Flavor::FishScript),
        budget,
        true,
    );
    std::fs::create_dir_all(&dir).ok()?;
    match text {
        Ok(text) if !text.trim().is_empty() => {
            let partial = dir.join(format!("{name}.fish.partial"));
            std::fs::write(&partial, text).ok()?;
            std::fs::rename(&partial, &script).ok()?;
            Some((fish, script))
        }
        Ok(_) | Err(CompletionError::Failed(_)) => {
            let _ = std::fs::write(&missing, "");
            None
        }
        Err(_) => None,
    }
}

/// Where the completions come from.
pub(super) enum Source<'a> {
    File(&'a Path),
    /// Registrations and definitions passed by the parent session.
    Text(&'a str),
}

/// The variable the fish integration fills: per command, a
/// `#notypo-command <word>` line and its `complete` lines; then a
/// `#notypo-functions` line and `functions` output.
const MEMORY: &str = "NOTYPO_FISH_COMPLETIONS";
const MEMORY_LIMIT: usize = 64 * 1024;

/// Code that registers the session's completions for `name`.
pub(super) fn memory_script(name: &str) -> Option<String> {
    memory_script_in(&std::env::var(MEMORY).ok()?, name)
}

fn memory_script_in(text: &str, name: &str) -> Option<String> {
    if text.len() > MEMORY_LIMIT || text.contains('\0') {
        return None;
    }
    let (specs, definitions) = text.split_once("#notypo-functions\n").unwrap_or((text, ""));
    let header = format!("#notypo-command {name}");
    let mut lines = specs.lines().skip_while(|line| *line != header).skip(1);
    let registrations: Vec<&str> = lines
        .by_ref()
        .take_while(|line| !line.starts_with("#notypo-command "))
        .collect();
    if registrations.is_empty()
        || !registrations
            .iter()
            .all(|line| line.starts_with("complete "))
    {
        return None;
    }
    Some(format!("{definitions}\n{}\n", registrations.join("\n")))
}

pub(super) fn complete(
    fish: &Path,
    source: &Source<'_>,
    name: &str,
    words: &[&str],
    prefix: &str,
    env: Vec<(OsString, Option<OsString>)>,
    budget: &mut Budget,
) -> Result<Vec<CompletionItem>, CompletionError> {
    let mut line = parser::quote_word_with_dialect(name, Dialect::Fish);
    for word in words {
        line.push(' ');
        line.push_str(&parser::quote_word_with_dialect(word, Dialect::Fish));
    }
    line.push(' ');
    if !prefix.is_empty() {
        line.push_str(&parser::quote_word_with_dialect(prefix, Dialect::Fish));
    }
    let functions = std::fs::canonicalize(fish)
        .unwrap_or_else(|_| fish.to_owned())
        .parent()
        .and_then(Path::parent)
        .map(|prefix| prefix.join("share/fish/functions"))
        .unwrap_or_default();
    let (mode, code, shown): (&str, OsString, String) = match source {
        Source::File(script) => ("file", script.into(), script.display().to_string()),
        Source::Text(text) => ("text", text.into(), "your fish session".into()),
    };
    let text = run_stdout(
        fish,
        vec![
            "--no-config".into(),
            "--private".into(),
            "-c".into(),
            DRIVER.into(),
            code,
            name.into(),
            line.into(),
            functions.into(),
            mode.into(),
        ],
        env,
        budget,
        true,
    )
    .map_err(|error| match error {
        CompletionError::Failed(why) if why == "exited with Some(5)" => {
            CompletionError::Unsupported(format!("{shown} registers no completion for {name}"))
        }
        other => other,
    })?;
    let mut arities: HashMap<String, Option<bool>> = HashMap::new();
    let mut raw = Vec::new();
    for line in text.lines() {
        if let Some(definition) = line.strip_prefix('\x1f') {
            let parsed = parser::parse_with_dialect(definition, Dialect::Fish);
            if !parsed.is_fully_supported() || parsed.commands.len() != 1 {
                continue;
            }
            let words: Vec<_> = parsed.commands[0]
                .words
                .iter()
                .filter_map(|w| w.literal())
                .collect();
            if words.first() != Some(&"complete") {
                continue;
            }
            let required = words
                .iter()
                .any(|w| ["--require-parameter", "--exclusive", "-r", "-x"].contains(w));
            for pair in words.windows(2) {
                let option = match pair[0] {
                    "-l" | "--long-option" => format!("--{}", pair[1]),
                    "-s" | "--short-option" | "-o" | "--old-option" => format!("-{}", pair[1]),
                    _ => continue,
                };
                arities
                    .entry(option)
                    .and_modify(|arity| {
                        if *arity != Some(required) {
                            *arity = None;
                        }
                    })
                    .or_insert(Some(required));
            }
        } else {
            raw.push(line.split_once('\t').unwrap_or((line, "")));
        }
    }
    let mut items = Vec::new();
    for (value, description) in raw {
        if value.is_empty()
            || value.contains(char::is_control)
            || !value.starts_with(prefix)
            || items
                .iter()
                .any(|item: &CompletionItem| item.value == value)
        {
            continue;
        }
        let takes_value = arities.get(value).copied().flatten();
        items.push(CompletionItem {
            value: value.to_owned(),
            takes_value,
            description: super::clean_description(description),
        });
        if items.len() > budget.max_candidates {
            return Err(super::over_limit());
        }
    }
    Ok(items)
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::{Backend, Flavor, NativeCompletionBackend, tests::Dir};
    use super::*;
    use std::time::Duration;

    #[test]
    fn session_completions_select_their_command_and_carry_helpers() {
        let text = "#notypo-command tool\ncomplete tool -l release\ncomplete --no-files tool -a 'build deploy' -n __tool_needs\n#notypo-command other\ncomplete other -l x\n#notypo-functions\nfunction __tool_needs\n return 0\nend\n";
        let script = memory_script_in(text, "tool").unwrap();
        assert!(script.starts_with("function __tool_needs"), "{script}");
        assert!(script.ends_with("-n __tool_needs\n"), "{script}");
        assert!(!script.contains("complete other"));
        assert_eq!(memory_script_in(text, "missing"), None);
        assert_eq!(
            memory_script_in(
                "#notypo-command tool\nrm -rf x\n#notypo-functions\n",
                "tool"
            ),
            None,
            "only complete lines"
        );
        let session = memory_script_in(text, "tool").unwrap();
        let Some(fish) = crate::utils::which("fish") else {
            return;
        };
        let mut budget = Budget::new(Duration::from_secs(20), Duration::from_secs(5), 8);
        let items = complete(
            &fish,
            &Source::Text(&session),
            "tool",
            &[],
            "",
            Vec::new(),
            &mut budget,
        )
        .unwrap();
        assert_eq!(
            items.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(),
            ["build", "deploy"]
        );
    }

    #[test]
    fn embedded_scripts_are_materialized_once_per_fish_binary() {
        let mut budget = Budget::new(Duration::from_secs(20), Duration::from_secs(5), 8);
        for name in ["", "../jq", "-x", ".hidden", "a b", "jq;touch"] {
            assert_eq!(embedded(name, &mut budget), None, "{name:?}");
        }
        assert_eq!(budget.spawned(), 0, "invalid names never start fish");
        let Some(fish) = crate::utils::which("fish") else {
            eprintln!("skipped: fish is not installed");
            return;
        };
        let supported = std::process::Command::new(&fish)
            .args(["--no-config", "-c", "status get-file completions/jq.fish"])
            .output()
            .is_ok_and(|output| output.status.success() && !output.stdout.is_empty());
        let missing = "notypo-no-such-tool";
        assert_eq!(embedded(missing, &mut budget), None);
        let spawned = budget.spawned();
        assert_eq!(embedded(missing, &mut budget), None);
        assert_eq!(budget.spawned(), spawned, "a missing script is remembered");
        if !supported {
            eprintln!("skipped: this fish embeds no completion files");
            return;
        }
        let (found, script) = embedded("jq", &mut budget).unwrap();
        assert_eq!(found, fish);
        assert!(
            std::fs::read_to_string(&script)
                .unwrap()
                .contains("complete -c jq")
        );
        let spawned = budget.spawned();
        assert_eq!(embedded("jq", &mut budget).unwrap().1, script);
        assert_eq!(budget.spawned(), spawned, "the file is reused");
    }

    #[test]
    fn queries_native_conditions_options_values_and_quoted_context() {
        let Some(fish) = crate::utils::which("fish") else {
            return;
        };
        let dir = Dir::new("fish-completion");
        let script = dir.script("tool.fish", r#"complete -c tool -f
complete -c tool -n 'not __fish_seen_subcommand_from build' -a 'build deploy'
complete -c tool -n '__fish_seen_subcommand_from build' -l target -x -a 'native wasm' -d 'Target architecture'
complete -c tool -n '__fish_seen_subcommand_from build' -l release -d 'Release build'
"#);
        let backend = Backend::new(Flavor::FishScript, "tool", fish).with_helper(script);
        let mut budget = Budget::new(Duration::from_secs(5), Duration::from_secs(2), 8);
        let names =
            |items: Vec<CompletionItem>| items.into_iter().map(|i| i.value).collect::<Vec<_>>();
        assert_eq!(
            names(backend.complete(&[], "", &mut budget).unwrap()),
            ["build", "deploy"]
        );
        let options = backend
            .complete(&["build", "path with 'quote'"], "--", &mut budget)
            .unwrap();
        assert_eq!(
            options
                .iter()
                .find(|i| i.value == "--target")
                .unwrap()
                .takes_value,
            Some(true)
        );
        let release = options.iter().find(|i| i.value == "--release").unwrap();
        assert_eq!(release.takes_value, Some(false));
        assert_eq!(release.description.as_deref(), Some("Release build"));
        assert_eq!(
            names(
                backend
                    .complete_values(&["build", "--target"], &mut budget)
                    .unwrap()
            ),
            ["native", "wasm"]
        );
    }

    #[test]
    fn preserves_literal_candidates_and_rejects_failed_or_truncated_scripts() {
        let Some(fish) = crate::utils::which("fish") else {
            return;
        };
        let dir = Dir::new("fish-untrusted-output");
        let script = dir.script(
            "tool.fish",
            "complete -c tool -f -a \"'build;echo_marker'\"\n",
        );
        let backend = Backend::new(Flavor::FishScript, "tool", fish.clone()).with_helper(script);
        let mut budget = Budget::new(Duration::from_secs(10), Duration::from_secs(3), 8);
        let items = backend.complete(&[], "", &mut budget).unwrap();
        assert_eq!(items[0].value, "build;echo_marker");
        budget.max_output = 4;
        assert!(
            matches!(backend.complete(&[], "", &mut budget), Err(CompletionError::Failed(why)) if why.contains("probe limit"))
        );
        let broken = dir.script("broken.fish", "exit 17\n");
        let backend = Backend::new(Flavor::FishScript, "tool", fish.clone()).with_helper(broken);
        assert!(matches!(
            backend.complete(&[], "", &mut budget),
            Err(CompletionError::Failed(_))
        ));
        let missing = dir.script("missing.fish", "true\n");
        let backend = Backend::new(Flavor::FishScript, "tool", fish).with_helper(missing);
        assert!(matches!(
            backend.complete(&[], "", &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
    }
}
