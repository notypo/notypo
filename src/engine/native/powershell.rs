//! PowerShell's own commands (cmdlets, functions, and aliases), answered by
//! PowerShell from command metadata.
//!
//! A profile-free, non-interactive PowerShell (the parent session's own
//! executable when its function names it) answers two queries: the names of
//! the commands it can run, read from module manifests without importing
//! anything, and one command's parameters (names, aliases, switches, and
//! the values of enumerations and `ValidateSet`s). Describing a command
//! imports its module, which runs the module's code, so only commands built
//! into PowerShell or shipped under `$PSHOME` are described automatically;
//! another module needs `trusted_completers` to name it
//! (`powershell:<Module>`) or the command. The query travels in the
//! environment; the script itself is fixed.
//! Declared parameter completers and captured session registrations answer
//! value queries through PowerShell's own completion binder. Callback values
//! need confirmation and are never cached on disk.
//!
//! Parameter names follow PowerShell's binder: case doesn't matter, an alias
//! or a prefix of one parameter names it, and a prefix shared by several
//! names the command's own parameter when only one of them isn't a common
//! parameter (`-In` is `-Include`, not `-InformationAction`).

use super::powershell_values::{self, Snapshot};
use super::{Capabilities, CompletionError, CompletionItem, Trust, ValueContext, run_stdout};
use crate::engine::probe::{self, Budget};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Passed with `-Command`, so it holds no double quotes: Windows argument
/// quoting can't then change it. Output is one tab-separated record per
/// line; a record with a control character in any field is left out.
const SCRIPT: &str = concat!(
    r#"$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
try { [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false) } catch { }
"#,
    include_str!("powershell_location.ps1"),
    r#"
$tab = [string][char]9
function __notypo_emit([string[]] $Fields) {
    foreach ($field in $Fields) { if ($field -match '[\x00-\x1f\x7f]') { return } }
    [Console]::Out.Write(($Fields -join $tab) + [char]10)
}
$order = 'Alias', 'Function', 'Filter', 'Cmdlet'
if ($env:NOTYPO_PS_QUERY -eq 'list') {
    foreach ($command in Microsoft.PowerShell.Core\Get-Command -ListImported -CommandType $order -ErrorAction SilentlyContinue) {
        __notypo_emit 'command', $command.Name
    }
    foreach ($available in (Microsoft.PowerShell.Core\Get-Module -ListAvailable -ErrorAction SilentlyContinue)) {
        foreach ($exported in $available.ExportedCommands.Keys) { __notypo_emit 'command', $exported }
    }
    return
}
$name = $env:NOTYPO_PS_NAME
$trusted = @([string]$env:NOTYPO_PS_TRUSTED -split [char]10 | Microsoft.PowerShell.Core\Where-Object { $_ })
$session = $false
if ($env:NOTYPO_PS_DEFINITIONS) {
    if (-not ($trusted -contains '*' -or $trusted -contains ('command:' + $name))) { __notypo_emit 'untrusted', $name, 'session'; return }
    . ([scriptblock]::Create($env:NOTYPO_PS_DEFINITIONS))
    $session = $env:NOTYPO_PS_SESSION -eq '1'
}
function __notypo_emit([string[]] $Fields) {
    foreach ($field in $Fields) { if ($field -match '[\x00-\x1f\x7f]') { return } }
    [Console]::Out.Write(($Fields -join $tab) + [char]10)
}
$separator = [IO.Path]::DirectorySeparatorChar
$shippedRoot = [IO.Path]::GetFullPath($PSHOME).TrimEnd($separator) + $separator
for ($hop = 0; $hop -lt 4; $hop++) {
    # A wildcard lookup reads module manifests without importing modules.
    $pattern = $name.Substring(0, $name.Length - 1) + '[' + $name[-1] + ']'
    $command = Microsoft.PowerShell.Core\Get-Command -ListImported -Name $pattern -CommandType $order -ErrorAction SilentlyContinue |
        Microsoft.PowerShell.Core\Where-Object { $_.Name -eq $name } |
        Microsoft.PowerShell.Utility\Sort-Object { $order.IndexOf([string]$_.CommandType) } |
        Microsoft.PowerShell.Utility\Select-Object -First 1
    if (-not $command) {
        # An updated module search path may not yet be in Get-Command's index.
        # ListAvailable reads exports without importing the module's code.
        foreach ($available in (Microsoft.PowerShell.Core\Get-Module -ListAvailable -ErrorAction SilentlyContinue)) {
            $exported = $available.ExportedCommands[$name]
            if ($exported -and [string]$exported.CommandType -in $order) { $command = $exported; break }
        }
    }
    if (-not $command -or [string]$command.CommandType -ne 'Alias') { break }
    $name = $command.Definition
    if ($name -notmatch '^[A-Za-z0-9_.-]+$') { $command = $null; break }
}
if (-not $command -or [string]$command.CommandType -eq 'Alias') { __notypo_emit 'missing', $name; return }
$module = [string]$command.ModuleName
$shipped = -not $session -and (-not $command.Module -or
    ([IO.Path]::GetFullPath($command.Module.ModuleBase).TrimEnd($separator) + $separator).StartsWith($shippedRoot, [StringComparison]::OrdinalIgnoreCase))
if (-not ($shipped -or $trusted -contains '*' -or $trusted -contains ('module:' + $module) -or $trusted -contains ('command:' + $command.Name))) {
    __notypo_emit 'untrusted', $command.Name, $module
    return
}
$lookup = @{ Name = $command.Name; CommandType = $command.CommandType }
if ($command.Module) { $lookup.Module = $module }
if ($env:NOTYPO_PS_QUERY -eq 'options') {
    # Classic completion resolves function names in the runspace's global
    # scope. Publish only this trusted restored definition inside the probe.
    if ($session -and $command -is [System.Management.Automation.FunctionInfo]) {
        Microsoft.PowerShell.Management\Set-Item -LiteralPath ('Function:global:' + $command.Name) -Value $command.ScriptBlock
    }
    $null = Microsoft.PowerShell.Core\Get-Command @lookup
    $result = [System.Management.Automation.CommandCompletion]::CompleteInput($env:NOTYPO_PS_LINE, [int]$env:NOTYPO_PS_CURSOR, $null)
    __notypo_emit 'answered'
    __notypo_emit 'range', ([string]$result.ReplacementIndex), ([string]$result.ReplacementLength)
    foreach ($match in $result.CompletionMatches) {
        if ([string]$match.ResultType -eq 'ParameterName') {
            __notypo_emit 'match', $match.CompletionText, 'ParameterName', ([string]$match.ToolTip -replace '[\x00-\x1f\x7f]', ' ')
        }
    }
    return
}
if ($env:NOTYPO_PS_ARGUMENTS) {
    $arguments = @()
    $decoded = Microsoft.PowerShell.Utility\ConvertFrom-Json -InputObject $env:NOTYPO_PS_ARGUMENTS
    foreach ($record in $decoded.arguments) {
        if ($record[0]) { $arguments += [string]$record[1] }
        else {
            $literal = [PSObject]::AsPSObject([string]$record[1])
            $literal.PSObject.Properties.Add([System.Management.Automation.PSNoteProperty]::new('__notypo_literal', $true))
            $arguments += ,$literal
        }
    }
    $lookup.ArgumentList = $arguments
}
$info = Microsoft.PowerShell.Core\Get-Command @lookup | Microsoft.PowerShell.Utility\Select-Object -First 1
$reflection = [System.Reflection.BindingFlags]'NonPublic,Instance'
$context = $ExecutionContext.GetType().GetField('_context', $reflection).GetValue($ExecutionContext)
$callbacks = $context.GetType().GetProperty('CustomArgumentCompleters', $reflection).GetValue($context)
$binding = if ($info -is [System.Management.Automation.CmdletInfo]) {
    $info.ImplementingType.GetCustomAttributes([System.Management.Automation.CmdletAttribute], $true) | Microsoft.PowerShell.Utility\Select-Object -First 1
} else {
    $info.ScriptBlock.Attributes | Microsoft.PowerShell.Core\Where-Object { $_ -is [System.Management.Automation.CmdletBindingAttribute] } | Microsoft.PowerShell.Utility\Select-Object -First 1
}
$open = -not $binding
$common = @([System.Management.Automation.Cmdlet]::CommonParameters) + @([System.Management.Automation.Cmdlet]::OptionalCommonParameters)
if ($binding -and $binding.SupportsPaging) { $common += 'First', 'Skip', 'IncludeTotalCount' }
$parameters = @(if ($info.Parameters) { $info.Parameters.Values })
$argumentHintsType = 'System.Management.Automation.ArgumentCompletionsAttribute' -as [type]
foreach ($parameter in $parameters) {
    foreach ($attribute in $parameter.Attributes) {
        if ($attribute -is [System.Management.Automation.ParameterAttribute] -and $attribute.ValueFromRemainingArguments) { $open = $true }
    }
}
__notypo_emit 'command', $info.Name, ([string]$info.CommandType), $module, $(if ($shipped) { 'shipped' } else { 'trusted' }), $(if ($open) { 'open' } else { 'closed' })
if (($info -is [System.Management.Automation.CmdletInfo] -and [System.Management.Automation.IDynamicParameters].IsAssignableFrom($info.ImplementingType)) -or
    ($info -is [System.Management.Automation.FunctionInfo] -and ($info.ScriptBlock.Ast.DynamicParamBlock -or $info.ScriptBlock.Ast.Body.DynamicParamBlock))) { __notypo_emit 'contextual' }
foreach ($parameter in $parameters) {
    __notypo_emit 'parameter', $parameter.Name, $(if ($parameter.SwitchParameter) { 'switch' } else { 'value' }), $(if ($common -contains $parameter.Name) { 'common' } else { 'declared' }), $(if ($parameter.IsDynamic) { 'dynamic' } else { 'static' })
    if (@($parameter.ParameterSets.Values | Microsoft.PowerShell.Core\Where-Object { $_.Position -ge 0 }).Count) { __notypo_emit 'positional', $parameter.Name }
    foreach ($alias in $parameter.Aliases) { __notypo_emit 'alias', $parameter.Name, $alias }
    $type = $parameter.ParameterType
    if ($type.IsArray) { $type = $type.GetElementType() }
    $underlying = [Nullable]::GetUnderlyingType($type)
    if ($underlying) { $type = $underlying }
    if ($type.IsEnum) {
        foreach ($value in [Enum]::GetNames($type)) { __notypo_emit 'value', $parameter.Name, $value }
        if ($type.IsDefined([FlagsAttribute], $false)) { __notypo_emit 'flags', $parameter.Name }
    }
    foreach ($attribute in $parameter.Attributes) {
        if ($attribute -is [System.Management.Automation.ValidateSetAttribute]) {
            foreach ($value in $attribute.ValidValues) { __notypo_emit 'value', $parameter.Name, $value }
            if (-not $attribute.IgnoreCase) { __notypo_emit 'exact', $parameter.Name }
        }
        if ($attribute -is [System.Management.Automation.ArgumentCompleterAttribute] -or ($argumentHintsType -and $argumentHintsType.IsInstanceOfType($attribute))) { __notypo_emit 'completer', $parameter.Name }
    }
    if ($callbacks -and ($callbacks.ContainsKey($info.Name + ':' + $parameter.Name) -or $callbacks.ContainsKey($parameter.Name))) { __notypo_emit 'completer', $parameter.Name }
}
"#
);

