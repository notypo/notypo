//! Click 8's completion protocol: with `<VAR>=zsh_complete`, the app reads
//! `COMP_WORDS` (split like `shlex.split`) and `COMP_CWORD`, prints three
//! lines per item (type, literal value, description), and exits before
//! running any command. No shell needs to run to use this wire format.
//! Described colons in newer versions are verified against the literal
//! Bash format; multiline descriptions also fall back to that format.
//! `plain` items are words; `file` and `dir` ask the shell to complete
//! paths instead. The variable is derived from the program name the app was
//! started with, or set by the app, and an app ignoring it just runs. So it
//! comes from evidence only: an installed click script (read, never
//! sourced), or, under `trusted_help`, a Python console script whose own
//! help carries click's signature and names the program click uses. An app
//! that rewords click's help option (oci) counts when its help has click's
//! `Usage: <prog> [OPTIONS]` line and click 8 is installed beside the app's
//! own module, in the site-packages its interpreter reads.
//!
//! Resolving the context runs parameter type conversions and callbacks
//! (without prompts), as the app's own completion does; command callbacks
//! never run. Groups that load commands lazily can import code while
//! listing them (flask loads the project's application).

use super::clap::{safe_variable, same_application};
use super::{Backend, Capabilities, CompletionError, CompletionItem, Flavor, Trust, run_stdout};
use crate::engine::cache::fingerprint;
use crate::engine::probe::Budget;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Protocol {
    variable: String,
    /// The app may have been replaced since its evidence was read.
    application_fingerprint: String,
    /// Only own help distinguishes root command names from callback values.
    root_commands: Option<Vec<String>>,
}

fn app_fingerprint(path: &Path) -> String {
    let mut files = vec![path.to_owned()];
    files.extend(std::fs::canonicalize(path).ok());
    fingerprint(&files, &["click"])
}

/// `_TOOL_COMPLETE` for `tool`, as click derives it.
fn variable_for(program: &str) -> Option<String> {
    if program.is_empty()
        || program.len() > 64
        || !program
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
    {
        return None;
    }
    let variable = format!("_{}_COMPLETE", program.replace(['-', '.'], "_")).to_uppercase();
    safe_variable(&variable).then_some(variable)
}

pub(super) fn backend(
    name: &str,
    path: &Path,
    identity: Option<String>,
    protocol: Protocol,
) -> Backend {
    let mut backend =
        Backend::new(Flavor::Click, name, path.to_owned()).with_application(path.to_owned());
    backend.identity = identity;
    backend.click = Some(Box::new(protocol));
    backend
}

impl Protocol {
    /// A click 8 bash, zsh, or fish script: its query line names the
    /// variable and program, and its registration binds that program. Other
    /// layouts (click 7's instructions, edited scripts) are not recognized.
    pub(super) fn from_script(script: &str, name: &str, path: &Path) -> Option<Self> {
        let lines: Vec<&str> = script.lines().map(str::trim).collect();
        let bash = regex!(
            r#"^response=\$\(env COMP_WORDS="\$\{COMP_WORDS\[\*\]\}" COMP_CWORD=\$COMP_CWORD ([A-Za-z_][A-Za-z0-9_]*)=bash_complete \$1\)$"#
        );
        let zsh = regex!(
            r#"^response=\("\$\{\(@f\)\$\(env COMP_WORDS="\$\{words\[\*\]\}" COMP_CWORD=\$\(\(CURRENT-1\)\) ([A-Za-z_][A-Za-z0-9_]*)=zsh_complete ([^\s"$`\\()]+)\)\}"\)$"#
        );
        let fish = regex!(
            r#"^(?:set -l response|for value in) \(env ([A-Za-z_][A-Za-z0-9_]*)=fish_complete COMP_WORDS=\(commandline -cp\) COMP_CWORD=\(commandline -t\) ([^\s"$`\\()]+)\);?$"#
        );
        let mut found: Option<(String, Option<String>, char)> = None;
        for line in &lines {
            let hit = if let Some(caps) = bash.captures(line) {
                Some((caps[1].to_owned(), None, 'b'))
            } else if let Some(caps) = zsh.captures(line) {
                Some((caps[1].to_owned(), Some(caps[2].to_owned()), 'z'))
            } else {
                fish.captures(line)
                    .map(|caps| (caps[1].to_owned(), Some(caps[2].to_owned()), 'f'))
            };
            if let Some(hit) = hit {
                // One query line only: a script mixing layouts is not click's.
                if found.is_some() {
                    return None;
                }
                found = Some(hit);
            }
        }
        let (variable, program, shell) = found?;
        if !safe_variable(&variable) {
            return None;
        }
        // The registration must bind this application, not another name.
        fn words(line: &str) -> Option<Vec<String>> {
            let words = crate::shlex::split(line).ok()?;
            Some(words.into_iter().map(|word| word.into_owned()).collect())
        }
        let bound = match shell {
            'b' => lines.iter().filter_map(|line| words(line)).any(|words| {
                matches!(words.as_slice(), [complete, o, nosort, f, function, bin]
                    if complete == "complete" && o == "-o" && nosort == "nosort" && f == "-F"
                        && function.starts_with('_') && same_application(bin, name, path))
            }),
            'z' => {
                let program = program.as_deref()?;
                same_application(program, name, path)
                    && lines.iter().any(|line| {
                        line.strip_prefix("#compdef ")
                            .and_then(words)
                            .is_some_and(|names| names.iter().any(|bin| bin == program))
                    })
                    && lines.iter().filter_map(|line| words(line.trim_end_matches(';'))).any(
                        |words| {
                            matches!(words.as_slice(), [compdef, function, bin]
                                if compdef == "compdef" && function.starts_with('_') && bin == program)
                        },
                    )
            }
            _ => {
                let program = program.as_deref()?;
                same_application(program, name, path)
                    && lines.iter().filter_map(|line| words(line.trim_end_matches(';'))).any(
                        |words| {
                            matches!(words.as_slice(), [complete, no_files, command, bin, arguments, function]
                                if complete == "complete" && no_files == "--no-files"
                                    && command == "--command" && bin == program
                                    && arguments == "--arguments" && function.starts_with("(_")
                                    && function.ends_with(')'))
                        },
                    )
            }
        };
        bound.then(|| Self {
            variable,
            application_fingerprint: app_fingerprint(path),
            root_commands: None,
        })
    }

