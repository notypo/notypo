//! clap_complete's environment protocol, learned from its generated script.
//!
//! An unknown completion variable runs the app normally, so neither a program
//! name nor a Rust binary is evidence for this protocol. Accept only supported
//! generated layouts. Never source the script or interpolate command words.

use super::{Backend, CompletionError, CompletionItem, Flavor, clean_description, run_stdout};
use crate::engine::cache::fingerprint;
use crate::engine::docs;
use crate::engine::probe::Budget;
use std::ffi::OsString;
use std::path::Path;

pub(super) fn read_script(path: &Path, limit: usize) -> Option<String> {
    use std::io::Read;
    let mut data = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(limit.saturating_add(1) as u64)
        .read_to_end(&mut data)
        .ok()?;
    (data.len() <= limit)
        .then(|| String::from_utf8(data).ok())
        .flatten()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Bash,
    Zsh,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
        }
    }

    fn separator(self) -> &'static str {
        match self {
            Self::Bash => "\x0b",
            Self::Zsh => "\n",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Protocol {
    variable: String,
    mode: Mode,
    binary_name: String,
    /// The app may have been replaced since its script was inspected.
    application_fingerprint: String,
}

fn app_fingerprint(path: &Path) -> String {
    let mut files = vec![path.to_owned()];
    files.extend(std::fs::canonicalize(path).ok());
    fingerprint(&files, &["clap-dynamic"])
}

pub(super) fn safe_variable(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .enumerate()
            .all(|(i, c)| c == b'_' || c.is_ascii_alphabetic() || i > 0 && c.is_ascii_digit())
        && !name.starts_with("_CLAP_")
        && !name.starts_with("LD_")
        && !name.starts_with("DYLD_")
        && ![
            "PATH", "HOME", "SHELL", "ENV", "BASH_ENV", "ZDOTDIR", "FPATH", "PWD", "IFS",
        ]
        .contains(&name)
}

pub(super) fn same_application(invoked: &str, name: &str, path: &Path) -> bool {
    invoked == name
        || Path::new(invoked).is_absolute()
            && std::fs::canonicalize(invoked)
                .ok()
                .is_some_and(|real| std::fs::canonicalize(path).ok().as_ref() == Some(&real))
        || std::fs::canonicalize(path)
            .ok()
            .and_then(|real| real.file_name().map(|base| base == invoked))
            .unwrap_or(false)
}

impl Protocol {
    pub(super) fn descriptions(&self) -> bool {
        self.mode == Mode::Zsh
    }

    /// The protocol-defining block is read as data. Calls to other helpers,
    /// extra arguments, ambiguous activation variables, and changed layouts
    /// are unsupported rather than tried against the application.
    pub(super) fn from_script(script: &str, name: &str, path: &Path) -> Option<Self> {
        let activation =
            regex!(r#"^([A-Za-z_][A-Za-z0-9_]*)=(?:"(bash|zsh)"|'(bash|zsh)'|(bash|zsh))$"#);
        let lines: Vec<_> = script
            .lines()
            .map(|line| line.trim().trim_end_matches('\\').trim())
            .collect();
        let mut found = None;
        for (i, line) in lines.iter().enumerate() {
            let Some(caps) = activation.captures(line) else {
                continue;
            };
            if found.is_some() || !safe_variable(&caps[1]) {
                return None;
            }
            let shell = caps
                .get(2)
                .or_else(|| caps.get(3))
                .or_else(|| caps.get(4))?
                .as_str();
            let mode = match shell {
                "bash" => Mode::Bash,
                "zsh" => Mode::Zsh,
                _ => return None,
            };
            let call = crate::shlex::split(lines.get(i + 1)?).ok()?;
            let [invoked, separator, words, tail @ ..] = call.as_slice() else {
                return None;
            };
            if !same_application(invoked, name, path)
                || separator != "--"
                || words != "${words[@]}"
                || !(tail.is_empty() || tail == ["2>/dev/null"])
            {
                return None;
            }
            // Require the actual bindings, not just incidental mentions in
            // comments, and the index convention of the supported template.
            let declaration = match mode {
                Mode::Zsh => {
                    regex!(r"^(?:function\s+)?(_clap_dynamic_completer_[A-Za-z0-9_-]+)\(\)\s*\{$")
                }
                Mode::Bash => {
                    regex!(r"^(?:function\s+)?(_clap_complete_[A-Za-z0-9_-]+)\(\)\s*\{$")
                }
            };
            let (start, function) = lines[..i]
                .iter()
                .enumerate()
                .filter_map(|(index, line)| {
                    let caps = declaration.captures(line)?;
                    Some((index, caps[1].to_owned()))
                })
                .next_back()?;
            let before = &lines[start + 1..i];
            if before.contains(&"}") {
                return None;
            }
            if !before.contains(&"_CLAP_COMPLETE_INDEX=\"$_CLAP_COMPLETE_INDEX\"") {
                return None;
            }
            let binary_name = match mode {
                Mode::Zsh => {
                    if !before.contains(&"_CLAP_IFS=\"$_CLAP_IFS\"")
                        || !before.contains(&"local _CLAP_IFS=$'\\n'")
                        || !before.iter().any(|line| {
                            regex!(r"^local _CLAP_COMPLETE_INDEX=\$\(expr\s+\$CURRENT\s+-\s+1\)$")
                                .is_match(line)
                        })
                    {
                        return None;
                    }
                    let bin = script.lines().find_map(|line| {
                        let names = crate::shlex::split(line.strip_prefix("#compdef ")?).ok()?;
                        names
                            .into_iter()
                            .find(|bin| same_application(bin, name, path))
                    })?;
                    if !script.lines().any(|line| {
                        crate::shlex::split(line.trim()).is_ok_and(|words| {
                            words.first().is_some_and(|word| word == "compdef")
                                && words.get(1).is_some_and(|word| word == &function)
                                && words.get(2).is_some_and(|word| word == &bin)
                                && words.len() == 3
                        })
                    }) {
                        return None;
                    }
                    bin
                }
                Mode::Bash => {
                    if !before.contains(&"_CLAP_IFS=\"$IFS\"")
                        || !before.contains(&"local IFS=$'\\013'")
                        || !before.contains(&"local _CLAP_COMPLETE_INDEX=${COMP_CWORD}")
                        || !before
                            .contains(&"_CLAP_COMPLETE_COMP_TYPE=\"$_CLAP_COMPLETE_COMP_TYPE\"")
                        || !before.contains(&"_CLAP_COMPLETE_SPACE=\"$_CLAP_COMPLETE_SPACE\"")
                    {
                        return None;
                    }
                    script.lines().find_map(|line| {
                        let words = crate::shlex::split(line.trim()).ok()?;
                        (words.first().is_some_and(|word| word == "complete")
                            && words
                                .windows(2)
                                .any(|pair| pair[0] == "-F" && pair[1] == function))
                        .then(|| words.last().cloned())
                        .flatten()
                        .filter(|bin| same_application(bin, name, path))
                    })?
                }
            };
            found = Some(Self {
                variable: caps[1].to_owned(),
                mode,
                binary_name: binary_name.into_owned(),
                application_fingerprint: app_fingerprint(path),
            });
        }
        found
    }

    pub(super) fn complete(
        &self,
        backend: &Backend,
        words: &[&str],
        prefix: &str,
        env: Vec<(OsString, Option<OsString>)>,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let mut items = self.query(backend, words, prefix, env.clone(), budget)?;
        // For `-`, clap lists a flag's short form and omits its long one
        // (`-n`, not `--dry-run`); `--` lists every long form.
        if prefix == "-" {
            for item in self.query(backend, words, "--", env, budget)? {
                if items.len() >= budget.max_candidates {
                    break;
                }
                if !items.iter().any(|existing| existing.value == item.value) {
                    items.push(item);
                }
            }
        }
        Ok(items)
    }

    fn query(
        &self,
        backend: &Backend,
        words: &[&str],
        prefix: &str,
        mut env: Vec<(OsString, Option<OsString>)>,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        if app_fingerprint(&backend.completer) != self.application_fingerprint {
            return Err(CompletionError::Failed(
                "the app changed after its completion script was inspected".into(),
            ));
        }
        let mut args: Vec<OsString> = vec!["--".into(), self.binary_name.clone().into()];
        args.extend(words.iter().map(OsString::from));
        args.push(prefix.into());
        env.extend([
            (self.variable.clone().into(), Some(self.mode.name().into())),
            (
                "_CLAP_COMPLETE_INDEX".into(),
                Some((words.len() + 1).to_string().into()),
            ),
            ("_CLAP_IFS".into(), Some(self.mode.separator().into())),
            ("_CLAP_COMPLETE_COMP_TYPE".into(), Some("9".into())),
            ("_CLAP_COMPLETE_SPACE".into(), Some("false".into())),
        ]);
        let offline = env.iter().any(|(key, value)| {
            key == "HTTPS_PROXY"
                && value.as_deref() == Some(std::ffi::OsStr::new(super::BLOCKED_PROXY))
        });
        let mut key = vec!["clap", if offline { "offline" } else { "network" }];
        key.extend_from_slice(words);
        key.push(prefix);
        let text = backend.memoized(&key, || {
            run_stdout(&backend.completer, args, env, budget, true)
        })?;
        let items = self.normalize(&text, prefix, budget.max_candidates.saturating_add(1));
        if items.len() > budget.max_candidates {
            return Err(super::over_limit());
        }
        Ok(items)
    }

    fn normalize(&self, text: &str, prefix: &str, limit: usize) -> Vec<CompletionItem> {
        if limit == 0 {
            return Vec::new();
        }
        let mut items = Vec::new();
        for line in text.split(self.mode.separator()) {
            let (value, description) = match self.mode {
                Mode::Bash => (line.to_owned(), None),
                Mode::Zsh => {
                    let mut value = String::new();
                    let mut chars = line.char_indices();
                    let mut description = None;
                    let mut valid = true;
                    while let Some((i, ch)) = chars.next() {
                        match ch {
                            ':' => {
                                description = clean_description(&line[i + 1..]);
                                break;
                            }
                            '\\' => match chars.next() {
                                Some((_, escaped @ ('\\' | ':'))) => value.push(escaped),
                                _ => {
                                    valid = false;
                                    break;
                                }
                            },
                            _ => value.push(ch),
                        }
                    }
                    if !valid {
                        continue;
                    }
                    (value, description)
                }
            };
            let mut normalized =
                super::normalize(std::iter::once(value.as_str()), Flavor::ClapDynamic, 1);
            let Some(mut item) = normalized.pop() else {
                continue;
            };
            item.description = description;
            if item.value.starts_with(prefix)
                && !items
                    .iter()
                    .any(|existing: &CompletionItem| existing.value == item.value)
            {
                items.push(item);
                if items.len() >= limit {
                    break;
                }
            }
        }
        items
    }
}

pub(super) fn backend(
    name: &str,
    path: &Path,
    identity: Option<String>,
    protocol: Protocol,
) -> Backend {
    let mut backend =
        Backend::new(Flavor::ClapDynamic, name, path.to_owned()).with_application(path.to_owned());
    backend.identity = identity;
    backend.clap = Some(Box::new(protocol));
    backend
}

/// Generator selection comes exclusively from the app's help. No failed
/// command words are forwarded, and no activation variable is set until a
/// generated script declares it. Unsupported/static scripts mean no bridge.
pub(super) fn discover_generated(
    name: &str,
    path: &Path,
    budget: &mut Budget,
) -> Result<Option<Protocol>, CompletionError> {
    let before = app_fingerprint(path);
    let help = run_stdout(
        path,
        vec!["--help".into()],
        super::offline_env(Flavor::ClapDynamic),
        budget,
        true,
    )?;
    for command in docs::subcommands(&help)
        .into_iter()
        .filter(|item| {
            [
                "completion",
                "completions",
                "generate-completion",
                "generate-completions",
            ]
            .contains(&item.value.as_str())
        })
        .take(2)
    {
        let help = run_stdout(
            path,
            vec![command.value.clone().into(), "--help".into()],
            super::offline_env(Flavor::ClapDynamic),
            budget,
            true,
        )?;
        for mode in [Mode::Zsh, Mode::Bash] {
            let Some(selector) = shell_selector(&help, &command.value, mode.name()) else {
                continue;
            };
            let mut args = vec![command.value.clone().into()];
            args.extend(selector.into_iter().map(OsString::from));
            let script = run_stdout(
                path,
                args,
                super::offline_env(Flavor::ClapDynamic),
                budget,
                true,
            )?;
            if let Some(protocol) = Protocol::from_script(&script, name, path) {
                if protocol.mode != mode || before != protocol.application_fingerprint {
                    return Err(CompletionError::Failed(
                        "the completion generator or application changed during discovery".into(),
                    ));
                }
                return Ok(Some(protocol));
            }
        }
    }
    Ok(None)
}

/// clap's recommended registration (`source <(VAR=zsh app)`, or Homebrew's
/// `eval "$(VAR=bash app)"`) names the variable without the generated
/// layout. Run that same generator, as the shell does when it loads the
/// file, and accept only a supported generated script for that variable.
/// Files with any other content are not shims.
pub(super) fn shim_protocol(
    script: &str,
    name: &str,
    path: &Path,
    budget: &mut Budget,
) -> Result<Option<Protocol>, CompletionError> {
    let shim = regex!(
        r#"^(?:(?:source|\.)\s+<\(\s*([A-Za-z_][A-Za-z0-9_]*)=(bash|zsh)\s+([^\s"'$`()\\;|&<>]+)\s*\)|eval\s+"\$\(\s*([A-Za-z_][A-Za-z0-9_]*)=(bash|zsh)\s+([^\s"'$`()\\;|&<>]+)\s*\)")$"#
    );
    let mut found = None;
    for line in script.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(caps) = shim.captures(line) {
            if found.is_some() {
                return Ok(None);
            }
            let group = |a: usize, b: usize| {
                caps.get(a)
                    .or_else(|| caps.get(b))
                    .map(|m| m.as_str().to_owned())
            };
            found = group(1, 4).zip(group(2, 5)).zip(group(3, 6));
            continue;
        }
        // Homebrew's zsh wrapper calls the function the generator defines.
        let wrapper = line == "fi"
            || regex!(r#"^if \[ "\$funcstack\[1\]" = "_[A-Za-z0-9_-]+" \]; then$"#).is_match(line)
            || regex!(r#"^_clap_dynamic_completer_[A-Za-z0-9_-]+ "\$@"$"#).is_match(line);
        if !wrapper {
            return Ok(None);
        }
    }
    let Some(((variable, shell), program)) = found else {
        return Ok(None);
    };
    if !safe_variable(&variable) || !same_application(&program, name, path) {
        return Ok(None);
    }
    let before = app_fingerprint(path);
    let mut env = super::offline_env(Flavor::ClapDynamic);
    env.push((variable.clone().into(), Some(shell.clone().into())));
    let generated = run_stdout(path, Vec::new(), env, budget, true)?;
    let Some(protocol) = Protocol::from_script(&generated, name, path) else {
        return Ok(None);
    };
    if protocol.variable != variable
        || protocol.mode.name() != shell
        || protocol.application_fingerprint != before
    {
        return Err(CompletionError::Failed(
            "the completion generator or application changed during discovery".into(),
        ));
    }
    Ok(Some(protocol))
}

fn shell_selector(help: &str, command: &str, shell: &str) -> Option<Vec<String>> {
    let usage = help.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Usage: ")
            .or_else(|| line.trim().strip_prefix("USAGE: "))
    })?;
    let mut words = usage.split_whitespace();
    // The program name may be an absolute path containing spaces. Match the
    // documented generator word, then require only its shell and options.
    words.find(|word| *word == command)?;
    let args: Vec<_> = words.collect();
    if args
        .iter()
        .any(|word| !["[OPTIONS]", "<SHELL>", "[SHELL]"].contains(word))
    {
        return None;
    }
    if args
        .iter()
        .any(|word| ["<SHELL>", "[SHELL]"].contains(word))
    {
        let mut argument = false;
        let mut shell_argument = false;
        for line in help.lines() {
            let trimmed = line.trim();
            if trimmed == "Arguments:" {
                argument = true;
                continue;
            }
            if !line.starts_with(char::is_whitespace) && !trimmed.is_empty() {
                argument = false;
            }
            if !argument {
                continue;
            }
            if trimmed.starts_with("<SHELL>") || trimmed.starts_with("[SHELL]") {
                shell_argument = true;
            } else if trimmed.starts_with(['<', '[']) && !trimmed.starts_with("[possible values:") {
                shell_argument = false;
            }
            if shell_argument
                && let Some(caps) = regex!(r"\[possible values:\s*([^\[\]]+)\]").captures(trimmed)
                && caps[1]
                    .split(',')
                    .map(str::trim)
                    .any(|choice| choice == shell)
            {
                return Some(vec![shell.to_owned()]);
            }
        }
    } else if docs::options(help)
        .iter()
        .any(|item| item.value == "--shell" && item.takes_value == Some(true))
        && docs::option_values(help, "--shell")
            .iter()
            .any(|item| item.value == shell)
    {
        return Some(vec!["--shell".into(), shell.to_owned()]);
    }
    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::tests::Dir;
    use super::super::{Discovery, NativeCompletionBackend, discover_for_shell_with_budget};
    use super::*;
    use crate::shells::Shell;
    use std::time::Duration;

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(10), Duration::from_secs(2), 16)
    }

    // Minimal valid registrations, authored as fixtures. The bridge never
    // sources the side effect placed outside their completion function.
    fn zsh_registration(name: &str, variable: &str) -> String {
        format!(
            r#"#compdef {name}
function _clap_dynamic_completer_fixture() {{
  local _CLAP_COMPLETE_INDEX=$(expr $CURRENT - 1)
  local _CLAP_IFS=$'\n'
  local replies=$( \
    _CLAP_IFS="$_CLAP_IFS" \
    _CLAP_COMPLETE_INDEX="$_CLAP_COMPLETE_INDEX" \
    {variable}="zsh" \
    {name} -- "${{words[@]}}" 2>/dev/null \
  )
}}
compdef _clap_dynamic_completer_fixture {name}
printf sourced > registration-marker
"#
        )
    }

    fn bash_registration(name: &str, variable: &str) -> String {
        format!(
            r#"_clap_complete_fixture() {{
  local IFS=$'\013'
  local _CLAP_COMPLETE_INDEX=${{COMP_CWORD}}
  local replies=$( \
    _CLAP_IFS="$IFS" \
    _CLAP_COMPLETE_INDEX="$_CLAP_COMPLETE_INDEX" \
    _CLAP_COMPLETE_COMP_TYPE="$_CLAP_COMPLETE_COMP_TYPE" \
    _CLAP_COMPLETE_SPACE="$_CLAP_COMPLETE_SPACE" \
    {variable}="bash" \
    "{name}" -- "${{words[@]}}" \
  )
}}
complete -F _clap_complete_fixture {name}
printf sourced > registration-marker
"#
        )
    }

    const APP: &str = r#"#!/bin/sh
