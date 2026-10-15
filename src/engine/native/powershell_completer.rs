//! Argument completers registered in the user's PowerShell session with
//! `Register-ArgumentCompleter -Native` (or, as cobra's scripts do, without a
//! parameter name), usually by `<app> completion powershell | Out-String |
//! Invoke-Expression` in a profile.
//!
//! A registration lives only in the session's memory. The PowerShell
//! integration passes, for the command names of the failed line, each
//! completer's text with the script's `using namespace` lines, and the
//! function definitions of the script that created it (its root syntax
//! tree: the profile, a dot-sourced file, or the text given to
//! `Invoke-Expression`). The rest of that script never runs.
//!
//! A profile-free, non-interactive PowerShell defines those functions,
//! registers the completer under the queried name, and completes the line
//! as Tab would (`CommandCompletion.CompleteInput`, which `TabExpansion2`
//! calls). The completer is wrapped so the probe knows
//! whether it answered: PowerShell completes file names when a native
//! completer returns nothing, and those are not the app's words. Running a
//! completer runs its code, so this needs `trusted_completers`.

use super::{CompletionError, CompletionItem, run_stdout};
use crate::engine::parser::{self, Dialect};
use crate::engine::probe::Budget;
use std::ffi::OsString;
use std::path::Path;

/// The variable the PowerShell integration fills: a `#notypo-command <name>`
/// line before each completer's text, then a `#notypo-functions` line and
/// the namespaces and function definitions of the scripts that created them.
const MEMORY: &str = "NOTYPO_POWERSHELL_COMPLETIONS";
const COMMAND: &str = "#notypo-command ";
const FUNCTIONS: &str = "#notypo-functions";
const MEMORY_LIMIT: usize = 64 * 1024;

/// Passed with `-Command`, so it holds no double quotes. Each answer is one
/// tab-separated record per line; a record with a control character is left
/// out rather than misread. The session's functions are defined first and
/// the probe calls cmdlets by module-qualified name and asks
/// `CommandCompletion` (what `TabExpansion2` calls) directly, so a session
/// function can't stand in for the probe's own commands.
const SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
if ($env:NOTYPO_PS_LOCATION) { Microsoft.PowerShell.Management\Set-Location -LiteralPath $env:NOTYPO_PS_LOCATION }
if ($env:NOTYPO_PS_DEFINITIONS) { . ([scriptblock]::Create($env:NOTYPO_PS_DEFINITIONS)) }
try { [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false) } catch { }
$tab = [string][char]9
function __notypo_emit([string[]] $Fields) {
    foreach ($field in $Fields) { if ($field -match '[\x00-\x1f\x7f]') { return } }
    [Console]::Out.Write(($Fields -join $tab) + [char]10)
}
$global:NotypoCompleter = [scriptblock]::Create($env:NOTYPO_PS_COMPLETER)
$global:NotypoPrefix = [string]$env:NOTYPO_PS_PREFIX
$global:NotypoAnswered = $null
$global:NotypoFailure = $null
$registration = @{ CommandName = $env:NOTYPO_PS_NAME }
if ($PSVersionTable.PSVersion.Major -ge 6) { $registration.Native = $true }
Microsoft.PowerShell.Core\Register-ArgumentCompleter @registration -ScriptBlock {
    param($word, $ast, $cursor)
    $ErrorActionPreference = 'Stop'
    try {
        $results = @(& $global:NotypoCompleter $global:NotypoPrefix $ast $cursor 3>$null 4>$null 5>$null 6>$null)
        $global:NotypoAnswered = $results.Count
        $results
    } catch {
        $global:NotypoFailure = [string]$_
    }
}
$line = $env:NOTYPO_PS_LINE
$cursor = [int]$env:NOTYPO_PS_CURSOR
foreach ($name in 'NOTYPO_PS_DEFINITIONS', 'NOTYPO_PS_COMPLETER', 'NOTYPO_PS_NAME', 'NOTYPO_PS_LINE', 'NOTYPO_PS_CURSOR', 'NOTYPO_PS_PREFIX') {
    [Environment]::SetEnvironmentVariable($name, $null)
}
$result = [System.Management.Automation.CommandCompletion]::CompleteInput($line, $cursor, $null)
if ($null -ne $global:NotypoFailure) { __notypo_emit 'failed'; return }
if ($null -eq $global:NotypoAnswered) { __notypo_emit 'unanswered'; return }
__notypo_emit 'answered'
__notypo_emit 'range', ([string]$result.ReplacementIndex), ([string]$result.ReplacementLength)
if ($global:NotypoAnswered -eq 0) { return }
foreach ($match in $result.CompletionMatches) {
    __notypo_emit 'match', $match.CompletionText, ([string]$match.ResultType), ([string]$match.ToolTip -replace '[\x00-\x1f\x7f]', ' ')
}
"#;