    /// Reads a Python console script's own help: click's help option text
    /// marks the library, and its usage line names the program click derives
    /// the variable from (custom `prog_name` included).
    pub(super) fn from_help(help: &str, path: &Path) -> Option<Self> {
        if !help.contains("Show this message and exit.") {
            return None;
        }
        Self::from_usage(help, path, false)
    }

    /// The program click's usage line names. With `options`, the line must
    /// have click's own shape, `Usage: <prog> [OPTIONS]`.
    fn from_usage(help: &str, path: &Path, options: bool) -> Option<Self> {
        let program = help.lines().find_map(|line| {
            let line = line.trim_matches(|c: char| c.is_whitespace() || "│┃|".contains(c));
            let mut words = line.strip_prefix("Usage: ")?.split_whitespace();
            let program = words.next()?;
            (!options || words.next() == Some("[OPTIONS]")).then_some(program)
        })?;
        Some(Self {
            variable: variable_for(program)?,
            application_fingerprint: app_fingerprint(path),
            root_commands: root_commands(help),
        })
    }

    pub fn capabilities(&self, trust: Trust) -> Capabilities {
        Capabilities {
            subcommands: true,
            options: true,
            option_arity: false,
            values: true,
            resources: false,
            option_prefix: "-",
            short_options: true,
            // Hidden commands and options are omitted, as in cobra.
            complete_options: true,
            complete_subcommands: self.root_commands.is_some(),
            descriptions: true,
            query_dialect: None,
            trust,
        }
    }

    pub fn capabilities_for(&self, words: &[&str], trust: Trust) -> Capabilities {
        let mut capabilities = self.capabilities(trust);
        // Below the root, the slot may hold a command's argument, and click
        // completes only arguments with declared choices or callbacks.
        if words.iter().any(|word| !word.starts_with('-')) {
            capabilities.complete_subcommands = false;
        }
        capabilities
    }

    pub fn resource(&self, words: &[&str], item: &CompletionItem) -> bool {
        // The wire format has no enum/resource distinction. Only root names
        // listed by own help are known commands; all other plain values may
        // be local resources even when HTTP access is disabled.
        !item.is_option()
            && !(words.is_empty()
                && self
                    .root_commands
                    .as_ref()
                    .is_some_and(|commands| commands.contains(&item.value)))
    }

    fn query(
        &self,
        backend: &Backend,
        words: &[&str],
        incomplete: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let text = self.raw(backend, words, incomplete, "zsh_complete", budget)?;
        let mut items = match parse(&text, budget.max_candidates) {
            Ok(items) => items,
            Err(error) => {
                // Click prints descriptions without escaping newlines.
                // An application's multiline help breaks the zsh framing;
                // use a separately validated literal answer without help.
                let text = self.raw(backend, words, incomplete, "bash_complete", budget)?;
                return parse_bash(&text, budget.max_candidates).map_err(|_| error);
            }
        };
        // Click 8.2.2+ escapes described colons for zsh's `_describe`.
        // Older versions can return a literal backslash before a colon.
        // Verify ambiguous bytes with the library's literal bash format;
        // neither shell is launched. Changing callback lists fail closed.
        if items.iter().any(|item| item.value.contains(r"\:")) {
            let text = self.raw(backend, words, incomplete, "bash_complete", budget)?;
            restore_literals(&mut items, &text, budget.max_candidates)?;
        }
        Ok(items)
    }

    fn raw(
        &self,
        backend: &Backend,
        words: &[&str],
        incomplete: &str,
        instruction: &str,
        budget: &mut Budget,
    ) -> Result<String, CompletionError> {
        if app_fingerprint(&backend.completer) != self.application_fingerprint {
            return Err(CompletionError::Failed(
                "the app changed after its click completion evidence was read".into(),
            ));
        }
        if words
            .iter()
            .chain([&incomplete])
            .any(|word| word.contains(char::is_control))
        {
            return Err(CompletionError::Unsupported(
                "click completion context contains control characters".into(),
            ));
        }
        // shlex-quoted words survive click's split, including empty words.
        let mut line = crate::shlex::quote(&backend.name).into_owned();
        for word in words {
            line.push(' ');
            line.push_str(&crate::shlex::quote(word));
        }
        if !incomplete.is_empty() {
            line.push(' ');
            line.push_str(&crate::shlex::quote(incomplete));
        }
        let mut key = vec!["click", instruction];
        key.extend_from_slice(words);
        key.push(incomplete);
        backend.memoized(&key, || {
            let mut env = super::offline_env(Flavor::Click);
            env.extend([
                (
                    OsString::from(&self.variable),
                    Some(OsString::from(instruction)),
                ),
                ("COMP_WORDS".into(), Some(line.into())),
                (
                    "COMP_CWORD".into(),
                    Some((words.len() + 1).to_string().into()),
                ),
            ]);
            run_stdout(&backend.completer, Vec::new(), env, budget, true)
        })
    }

