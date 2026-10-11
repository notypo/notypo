//! Parameter-value callbacks of PowerShell commands. The completion engine
//! supplies the command AST and its fake bound parameters; the command body
//! never runs. Callback results remain resources, even for offline callbacks.

use super::powershell::{Binding, Description, Parameter};
use super::{CompletionError, CompletionItem};
use crate::engine::probe::Budget;
use std::ffi::OsString;
use std::path::Path;

pub(super) const MEMORY: &str = "NOTYPO_POWERSHELL_PARAMETERS";

#[derive(Clone, Debug, Default)]
pub(super) struct Snapshot {
    pub session: bool,
    pub portable: bool,
    pub definitions: String,
    registrations: Vec<Registration>,
}

#[derive(Clone, Debug)]
struct Registration {
    parameter: String,
    specific: bool,
    portable: bool,
    body: String,
}

fn invalid() -> CompletionError {
    CompletionError::Failed("invalid PowerShell parameter-completer snapshot".into())
}

pub(super) fn memory(name: &str) -> Result<Option<Snapshot>, CompletionError> {
    match std::env::var(MEMORY) {
        Ok(text) if !text.is_empty() => parse_snapshot(&text, name),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(invalid()),
    }
}

fn parse_snapshot(text: &str, name: &str) -> Result<Option<Snapshot>, CompletionError> {
    if text.len() > 64 * 1024 || text.contains('\0') {
        return Err(invalid());
    }
    let root: serde_json::Value = serde_json::from_str(text).map_err(|_| invalid())?;
    if root["version"].as_u64() != Some(1) {
        return Err(invalid());
    }
    let mut found = None;
    let mut names: Vec<String> = Vec::new();
    for command in root["commands"].as_array().ok_or_else(invalid)? {
        let registered = command["name"].as_str().ok_or_else(invalid)?;
        if !super::powershell::is_command_name(registered)
            || names.iter().any(|n| n.eq_ignore_ascii_case(registered))
        {
            return Err(invalid());
        }
        names.push(registered.to_owned());
        let mut snapshot = Snapshot {
            session: command["session"].as_bool().ok_or_else(invalid)?,
            portable: command["portable"].as_bool().ok_or_else(invalid)?,
            definitions: command["definitions"]
                .as_str()
                .ok_or_else(invalid)?
                .to_owned(),
            registrations: Vec::new(),
        };
        if snapshot.definitions.contains('\0') {
            return Err(invalid());
        }
        for entry in command["registrations"].as_array().ok_or_else(invalid)? {
            let registration = Registration {
                parameter: entry["parameter"].as_str().ok_or_else(invalid)?.to_owned(),
                specific: entry["specific"].as_bool().ok_or_else(invalid)?,
                portable: entry["portable"].as_bool().ok_or_else(invalid)?,
                body: entry["body"].as_str().ok_or_else(invalid)?.to_owned(),
            };
            if !super::powershell::parameter_name(&registration.parameter)
                || registration.body.trim().is_empty()
                || registration.body.contains('\0')
                || snapshot.registrations.iter().any(|r| {
                    r.parameter.eq_ignore_ascii_case(&registration.parameter)
                        && r.specific == registration.specific
                })
            {
                return Err(invalid());
            }
            snapshot.registrations.push(registration);
        }
        if registered.eq_ignore_ascii_case(name) {
            found = Some(snapshot);
        }
    }
    Ok(found)
}

impl Snapshot {
    fn registration(&self, parameter: &str) -> Option<&Registration> {
        self.registrations
            .iter()
            .filter(|r| r.parameter.eq_ignore_ascii_case(parameter))
            .max_by_key(|r| r.specific)
    }

    pub fn has_registration(&self, parameter: &str) -> bool {
        self.registration(parameter).is_some()
    }
}