root=$(dirname "$0")
printf '[%s]' "$@" >> "$root/calls"
printf '\n' >> "$root/calls"
if [ "$#" = 0 ] && [ "$FIXTURE_COMPLETE" = zsh ]; then cat "$root/registration"; exit; fi
case "$*" in
  '--help') printf 'Commands:\n  completion  Print a shell completion script\n'; exit;;
  'completion --help') cat "$root/help"; exit;;
  'completion zsh') cat "$root/registration"; exit;;
esac
if [ "$FIXTURE_COMPLETE" != zsh ]; then
  printf operation > "$root/operation-marker"
  exit 3
fi
[ "$HTTPS_PROXY" = http://127.0.0.1:9 ] || exit 4
[ "$KUBECONFIG" = /dev/null ] || exit 5
[ "$1" = -- ] || exit 6
shift
[ "$1" = tool ] || exit 7
shift
[ "$_CLAP_COMPLETE_INDEX" = "$#" ] || exit 8
if [ -f "$root/result-mode" ]; then
  case "$(cat "$root/result-mode")" in
    failed) printf 'nodes:Not a valid answer\n'; exit 7;;
    utf8) printf '\377'; exit;;
    oversized) printf '%8192s' x; exit;;
    hanging) (sleep 1; touch "$root/delayed-marker") & wait; exit;;
  esac