    pub fn complete(
        &self,
        backend: &Backend,
        words: &[&str],
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let mut items = self.query(backend, words, prefix, budget)?;
        items.retain(|item| item.value.starts_with(prefix));
        Ok(items)
    }

    /// An option's values, when it takes one. A flag leaves the slot to the
    /// next argument or command, so its answer repeats the parent's.
    pub fn values(
        &self,
        backend: &Backend,
        words: &[&str],
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let Some((option, parent)) = words.split_last() else {
            return Ok(Vec::new());
        };
        if !option.starts_with('-') {
            return Ok(Vec::new());
        }
        let values = self.query(backend, words, "", budget)?;
        if self.query(backend, parent, "", budget)? == values {
            return Err(CompletionError::Unsupported(format!(
                "{option} takes no value that click completes"
            )));
        }
        Ok(values)
    }
}

fn parse(text: &str, limit: usize) -> Result<Vec<CompletionItem>, CompletionError> {
    let mut items: Vec<CompletionItem> = Vec::new();
    if text == "\n" {
        // Click's echo adds one newline to an empty completion answer.
        return Ok(items);
    }
    let mut lines = text.split_terminator('\n');
    while let Some(kind) = lines.next() {
        let (Some(value), Some(description)) = (lines.next(), lines.next()) else {
            return Err(CompletionError::Failed(
                "click completion returned an incomplete record".into(),
            ));
        };
        match kind {
            // The shell would complete paths here; no words are listed.
            "file" | "dir" => continue,
            "plain" => {}
            _ => {
                return Err(CompletionError::Failed(format!(
                    "click completion returned an unknown item type {kind:?}"
                )));
            }
        }
        // Values are literal bytes. The edit layer quotes spaces, commas,
        // quotes, Unicode, and shell operators for the user's shell.
        if value.is_empty()
            || value.contains(char::is_control)
            || items.iter().any(|item| item.value == value)
        {
            continue;
        }
        if items.len() >= limit {
            return Err(CompletionError::Failed(
                "click completion exceeded the candidate limit".into(),
            ));
        }
        items.push(CompletionItem {
            value: value.to_owned(),
            takes_value: None,
            description: (!description.is_empty()
                && description != "_"
                && !description.contains(char::is_control))
            .then(|| description.to_owned()),
        });
    }
    Ok(items)
}

fn root_commands(help: &str) -> Option<Vec<String>> {
    let help = crate::engine::docs::strip_formatting(help);
    let mut commands = Vec::new();
    let mut section = false;
    let mut found = false;
    for line in help.lines() {
        if line.trim() == "Commands:" {
            section = true;
            found = true;
            continue;
        }
        if !section || line.trim().is_empty() {
            continue;
        }
        if !line.starts_with("  ") {
            section = false;
            continue;
        }
        if line.starts_with("   ") {
            continue;
        }
        let name = line.split_whitespace().next()?;
        if !name.contains(char::is_control) && !commands.iter().any(|c| c == name) {
            commands.push(name.to_owned());
        }
    }
    found.then_some(commands)
}

fn parse_bash(bash: &str, limit: usize) -> Result<Vec<CompletionItem>, CompletionError> {
    if bash == "\n" {
        return Ok(Vec::new());
    }
    // Convert the literal bash answer into the same validated records.
    let mut records = String::new();
    for line in bash.split_terminator('\n') {
        let Some((kind, value)) = line.split_once(',') else {
            return Err(CompletionError::Failed(
                "click literal verification returned a line without a type".into(),
            ));
        };
        records.push_str(kind);
        records.push('\n');
        records.push_str(value);
        records.push_str("\n_\n");
    }
    parse(&records, limit)
}

fn restore_literals(
    items: &mut [CompletionItem],
    bash: &str,
    limit: usize,
) -> Result<(), CompletionError> {
    let literal = parse_bash(bash, limit)?;
    let same = items.len() == literal.len()
        && items
            .iter()
            .zip(&literal)
            .all(|(item, literal)| item.value == literal.value);
    let escaped = items.len() == literal.len()
        && items.iter().zip(&literal).all(|(item, literal)| {
            item.value
                == if item.description.is_some() {
                    literal.value.replace(':', r"\:")
                } else {
                    literal.value.clone()
                }
        });
    if !same && !escaped {
        return Err(CompletionError::Failed(
            "click completion changed during literal verification".into(),
        ));
    }
    for (item, literal) in items.iter_mut().zip(literal) {
        item.value = literal.value;
    }
    Ok(())
}