// Fixed code, query data in the environment. Qualified cmdlets prevent a
// restored function from replacing any of the probe's own commands.
const SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
try { [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false) } catch { }
if ($env:NOTYPO_PS_DEFINITIONS) { . ([scriptblock]::Create($env:NOTYPO_PS_DEFINITIONS)) }
$lookup = @{ Name = [WildcardPattern]::Escape($env:NOTYPO_PS_NAME); CommandType = @('Function','Filter','Cmdlet'); ErrorAction = 'Stop' }
if ($env:NOTYPO_PS_MODULE) { $lookup.Module = $env:NOTYPO_PS_MODULE }
$info = Microsoft.PowerShell.Core\Get-Command @lookup |
    Microsoft.PowerShell.Utility\Select-Object -First 1
$parameter = $info.Parameters[$env:NOTYPO_PS_PARAMETER]
if (-not $parameter -or $parameter.SwitchParameter) { [Console]::Out.WriteLine('unanswered'); return }
$global:NotypoOriginal = $null
$global:NotypoInstance = $null
if ($env:NOTYPO_PS_COMPLETER) {
    $global:NotypoOriginal = [scriptblock]::Create($env:NOTYPO_PS_COMPLETER)
} else {
    $reflection = [System.Reflection.BindingFlags]'NonPublic,Instance'
    $context = $ExecutionContext.GetType().GetField('_context', $reflection).GetValue($ExecutionContext)
    $registered = $context.GetType().GetProperty('CustomArgumentCompleters', $reflection).GetValue($context)
    foreach ($key in @(($info.Name + ':' + $parameter.Name), $parameter.Name)) {
        $block = $null
        if ($registered -and $registered.TryGetValue($key, [ref]$block) -and $block) { $global:NotypoOriginal = $block; break }
    }
    if (-not $global:NotypoOriginal) { foreach ($attribute in $parameter.Attributes) {
        if ($attribute -is [System.Management.Automation.ArgumentCompleterAttribute]) {
            if ($attribute -is [System.Management.Automation.ArgumentCompleterFactoryAttribute]) {
                $global:NotypoInstance = $attribute.Create()
            } elseif ($attribute.Type) {
                $global:NotypoInstance = [Activator]::CreateInstance($attribute.Type)
            } else { $global:NotypoOriginal = $attribute.ScriptBlock }
            break
        }
    } }
    if (-not $global:NotypoOriginal -and -not $global:NotypoInstance) {
        foreach ($attribute in $parameter.Attributes) {
            if ($attribute -is [System.Management.Automation.ArgumentCompletionsAttribute]) { $global:NotypoInstance = $attribute; break }
        }
    }
}
if (-not $global:NotypoOriginal -and -not $global:NotypoInstance) { [Console]::Out.WriteLine('unanswered'); return }
$global:NotypoAnswered = $null
$global:NotypoFailure = $false
$wrapper = {
    param($commandName, $parameterName, $word, $ast, $bound)
    $ErrorActionPreference = 'Stop'
    try {
        # CompleteInput passes two quote characters for the empty literal
        # slot used before following arguments. Enumeration queries always
        # give the callback an empty prefix, as for a slot at end of line.
        $arguments = @($commandName, $parameterName, '', $ast, $bound)
        $results = @(if ($global:NotypoOriginal) {
            & $global:NotypoOriginal @arguments 3>$null 4>$null 5>$null 6>$null
        } else {
            $method = if ($global:NotypoInstance -is [System.Management.Automation.IArgumentCompleter]) {
                [System.Management.Automation.IArgumentCompleter].GetMethod('CompleteArgument')
            } else { $global:NotypoInstance.GetType().GetMethod('CompleteArgument') }
            $method.Invoke($global:NotypoInstance, $arguments)
        })
        $global:NotypoAnswered = $results.Count
        $results
    } catch { $global:NotypoFailure = $true }
}
$callbacks = @{}
$callbacks[$info.Name + ':' + $parameter.Name] = $wrapper
$line = $env:NOTYPO_PS_LINE
$cursor = [int]$env:NOTYPO_PS_CURSOR
foreach ($name in 'NOTYPO_PS_DEFINITIONS', 'NOTYPO_PS_COMPLETER', 'NOTYPO_PS_NAME', 'NOTYPO_PS_MODULE', 'NOTYPO_PS_PARAMETER', 'NOTYPO_PS_LINE', 'NOTYPO_PS_CURSOR') {
    [Environment]::SetEnvironmentVariable($name, $null)
}
$result = [System.Management.Automation.CommandCompletion]::CompleteInput($line, $cursor, @{ CustomArgumentCompleters = $callbacks })
if ($global:NotypoFailure) { [Console]::Out.WriteLine('failed'); return }
if ($null -eq $global:NotypoAnswered) { [Console]::Out.WriteLine('unanswered'); return }
[Console]::Out.WriteLine('answered')
if ($global:NotypoAnswered -eq 0) { return }
foreach ($match in $result.CompletionMatches) {
    $fields = @('match', $match.CompletionText, [string]$match.ResultType, ([string]$match.ToolTip -replace '[\x00-\x1f\x7f]', ' '))
    if (@($fields | Microsoft.PowerShell.Core\Where-Object { $_ -match '[\x00-\x1f\x7f]' }).Count) { continue }
    [Console]::Out.Write(($fields -join [char]9) + [char]10)
}
"#;

fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn command_name(description: &Description) -> Result<String, CompletionError> {
    if description.module.is_empty() {
        return Ok(description.name.clone());
    }
    if !super::powershell::is_command_name(&description.module) {
        return Err(CompletionError::Unsupported(
            "unsupported PowerShell module name in completion context".into(),
        ));
    }
    Ok(format!("{}\\{}", description.module, description.name))
}

/// Preserve parameter syntax; all data words are single-quoted, including
/// dash-prefixed values. Attaching a value quotes only the part after `:`.
#[cfg(test)]
fn line(description: &Description, words: &[&str]) -> Result<String, CompletionError> {
    line_with_literals(description, words, &[])
}

fn line_with_literals(
    description: &Description,
    words: &[&str],
    literals: &[usize],
) -> Result<String, CompletionError> {
    let mut line = command_name(description)?;
    let mut pending = false;
    for (index, word) in words.iter().enumerate() {
        if word.contains(char::is_control) {
            return Err(CompletionError::Unsupported(
                "control character in PowerShell context".into(),
            ));
        }
        line.push(' ');
        if pending
            && !literals.contains(&index)
            && word
                .strip_prefix('-')
                .is_some_and(|w| w.starts_with(|c: char| c.is_alphabetic() || "_?".contains(c)))
        {
            return Err(CompletionError::Unsupported(
                "PowerShell context parameter is missing its value".into(),
            ));
        }
        if pending || literals.contains(&index) {
            line.push_str(&quoted(word));
            pending = false;
        } else {
            let (name, attached) = word
                .split_once(':')
                .map_or((*word, None), |(n, v)| (n, Some(v)));
            if word.starts_with('-')
                && let Binding::Bound(parameter) = description.bind(name)
            {
                line.push('-');
                line.push_str(&parameter.name);
                if let Some(value) = attached {
                    line.push(':');
                    line.push_str(&quoted(value));
                } else {
                    pending = !parameter.switch;
                }
            } else {
                if word.starts_with('-') {
                    return Err(CompletionError::Unsupported(
                        "unresolved PowerShell parameter in completion context".into(),
                    ));
                }
                line.push_str(&quoted(word));
            }
        }
    }
    line.push(' ');
    Ok(line)
}

