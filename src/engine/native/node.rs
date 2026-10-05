//! Node.js prints its own bash completion with `node --completion-bash`:
//! one `compgen -W` list of the options node knows by name, including some
//! V8 flags, and files for every other word. Printing it is part of option
//! processing, so no script, `--require` hook, or REPL runs; the probe
//! still clears NODE_OPTIONS. V8 accepts flags the list omits
//! (`--allow-natives-syntax`), so the list is partial, and the first word
//! that isn't an option is the script, whose own arguments follow.
//! `node --help` says which options take a value (`--require=...`), which
//! take only an optional attached one (`--inspect[=[host:]port]`, so the
//! next word is not theirs), and that the rest are switches; options it
//! doesn't show keep an unknown arity.

use super::{CompletionError, CompletionItem, run_stdout};
use crate::engine::probe::Budget;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;

/// node's completion script (`--completion-bash`) or help (`--help`),
/// printed by node itself.
pub(super) fn print(
    node: &Path,
    flag: &str,
    env: Vec<(OsString, Option<OsString>)>,
    budget: &mut Budget,
) -> Result<String, CompletionError> {
    run_stdout(node, vec![flag.into()], env, budget, true)
}

/// Whether the next word is an option's value, from `node --help`:
/// `Some(true)` for `--require=...`, `Some(false)` for switches and for
/// optional attached values (`--inspect[=port]`).
pub(super) fn arity(help: &str) -> HashMap<String, Option<bool>> {
    let mut arity = HashMap::new();
    for line in help.lines() {
        let Some(entry) = line.strip_prefix("  ").filter(|e| e.starts_with('-')) else {
            continue;
        };
        // The names end where the description starts.
        let head = entry.split("  ").next().unwrap_or_default().trim();
        let takes = !head.contains("[=") && (head.contains('=') || head.contains(" ["));
        for name in head.split(", ") {
            let name = name.split(['=', '[', ' ']).next().unwrap_or_default();
            if regex!(r"^--?[A-Za-z0-9][A-Za-z0-9_.-]*$").is_match(name) {
                arity.entry(name.to_owned()).or_insert(Some(takes));
            }
        }
    }
    arity
}

/// The `compgen -W '...'` list of `_node_complete`, registered for node.
/// Anything else, or an unexpected word, fails the whole answer.
pub(super) fn parse(text: &str, limit: usize) -> Result<Vec<CompletionItem>, String> {
    let registers = |line: &str| {
        let words: Vec<&str> = line.split_whitespace().collect();
        words.first() == Some(&"complete")
            && words.windows(2).any(|w| w == ["-F", "_node_complete"])
            && words.contains(&"node")
    };
    if !text.starts_with("_node_complete() {") || !text.lines().any(registers) {
        return Err("not node's completion script".into());
    }
    let list = regex!(r#"compgen -W '([^']*)' -- "\$\{cur_word\}""#)
        .captures(text)
        .ok_or("node's completion script has no option list")?;
    let mut items: Vec<CompletionItem> = Vec::new();
    for word in list[1].split_whitespace() {
        // A placeholder for an option's value.
        if word.starts_with('<') && word.ends_with('>') {
            continue;
        }
        let (name, attached) = match word.strip_suffix('=') {
            Some(name) => (name, true),
            None => (word, false),
        };
        if !regex!(r"^--?[A-Za-z0-9][A-Za-z0-9_.-]*$").is_match(name) {
            return Err(format!("unexpected word `{word}` in node's option list"));
        }
        match items.iter_mut().find(|item| item.value == name) {
            // `--inspect` and `--inspect=`: the value is optional.
            Some(item) => item.takes_value = None,
            None => items.push(CompletionItem {
                value: name.to_owned(),
                takes_value: attached.then_some(true),
                description: None,
            }),
        }
        if items.len() > limit {
            return Err("node's option list exceeded the candidate limit".into());
        }
    }
    if items.is_empty() {
        return Err("node's option list is empty".into());
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = r#"_node_complete() {
  local cur_word options
  cur_word="${COMP_WORDS[COMP_CWORD]}"
  if [[ "${cur_word}" == -* ]] ; then
    COMPREPLY=( $(compgen -W '--run --inspect --inspect= -r <arg> --require --max-old-space-size -pe --inspect-brk=' -- "${cur_word}") )
    return 0
  else
    COMPREPLY=( $(compgen -f "${cur_word}") )
    return 0
  fi
}
complete -o filenames -o nospace -o bashdefault -F _node_complete node node_g
"#;

    #[test]
    fn node_lists_its_options_in_its_own_completion_script() {
        let items = parse(SCRIPT, 100).unwrap();
        let names: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(
            names,
            [
                "--run",
                "--inspect",
                "-r",
                "--require",
                "--max-old-space-size",
                "-pe",
                "--inspect-brk"
            ]
        );
        assert_eq!(items[1].takes_value, None, "--inspect[=port]");
        assert_eq!(items[6].takes_value, Some(true));
        assert!(parse(SCRIPT, 3).is_err());
        for broken in [
            "",
            "echo hi",
            &SCRIPT.replace("_node_complete node", "_node_complete nodex"),
            &SCRIPT.replace("--run", "--run;rm"),
            &SCRIPT.replace("compgen -W", "compgen -X"),
        ] {
            assert!(parse(broken, 100).is_err(), "{broken}");
        }
    }

    #[test]
    fn help_says_which_options_take_values() {
        let help = "Options:\n  -                           script read from stdin\n  \
                    --abort-on-uncaught-exception\n                              aborting\n  \
                    -e, --eval=...              evaluate script\n  \
                    --inspect[=[host:]port]     activate inspector\n  \
                    --debug-port, --inspect-port=[host:]port\n  \
                    -p, --print [...]           evaluate script and print result\n  \
                    --watch                     run in watch mode\n\nEnvironment variables:\n\
                    FORCE_COLOR                 when set\n";
        let arity = arity(help);
        assert_eq!(arity["--abort-on-uncaught-exception"], Some(false));
        assert_eq!(arity["-e"], Some(true));
        assert_eq!(arity["--eval"], Some(true));
        assert_eq!(arity["--inspect"], Some(false), "only an attached value");
        assert_eq!(arity["--debug-port"], Some(true));
        assert_eq!(arity["--inspect-port"], Some(true));
        assert_eq!(arity["-p"], Some(true));
        assert_eq!(arity["--watch"], Some(false));
        assert!(!arity.contains_key("-"));
        assert!(!arity.contains_key("FORCE_COLOR"));
    }

    #[test]
    fn installed_node_answers_offline() {
        let Some(node) = crate::utils::which("node") else {
            eprintln!("skipped: node is not installed");
            return;
        };
        let mut budget = Budget::new(
            std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(5),
            2,
        );
        let env = || vec![(OsString::from("NODE_OPTIONS"), None)];
        let text = print(&node, "--completion-bash", env(), &mut budget).unwrap();
        let items = parse(&text, 5000).unwrap();
        assert!(items.iter().any(|i| i.value == "--inspect"));
        assert!(items.iter().any(|i| i.value == "--require"));
        let arity = arity(&print(&node, "--help", env(), &mut budget).unwrap());
        assert_eq!(arity.get("--require"), Some(&Some(true)));
        assert_eq!(arity.get("--watch"), Some(&Some(false)));
    }
}