/// Help-based discovery for a trusted Python console script. Only `--help`
/// runs; no completion variable is set unless the help shows click's.
impl Protocol {
    /// Root command names from the app's own `--help`, for a protocol found
    /// through its completion script when that help is trusted.
    pub(super) fn read_root_commands(&mut self, path: &Path, budget: &mut Budget) {
        if let Ok(help) = run_stdout(
            path,
            vec!["--help".into()],
            super::offline_env(Flavor::Click),
            budget,
            true,
        ) {
            self.root_commands = root_commands(&help);
        }
    }
}

pub(super) fn discover_from_help(
    path: &Path,
    identity: Option<&str>,
    budget: &mut Budget,
) -> Result<Option<Protocol>, CompletionError> {
    let before = app_fingerprint(path);
    let help = run_stdout(
        path,
        vec!["--help".into()],
        super::offline_env(Flavor::Click),
        budget,
        true,
    )?;
    let module = identity
        .and_then(|identity| identity.strip_prefix("python:"))
        .and_then(|module| module.split('.').next());
    let protocol = Protocol::from_help(&help, path).or_else(|| {
        module
            .is_some_and(|module| click_8_beside(path, module))
            .then(|| Protocol::from_usage(&help, path, true))
            .flatten()
    });
    let Some(protocol) = protocol else {
        return Ok(None);
    };
    if protocol.application_fingerprint != before {
        return Err(CompletionError::Failed(
            "the application changed during click discovery".into(),
        ));
    }
    Ok(Some(protocol))
}

/// Whether click 8 is installed in the site-packages that holds `module`,
/// the console script's own top-level package, for the interpreter its
/// shebang names: a virtualenv or a Homebrew `libexec`. Only directory
/// names are read.
fn click_8_beside(script: &Path, module: &str) -> bool {
    let Some(site) = site_packages(script, module) else {
        return false;
    };
    std::fs::read_dir(&site).is_ok_and(|entries| {
        entries.filter_map(Result::ok).take(4096).any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("click-8.") && name.ends_with(".dist-info")
        })
    })
}

fn site_packages(script: &Path, module: &str) -> Option<PathBuf> {
    if module.is_empty()
        || !module
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return None;
    }
    let real = std::fs::canonicalize(script).ok()?;
    let head = super::head(&real, 4096);
    let interpreter = super::identity::python_interpreter(&head)?;
    let prefix = Path::new(&interpreter).parent()?.parent()?;
    let mut libs: Vec<PathBuf> = std::fs::read_dir(prefix.join("lib"))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|lib| {
            lib.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("python"))
        })
        .take(64)
        .collect();
    libs.sort();
    libs.into_iter()
        .map(|lib| lib.join("site-packages"))
        .find(|site| site.join(module).is_dir() || site.join(format!("{module}.py")).is_file())
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::tests::Dir;
    use super::super::{
        Discovery, NativeCompletionBackend, discover, discover_for_shell_with_budget,
    };
    use super::*;
    use crate::shells::Shell;
    use std::path::PathBuf;
    use std::time::Duration;

    // As click 8.5.0 generated them for a console script named clickfix.
    const BASH: &str = r#"_clickfix_completion() {
    local IFS=$'\n'
    local response

    response=$(env COMP_WORDS="${COMP_WORDS[*]}" COMP_CWORD=$COMP_CWORD _CLICKFIX_COMPLETE=bash_complete $1)

    for completion in $response; do
        IFS=',' read type value <<< "$completion"
    done
    return 0
}

_clickfix_completion_setup() {
    complete -o nosort -F _clickfix_completion clickfix
}

_clickfix_completion_setup;
"#;
    const ZSH: &str = r#"#compdef clickfix

_clickfix_completion() {
    local -a response
    (( ! $+commands[clickfix] )) && return 1

    response=("${(@f)$(env COMP_WORDS="${words[*]}" COMP_CWORD=$((CURRENT-1)) _CLICKFIX_COMPLETE=zsh_complete clickfix)}")
}

if [[ $zsh_eval_context[-1] == loadautofunc ]]; then
    _clickfix_completion "$@"
else
    compdef _clickfix_completion clickfix
fi
"#;
    const FISH: &str = r#"function _clickfix_completion;
    set -l response (env _CLICKFIX_COMPLETE=fish_complete COMP_WORDS=(commandline -cp) COMP_CWORD=(commandline -t) clickfix);
end;

complete --no-files --command clickfix --arguments "(_clickfix_completion)";
"#;
    // click 8.0's fish layout collected the answer in a loop.
    const FISH_8_0: &str = r#"function _clickfix_completion;
    set -l response;

    for value in (env _CLICKFIX_COMPLETE=fish_complete COMP_WORDS=(commandline -cp) COMP_CWORD=(commandline -t) clickfix);
        set response $response $value;
    end;
end;

complete --no-files --command clickfix --arguments "(_clickfix_completion)";
"#;
    const HELP: &str = "Usage: clickfix [OPTIONS] COMMAND [ARGS]...\n\n  Fixture app.\n\nOptions:\n  -c, --config TEXT    Config file\n  --help               Show this message and exit.\n\nCommands:\n  db:migrate  Run migrations.\n  deploy      Deploy an app.\n";

    /// A stand-in for Python running a click app: completion answers like
    /// click 8.1.8 did for the same command tree; anything else is a run.
    const PYTHON: &str = r#"#!/bin/sh