/// Ask PowerShell which parameter owns a literal positional slot. Static
/// binding inspects the AST, including named arguments after the slot; it
/// does not invoke the command, its validators, or its completers.
const BIND_SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
try { [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false) } catch { }
if ($env:NOTYPO_PS_DEFINITIONS) { . ([scriptblock]::Create($env:NOTYPO_PS_DEFINITIONS)) }
$lookup = @{ Name = [WildcardPattern]::Escape($env:NOTYPO_PS_NAME); CommandType = @('Function','Filter','Cmdlet'); ErrorAction = 'Stop' }
if ($env:NOTYPO_PS_MODULE) { $lookup.Module = $env:NOTYPO_PS_MODULE }
$null = Microsoft.PowerShell.Core\Get-Command @lookup
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($env:NOTYPO_PS_LINE, [ref]$null, [ref]$errors)
if ($errors.Count) { [Console]::Out.WriteLine('unbound'); return }
$call = $ast.Find({ $args[0] -is [System.Management.Automation.Language.CommandAst] }, $true)
$result = [System.Management.Automation.Language.StaticParameterBinder]::BindCommand($call, $true)
if ($result.BindingExceptions.Count) { [Console]::Out.WriteLine('unbound'); return }
$cursor = [int]$env:NOTYPO_PS_CURSOR
$names = @(foreach ($entry in $result.BoundParameters.GetEnumerator()) {
    $value = $entry.Value.Value
    if ($value -and $value.Extent.StartOffset -eq ($cursor - 2) -and $value.Extent.EndOffset -eq $cursor) { $entry.Key }
})
if ($names.Count -ne 1) { [Console]::Out.WriteLine('unbound'); return }
[Console]::Out.WriteLine('bound' + [char]9 + $names[0])
"#;

pub(super) fn bind_positional(
    shell: &Path,
    description: &Description,
    snapshot: Option<&Snapshot>,
    context: &super::ValueContext<'_>,
    budget: &mut Budget,
) -> Result<Option<Parameter>, CompletionError> {
    if snapshot.is_some_and(|s| !s.portable) {
        return Err(CompletionError::Unsupported(
            "PowerShell command depends on uncaptured session state".into(),
        ));
    }
    let mut query_line =
        line_with_literals(description, context.words, context.literal_arguments.0)?;
    query_line.push_str("''");
    let cursor = query_line.encode_utf16().count().to_string();
    let suffix = line_with_literals(description, context.following, context.literal_arguments.1)?;
    query_line.push_str(&suffix[command_name(description)?.len()..]);
    let env = super::powershell::environment(&[
        ("NOTYPO_PS_NAME", &description.name),
        ("NOTYPO_PS_MODULE", &description.module),
        ("NOTYPO_PS_LINE", &query_line),
        ("NOTYPO_PS_CURSOR", &cursor),
        (
            "NOTYPO_PS_DEFINITIONS",
            snapshot.map_or("", |s| s.definitions.as_str()),
        ),
    ]);
    let args = [
        "-NoProfile",
        "-NonInteractive",
        "-NoLogo",
        "-Command",
        BIND_SCRIPT,
    ]
    .map(OsString::from)
    .to_vec();
    let text = super::run_stdout(shell, args, env, budget, true)?;
    parse_binding(&text, description)
}

fn parse_binding(
    text: &str,
    description: &Description,
) -> Result<Option<Parameter>, CompletionError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r'));
    let first = lines.next();
    if lines.next().is_none() {
        if first == Some("unbound") {
            return Ok(None);
        }
        if let Some(name) = first.and_then(|l| l.strip_prefix("bound\t"))
            && let Some(parameter) = description.parameters.iter().find(|p| p.name == name)
            && parameter.positional
            && !parameter.switch
        {
            return Ok(Some(parameter.clone()));
        }
    }
    Err(CompletionError::Failed(
        "unexpected PowerShell positional binding answer".into(),
    ))
}

#[cfg(test)]
pub(super) fn query(
    shell: &Path,
    description: &Description,
    parameter: &Parameter,
    snapshot: Option<&Snapshot>,
    words: &[&str],
    following: &[&str],
    budget: &mut Budget,
) -> Result<Vec<CompletionItem>, CompletionError> {
    query_context(
        shell,
        description,
        parameter,
        snapshot,
        &super::ValueContext {
            words,
            following,
            literal_arguments: (&[], &[]),
            positional: false,
        },
        budget,
    )
}