fi
context=
while [ "$#" -gt 1 ]; do
  context="$context $1"
  shift
done
while IFS='|' read -r path value description; do
  [ "$context" = "$path" ] || continue
  # Like clap: for `-`, a flag with a short form is listed only by it.
  if [ -f "$root/short-dash" ] && [ "$1" = - ]; then
    case "$value" in --format) continue;; esac
  fi
  case "$value" in "$1"*) printf '%s:%s\n' "$value" "$description";; esac
done < "$root/tree"
"#;

    fn app(dir: &Dir) -> std::path::PathBuf {
        let app = dir.script("tool", APP);
        dir.script(
            "help",
            "Usage: tool completion <SHELL>\n\nArguments:\n  <SHELL>  [possible values: zsh]\n",
        );
        dir.script(
            "registration",
            &zsh_registration("tool", "FIXTURE_COMPLETE"),
        );
        dir.script("tree", "|nodes|Manage nodes\n nodes|list|List nodes\n nodes list|--format|Output format\n nodes list|--target|A target\n nodes list --format|json|JSON\n nodes list --format|yaml|YAML\n");
        app
    }

    #[test]
    fn generated_protocol_is_generic_bounded_literal_and_request_memoized() {
        let dir = Dir::new("clap-generated");
        let path = app(&dir);
        let mut budget = budget();
        let Discovery::Found(backend) = discover_for_shell_with_budget(
            "tool",
            &path,
            &["tool".into()],
            &["tool".into()],
            Shell::Fish,
            &mut budget,
        ) else {
            panic!("the app's generated script should declare its protocol");
        };
        assert_eq!(backend.id(), "clap");
        assert!(backend.capabilities().descriptions);
        assert_eq!(backend.capabilities().query_dialect, None);
        assert!(!backend.capabilities().option_arity);
        assert_eq!(budget.spawned(), 3);
        let root = backend.complete(&[], "", &mut budget).unwrap();
        assert_eq!(root[0].value, "nodes");
        assert_eq!(root[0].description.as_deref(), Some("Manage nodes"));
        assert_eq!(backend.complete(&[], "", &mut budget).unwrap(), root);
        assert_eq!(budget.spawned(), 4);
        let values = backend
            .complete_values(&["nodes", "list", "--format"], &mut budget)
            .unwrap();
        assert_eq!(
            values
                .iter()
                .map(|item| item.value.as_str())
                .collect::<Vec<_>>(),
            ["json", "yaml"]
        );
        backend
            .complete(
                &["nodes", "list", "--target", "$(touch operation-marker)"],
                "",
                &mut budget,
            )
            .unwrap();
        assert!(
            std::fs::read_to_string(dir.0.join("calls"))
                .unwrap()
                .contains("[$(touch operation-marker)]")
        );
        for marker in ["operation-marker", "registration-marker"] {
            assert!(!dir.0.join(marker).exists());
        }
        assert_eq!(
            backend.cache_identity(),
            None,
            "generic callbacks are not persistent metadata"
        );
    }

    #[test]
    fn dash_queries_add_long_flags_clap_lists_only_for_double_dashes() {
        let dir = Dir::new("clap-dash");
        let path = app(&dir);
        dir.script("tree", "|nodes|Manage nodes\n nodes list|--format|Output format\n nodes list|-f|Output format\n nodes list|--target|A target\n");
        dir.script("short-dash", "");
        let mut budget = budget();
        let Discovery::Found(backend) = discover_for_shell_with_budget(
            "tool",
            &path,
            &["tool".into()],
            &["tool".into()],
            Shell::Bash,
            &mut budget,
        ) else {
            panic!()
        };
        let options: Vec<String> = backend
            .complete(&["nodes", "list"], "-", &mut budget)
            .unwrap()
            .into_iter()
            .map(|item| item.value)
            .collect();
        assert_eq!(options, ["-f", "--target", "--format"]);
    }

    #[test]
    fn recommended_shims_run_only_their_declared_generator() {
        let dir = Dir::new("clap-shim");
        let path = app(&dir);
        let mut budget = budget();
        for shim in [
            "#compdef tool\nsource <(FIXTURE_COMPLETE=zsh tool)\nif [ \"$funcstack[1]\" = \"_tool\" ]; then\n  _clap_dynamic_completer_tool \"$@\"\nfi\n",
            "source <(FIXTURE_COMPLETE=zsh tool)\n",
        ] {
            let protocol = shim_protocol(shim, "tool", &path, &mut budget)
                .unwrap()
                .unwrap();
            assert_eq!(protocol.variable, "FIXTURE_COMPLETE");
        }
        assert_eq!(
            std::fs::read_to_string(dir.0.join("calls")).unwrap(),
            "[]\n[]\n",
            "only the generator ran, without arguments"
        );
        for shim in [
            // Other content, another program, unsafe variables, or two
            // generators are not the recommended registration.
            "source <(FIXTURE_COMPLETE=zsh tool)\ntouch registration-marker\n",
            "source <(FIXTURE_COMPLETE=zsh other)\n",
            "source <(PATH=zsh tool)\n",
            "source <(FIXTURE_COMPLETE=zsh tool)\nsource <(FIXTURE_COMPLETE=zsh tool)\n",
            "source <(FIXTURE_COMPLETE=zsh tool --flag)\n",
        ] {
            assert_eq!(
                shim_protocol(shim, "tool", &path, &mut budget),
                Ok(None),
                "{shim}"
            );
        }
        // A generator whose output is not a supported layout yields nothing.
        dir.script("registration", "#compdef tool\n_tool() { :; }\n");
        assert_eq!(
            shim_protocol(
                "source <(FIXTURE_COMPLETE=zsh tool)\n",
                "tool",
                &path,
                &mut budget
            ),
            Ok(None)
        );
        for marker in ["operation-marker", "registration-marker"] {
            assert!(!dir.0.join(marker).exists());
        }
    }

    #[test]
    fn registration_layouts_are_declared_and_unknown_layouts_are_not_tried() {
        let dir = Dir::new("clap-layout");
        let path = app(&dir);
        let zsh = zsh_registration("tool", "CUSTOM_VARIABLE");
        let bash = bash_registration("tool", "OTHER_VARIABLE");
        assert_eq!(
            Protocol::from_script(&zsh, "tool", &path).unwrap().variable,
            "CUSTOM_VARIABLE"
        );
        assert_eq!(
            Protocol::from_script(&bash, "tool", &path).unwrap().mode,
            Mode::Bash
        );
        for invalid in [
            zsh.replace("CUSTOM_VARIABLE", "PATH"),
            zsh.replace("tool --", "tool --unknown-mode"),
            zsh.replace("tool --", "other-helper --"),
            zsh.replace("${words[@]}", "$unsafe"),
            zsh.replace("expr $CURRENT - 1", "expr $CURRENT - 2"),
            zsh.replace("#compdef tool", "#compdef other"),
            zsh.replace(
                "compdef _clap_dynamic_completer_fixture tool",
                "compdef _clap_dynamic_completer_unrelated tool",
            ),
            zsh.replace("function _clap_dynamic_completer_fixture() {", "# marker"),
            zsh.replace("  local _CLAP_IFS", "}\n  local _CLAP_IFS"),
            zsh.replace(
                "CUSTOM_VARIABLE=\"zsh\"",
                "CUSTOM_VARIABLE=\"zsh\"\nOTHER=\"zsh\"",
            ),
            bash.replace("local IFS=$'\\013'", "local IFS=' '"),
            bash.replace(
                "complete -F _clap_complete_fixture tool",
                "complete -F _clap_complete_other tool",
            ),
        ] {
            assert!(
                Protocol::from_script(&invalid, "tool", &path).is_none(),
                "{invalid}"
            );
        }
        assert!(!dir.0.join("calls").exists());
    }

    #[test]
    fn missing_trust_script_or_documented_shell_never_starts_the_app() {
        let dir = Dir::new("clap-no-proof");
        let path = app(&dir);
        let mut budget = budget();
        assert_eq!(
            discover_for_shell_with_budget("tool", &path, &[], &[], Shell::Bash, &mut budget),
            Discovery::None
        );
        assert_eq!(budget.spawned(), 0);
        assert_eq!(
            discover_for_shell_with_budget(
                "tool",
                &path,
                &["tool".into()],
                &[],
                Shell::Bash,
                &mut budget
            ),
            Discovery::None
        );
        assert_eq!(
            budget.spawned(),
            0,
            "completion trust alone doesn't authorize help probes"
        );
        dir.script(
            "help",
            "Usage: tool completion <PROJECT>\nArguments:\n  <PROJECT>  A project named zsh\n",
        );
        assert_eq!(
            discover_for_shell_with_budget(
                "tool",
                &path,
                &["tool".into()],
                &["tool".into()],
                Shell::Bash,
                &mut budget
            ),
            Discovery::None
        );
        assert_eq!(budget.spawned(), 2);
        dir.script(
            "help",
            "Usage: tool completion <SHELL>\nArguments:\n  <SHELL>  [possible values: zsh]\n",
        );
        dir.script(
            "registration",
            "#compdef tool\n# A static script: no dynamic protocol\n",
        );
        assert_eq!(
            discover_for_shell_with_budget(
                "tool",
                &path,
                &["tool".into()],
                &["tool".into()],
                Shell::Bash,
                &mut budget
            ),
            Discovery::None
        );
        assert_eq!(budget.spawned(), 5);
        assert!(!dir.0.join("operation-marker").exists());
    }

    #[test]
    fn upgrades_are_detected_before_query_and_new_scripts_and_commands_are_used() {
        let dir = Dir::new("clap-upgrade");
        let path = app(&dir);
        let mut budget = budget();
        let Discovery::Found(old) = discover_for_shell_with_budget(
            "tool",
            &path,
            &["tool".into()],
            &["tool".into()],
            Shell::Bash,
            &mut budget,
        ) else {
            panic!()
        };
        let calls = std::fs::read(dir.0.join("calls")).unwrap();
        dir.script(
            "tool",
            &(APP.replace("FIXTURE_COMPLETE", "UPGRADED_COMPLETE") + "\n# upgraded\n"),
        );
        dir.script(
            "registration",
            &zsh_registration("tool", "UPGRADED_COMPLETE"),
        );
        dir.script("tree", "|deploy|Newly installed command\n");
        assert!(
            matches!(old.complete(&[], "", &mut budget), Err(CompletionError::Failed(why)) if why.contains("changed"))
        );
        assert_eq!(
            std::fs::read(dir.0.join("calls")).unwrap(),
            calls,
            "no stale variable is sent to the updated app"
        );
        let Discovery::Found(new) = discover_for_shell_with_budget(
            "tool",
            &path,
            &["tool".into()],
            &["tool".into()],
            Shell::Bash,
            &mut budget,
        ) else {
            panic!()
        };
        assert_eq!(
            new.complete(&[], "", &mut budget).unwrap()[0].value,
            "deploy"
        );
        assert!(!dir.0.join("operation-marker").exists());
    }

    #[test]
    fn rich_results_unescape_values_clean_descriptions_and_obey_limits() {
        let dir = Dir::new("clap-results");
        let path = app(&dir);
        let protocol =
            Protocol::from_script(&zsh_registration("tool", "FIXTURE_COMPLETE"), "tool", &path)
                .unwrap();
        let results = protocol.normalize("value\\:one:Has: a colon\n--format=:Output\nother:unsafe\t\u{202e} description\nbad\\q:Invalid escape\nvalue\\:one:Duplicate\n", "", 3);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].value, "value:one");
        assert_eq!(results[0].description.as_deref(), Some("Has: a colon"));
        assert_eq!(results[1].value, "--format");
        assert_eq!(results[1].takes_value, Some(true));
        assert_eq!(
            results[2].description.as_deref(),
            Some("unsafe description")
        );
        assert!(protocol.normalize("nodes:Description", "", 0).is_empty());
        let bash = Protocol::from_script(
            &bash_registration("tool", "FIXTURE_COMPLETE"),
            "tool",
            &path,
        )
        .unwrap();
        assert_eq!(bash.normalize("a\x0bb\x0bc", "", 2).len(), 2);
        assert!(!bash.descriptions());
    }

    #[test]
    fn bash_only_and_shell_option_generators_follow_their_documented_layouts() {
        let dir = Dir::new("clap-bash-only");
        let path = app(&dir);
        dir.script(
            "tool",
            &APP.replace("'completion zsh'", "'completion bash'")
                .replace("!= zsh", "!= bash")
                .replace(
                    "printf '%s:%s\\n' \"$value\" \"$description\"",
                    "printf '%s\\013' \"$value\"",
                ),
        );
        dir.script(
            "help",
            "Usage: tool completion <SHELL>\nArguments:\n  <SHELL>  [possible values: bash]\n",
        );
        dir.script(
            "registration",
            &bash_registration("tool", "FIXTURE_COMPLETE"),
        );
        let mut budget = budget();
        let Discovery::Found(backend) = discover_for_shell_with_budget(
            "tool",
            &path,
            &["tool".into()],
            &["tool".into()],
            Shell::Zsh,
            &mut budget,
        ) else {
            panic!()
        };
        assert!(!backend.capabilities().descriptions);
        assert_eq!(
            backend.complete(&[], "", &mut budget).unwrap()[0].value,
            "nodes"
        );
        assert_eq!(budget.spawned(), 4);

        let dir = Dir::new("clap-shell-option");
        let path = app(&dir);
        dir.script(
            "tool",
            &APP.replace("'completion zsh'", "'completion --shell zsh'"),
        );
        dir.script("help", "Usage: tool completion [OPTIONS]\nOptions:\n  --shell <SHELL>  [possible values: zsh]\n");
        let mut budget = self::budget();
        let Discovery::Found(backend) = discover_for_shell_with_budget(
            "tool",
            &path,
            &["tool".into()],
            &["tool".into()],
            Shell::Bash,
            &mut budget,
        ) else {
            panic!()
        };
        assert_eq!(
            backend.complete(&[], "", &mut budget).unwrap()[0].value,
            "nodes"
        );
        assert!(
            std::fs::read_to_string(dir.0.join("calls"))
                .unwrap()
                .contains("[completion][--shell][zsh]")
        );
        assert!(!dir.0.join("operation-marker").exists());
    }

    #[test]
    fn failed_invalid_truncated_and_hanging_answers_are_discarded() {
        let dir = Dir::new("clap-failures");
        let path = app(&dir);
        for (mode, limit, reason) in [
            ("failed", 1024, "exited"),
            ("utf8", 1024, "UTF-8"),
            ("oversized", 64, "limit"),
            ("hanging", 1024, "timed out"),
        ] {
            dir.script("result-mode", mode);
            let protocol =
                Protocol::from_script(&zsh_registration("tool", "FIXTURE_COMPLETE"), "tool", &path)
                    .unwrap();
            let backend = backend("tool", &path, None, protocol);
            let timeout = if mode == "hanging" {
                Duration::from_millis(250)
            } else {
                Duration::from_secs(2)
            };
            let mut budget = Budget::new(Duration::from_secs(3), timeout, 1);
            budget.max_output = limit;
            let answer = backend.complete(&[], "", &mut budget);
            assert!(
                matches!(&answer, Err(CompletionError::Failed(why)) if why.contains(reason)),
                "{mode}: {answer:?}"
            );
            assert_eq!(budget.spawned(), 1);
        }
        std::thread::sleep(Duration::from_millis(1050));
        assert!(!dir.0.join("delayed-marker").exists());
        let mut budget = Budget::new(Duration::from_secs(1), Duration::from_secs(1), 1);
        assert!(
            matches!(discover_for_shell_with_budget("tool", &path, &["tool".into()], &["tool".into()], Shell::Bash, &mut budget), Discovery::Unavailable(why) if why.contains("budget"))
        );
        assert!(!dir.0.join("operation-marker").exists());
    }
}