root=$(dirname "$0")
[ "$1" = -m ] && [ "$2" = clickfix ] || exit 8
shift 2
printf '[%s]' "$@" >> "$root/calls"
printf '%s|%s|%s\n' "$_CLICKFIX_COMPLETE" "$COMP_CWORD" "$COMP_WORDS" >> "$root/calls"
[ "$PYTHONDONTWRITEBYTECODE" = 1 ] && [ "$HTTPS_PROXY" = http://127.0.0.1:9 ] || exit 8
if [ "$*" = --help ] && [ -z "$_CLICKFIX_COMPLETE" ]; then cat "$root/help"; exit 0; fi
[ "$#" = 0 ] || { touch "$root/operation-marker"; exit 2; }
case "$_CLICKFIX_COMPLETE" in zsh_complete|bash_complete) ;; *) touch "$root/operation-marker"; exit 2;; esac
emit() {
  if [ "$_CLICKFIX_COMPLETE" = bash_complete ]; then
    awk 'NR % 3 == 1 { kind = $0 } NR % 3 == 2 { printf "%s,%s\n", kind, $0 }'
  else cat; fi
}
if [ -f "$root/mode" ]; then
  case "$(cat "$root/mode")" in
    unknown) printf 'plain\ndeploy\n_\nmystery\nvalue\n_\n' | emit; exit 0;;
    failed) printf 'plain\ndeploy\n_\n'; exit 1;;
    utf8) printf 'plain\n\377\n_\n'; exit 0;;
    truncated) if [ "$_CLICKFIX_COMPLETE" = bash_complete ]; then printf 'missing-type\n'; else printf 'plain\ndeploy\n_\nplain\nmissing-help\n'; fi; exit 0;;
    multiline) if [ "$_CLICKFIX_COMPLETE" = bash_complete ]; then printf 'plain,deploy\nplain,with space\n'; else printf 'plain\ndeploy\nHelp\n\nContinued help\nplain\nwith space\n_\n'; fi; exit 0;;
    hanging) (sleep 1; touch "$root/delayed-marker") & wait; exit 0;;
  esac
fi
w="$COMP_WORDS "
w="${w#* }"
w="${w% }"
case "$COMP_CWORD|$w" in
  '1|-') printf 'plain\n--config\nConfig file\nplain\n-c\nConfig file\nplain\n--verbose\n_\nplain\n--help\nHelp\n';;
  '2|deploy') printf 'plain\nstatus\nShow status\n';;
  '3|deploy status') printf 'plain\nprod\n_\nplain\nstaging\n_\n';;
  '3|deploy status -') printf 'plain\n--watch\n_\nplain\n--format\nOutput format\nplain\n--help\nHelp\n';;
  '4|deploy status --format') printf 'plain\njson\n_\nplain\nyaml\n_\n';;
  '2|open') printf 'file\n\n_\n';;
  *) cat "$root/root";;
