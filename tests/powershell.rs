//! Real shell integration on Unix, PowerShell 7 on Windows, and Windows
//! PowerShell 5.1 when available. No Unix wrapper scripts are needed.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{fs, thread};

struct Workspace(PathBuf);
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn runtimes() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::env::var_os("NOTYPO_TEST_PWSH")
        .map(PathBuf::from)
        .or_else(|| notypo::utils::which("pwsh"))
        .into_iter()
        .collect();
    if cfg!(windows)
        && let Some(classic) = notypo::utils::which("powershell")
    {
        paths.push(classic);
    }
    assert!(
        !paths.is_empty() || std::env::var("NOTYPO_REQUIRE_PWSH").as_deref() != Ok("1"),
        "PowerShell runtime is required for this validation run"
    );
    if paths.is_empty() {
        eprintln!("unverified: no PowerShell runtime is installed");
    }
    paths
}

fn run(shell: &Path, dir: &Path, input: &str) -> (Vec<u8>, Vec<u8>) {
    let mut process = Command::new(shell);
    // Windows PowerShell cannot load PowerShell 7's standard modules.
    // Let 5.1 construct its own defaults when the fixture launches it.
    if shell
        .file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("powershell"))
    {
        process.env_remove("PSModulePath");
    }
    let mut child = process
        .current_dir(dir)
        .env("XDG_CONFIG_HOME", dir)
        .env("XDG_CACHE_HOME", dir)
        .env("XDG_DATA_HOME", dir)
        .env("THEFUCK_RULES", "")
        .env("THEFUCK_ALTER_HISTORY", "false")
        .env("THEFUCK_NO_COLORS", "true")
        .env("NOTYPO_NO_CACHE", "1")
        .env("NOTYPO_TRUSTED_COMPLETERS", r#"["Get-CoreFixture"]"#)
        .env("NOTYPO_DISABLED_SOURCES", "history:legacy:help:man")
        .env_remove("TF_HISTORY")
        .env_remove("NOTYPO_CURRENT_COMMAND")
        .env_remove("NOTYPO_POWERSHELL_PARAMETERS")
        .env_remove("NOTYPO_POWERSHELL_COMPLETIONS")
        .args(["-NoProfile", "-NoLogo", "-Command", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let output = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let errors = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).unwrap();
        bytes
    });
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("PowerShell integration did not finish within 60 seconds");
        }
        thread::sleep(Duration::from_millis(10));
    }
    (output.join().unwrap(), errors.join().unwrap())
}

#[test]
fn shell_context_and_parameter_repairs_execute_each_selected_command_once() {
    for (index, shell) in runtimes().into_iter().enumerate() {
        let workspace = Workspace(std::env::temp_dir().join(format!(
            "notypo-core-powershell-{}-{index}",
            std::process::id()
        )));
        fs::create_dir(&workspace.0).unwrap();
        let marker = workspace.0.join("operation.log");
        let quote = |value: &str| format!("'{}'", value.replace('\'', "''"));
        let input = format!(
            "$env:TF_SHELL = 'powershell'\n\
             Invoke-Expression ((& {} --alias corefix) -join \"`n\")\n\
             function Get-CoreFixture {{ [CmdletBinding()] param([string] $Name) Add-Content -LiteralPath {} -Value $Name }}\n\
             Get-CoreFixture -Nmae west\ncorefix -y\n\
             Get-CoreFixture -Nmae 'two words'\ncorefix -y\n\
             [Console]::Out.WriteLine('context-left:' + [bool]$env:NOTYPO_EXIT_STATUS + ':' + [bool]$env:NOTYPO_CURRENT_COMMAND)\nexit\n",
            quote(env!("CARGO_BIN_EXE_notypo")),
            quote(marker.to_str().unwrap()),
        );
        let (output, errors) = run(&shell, &workspace.0, &input);
        let detail = format!(
            "{}\n{}\n{}",
            shell.display(),
            String::from_utf8_lossy(&output),
            String::from_utf8_lossy(&errors)
        );
        let contents = fs::read_to_string(&marker).unwrap_or_default();
        assert_eq!(
            contents.lines().collect::<Vec<_>>(),
            ["west", "two words"],
            "{detail}"
        );
        assert!(
            String::from_utf8_lossy(&output).contains("context-left:False:False"),
            "{detail}"
        );
    }
}

