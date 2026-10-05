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
//!
//! Parameter names follow PowerShell's binder: case doesn't matter, an alias
//! or a prefix of one parameter names it, and a prefix shared by several
//! names the command's own parameter when only one of them isn't a common
//! parameter (`-In` is `-Include`, not `-InformationAction`).

use super::{BLOCKED_PROXY, Capabilities, CompletionError, CompletionItem, Trust, run_stdout};
use crate::engine::probe::{self, Budget};
use std::cell::RefCell;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Passed with `-Command`, so it holds no double quotes: Windows argument
/// quoting can't then change it. Output is one tab-separated record per
/// line; a record with a control character in any field is left out.
const SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false } catch { }
$tab = [string][char]9
function Emit([string[]] $Fields) {
    foreach ($field in $Fields) { if ($field -match '[\x00-\x1f\x7f]') { return } }
    [Console]::Out.Write(($Fields -join $tab) + [char]10)
}
$order = 'Alias', 'Function', 'Filter', 'Cmdlet'
if ($env:NOTYPO_PS_QUERY -eq 'list') {
    foreach ($command in Get-Command -CommandType $order -ErrorAction SilentlyContinue) {
        Emit 'command', $command.Name
    }
    return
}
$name = $env:NOTYPO_PS_NAME
$trusted = @([string]$env:NOTYPO_PS_TRUSTED -split [char]10 | Where-Object { $_ })
$separator = [IO.Path]::DirectorySeparatorChar
$shippedRoot = [IO.Path]::GetFullPath($PSHOME).TrimEnd($separator) + $separator
for ($hop = 0; $hop -lt 4; $hop++) {
    # A wildcard lookup reads module manifests without importing modules.
    $pattern = $name.Substring(0, $name.Length - 1) + '[' + $name[-1] + ']'
    $command = Get-Command -Name $pattern -CommandType $order -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -eq $name } |
        Sort-Object { $order.IndexOf([string]$_.CommandType) } |
        Select-Object -First 1
    if (-not $command -or [string]$command.CommandType -ne 'Alias') { break }
    $name = $command.Definition
    if ($name -notmatch '^[A-Za-z0-9_.-]+$') { $command = $null; break }
}
if (-not $command -or [string]$command.CommandType -eq 'Alias') { Emit 'missing', $name; return }
$module = [string]$command.ModuleName
$shipped = -not $command.Module -or
    ([IO.Path]::GetFullPath($command.Module.ModuleBase).TrimEnd($separator) + $separator).StartsWith($shippedRoot, [StringComparison]::OrdinalIgnoreCase)
if (-not ($shipped -or $trusted -contains '*' -or $trusted -contains ('module:' + $module) -or $trusted -contains ('command:' + $command.Name))) {
    Emit 'untrusted', $command.Name, $module
    return
}
$lookup = @{ Name = $command.Name; CommandType = $command.CommandType }
if ($command.Module) { $lookup.Module = $module }
$info = Get-Command @lookup | Select-Object -First 1
$binding = if ($info -is [System.Management.Automation.CmdletInfo]) {
    $info.ImplementingType.GetCustomAttributes([System.Management.Automation.CmdletAttribute], $true) | Select-Object -First 1
} else {
    $info.ScriptBlock.Attributes | Where-Object { $_ -is [System.Management.Automation.CmdletBindingAttribute] } | Select-Object -First 1
}
$open = -not $binding
$common = @([System.Management.Automation.Cmdlet]::CommonParameters) + @([System.Management.Automation.Cmdlet]::OptionalCommonParameters)
if ($binding -and $binding.SupportsPaging) { $common += 'First', 'Skip', 'IncludeTotalCount' }
$parameters = @(if ($info.Parameters) { $info.Parameters.Values })
foreach ($parameter in $parameters) {
    foreach ($attribute in $parameter.Attributes) {
        if ($attribute -is [System.Management.Automation.ParameterAttribute] -and $attribute.ValueFromRemainingArguments) { $open = $true }
    }
}
Emit 'command', $info.Name, ([string]$info.CommandType), $module, $(if ($shipped) { 'shipped' } else { 'trusted' }), $(if ($open) { 'open' } else { 'closed' })
foreach ($parameter in $parameters) {
    Emit 'parameter', $parameter.Name, $(if ($parameter.SwitchParameter) { 'switch' } else { 'value' }), $(if ($common -contains $parameter.Name) { 'common' } else { 'declared' }), $(if ($parameter.IsDynamic) { 'dynamic' } else { 'static' })
    foreach ($alias in $parameter.Aliases) { Emit 'alias', $parameter.Name, $alias }
    $type = $parameter.ParameterType
    if ($type.IsArray) { $type = $type.GetElementType() }
    $underlying = [Nullable]::GetUnderlyingType($type)
    if ($underlying) { $type = $underlying }
    if ($type.IsEnum) {
        foreach ($value in [Enum]::GetNames($type)) { Emit 'value', $parameter.Name, $value }
        if ($type.IsDefined([FlagsAttribute], $false)) { Emit 'flags', $parameter.Name }
    }
    foreach ($attribute in $parameter.Attributes) {
        if ($attribute -is [System.Management.Automation.ValidateSetAttribute]) {
            foreach ($value in $attribute.ValidValues) { Emit 'value', $parameter.Name, $value }
            if (-not $attribute.IgnoreCase) { Emit 'exact', $parameter.Name }
        }
    }
}
"#;

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
                    parameters: Vec::new(),
                });
            }
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
            _ => return Err(format!("unexpected PowerShell record `{line}`")),
        }
    }
    description
        .map(Answer::Described)
        .ok_or_else(|| "PowerShell described nothing".to_owned())
}

