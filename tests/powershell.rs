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
    let mut child = Command::new(shell)
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
