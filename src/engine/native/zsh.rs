//! Installed zsh autoload handlers run inside a real completion widget.
//! `vared` edits a private variable; accepting that buffer never executes it.
//! Only an opted-in handler's matches are captured, over a separate descriptor.

use super::{CompletionError, CompletionItem, probe_error};
use crate::engine::parser::{self, Dialect};
use crate::engine::probe::{self, Budget, Capture, Probe};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const DRIVER: &str = r#"unsetopt monitor
unset HISTFILE
autoload -Uz +X compinit || exit 3
fpath=( ${1:h} $fpath )
compinit -u -D || exit 3
zmodload zsh/zutil || exit 3
typeset -g notypo_function=${1:t}
autoload -Uz -- "$1" || exit 3
typeset -ga notypo_matches=()
typeset -gi notypo_called=0 notypo_status=0
_notypo_handler() {
    local notypo_collect=1
    notypo_called=1
    "$notypo_function" "$@"
    notypo_status=$?
    return $notypo_status
}
compdef _notypo_handler "$2" || exit 3
compadd() {
    if (( ${notypo_collect:-0} )); then
        local -a notypo_added=()
        local -A notypo_flags=()
        local notypo_simple=1 notypo_flag
        zparseopts -E -A notypo_flags a k q Q f e n U l 1 2 C F: P: S: p: s: i: I: W: d: J: X: x: V: o:: r: R: D: O: A: E: M: || notypo_simple=0
        # Affixes need a richer replacement protocol.
        # Internal filtering calls must run once, since -D mutates arrays.
        for notypo_flag in -P -p -s -i -I -W -D -O -A; do
            [[ -n ${notypo_flags[$notypo_flag]-} ]] && notypo_simple=0
        done
        [[ -n $IPREFIX || -n $ISUFFIX ]] && notypo_simple=0
        [[ -n ${notypo_flags[-S]-} && ${notypo_flags[-S]} != ' ' ]] && notypo_simple=0
        if (( notypo_simple )); then
            builtin compadd -O notypo_added "$@"
            notypo_matches+=( "${notypo_added[@]}" )
        fi
    fi
    builtin compadd "$@"
}
_notypo_complete() {
    unset 'compstate[vared]'
    _main_complete
    compstate[list]=''
    compstate[insert]=''
}
zle -C notypo-complete complete-word _notypo_complete
zle-line-init() {
    zle notypo-complete
    zle accept-line
}
zle -N zle-line-init
bindkey -e
typeset notypo_buffer=$3
vared notypo_buffer || exit 4
(( notypo_called )) || exit 5
(( notypo_status <= 1 )) || exit 6
if (( ${#notypo_matches} )); then
    printf '%s\0' "${notypo_matches[@]}" >&8
fi
"#;

/// Autoload files named `_app`, plus aliases in installed `#compdef` headers.
/// The parent's fpath is data for discovery, never startup code to evaluate.
pub(super) fn script_for(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\']) {
        return None;
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("NOTYPO_ZSH_FPATH")
        .map(|paths| std::env::split_paths(&paths).collect())
        .unwrap_or_default();
    for prefix in ["/opt/homebrew", "/usr/local", "/usr"] {
        let root = PathBuf::from(prefix).join("share/zsh");
        dirs.push(root.join("site-functions"));
        if let Ok(entries) = std::fs::read_dir(&root) {
            let mut versions: Vec<_> = entries
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(char::is_numeric)
                })
                .map(|e| e.path().join("functions"))
                .collect();
            versions.sort();
            versions.reverse();
            dirs.extend(versions);
        }
    }
    for dir in dirs.into_iter().filter(|dir| dir.is_absolute()) {
        let direct = dir.join(format!("_{name}"));
        if direct.is_file() && declares(&direct, name) {
            return Some(direct);
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<_> = entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with('_'))
            .map(|e| e.path())
            .collect();
        files.sort();
        if let Some(path) = files.into_iter().find(|path| declares(path, name)) {
            return Some(path);
        }
    }
    None
}

fn declares(path: &Path, name: &str) -> bool {
    let text = super::head(path, 4096);
    let Some(header) = text.lines().next().and_then(|l| l.strip_prefix("#compdef")) else {
        return false;
    };
    // Pattern/widget registrations aren't ordinary command handlers.
    if !header.starts_with(char::is_whitespace) || header.trim_start().starts_with('-') {
        return false;
    }
    header
        .split_whitespace()
        .any(|word| word.split('=').next() == Some(name))
}

pub(super) fn complete(
    zsh: &Path,
    script: &Path,
    name: &str,
    words: &[&str],
    prefix: &str,
    mut env: Vec<(OsString, Option<OsString>)>,
    budget: &mut Budget,
) -> Result<Vec<CompletionItem>, CompletionError> {
    let mut line = parser::quote_word_with_dialect(name, Dialect::Posix);
    for word in words {
        line.push(' ');
        line.push_str(&parser::quote_word_with_dialect(word, Dialect::Posix));
    }
    line.push(' ');
    if !prefix.is_empty() {
        line.push_str(&parser::quote_word_with_dialect(prefix, Dialect::Posix));
    }
    env.extend([
        ("TERM".into(), Some("dumb".into())),
        ("FPATH".into(), None),
        ("fpath".into(), None),
        ("HISTFILE".into(), None),
    ]);
    let output = probe::run_completion_terminal(
        &Probe {
            program: zsh,
            args: vec![
                "-f".into(),
                "-i".into(),
                "-c".into(),
                DRIVER.into(),
                "notypo-completion".into(),
                script.into(),
                name.into(),
                line.into(),
            ],
            env,
            capture: Capture::Descriptor(8),
        },
        budget,
    )
    .map_err(probe_error)?;
    if output.status == Some(5) {
        return Err(CompletionError::Unsupported(
            "the zsh handler was not called at this position".into(),
        ));
    }
    if output.status != Some(0) {
        return Err(CompletionError::Failed(format!(
            "zsh completion exited with {:?}",
            output.status
        )));
    }
    if output.truncated {
        return Err(CompletionError::Failed(
            "completion output exceeded the probe limit".into(),
        ));
    }
    let text = String::from_utf8(output.data)
        .map_err(|_| CompletionError::Failed("completion output is not valid UTF-8".into()))?;
    let mut items: Vec<CompletionItem> = Vec::new();
    for value in text.split('\0') {
        if value.is_empty()
            || value.contains(char::is_control)
            || !value.starts_with(prefix)
            || items.iter().any(|i| i.value == value)
        {
            continue;
        }
        items.push(CompletionItem {
            value: value.to_owned(),
            takes_value: None,
            description: None,
        });
        if items.len() >= budget.max_candidates {
            break;
        }
    }
    Ok(items)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::engine::native::{Backend, Flavor, NativeCompletionBackend, tests::Dir};
    use std::time::Duration;

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(20), Duration::from_secs(3), 10)
    }

    fn values(items: Vec<CompletionItem>) -> Vec<String> {
        items.into_iter().map(|i| i.value).collect()
    }

    #[test]
    fn queries_real_zle_commands_options_values_and_newly_installed_commands() {
        let Some(zsh) = crate::utils::which("zsh") else {
            return;
        };
        let dir = Dir::new("zsh-completion");
        let script = dir.script("_tool", "#compdef tool\n_arguments '1:command:(build deploy)' '--target[Target]:target:(native wasm)' '--release[Release mode]'\n");
        let backend = Backend::new(Flavor::ZshFunction, "tool", zsh).with_helper(script.clone());
        let mut budget = budget();
        assert_eq!(
            values(backend.complete(&[], "", &mut budget).unwrap()),
            ["build", "deploy"]
        );
        assert_eq!(
            values(backend.complete(&["build"], "--", &mut budget).unwrap()),
            ["--target", "--release"]
        );
        assert_eq!(
            values(
                backend
                    .complete_values(&["build", "--target"], &mut budget)
                    .unwrap()
            ),
            ["native", "wasm"]
        );
        assert!(!backend.capabilities().complete_subcommands);
        let identity = backend.cache_identity().unwrap();
        std::fs::write(
            script,
            "#compdef tool\n_arguments '1:command:(build deploy newly-installed)'\n",
        )
        .unwrap();
        assert_ne!(identity, backend.cache_identity().unwrap());
        assert!(
            values(backend.complete(&[], "", &mut budget).unwrap())
                .contains(&"newly-installed".into())
        );
    }

    #[test]
    fn literal_context_and_results_cannot_execute_and_user_startup_is_skipped() {
        let Some(zsh) = crate::utils::which("zsh") else {
            return;
        };
        let dir = Dir::new("zsh-literal");
        dir.script(".zshenv", "print ran > $HOME/config-marker\n");
        let script = dir.script("_tool", "#compdef tool\n[[ ! -t 0 ]] || return 3\n[[ ${(Q)words[2]} == \"path with 'quote'\" ]] || return 1\nprintf 'callback chatter'\ncompadd -- 'value with spaces' 'build;touch marker' '$(touch marker)'\n");
        let backend = Backend::new(Flavor::ZshFunction, "tool", zsh).with_helper(script);
        let mut budget = budget();
        let env = vec![
            ("HOME".into(), Some(dir.0.clone().into_os_string())),
            ("ZDOTDIR".into(), Some(dir.0.clone().into_os_string())),
        ];
        let items = backend
            .query(&["path with 'quote'"], "", &mut budget, env.clone())
            .unwrap();
        assert_eq!(
            values(items),
            ["value with spaces", "build;touch marker", "$(touch marker)"]
        );
        assert!(
            backend
                .query(&["$(touch marker); x"], "$(touch marker)", &mut budget, env)
                .unwrap()
                .is_empty()
        );
        assert!(!dir.0.join("config-marker").exists());
        assert!(!dir.0.join(".zcompdump").exists());
        assert!(!Path::new("marker").exists());
    }

    #[test]
    fn failed_truncated_and_hanging_handlers_fail_closed() {
        let Some(zsh) = crate::utils::which("zsh") else {
            return;
        };
        let dir = Dir::new("zsh-failures");
        let script = dir.script("_tool", "#compdef tool\ncompadd -- build deploy\n");
        let backend = Backend::new(Flavor::ZshFunction, "tool", zsh.clone()).with_helper(script);
        let mut limits = budget();
        limits.max_output = 4;
        assert!(
            matches!(backend.complete(&[], "", &mut limits), Err(CompletionError::Failed(why)) if why.contains("probe limit"))
        );
        let script = dir.script("_broken", "#compdef tool\nexit 17\n");
        let backend = Backend::new(Flavor::ZshFunction, "tool", zsh.clone()).with_helper(script);
        assert!(matches!(
            backend.complete(&[], "", &mut budget()),
            Err(CompletionError::Failed(_))
        ));
        let script = dir.script("_invalid", "#compdef tool\ncompadd -- $'\\xff'\n");
        let backend = Backend::new(Flavor::ZshFunction, "tool", zsh.clone()).with_helper(script);
        assert!(
            matches!(backend.complete(&[], "", &mut budget()), Err(CompletionError::Failed(why)) if why.contains("UTF-8"))
        );
        let script = dir.script(
            "_hang",
            &format!(
                "#compdef tool\n(sleep 1; print ran > {}) &\nwait\n",
                crate::shlex::quote(&dir.0.join("ran").to_string_lossy())
            ),
        );
        let backend = Backend::new(Flavor::ZshFunction, "tool", zsh).with_helper(script);
        let mut limits = Budget::new(Duration::from_secs(1), Duration::from_millis(500), 1);
        assert!(
            matches!(backend.complete(&[], "", &mut limits), Err(CompletionError::Failed(why)) if why.contains("timed out"))
        );
        // A surviving background child would write after the probe returned.
        std::thread::sleep(Duration::from_millis(1100));
        assert!(!dir.0.join("ran").exists());
    }

    #[test]
    fn recognizes_command_headers_without_accepting_pattern_or_widget_registrations() {
        let dir = Dir::new("zsh-headers");
        for (header, matches) in [
            ("#compdef tool alias", true),
            ("#compdef\ttool", true),
            ("#compdeftool", false),
            ("#compdef tool=service", true),
            ("#compdef toolkit", false),
            ("#compdef -p tool*", false),
            ("#compdef -k tool complete-word", false),
            ("#autoload", false),
            ("#compdef tool\nprint tool", true),
        ] {
            let path = dir.script("_tool", header);
            assert_eq!(declares(&path, "tool"), matches, "{header}");
        }
    }

    #[test]
    fn preserves_service_aliases_declared_by_the_native_registration() {
        let Some(zsh) = crate::utils::which("zsh") else {
            return;
        };
        let dir = Dir::new("zsh-service");
        let script = dir.script("_shared", "#compdef tool=actual-service\n[[ $service == actual-service ]] || return 3\ncompadd -- build deploy\n");
        let backend = Backend::new(Flavor::ZshFunction, "tool", zsh).with_helper(script);
        assert_eq!(
            values(backend.complete(&[], "", &mut budget()).unwrap()),
            ["build", "deploy"]
        );
    }

    #[test]
    fn completion_affixes_and_ignored_prefixes_are_not_mistaken_for_whole_words() {
        let Some(zsh) = crate::utils::which("zsh") else {
            return;
        };
        let dir = Dir::new("zsh-affixes");
        for (body, prefix) in [
            ("compadd -P pre -S / -- fix", ""),
            ("IPREFIX=pre; PREFIX=''; compadd -- fix", "pre"),
            ("compadd -p pre -s / -- fix", "pre"),
        ] {
            let script = dir.script("_tool", &format!("#compdef tool\n{body}\n"));
            let backend =
                Backend::new(Flavor::ZshFunction, "tool", zsh.clone()).with_helper(script);
            assert!(
                backend
                    .complete(&[], prefix, &mut budget())
                    .unwrap()
                    .is_empty(),
                "{body}"
            );
        }
    }
}