/// The PowerShell to ask: the one the parent session runs in, when its
/// function names an absolute PowerShell executable, else `pwsh` (or
/// Windows PowerShell) on `$PATH`.
pub fn executable(which: impl Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    let named = |path: &Path| {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| {
                stem.eq_ignore_ascii_case("pwsh") || stem.eq_ignore_ascii_case("powershell")
            })
    };
    if let Some(path) = std::env::var_os("NOTYPO_POWERSHELL").map(PathBuf::from)
        && probe::is_trusted_location(&path)
        && named(&path)
        && path.is_file()
    {
        return Some(path);
    }
    which("pwsh")
        .or_else(|| cfg!(windows).then(|| which("powershell")).flatten())
        .filter(|path| probe::is_trusted_location(path))
}

/// A name PowerShell could resolve, safe to pass to the lookup pattern.
pub fn is_command_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with(['-', '.'])
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parameter {
    pub name: String,
    pub switch: bool,
    /// A common (or ShouldProcess/paging) parameter PowerShell adds.
    pub common: bool,
    pub dynamic: bool,
    pub aliases: Vec<String>,
    /// Enumeration names and `ValidateSet` values.
    pub values: Vec<String>,
    /// A `[Flags]` enumeration also takes comma-separated combinations.
    pub flags: bool,
    /// A case-sensitive `ValidateSet`.
    pub exact: bool,
    /// Declared ArgumentCompleter/ArgumentCompletions callback.
    pub completer: bool,
    /// Positional in at least one of PowerShell's parameter sets.
    pub positional: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Description {
    pub name: String,
    /// `Cmdlet`, `Function`, or `Filter`.
    pub kind: String,
    pub module: String,
    /// Built into PowerShell or shipped under `$PSHOME`.
    pub shipped: bool,
    /// Parameters it doesn't declare are accepted too: a simple function's
    /// `$args`, or a parameter taking the remaining arguments.
    pub open: bool,
    /// Dynamic parameter metadata can depend on bound literal arguments.
    pub contextual: bool,
    pub parameters: Vec<Parameter>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    Described(Description),
    Missing,
    Untrusted { command: String, module: String },
}

/// What a typed parameter name binds to.
#[derive(Debug, PartialEq, Eq)]
pub enum Binding<'d> {
    Bound(&'d Parameter),
    Ambiguous(Vec<&'d Parameter>),
    NotFound,
}

impl Description {
    /// PowerShell's binder: an exact name or alias wins, then a unique
    /// prefix of names and aliases, then the one declared parameter among
    /// several matches. `typed` may keep its dash.
    pub fn bind(&self, typed: &str) -> Binding<'_> {
        let typed = typed.strip_prefix('-').unwrap_or(typed).to_lowercase();
        if typed.is_empty() {
            return Binding::NotFound;
        }
        let names = |p: &'_ Parameter| {
            std::iter::once(p.name.to_lowercase())
                .chain(p.aliases.iter().map(|a| a.to_lowercase()))
                .collect::<Vec<_>>()
        };
        let mut matches: Vec<&Parameter> = Vec::new();
        for parameter in &self.parameters {
            let names = names(parameter);
            if names.contains(&typed) {
                return Binding::Bound(parameter);
            }
            if names.iter().any(|name| name.starts_with(&typed)) {
                matches.push(parameter);
            }
        }
        match matches.as_slice() {
            [] => Binding::NotFound,
            [one] => Binding::Bound(one),
            _ => {
                let declared: Vec<&Parameter> =
                    matches.iter().copied().filter(|p| !p.common).collect();
                match declared.as_slice() {
                    [one] => Binding::Bound(one),
                    _ => Binding::Ambiguous(matches),
                }
            }
        }
    }

    /// Unknown parameter names are errors, so the parameter list decides.
    pub fn closed(&self) -> bool {
        !self.open
    }
}

/// Reads the describe query's records. Any malformed record fails the
/// whole answer rather than leaving a partial parameter list.
pub fn parse(text: &str) -> Result<Answer, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut description: Option<Description> = None;
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        match (fields.as_slice(), description.as_mut()) {
            (["missing", _], None) => return Ok(Answer::Missing),
            (["untrusted", command, module], None) => {
                return Ok(Answer::Untrusted {
                    command: (*command).to_owned(),
                    module: (*module).to_owned(),
                });
            }
            (["command", name, kind, module, origin, open], None)
                if is_command_name(name)
                    && matches!(*kind, "Cmdlet" | "Function" | "Filter")
                    && matches!(*origin, "shipped" | "trusted")
                    && matches!(*open, "open" | "closed") =>
            {
                description = Some(Description {
                    name: (*name).to_owned(),
                    kind: (*kind).to_owned(),
                    module: (*module).to_owned(),
                    shipped: *origin == "shipped",
                    open: *open == "open",
                    contextual: false,
                    parameters: Vec::new(),
                });
            }
            (["contextual"], Some(d)) if !d.contextual => d.contextual = true,
            (["parameter", name, arity, role, origin], Some(d))
                if parameter_name(name)
                    && matches!(*arity, "switch" | "value")
                    && matches!(*role, "common" | "declared")
                    && matches!(*origin, "dynamic" | "static")
                    && !d.parameters.iter().any(|p| p.name == *name) =>
            {
                d.parameters.push(Parameter {
                    name: (*name).to_owned(),
                    switch: *arity == "switch",
                    common: *role == "common",
                    dynamic: *origin == "dynamic",
                    ..Parameter::default()
                });
            }
            (["alias", name, alias], Some(d)) if parameter_name(alias) => {
                last(d, name)?.aliases.push((*alias).to_owned());
            }
            (["value", name, value], Some(d)) if !value.is_empty() => {
                let parameter = last(d, name)?;
                if !parameter.values.iter().any(|v| v == value) {
                    parameter.values.push((*value).to_owned());
                }
            }
            (["flags", name], Some(d)) => last(d, name)?.flags = true,
            (["exact", name], Some(d)) => last(d, name)?.exact = true,
            (["completer", name], Some(d)) => last(d, name)?.completer = true,
            (["positional", name], Some(d)) => last(d, name)?.positional = true,
            _ => return Err(format!("unexpected PowerShell record `{line}`")),
        }
    }
    description
        .map(Answer::Described)
        .ok_or_else(|| "PowerShell described nothing".to_owned())
}

pub(super) fn parameter_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '?')
}