#[test]
fn selected_echo_commands_are_executed_without_stripping_their_program_name() {
    for (index, shell) in runtimes().into_iter().enumerate() {
        let workspace = Workspace(
            std::env::temp_dir().join(format!("notypo-core-echo-{}-{index}", std::process::id())),
        );
        fs::create_dir(&workspace.0).unwrap();
        let marker = workspace.0.join("operation.log");
        let quote = |value: &str| format!("'{}'", value.replace('\'', "''"));
        let function =
            notypo::shells::Shell::Powershell.app_alias("corefix", "Read-CoreSelection", false);
        let definition = workspace.0.join("alias.ps1");
        fs::write(&definition, function).unwrap();
        let input = format!(
            "function Read-CoreSelection {{ $env:NOTYPO_TEST_SELECTED }}\n\
             function echoish {{ Add-Content -LiteralPath {} -Value 'once' }}\n\
             . {}\n\
             $env:NOTYPO_TEST_SELECTED = 'echo ''once'''\nWrite-Output previous\ncorefix\n\
             $env:NOTYPO_TEST_SELECTED = 'echoish'\nWrite-Output previous\ncorefix\nexit\n",
            quote(marker.to_str().unwrap()),
            quote(definition.to_str().unwrap()),
        );
        let (output, errors) = run(&shell, &workspace.0, &input);
        let detail = format!(
            "{}\n{}\n{}",
            shell.display(),
            String::from_utf8_lossy(&output),
            String::from_utf8_lossy(&errors)
        );
        assert_eq!(
            String::from_utf8_lossy(&output).matches("once").count(),
            1,
            "{detail}"
        );
        assert_eq!(
            fs::read_to_string(&marker)
                .unwrap_or_default()
                .lines()
                .collect::<Vec<_>>(),
            ["once"],
            "{detail}"
        );
    }
}

fn reports(output: &[u8]) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let start = line.find('{')?;
            serde_json::from_str::<serde_json::Value>(&line[start..])
                .ok()
                .filter(|value| value.get("candidates").is_some())
        })
        .collect()
}

#[test]
fn dynamic_parameters_follow_literal_context_through_the_generated_alias() {
    for (index, shell) in runtimes().into_iter().enumerate() {
        let workspace = Workspace(std::env::temp_dir().join(format!(
            "notypo-core-dynamic-{}-{index}",
            std::process::id()
        )));
        fs::create_dir(&workspace.0).unwrap();
        let marker = workspace.0.join("operation.log");
        let quote = |value: &str| format!("'{}'", value.replace('\'', "''"));
        let definition = "function Get-CoreFixture { [CmdletBinding()] param([string]$Scope) dynamicparam { $set = [System.Management.Automation.RuntimeDefinedParameterDictionary]::new(); if ($Scope -in @('west','-west')) { $attributes = [System.Collections.ObjectModel.Collection[System.Attribute]]::new(); $attributes.Add([System.Management.Automation.ParameterAttribute]::new()); $attributes.Add([System.Management.Automation.ValidateSetAttribute]::new([string[]]@('one','two'))); $set.Add('Flavor', [System.Management.Automation.RuntimeDefinedParameter]::new('Flavor',[string],$attributes)) }; $set } process { [IO.File]::WriteAllText(MARKER,'ran') } }".replace("MARKER", &quote(marker.to_str().unwrap()));
        let input = format!(
            "$env:TF_SHELL = 'powershell'\nInvoke-Expression ((& {} --alias corefix) -join \"`n\")\n{definition}\n\
             Get-CoreFixture -Flavro one -Scope west\ncorefix --json\n\
             Get-CoreFixture -Scope west -Flavor oen\ncorefix --json\n\
             Get-CoreFixture '-west' -Flavro two\ncorefix --json\nexit\n",
            quote(env!("CARGO_BIN_EXE_notypo"))
        );
        let (output, errors) = run(&shell, &workspace.0, &input);
        let detail = format!(
            "{}\n{}\n{}",
            shell.display(),
            String::from_utf8_lossy(&output),
            String::from_utf8_lossy(&errors)
        );
        let reports = reports(&output);
        assert_eq!(reports.len(), 3, "{detail}");
        for (report, expected) in reports.iter().zip([
            "Get-CoreFixture -Flavor one -Scope west",
            "Get-CoreFixture -Scope west -Flavor one",
            "Get-CoreFixture '-west' -Flavor two",
        ]) {
            assert_eq!(
                report["candidates"][0]["command"].as_str(),
                Some(expected),
                "{detail}"
            );
        }
        assert!(
            !marker.exists(),
            "discovery executed the failed operation: {detail}"
        );
    }
}

