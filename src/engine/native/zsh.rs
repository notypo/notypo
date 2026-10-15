//! Installed zsh autoload handlers run inside a real completion widget.
//! `vared` edits a private variable; accepting that buffer never executes it.
//! Only an opted-in handler's matches are captured, over a separate descriptor.
//!
//! A handler the user's session defined in memory (inline in `.zshrc`, or
//! by `source <(tool completion zsh)`) has no autoload file. The shell
//! integration passes its `compdef` registration and the definitions of the
//! functions from the same source (`$functions_source`), which the driver
//! evaluates; the session's startup files are never run.

use super::{CompletionError, CompletionItem, probe_error};
use crate::engine::parser::{self, Dialect};
use crate::engine::probe::{self, Budget, Capture, Probe};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const DRIVER: &str = r#"unsetopt monitor
unset HISTFILE
autoload -Uz +X compinit || exit 3
[[ $1 == file ]] && fpath=( ${2:h} $fpath )
compinit -u -D || exit 3
zmodload zsh/zutil || exit 3
if [[ $1 == file ]]; then
    typeset -g notypo_function=${2:t}
    autoload -Uz +X -- "$2" || exit 3
elif [[ $1 == text ]]; then
    eval "$2" >/dev/null 2>&1 || exit 3
    typeset -g notypo_function=$3
    (( $+functions[$notypo_function] )) || exit 3
else
    exit 3