/// What the session passed for one command name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registration {
    /// The completer's text, after its script's `using namespace` lines.
    pub completer: String,
    /// `using namespace` lines, then the function definitions of the
    /// scripts that registered the session's completers.
    pub definitions: String,
    /// Captured live objects cannot safely be moved into a probe process.
    pub unsupported: Option<String>,
}

/// The session's completer for `name`, compared without regard to case as
/// PowerShell compares command names.
pub(super) fn memory(name: &str) -> Option<Registration> {
    memory_in(&std::env::var(MEMORY).ok()?, name)
}

fn memory_in(text: &str, name: &str) -> Option<Registration> {
    if text.len() > MEMORY_LIMIT || text.contains('\0') {
        return None;
    }
    if text.trim_start().starts_with('{') {
        return snapshot(text, name);
    }
    let lines: Vec<&str> = text.lines().collect();
    let functions = lines.iter().position(|line| *line == FUNCTIONS)?;
    let (registrations, definitions) = lines.split_at(functions);
    let start = registrations.iter().position(|line| {
        line.strip_prefix(COMMAND)
            .is_some_and(|registered| registered.eq_ignore_ascii_case(name))
    })?;
    let body: Vec<&str> = registrations[start + 1..]
        .iter()
        .take_while(|line| !line.starts_with(COMMAND))
        .copied()
        .collect();
    let completer = body.join("\n");
    if completer.trim().is_empty() {
        return None;
    }
    Some(Registration {
        completer,
        definitions: definitions[1..].join("\n"),
        unsupported: None,
    })
}