#[test]
fn parent_provider_location_does_not_lend_filesystem_parameters() {
    for (index, shell) in runtimes().into_iter().enumerate() {
        let workspace = Workspace(std::env::temp_dir().join(format!(
            "notypo-core-provider-{}-{index}",
            std::process::id()
        )));
        fs::create_dir(&workspace.0).unwrap();
        let quote = |value: &str| format!("'{}'", value.replace('\'', "''"));
        let input = format!(
            "$env:TF_SHELL = 'powershell'\nInvoke-Expression ((& {} --alias corefix) -join \"`n\")\n\
             Set-Location Env:\nGet-ChildItem -Direcotry\ncorefix --json\n\
             [Console]::Out.WriteLine('provider-context-left:' + [bool]$env:NOTYPO_POWERSHELL_LOCATION)\nexit\n",
            quote(env!("CARGO_BIN_EXE_notypo"))
        );
        let (output, errors) = run(&shell, &workspace.0, &input);
        let detail = format!(
            "{}\n{}\n{}",
            shell.display(),
            String::from_utf8_lossy(&output),
            String::from_utf8_lossy(&errors)
        );
        let reports = reports(&output);
        assert_eq!(reports.len(), 1, "{detail}");
        assert!(
            !reports[0]["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .any(|candidate| candidate["command"]
                    .as_str()
                    .is_some_and(|command| command.contains("-Directory"))),
            "{detail}"
        );
        assert!(
            String::from_utf8_lossy(&output).contains("provider-context-left:False"),
            "{detail}"
        );
    }
}

#[test]
fn parameter_callbacks_restore_primitive_closure_and_private_module_state() {
    for (index, shell) in runtimes().into_iter().enumerate() {
        for kind in ["closure", "module"] {
            let workspace = Workspace(std::env::temp_dir().join(format!(
                "notypo-core-capture-{}-{index}-{kind}",
                std::process::id()
            )));
            fs::create_dir(&workspace.0).unwrap();
            let quote = |value: &str| format!("'{}'", value.replace('\'', "''"));
            let operations = workspace.0.join("operations");
            let imports = workspace.0.join("imports");
            let setup = workspace.0.join("setup.ps1");
            let registration = if kind == "closure" {
                "$callback = & { $team = 'west'; { param($commandName,$parameterName,$word,$ast,$bound) [System.Management.Automation.CompletionResult]::new(($team + '-one'),($team + '-one'),'ParameterValue','Captured team') }.GetNewClosure() }; Register-ArgumentCompleter -CommandName Get-CoreFixture -ParameterName Target -ScriptBlock $callback".to_owned()
            } else {
                format!(
                    "$null = New-Module -Name CapturedFixture -ScriptBlock {{ [IO.File]::AppendAllText({},'once'); $script:teams = [string[]]@('west-one','west-two'); function Get-TeamValues {{ $script:teams }}; Register-ArgumentCompleter -CommandName Get-CoreFixture -ParameterName Target -ScriptBlock {{ param($commandName,$parameterName,$word,$ast,$bound) foreach ($team in (Get-TeamValues)) {{ [System.Management.Automation.CompletionResult]::new($team,$team,'ParameterValue','Private module team') }} }} }}",
                    quote(imports.to_str().unwrap())
                )
            };
            fs::write(&setup, format!(
                "function Get-CoreFixture {{ [CmdletBinding()] param([string]$Target) [IO.File]::AppendAllText({},'original'); throw 'fixture failure' }}\n{registration}\n",
                quote(operations.to_str().unwrap())
            )).unwrap();
            let input = format!(
                "$env:TF_SHELL = 'powershell'\nInvoke-Expression ((& {} --alias corefix) -join \"BTn\")\n. {}\nGet-CoreFixture -Target wesst-one\ncorefix --json\nexit\n",
                quote(env!("CARGO_BIN_EXE_notypo")), quote(setup.to_str().unwrap())
            ).replace("BT", &char::from(96).to_string());
            let (output, errors) = run(&shell, &workspace.0, &input);
            let detail = format!(
                "{kind}: {}\n{}\n{}",
                shell.display(),
                String::from_utf8_lossy(&output),
                String::from_utf8_lossy(&errors)
            );
            let reports = reports(&output);
            assert_eq!(reports.len(), 1, "{detail}");
            assert_eq!(
                reports[0]["candidates"][0]["command"].as_str(),
                Some("Get-CoreFixture -Target west-one"),
                "{detail}"
            );
            assert_eq!(
                reports[0]["candidates"][0]["safety"]["decision"], "confirm",
                "{detail}"
            );
            assert_eq!(
                fs::read_to_string(&operations).unwrap(),
                "original",
                "discovery replayed the operation: {detail}"
            );
            if kind == "module" {
                assert_eq!(
                    fs::read_to_string(&imports).unwrap(),
                    "once",
                    "module initialization was replayed: {detail}"
                );
            }
        }
    }
}