fi
typeset -ga notypo_matches=()
typeset -gi notypo_called=0 notypo_status=0
_notypo_handler() {
    local notypo_collect=1 notypo_before=${functions[$notypo_function]-}
    notypo_called=1
    "$notypo_function" "$@"
    notypo_status=$?
    # A file that only defines its handler (bitwarden's `_bw`) defines it on
    # the first Tab and completes on the second; so does this.
    if (( ! ${#notypo_matches} )) && [[ ${functions[$notypo_function]-} != $notypo_before ]]; then
        "$notypo_function" "$@"
        notypo_status=$?
    fi
    return $notypo_status
}
compdef _notypo_handler "$4" || exit 3
compadd() {
    if (( ${notypo_collect:-0} )); then
        local -a notypo_added=() notypo_display=() notypo_raw=()
        local -A notypo_flags=()
        local notypo_simple=1 notypo_flag notypo_spec notypo_value notypo_suffix notypo_hidden_prefix notypo_hidden_suffix
        local -i notypo_index=0
        zparseopts -E -A notypo_flags a k q Q f e n U l 1 2 C F: P: S: p: s: i: I: W: d: J: X: x: V: o:: r: R: D: O: A: E: M: || notypo_simple=0
        # Internal filtering calls mutate caller arrays and add no matches.
        for notypo_flag in -D -O -A -C; do
            (( ${+notypo_flags[$notypo_flag]} )) && notypo_simple=0
        done
        if (( notypo_simple )); then
            notypo_spec=${notypo_flags[-d]-}
            if [[ $notypo_spec =~ '^[a-zA-Z_][a-zA-Z0-9_]*$' ]]; then
                notypo_display=( "${(@P)notypo_spec}" )
            elif [[ $notypo_spec == \(*\) ]]; then
                notypo_display=( ${(Q)${(z)notypo_spec[2,-2]}} )
            fi
            # -D retains the display entry corresponding to each match;
            # -A includes transformations from native matching controls.
            (( ${#notypo_display} )) && builtin compadd -D notypo_display "$@"
            builtin compadd -A notypo_added "$@"
            builtin compadd -O notypo_raw "$@"
            for notypo_value in "${notypo_added[@]}"; do
                (( ++notypo_index ))
                notypo_suffix=${notypo_flags[-S]-}
                [[ $notypo_suffix == ' ' ]] && notypo_suffix=''
                notypo_hidden_prefix=${notypo_flags[-p]-}
                notypo_hidden_suffix=${notypo_flags[-s]-}
                notypo_value="${(q)notypo_hidden_prefix}${notypo_value}${(q)notypo_hidden_suffix}"
                # Equals marks an option name, while non-option keys retain it.
                [[ $notypo_suffix == '=' && ${notypo_flags[-P]-}${notypo_hidden_prefix}${notypo_raw[notypo_index]}${notypo_hidden_suffix} == [-+]?* ]] && notypo_suffix=''
                # -A already quotes the body as Zsh would insert it.
                # Visible affixes are shell text; Rust accepts one literal word.
                notypo_value="${notypo_flags[-P]-}${notypo_value}${notypo_suffix}${ISUFFIX}${notypo_flags[-I]-}"
                if (( ${+notypo_flags[-U]} )); then
                    notypo_value="${notypo_flags[-i]-}${notypo_value}"
                else
                    notypo_value="${IPREFIX}${notypo_flags[-i]-}${notypo_value}"
                fi
                notypo_matches+=( "$notypo_value" "${notypo_display[notypo_index]-}" "${notypo_raw[notypo_index]-}" )
            done
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
    CURSOR=$notypo_cursor
    zle notypo-complete
    zle accept-line
}
zle -N zle-line-init
bindkey -e
typeset notypo_buffer=$5
typeset -gi notypo_cursor=$6
vared notypo_buffer || exit 4
(( notypo_called )) || exit 5
(( notypo_status <= 1 )) || exit 6
printf '#notypo-zsh-v2\0' >&8
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

/// Where the handler comes from.
pub(super) enum Source<'a> {
    File(&'a Path),
    /// Definitions passed by the parent session, and the handler's name.
    Text {
        definitions: &'a str,
        function: &'a str,
    },
}

/// The variable the zsh integration fills: `compdef <function> <word>`
/// lines, a `#notypo-functions` line, then `functions` output.
const MEMORY: &str = "NOTYPO_ZSH_COMPLETIONS";
const SEPARATOR: &str = "#notypo-functions\n";
const MEMORY_LIMIT: usize = 64 * 1024;

/// The session's handler for `name`: its definitions and function name.
pub(super) fn memory(name: &str) -> Option<(String, String)> {
    memory_in(&std::env::var(MEMORY).ok()?, name)
}

fn memory_in(text: &str, name: &str) -> Option<(String, String)> {
    if text.len() > MEMORY_LIMIT || text.contains('\0') {
        return None;
    }
    let (specs, definitions) = text.split_once(SEPARATOR)?;
    let function = specs.lines().find_map(|line| {
        let mut words = line.split(' ');
        match (words.next(), words.next(), words.next(), words.next()) {
            (Some("compdef"), Some(function), Some(word), None) if word == name => Some(function),
            _ => None,
        }
    })?;
    let valid = |c: char| c.is_ascii_alphanumeric() || "_-:.".contains(c);
    if function.is_empty() || !function.chars().all(valid) || definitions.trim().is_empty() {
        return None;
    }
    Some((definitions.to_owned(), function.to_owned()))
}

#[cfg(all(test, unix))]
pub(super) fn complete(
    zsh: &Path,
    source: &Source<'_>,
    name: &str,
    words: &[&str],
    prefix: &str,
    env: Vec<(OsString, Option<OsString>)>,
    budget: &mut Budget,
) -> Result<Vec<CompletionItem>, CompletionError> {
    complete_at(zsh, source, name, words, prefix, &[], env, budget)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn complete_at(
    zsh: &Path,
    source: &Source<'_>,
    name: &str,
    words: &[&str],
    prefix: &str,
    following: &[&str],
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
    let cursor = line.chars().count();
    for word in following {
        line.push(' ');
        line.push_str(&parser::quote_word_with_dialect(word, Dialect::Posix));
    }
    env.extend([
        ("TERM".into(), Some("dumb".into())),
        ("FPATH".into(), None),
        ("fpath".into(), None),
        ("HISTFILE".into(), None),
    ]);
    let (mode, code, function): (&str, OsString, &str) = match source {
        Source::File(script) => ("file", script.into(), ""),
        Source::Text {
            definitions,
            function,
        } => ("text", definitions.into(), function),
    };
    let output = probe::run_completion_terminal(
        &Probe {
            program: zsh,
            args: vec![
                "-f".into(),
                "-i".into(),
                "-c".into(),
                DRIVER.into(),
                "notypo-completion".into(),
                mode.into(),
                code,
                function.into(),
                name.into(),
                line.into(),
                cursor.to_string().into(),
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
    parse(&text, prefix, budget.max_candidates)
}

fn parse(text: &str, prefix: &str, limit: usize) -> Result<Vec<CompletionItem>, CompletionError> {
    let Some(records) = text.strip_prefix("#notypo-zsh-v2\0") else {
        return Err(CompletionError::Failed(
            "unexpected Zsh completion response".into(),
        ));
    };
    if !records.is_empty() && !records.ends_with('\0') {
        return Err(CompletionError::Failed(
            "incomplete Zsh completion response".into(),
        ));
    }
    let fields: Vec<&str> = records.split_terminator('\0').collect();
    let (records, trailing) = fields.as_chunks::<3>();
    if !trailing.is_empty() {
        return Err(CompletionError::Failed(
            "incomplete Zsh completion record".into(),
        ));
    }
    let mut items: Vec<CompletionItem> = Vec::new();
    for [insertion, description, _raw] in records {
        let source = format!("__notypo {insertion}");
        let parsed = parser::parse_with_dialect(&source, Dialect::Posix);
        if !parsed.is_fully_supported() || parsed.commands.len() != 1 {
            continue;
        }
        let command = &parsed.commands[0];
        if command.words.len() != 2 || !command.redirections.is_empty() {
            continue;
        }
        let Some(value) = command.words[1].literal() else {
            continue;
        };
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
            description: super::clean_description(description).filter(|text| text != value),
        });
        if items.len() > limit {
            return Err(super::over_limit());
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
    fn middle_slots_keep_the_later_arguments_in_real_zle() {
        let Some(zsh) = crate::utils::which("zsh") else {
            return;
        };
        let dir = Dir::new("zsh-middle");
        let script = dir.script("_tool", "#compdef tool\n[[ $CURRENT == 3 && ${(Q)words[2]} == '🦀' && $words[4] == --scope && ${(Q)words[5]} == '三 west' ]] && compadd build deploy\n");
        let items = complete_at(
            &zsh,
            &Source::File(&script),
            "tool",
            &["🦀"],
            "",
            &["--scope", "三 west"],
            Vec::new(),
            &mut budget(),
        )
        .unwrap();
        assert_eq!(values(items), ["build", "deploy"]);
    }

    #[test]
    fn rich_affixes_descriptions_arrays_and_quoting_survive_real_zle() {
        let Some(zsh) = crate::utils::which("zsh") else {
            return;
        };
        let dir = Dir::new("zsh-rich");
        let script = dir.script("_tool", "#compdef tool\nlocal -a choices=('build' 'two words') descriptions=('Compile it' 'A spaced value')\ncompadd -d descriptions -a choices\ncompadd -P pre- -p hidden- -s -inside -S -post -i ignored- -I -tail -- body\ncompadd -Q -- \"'quoted value'\"\ncompadd -P '$(' -S ')' -- 'touch marker'\n");
        let items = complete(
            &zsh,
            &Source::File(&script),
            "tool",
            &[],
            "",
            Vec::new(),
            &mut budget(),
        )
        .unwrap();
        let actual: Vec<_> = items
            .iter()
            .map(|item| (item.value.as_str(), item.description.as_deref()))
            .collect();
        assert_eq!(
            actual,
            [
                ("build", Some("Compile it")),
                ("two words", Some("A spaced value")),
                ("ignored-pre-hidden-body-inside-post-tail", None),
                ("quoted value", None),
            ]
        );
        assert!(!dir.0.join("marker").exists());
    }

    #[test]
    fn rich_records_reject_incomplete_packets_and_preserve_native_filtering() {
        for response in ["", "#notypo-zsh-v2\0'build'", "#notypo-zsh-v2\0'build'\0"] {
            assert!(parse(response, "", 10).is_err(), "{response:?}");
        }
        assert!(parse("#notypo-zsh-v2\0'one'\0\0one\0'two'\0\0two\0", "", 1).is_err());
        let Some(zsh) = crate::utils::which("zsh") else {
            return;
        };
        let dir = Dir::new("zsh-rich-filter");
        let script = dir.script("_tool", "#compdef tool\nlocal -a choices=('build' 'deploy') descriptions=('Compile it' 'Ship it')\ncompadd -d descriptions -a choices\n");
        let items = complete(
            &zsh,
            &Source::File(&script),
            "tool",
            &[],
            "de",
            Vec::new(),
            &mut budget(),
        )
        .unwrap();
        assert_eq!(items[0].value, "deploy");
        assert_eq!(items[0].description.as_deref(), Some("Ship it"));
    }

    #[test]
    fn options_whose_values_follow_an_equals_sign_are_listed() {
        let Some(zsh) = crate::utils::which("zsh") else {
            return;
        };
        let dir = Dir::new("zsh-equals");
        // clap's generated scripts declare valued options as `--name=`, which
        // _arguments adds with an `=` suffix. Other suffixed words keep it.
        let script = dir.script(
            "_tool",
            "#compdef tool\nif [[ $words[CURRENT] == -* ]]; then\n  _arguments '--name=[Name]:name:_default' '--level=-[Level]:level:(1 2)' '--quiet[Quiet]'\nelse\n  compadd -S = -- KEY\n  compadd -- build\nfi\n",
        );
        let backend = Backend::new(Flavor::ZshFunction, "tool", zsh).with_helper(script);
        let mut budget = budget();
        let options = backend.complete(&[], "--", &mut budget).unwrap();
        assert!(options.iter().all(|item| item.takes_value.is_none()));
        let mut options = values(options);
        options.sort();
        assert_eq!(options, ["--level", "--name", "--quiet"]);
        assert_eq!(
            values(backend.complete(&[], "", &mut budget).unwrap()),
            ["KEY=", "build"]
        );
        // dig's query options: `+timeout=` keeps its name like `--name=`.
        let script = dir.script(
            "_dig",
            "#compdef dig\nlocal -a args\n[[ -prefix + ]] && args=('*+timeout=[Timeout]:seconds' '*+'{no,}'short[Short]')\n_arguments -s $args '-p+[Port]:port' '*:name:'\n",
        );
        let dig = Backend::new(
            Flavor::ZshFunction,
            "dig",
            crate::utils::which("zsh").unwrap(),
        )
        .with_helper(script);
        let mut plus = values(dig.complete(&["example.com"], "+", &mut budget).unwrap());
        plus.sort();
        assert_eq!(plus, ["+noshort", "+short", "+timeout"]);
        // Only a `+` prefix lists them, so that is what dig's words ask.
        assert_eq!(dig.option_prefix("+shrot"), "+");
        assert!(!dig.is_short_option("+shrot"));
        assert_eq!(dig.option_separator("+tiemout=5"), Some('='));
        assert_eq!(
            dig.option_requires_value(&[], "+short", Some("x"), &mut budget),
            Some(false)
        );
        let chmod = Backend::new(
            Flavor::ZshFunction,
            "chmod",
            crate::utils::which("zsh").unwrap(),
        );
        assert_eq!(chmod.option_separator("+x"), None);
    }

    #[test]
    fn session_handlers_name_their_function_and_carry_definitions() {
        let text = "compdef _tool tool\ncompdef _other other\n#notypo-functions\n_tool () {\n\t_tool_commands\n}\n";
        let (definitions, function) = memory_in(text, "tool").unwrap();
        assert_eq!(function, "_tool");
        assert!(definitions.starts_with("_tool () {"));
        assert_eq!(memory_in(text, "missing"), None);
        assert_eq!(
            memory_in("compdef _tool tool\n#notypo-functions\n", "tool"),
            None
        );
        assert_eq!(
            memory_in("compdef $(x) tool\n#notypo-functions\nf () {}\n", "tool"),
            None
        );
        assert_eq!(
            memory_in("compdef _tool tool extra\n#notypo-functions\nf\n", "tool"),
            None
        );
    }

    #[test]
    fn session_definitions_run_in_a_real_widget_with_their_helpers() {
        let Some(zsh) = crate::utils::which("zsh") else {
            return;
        };
        let definitions = "_tool () {\n\t_tool_commands\n}\n_tool_commands () {\n\t_arguments '1:command:(build deploy)'\n}\n";
        let source = Source::Text {
            definitions,
            function: "_tool",
        };
        let items = complete(&zsh, &source, "tool", &[], "", Vec::new(), &mut budget()).unwrap();
        assert_eq!(values(items), ["build", "deploy"]);
        let missing = Source::Text {
            definitions,
            function: "_absent",
        };
        assert!(matches!(
            complete(&zsh, &missing, "tool", &[], "", Vec::new(), &mut budget()),
            Err(CompletionError::Failed(_))
        ));
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

    /// bitwarden's `_bw` only defines functions, `_bw` among them: zsh
    /// completes from the second Tab. A handler that keeps its definition
    /// runs once.
    #[test]
    fn handlers_that_define_themselves_are_called_again_once() {
        let Some(zsh) = crate::utils::which("zsh") else {
            return;
        };
        let dir = Dir::new("zsh-self-defining");
        let calls = dir.0.join("calls");
        let quoted = crate::shlex::quote(calls.to_str().unwrap());
        let script = dir.script(
            "_tool",
            &format!(
                "#compdef _tool tool\nfunction _tool {{\n  print call >> {quoted}\n  compadd -- build deploy\n}}\nfunction _tool_helper {{ :; }}\n"
            ),
        );
        let backend = Backend::new(Flavor::ZshFunction, "tool", zsh.clone()).with_helper(script);
        let words: Vec<String> = backend
            .complete(&[], "", &mut budget())
            .unwrap()
            .into_iter()
            .map(|item| item.value)
            .collect();
        assert_eq!(words, ["build", "deploy"]);
        assert_eq!(std::fs::read_to_string(&calls).unwrap(), "call\n");
        let once = dir.0.join("once");
        let quoted = crate::shlex::quote(once.to_str().unwrap());
        let plain = dir.script(
            "_plain",
            &format!("#compdef plain\nprint call >> {quoted}\n_files\n"),
        );
        let backend = Backend::new(Flavor::ZshFunction, "plain", zsh).with_helper(plain);
        // _files must see only this fixture, not the runner's checkout.
        backend
            .complete(&[], "", &mut budget().with_cwd(Some(&dir.0)))
            .unwrap_or_else(|error| panic!("plain handler completion failed: {error}"));
        assert_eq!(std::fs::read_to_string(&once).unwrap(), "call\n");
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
        for (body, prefix, expected) in [
            ("compadd -P pre -S / -- fix", "", "prefix/"),
            ("IPREFIX=pre; PREFIX=''; compadd -- fix", "pre", "prefix"),
            ("compadd -p pre -s / -- fix", "pre", "prefix/"),
        ] {
            let script = dir.script("_tool", &format!("#compdef tool\n{body}\n"));
            let backend =
                Backend::new(Flavor::ZshFunction, "tool", zsh.clone()).with_helper(script);
            assert_eq!(
                values(backend.complete(&[], prefix, &mut budget()).unwrap()),
                [expected],
                "{body}"
            );
        }
    }
}