pub(super) fn query_context(
    shell: &Path,
    description: &Description,
    parameter: &Parameter,
    snapshot: Option<&Snapshot>,
    context: &super::ValueContext<'_>,
    budget: &mut Budget,
) -> Result<Vec<CompletionItem>, CompletionError> {
    let registration = snapshot.and_then(|s| s.registration(&parameter.name));
    if snapshot.is_some_and(|s| !s.portable) || registration.is_some_and(|r| !r.portable) {
        return Err(CompletionError::Unsupported(
            "PowerShell completer depends on uncaptured session state".into(),
        ));
    }
    let mut query_line =
        line_with_literals(description, context.words, context.literal_arguments.0)?;
    if !context.following.is_empty() {
        query_line.push_str("''");
    }
    let cursor = query_line.encode_utf16().count().to_string();
    if !context.following.is_empty() {
        let suffix =
            line_with_literals(description, context.following, context.literal_arguments.1)?;
        query_line.push_str(&suffix[command_name(description)?.len()..]);
    }
    let env = super::powershell::environment(&[
        ("NOTYPO_PS_NAME", &description.name),
        ("NOTYPO_PS_MODULE", &description.module),
        ("NOTYPO_PS_PARAMETER", &parameter.name),
        ("NOTYPO_PS_LINE", &query_line),
        ("NOTYPO_PS_CURSOR", &cursor),
        (
            "NOTYPO_PS_DEFINITIONS",
            snapshot.map_or("", |s| s.definitions.as_str()),
        ),
        (
            "NOTYPO_PS_COMPLETER",
            registration.map_or("", |r| r.body.as_str()),
        ),
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
    let text = super::run_stdout(shell, args, env, budget, true)?;
    parse(&text, budget.max_candidates)
}

fn parse(text: &str, limit: usize) -> Result<Vec<CompletionItem>, CompletionError> {
    let mut lines = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .lines()
        .map(|l| l.trim_end_matches('\r'));
    match lines.next() {
        Some("answered") => {}
        Some("failed") => {
            return Err(CompletionError::Failed(
                "PowerShell parameter completer failed".into(),
            ));
        }
        Some("unanswered") => {
            return Err(CompletionError::Unsupported(
                "PowerShell did not call the parameter completer".into(),
            ));
        }
        _ => {
            return Err(CompletionError::Failed(
                "unexpected PowerShell parameter completion answer".into(),
            ));
        }
    }
    let mut items: Vec<CompletionItem> = Vec::new();
    for record in lines {
        let fields: Vec<&str> = record.split('\t').collect();
        let ["match", completion, kind, tooltip] = fields.as_slice() else {
            return Err(CompletionError::Failed(
                "unexpected PowerShell parameter completion record".into(),
            ));
        };
        if !matches!(*kind, "ParameterValue" | "Text") {
            continue;
        }
        let Some(value) = super::powershell_completer::literal(completion) else {
            continue;
        };
        if value.is_empty()
            || value.contains(char::is_control)
            || items.iter().any(|i| i.value == value)
        {
            continue;
        }
        let description =
            super::clean_description(tooltip).filter(|d| *d != value && d != completion.trim());
        items.push(CompletionItem {
            value,
            takes_value: None,
            description,
        });
        if items.len() > limit {
            return Err(super::over_limit());
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn parameter(name: &str) -> Parameter {
        Parameter {
            name: name.into(),
            ..Parameter::default()
        }
    }

    #[test]
    fn snapshots_validate_all_records_and_specific_callbacks_win() {
        let entry = |specific, body| serde_json::json!({"parameter":"Target", "specific":specific, "portable":true, "body":body});
        let root = serde_json::json!({"version":1,"commands":[{
            "name":"Invoke-Fixture", "session":true, "portable":true,
            "definitions":"function Invoke-Fixture { param($Target) }",
            "registrations":[entry(false,"'global'"),entry(true,"'specific'")]
        }]});
        let snapshot = parse_snapshot(&root.to_string(), "invoke-fixture")
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.registration("TARGET").unwrap().body, "'specific'");
        assert!(
            parse_snapshot(&root.to_string(), "Get-Other")
                .unwrap()
                .is_none()
        );
        for broken in ["", "null", "{}", "{\"version\":2,\"commands\":[]}"] {
            assert!(parse_snapshot(broken, "Invoke-Fixture").is_err());
        }
        let mut broken = root.clone();
        broken["commands"][0]["definitions"] = "encoded\0nul".into();
        assert!(parse_snapshot(&broken.to_string(), "Get-Other").is_err());
        let mut broken = root.clone();
        broken["commands"][0]["registrations"][1] = entry(false, "'duplicate'");
        assert!(parse_snapshot(&broken.to_string(), "Invoke-Fixture").is_err());
        let mut broken = root.clone();
        broken["commands"]
            .as_array_mut()
            .unwrap()
            .push(root["commands"][0].clone());
        assert!(parse_snapshot(&broken.to_string(), "Invoke-Fixture").is_err());
        assert!(parse_snapshot(&"x".repeat(65537), "Invoke-Fixture").is_err());
    }

    #[test]
    fn completion_lines_preserve_attached_values_switches_and_dash_data() {
        let description = Description {
            name: "Invoke-Fixture".into(),
            parameters: vec![
                Parameter {
                    switch: true,
                    ..parameter("Loud")
                },
                Parameter {
                    aliases: vec!["s".into()],
                    ..parameter("Scope")
                },
                parameter("Target"),
            ],
            ..Description::default()
        };
        assert_eq!(
            line(&description, &["-Loud", "-s:team one's", "-Target"]).unwrap(),
            "Invoke-Fixture -Loud -Scope:'team one''s' -Target "
        );
        assert_eq!(
            line_with_literals(&description, &["-Scope", "-west", "-Target"], &[1]).unwrap(),
            "Invoke-Fixture -Scope '-west' -Target "
        );
        assert_eq!(
            line(&description, &["$(touch marker)", "a;exit", "-Target"]).unwrap(),
            "Invoke-Fixture '$(touch marker)' 'a;exit' -Target "
        );
        assert!(line(&description, &["bad\ncontext", "-Target"]).is_err());
        assert!(line(&description, &["-Scope", "-Target"]).is_err());
        assert!(!SCRIPT.contains('"'));
        assert!(!BIND_SCRIPT.contains('"'));
        assert_eq!(
            line_with_literals(&description, &["-Target", "-Target"], &[0, 1]).unwrap(),
            "Invoke-Fixture '-Target' '-Target' "
        );
        let mut module = description.clone();
        module.module = "Fixture.Module".into();
        assert_eq!(
            line(&module, &["-Target"]).unwrap(),
            "Fixture.Module\\Invoke-Fixture -Target "
        );
        module.module = "bad;Invoke-Fixture".into();
        assert!(line(&module, &[]).is_err());
    }

    #[test]
    fn positional_binding_answers_reject_unknown_parameters_and_extra_records() {
        let description = Description {
            name: "Invoke-Fixture".into(),
            parameters: vec![
                Parameter {
                    positional: true,
                    ..parameter("Target")
                },
                parameter("Named"),
            ],
            ..Description::default()
        };
        assert_eq!(
            parse_binding("bound\tTarget\n", &description)
                .unwrap()
                .unwrap()
                .name,
            "Target"
        );
        assert!(parse_binding("unbound\n", &description).unwrap().is_none());
        for malformed in [
            "",
            "bound\tNamed\n",
            "bound\tForeign\n",
            "unbound\nnoise\n",
            "bound\tTarget\nbound\tTarget\n",
            "bound\tTarget\textra\n",
        ] {
            assert!(
                parse_binding(malformed, &description).is_err(),
                "{malformed}"
            );
        }
    }

    #[test]
    fn answers_keep_literal_words_and_descriptions_without_evaluating_code() {
        let answer = "answered\n\
            match\t'team one'\tParameterValue\tA team\n\
            match\t'it''s;exit'\tText\tA literal\n\
            match\tõun/üks\tParameterValue\tA region\n\
            match\t-dash\tParameterValue\tA selector\n\
            match\t$secret\tParameterValue\tUnsafe\n\
            match\t$(touch marker)\tParameterValue\tUnsafe\n\
            match\ta;exit\tParameterValue\tUnsafe\n\
            match\ttwo words\tParameterValue\tUnsafe\n\
            match\tfile\tProviderItem\tFallback\n\
            match\t'-dash'\tParameterValue\tDuplicate\n";
        let items = parse(answer, 4).unwrap();
        assert_eq!(
            items.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(),
            ["team one", "it's;exit", "õun/üks", "-dash"]
        );
        assert_eq!(items[0].description.as_deref(), Some("A team"));
        assert!(parse(answer, 3).is_err());
        assert!(parse("answered\n", 0).unwrap().is_empty());
        for broken in [
            "",
            "noise\nanswered\n",
            "answered\ntruncated",
            "failed\n",
            "unanswered\n",
        ] {
            assert!(parse(broken, 8).is_err(), "{broken}");
        }
    }

    #[test]
    fn real_parameter_callbacks_receive_bound_arguments_and_empty_results_stay_empty() {
        let Some(shell) = super::super::powershell::tests::pwsh() else {
            eprintln!("skipped: PowerShell is not installed");
            return;
        };
        let description = Description {
            name: "Invoke-Fixture".into(),
            parameters: vec![
                Parameter {
                    switch: true,
                    ..parameter("Loud")
                },
                Parameter {
                    aliases: vec!["s".into()],
                    ..parameter("Scope")
                },
                parameter("Target"),
            ],
            ..Description::default()
        };
        let original = Snapshot { portable:true, definitions:
            "function Choice { param($scope) if ($scope -eq 'east') { 'east-one' } else { 'west-one' } }\n\
             function Invoke-Fixture { [CmdletBinding()] param([switch]$Loud, [string]$Scope, \
             [ArgumentCompleter({ param($cmd,$param,$word,$ast,$bound); Choice $bound['Scope'] })][string]$Target) \
             throw 'the command must never run' }".into(), ..Snapshot::default() };
        let mut budget = Budget::new(Duration::from_secs(30), Duration::from_secs(5), 16);
        for (words, expected) in [
            (vec!["-Loud", "-s:east", "-Target"], "east-one"),
            (vec!["-Scope", "west", "-Target"], "west-one"),
        ] {
            let items = query(
                &shell,
                &description,
                &parameter("Target"),
                Some(&original),
                &words,
                &[],
                &mut budget,
            )
            .unwrap();
            assert_eq!(
                items.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(),
                [expected]
            );
        }
        for (body, succeeds) in [("", true), ("throw 'failed'", false), ("'$secret'", true)] {
            let mut snapshot = original.clone();
            snapshot.registrations.push(Registration {
                parameter: "Target".into(),
                specific: true,
                portable: true,
                body: format!("param($cmd,$param,$word,$ast,$bound); {body}"),
            });
            let result = query(
                &shell,
                &description,
                &parameter("Target"),
                Some(&snapshot),
                &["-Target"],
                &[],
                &mut budget,
            );
            if succeeds {
                assert!(result.unwrap().is_empty());
            } else {
                assert!(matches!(result, Err(CompletionError::Failed(_))));
            }
        }
        let mut unportable = original.clone();
        unportable.portable = false;
        let before = budget.spawned();
        assert!(
            query(
                &shell,
                &description,
                &parameter("Target"),
                Some(&unportable),
                &["-Target"],
                &[],
                &mut budget
            )
            .is_err()
        );
        assert_eq!(before, budget.spawned());
        let mut many = original.clone();
        many.registrations.push(Registration {
            parameter: "Target".into(),
            specific: true,
            portable: true,
            body: "param($a,$b,$c,$d,$e); 'one','two','three'".into(),
        });
        budget.max_candidates = 2;
        assert!(
            query(
                &shell,
                &description,
                &parameter("Target"),
                Some(&many),
                &["-Target"],
                &[],
                &mut budget
            )
            .is_err()
        );
    }
}