esac | emit
"#;

    fn app(dir: &Dir, name: &str) -> PathBuf {
        dir.script("python3", PYTHON);
        dir.script("help", HELP);
        dir.script("root", "plain\ndb:migrate\nRun migrations\nplain\ndeploy\nDeploy an app\nplain\nwith space\n_\n");
        dir.script(
            name,
            &format!(
                "#!/bin/sh\nexec {}/python3 -m clickfix \"$@\"\n",
                dir.0.display()
            ),
        )
    }

    fn calls(dir: &Dir) -> String {
        std::fs::read_to_string(dir.0.join("calls")).unwrap_or_default()
    }

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(10), Duration::from_secs(5), 16)
    }

    fn found(dir: &Dir) -> Backend {
        let path = app(dir, "clickfix");
        let protocol = Protocol::from_help(HELP, &path).unwrap();
        backend("clickfix", &path, Some("python:clickfix".into()), protocol)
    }

    fn values(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.value.as_str()).collect()
    }

    /// oci-cli rewords click's help option; click 8 beside the app's own
    /// module, in its interpreter's site-packages, plus click's usage line
    /// stand in for the signature.
    #[test]
    fn click_usage_counts_with_click_8_beside_the_apps_module() {
        let dir = Dir::new("click-site");
        let python = dir.script("venv/bin/python3", "");
        let script = dir.script(
            "venv/bin/app",
            &format!(
                "#!{}\nimport sys\nfrom app_mod.cli import cli\nsys.exit(cli())\n",
                python.display()
            ),
        );
        let site = dir.0.join("venv/lib/python3.14/site-packages");
        dir.script("venv/lib/python3.14/site-packages/app_mod/__init__.py", "");
        assert!(!click_8_beside(&script, "app_mod"), "no click installed");
        std::fs::create_dir_all(site.join("click-7.1.2.dist-info")).unwrap();
        assert!(
            !click_8_beside(&script, "app_mod"),
            "click 7 speaks another protocol"
        );
        std::fs::create_dir_all(site.join("click-8.4.2.dist-info")).unwrap();
        assert!(click_8_beside(&script, "app_mod"));
        assert!(
            !click_8_beside(&script, "other_mod"),
            "not the app's site-packages"
        );
        assert!(!click_8_beside(&script, "../app_mod"));
        let help = "Usage: app [OPTIONS] [COMMAND] [ARGS]...\n\nOptions:\n  -?, -h, --help  For detailed help, enter <command> --help.\n";
        assert_eq!(Protocol::from_help(help, &script), None);
        assert_eq!(
            Protocol::from_usage(help, &script, true).map(|p| p.variable),
            Some("_APP_COMPLETE".to_owned())
        );
        assert_eq!(
            Protocol::from_usage("usage: app [-h] {a,b}\n", &script, true),
            None,
            "argparse's usage line"
        );
        assert_eq!(
            Protocol::from_usage("Usage: app COMMAND\n", &script, true),
            None
        );
    }

    #[test]
    fn click_8_scripts_are_recognized_only_with_their_registration() {
        let dir = Dir::new("click-scripts");
        let path = app(&dir, "clickfix");
        for script in [
            BASH,
            ZSH,
            FISH,
            FISH_8_0,
            &ZSH.replace("clickfix\nfi", "clickfix;\nfi"),
        ] {
            let protocol = Protocol::from_script(script, "clickfix", &path).unwrap();
            assert_eq!(protocol.variable, "_CLICKFIX_COMPLETE");
        }
        for (script, why) in [
            (BASH.replace("-F _clickfix_completion clickfix", "-F _clickfix_completion other"), "unbound"),
            (ZSH.replace("compdef _clickfix_completion clickfix", "compdef _x other"), "unbound"),
            (ZSH.replace("zsh_complete clickfix", "zsh_complete other"), "other program"),
            (FISH.replace("--command clickfix", "--command other"), "unbound"),
            (BASH.replace("_CLICKFIX_COMPLETE=", "PATH="), "unsafe variable"),
            (format!("{BASH}{ZSH}"), "two query lines"),
            // click 7 used different instructions and output.
            (
                "#compdef clickfix\n_clickfix_completion() {\n  response=(\"${(@f)$( env COMP_WORDS=\"${words[*]}\" COMP_CWORD=$((CURRENT-1)) _CLICKFIX_COMPLETE=\"complete_zsh\" clickfix )}\")\n}\ncompdef _clickfix_completion clickfix\n".to_owned(),
                "click 7",
            ),
        ] {
            assert_eq!(Protocol::from_script(&script, "clickfix", &path), None, "{why}");
        }
        assert!(calls(&dir).is_empty(), "scripts are read, never run");
    }

    #[test]
    fn help_reveals_click_and_the_program_name_it_uses() {
        let path = PathBuf::from("/usr/bin/true");
        let variable = |help: &str| Protocol::from_help(help, &path).map(|p| p.variable);
        assert_eq!(variable(HELP).as_deref(), Some("_CLICKFIX_COMPLETE"));
        let typer = "\n Usage: my-tool.py [OPTIONS] COMMAND [ARGS]...\n\n╭─ Options ─────────╮\n│ --help          Show this message and exit. │\n╰───────────────────╯\n";
        assert_eq!(variable(typer).as_deref(), Some("_MY_TOOL_PY_COMPLETE"));
        let argparse =
            "usage: tool [-h]\n\noptions:\n  -h, --help  show this help message and exit\n";
        assert_eq!(variable(argparse), None);
        assert_eq!(
            variable("Usage: /opt/x/tool [OPTIONS]\n  --help  Show this message and exit.\n"),
            None
        );
        assert_eq!(variable("Show this message and exit.\n"), None);
    }

    #[test]
    fn queries_quote_words_and_read_only_plain_items_without_arguments() {
        let dir = Dir::new("click-queries");
        let backend = found(&dir);
        let mut budget = budget();
        assert_eq!(
            values(&backend.complete(&[], "", &mut budget).unwrap()),
            ["db:migrate", "deploy", "with space"]
        );
        assert_eq!(
            values(&backend.complete(&[], "-", &mut budget).unwrap()),
            ["--config", "-c", "--verbose", "--help"]
        );
        assert_eq!(
            values(
                &backend
                    .complete(&["deploy", "status"], "", &mut budget)
                    .unwrap()
            ),
            ["prod", "staging"]
        );
        assert!(
            backend
                .complete(&["open"], "", &mut budget)
                .unwrap()
                .is_empty()
        );
        backend
            .complete(
                &["a b", "", "it's", "$(touch operation-marker)"],
                "",
                &mut budget,
            )
            .unwrap();
        let calls = calls(&dir);
        assert!(
            calls.contains(
                r#"zsh_complete|5|clickfix 'a b' '' 'it'"'"'s' '$(touch operation-marker)'"#
            ),
            "{calls}"
        );
        assert!(
            calls
                .lines()
                .all(|line| line.starts_with("[]zsh_complete|")),
            "no arguments reach the app: {calls}"
        );
        assert!(!dir.0.join("operation-marker").exists());
        assert!(backend.capabilities_for(&[]).complete_subcommands);
        assert!(
            backend
                .capabilities_for(&["--verbose"])
                .complete_subcommands
        );
        assert!(!backend.capabilities_for(&["deploy"]).complete_subcommands);
        assert_eq!(backend.cache_identity(), None);
    }

    #[test]
    fn values_need_an_option_that_changes_the_slot() {
        let dir = Dir::new("click-values");
        let backend = found(&dir);
        let mut budget = budget();
        assert_eq!(
            values(
                &backend
                    .complete_values(&["deploy", "status", "--format"], &mut budget)
                    .unwrap()
            ),
            ["json", "yaml"]
        );
        // A flag leaves the slot to the next command: the root's answer.
        assert!(matches!(
            backend.complete_values(&["--verbose"], &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
    }

    #[test]
    fn records_preserve_literal_values_and_descriptions_and_reject_bad_framing() {
        let literal = "team 😀/it's $(touch marker), a;b";
        let items = parse(
            &format!("plain\n{literal}\nCurrent team\nfile\n\n_\ndir\nignored\n_\nplain\njson\n_\nplain\n{literal}\nDuplicate\n"),
            3,
        )
        .unwrap();
        assert_eq!(values(&items), [literal, "json"]);
        assert_eq!(items[0].description.as_deref(), Some("Current team"));
        assert_eq!(items[1].description, None);
        assert!(parse("", 1).unwrap().is_empty());
        assert!(parse("\n", 1).unwrap().is_empty());
        assert!(
            parse("plain\n\n_\nplain\na\tb\n_\nplain\na\rb\n_\n", 1)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            parse("plain\nword\n\x1b[31mUnsafe\n", 1).unwrap()[0].description,
            None
        );
        for bad in [
            "plain,word\n",
            "plain\nword\n",
            "plain\nword\n_\nplain\n",
            "plain\nline\nbreak\n_\n",
            "mystery\nword\n_\n",
        ] {
            assert!(parse(bad, 8).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn plain_values_need_confirmation_without_help_evidence_of_root_commands() {
        let dir = Dir::new("click-resources");
        let backend = found(&dir);
        let mut budget = budget();
        let root = backend.complete(&[], "", &mut budget).unwrap();
        assert!(!backend.candidate_is_resource(&[], &root[0]));
        assert!(!backend.candidate_is_resource(&[], &root[1]));
        assert!(backend.candidate_is_resource(&[], &root[2]));
        assert!(backend.candidate_is_resource(&["deploy"], &root[1]));
        let options = backend.complete(&[], "-", &mut budget).unwrap();
        assert!(!backend.candidate_is_resource(&[], &options[0]));
        let path = dir.0.join("clickfix");
        let script = Protocol::from_script(BASH, "clickfix", &path).unwrap();
        assert!(!script.capabilities(Trust::UserTrusted).complete_subcommands);
        assert!(script.resource(&[], &root[1]));
        // mkdocs's bash script with trusted help: its root commands are known.
        let mut script = script;
        script.read_root_commands(&path, &mut budget);
        assert!(script.capabilities(Trust::UserTrusted).complete_subcommands);
        assert!(!script.resource(&[], &root[1]));
        assert!(script.resource(&[], &root[2]));
        assert_eq!(
            root_commands(
                "Commands:\n  first  Wrapped description\n      continuation\n  db:migrate  Second\n\nOptions:\n  --bad  Ignored\n"
            ),
            Some(vec!["first".into(), "db:migrate".into()])
        );
    }

    #[test]
    fn colon_verification_preserves_old_and_new_formats_and_literal_backslashes() {
        let bash = "plain,team:one\nplain,team\\:two\nplain,team:three\n";
        for wire in [
            "plain\nteam:one\nTeam\nplain\nteam\\:two\nTeam\nplain\nteam:three\n_\n",
            "plain\nteam\\:one\nTeam\nplain\nteam\\\\:two\nTeam\nplain\nteam:three\n_\n",
        ] {
            let mut items = parse(wire, 4).unwrap();
            restore_literals(&mut items, bash, 4).unwrap();
            assert_eq!(values(&items), ["team:one", r"team\:two", "team:three"]);
            assert_eq!(items[0].description.as_deref(), Some("Team"));
        }
        let mut items = parse("plain\nteam\\:one\nTeam\n", 1).unwrap();
        for bad in [
            "plain,other\n",
            "plain,team:one\nplain,extra\n",
            "plain",
            "mystery,team:one\n",
        ] {
            assert!(restore_literals(&mut items, bad, 2).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn multiline_help_uses_validated_literal_answers_without_descriptions() {
        let dir = Dir::new("click-multiline");
        let backend = found(&dir);
        dir.script("mode", "multiline");
        let mut budget = budget();
        let items = backend.complete(&[], "", &mut budget).unwrap();
        assert_eq!(values(&items), ["deploy", "with space"]);
        assert!(items.iter().all(|item| item.description.is_none()));
        assert_eq!(budget.spawned(), 2);
        backend.complete(&[], "", &mut budget).unwrap();
        assert_eq!(budget.spawned(), 2, "both formats are memoized");
        assert!(!dir.0.join("operation-marker").exists());
        for bad in ["plain", "plain,word\ncontinued value\n", "mystery,word\n"] {
            assert!(parse_bash(bad, 3).is_err());
        }
    }

    #[test]
    fn failures_changed_apps_and_unrepresentable_words_fail_closed() {
        for (mode, why) in [
            ("unknown", "unknown item type"),
            ("failed", "exited"),
            ("utf8", "UTF-8"),
            ("truncated", "incomplete record"),
            ("hanging", "timed out"),
        ] {
            let dir = Dir::new("click-failures");
            let backend = found(&dir);
            dir.script("mode", mode);
            let per_probe = if mode == "hanging" {
                Duration::from_millis(300)
            } else {
                Duration::from_secs(2)
            };
            let mut budget = Budget::new(Duration::from_secs(3), per_probe, 2);
            let result = backend.complete(&[], "", &mut budget);
            assert!(
                matches!(&result, Err(CompletionError::Failed(message)) if message.contains(why)),
                "{mode}: {result:?}"
            );
            if mode == "hanging" {
                std::thread::sleep(Duration::from_millis(1100));
                assert!(!dir.0.join("delayed-marker").exists());
            }
        }
        let dir = Dir::new("click-limits");
        let backend = found(&dir);
        let mut budget = budget();
        budget.max_candidates = 1;
        assert!(matches!(
            backend.complete(&[], "", &mut budget),
            Err(CompletionError::Failed(why)) if why.contains("limit")
        ));
        assert!(matches!(
            backend.complete(&["line\nbreak"], "", &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
        dir.script(
            "clickfix",
            "#!/bin/sh\n# replaced by another program\nexit 0\n",
        );
        assert!(matches!(
            backend.complete(&["deploy"], "", &mut budget),
            Err(CompletionError::Failed(why)) if why.contains("changed")
        ));
    }

    #[test]
    fn console_scripts_need_both_trust_settings_and_click_help() {
        let dir = Dir::new("click-discovery");
        let path = app(&dir, "clickfix");
        let mut budget = budget();
        let discover_with = |trusted: &[&str], help: &[&str], budget: &mut Budget| {
            let trusted: Vec<String> = trusted.iter().map(|t| t.to_string()).collect();
            let help: Vec<String> = help.iter().map(|t| t.to_string()).collect();
            discover_for_shell_with_budget("clickfix", &path, &trusted, &help, Shell::Bash, budget)
        };
        for (trusted, help) in [
            (&[][..], &["clickfix"][..]),
            (&["python:clickfix"][..], &[][..]),
        ] {
            assert!(!matches!(
                discover_with(trusted, help, &mut budget),
                Discovery::Found(_)
            ));
        }
        assert!(
            calls(&dir).is_empty(),
            "nothing runs without both permissions"
        );
        let Discovery::Found(backend) =
            discover_with(&["python:clickfix"], &["python:clickfix"], &mut budget)
        else {
            panic!()
        };
        assert_eq!(backend.flavor, Flavor::Click);
        assert_eq!(backend.identity.as_deref(), Some("python:clickfix"));
        assert_eq!(calls(&dir), "[--help]||\n", "discovery only asks for help");
        dir.script(
            "help",
            "usage: clickfix [-h]\n  -h, --help  show this help message and exit\n",
        );
        assert!(!matches!(
            discover_with(&["python:clickfix"], &["clickfix"], &mut budget),
            Discovery::Found(_)
        ));
        assert!(!dir.0.join("operation-marker").exists());
        // A renamed launcher keeps the identity that trust names.
        dir.script("help", HELP);
        let renamed = dir.0.join("renamed");
        std::os::unix::fs::symlink(&path, &renamed).unwrap();
        assert!(matches!(
            discover("renamed", &renamed, &["python:clickfix".into()]),
            Discovery::None
        ));
    }

    #[test]
    fn the_engine_repairs_commands_arguments_and_values() {
        use crate::engine::{self, FailureContext, Outcome};
        use crate::settings::Settings;
        use crate::types::Context;
        let dir = Dir::new("click-engine");
        let path = app(&dir, "clickfix");
        let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into())
            .with_which("clickfix", Some(path.to_str().unwrap()))
            .with_which("man", None)
            .with_history::<&str>(&[]);
        let run = |source: &str| {
            let protocol = Protocol::from_help(HELP, &path).unwrap();
            let backend: std::rc::Rc<dyn NativeCompletionBackend> =
                std::rc::Rc::new(backend("clickfix", &path, None, protocol));
            let failure = FailureContext {
                source: source.into(),
                ..FailureContext::default()
            };
            engine::correct_with_backends(&failure, &ctx, vec![("clickfix".into(), backend)])
        };
        let report = run("clickfix deplyo");
        assert!(
            matches!(report.outcome, Outcome::Suggestion(_)),
            "{:?}",
            report.notes
        );
        assert_eq!(report.outcome.candidates()[0].script, "clickfix deploy");
        for (source, expected) in [
            ("clickfix deploy statsu prod", "clickfix deploy status prod"),
            ("clickfix deploy status prdo", "clickfix deploy status prod"),
            (
                "clickfix deploy status --format jsno prod",
                "clickfix deploy status --format json prod",
            ),
            ("clickfix --confg x deploy", "clickfix --config x deploy"),
        ] {
            let report = run(source);
            assert_eq!(
                report
                    .outcome
                    .candidates()
                    .first()
                    .map(|c| c.script.as_str()),
                Some(expected),
                "{source}: {:?}",
                report.notes
            );
        }
        assert!(!dir.0.join("operation-marker").exists());
    }

    #[test]
    fn new_commands_are_answered_on_the_next_request() {
        let dir = Dir::new("click-upgrade");
        let mut budget = budget();
        let before = found(&dir);
        assert!(!values(&before.complete(&[], "", &mut budget).unwrap()).contains(&"rollback"));
        dir.script(
            "root",
            "plain\ndb:migrate\n_\nplain\ndeploy\n_\nplain\nrollback\n_\n",
        );
        let path = dir.0.join("clickfix");
        let protocol = Protocol::from_help(HELP, &path).unwrap();
        let after = backend("clickfix", &path, None, protocol);
        assert!(values(&after.complete(&[], "", &mut budget).unwrap()).contains(&"rollback"));
    }
}
