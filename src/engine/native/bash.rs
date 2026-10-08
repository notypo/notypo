//! Completion functions registered by bash completion scripts (`complete -F
//! _brew brew`), for apps that ship no other completion protocol.
//!
//! The script is sourced in a fresh, non-interactive bash, and the function
//! it registers is called with `COMP_WORDS`/`COMP_CWORD` set; `COMPREPLY`
//! is the answer. The driver below is fixed text: the script path and the
//! command words are passed as arguments, never spliced into shell code.
//! Sourcing a script and calling its function run its code, so this is only
//! used for programs listed in `trusted_completers`.
//!
//! A function the user's bash session defined in memory (inline in an rc
//! file, or by `eval "$(tool completion bash)"`) has no file to source. The
//! shell integration passes its `complete -p` registration and the
//! definitions of the functions from the same file (`extdebug` names it),
//! which the driver evaluates; the rc file itself is never run.

use super::{CompletionError, CompletionItem, run_stdout};
use crate::engine::probe::Budget;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const DRIVER: &str = r#"case $1 in
  file) source "$2" >/dev/null 2>&1 || exit 3;;
  text) eval "$2" >/dev/null 2>&1 || exit 3;;
  *) exit 3;;
esac
command=$3
shift 3
spec=$(complete -p "$command" 2>/dev/null) || exit 4
[[ $spec =~ -F\ ([^ ]+) ]] || exit 5
function=${BASH_REMATCH[1]}
COMP_WORDS=("$command" "$@")
COMP_CWORD=$((${#COMP_WORDS[@]} - 1))
COMP_LINE="${COMP_WORDS[*]}"
COMP_POINT=${#COMP_LINE}
COMP_TYPE=9
COMP_KEY=9
COMPREPLY=()
"$function" "$command" "${COMP_WORDS[COMP_CWORD]}" "${COMP_WORDS[COMP_CWORD-1]}" >/dev/null 2>&1
printf '%s\n' "${COMPREPLY[@]}"
"#;

/// The bash completion script installed for `name`, from the usual
/// bash-completion locations.
pub(super) fn script_for(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(user) = std::env::var_os("BASH_COMPLETION_USER_DIR") {
        dirs.push(PathBuf::from(user).join("completions"));
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::utils::expand_user("~/.local/share"));
    dirs.push(data.join("bash-completion/completions"));
    for prefix in ["/opt/homebrew", "/usr/local", "/usr", ""] {
        dirs.push(PathBuf::from(format!(
            "{prefix}/share/bash-completion/completions"
        )));
        dirs.push(PathBuf::from(format!("{prefix}/etc/bash_completion.d")));
    }
    dirs.iter()
        .flat_map(|dir| {
            [name.to_owned(), format!("{name}.bash"), format!("_{name}")].map(|file| dir.join(file))
        })
        .find(|path| path.is_file())
        .or_else(|| {
            let compat: Vec<PathBuf> = ["/opt/homebrew", "/usr/local", "/usr", ""]
                .iter()
                .map(|prefix| PathBuf::from(format!("{prefix}/etc/bash_completion.d")))
                .collect();
            registering_script(name, &compat)
        })
}

/// bash-completion sources every file in `bash_completion.d` at startup,
/// and one file may register several commands (the Cloud SDK's registers
/// gcloud, bq, and gsutil). Find a file whose text registers `name` with a
/// function. Files are only read here; nothing is sourced. The directories
/// are indexed once per process: discovery asks about every candidate
/// program, and Homebrew's directory alone can hold dozens of scripts.
fn registering_script(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    type Index = HashMap<String, PathBuf>;
    static INDEXES: OnceLock<Mutex<HashMap<Vec<PathBuf>, Index>>> = OnceLock::new();
    let mut indexes = INDEXES.get_or_init(Mutex::default).lock().ok()?;
    let index = indexes
        .entry(dirs.to_vec())
        .or_insert_with(|| index_registrations(dirs));
    index.get(name).cloned()
}

/// The first file (by directory, then name) registering each command as the
/// last word of a `complete ... -F function ...` line.
fn index_registrations(dirs: &[PathBuf]) -> HashMap<String, PathBuf> {
    let mut index: HashMap<String, PathBuf> = HashMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .take(512)
            .collect();
        paths.sort();
        for path in paths {
            if !path.is_file() || path.metadata().map_or(true, |m| m.len() > 1024 * 1024) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in text.lines().map(str::trim) {
                // Only registrations are worth splitting.
                if !line.starts_with("complete") || !line.contains("-F") {
                    continue;
                }
                if let Ok(words) = crate::shlex::split(line)
                    && words.first().is_some_and(|word| word == "complete")
                    && words.iter().any(|word| word == "-F")
                    && let Some(name) = words.last()
                {
                    index
                        .entry(name.to_string())
                        .or_insert_with(|| path.clone());
                }
            }
        }
    }
    index
}

/// Where the registering code comes from.
pub(super) enum Source<'a> {
    File(&'a Path),
    /// Definitions and a registration passed by the parent session.
    Text(&'a str),
}

/// The variable the bash integration fills: `complete -p` lines for the
/// previous commands' words, a `#notypo-functions` line, then `declare -f`
/// output for the functions defined in the same files.
const MEMORY: &str = "NOTYPO_BASH_COMPLETIONS";
const SEPARATOR: &str = "#notypo-functions\n";
const MEMORY_LIMIT: usize = 64 * 1024;

/// Code that registers the session's completion function for `name`: the
/// session's function definitions and the one `complete -F` line naming it.
pub(super) fn memory_script(name: &str) -> Option<String> {
    let text = std::env::var(MEMORY).ok()?;
    memory_script_in(&text, name)
}

fn memory_script_in(text: &str, name: &str) -> Option<String> {
    if text.len() > MEMORY_LIMIT || text.contains('\0') {
        return None;
    }
    let (specs, functions) = text.split_once(SEPARATOR)?;
    let spec = specs.lines().find(|line| {
        crate::shlex::split(line).is_ok_and(|words| {
            words.first().is_some_and(|word| word == "complete")
                && words
                    .windows(2)
                    .any(|pair| pair[0] == "-F" && !pair[1].is_empty())
                && words.last().is_some_and(|word| word == name)
        })
    })?;
    if functions.trim().is_empty() {
        return None;
    }
    Some(format!("{functions}\n{spec}\n"))
}

pub(super) fn complete(
    bash: &Path,
    source: &Source<'_>,
    name: &str,
    words: &[&str],
    prefix: &str,
    mut env: Vec<(OsString, Option<OsString>)>,
    budget: &mut Budget,
) -> Result<Vec<CompletionItem>, CompletionError> {
    // Noninteractive bash reads BASH_ENV even with --norc/--noprofile.
    env.extend(["BASH_ENV", "ENV"].map(|name| (name.into(), None)));
    let (mode, code, shown): (&str, OsString, String) = match source {
        Source::File(script) => ("file", script.into(), script.display().to_string()),
        Source::Text(text) => ("text", text.into(), "your bash session".into()),
    };
    let mut args: Vec<OsString> = vec![
        "--norc".into(),
        "--noprofile".into(),
        "-c".into(),
        DRIVER.into(),
        "notypo".into(),
        mode.into(),
        code,
        name.into(),
    ];
    args.extend(words.iter().map(OsString::from));
    args.push(prefix.into());
    // A script that registers nothing (yadm's needs git's handler loaded
    // first) is broken, not a statement that nothing here is a command:
    // documentation still applies.
    let text = run_stdout(bash, args, env, budget, true).map_err(|error| match error {
        CompletionError::Failed(why)
            if ["exited with Some(4)", "exited with Some(5)"].contains(&why.as_str()) =>
        {
            CompletionError::Failed(format!(
                "{shown} registers no usable completion function for {name}"
            ))
        }
        other => other,
    })?;
    super::normalize_bounded(
        text.lines(),
        super::Flavor::BashFunction,
        budget.max_candidates,
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::tests::Dir;
    use super::super::{Backend, Flavor, NativeCompletionBackend};
    use super::*;
    use std::time::Duration;

    #[test]
    fn shared_compatibility_files_are_found_by_their_registrations() {
        let dir = Dir::new("bash-compat");
        let sdk = dir.script(
            "bash_completion.d/google-cloud-sdk",
            "_python_argcomplete() { :; }\ncomplete -o nospace -F _python_argcomplete \"gcloud\"\n_bq_completer() { :; }\ncomplete -F _bq_completer bq\ncomplete -o nospace -F _python_argcomplete gsutil\n# complete -F _old gsutil-old\n",
        );
        dir.script("bash_completion.d/other", "complete -W 'a b' words-only\n");
        let dirs = [dir.0.join("missing"), dir.0.join("bash_completion.d")];
        for name in ["gcloud", "bq", "gsutil"] {
            assert_eq!(registering_script(name, &dirs), Some(sdk.clone()), "{name}");
        }
        for name in ["gsutil-old", "words-only", "complete", "missing"] {
            assert_eq!(registering_script(name, &dirs), None, "{name}");
        }
    }

    #[test]
    fn session_registrations_select_their_command_and_carry_its_definitions() {
        let text = "complete -o default -F _tool tool\ncomplete -F _other 'other tool'\ncomplete -W 'a b' words\n#notypo-functions\n_tool () \n{ \n    COMPREPLY=(build)\n}\n";
        let script = memory_script_in(text, "tool").unwrap();
        assert!(script.starts_with("_tool () "), "{script}");
        assert!(
            script.ends_with("complete -o default -F _tool tool\n"),
            "{script}"
        );
        assert_eq!(
            memory_script_in(text, "other"),
            None,
            "names must match exactly"
        );
        assert_eq!(
            memory_script_in(text, "words"),
            None,
            "only -F registrations"
        );
        assert_eq!(memory_script_in(text, "missing"), None);
        assert_eq!(
            memory_script_in("complete -F _tool tool\n#notypo-functions\n", "tool"),
            None,
            "no definitions"
        );
        assert_eq!(memory_script_in("complete -F _tool tool\n", "tool"), None);
        let huge = format!("{text}{}", "#".repeat(MEMORY_LIMIT));
        assert_eq!(memory_script_in(&huge, "tool"), None);
    }

    #[test]
    fn evaluates_session_definitions_without_a_file() {
        let Some(bash) = crate::utils::which("bash") else {
            return;
        };
        let script = "_tool () \n{ \n    case \"${COMP_WORDS[*]}\" in \"tool \") COMPREPLY=(build deploy);; esac\n}\ncomplete -F _tool tool\n";
        let mut budget = Budget::new(Duration::from_secs(10), Duration::from_secs(5), 8);
        let words = complete(
            &bash,
            &Source::Text(script),
            "tool",
            &[],
            "",
            Vec::new(),
            &mut budget,
        )
        .unwrap();
        assert_eq!(
            words.iter().map(|w| w.value.as_str()).collect::<Vec<_>>(),
            ["build", "deploy"]
        );
        assert!(matches!(
            complete(&bash, &Source::Text("true\n"), "tool", &[], "", Vec::new(), &mut budget),
            Err(CompletionError::Failed(why)) if why.contains("your bash session")
        ));
    }

    #[test]
    fn calls_the_registered_function_with_the_command_words() {
        let Some(bash) = crate::utils::which("bash") else {
            return;
        };
        let dir = Dir::new("bash");
        let script = dir.script(
            "completions/tool",
            r#"_tool() {
  case "${COMP_WORDS[*]}" in
    "tool ") COMPREPLY=(build deploy "with space");;
    "tool build --") COMPREPLY=(--release --target=);;
    *) touch "$NOTYPO_MARK";;
  esac
}
complete -F _tool tool
"#,
        );
        let backend = Backend::new(Flavor::BashFunction, "tool", bash).with_helper(script);
        let mut budget = Budget::new(Duration::from_secs(10), Duration::from_secs(5), 8);
        let words: Vec<String> = backend
            .complete(&[], "", &mut budget)
            .unwrap()
            .into_iter()
            .map(|i| i.value)
            .collect();
        assert_eq!(words, ["build", "deploy"], "words with spaces are dropped");
        let options = backend.complete(&["build"], "--", &mut budget).unwrap();
        assert_eq!(options[1].value, "--target");
        assert_eq!(options[1].takes_value, Some(true));
        let broken = dir.script("completions/other", "true\n");
        let backend = Backend::new(
            Flavor::BashFunction,
            "other",
            crate::utils::which("bash").unwrap(),
        )
        .with_helper(broken);
        assert!(matches!(
            backend.complete(&[], "", &mut budget),
            Err(CompletionError::Failed(_))
        ));
    }

    /// borg's handlers list archives with `borg list`, which would lock
    /// the repository and could run BORG_PASSCOMMAND or ssh.
    #[test]
    fn borg_handlers_call_a_stub_instead_of_borg() {
        let Some(bash) = crate::utils::which("bash") else {
            return;
        };
        let dir = Dir::new("bash-borg");
        let marker = dir.0.join("resolved");
        let script = dir.script(
            "completions/borg",
            &format!(
                "_borg() {{\n  COMPREPLY=(create delete list)\n  type -P borg > {}\n  borg list repo:: && COMPREPLY=(opened)\n}}\ncomplete -F _borg borg\n",
                crate::shlex::quote(marker.to_str().unwrap())
            ),
        );
        let backend = Backend::new(Flavor::BashFunction, "borg", bash).with_helper(script);
        let mut budget = Budget::new(Duration::from_secs(10), Duration::from_secs(5), 8);
        let words: Vec<String> = backend
            .complete(&["delete", "repo::a1"], "", &mut budget)
            .unwrap()
            .into_iter()
            .map(|i| i.value)
            .collect();
        assert_eq!(words, ["create", "delete", "list"]);
        let resolved = std::fs::read_to_string(&marker).unwrap();
        assert!(
            Path::new(resolved.trim()).starts_with(crate::utils::cache_dir().join("stubs/borg")),
            "{resolved}"
        );
    }

    #[test]
    fn never_loads_startup_configuration_and_reports_output_limits() {
        let Some(bash) = crate::utils::which("bash") else {
            return;
        };
        let dir = Dir::new("bash-startup");
        let marker = dir.0.join("startup-marker");
        let config = dir.script(
            "config",
            &format!(
                "printf loaded > {}\n",
                crate::shlex::quote(marker.to_str().unwrap())
            ),
        );
        let script = dir.script(
            "tool",
            "_tool() { COMPREPLY=(build release deploy); }\ncomplete -F _tool tool\n",
        );
        let mut budget = Budget::new(Duration::from_secs(10), Duration::from_secs(3), 4);
        let env = vec![("BASH_ENV".into(), Some(config.into()))];
        let result = complete(
            &bash,
            &Source::File(&script),
            "tool",
            &[],
            "",
            env,
            &mut budget,
        )
        .unwrap();
        assert_eq!(result.len(), 3);
        assert!(!marker.exists());
        budget.max_output = 4;
        assert!(
            matches!(complete(&bash, &Source::File(&script), "tool", &[], "", Vec::new(), &mut budget), Err(CompletionError::Failed(why)) if why.contains("probe limit"))
        );
    }
}