fn parameter_name(name: &str) -> bool {
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
    let mut env: Vec<_> = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ]
    .into_iter()
    .map(|k| set(k, BLOCKED_PROXY))
    .chain(["NO_PROXY", "no_proxy"].map(|k| (OsString::from(k), None)))
    .collect();
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
        ["NOTYPO_PS_QUERY", "NOTYPO_PS_NAME", "NOTYPO_PS_TRUSTED"]
            .map(|k| (OsString::from(k), None)),
    );
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
    let text = run(shell, &[("NOTYPO_PS_QUERY", "list")], budget)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut names = Vec::new();
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        match line.split('\t').collect::<Vec<_>>().as_slice() {
            [] | [""] => {}
            ["command", name] if is_command_name(name) => names.push((*name).to_owned()),
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
}

impl Command {
    pub fn new(name: &str, shell: PathBuf, trusted: &[String]) -> Command {
        Command {
            name: name.to_owned(),
            shell,
            trusted: trust_entries(trusted),
            described: RefCell::new(None),
        }
    }

    /// A command whose description is already known (tests and embedders).
    pub fn described(description: Description) -> Command {
        Command {
            name: description.name.clone(),
            shell: PathBuf::new(),
            trusted: String::new(),
            described: RefCell::new(Some(Ok(Rc::new(description)))),
        }
    }

    pub fn describe(&self, budget: &mut Budget) -> Result<Rc<Description>, CompletionError> {
        if let Some(known) = self.described.borrow().as_ref() {
            return known.clone();
        }
        let answer = if is_command_name(&self.name) {
            run(
                &self.shell,
                &[
                    ("NOTYPO_PS_QUERY", "describe"),
                    ("NOTYPO_PS_NAME", &self.name),
                    ("NOTYPO_PS_TRUSTED", &self.trusted),
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
        let result = match answer {
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
        };
        // A spent budget may recover on a later request; other answers stand.
        if result != Err(CompletionError::BudgetExhausted) {
            *self.described.borrow_mut() = Some(result.clone());
        }
        result
    }

    fn known(&self) -> Option<Rc<Description>> {
        self.described.borrow().as_ref()?.as_ref().ok().cloned()
    }

    fn option(parameter: &Parameter) -> CompletionItem {
        CompletionItem {
            value: format!("-{}", parameter.name),
            takes_value: Some(!parameter.switch),
            description: None,
        }
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
            option_arity: true,
            values: true,
            resources: false,
            option_prefix: "-",
            short_options: true,
            // A level with no subcommands: positional words are arguments.
            complete_subcommands: true,
            complete_options: known.as_ref().is_some_and(|d| d.closed()),
            descriptions: false,
            query_dialect: None,
            trust: match known {
                Some(d) if !d.shipped => Trust::UserTrusted,
                _ => Trust::Bridge,
            },
        }
    }

    fn complete(
        &self,
        _words: &[&str],
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let description = self.describe(budget)?;
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
        let description = self.describe(budget)?;
        let Some(option) = words.last().filter(|w| w.starts_with('-')) else {
            return Ok(Vec::new());
        };
        let Binding::Bound(parameter) = description.bind(option) else {
            return Ok(Vec::new());
        };
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
        let Some(parameter) = parameter else {
            return Ok(items);
        };
        let same = |a: &str, b: &str| {
            if parameter.exact {
                a == b
            } else {
                a.to_lowercase() == b.to_lowercase()
            }
        };
        if typed.parse::<i64>().is_ok() {
            return Ok(Vec::new());
        }
        let parts: Vec<&str> = typed.split(',').map(str::trim).collect();
        let accepted = (parameter.flags || parts.len() == 1)
            && parts
                .iter()
                .all(|part| items.iter().any(|item| same(&item.value, part)));
        if accepted {
            return Ok(vec![CompletionItem {
                value: typed.to_owned(),
                takes_value: None,
                description: None,
            }]);
        }
        // Lists and combinations are left to PowerShell.
        if parts.len() > 1 {
            return Ok(Vec::new());
        }
        Ok(items)
    }

    fn resolve_option(
        &self,
        _words: &[&str],
        typed: &str,
        budget: &mut Budget,
    ) -> Option<CompletionItem> {
        let description = self.describe(budget).ok()?;
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

#[cfg(test)]
pub(crate) mod tests {
    use super::super::NativeCompletionBackend;
    use super::*;
    use std::time::Duration;

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
        let path = std::env::join_paths(
            std::iter::once(dir.clone()).chain(
                std::env::var_os("PSModulePath")
                    .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
                    .unwrap_or_default(),
            ),
        )
        .unwrap();
        let ask = |trusted: &[String]| {
            let text = std::process::Command::new(&pwsh)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-NoLogo",
                    "-Command",
                    SCRIPT,
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
                .env("PSModulePath", &path)
                .output()
                .unwrap()
                .stdout;
            parse(&String::from_utf8(text).unwrap()).unwrap()
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