/// Records about a parameter follow its own record.
fn last<'d>(description: &'d mut Description, name: &str) -> Result<&'d mut Parameter, String> {
    description
        .parameters
        .last_mut()
        .filter(|p| p.name == name)
        .ok_or_else(|| format!("a PowerShell record for `{name}` is out of order"))
}

/// The probe's environment: no profile code, telemetry, update checks, or
/// network, and a module analysis cache of notypo's own.
pub(super) fn environment(query: &[(&str, &str)]) -> Vec<(OsString, Option<OsString>)> {
    let set = |k: &str, v: &str| (OsString::from(k), Some(OsString::from(v)));
    let mut env = super::offline_env(super::Flavor::PowerShellCompleter);
    env.extend(super::offline_env(super::Flavor::AwsCompleter));
    env.extend(
        [
            "GOOGLE_APPLICATION_CREDENTIALS",
            "CLOUDSDK_AUTH_ACCESS_TOKEN",
            "AZURE_CLIENT_SECRET",
            "AZURE_PASSWORD",
            "AZURE_FEDERATED_TOKEN_FILE",
            "ARM_CLIENT_SECRET",
            "ARM_OIDC_TOKEN",
        ]
        .map(|k| (OsString::from(k), None)),
    );
    env.extend([
        set("POWERSHELL_TELEMETRY_OPTOUT", "1"),
        set("POWERSHELL_UPDATECHECK", "Off"),
        set("NO_COLOR", "1"),
        set("TERM", "dumb"),
    ]);
    env.push((
        OsString::from("PSModuleAnalysisCachePath"),
        Some(
            crate::utils::cache_dir()
                .join("powershell")
                .join("ModuleAnalysisCache")
                .into_os_string(),
        ),
    ));
    env.extend(
        [
            "NOTYPO_PS_QUERY",
            "NOTYPO_PS_NAME",
            "NOTYPO_PS_TRUSTED",
            "NOTYPO_PS_DEFINITIONS",
            "NOTYPO_PS_COMPLETER",
            "NOTYPO_PS_LINE",
            "NOTYPO_PS_PARAMETER",
            "NOTYPO_PS_SESSION",
            "NOTYPO_PS_CURSOR",
            "NOTYPO_PS_START",
            "NOTYPO_PS_MODULE",
            "NOTYPO_PS_ARGUMENTS",
            "NOTYPO_PS_LOCATION",
            "NOTYPO_PS_DRIVES",
            "NOTYPO_PS_PREFIX",
        ]
        .map(|k| (OsString::from(k), None)),
    );
    env.push((
        "NOTYPO_PS_LOCATION".into(),
        std::env::var_os("NOTYPO_POWERSHELL_LOCATION"),
    ));
    env.push((
        "NOTYPO_PS_DRIVES".into(),
        std::env::var_os("NOTYPO_POWERSHELL_DRIVES"),
    ));
    env.extend(query.iter().map(|(k, v)| set(k, v)));
    env
}

fn run(
    shell: &Path,
    query: &[(&str, &str)],
    budget: &mut Budget,
) -> Result<String, CompletionError> {
    let args = [
        "-NoProfile",
        "-NonInteractive",
        "-NoLogo",
        "-Command",
        SCRIPT,
    ]
    .map(OsString::from)
    .to_vec();
    run_stdout(shell, args, environment(query), budget, true)
}

/// The names of the cmdlets, functions, and aliases PowerShell can run
/// without user configuration, read without importing any module.
pub fn commands(shell: &Path, budget: &mut Budget) -> Result<Vec<String>, CompletionError> {
    if let Ok(snapshot) = std::env::var("NOTYPO_POWERSHELL_NAMES")
        && !snapshot.is_empty()
    {
        // Command names are discovery metadata. Candidate limits apply
        // after the executable provider ranks this bounded vocabulary.
        return command_snapshot(&snapshot, 65_536);
    }
    if let Some(path) = std::env::var_os("NOTYPO_POWERSHELL_NAMES_FILE") {
        return command_snapshot_file(Path::new(&path));
    }
    let text = run(shell, &[("NOTYPO_PS_QUERY", "list")], budget)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut names = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        match line.split('\t').collect::<Vec<_>>().as_slice() {
            [] | [""] => {}
            ["command", name] if is_command_name(name) => {
                if seen.insert(name.to_ascii_lowercase()) {
                    names.push((*name).to_owned());
                }
            }
            // Script names (`cd..`) and other odd names aren't offered.
            ["command", _] => {}
            _ => {
                return Err(CompletionError::Failed(format!(
                    "unexpected PowerShell record `{line}`"
                )));
            }
        }
    }
    Ok(names)
}

const MAX_COMMAND_SNAPSHOT: usize = 4 * 1024 * 1024;

fn command_snapshot(text: &str, limit: usize) -> Result<Vec<String>, CompletionError> {
    let invalid = || CompletionError::Failed("invalid PowerShell command-name snapshot".into());
    if text.len() > MAX_COMMAND_SNAPSHOT {
        return Err(invalid());
    }
    let root: serde_json::Value = serde_json::from_str(text).map_err(|_| invalid())?;
    if root["version"] != 1 || root.as_object().is_none_or(|root| root.len() != 2) {
        return Err(invalid());
    }
    let mut seen = std::collections::HashSet::new();
    let mut names = Vec::new();
    for entry in root["names"].as_array().ok_or_else(invalid)? {
        let name = entry
            .as_str()
            .filter(|name| is_command_name(name))
            .ok_or_else(invalid)?;
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(invalid());
        }
        names.push(name.into());
        if names.len() > limit {
            return Err(super::over_limit());
        }
    }
    Ok(names)
}

fn command_snapshot_file(path: &Path) -> Result<Vec<String>, CompletionError> {
    use std::io::Read;
    let failed =
        |error| CompletionError::Failed(format!("PowerShell command-name snapshot: {error}"));
    let file = std::fs::File::open(path).map_err(failed)?;
    let mut text = String::new();
    file.take(MAX_COMMAND_SNAPSHOT as u64 + 1)
        .read_to_string(&mut text)
        .map_err(failed)?;
    command_snapshot(&text, 65_536)
}