/// Validate the entire packet, including registrations for other commands.
fn snapshot(text: &str, name: &str) -> Option<Registration> {
    let root: serde_json::Value = serde_json::from_str(text).ok()?;
    let fields = |value: &serde_json::Value, names: &[&str]| {
        value.as_object().is_some_and(|object| {
            object.len() == names.len() && names.iter().all(|name| object.contains_key(*name))
        })
    };
    if !fields(&root, &["version", "registrations"]) || root["version"] != 2 {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    let mut found = None;
    for entry in root["registrations"].as_array()? {
        if !fields(
            entry,
            &["name", "body", "definitions", "portable", "capture"],
        ) {
            return None;
        }
        let registered = entry["name"].as_str()?;
        let body = entry["body"].as_str()?;
        let definitions = entry["definitions"].as_str()?;
        let portable = entry["portable"].as_bool()?;
        if !super::powershell::is_command_name(registered)
            || !seen.insert(registered.to_ascii_lowercase())
            || body.trim().is_empty()
            || body.contains('\0')
            || definitions.contains('\0')
        {
            return None;
        }
        let capture = (!entry["capture"].is_null())
            .then(|| super::powershell_state::CapturedState::parse(&entry["capture"]))
            .transpose()
            .ok()?;
        let registration = match capture.filter(|_| portable) {
            Some(capture) => Registration {
                completer: capture.wrap_body(body).ok()?,
                definitions: String::new(),
                unsupported: None,
            },
            None => Registration {
                completer: body.into(),
                definitions: definitions.into(),
                unsupported: (!portable).then(|| {
                    "the PowerShell completer depends on unsupported captured state".into()
                }),
            },
        };
        if registered.eq_ignore_ascii_case(name) {
            found = Some(registration);
        }
    }
    found
}

/// The line Tab would complete: the command, its words, and the slot's
/// prefix, quoted as PowerShell reads arguments.
fn line(name: &str, words: &[&str], prefix: &str) -> String {
    let quote = |word: &str| parser::quote_word_with_dialect(word, Dialect::PowerShell);
    let mut line = quote(name);
    for word in words {
        line.push(' ');
        line.push_str(&quote(word));
    }
    line.push(' ');
    if !prefix.is_empty() {
        line.push_str(&quote(prefix));
    }
    line
}

/// The raw answer for one query line.
#[cfg(test)]
pub(super) fn query(
    shell: &Path,
    registration: &Registration,
    name: &str,
    words: &[&str],
    prefix: &str,
    env: Vec<(OsString, Option<OsString>)>,
    budget: &mut Budget,
) -> Result<String, CompletionError> {
    query_at(shell, registration, name, words, prefix, &[], env, budget)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn query_at(
    shell: &Path,
    registration: &Registration,
    name: &str,
    words: &[&str],
    prefix: &str,
    following: &[&str],
    mut env: Vec<(OsString, Option<OsString>)>,
    budget: &mut Budget,
) -> Result<String, CompletionError> {
    if let Some(reason) = &registration.unsupported {
        return Err(CompletionError::Unsupported(reason.clone()));
    }
    let mut query_line = line(name, words, prefix);
    // A literal placeholder keeps the target's AST position before a suffix.
    // The callback receives the actual empty prefix through the wrapper.
    let placeholder = prefix.is_empty() && !following.is_empty();
    if placeholder {
        query_line.push_str("''");
    }
    let target_end = query_line.len();
    let target_start = target_end
        - if placeholder {
            2
        } else if prefix.is_empty() {
            0
        } else {
            parser::quote_word_with_dialect(prefix, Dialect::PowerShell).len()
        };
    let cursor = query_line.encode_utf16().count().to_string();
    for word in following {
        query_line.push(' ');
        query_line.push_str(&parser::quote_word_with_dialect(word, Dialect::PowerShell));
    }
    let set = |k: &str, v: &str| (OsString::from(k), Some(OsString::from(v)));
    env.extend(super::powershell::environment(&[]));
    env.extend([
        set("NOTYPO_PS_DEFINITIONS", &registration.definitions),
        set("NOTYPO_PS_COMPLETER", &registration.completer),
        set("NOTYPO_PS_NAME", name),
        set("NOTYPO_PS_LINE", &query_line),
        set("NOTYPO_PS_CURSOR", &cursor),
        set("NOTYPO_PS_PREFIX", prefix),
    ]);
    let args = [
        "-NoProfile",
        "-NonInteractive",
        "-NoLogo",
        "-Command",
        SCRIPT,
    ]
    .map(OsString::from)
    .to_vec();
    let text = run_stdout(shell, args, env, budget, true)?;
    normalize_replacement(
        &text,
        &query_line,
        parser::Span {
            start: target_start,
            end: target_end,
        },
    )
}

/// CommandCompletion reports UTF-16 offsets into the query, while edits in
/// our AST use UTF-8 byte spans. Only the queried word may be replaced: a
/// completer that selects a preceding argument cannot authorize editing it.
/// https://learn.microsoft.com/dotnet/api/system.management.automation.commandcompletion
pub(super) fn normalize_replacement(
    text: &str,
    source: &str,
    target: parser::Span,
) -> Result<String, CompletionError> {
    let text = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n");
    let Some(records) = text.strip_prefix("answered\n") else {
        return Ok(text);
    };
    let (header, records) = records.split_once('\n').unwrap_or((records, ""));
    let fields: Vec<&str> = header.split('\t').collect();
    let ["range", start, length] = fields.as_slice() else {
        return Err(CompletionError::Failed(
            "missing PowerShell completion replacement range".into(),
        ));
    };
    // PowerShell may use sentinel offsets when it has no completions.
    if *start == "-1" && *length == "-1" && records.is_empty() {
        return Ok("answered\n".into());
    }
    let byte_at = |offset: usize| {
        let mut units = 0;
        for (byte, character) in source.char_indices() {
            if units == offset {
                return Some(byte);
            }
            units += character.len_utf16();
        }
        (units == offset).then_some(source.len())
    };
    let normalized = start
        .parse::<usize>()
        .ok()
        .zip(length.parse::<usize>().ok())
        .and_then(|(start, length)| start.checked_add(length).map(|end| (start, end)))
        .and_then(|(start, end)| byte_at(start).zip(byte_at(end)))
        .map(|(start, end)| parser::Span { start, end });
    if normalized != Some(target) {
        return Err(CompletionError::Unsupported(
            "PowerShell completion replaces a different source span".into(),
        ));
    }
    Ok(format!("answered\n{records}"))
}

/// Reads the probe's records. Each completion text is PowerShell code, the
/// text Tab would insert, so it is read as PowerShell reads an argument: a
/// text that isn't exactly one literal word (`my file`, `$x`) is left out.
/// File names and PowerShell's own kinds of results are not the app's words.
pub(super) fn parse(
    text: &str,
    name: &str,
    limit: usize,
) -> Result<Vec<CompletionItem>, CompletionError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line));
    match lines.next() {
        Some("answered") => {}
        Some("unanswered") => {
            return Err(CompletionError::Unsupported(format!(
                "PowerShell did not call your session's completer for {name}"
            )));
        }
        Some("failed") => {
            return Err(CompletionError::Failed(format!(
                "your session's PowerShell completer for {name} failed"
            )));
        }
        _ => {
            return Err(CompletionError::Failed(
                "unexpected PowerShell completion answer".into(),
            ));
        }
    }
    let mut items: Vec<CompletionItem> = Vec::new();
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        let ["match", completion, kind, tooltip] = fields.as_slice() else {
            return Err(CompletionError::Failed(format!(
                "unexpected PowerShell completion record `{line}`"
            )));
        };
        if !matches!(*kind, "ParameterName" | "ParameterValue" | "Text") {
            continue;
        }
        let Some(value) = literal(completion) else {
            continue;
        };
        if value.is_empty() || value.contains(char::is_control) {
            continue;
        }
        if items.iter().any(|item| item.value == value) {
            continue;
        }
        // A plain string's tooltip is the string itself.
        let description =
            super::clean_description(tooltip).filter(|d| *d != value && d != completion.trim());
        items.push(match value.strip_suffix('=') {
            Some(option) if option.starts_with('-') => CompletionItem {
                value: option.to_owned(),
                takes_value: Some(true),
                description,
            },
            _ => CompletionItem {
                value,
                takes_value: None,
                description,
            },
        });
        if items.len() > limit {
            return Err(super::over_limit());
        }
    }
    Ok(items)
}

