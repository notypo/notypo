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
COMP_CWORD=$4
COMP_LINE=$5
COMP_POINT=$6
shift 6
spec=$(complete -p "$command" 2>/dev/null) || exit 4
COMP_WORDS=("$command" "$@")
COMP_TYPE=9
COMP_KEY=9
# complete -p emits reusable, shell-quoted code. Only the registration is
# decoded; command words remain array elements and never enter eval text.
eval "notypo_spec=(${spec#complete })" || exit 5
notypo_spec=("${notypo_spec[@]:0:${#notypo_spec[@]}-1}")
notypo_words=("${COMP_WORDS[@]}")
notypo_cword=$COMP_CWORD notypo_line=$COMP_LINE notypo_point=$COMP_POINT
notypo_word=${COMP_WORDS[COMP_CWORD]} notypo_previous=${COMP_WORDS[COMP_CWORD-1]}
notypo_function= notypo_external=
for ((notypo_i=0; notypo_i<${#notypo_spec[@]}; notypo_i++)); do
    case ${notypo_spec[notypo_i]} in
        -F) notypo_function=${notypo_spec[notypo_i+1]}; notypo_spec[notypo_i+1]=_notypo_function;;
        -C) notypo_external=${notypo_spec[notypo_i+1]}; notypo_spec[notypo_i+1]=_notypo_external;;
    esac
done
_notypo_context() {
    COMP_WORDS=("${notypo_words[@]}")
    COMP_CWORD=$notypo_cword COMP_LINE=$notypo_line COMP_POINT=$notypo_point
    COMP_TYPE=9 COMP_KEY=9
}
_notypo_function() {
    _notypo_context
    COMPREPLY=()
    "$notypo_function" "$command" "$notypo_word" "$notypo_previous" >/dev/null 2>&1
    notypo_callback_status=$?
    if (( notypo_callback_status > 1 )); then
        COMPREPLY=()
        printf '\036notypo-completion-failed\n'
    fi
    return $notypo_callback_status
}
_notypo_external() {
    _notypo_context
    export COMP_LINE COMP_POINT COMP_TYPE COMP_KEY
    # Only the explicitly trusted registration supplies code here. These
    # variable references pass query words literally, including $() text.
    eval "$notypo_external \"\$command\" \"\$notypo_word\" \"\$notypo_previous\""
    notypo_callback_status=$?
    if (( notypo_callback_status > 1 )); then printf '\036notypo-completion-failed\n'; fi
    return $notypo_callback_status
}
# Bash applies -W/-G, -X filters and -P/-S affixes in its own implementation.
# https://www.gnu.org/software/bash/manual/html_node/Programmable-Completion.html
compgen "${notypo_spec[@]}" -- "$notypo_word"
notypo_result=$?
(( notypo_result <= 1 )) || exit 5
exit 0
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
                if !line.starts_with("complete")
                    || !["-F", "-C", "-W"].iter().any(|flag| line.contains(flag))
                {
                    continue;
                }
                if let Ok(words) = crate::shlex::split(line)
                    && words.first().is_some_and(|word| word == "complete")
                    && words
                        .iter()
                        .any(|word| matches!(word.as_ref(), "-F" | "-C" | "-W"))
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
                && words.windows(2).any(|pair| {
                    matches!(pair[0].as_ref(), "-F" | "-C" | "-W") && !pair[1].is_empty()
                })
                && words.last().is_some_and(|word| word == name)
        })
    })?;
    if functions.trim().is_empty()
        && crate::shlex::split(spec).is_ok_and(|words| words.iter().any(|word| word == "-F"))
    {
        return None;
    }
    Some(format!("{functions}\n{spec}\n"))
}