#[test]
fn native_callbacks_restore_session_state_through_the_generated_alias() {
    for (index, shell) in runtimes().into_iter().enumerate() {
        for kind in ["closure", "module"] {
            let workspace = Workspace(std::env::temp_dir().join(format!(
                "notypo-core-native-capture-{}-{index}-{kind}",
                std::process::id()
            )));
            fs::create_dir(&workspace.0).unwrap();
            let quote = |value: &str| format!("'{}'", value.replace('\'', "''"));
            let imports = workspace.0.join("imports");
            let setup = workspace.0.join("setup.ps1");
            let registration = if kind == "closure" {
                "$callback = & { $choice = '--help'; { param($word,$ast,$cursor) $choice }.GetNewClosure() }; Register-ArgumentCompleter -CommandName notypo -ScriptBlock $callback".to_owned()
            } else {
                format!(
                    "$null = New-Module -Name NativeFixture -ScriptBlock {{ [IO.File]::AppendAllText({},'once'); $script:choices = [string[]]@('--help','--version'); function Get-Choices {{ $script:choices }}; Register-ArgumentCompleter -CommandName notypo -ScriptBlock {{ param($word,$ast,$cursor) Get-Choices }} }}",
                    quote(imports.to_str().unwrap())
                )
            };
            fs::write(&setup, registration).unwrap();
            let binary = Path::new(env!("CARGO_BIN_EXE_notypo"));
            let input = format!(
                "$env:PATH = {} + [IO.Path]::PathSeparator + $env:PATH\n$env:NOTYPO_TRUSTED_COMPLETERS = '[\"notypo\"]'\nInvoke-Expression ((& {} --alias corefix) -join \"BTn\")\n. {}\nnotypo --hepl\ncorefix --json\n[Console]::Out.WriteLine('native-snapshot-left:' + [bool]$env:NOTYPO_POWERSHELL_COMPLETIONS)\nexit\n",
                quote(binary.parent().unwrap().to_str().unwrap()), quote(binary.to_str().unwrap()), quote(setup.to_str().unwrap())
            ).replace("BT", &char::from(96).to_string());
            let (output, errors) = run(&shell, &workspace.0, &input);
            let detail = format!(
                "{kind}: {}\n{}\n{}",
                shell.display(),
                String::from_utf8_lossy(&output),
                String::from_utf8_lossy(&errors)
            );
            let reports = reports(&output);
            assert_eq!(reports.len(), 1, "{detail}");
            assert_eq!(
                reports[0]["candidates"][0]["command"], "notypo --help",
                "{detail}"
            );
            assert!(
                String::from_utf8_lossy(&output).contains("native-snapshot-left:False"),
                "{detail}"
            );
            if kind == "module" {
                assert_eq!(fs::read_to_string(&imports).unwrap(), "once", "{detail}");
            }
        }
    }
}
