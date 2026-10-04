//! Completion functions registered by bash completion scripts (`complete -F
//! _brew brew`), for apps that ship no other completion protocol.
//!
//! The script is sourced in a fresh, non-interactive bash, and the function
//! it registers is called with `COMP_WORDS`/`COMP_CWORD` set; `COMPREPLY`
//! is the answer. The driver below is fixed text: the script path and the
//! command words are passed as arguments, never spliced into shell code.
//! Sourcing a script and calling its function run its code, so this is only
//! used for programs listed in `trusted_completers`.

use super::{CompletionError, CompletionItem, run_stdout};
use crate::engine::probe::Budget;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const DRIVER: &str = r#"source "$1" >/dev/null 2>&1 || exit 3
command=$2
shift 2
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
}

pub(super) fn complete(
    bash: &Path,
    script: &Path,
    name: &str,
    words: &[&str],
    prefix: &str,
    mut env: Vec<(OsString, Option<OsString>)>,
    budget: &mut Budget,
) -> Result<Vec<CompletionItem>, CompletionError> {
    // Noninteractive bash reads BASH_ENV even with --norc/--noprofile.
    env.extend(["BASH_ENV", "ENV"].map(|name| (name.into(), None)));
    let mut args: Vec<OsString> = vec![
        "--norc".into(),
        "--noprofile".into(),
        "-c".into(),
        DRIVER.into(),
        "notypo".into(),
        script.into(),
        name.into(),
    ];
    args.extend(words.iter().map(OsString::from));
    args.push(prefix.into());
    let text = run_stdout(bash, args, env, budget, true).map_err(|error| match error {
        CompletionError::Failed(why)
            if ["exited with Some(4)", "exited with Some(5)"].contains(&why.as_str()) =>
        {
            CompletionError::Unsupported(format!(
                "{} registers no usable completion function for {name}",
                script.display()
            ))
        }
        other => other,
    })?;
    Ok(super::normalize(
        text.lines(),
        super::Flavor::BashFunction,
        budget.max_candidates,
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::tests::Dir;
    use super::super::{Backend, Flavor, NativeCompletionBackend};
    use super::*;
    use std::time::Duration;

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
            Err(CompletionError::Unsupported(_))
        ));
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
        let result = complete(&bash, &script, "tool", &[], "", env, &mut budget).unwrap();
        assert_eq!(result.len(), 3);
        assert!(!marker.exists());
        budget.max_output = 4;
        assert!(
            matches!(complete(&bash, &script, "tool", &[], "", Vec::new(), &mut budget), Err(CompletionError::Failed(why)) if why.contains("probe limit"))
        );
    }
}
