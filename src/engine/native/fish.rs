//! Native fish completion scripts. The driver uses argument arrays and
//! `complete --do-complete`; it never evaluates the failed command. Loading
//! a script and its callbacks executes code, so discovery requires the
//! program to be listed in `trusted_completers`.

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
source $argv[1] >/dev/null 2>&1; or exit 3
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

pub(super) fn complete(
    fish: &Path,
    script: &Path,
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
    let text = run_stdout(
        fish,
        vec![
            "--no-config".into(),
            "--private".into(),
            "-c".into(),
            DRIVER.into(),
            script.into(),
            name.into(),
            line.into(),
            functions.into(),
        ],
        env,
        budget,
        true,
    )
    .map_err(|error| match error {
        CompletionError::Failed(why) if why == "exited with Some(5)" => {
            CompletionError::Unsupported(format!(
                "{} registers no completion for {name}",
                script.display()
            ))
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
        if items.len() >= budget.max_candidates {
            break;
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