/// The one argument `code` passes to a native program, when it is literal.
pub(super) fn literal(code: &str) -> Option<String> {
    let source = format!("x {code}");
    let script = parser::parse_with_dialect(&source, Dialect::PowerShell);
    if !script.is_fully_supported() || script.commands.len() != 1 {
        return None;
    }
    let command = &script.commands[0];
    if !command.redirections.is_empty() || !command.assignments.is_empty() {
        return None;
    }
    match command.words.as_slice() {
        [_, word] => word.literal().map(str::to_owned),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Workspace(std::path::PathBuf);

    impl Workspace {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("notypo-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn session_completers_are_found_by_name_without_regard_to_case() {
        let text = "#notypo-command rustup\nusing namespace System.Management.Automation\n\
                    param($w)\n'install'\n#notypo-command Tool\n__tool_words\n\
                    #notypo-functions\nusing namespace System.Management.Automation\n\
                    function __tool_words { 'build' }";
        let rustup = memory_in(text, "rustup").unwrap();
        assert_eq!(
            rustup.completer,
            "using namespace System.Management.Automation\nparam($w)\n'install'"
        );
        assert_eq!(
            rustup.definitions,
            "using namespace System.Management.Automation\nfunction __tool_words { 'build' }"
        );
        assert_eq!(memory_in(text, "TOOL").unwrap().completer, "__tool_words");
        assert_eq!(memory_in(text, "other"), None);
        assert_eq!(memory_in(text, "too"), None, "names match whole");
        assert_eq!(
            memory_in("#notypo-command tool\n'x'\n", "tool"),
            None,
            "no functions marker"
        );
        assert_eq!(
            memory_in("#notypo-command tool\n  \n#notypo-functions\n", "tool"),
            None,
            "an empty completer"
        );
        let huge = format!("{text}\n{}", "#".repeat(MEMORY_LIMIT));
        assert_eq!(memory_in(&huge, "tool"), None);
    }

    #[test]
    fn versioned_snapshots_validate_every_registration_before_use() {
        let entry = serde_json::json!({
            "name":"tool", "body":"param($word) 'build'", "definitions":"",
            "portable":true, "capture":null
        });
        let packet = |entries| serde_json::json!({"version":2,"registrations":entries}).to_string();
        assert_eq!(
            memory_in(&packet(vec![entry.clone()]), "TOOL")
                .unwrap()
                .completer,
            "param($word) 'build'"
        );
        let mut unsupported = entry.clone();
        unsupported["portable"] = false.into();
        assert!(
            memory_in(&packet(vec![unsupported]), "tool")
                .unwrap()
                .unsupported
                .is_some()
        );
        let mut invalid = entry.clone();
        invalid["name"] = "other".into();
        invalid["capture"] = serde_json::json!({"module":"","definitions":"","variables":[{
            "name":"state","scope":"script","value":{"kind":"Object","value":"code"}
        }]});
        assert!(memory_in(&packet(vec![entry.clone(), invalid]), "tool").is_none());
        assert!(memory_in(&packet(vec![entry.clone(), entry]), "tool").is_none());
    }

    #[test]
    fn real_snapshots_restore_native_closures_and_private_helpers_without_reinitializing() {
        let Some(pwsh) = super::super::powershell::tests::pwsh() else {
            return;
        };
        let dir = Workspace::new("ps-native-capture");
        let marker = dir.0.join("initialized");
        for kind in ["closure", "module"] {
            let setup = match kind {
                "closure" => "$callback = & { $team = 'west'; { param($word,$ast,$cursor) ($team + '-one') }.GetNewClosure() }; Register-ArgumentCompleter -CommandName tool -ScriptBlock $callback".to_owned(),
                _ => format!(
                    "$null = New-Module -Name NativeFixture -ScriptBlock {{ [IO.File]::AppendAllText('{}','once'); $script:teams = [string[]]@('west-one','west-two'); function Get-Teams {{ $script:teams }}; Register-ArgumentCompleter -CommandName tool -ScriptBlock {{ param($word,$ast,$cursor) Get-Teams }} }}",
                    marker.to_string_lossy().replace('\'', "''")
                ),
            };
            let setup_path = dir.0.join("setup.ps1");
            std::fs::write(&setup_path, setup).unwrap();
            let source = format!(
                ". '{}'\n$history = 'tool wesst-one'\n{}\n[Console]::Out.Write($env:NOTYPO_POWERSHELL_COMPLETIONS)",
                setup_path.to_string_lossy().replace('\'', "''"),
                crate::shells::POWERSHELL_COMPLETERS
            );
            let script = dir.0.join("capture.ps1");
            std::fs::write(&script, source).unwrap();
            let output = std::process::Command::new(&pwsh)
                .args(["-NoProfile", "-NonInteractive", "-File"])
                .arg(&script)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8(output.stdout).unwrap();
            let registration = memory_in(&text, "tool").unwrap_or_else(|| {
                panic!(
                    "{kind}: {text}\n{}",
                    String::from_utf8_lossy(&output.stderr)
                )
            });
            assert!(registration.unsupported.is_none(), "{kind}: {text}");
            let mut budget = Budget::new(Duration::from_secs(20), Duration::from_secs(5), 8);
            let reply = query(
                &pwsh,
                &registration,
                "tool",
                &[],
                "",
                Vec::new(),
                &mut budget,
            )
            .unwrap();
            let items = parse(&reply, "tool", 10).unwrap();
            assert_eq!(items[0].value, "west-one", "{kind}: {reply}");
            if kind == "module" {
                assert_eq!(items.len(), 2);
                assert_eq!(std::fs::read_to_string(&marker).unwrap(), "once");
            }
        }
    }

    #[test]
    fn unsupported_objects_are_not_inspected_or_run_during_snapshot_export() {
        let Some(pwsh) = super::super::powershell::tests::pwsh() else {
            return;
        };
        let dir = Workspace::new("ps-native-object");
        let marker = dir.0.join("getter-ran");
        let setup = format!(
            "$callback = & {{ $state = [pscustomobject]@{{ Name='west' }}; $state | Add-Member -Force -MemberType ScriptMethod -Name GetType -Value {{ [IO.File]::WriteAllText('{}','method'); [string] }}; $state | Add-Member -MemberType ScriptProperty -Name Secret -Value {{ [IO.File]::WriteAllText('{}','getter'); 'west' }}; {{ param($word,$ast,$cursor) $state.Secret }}.GetNewClosure() }}; Register-ArgumentCompleter -CommandName tool -ScriptBlock $callback",
            marker.to_string_lossy().replace('\'', "''"),
            marker.to_string_lossy().replace('\'', "''")
        );
        let setup_path = dir.0.join("setup.ps1");
        std::fs::write(&setup_path, setup).unwrap();
        let source = format!(
            ". '{}'\n$history = 'tool wesst-one'\n{}\n[Console]::Out.Write($env:NOTYPO_POWERSHELL_COMPLETIONS)",
            setup_path.to_string_lossy().replace('\'', "''"),
            crate::shells::POWERSHELL_COMPLETERS
        );
        let script = dir.0.join("capture.ps1");
        std::fs::write(&script, source).unwrap();
        let output = std::process::Command::new(&pwsh)
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(&script)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        let registration = memory_in(&text, "tool")
            .unwrap_or_else(|| panic!("{text}\n{}", String::from_utf8_lossy(&output.stderr)));
        let mut budget = Budget::new(Duration::from_secs(20), Duration::from_secs(5), 8);
        assert!(matches!(
            query(
                &pwsh,
                &registration,
                "tool",
                &[],
                "",
                Vec::new(),
                &mut budget
            ),
            Err(CompletionError::Unsupported(_))
        ));
        assert_eq!(budget.spawned(), 0);
        assert!(!marker.exists(), "snapshot export invoked an object's code");
    }

    #[test]
    fn query_lines_quote_words_as_powershell_reads_them() {
        assert_eq!(line("tool", &[], ""), "tool ");
        assert_eq!(line("tool", &["sub"], "-"), "tool sub -");
        assert_eq!(line("tool", &["a b", "it's"], "x"), "tool 'a b' 'it''s' x");
    }

    #[test]
    fn replacement_ranges_use_utf16_and_must_match_the_target_word() {
        let source = "tool '🦀' --";
        let target = parser::Span {
            start: source.len() - 2,
            end: source.len(),
        };
        let start = source[..target.start].encode_utf16().count();
        let reply =
            format!("answered\nrange\t{start}\t2\nmatch\t--verbose\tParameterName\tVerbose\n");
        let normalized = normalize_replacement(&reply, source, target).unwrap();
        assert_eq!(
            parse(&normalized, "tool", 10).unwrap()[0].value,
            "--verbose"
        );
        for reply in [
            "answered\nrange\t0\t4\nmatch\tx\tText\t\n",
            "answered\nrange\t7\t1\nmatch\tx\tText\t\n", // middle of a surrogate pair
            "answered\nrange\t999\t1\nmatch\tx\tText\t\n",
            "answered\nmatch\tx\tText\t\n",
        ] {
            assert!(
                normalize_replacement(reply, source, target).is_err(),
                "{reply}"
            );
        }
    }

    #[test]
    fn answers_keep_the_literal_words_tab_would_insert() {
        let text = "answered\n\
                    match\t--verbose\tParameterName\tUse verbose output\n\
                    match\tbuild \tParameterValue\tCompile the current package\n\
                    match\t'with space'\tParameterValue\tx\n\
                    match\tmy file\tText\tmy file\n\
                    match\t'it''s'\tParameterValue\t \n\
                    match\tx`$y\tText\tx`$y\n\
                    match\t$env:HOME\tText\t\n\
                    match\t./src\tProviderContainer\tsrc\n\
                    match\t--target=\tParameterName\t\n\
                    match\tbuild\tParameterValue\tagain\n\
                    match\tGet-Item\tCommand\t\n";
        let items = parse(text, "tool", 16).unwrap();
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(
            values,
            [
                "--verbose",
                "build",
                "with space",
                "it's",
                "x$y",
                "--target"
            ]
        );
        assert_eq!(items[0].description.as_deref(), Some("Use verbose output"));
        assert_eq!(items[4].description, None, "a tooltip repeating the word");
        assert_eq!(items[5].takes_value, Some(true));
        assert!(parse("answered\n", "tool", 16).unwrap().is_empty());
        assert!(matches!(
            parse("unanswered\n", "tool", 16),
            Err(CompletionError::Unsupported(_))
        ));
        assert!(matches!(
            parse("failed\n", "tool", 16),
            Err(CompletionError::Failed(_))
        ));
        for broken in [
            "",
            "match\tx\tText\t\n",
            "answered\nsurprise\n",
            "answered\nmatch\tx\n",
        ] {
            assert!(parse(broken, "tool", 16).is_err(), "{broken:?}");
        }
        assert!(matches!(
            parse("answered\nmatch\ta\tText\t\nmatch\tb\tText\t\n", "tool", 1),
            Err(CompletionError::Failed(why)) if why.contains("limit")
        ));
    }

    #[test]
    fn script_has_no_double_quotes() {
        assert!(!SCRIPT.contains('"'));
    }

    #[test]
    fn middle_slots_give_native_callbacks_the_full_ast_and_original_cursor() {
        let Some(pwsh) = super::super::powershell::tests::pwsh() else {
            return;
        };
        let registration = Registration {
            completer: "param($w, $ast, $cursor) if ($w -eq '' -and $cursor -eq 12 -and $ast.CommandElements[2].Value -eq '' -and $ast.CommandElements[-1].Value -eq '三 west') { 'build' }".into(),
            definitions: String::new(),
            unsupported: None,
        };
        let mut budget = Budget::new(Duration::from_secs(20), Duration::from_secs(5), 8);
        let text = query_at(
            &pwsh,
            &registration,
            "tool",
            &["🦀"],
            "",
            &["--scope", "三 west"],
            Vec::new(),
            &mut budget,
        )
        .unwrap();
        assert_eq!(parse(&text, "tool", 10).unwrap()[0].value, "build");
    }

    /// The probe defines the session's functions, runs the completer as Tab
    /// does, and never runs the rest of the script that registered it; a
    /// session function named like one of the probe's commands isn't called.
    #[test]
    fn real_powershell_runs_the_session_completer_through_tab_expansion() {
        let Some(pwsh) = super::super::powershell::tests::pwsh() else {
            eprintln!("skipped: PowerShell is not installed");
            return;
        };
        let dir = std::env::temp_dir().join(format!("notypo-ps-completer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("marker");
        let registration = Registration {
            completer: "using namespace System.Management.Automation\n\
                        param($wordToComplete, $commandAst, $cursorPosition)\n\
                        $path = @($commandAst.CommandElements | Select-Object -Skip 1 | ForEach-Object { [string]$_ } | Where-Object { $_ -ne $wordToComplete }) -join ' '\n\
                        __tool_words $path | Where-Object { $_.CompletionText -like ($wordToComplete + '*') }"
                .into(),
            definitions: format!(
                "using namespace System.Management.Automation\n\
                 function __tool_words($path) {{ switch ($path) {{\n\
                 '' {{ [CompletionResult]::new('build', 'build', [CompletionResultType]::ParameterValue, 'Compile it'); [CompletionResult]::new('--verbose', '--verbose', [CompletionResultType]::ParameterName, 'Talk more') }}\n\
                 'build' {{ [CompletionResult]::new('--release', '--release', [CompletionResultType]::ParameterName, 'Optimize') }}\n\
                 }} }}\n\
                 function __tool_unrelated {{ Set-Content -Path '{marker}' -Value ran }}\n\
                 function Emit {{ Set-Content -Path '{marker}' -Value emit }}\n\
                 function TabExpansion2 {{ Set-Content -Path '{marker}' -Value tab }}\n\
                 function Register-ArgumentCompleter {{ Set-Content -Path '{marker}' -Value register }}",
                marker = marker.display()
            ),
            unsupported: None,
        };
        let mut budget = Budget::new(Duration::from_secs(60), Duration::from_secs(30), 16);
        let ask = |words: &[&str], prefix: &str, budget: &mut Budget| {
            let text = query(
                &pwsh,
                &registration,
                "tool",
                words,
                prefix,
                Vec::new(),
                budget,
            )?;
            parse(&text, "tool", 16)
        };
        let root = ask(&[], "", &mut budget).unwrap();
        assert_eq!(
            root.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(),
            ["build", "--verbose"]
        );
        assert_eq!(root[0].description.as_deref(), Some("Compile it"));
        let nested = ask(&["build"], "-", &mut budget).unwrap();
        assert_eq!(nested[0].value, "--release");
        // Nothing listed for this path: PowerShell's file names don't count.
        assert!(ask(&["nothing"], "", &mut budget).unwrap().is_empty());
        for body in [
            "param($w) throw 'broken'",
            "param($w) 'build'; Write-Error 'broken'",
        ] {
            let broken = Registration {
                completer: body.into(),
                definitions: String::new(),
                unsupported: None,
            };
            let text = query(&pwsh, &broken, "tool", &[], "", Vec::new(), &mut budget).unwrap();
            assert!(matches!(
                parse(&text, "tool", 16),
                Err(CompletionError::Failed(_))
            ));
        }
        assert!(!marker.exists(), "only definitions ran");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