/// The trust entries the probe understands: `*`, `module:<Module>` from
/// `powershell:<Module>`, and `command:<Name>` from any other entry.
fn trust_entries(trusted: &[String]) -> String {
    trusted
        .iter()
        .filter(|t| !t.contains(['\n', '\r']))
        .map(|t| match t.strip_prefix("powershell:") {
            _ if t == "*" => "*".to_owned(),
            Some(module) => format!("module:{module}"),
            None => format!("command:{t}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One PowerShell command, described on first use.
#[derive(Debug)]
pub struct Command {
    name: String,
    shell: PathBuf,
    trusted: String,
    described: RefCell<Option<Result<Rc<Description>, CompletionError>>>,
    active: RefCell<Option<Rc<Description>>>,
    fallback_context: Cell<bool>,
    contextual: RefCell<ContextualDescriptions>,
    snapshot: Result<Option<Snapshot>, CompletionError>,
    positional_bindings: RefCell<PositionalBindings>,
}

type PositionalBindings = HashMap<PositionalKey, Result<Option<Parameter>, CompletionError>>;
type PositionalKey = (Vec<String>, Vec<String>, Vec<usize>, Vec<usize>);
type ContextualDescriptions =
    HashMap<(PositionalKey, bool), Result<Rc<Description>, CompletionError>>;

impl Command {
    pub fn new(name: &str, shell: PathBuf, trusted: &[String]) -> Command {
        Command {
            name: name.to_owned(),
            shell,
            trusted: trust_entries(trusted),
            described: RefCell::new(None),
            active: RefCell::new(None),
            fallback_context: Cell::new(false),
            contextual: RefCell::new(HashMap::new()),
            snapshot: powershell_values::memory(name),
            positional_bindings: RefCell::new(HashMap::new()),
        }
    }

    /// A command whose description is already known (tests and embedders).
    pub fn described(description: Description) -> Command {
        Command {
            name: description.name.clone(),
            shell: PathBuf::new(),
            trusted: String::new(),
            described: RefCell::new(Some(Ok(Rc::new(description)))),
            active: RefCell::new(None),
            fallback_context: Cell::new(false),
            contextual: RefCell::new(HashMap::new()),
            snapshot: Ok(None),
            positional_bindings: RefCell::new(HashMap::new()),
        }
    }

    pub fn describe(&self, budget: &mut Budget) -> Result<Rc<Description>, CompletionError> {
        if let Some(known) = self.described.borrow().as_ref() {
            return known.clone();
        }
        let result = self.query_description(None, budget);
        if result != Err(CompletionError::BudgetExhausted) {
            *self.described.borrow_mut() = Some(result.clone());
        }
        result
    }

    fn query_description(
        &self,
        arguments: Option<&[(bool, String)]>,
        budget: &mut Budget,
    ) -> Result<Rc<Description>, CompletionError> {
        let snapshot = self.snapshot.as_ref().map_err(Clone::clone)?;
        let definitions = if let Some(snapshot) = snapshot {
            if !snapshot.definitions.is_empty() {
                if !self.session_trusted(&self.name) {
                    return Err(CompletionError::Unsupported(format!(
                        "restoring session definitions for {} requires trusted_completers to name the command",
                        self.name
                    )));
                }
                if !snapshot.portable {
                    return Err(CompletionError::Unsupported(
                        "PowerShell command depends on uncaptured session state".into(),
                    ));
                }
            }
            snapshot.definitions.as_str()
        } else {
            ""
        };
        let answer = if is_command_name(&self.name) {
            let arguments = arguments.map(encode_metadata_arguments).unwrap_or_default();
            run(
                &self.shell,
                &[
                    ("NOTYPO_PS_QUERY", "describe"),
                    ("NOTYPO_PS_NAME", &self.name),
                    ("NOTYPO_PS_TRUSTED", &self.trusted),
                    ("NOTYPO_PS_DEFINITIONS", definitions),
                    ("NOTYPO_PS_ARGUMENTS", &arguments),
                    (
                        "NOTYPO_PS_SESSION",
                        if snapshot.as_ref().is_some_and(|s| s.session) {
                            "1"
                        } else {
                            "0"
                        },
                    ),
                ],
                budget,
            )
            .and_then(|text| parse(&text).map_err(CompletionError::Failed))
        } else {
            Err(CompletionError::Unsupported(format!(
                "`{}` is not a name PowerShell commands use",
                self.name
            )))
        };
        match answer {
            Ok(Answer::Described(description)) => Ok(Rc::new(description)),
            Ok(Answer::Missing) => Err(CompletionError::Failed(format!(
                "PowerShell without your profile has no command `{}`",
                self.name
            ))),
            Ok(Answer::Untrusted { command, module }) => Err(CompletionError::Failed(format!(
                "{command} comes from the module {module}, which PowerShell would import to \
                 describe it; add `powershell:{module}` to trusted_completers to allow that"
            ))),
            Err(error) => Err(error),
        }
    }

    fn known(&self) -> Option<Rc<Description>> {
        self.active
            .borrow()
            .clone()
            .or_else(|| self.described.borrow().as_ref()?.as_ref().ok().cloned())
    }

    fn describe_context(
        &self,
        context: &ValueContext<'_>,
        value_slot: bool,
        budget: &mut Budget,
    ) -> Result<Rc<Description>, CompletionError> {
        self.fallback_context.set(false);
        let base = self.describe(budget)?;
        if !base.contextual || context.words.is_empty() && context.following.is_empty() {
            *self.active.borrow_mut() = Some(base.clone());
            return Ok(base);
        }
        let key = (Self::positional_key(context), value_slot);
        let cached = self.contextual.borrow().get(&key).cloned();
        let result = match cached {
            Some(known) => known,
            None => {
                let known = self.known().unwrap_or_else(|| base.clone());
                metadata_arguments(&known, context, value_slot)
                    .and_then(|arguments| self.query_description(Some(&arguments), budget))
            }
        };
        if result != Err(CompletionError::BudgetExhausted) {
            self.contextual.borrow_mut().insert(key, result.clone());
        }
        *self.active.borrow_mut() = result.as_ref().ok().cloned();
        result
    }

    fn query_options(
        &self,
        context: &ValueContext<'_>,
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let description = self.describe(budget)?;
        if !prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_?".contains(&byte))
            || !description.module.is_empty() && !is_command_name(&description.module)
        {
            return Err(CompletionError::Unsupported(
                "unsupported PowerShell parameter prefix or module name".into(),
            ));
        }
        let mut line = if description.module.is_empty() {
            description.name.clone()
        } else {
            format!("{}\\{}", description.module, description.name)
        };
        let append = |line: &mut String,
                      words: &[&str],
                      literals: &[usize]|
         -> Result<(), CompletionError> {
            for (index, word) in words.iter().enumerate() {
                if word.contains(char::is_control) {
                    return Err(CompletionError::Unsupported(
                        "control character in PowerShell context".into(),
                    ));
                }
                line.push(' ');
                let (name, value) = word
                    .split_once(':')
                    .map_or((*word, None), |(name, value)| (name, Some(value)));
                if !literals.contains(&index) && name.strip_prefix('-').is_some_and(parameter_name)
                {
                    line.push_str(name);
                    if let Some(value) = value {
                        line.push(':');
                        line.push_str(&format!("'{}'", value.replace('\'', "''")));
                    }
                } else {
                    line.push_str(&format!("'{}'", word.replace('\'', "''")));
                }
            }
            Ok(())
        };
        append(&mut line, context.words, context.literal_arguments.0)?;
        line.push(' ');
        let start = line.len();
        line.push_str(prefix);
        let end = line.len();
        let cursor = line.encode_utf16().count().to_string();
        append(&mut line, context.following, context.literal_arguments.1)?;
        let snapshot = self.snapshot.as_ref().map_err(Clone::clone)?.as_ref();
        let text = run(
            &self.shell,
            &[
                ("NOTYPO_PS_QUERY", "options"),
                ("NOTYPO_PS_NAME", &self.name),
                ("NOTYPO_PS_TRUSTED", &self.trusted),
                (
                    "NOTYPO_PS_DEFINITIONS",
                    snapshot.map_or("", |snapshot| snapshot.definitions.as_str()),
                ),
                (
                    "NOTYPO_PS_SESSION",
                    if snapshot.is_some_and(|snapshot| snapshot.session) {
                        "1"
                    } else {
                        "0"
                    },
                ),
                ("NOTYPO_PS_LINE", &line),
                ("NOTYPO_PS_CURSOR", &cursor),
            ],
            budget,
        )?;
        let text = super::powershell_completer::normalize_replacement(
            &text,
            &line,
            crate::engine::parser::Span { start, end },
        )?;
        let mut items =
            super::powershell_completer::parse(&text, &self.name, budget.max_candidates)?;
        for item in &mut items {
            if let Binding::Bound(parameter) = description.bind(&item.value) {
                item.takes_value = Some(!parameter.switch);
            }
        }
        *self.active.borrow_mut() = Some(description);
        self.fallback_context.set(true);
        Ok(items)
    }

    fn session_trusted(&self, name: &str) -> bool {
        self.trusted.lines().any(|t| {
            t == "*"
                || t.strip_prefix("command:")
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
    }

    fn value_parameter(&self, words: &[&str]) -> Option<Parameter> {
        let description = self.known()?;
        match description.bind(words.last()?) {
            Binding::Bound(p) if !p.switch => Some(p.clone()),
            _ => None,
        }
    }

    fn option(parameter: &Parameter) -> CompletionItem {
        CompletionItem {
            value: format!("-{}", parameter.name),
            takes_value: Some(!parameter.switch),
            description: None,
        }
    }

    fn positional_key(context: &ValueContext<'_>) -> PositionalKey {
        (
            context.words.iter().map(|w| (*w).to_owned()).collect(),
            context.following.iter().map(|w| (*w).to_owned()).collect(),
            context.literal_arguments.0.to_vec(),
            context.literal_arguments.1.to_vec(),
        )
    }

    fn positional_parameter(&self, context: &ValueContext<'_>) -> Option<Parameter> {
        self.positional_bindings
            .borrow()
            .get(&Self::positional_key(context))?
            .as_ref()
            .ok()?
            .clone()
    }

    fn parameter_values(
        &self,
        description: &Description,
        parameter: &Parameter,
        context: &ValueContext<'_>,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        if parameter.values.is_empty() && !parameter.switch {
            let snapshot = self.snapshot.as_ref().map_err(Clone::clone)?.as_ref();
            if snapshot.is_some_and(|s| s.has_registration(&parameter.name)) {
                if !self.session_trusted(&description.name) {
                    return Err(CompletionError::Unsupported(format!(
                        "session parameter callback for {} requires trusted_completers to name the command",
                        description.name
                    )));
                }
            } else if !parameter.completer {
                return Ok(Vec::new());
            }
            return powershell_values::query_context(
                &self.shell,
                description,
                parameter,
                snapshot,
                context,
                budget,
            );
        }
        Ok(parameter
            .values
            .iter()
            .map(|value| CompletionItem {
                value: value.clone(),
                takes_value: None,
                description: None,
            })
            .collect())
    }
}

impl super::NativeCompletionBackend for Command {
    fn id(&self) -> &str {
        "powershell"
    }

    fn capabilities(&self) -> Capabilities {
        let known = self.known();
        Capabilities {
            subcommands: false,
            options: true,
            option_arity: !self.fallback_context.get(),
            values: true,
            resources: false,
            option_prefix: "-",
            short_options: true,
            // A level with no subcommands: positional words are arguments.
            complete_subcommands: true,
            complete_options: !self.fallback_context.get()
                && known.as_ref().is_some_and(|d| d.closed()),
            descriptions: true,
            query_dialect: None,
            trust: match known {
                Some(d) if !d.shipped => Trust::UserTrusted,
                _ => Trust::Bridge,
            },
        }
    }

    fn complete(
        &self,
        words: &[&str],
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        self.complete_context(
            &ValueContext {
                words,
                following: &[],
                literal_arguments: (&[], &[]),
                positional: false,
            },
            prefix,
            budget,
        )
    }

    fn complete_at(
        &self,
        words: &[&str],
        prefix: &str,
        following: &[&str],
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        self.complete_context(
            &ValueContext {
                words,
                following,
                literal_arguments: (&[], &[]),
                positional: false,
            },
            prefix,
            budget,
        )
    }

    fn complete_context(
        &self,
        context: &ValueContext<'_>,
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let description = match self.describe_context(context, false, budget) {
            Ok(description) => description,
            Err(error)
                if self.describe(budget)?.contextual
                    && error != CompletionError::BudgetExhausted =>
            {
                if prefix.starts_with('-') {
                    return self.query_options(context, prefix, budget);
                }
                return Ok(Vec::new());
            }
            Err(error) => return Err(error),
        };
        if !prefix.starts_with('-') {
            return Ok(Vec::new());
        }
        let typed = prefix.to_lowercase();
        Ok(description
            .parameters
            .iter()
            .map(Command::option)
            .filter(|item| item.value.to_lowercase().starts_with(&typed))
            .collect())
    }

    fn requires_literal_completion_context(&self) -> bool {
        self.known()
            .is_none_or(|description| description.contextual)
    }

    fn prepare_query_context(
        &self,
        context: &ValueContext<'_>,
        value_slot: bool,
        budget: &mut Budget,
    ) -> Result<(), CompletionError> {
        match self.describe_context(context, value_slot, budget) {
            Ok(_) => Ok(()),
            Err(error)
                if !value_slot
                    && self.describe(budget)?.contextual
                    && error != CompletionError::BudgetExhausted =>
            {
                *self.active.borrow_mut() = Some(self.describe(budget)?);
                self.fallback_context.set(true);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn static_option(&self, typed: &str, budget: &mut Budget) -> Option<CompletionItem> {
        let description = self.describe(budget).ok()?;
        let name = typed.strip_prefix('-')?;
        let parameter = description.parameters.iter().find(|parameter| {
            !parameter.dynamic
                && (parameter.name.eq_ignore_ascii_case(name)
                    || parameter
                        .aliases
                        .iter()
                        .any(|alias| alias.eq_ignore_ascii_case(name)))
        })?;
        Some(Self::option(parameter))
    }

    fn location(&self) -> String {
        match self.known() {
            Some(d) if !d.module.is_empty() => {
                format!("{} (module {})", self.shell.display(), d.module)
            }
            _ => self.shell.display().to_string(),
        }
    }

    fn complete_values(
        &self,
        words: &[&str],
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        self.complete_values_with_following(words, &[], budget)
    }

    fn complete_values_with_following(
        &self,
        words: &[&str],
        following: &[&str],
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        self.complete_value_context(
            &ValueContext {
                words,
                following,
                literal_arguments: (&[], &[]),
                positional: false,
            },
            budget,
        )
    }

    fn supports_positional_values(&self) -> bool {
        true
    }

    fn complete_positionals_with_following(
        &self,
        words: &[&str],
        following: &[&str],
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        self.complete_value_context(
            &ValueContext {
                words,
                following,
                literal_arguments: (&[], &[]),
                positional: true,
            },
            budget,
        )
    }

    fn complete_value_context(
        &self,
        context: &ValueContext<'_>,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let description = self.describe_context(context, true, budget)?;
        if !context.positional {
            let Some(option) = context.words.last().filter(|w| w.starts_with('-')) else {
                return Ok(Vec::new());
            };
            let Binding::Bound(parameter) = description.bind(option) else {
                return Ok(Vec::new());
            };
            return self.parameter_values(&description, parameter, context, budget);
        }
        let snapshot = self.snapshot.as_ref().map_err(Clone::clone)?.as_ref();
        if !description.closed()
            || !description.parameters.iter().any(|p| {
                p.positional
                    && (!p.values.is_empty()
                        || p.completer
                        || snapshot.is_some_and(|s| s.has_registration(&p.name)))
            })
        {
            return Ok(Vec::new());
        }
        let key = Self::positional_key(context);
        let known = self.positional_bindings.borrow().get(&key).cloned();
        let result = match known {
            Some(result) => result,
            None => powershell_values::bind_positional(
                &self.shell,
                &description,
                snapshot,
                context,
                budget,
            ),
        };
        if result != Err(CompletionError::BudgetExhausted) {
            self.positional_bindings
                .borrow_mut()
                .insert(key, result.clone());
        }
        let Some(parameter) = result? else {
            return Ok(Vec::new());
        };
        self.parameter_values(&description, &parameter, context, budget)
    }

    fn prepare_positional_value_candidates(
        &self,
        context: &ValueContext<'_>,
        typed: &str,
        items: Vec<CompletionItem>,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        Ok(prepare_values(
            self.positional_parameter(context),
            typed,
            items,
        ))
    }

    fn positional_candidate_is_resource(
        &self,
        context: &ValueContext<'_>,
        _item: &CompletionItem,
    ) -> bool {
        self.positional_parameter(context)
            .is_none_or(|p| p.values.is_empty())
    }

    /// Enumeration and `ValidateSet` values ignore case unless the set
    /// says otherwise, and `[Flags]` values combine with commas. A value
    /// PowerShell would accept is kept as typed; numbers aren't checked.
    fn prepare_value_candidates(
        &self,
        words: &[&str],
        typed: &str,
        items: Vec<CompletionItem>,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let parameter = self.known().and_then(|description| {
            let option = words.last()?;
            match description.bind(option) {
                Binding::Bound(parameter) => Some(parameter.clone()),
                _ => None,
            }
        });
        Ok(prepare_values(parameter, typed, items))
    }

    fn values_require_literal_context(&self, words: &[&str]) -> bool {
        self.value_parameter(words)
            .is_some_and(|p| p.values.is_empty())
    }

    fn candidate_is_resource(&self, words: &[&str], item: &CompletionItem) -> bool {
        item.takes_value.is_none() && self.values_require_literal_context(words)
    }

    fn is_option(&self, words: &[&str], item: &CompletionItem) -> bool {
        if item.takes_value.is_none() && self.value_parameter(words).is_some() {
            false
        } else {
            item.is_option()
        }
    }

    fn value_syntax_contains_slashes(&self, _words: &[&str]) -> bool {
        true
    }

    fn resolve_option(
        &self,
        _words: &[&str],
        typed: &str,
        budget: &mut Budget,
    ) -> Option<CompletionItem> {
        let description = self.known().or_else(|| self.describe(budget).ok())?;
        // `-?` asks for help.
        if typed == "-?" {
            return Some(CompletionItem {
                value: typed.to_owned(),
                takes_value: Some(false),
                description: None,
            });
        }
        match description.bind(typed) {
            Binding::Bound(parameter) => Some(Command::option(parameter)),
            _ => None,
        }
    }
}

/// Get-Command interprets plain dash-prefixed strings as parameter names.
/// The wire format marks those names explicitly; all data is restored as a
/// wrapped literal PSObject, preserving quoted dashes without a `--` argument.
pub(super) fn encode_metadata_arguments(arguments: &[(bool, String)]) -> String {
    serde_json::json!({"arguments": arguments}).to_string()
}

pub(super) fn metadata_arguments(
    description: &Description,
    context: &ValueContext<'_>,
    value_slot: bool,
) -> Result<Vec<(bool, String)>, CompletionError> {
    let before = context
        .words
        .len()
        .saturating_sub(usize::from(value_slot && !context.positional));
    let words: Vec<_> = context.words[..before]
        .iter()
        .enumerate()
        .map(|(index, word)| (*word, context.literal_arguments.0.contains(&index)))
        .chain(
            context
                .following
                .iter()
                .enumerate()
                .map(|(index, word)| (*word, context.literal_arguments.1.contains(&index))),
        )
        .collect();
    let mut arguments = Vec::new();
    let mut index = 0;
    let mut after_delimiter = false;
    while let Some(&(word, literal)) = words.get(index) {
        if word.contains(char::is_control) {
            return Err(CompletionError::Unsupported(
                "PowerShell metadata context contains control characters".into(),
            ));
        }
        if !literal && word == "--" {
            after_delimiter = true;
        } else if !literal
            && !after_delimiter
            && word
                .strip_prefix('-')
                .is_some_and(|rest| rest.starts_with(char::is_alphabetic))
        {
            let (name, attached) = word
                .split_once(':')
                .map_or((word, None), |(name, value)| (name, Some(value)));
            let Binding::Bound(parameter) = description.bind(name) else {
                return Err(CompletionError::Unsupported(format!(
                    "PowerShell metadata context contains an unresolved parameter {name}"
                )));
            };
            let canonical = format!("-{}", parameter.name);
            if let Some(value) = attached {
                arguments.push((true, format!("{canonical}:{value}")));
            } else if parameter.switch {
                arguments.push((true, canonical));
            } else {
                let Some(&(value, _)) = words.get(index + 1) else {
                    return Err(CompletionError::Unsupported(format!(
                        "PowerShell metadata context lacks a value for {name}"
                    )));
                };
                if value.contains(char::is_control) {
                    return Err(CompletionError::Unsupported(
                        "PowerShell metadata context contains control characters".into(),
                    ));
                }
                arguments.push((true, canonical));
                arguments.push((false, value.to_owned()));
                index += 1;
            }
        } else {
            arguments.push((false, word.to_owned()));
        }
        index += 1;
    }
    Ok(arguments)
}

fn prepare_values(
    parameter: Option<Parameter>,
    typed: &str,
    items: Vec<CompletionItem>,
) -> Vec<CompletionItem> {
    let Some(parameter) = parameter else {
        return items;
    };
    if parameter.values.is_empty() {
        return items;
    }
    let same = |a: &str, b: &str| {
        if parameter.exact {
            a == b
        } else {
            a.to_lowercase() == b.to_lowercase()
        }
    };
    if typed.parse::<i64>().is_ok() {
        return Vec::new();
    }
    let parts: Vec<&str> = typed.split(',').map(str::trim).collect();
    let accepted = (parameter.flags || parts.len() == 1)
        && parts
            .iter()
            .all(|part| items.iter().any(|item| same(&item.value, part)));
    if accepted {
        return vec![CompletionItem {
            value: typed.to_owned(),
            takes_value: None,
            description: None,
        }];
    }
    if parts.len() > 1 { Vec::new() } else { items }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::NativeCompletionBackend;
    use super::*;
    use std::time::Duration;

    #[test]
    fn parent_command_names_are_validated_and_bounded() {
        let snapshot = r#"{"version":1,"names":["Get-ChildItem","gci"]}"#;
        assert_eq!(
            command_snapshot(snapshot, 2).unwrap(),
            ["Get-ChildItem", "gci"]
        );
        assert!(command_snapshot(snapshot, 1).is_err());
        for snapshot in [
            r#"{"version":1,"names":["gci","GCI"]}"#,
            r#"{"version":1,"names":["bad name"]}"#,
            r#"{"version":1,"names":null}"#,
            r#"{"version":2,"names":[]}"#,
            r#"{"version":1,"names":[],"extra":1}"#,
        ] {
            assert!(command_snapshot(snapshot, 10).is_err(), "{snapshot}");
        }
    }

    #[test]
    fn large_command_name_files_are_bounded_data() {
        let dir = FixtureDirectory::new();
        let path = dir.0.join("names.json");
        let names: Vec<_> = (0..6000)
            .map(|index| format!("Get-InstalledFixture{index}"))
            .collect();
        std::fs::write(
            &path,
            serde_json::json!({"version":1,"names":names}).to_string(),
        )
        .unwrap();
        assert_eq!(command_snapshot_file(&path).unwrap().len(), 6000);
        std::fs::write(&path, " ".repeat(MAX_COMMAND_SNAPSHOT + 1)).unwrap();
        assert!(command_snapshot_file(&path).is_err());
        std::fs::write(&path, "{\"version\":1,\"names\":[\"bad name\"]}").unwrap();
        assert!(command_snapshot_file(&path).is_err());
        assert!(command_snapshot_file(&dir.0.join("missing")).is_err());
    }

    struct FixtureDirectory(PathBuf);

    impl FixtureDirectory {
        fn new() -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "notypo-powershell-metadata-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for FixtureDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(60), Duration::from_secs(30), 16)
    }

    fn parameter(name: &str, aliases: &[&str], common: bool) -> Parameter {
        Parameter {
            name: name.into(),
            common,
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            ..Parameter::default()
        }
    }

    #[test]
    fn metadata_arguments_keep_quoted_dash_data_and_named_bindings() {
        let description = Description {
            name: "Get-Fixture".into(),
            parameters: vec![
                parameter("Scope", &["s"], false),
                parameter("Target", &[], false),
                Parameter {
                    switch: true,
                    ..parameter("Loud", &[], false)
                },
            ],
            ..Description::default()
        };
        let context = ValueContext {
            words: &["-west", "-s", "east", "-Loud"],
            following: &["-Scope"],
            literal_arguments: (&[0], &[0]),
            positional: false,
        };
        assert_eq!(
            metadata_arguments(&description, &context, false).unwrap(),
            [
                (false, "-west"),
                (true, "-Scope"),
                (false, "east"),
                (true, "-Loud"),
                (false, "-Scope")
            ]
            .map(|(named, value)| (named, value.to_owned()))
        );
        let context = ValueContext {
            words: &["-Scope:-west", "-Target"],
            following: &[],
            literal_arguments: (&[], &[]),
            positional: false,
        };
        assert_eq!(
            metadata_arguments(&description, &context, true).unwrap(),
            [(true, "-Scope:-west".to_owned())]
        );
        assert!(metadata_arguments(&description, &context, false).is_err());
    }

    #[test]
    fn real_dynamic_metadata_uses_both_sides_and_never_runs_the_operation() {
        let Some(shell) = pwsh() else { return };
        let dir = FixtureDirectory::new();
        let marker = dir.0.join("operation");
        let definitions = r#"function Get-DynamicFixture {
 [CmdletBinding()] param([string]$Scope)
 dynamicparam {
  $set = [System.Management.Automation.RuntimeDefinedParameterDictionary]::new()
  if ($Scope -in @('west','-west')) {
   $attributes = [System.Collections.ObjectModel.Collection[System.Attribute]]::new()
   $attributes.Add([System.Management.Automation.ParameterAttribute]::new())
   $attributes.Add([System.Management.Automation.ValidateSetAttribute]::new([string[]]@('one','two')))
   $set.Add('Flavor', [System.Management.Automation.RuntimeDefinedParameter]::new('Flavor',[string],$attributes))
  }
  $set
 }
 process { [IO.File]::WriteAllText('OPERATION_MARKER','ran') }
}"#.replace("OPERATION_MARKER", &marker.to_string_lossy().replace('\'', "''"));
        let mut backend = Command::new("Get-DynamicFixture", shell, &["Get-DynamicFixture".into()]);
        let mut snapshot = Snapshot::default();
        snapshot.session = true;
        snapshot.portable = true;
        snapshot.definitions = definitions;
        backend.snapshot = Ok(Some(snapshot));
        let mut probes = budget();
        assert!(backend.describe(&mut probes).unwrap().contextual);
        for (before, after, literals, expected) in [
            (vec!["-Scope", "west"], vec![], vec![], true),
            (vec![], vec!["-Scope", "east"], vec![], false),
            (vec![], vec!["-Scope", "west"], vec![], true),
            (vec!["-west"], vec![], vec![0], true),
        ] {
            let context = ValueContext {
                words: &before,
                following: &after,
                literal_arguments: (&literals, &[]),
                positional: false,
            };
            let options = backend
                .complete_context(&context, "-", &mut probes)
                .unwrap();
            assert_eq!(
                options.iter().any(|option| option.value == "-Flavor"),
                expected,
                "{context:?}"
            );
            let spawned = probes.spawned();
            assert_eq!(
                backend
                    .complete_context(&context, "-", &mut probes)
                    .unwrap(),
                options
            );
            assert_eq!(probes.spawned(), spawned);
        }
        let context = ValueContext {
            words: &["-Scope", "west", "-Flavor"],
            following: &[],
            literal_arguments: (&[], &[]),
            positional: false,
        };
        let values = backend
            .complete_value_context(&context, &mut probes)
            .unwrap();
        assert_eq!(
            values
                .iter()
                .map(|value| value.value.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        let scope = backend
            .resolve_option(&["-Scope", "west"], "-flav", &mut probes)
            .unwrap();
        assert_eq!(scope.value, "-Flavor");
        assert_eq!(scope.takes_value, Some(true));
        assert!(!marker.exists());
    }

    #[test]
    fn provider_metadata_uses_explicit_paths_and_parent_provider_location() {
        let Some(shell) = pwsh() else { return };
        let backend = Command::new("Get-ChildItem", shell.clone(), &[]);
        let mut probes = budget();
        let file_options = backend.complete(&[], "-", &mut probes).unwrap();
        assert!(
            backend.describe(&mut probes).unwrap().contextual,
            "Get-ChildItem must advertise dynamic metadata"
        );
        assert!(
            file_options
                .iter()
                .any(|option| option.value == "-Directory")
        );
        let env_options = backend
            .complete_at(&[], "-", &["Env:"], &mut probes)
            .unwrap();
        assert!(
            !env_options
                .iter()
                .any(|option| option.value == "-Directory")
        );
        let returned = run(
            &shell,
            &[
                ("NOTYPO_PS_QUERY", "describe"),
                ("NOTYPO_PS_NAME", "Get-ChildItem"),
                ("NOTYPO_PS_LOCATION", "Env:"),
            ],
            &mut probes,
        )
        .unwrap();
        let Answer::Described(description) = parse(&returned).unwrap() else {
            panic!("missing provider metadata")
        };
        assert!(
            !description
                .parameters
                .iter()
                .any(|parameter| parameter.name == "Directory")
        );
        let malformed = run(
            &shell,
            &[
                ("NOTYPO_PS_QUERY", "describe"),
                ("NOTYPO_PS_NAME", "Get-ChildItem"),
                ("NOTYPO_PS_LOCATION", "NotypoMissingProvider:"),
            ],
            &mut probes,
        );
        assert!(
            malformed.is_err(),
            "an unavailable provider must not fall through to filesystem metadata"
        );
    }

    #[test]
    fn malformed_drive_context_never_falls_through_to_default_metadata() {
        let Some(shell) = pwsh() else { return };
        let root = std::env::temp_dir().to_string_lossy().into_owned();
        let entry = serde_json::json!({"name":"NotypoDrive","root":root});
        for snapshot in [
            serde_json::json!({"version":1,"wrong":[]}),
            serde_json::json!({"version":1,"drives":null}),
            serde_json::json!({"version":2,"drives":[]}),
            serde_json::json!({"version":1,"drives":[{"name":"Bad*Drive","root":root}]}),
            serde_json::json!({"version":1,"drives":[{"name":"Drive","root":"relative/path"}]}),
            serde_json::json!({"version":1,"drives":[entry.clone(),entry]}),
        ] {
            assert!(
                run(
                    &shell,
                    &[
                        ("NOTYPO_PS_QUERY", "describe"),
                        ("NOTYPO_PS_NAME", "Get-ChildItem"),
                        ("NOTYPO_PS_DRIVES", &snapshot.to_string()),
                    ],
                    &mut budget()
                )
                .is_err(),
                "{snapshot}"
            );
        }
    }

    /// Get-ChildItem's parameters as PowerShell 7.6 lists them in a
    /// file-system location (abridged).
    fn get_child_item() -> Description {
        let mut parameters = vec![
            parameter("Path", &[], false),
            parameter("LiteralPath", &["PSPath", "LP"], false),
            parameter("Include", &[], false),
            parameter("Exclude", &[], false),
            Parameter {
                switch: true,
                ..parameter("Recurse", &["s", "r"], false)
            },
            parameter("Depth", &[], false),
            parameter("ErrorAction", &["ea"], true),
            parameter("ErrorVariable", &["ev"], true),
            parameter("InformationAction", &["infa"], true),
            parameter("InformationVariable", &["iv"], true),
            parameter("OutVariable", &["ov"], true),
            parameter("OutBuffer", &["ob"], true),
            Parameter {
                switch: true,
                ..parameter("Verbose", &["vb"], true)
            },
            Parameter {
                switch: true,
                dynamic: true,
                ..parameter("Directory", &["ad"], false)
            },
            Parameter {
                switch: true,
                dynamic: true,
                ..parameter("ReadOnly", &["ar"], false)
            },
        ];
        parameters[6].values = vec!["SilentlyContinue".into(), "Stop".into()];
        Description {
            name: "Get-ChildItem".into(),
            kind: "Cmdlet".into(),
            module: "Microsoft.PowerShell.Management".into(),
            shipped: true,
            open: false,
            contextual: false,
            parameters,
        }
    }

    fn bound(description: &Description, typed: &str) -> Option<String> {
        match description.bind(typed) {
            Binding::Bound(p) => Some(p.name.clone()),
            Binding::Ambiguous(_) => Some("ambiguous".into()),
            Binding::NotFound => None,
        }
    }

    #[test]
    fn parameter_names_bind_as_powershell_binds_them() {
        let gci = get_child_item();
        for (typed, expected) in [
            ("-Recurse", Some("Recurse")),
            ("-RECURSE", Some("Recurse")),
            ("-rec", Some("Recurse")),
            ("-s", Some("Recurse")),
            ("-LP", Some("LiteralPath")),
            ("-ea", Some("ErrorAction")),
            // The declared parameter wins over common ones sharing a prefix.
            ("-In", Some("Include")),
            ("-e", Some("Exclude")),
            ("-Inf", Some("ambiguous")),
            ("-O", Some("ambiguous")),
            ("-Re", Some("ambiguous")),
            ("-D", Some("ambiguous")),
            ("-ad", Some("Directory")),
            ("-De", Some("Depth")),
            ("-Ver", Some("Verbose")),
            ("-Recrse", None),
            ("-", None),
        ] {
            assert_eq!(bound(&gci, typed).as_deref(), expected, "{typed}");
        }
    }

    #[test]
    fn descriptions_are_read_strictly() {
        let text = "command\tSet-ExecutionPolicy\tCmdlet\tMicrosoft.PowerShell.Security\tshipped\tclosed\n\
                    parameter\tExecutionPolicy\tvalue\tdeclared\tstatic\n\
                    completer\tExecutionPolicy\n\
                    positional\tExecutionPolicy\n\
                    value\tExecutionPolicy\tRemoteSigned\n\
                    value\tExecutionPolicy\tBypass\n\
                    parameter\tForce\tswitch\tdeclared\tstatic\n\
                    parameter\tVerbose\tswitch\tcommon\tstatic\n\
                    alias\tVerbose\tvb\n";
        let Ok(Answer::Described(d)) = parse(text) else {
            panic!("{:?}", parse(text));
        };
        assert_eq!(d.name, "Set-ExecutionPolicy");
        assert!(d.shipped && d.closed());
        assert_eq!(d.parameters[0].values, ["RemoteSigned", "Bypass"]);
        assert!(d.parameters[0].completer);
        assert!(d.parameters[0].positional);
        assert!(d.parameters[1].switch && !d.parameters[1].common);
        assert_eq!(d.parameters[2].aliases, ["vb"]);
        assert_eq!(parse("missing\tNope\n"), Ok(Answer::Missing));
        assert_eq!(
            parse("untrusted\tInvoke-Fake\tFakeTool\n"),
            Ok(Answer::Untrusted {
                command: "Invoke-Fake".into(),
                module: "FakeTool".into()
            })
        );
        for broken in [
            "",
            "parameter\tPath\tvalue\tdeclared\tstatic\n",
            "command\tGet-X\tCmdlet\tM\tshipped\tclosed\nvalue\tPath\tx\n",
            "command\tGet-X\tCmdlet\tM\tshipped\tclosed\nparameter\tPath\tvalue\tdeclared\tstatic\nalias\tOther\tp\n",
            "command\tGet-X\tApplication\tM\tshipped\tclosed\n",
            "command\tGet X\tCmdlet\tM\tshipped\tclosed\n",
            "command\tGet-X\tCmdlet\tM\tshipped\tclosed\nsurprise\n",
        ] {
            assert!(parse(broken).is_err(), "{broken:?}");
        }
    }

    #[test]
    fn values_ignore_case_and_combine_flags_like_powershell() {
        let mut description = get_child_item();
        description.parameters.push(Parameter {
            name: "Attributes".into(),
            values: vec!["Hidden".into(), "ReadOnly".into()],
            flags: true,
            ..Parameter::default()
        });
        description.parameters.push(Parameter {
            name: "Mode".into(),
            values: vec!["Fast".into()],
            exact: true,
            ..Parameter::default()
        });
        let command = Command::described(description);
        let values = |words: &[&str], typed: &str| {
            let items = command.complete_values(words, &mut budget()).unwrap();
            command
                .prepare_value_candidates(words, typed, items)
                .unwrap()
                .into_iter()
                .map(|i| i.value)
                .collect::<Vec<_>>()
        };
        assert_eq!(values(&["-ea"], "stop"), ["stop"]);
        assert_eq!(values(&["-ErrorA"], "Stpo"), ["SilentlyContinue", "Stop"]);
        assert!(values(&["-ea"], "1").is_empty(), "numbers are PowerShell's");
        assert_eq!(
            values(&["-Attributes"], "hidden,readonly"),
            ["hidden,readonly"]
        );
        assert!(values(&["-ea"], "Stop,x").is_empty());
        assert_eq!(values(&["-Mode"], "fast"), ["Fast"]);
        assert!(values(&["-Recurse"], "x").is_empty());
        let mut budget = budget();
        let options: Vec<String> = command
            .complete(&[], "-", &mut budget)
            .unwrap()
            .into_iter()
            .map(|i| i.value)
            .collect();
        assert!(options.contains(&"-Recurse".to_owned()));
        assert!(command.complete(&[], "", &mut budget).unwrap().is_empty());
        assert_eq!(
            command.resolve_option(&[], "-rec", &mut budget),
            Some(CompletionItem {
                value: "-Recurse".into(),
                takes_value: Some(false),
                description: None
            })
        );
        assert_eq!(command.resolve_option(&[], "-Recrse", &mut budget), None);
        assert!(command.capabilities().complete_options);
    }

    #[test]
    fn callback_choices_are_resources_and_keep_their_own_value_semantics() {
        let command = Command::described(Description {
            name: "Get-Fixture".into(),
            parameters: vec![
                Parameter {
                    completer: true,
                    ..parameter("Target", &["t"], false)
                },
                Parameter {
                    values: vec!["json".into()],
                    ..parameter("Format", &[], false)
                },
            ],
            ..Description::default()
        });
        let item = CompletionItem {
            value: "-dash".into(),
            takes_value: None,
            description: Some("Resource".into()),
        };
        assert!(command.candidate_is_resource(&["-t"], &item));
        assert!(!command.is_option(&["-Target"], &item));
        assert!(command.value_syntax_contains_slashes(&["-Target"]));
        assert!(command.values_require_literal_context(&["-Target"]));
        assert!(!command.values_require_literal_context(&["-Format"]));
        assert!(!command.candidate_is_resource(&["-Format"], &item));
        assert_eq!(
            command
                .prepare_value_candidates(&["-Target"], "1", vec![item.clone()])
                .unwrap(),
            [item]
        );
    }

    #[test]
    fn trust_entries_name_modules_and_commands() {
        assert_eq!(
            trust_entries(&[
                "*".into(),
                "powershell:Az.Accounts".into(),
                "Invoke-Fake".into(),
                "bad\nentry".into()
            ]),
            "*\nmodule:Az.Accounts\ncommand:Invoke-Fake"
        );
    }

    #[test]
    fn script_has_no_double_quotes() {
        assert!(!SCRIPT.contains('"'));
    }

    pub(crate) fn pwsh() -> Option<PathBuf> {
        std::env::var_os("NOTYPO_TEST_PWSH")
            .map(PathBuf::from)
            .or_else(|| crate::utils::which("pwsh"))
    }

    /// PowerShell's own static binder decides what each typed name binds
    /// to; the description read from the real probe must agree.
    #[test]
    fn binding_matches_powershells_static_binder() {
        let Some(pwsh) = pwsh() else {
            eprintln!("skipped: PowerShell is not installed");
            return;
        };
        let mut budget = budget();
        let cases: &[(&str, &[&str])] = &[
            (
                "Get-ChildItem",
                &[
                    "-rec",
                    "-r",
                    "-Re",
                    "-In",
                    "-Inf",
                    "-Recrse",
                    "-s",
                    "-ea",
                    "-e",
                    "-Pa",
                    "-LP",
                    "-RECURSE",
                    "-O",
                    "-wa",
                    "-ProgressA",
                    "-Ver",
                    "-D",
                    "-Di",
                    "-dir",
                    "-ad",
                    "-Pipeline",
                    "-Out",
                    "-F",
                    "-Fi",
                    "-Hid",
                    "-Name",
                    "-N",
                    "-x",
                ],
            ),
            (
                "Remove-Item",
                &[
                    "-Wh", "-wi", "-Conf", "-C", "-cf", "-Forc", "-Stream", "-St", "-Rcurse",
                ],
            ),
            (
                "Get-Process",
                &[
                    "-Na",
                    "-Id",
                    "-I",
                    "-Inc",
                    "-Mod",
                    "-FileV",
                    "-Pid",
                    "-Proc",
                    "-ProcessName",
                ],
            ),
            ("gci", &["-rec", "-In", "-Dpth"]),
        ];
        for (command, typed) in cases {
            let backend = Command::new(command, pwsh.clone(), &[]);
            let description = backend.describe(&mut budget).unwrap();
            // A value follows only a parameter that takes one, so nothing
            // binds by position.
            let lines: Vec<String> = typed
                .iter()
                .map(|t| match description.bind(t) {
                    Binding::Bound(p) if !p.switch => format!("{command} {t} x"),
                    _ => format!("{command} {t}"),
                })
                .collect();
            let script = "foreach ($line in $env:NOTYPO_TEST_LINES -split [char]10) { \
                $ast = [System.Management.Automation.Language.Parser]::ParseInput($line, [ref]$null, [ref]$null); \
                $cmd = $ast.Find({ $args[0] -is [System.Management.Automation.Language.CommandAst] }, $true); \
                $r = [System.Management.Automation.Language.StaticParameterBinder]::BindCommand($cmd, $true); \
                $errors = @($r.BindingExceptions.Values | ForEach-Object { $_.BindingException.ErrorId }); \
                if ($errors -contains 'AmbiguousParameter') { 'ambiguous' } \
                elseif ($errors.Count) { '' } \
                else { @($r.BoundParameters.Keys) -join ',' } }";
            let output = std::process::Command::new(&pwsh)
                .args(["-NoProfile", "-NonInteractive", "-Command", script])
                .env("NOTYPO_TEST_LINES", lines.join("\n"))
                .output()
                .unwrap();
            let text = String::from_utf8(output.stdout).unwrap();
            let expected: Vec<&str> = text.lines().collect();
            assert_eq!(expected.len(), typed.len(), "{command}: {text}");
            for (typed, expected) in typed.iter().zip(expected) {
                let ours = bound(&description, typed).unwrap_or_default();
                assert_eq!(ours, expected, "{command} {typed}");
            }
        }
    }

    #[test]
    fn real_powershell_describes_shipped_commands_and_refuses_other_modules() {
        let Some(pwsh) = pwsh() else {
            eprintln!("skipped: PowerShell is not installed");
            return;
        };
        let mut budget = budget();
        let gci = Command::new("gci", pwsh.clone(), &[]);
        let description = gci.describe(&mut budget).unwrap();
        assert_eq!(description.name, "Get-ChildItem");
        assert!(description.shipped && description.closed());
        assert!(
            description
                .parameters
                .iter()
                .any(|p| p.name == "Recurse" && p.switch && p.aliases.contains(&"s".into()))
        );
        let policy = Command::new("Set-ExecutionPolicy", pwsh.clone(), &[]);
        let values = policy
            .complete_values(&["-ExecutionPolicy"], &mut budget)
            .unwrap();
        assert!(values.iter().any(|v| v.value == "RemoteSigned"));
        assert!(
            Command::new("Get-Command", pwsh.clone(), &[])
                .describe(&mut budget)
                .unwrap()
                .open,
            "Get-Command takes remaining arguments"
        );
        assert!(matches!(
            Command::new("Nope-Thing", pwsh.clone(), &[]).describe(&mut budget),
            Err(CompletionError::Failed(_))
        ));
        let names = commands(&pwsh, &mut budget).unwrap();
        assert!(names.iter().any(|n| n == "Get-ChildItem"));
        assert!(names.iter().any(|n| n == "gci"));

        // A module outside $PSHOME is never imported without trust.
        let dir = std::env::temp_dir().join(format!("notypo-ps-{}", std::process::id()));
        let module = dir.join("FakeTool");
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(
            module.join("FakeTool.psm1"),
            "Set-Content -Path (Join-Path $PSScriptRoot 'imported') -Value 'yes'\n\
             function Invoke-Fake { [CmdletBinding()] param([ValidateSet('One','Two')][string]$Mode, [switch]$Loud) }\n\
             Export-ModuleMember -Function Invoke-Fake\n",
        )
        .unwrap();
        std::fs::write(
            module.join("FakeTool.psd1"),
            "@{ RootModule = 'FakeTool.psm1'; ModuleVersion = '1.0.0'; \
             GUID = '1b0b5c3e-4c39-4e33-9f55-0f4c1c7d2a11'; FunctionsToExport = @('Invoke-Fake') }\n",
        )
        .unwrap();
        let ask = |trusted: &[String]| {
            // PowerShell reconstructs PSModulePath during Windows startup.
            // Add this isolated fixture to the actual child's search path.
            let script = format!(
                "$env:PSModulePath = $env:NOTYPO_TEST_MODULE_ROOT + [IO.Path]::PathSeparator + $env:PSModulePath; & {{ {SCRIPT} }}; [Console]::Error.WriteLine('fixture available modules: ' + @(Microsoft.PowerShell.Core\\Get-Module -ListAvailable -Name FakeTool).Count)"
            );
            let output = std::process::Command::new(&pwsh)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-NoLogo",
                    "-Command",
                    &script,
                ])
                .envs(
                    environment(&[
                        ("NOTYPO_PS_QUERY", "describe"),
                        ("NOTYPO_PS_NAME", "Invoke-Fake"),
                        ("NOTYPO_PS_TRUSTED", &trust_entries(trusted)),
                    ])
                    .into_iter()
                    .filter_map(|(k, v)| Some((k, v?))),
                )
                .env("NOTYPO_TEST_MODULE_ROOT", &dir)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let answer = parse(&String::from_utf8(output.stdout).unwrap()).unwrap();
            assert_ne!(
                answer,
                Answer::Missing,
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            answer
        };
        assert_eq!(
            ask(&[]),
            Answer::Untrusted {
                command: "Invoke-Fake".into(),
                module: "FakeTool".into()
            }
        );
        assert!(!module.join("imported").exists());
        let Answer::Described(fake) = ask(&["powershell:faketool".into()]) else {
            panic!("trusted module was not described");
        };
        assert!(!fake.shipped && fake.closed());
        assert_eq!(fake.parameters[0].values, ["One", "Two"]);
        assert!(module.join("imported").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