#[cfg(all(test, unix))]
pub(super) fn complete(
    bash: &Path,
    source: &Source<'_>,
    name: &str,
    words: &[&str],
    prefix: &str,
    env: Vec<(OsString, Option<OsString>)>,
    budget: &mut Budget,
) -> Result<Vec<CompletionItem>, CompletionError> {
    complete_at(bash, source, name, words, prefix, &[], env, budget)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn complete_at(
    bash: &Path,
    source: &Source<'_>,
    name: &str,
    words: &[&str],
    prefix: &str,
    following: &[&str],
    mut env: Vec<(OsString, Option<OsString>)>,
    budget: &mut Budget,
) -> Result<Vec<CompletionItem>, CompletionError> {
    // Noninteractive bash reads BASH_ENV even with --norc/--noprofile.
    env.extend(["BASH_ENV", "ENV"].map(|name| (name.into(), None)));
    let (mode, code, shown): (&str, OsString, String) = match source {
        Source::File(script) => ("file", script.into(), script.display().to_string()),
        Source::Text(text) => ("text", text.into(), "your bash session".into()),
    };
    let mut line = super::completion_line(name, words, prefix);
    let point = line.len();
    for word in following {
        line.push(' ');
        line.push_str(&crate::shlex::quote(word));
    }
    let mut args: Vec<OsString> = vec![
        "--norc".into(),
        "--noprofile".into(),
        "-c".into(),
        DRIVER.into(),
        "notypo".into(),
        mode.into(),
        code,
        name.into(),
        (words.len() + 1).to_string().into(),
        line.into(),
        point.to_string().into(),
    ];
    args.extend(words.iter().map(OsString::from));
    args.push(prefix.into());
    args.extend(following.iter().map(OsString::from));
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
    if text.contains("\u{1e}notypo-completion-failed") {
        return Err(CompletionError::Failed(
            "the registered bash completer failed".into(),
        ));
    }
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
    fn failed_callbacks_cannot_lend_their_partial_output_to_a_correction() {
        let Some(bash) = crate::utils::which("bash") else {
            return;
        };
        let dir = Dir::new("bash-callback-failure");
        let external = dir.script("completer", "#!/bin/sh\nprintf 'build\\n'\nexit 2\n");
        let scripts = [
            "_tool() { COMPREPLY=(build); return 2; }\ncomplete -F _tool tool\n".to_owned(),
            format!("complete -C '{}' -P prefix tool\n", external.display()),
        ];
        for text in scripts {
            let mut budget = Budget::new(Duration::from_secs(10), Duration::from_secs(5), 8);
            assert!(matches!(
                complete(
                    &bash,
                    &Source::Text(&text),
                    "tool",
                    &[],
                    "",
                    Vec::new(),
                    &mut budget
                ),
                Err(CompletionError::Failed(_))
            ));
        }
    }

    #[test]
    fn word_lists_and_external_completers_use_bash_filtering_and_literal_arguments() {
        let Some(bash) = crate::utils::which("bash") else {
            return;
        };
        let mut budget = Budget::new(Duration::from_secs(10), Duration::from_secs(5), 8);
        let text = "complete -W 'build deploy danger' -X 'danger' -P '--' tool\n";
        let items = complete(
            &bash,
            &Source::Text(text),
            "tool",
            &[],
            "",
            Vec::new(),
            &mut budget,
        )
        .unwrap();
        assert_eq!(
            items.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(),
            ["--build", "--deploy"]
        );
        let dir = Dir::new("bash-external");
        let external = dir.script("completer", "#!/bin/sh\n[ \"$1\" = tool ] && [ \"$2\" = '' ] && [ \"$3\" = tool ] && [ \"$COMP_LINE\" = 'tool  --scope west' ] && [ \"$COMP_POINT\" = 5 ] || exit 3\nprintf 'build\\ndeploy\\n'\n");
        let text = format!("complete -C '{}' tool\n", external.display());
        let items = complete_at(
            &bash,
            &Source::Text(&text),
            "tool",
            &[],
            "",
            &["--scope", "west"],
            Vec::new(),
            &mut budget,
        )
        .unwrap();
        assert_eq!(
            items.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(),
            ["build", "deploy"]
        );
        let snapshot = "complete -W 'build deploy' tool\n#notypo-functions\n";
        assert!(memory_script_in(snapshot, "tool").is_some());
    }

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
        assert_eq!(
            registering_script("words-only", &dirs),
            Some(dir.0.join("bash_completion.d/other"))
        );
        for name in ["gsutil-old", "complete", "missing"] {
            assert_eq!(registering_script(name, &dirs), None, "{name}");
        }
    }

    #[test]
    fn middle_slots_keep_later_words_and_a_byte_cursor_in_the_quoted_line() {
        let Some(bash) = crate::utils::which("bash") else {
            return;
        };
        let text = r#"_tool() {
            [[ $COMP_CWORD == 2 && ${COMP_WORDS[1]} == '🦀' && ${COMP_WORDS[2]} == '' && ${COMP_WORDS[3]} == --scope && ${COMP_WORDS[4]} == '三 west' ]] || return
            [[ $COMP_POINT == 12 && $COMP_LINE == "tool '🦀'  --scope '三 west'" ]] || return
            COMPREPLY=(build deploy)
        }
        complete -F _tool tool
        "#;
        let mut budget = Budget::new(Duration::from_secs(10), Duration::from_secs(5), 8);
        let items = complete_at(
            &bash,
            &Source::Text(text),
            "tool",
            &["🦀"],
            "",
            &["--scope", "三 west"],
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
        assert!(
            memory_script_in(text, "words")
                .unwrap()
                .contains("complete -W 'a b' words")
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
