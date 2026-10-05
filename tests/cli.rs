#![cfg(unix)]

use notypo::logs::USER_COMMAND_MARK;
use notypo::shells::Shell;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use std::{fs, thread};

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "notypo-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn command(&self, rules: &str) -> Command {
        self.program(env!("CARGO_BIN_EXE_notypo"), rules)
    }

    fn program(&self, program: impl AsRef<std::ffi::OsStr>, rules: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.0)
            .env("XDG_CONFIG_HOME", &self.0)
            .env("XDG_CACHE_HOME", &self.0)
            .env("XDG_DATA_HOME", &self.0)
            .env("HISTFILE", self.0.join("history"))
            .env("TF_SHELL", "bash")
            .env("TF_ALIAS", "fuck")
            .env("THEFUCK_RULES", rules)
            .env("THEFUCK_EXCLUDE_RULES", "")
            .env("THEFUCK_ALTER_HISTORY", "false")
            .env("THEFUCK_NO_COLORS", "true")
            .env("THEFUCK_DEBUG", "false")
            .env("THEFUCK_INSTANT_MODE", "false")
            .env("THEFUCK_REPEAT", "false")
            .env("NOTYPO_NO_CACHE", "1")
            .env_remove("TF_HISTORY")
            .env_remove("NOTYPO_CURRENT_COMMAND")
            .env_remove("NOTYPO_EXIT_STATUS")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .env_remove("NOTYPO_BASH_COMPLETIONS")
            .env_remove("NOTYPO_ZSH_COMPLETIONS")
            .env_remove("NOTYPO_FISH_COMPLETIONS")
            .env_remove("NOTYPO_POWERSHELL")
            .env_remove("NOTYPO_POWERSHELL_COMMANDS")
            .env_remove("NOTYPO_POWERSHELL_ERRORS")
            .env_remove("NOTYPO_FISH_COMPLETE_PATH")
            .env_remove("NOTYPO_ZSH_FPATH")
            .env_remove("NOTYPO_FISH_HISTORY_SESSION")
            .env_remove("fish_history")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("SHELL_LOGGER_SOCKET")
            .env_remove("THEFUCK_OUTPUT_LOG")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        command
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn finish(child: Child) -> Output {
    finish_within(child, Duration::from_secs(10))
}

fn finish_within(mut child: Child, limit: Duration) -> Output {
    let deadline = Instant::now() + limit;
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("CLI did not exit within {limit:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn corrects_commands_through_the_cli() {
    let workspace = Workspace::new();
    // As the shell function runs it: the history and the exit status.
    let output = finish(
        workspace
            .command("cd_parent")
            .env("TF_HISTORY", "cd..\nfuck -y")
            .env("NOTYPO_EXIT_STATUS", "127")
            .arg("-y")
            .spawn()
            .unwrap(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "cd ..");
    // mkdir_p needs the error output: only an allowed replay provides it.
    let output = finish(
        workspace
            .command("mkdir_p")
            .args(["-y", "--force-command", "mkdir nested/dir"])
            .spawn()
            .unwrap(),
    );
    assert!(!output.status.success(), "without output there is no fix");
    let output = finish(
        workspace
            .command("mkdir_p")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "true")
            .args(["-y", "--force-command", "mkdir nested/dir"])
            .spawn()
            .unwrap(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "mkdir -p nested/dir"
    );
    assert!(
        !workspace.0.join("nested").exists(),
        "the CLI prints suggestions for the shell to execute"
    );
}

#[test]
fn instant_mode_uses_captured_output_without_rerunning() {
    let workspace = Workspace::new();
    let script = "printf rerun > marker";
    let log = workspace.0.join("output.log");
    fs::write(
        &log,
        format!(
            "{USER_COMMAND_MARK}$ {script}\r\nPermission denied\r\n{USER_COMMAND_MARK}$ fuck\r\n"
        ),
    )
    .unwrap();
    let output = finish(
        workspace
            .command("sudo")
            .env("THEFUCK_INSTANT_MODE", "true")
            .env("THEFUCK_OUTPUT_LOG", &log)
            .env("PS1", format!("{USER_COMMAND_MARK}$ "))
            .args(["-y", "--force-command", script])
            .spawn()
            .unwrap(),
    );
    // The sudo rule matches the captured output, but adding sudo needs a
    // person's approval, and `-y` without a terminal can't give it.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(output.stdout.is_empty());
    assert!(stderr.contains("sudo sh -c"), "{stderr}");
    assert!(stderr.contains("elevated privileges"), "{stderr}");
    assert!(!workspace.0.join("marker").exists());
}

#[test]
fn shell_logger_records_output_and_preserves_exit_status() {
    let workspace = Workspace::new();
    let log = workspace.0.join("output.log");
    let mut child = workspace
        .command("")
        .env("SHELL", "/bin/sh")
        .arg("--shell-logger")
        .arg(&log)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"printf 'native logger\\n'\nexit 7\n")
        .unwrap();
    let output = finish(child);
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let recorded = fs::read(&log).unwrap();
    assert_eq!(recorded.len(), 1024 * 1024);
    assert!(String::from_utf8_lossy(&recorded).contains("native logger\r\n"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("native logger\r\n"));
}

#[test]
fn instant_alias_is_valid_for_bash_and_zsh() {
    let workspace = Workspace::new();
    for shell in ["bash", "zsh"] {
        let output = finish(
            workspace
                .command("")
                .env("TF_SHELL", shell)
                .env("THEFUCK_INSTANT_MODE", "true")
                .args(["--alias", "--enable-experimental-instant-mode"])
                .spawn()
                .unwrap(),
        );
        assert!(output.status.success());
        let alias = String::from_utf8(output.stdout).unwrap();
        assert!(alias.contains(USER_COMMAND_MARK));
        assert!(alias.contains("TF_ALIAS=fuck"));
        assert!(alias.contains("TF_ALIAS=typo"));
        assert_eq!(alias.matches("export PS1=").count(), 1);
        if notypo::utils::which(shell).is_some() {
            let output = Command::new(shell)
                .args(["-n", "-c", &alias])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn zsh_alias_corrects_command_from_current_multiline_history_event() {
    let Some(zsh) = notypo::utils::which("zsh") else {
        return;
    };
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let git = bin.join("git");
    fs::write(
        &git,
        "#!/bin/sh\ncase $1 in\n  sttus) printf \"git: 'sttus' is not a git command. See 'git --help'.\\n\\nThe most similar command is\\n\\tstatus\\n\"; exit 1;;\n  status) printf 'correction executed\\n';;\n  --list-cmds=*) printf 'status\\n';;\n  -h) printf 'usage: git [--version]\\n'; exit 129;;\n  *) exit 1;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    for name in ["fuck", "typo", "fix"] {
        let mut command = workspace.command("git_not_command");
        command.env("TF_SHELL", "zsh").env("TF_ALIAS", "typo");
        if name == "fix" {
            command.args(["--alias", name]);
        } else {
            command.arg("--alias");
        }
        let output = finish(command.spawn().unwrap());
        assert!(output.status.success());
        let alias = String::from_utf8(output.stdout).unwrap();
        if name == "fix" {
            assert!(!alias.contains("TF_ALIAS=fuck"));
            assert!(!alias.contains("TF_ALIAS=typo"));
        } else {
            assert!(alias.contains("TF_ALIAS=fuck"));
            assert!(alias.contains("TF_ALIAS=typo"));
        }
        // A bracketed paste is one history event. Seed the real zsh history
        // array with that event: fc renders its newlines as backslash-n pairs.
        // The command after the correction call must never be selected.
        let event = format!("eval setup\ngit sttus\n{name} -y\nprintf future-command");
        let script = format!(
            "{alias}\nHISTSIZE=50\nfc -p\nprint -s -- 'older command'\nprint -s -- {}\n{name} -y\n",
            notypo::shlex::quote(&event),
        );
        let output = finish(
            workspace
                .program(&zsh, "git_not_command")
                .env("TF_SHELL", "zsh")
                .env("PATH", &bin)
                .env("HISTFILE", workspace.0.join("history"))
                .args(["-f", "-i", "-c", &script])
                .spawn()
                .unwrap(),
        );
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "correction executed\n",
            "{name} should execute the correction"
        );
    }
}

/// A fake `aws` whose `aws_completer` answers from a small command tree.
/// Running `aws` itself would leave a `ran` marker.
fn fake_aws(workspace: &Workspace) -> PathBuf {
    let bin = workspace.0.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let script = |name: &str, body: &str| {
        let path = bin.join(name);
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    script("aws", "#!/bin/sh\ntouch \"$(dirname \"$0\")/ran\"\n");
    script(
        "aws_completer",
        "#!/bin/sh\n# awscli test double\nline=${COMP_LINE#aws }\nprefix=${line##* }\ncontext=${line% *}\n[ \"$context\" = \"$line\" ] && context=\nwhile IFS='|' read -r pattern word; do\n  case $context in\n    $pattern) case $word in \"$prefix\"*) printf '%s\\n' \"$word\";; esac;;\n  esac\ndone < \"$(dirname \"$0\")/tree\"\n",
    );
    fs::write(
        bin.join("tree"),
        "|ec2\n|s3\nec2|describe-instances\nec2|run-instances\nec2|terminate-instances\nec2 *-instances*|--region\nec2 *-instances*|--dry-run\n",
    )
    .unwrap();
    bin
}

/// `bin` first, then the system directories the fake scripts rely on.
fn system_path(bin: &std::path::Path) -> String {
    format!("{}:/usr/bin:/bin", bin.display())
}

/// Alias fixtures seed in-memory history through the fish 4 `append` API.
/// Completion/quoting fixtures run on older installed fish versions too.
fn fish_with_history_append() -> Option<PathBuf> {
    let path = notypo::utils::which("fish")?;
    let output = Command::new(&path).arg("--version").output().ok()?;
    let version = String::from_utf8(output.stdout).ok()?;
    let major: u32 = version
        .split_whitespace()
        .last()?
        .split('.')
        .next()?
        .parse()
        .ok()?;
    (major >= 4).then_some(path)
}

#[test]
fn native_engine_never_replays_the_failed_command() {
    let workspace = Workspace::new();
    for replay in ["false", "true"] {
        let output = finish(
            workspace
                .command("")
                .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", replay)
                .args(["-y", "--force-command", "printf rerun > marker"])
                .spawn()
                .unwrap(),
        );
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            !workspace.0.join("marker").exists(),
            "replay={replay}: context capture must not run the command"
        );
    }
}

#[test]
fn native_engine_repairs_through_the_apps_completer() {
    let workspace = Workspace::new();
    let bin = fake_aws(&workspace);
    let output = finish(
        workspace
            .command("")
            .env("PATH", system_path(&bin))
            .env(
                "TF_HISTORY",
                "aws ec2 describ-instances --regoin eu-west-1 | cat\nfuck",
            )
            .env("NOTYPO_EXIT_STATUS", "252")
            .arg("-y")
            .spawn()
            .unwrap(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "aws ec2 describe-instances --region eu-west-1 | cat"
    );
    assert!(!bin.join("ran").exists(), "discovery must not run aws");
}

#[test]
fn native_engine_needs_a_person_for_risky_corrections() {
    let workspace = Workspace::new();
    let bin = fake_aws(&workspace);
    let output = finish(
        workspace
            .command("")
            .env("PATH", system_path(&bin))
            .args(["-y", "--force-command", "aws ec2 terminat-instances"])
            .spawn()
            .unwrap(),
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "nothing for the alias to run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Not running"), "{stderr}");
    assert!(stderr.contains("destructive"), "{stderr}");
}

#[test]
fn pip_completion_uses_package_identity_and_never_installs_or_replays_packages() {
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let interpreter = bin.join("python3");
    fs::write(
        &interpreter,
        r#"#!/bin/sh
root=$(dirname "$0")
[ "$1" = -m ] && [ "$2" = pip ] || exit 9
shift 2
[ "$#" = 0 ] && [ "$PIP_AUTO_COMPLETE" = 1 ] || { touch "$root/operation-marker"; exit 9; }
printf '%s|%s\n' "$COMP_CWORD" "$COMP_WORDS" >> "$root/queries"
[ "$PIP_CONFIG_FILE" = /dev/null ] && [ "$PIP_NO_INDEX" = 1 ] || exit 9
case "$COMP_WORDS" in
  *' config '*) printf 'get\nlist\n';;
  *' install -') printf '%s\n' '--dry-run' '--target=';;
  *' show '*) printf 'example-package\n';;
  *' install '*|*' list '*) ;;
  *) printf 'config install list show\n';;
esac
exit 1
"#,
    )
    .unwrap();
    fs::set_permissions(&interpreter, fs::Permissions::from_mode(0o755)).unwrap();
    for name in ["pip3.14", "package-tool"] {
        let executable = bin.join(name);
        fs::write(
            &executable,
            format!("#!/bin/sh\nexec {} -m pip \"$@\"\n", interpreter.display()),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let command = |trust: bool| {
        let mut command = workspace.command("");
        command
            .env("PATH", system_path(&bin))
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                if trust { r#"["python:pip"]"# } else { "" },
            )
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy");
        command
    };
    let output = finish(
        command(false)
            .args(["--json", "--force-command", "package-tool instlal example"])
            .spawn()
            .unwrap(),
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["probes"], 0, "{report}");
    assert!(!bin.join("queries").exists());
    for (source, expected, risky) in [
        (
            "package-tool instlal example",
            "package-tool install example",
            true,
        ),
        ("pip3.14 instlal example", "pip3.14 install example", true),
        (
            "package-tool --cache-dir /tmp instlal example",
            "package-tool --cache-dir /tmp install example",
            true,
        ),
        (
            "package-tool install --dry-rnu example",
            "package-tool install --dry-run example",
            true,
        ),
        (
            "package-tool config lsit",
            "package-tool config list",
            false,
        ),
        ("package-tool lits", "package-tool list", false),
        (
            "package-tool --log command.log lits",
            "package-tool --log command.log list",
            true,
        ),
        (
            "package-tool show exampel-package",
            "package-tool show example-package",
            true,
        ),
    ] {
        let output = finish(
            command(true)
                .args(["--json", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["candidates"][0]["command"], expected,
            "{source}: {report}"
        );
        assert_eq!(
            report["candidates"][0]["edits"][0]["via"], "pip completion",
            "{report}"
        );
        assert!(
            report["notes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|note| note.as_str().unwrap().contains("python:pip")),
            "{report}"
        );
        if source.contains(" show ") {
            assert!(
                report["candidates"][0]["safety"]["reasons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|reason| reason.as_str().unwrap().contains("acts on the resource")),
                "local package names require resource approval: {report}"
            );
        }
        if risky {
            assert_eq!(
                report["candidates"][0]["safety"]["decision"], "confirm",
                "{report}"
            );
            let output = finish(
                command(true)
                    .args(["-y", "--force-command", source])
                    .spawn()
                    .unwrap(),
            );
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
        }
    }
    // Replay uses the same resolved identity even with completion disabled.
    let output = finish(
        command(false)
            .env("NOTYPO_DISABLED_SOURCES", "native:help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "true")
            .args(["-y", "--force-command", "package-tool install example"])
            .spawn()
            .unwrap(),
    );
    assert!(output.stdout.is_empty());
    assert!(!bin.join("operation-marker").exists());
    let queries = fs::read_to_string(bin.join("queries")).unwrap();
    assert!(
        queries.contains("pip3.14 ") && queries.contains("package-tool "),
        "{queries}"
    );
}

#[test]
fn npm_completion_preserves_package_identity_and_requires_approval_for_scripts_and_publication() {
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    let package = workspace.0.join("node_modules/npm");
    fs::create_dir(&bin).unwrap();
    fs::create_dir_all(package.join("bin")).unwrap();
    fs::write(
        package.join("package.json"),
        r#"{"name":"npm","version":"11.19.1","bin":{"npm":"bin/npm-cli.js","npx":"bin/npx-cli.js"}}"#,
    ).unwrap();
    let executable = package.join("bin/npm-cli.js");
    fs::write(
        &executable,
        r#"#!/bin/sh
root=$(dirname "$0")
[ "$1" = completion ] || { touch "$root/operation-marker"; exit 9; }
shift
while [ "${1#--prefix=}" != "$1" ]; do shift; done
[ "$1" = -- ] || { touch "$root/operation-marker"; exit 9; }
shift
[ "$npm_config_offline" = true ] && [ "$npm_config_ignore_scripts" = true ] || exit 9
[ "$npm_config_logs_max" = 0 ] && [ "$npm_config_timing" = false ] || exit 9
[ -z "$NPM_CONFIG_OFFLINE" ] && [ -z "$Npm_Config_OffLine" ] && [ -z "$COMP_FISH" ] || exit 9
for arg in "$@"; do printf '<%s>' "$arg" >> "$root/queries"; done
printf '\n' >> "$root/queries"
for arg in "$@"; do
  case "$arg" in
    "'config'") printf 'get\nlist\nset\n'; exit;;
    "'run'") printf 'build\nreport package\n'; exit;;
  esac
done
printf 'config\ninstall\npublish\nrun\nstart\nview\n'
"#,
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(&executable, bin.join("package-tool")).unwrap();
    let npx = package.join("bin/npx-cli.js");
    fs::write(&npx, "#!/bin/sh\ntouch operation-marker\n").unwrap();
    fs::set_permissions(&npx, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(&npx, bin.join("package-runner")).unwrap();
    let command = |trust: bool| {
        let mut command = workspace.command("");
        command
            .env("PATH", system_path(&bin))
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                if trust { r#"["npm:npm"]"# } else { "" },
            )
            .env("NOTYPO_TRUSTED_HELP", "")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NPM_CONFIG_OFFLINE", "false")
            .env("Npm_Config_OffLine", "false")
            .env("COMP_FISH", "true");
        command
    };
    let output = finish(
        command(false)
            .args(["--json", "--force-command", "package-tool pbulish"])
            .spawn()
            .unwrap(),
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["probes"], 0, "{report}");
    assert!(!bin.join("queries").exists());
    for (source, expected, resource, risky) in [
        (
            "package-tool config lsit",
            "package-tool config list",
            false,
            false,
        ),
        ("package-tool pbulish", "package-tool publish", false, true),
        (
            "package-tool isntall example",
            "package-tool install example",
            false,
            true,
        ),
        (
            "package-tool run biuld",
            "package-tool run build",
            true,
            true,
        ),
        (
            "package-tool --prefix 'a b😀' run 'report pakcage'",
            "package-tool --prefix 'a b😀' run 'report package'",
            true,
            true,
        ),
        (
            "package-tool run biuld -- '$(touch operation-marker)'",
            "package-tool run build -- '$(touch operation-marker)'",
            true,
            true,
        ),
        ("package-tool strta", "package-tool start", false, true),
        (
            "package-tool config ste key value",
            "package-tool config set key value",
            false,
            true,
        ),
    ] {
        let output = finish(
            command(true)
                .args(["--json", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["candidates"][0]["command"], expected,
            "{source}: {report}"
        );
        assert!(
            report["candidates"][0]["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "npm completion"),
            "{report}"
        );
        if resource {
            assert!(
                report["candidates"][0]["safety"]["reasons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|reason| reason.as_str().unwrap().contains("acts on the resource")),
                "{report}"
            );
        }
        if risky {
            assert_eq!(
                report["candidates"][0]["safety"]["decision"], "confirm",
                "{report}"
            );
            let output = finish(
                command(true)
                    .args(["-y", "--force-command", source])
                    .spawn()
                    .unwrap(),
            );
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
        }
    }
    let output = finish(
        command(true)
            .args([
                "--json",
                "--force-command",
                "package-tool --offline=false run biuld",
            ])
            .spawn()
            .unwrap(),
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["candidates"].as_array().unwrap().is_empty(),
        "{report}"
    );
    assert!(
        report["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|note| note.as_str().unwrap().contains("offline input policy")),
        "{report}"
    );
    let output = finish(
        command(true)
            .args(["--json", "--force-command", "package-runner pbulish"])
            .spawn()
            .unwrap(),
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["probes"], 0,
        "npx must not be used as npm completion: {report}"
    );
    let output = finish(
        command(false)
            .env("NOTYPO_DISABLED_SOURCES", "native:help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "true")
            .args(["-y", "--force-command", "package-tool run build"])
            .spawn()
            .unwrap(),
    );
    assert!(output.stdout.is_empty());
    assert!(!bin.join("operation-marker").exists());
    assert!(!workspace.0.join("operation-marker").exists());
    let queries = fs::read_to_string(bin.join("queries")).unwrap();
    assert!(
        queries.contains("<'a b😀'>") && queries.contains("<'package-tool'>"),
        "{queries}"
    );
}

#[test]
fn cargo_completion_reads_offline_metadata_and_blocks_build_alias_and_replay_execution() {
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::create_dir_all(workspace.0.join("lib/rustlib")).unwrap();
    fs::write(
        workspace.0.join("lib/rustlib/manifest-cargo-fixture"),
        "file:bin/cargo\n",
    )
    .unwrap();
    let cargo = bin.join("cargo");
    fs::write(&cargo, r#"#!/bin/sh
root=$(dirname "$(dirname "$0")")
[ "$CARGO_NET_OFFLINE" = true ] && [ "$RUSTUP_AUTO_INSTALL" = 0 ] || exit 8
[ "$CARGO_HTTP_PROXY" = http://127.0.0.1:9 ] && [ -z "$CARGO_COMPLETE" ] || exit 8
for arg in "$@"; do printf '<%s>' "$arg" >> "$root/queries"; done
printf '\n' >> "$root/queries"
while [ "$#" -gt 0 ]; do
  case "$1" in --offline|--color=never|--config=*) shift;; *) break;; esac
done
case "$*" in
  '--list --verbose') printf 'Installed Commands:\n    build  Compile project\n    metadata  Read metadata\n    report  Report\n    version  Version\n    danger  alias: !touch operation-marker\n';;
  --help) printf 'Options:\n  --color <WHEN>  Color [possible values: auto, always, never]\n';;
  'build --help') printf 'Options:\n  --release  Release\n  --color <WHEN>  Color [possible values: auto, always, never]\n  -p, --package [<SPEC>]  Package\n  --bin [<NAME>]  Binary\n  --manifest-path <PATH>  Manifest\n  -F, --features <FEATURES>  Features\n';;
  'report --help') printf 'Commands:\n  future-incompatibilities  Show future reports\n';;
  'metadata --offline --locked --no-deps --format-version=1'*) printf '%s' '{"workspace_members":["demo-id"],"packages":[{"id":"demo-id","name":"demo","features":{"default":[],"quiet-mode":[]},"targets":[{"kind":["bin"],"name":"demo-bin"}]}]}';;
  *) touch "$root/operation-marker"; exit 9;;
esac
"#).unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(&cargo, bin.join("rust-packages")).unwrap();
    let command = |trust: bool, help: bool| {
        let mut command = workspace.command("");
        command
            .env("PATH", system_path(&bin))
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                if trust { r#"["rust:cargo"]"# } else { "" },
            )
            .env(
                "NOTYPO_TRUSTED_HELP",
                if help { r#"["rust:cargo"]"# } else { "" },
            )
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env("CARGO_COMPLETE", "bash");
        command
    };
    let output = finish(
        command(false, false)
            .args(["--json", "--force-command", "rust-packages biuld"])
            .spawn()
            .unwrap(),
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["probes"], 0, "{report}");
    assert!(!workspace.0.join("queries").exists());
    for (source, expected, resource) in [
        ("rust-packages biuld", "rust-packages build", false),
        (
            "rust-packages build --releae",
            "rust-packages build --release",
            false,
        ),
        (
            "rust-packages report future-incompatibilites",
            "rust-packages report future-incompatibilities",
            false,
        ),
        (
            "rust-packages build --color nveer",
            "rust-packages build --color never",
            false,
        ),
        (
            "rust-packages build --bin demo-bni",
            "rust-packages build --bin demo-bin",
            true,
        ),
        (
            "rust-packages build -p dmeo",
            "rust-packages build -p demo",
            true,
        ),
        (
            "rust-packages build --features quiet-mdoe",
            "rust-packages build --features quiet-mode",
            true,
        ),
        (
            "rust-packages build --features=default,quiet-mdoe",
            "rust-packages build --features=default,quiet-mode",
            true,
        ),
        (
            "rust-packages build --features demo/quiet-mdoe",
            "rust-packages build --features demo/quiet-mode",
            true,
        ),
        (
            "rust-packages build --features 'default demo/quiet-mdoe'",
            "rust-packages build --features 'default demo/quiet-mode'",
            true,
        ),
        ("rust-packages dagner", "rust-packages danger", false),
        (
            "env X=1 rust-packages biuld",
            "env X=1 rust-packages build",
            false,
        ),
        (
            "rust-packages build --manifest-path 'project 😀/Cargo.toml' --bin demo-bni",
            "rust-packages build --manifest-path 'project 😀/Cargo.toml' --bin demo-bin",
            true,
        ),
        (
            "rust-packages build --bin demo-bni -- '$(touch operation-marker)'",
            "rust-packages build --bin demo-bin -- '$(touch operation-marker)'",
            true,
        ),
    ] {
        let output = finish(
            command(true, true)
                .args(["--json", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["candidates"][0]["command"], expected,
            "{source}: {report}"
        );
        assert_eq!(
            report["candidates"][0]["safety"]["decision"], "confirm",
            "{report}"
        );
        assert!(
            report["candidates"][0]["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "cargo completion"),
            "{report}"
        );
        if resource {
            assert!(
                report["candidates"][0]["safety"]["reasons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|reason| reason.as_str().unwrap().contains("acts on the resource")),
                "{report}"
            );
        }
        let output = finish(
            command(true, true)
                .args(["-y", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        assert!(!output.status.success(), "{source}");
        assert!(output.stdout.is_empty(), "{source}");
        assert!(!workspace.0.join("operation-marker").exists());
    }
    let output = finish(
        command(true, false)
            .args(["--json", "--force-command", "rust-packages build --releae"])
            .spawn()
            .unwrap(),
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["candidates"].as_array().unwrap().is_empty(),
        "{report}"
    );
    let output = finish(
        command(false, false)
            .env("NOTYPO_DISABLED_SOURCES", "native:help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "true")
            .args(["-y", "--force-command", "rust-packages build"])
            .spawn()
            .unwrap(),
    );
    assert!(output.stdout.is_empty());
    assert!(!workspace.0.join("operation-marker").exists());
    let queries = fs::read_to_string(workspace.0.join("queries")).unwrap();
    assert!(
        queries.contains("<--manifest-path=project 😀/Cargo.toml>"),
        "{queries}"
    );
    assert!(
        !queries.contains("<$(touch operation-marker)>"),
        "{queries}"
    );
    assert!(!queries.contains("<danger><--help>"), "{queries}");
    assert!(!workspace.0.join("Cargo.lock").exists());
}

#[test]
fn clap_dynamic_completion_repairs_commands_options_and_values_without_running_operations() {
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let executable = bin.join("clapcli");
    fs::write(&executable, r#"#!/bin/sh
root=$(dirname "$0")
printf '[%s]' "$@" >> "$root/queries"
printf '\n' >> "$root/queries"
case "$*" in
  '--help') printf 'Commands:\n  completion  Print completion registration\n'; exit;;
  'completion --help') printf 'Usage: clapcli completion <SHELL>\nArguments:\n  <SHELL>  [possible values: zsh]\n'; exit;;
  'completion zsh') cat "$root/registration"; exit;;
esac
case "$CLAP_FIXTURE_MODE" in
  zsh|bash) ;;
  *) touch "$root/operation-marker"; exit 3;;
esac
[ "$1" = -- ] || exit 4
shift
[ "$1" = clapcli ] || exit 5
shift
[ "$_CLAP_COMPLETE_INDEX" = "$#" ] || exit 6
context=
while [ "$#" -gt 1 ]; do
  context="$context $1"
  shift
done
while IFS='|' read -r pattern value description; do
  case "$context" in
    $pattern) case "$value" in "$1"*)
      if [ "$CLAP_FIXTURE_MODE" = bash ]; then
        printf '%s\013' "$value"
      else
        printf '%s:%s\n' "$value" "$description"
      fi;; esac;;
  esac
done < "$root/tree"
"#).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        bin.join("registration"),
        r#"#compdef clapcli
function _clap_dynamic_completer_fixture() {
  local _CLAP_COMPLETE_INDEX=$(expr $CURRENT - 1)
  local _CLAP_IFS=$'\n'
  local replies=$( \
    _CLAP_IFS="$_CLAP_IFS" \
    _CLAP_COMPLETE_INDEX="$_CLAP_COMPLETE_INDEX" \
    CLAP_FIXTURE_MODE="zsh" \
    clapcli -- "${words[@]}" 2>/dev/null \
  )
}
compdef _clap_dynamic_completer_fixture clapcli
touch registration-marker
"#,
    )
    .unwrap();
    fs::write(bin.join("tree"), "|nodes|Manage nodes\n|plugin|Manage plugins\n nodes|list|List nodes\n nodes list*|--format|Output format\n nodes list*|--target|A target\n nodes list --format|json|JSON\n nodes list --format|yaml|YAML\n plugin|install|Install a plugin\n").unwrap();
    let command = |help: bool| {
        let mut command = workspace.command("");
        command
            .env("PATH", system_path(&bin))
            .env("NOTYPO_TRUSTED_COMPLETERS", "clapcli")
            .env("NOTYPO_TRUSTED_HELP", if help { "clapcli" } else { "" })
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy");
        command
    };
    let output = finish(
        command(false)
            .args(["--json", "--force-command", "clapcli nodse"])
            .spawn()
            .unwrap(),
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["probes"], 0,
        "help is a separate trust permission: {report}"
    );
    assert!(!bin.join("queries").exists());
    for (source, expected) in [
        ("clapcli nodse lsit", "clapcli nodes list"),
        (
            "clapcli nodes list --formta=jsno",
            "clapcli nodes list --format=json",
        ),
        (
            "clapcli nodes lsit --target '$(touch operation-marker)' --formta=json",
            "clapcli nodes list --target '$(touch operation-marker)' --format=json",
        ),
        (
            "clapcli plugin isntall example",
            "clapcli plugin install example",
        ),
    ] {
        let output = finish(
            command(true)
                .args(["--json", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        assert!(
            output.status.success(),
            "{source}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["candidates"][0]["command"], expected,
            "{source}: {report}"
        );
        assert!(
            report["candidates"][0]["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "clap completion"),
            "{report}"
        );
        assert!(
            report["candidates"][0]["edits"][0]["description"].is_string(),
            "{report}"
        );
        if source.contains("plugin") {
            assert_eq!(
                report["candidates"][0]["safety"]["decision"], "confirm",
                "{report}"
            );
            let output = finish(
                command(true)
                    .args(["-y", "--force-command", source])
                    .spawn()
                    .unwrap(),
            );
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert!(String::from_utf8_lossy(&output.stderr).contains("changes installed packages"));
        }
    }
    let queries = fs::read_to_string(bin.join("queries")).unwrap();
    assert!(queries.contains("[$(touch operation-marker)]"), "{queries}");
    // An installed registration supplies the proof without --help or sourcing.
    let completion_dir = bin.join("completions");
    fs::create_dir(&completion_dir).unwrap();
    fs::write(
        completion_dir.join("clapcli"),
        r#"_clap_complete_fixture() {
  local IFS=$'\013'
  local _CLAP_COMPLETE_INDEX=${COMP_CWORD}
  local replies=$( \
    _CLAP_IFS="$IFS" \
    _CLAP_COMPLETE_INDEX="$_CLAP_COMPLETE_INDEX" \
    _CLAP_COMPLETE_COMP_TYPE="$_CLAP_COMPLETE_COMP_TYPE" \
    _CLAP_COMPLETE_SPACE="$_CLAP_COMPLETE_SPACE" \
    CLAP_FIXTURE_MODE="bash" \
    clapcli -- "${words[@]}" \
  )
}
complete -F _clap_complete_fixture clapcli
touch registration-marker
"#,
    )
    .unwrap();
    fs::write(bin.join("queries"), "").unwrap();
    let output = finish(
        command(false)
            .env("BASH_COMPLETION_USER_DIR", &bin)
            .args(["--json", "--force-command", "clapcli nodse"])
            .spawn()
            .unwrap(),
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["candidates"][0]["command"], "clapcli nodes",
        "{report}"
    );
    assert_eq!(
        report["candidates"][0]["edits"][0]["via"],
        "clap completion"
    );
    assert_eq!(report["probes"], 1, "{report}");
    let queries = fs::read_to_string(bin.join("queries")).unwrap();
    assert_eq!(queries, "[--][clapcli][]\n");
    for marker in [
        bin.join("operation-marker"),
        workspace.0.join("operation-marker"),
        workspace.0.join("registration-marker"),
    ] {
        assert!(!marker.exists(), "unexpected {}", marker.display());
    }
}

#[test]
fn developer_operations_require_approval_and_y_emits_nothing_to_execute() {
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    for (typo, program, arguments, reason) in [
        (
            "diskutl",
            "diskutil",
            "eraseDisk APFS Example /dev/example",
            "wipe disks",
        ),
        ("userdl", "userdel", "example", "removes users"),
        ("shutdwn", "shutdown", "-h now", "stops things"),
        ("iptablse", "iptables", "-tfilter -F", "firewall rules"),
        ("nginxx", "nginx", "-s quit", "restart a service"),
        (
            "certbto",
            "certbot",
            "delete --cert-name example",
            "revokes certificates",
        ),
        ("flyawy", "flyway", "clean", "database data"),
        ("liquibsae", "liquibase", "dropAll", "database data"),
        ("primsa", "prisma", "migrate reset", "database data"),
        ("alembci", "alembic", "downgrade base", "database data"),
        ("dbmtae", "dbmate", "drop", "database data"),
        (
            "retsic",
            "restic",
            "-r example forget snapshot",
            "backup snapshots",
        ),
        ("brog", "borg", "prune example", "backup snapshots"),
        (
            "borgmtaic",
            "borgmatic",
            "--config example.yaml",
            "backup snapshots",
        ),
        ("tarsnpa", "tarsnap", "-dfexample", "backup snapshots"),
        (
            "rsnaphsot",
            "rsnapshot",
            "custom-interval",
            "backup snapshots",
        ),
        (
            "telepresnce",
            "telepresence",
            "intercept example",
            "intercepted traffic",
        ),
        ("wg-quik", "wg-quick", "down example", "tunnels"),
        (
            "opnevpn",
            "openvpn",
            "--config example.ovpn",
            "another host",
        ),
    ] {
        let executable = bin.join(program);
        fs::write(&executable, "#!/bin/sh\nprintf ran > operation-marker\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let source = format!("{typo} {arguments}");
        let expected = format!("{program} {arguments}");
        let command = || {
            let mut command = workspace.command("");
            command
                .env("PATH", system_path(&bin))
                .env("NOTYPO_DISABLED_SOURCES", "native:help:man:history:legacy")
                .env("TF_HISTORY", &source)
                .env("NOTYPO_EXIT_STATUS", "127");
            command
        };
        let output = finish(command().arg("--json").spawn().unwrap());
        assert!(
            output.status.success(),
            "{source}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["outcome"]["kind"], "suggestion",
            "{source}: {report}"
        );
        assert_eq!(
            report["candidates"][0]["command"], expected,
            "{source}: {report}"
        );
        let candidate = report["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["command"] == expected)
            .unwrap_or_else(|| panic!("{source}: {report}"));
        assert_eq!(
            candidate["safety"]["decision"], "confirm",
            "{source}: {report}"
        );
        assert!(
            candidate["safety"]["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry.as_str().unwrap().contains(reason)),
            "{source}: {report}"
        );
        let output = finish(command().arg("-y").spawn().unwrap());
        assert!(
            !output.status.success(),
            "{source}: approval needs a terminal"
        );
        assert!(
            output.stdout.is_empty(),
            "{source}: nothing for the alias to execute"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Not running") && stderr.contains(reason),
            "{source}: {stderr}"
        );
        assert!(
            !workspace.0.join("operation-marker").exists(),
            "discovery ran {program}"
        );
    }
    // The same executable correction, evidence, and providers accept a
    // read-only command. The risky cases above are refused by the gate.
    let output = finish(
        workspace
            .command("")
            .env("PATH", system_path(&bin))
            .env("NOTYPO_DISABLED_SOURCES", "native:help:man:history:legacy")
            .env("TF_HISTORY", "nginxx -t")
            .env("NOTYPO_EXIT_STATUS", "127")
            .arg("-y")
            .spawn()
            .unwrap(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"nginx -t");
    assert!(!workspace.0.join("operation-marker").exists());
}

#[test]
fn explain_reports_the_diagnosis_and_runs_nothing() {
    let workspace = Workspace::new();
    let bin = fake_aws(&workspace);
    let output = finish(
        workspace
            .command("")
            .env("PATH", system_path(&bin))
            .args(["--explain", "--force-command", "aws ec2 describ-instances"])
            .spawn()
            .unwrap(),
    );
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("suggestion: aws ec2 describe-instances"),
        "{stderr}"
    );
    assert!(stderr.contains("evidence (native)"), "{stderr}");
    assert!(stderr.contains("not captured"), "{stderr}");
    assert!(!bin.join("ran").exists());
}

#[test]
fn json_reports_the_diagnosis_for_automation() {
    let workspace = Workspace::new();
    let bin = fake_aws(&workspace);
    let output = finish(
        workspace
            .command("")
            .env("PATH", system_path(&bin))
            .args([
                "--json",
                "--force-command",
                "aws ec2 describ-instances | cat",
            ])
            .spawn()
            .unwrap(),
    );
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["outcome"]["kind"], "suggestion");
    let candidate = &report["candidates"][0];
    assert_eq!(candidate["command"], "aws ec2 describe-instances | cat");
    assert_eq!(candidate["safety"]["decision"], "allow");
    assert_eq!(candidate["edits"][0]["from"], "describ-instances");
    assert_eq!(candidate["edits"][0]["start"], 8);
    assert_eq!(report["output"], serde_json::Value::Null);
    assert!(!bin.join("ran").exists());
}

#[test]
fn nested_help_is_print_only_and_uncertain_corrections_require_confirmation() {
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let tool = bin.join("helpcli");
    fs::write(&tool, r#"#!/bin/sh
root=$(dirname "$0")
printf '[%s]' "$@" >> "$root/queries"
printf '\n' >> "$root/queries"
case "$*" in
  '--help') printf 'Commands:\n  nodes    Manage nodes\n';;
  'nodes --help') printf 'Commands:\n  list     List nodes\n';;
  'nodes list --help') printf 'Options:\n  --format <FORMAT>  [possible values: json, yaml]\n  --target <TARGET>  Target\n';;
  *) touch "$root/ran"; exit 1;;
esac
"#).unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let source = "helpcli nodes list --format=jsno --target 'private target'";
    let command = || {
        let mut command = workspace.command("");
        command
            .env("PATH", system_path(&bin))
            .env("NOTYPO_TRUSTED_HELP", "helpcli")
            .env("NOTYPO_DISABLED_SOURCES", "native:man:history:legacy");
        command
    };
    let output = finish(
        command()
            .args(["--json", "--force-command", source])
            .spawn()
            .unwrap(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["outcome"]["kind"], "ambiguous");
    assert_eq!(
        report["candidates"][0]["command"],
        source.replace("jsno", "json")
    );
    assert_eq!(report["candidates"][0]["edits"][0]["role"], "OptionValue");
    assert_eq!(report["probes"], 3);
    let output = finish(
        command()
            .args(["-y", "--force-command", source])
            .spawn()
            .unwrap(),
    );
    assert!(!output.status.success(), "help remains partial evidence");
    assert!(
        output.stdout.is_empty(),
        "the alias must receive no command"
    );
    assert!(!bin.join("ran").exists());
    assert_eq!(
        fs::read_to_string(bin.join("queries")).unwrap(),
        "[--help]\n[nodes][--help]\n[nodes][list][--help]\n[--help]\n[nodes][--help]\n[nodes][list][--help]\n"
    );
    assert!(!workspace.0.join("history").exists());
}

/// The alias passes the shell's function names: a function is a valid
/// command, and a misspelled one is corrected to it.
#[test]
fn shell_aliases_pass_function_names() {
    for shell in ["bash", "zsh", "fish"] {
        let Some(path) = (if shell == "fish" {
            fish_with_history_append()
        } else {
            notypo::utils::which(shell)
        }) else {
            continue;
        };
        let workspace = Workspace::new();
        let output = finish(
            workspace
                .command("")
                .env("TF_SHELL", shell)
                .arg("--alias")
                .spawn()
                .unwrap(),
        );
        let alias = String::from_utf8(output.stdout).unwrap();
        let (flags, seed, function): (&[&str], _, _) = match shell {
            "bash" => (
                &["--norc", "--noprofile", "-i", "-c"],
                "set -o history\nhistory -s 'deploy_site now'",
                "deploy_app() { echo \"deployed $1\"; }",
            ),
            "zsh" => (
                &["-f", "-i", "-c"],
                "HISTSIZE=50\nfc -p\nprint -s -- 'deploy_site now'",
                "deploy_app() { echo \"deployed $1\"; }",
            ),
            "fish" => (
                &["--no-config", "--private", "-c"],
                "builtin history append -- 'deploy_site now'",
                "function deploy_app; printf 'deployed %s\\n' $argv[1]; end",
            ),
            _ => unreachable!(),
        };
        let script = format!("{alias}\n{function}\n{seed}\ndeploy_site now\nfuck -y\n");
        let output = finish(
            workspace
                .program(&path, "")
                .env("TF_SHELL", shell)
                .env("PATH", workspace.0.join("bin"))
                .env("HISTFILE", workspace.0.join("history"))
                .args(flags)
                .arg(&script)
                .spawn()
                .unwrap(),
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "deployed now\n",
            "{shell}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// A completion function defined in the session (`eval "$(tool
/// completion bash)"`) has no file; the bash function passes its
/// definitions, which notypo evaluates without running the session's code.
#[test]
fn bash_passes_session_completion_functions_without_running_its_startup() {
    let Some(bash) = notypo::utils::which("bash") else {
        return;
    };
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let tool = bin.join("tool");
    fs::write(&tool, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let output = finish(
        workspace
            .command("")
            .env("TF_SHELL", "bash")
            .arg("--alias")
            .spawn()
            .unwrap(),
    );
    let alias = String::from_utf8(output.stdout).unwrap();
    let marker = workspace.0.join("startup-marker");
    let generated = "_tool() { case \"${COMP_WORDS[*]}\" in \"tool \") COMPREPLY=(build deploy);; esac; }\ncomplete -F _tool tool";
    let script = format!(
        "{alias}\neval \"$(printf '%s\\n' '{generated}')\"\n_unrelated() {{ touch '{}'; }}\nset -o history\nhistory -s 'tool biuld'\nfuck --explain\n",
        marker.display()
    );
    let output = finish(
        workspace
            .program(&bash, "")
            .env("TF_SHELL", "bash")
            .env("PATH", system_path(&bin))
            .env("HISTFILE", workspace.0.join("history"))
            .env("NOTYPO_TRUSTED_COMPLETERS", "tool")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .args(["--norc", "--noprofile", "-i", "-c"])
            .arg(&script)
            .spawn()
            .unwrap(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("your bash session's completion function"),
        "{stderr}"
    );
    assert!(stderr.contains("1. tool build"), "{stderr}");
    assert!(!marker.exists(), "only definitions were evaluated");
}

/// zsh's version: a handler from `source <(tool completion zsh)` is passed
/// with the other functions from that source, and runs in a real widget.
#[test]
fn zsh_passes_session_completion_functions_without_running_its_startup() {
    let Some(zsh) = notypo::utils::which("zsh") else {
        return;
    };
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let tool = bin.join("tool");
    fs::write(&tool, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let output = finish(
        workspace
            .command("")
            .env("TF_SHELL", "zsh")
            .arg("--alias")
            .spawn()
            .unwrap(),
    );
    let alias = String::from_utf8(output.stdout).unwrap();
    let marker = workspace.0.join("startup-marker");
    let generated = format!(
        "_tool() {{ _tool_commands; }}; _tool_commands() {{ _arguments '1:command:(build deploy)'; }}; _tool_unrelated() {{ touch '{}'; }}; compdef _tool tool",
        marker.display()
    );
    let script = format!(
        "autoload -Uz compinit\ncompinit -u -D\n{alias}\nsource <(print -r -- \"{generated}\")\nHISTSIZE=50\nfc -p\nprint -s -- 'tool biuld'\nfuck --explain\n"
    );
    let output = finish(
        workspace
            .program(&zsh, "")
            .env("TF_SHELL", "zsh")
            .env("PATH", system_path(&bin))
            .env("HISTFILE", workspace.0.join("history"))
            .env("NOTYPO_TRUSTED_COMPLETERS", "tool")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .args(["-f", "-i", "-c"])
            .arg(&script)
            .spawn()
            .unwrap(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("your zsh session's _tool"), "{stderr}");
    assert!(stderr.contains("1. tool build"), "{stderr}");
    assert!(!marker.exists(), "only definitions were evaluated");
}

/// fish's version: completions registered in the session, with the user
/// function their condition calls, are passed and loaded without config.
#[test]
fn fish_passes_session_completions_without_running_its_config() {
    let Some(fish) = fish_with_history_append() else {
        return;
    };
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    // Probes start the fish found on PATH.
    std::os::unix::fs::symlink(&fish, bin.join("fish")).unwrap();
    let tool = bin.join("tool");
    fs::write(&tool, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let output = finish(
        workspace
            .command("")
            .env("TF_SHELL", "fish")
            .arg("--alias")
            .spawn()
            .unwrap(),
    );
    let alias = String::from_utf8(output.stdout).unwrap();
    let marker = workspace.0.join("startup-marker");
    let script = format!(
        "{alias}\nfunction __tool_needs; __tool_inner; end\nfunction __tool_inner; return 0; end\nfunction __tool_unrelated; touch '{}'; end\ncomplete -c tool -f -n __tool_needs -a 'build deploy'\nbuiltin history append -- 'tool biuld'\nfuck --explain\n",
        marker.display()
    );
    let output = finish(
        workspace
            .program(&fish, "")
            .env("TF_SHELL", "fish")
            .env("PATH", system_path(&bin))
            .env("NOTYPO_TRUSTED_COMPLETERS", "tool")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .args(["--no-config", "--private", "-c"])
            .arg(&script)
            .spawn()
            .unwrap(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("your fish session's completions"),
        "{stderr}"
    );
    assert!(stderr.contains("1. tool build"), "{stderr}");
    assert!(!marker.exists(), "only definitions were loaded");
}

/// fish aliases are functions. The fish function passes the aliases among
/// the failed line's words, and those their values lead to, so they select
/// their target's vocabulary and safety policy as bash and zsh aliases do.
#[test]
fn fish_aliases_use_their_target_programs_vocabulary_and_safety_policy() {
    let Some(fish) = fish_with_history_append() else {
        return;
    };
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let help = "Usage: eza [options] [files...]\n\nOPTIONS\n  --color WHEN    when to use terminal colours\n  --icons         display icons\n";
    for (name, body) in [
        (
            "eza",
            format!(
                "printf 'eza:%s\\n' \"$*\" >> \"$(dirname \"$0\")/calls\"\n[ \"$*\" = --help ] && {{ printf '%s' '{help}'; exit 0; }}\ntouch \"$(dirname \"$0\")/operation-marker\"\n"
            ),
        ),
        (
            "ls",
            "printf 'ls:%s\\n' \"$*\" >> \"$(dirname \"$0\")/calls\"\n".to_owned(),
        ),
        (
            "rm",
            "touch \"$(dirname \"$0\")/operation-marker\"\n".to_owned(),
        ),
    ] {
        let path = bin.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::create_dir(workspace.0.join("build")).unwrap();
    let output = finish(
        workspace
            .command("")
            .env("TF_SHELL", "fish")
            .arg("--alias")
            .spawn()
            .unwrap(),
    );
    let alias = String::from_utf8(output.stdout).unwrap();
    // `ll` leads to `l`, which runs eza; `ls` itself is never asked.
    let script = format!(
        "{alias}\nalias l 'eza --icons'\nalias ll l\nalias rmf 'rm -rf'\n\
         builtin history append -- 'll --colro=auto'\nfuck --explain\n\
         builtin history append -- 'rmf ./buidl'\nfalse; fuck --explain\nfalse; fuck -y\n"
    );
    let output = finish(
        workspace
            .program(&fish, "")
            .env("TF_SHELL", "fish")
            .env("PATH", system_path(&bin))
            .env("NOTYPO_TRUSTED_COMPLETERS", "")
            .env("NOTYPO_TRUSTED_HELP", "eza:ls")
            .env("NOTYPO_DISABLED_SOURCES", "man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .args(["--no-config", "--private", "-c"])
            .arg(&script)
            .spawn()
            .unwrap(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("1. ll --color=auto"), "{stderr}");
    // `rmf` runs `rm -rf`: a changed target needs approval; -y runs nothing.
    let (_, rmf) = stderr.split_once("command:     rmf ./buidl").unwrap();
    assert!(rmf.contains("rmf ./build"), "{stderr}");
    assert!(rmf.contains("(through an alias)"), "{stderr}");
    let calls = fs::read_to_string(bin.join("calls")).unwrap_or_default();
    assert!(calls.contains("eza:--help"), "{calls}\n{stderr}");
    assert!(!calls.contains("ls:"), "{calls}");
    assert!(!bin.join("operation-marker").exists(), "{stderr}");
    assert!(workspace.0.join("build").exists());
}

/// A shell function that only runs one command with all its arguments
/// (`l() { eza --icons "$@"; }`) stands for that command, as an alias does:
/// each shell's function passes the definitions of the functions in the
/// failed line (two levels deep), and nothing in them runs.
#[test]
fn wrapper_functions_use_their_commands_vocabulary_and_safety_policy() {
    for shell in ["bash", "zsh", "fish"] {
        let Some(path) = (if shell == "fish" {
            fish_with_history_append()
        } else {
            notypo::utils::which(shell)
        }) else {
            continue;
        };
        let workspace = Workspace::new();
        let bin = workspace.0.join("bin");
        fs::create_dir(&bin).unwrap();
        let help = "Usage: eza [options] [files...]\n\nOPTIONS\n  --color WHEN    when to use terminal colours\n  --icons         display icons\n";
        for (name, body) in [
            (
                "eza",
                format!(
                    "printf 'eza:%s\\n' \"$*\" >> \"$(dirname \"$0\")/calls\"\n[ \"$*\" = --help ] && {{ printf '%s' '{help}'; exit 0; }}\ntouch \"$(dirname \"$0\")/operation-marker\"\n"
                ),
            ),
            (
                "rm",
                "touch \"$(dirname \"$0\")/operation-marker\"\n".to_owned(),
            ),
        ] {
            let path = bin.join(name);
            fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::create_dir(workspace.0.join("build")).unwrap();
        let marker = workspace.0.join("function-marker");
        let output = finish(
            workspace
                .command("")
                .env("TF_SHELL", shell)
                .arg("--alias")
                .spawn()
                .unwrap(),
        );
        let alias = String::from_utf8(output.stdout).unwrap();
        let (flags, definitions, seed): (&[&str], _, _) = match shell {
            "bash" => (
                &["--norc", "--noprofile", "-i", "-c"],
                format!(
                    "l() {{ eza --icons \"$@\"; }}\nll() {{ l \"$@\"; }}\nrmf() {{ rm -rf \"$@\"; }}\nboom() {{ touch '{}'; }}",
                    marker.display()
                ),
                "set -o history\nhistory -s",
            ),
            "zsh" => (
                &["-f", "-i", "-c"],
                format!(
                    "l() {{ eza --icons \"$@\" }}\nll() {{ l $@ }}\nrmf() {{ rm -rf \"$@\" }}\nboom() {{ touch '{}' }}",
                    marker.display()
                ),
                "HISTSIZE=50\nfc -p\nprint -s --",
            ),
            _ => (
                &["--no-config", "--private", "-c"],
                format!(
                    "function l; eza --icons $argv; end\nfunction ll; l $argv; end\nfunction rmf; rm -rf $argv; end\nfunction boom; touch '{}'; end",
                    marker.display()
                ),
                "builtin history append --",
            ),
        };
        // `boom` is named in the line, so its definition is passed too.
        let script = format!(
            "{alias}\n{definitions}\n\
             {seed} 'll --colro=auto boom'\nfalse; fuck --explain\n\
             {seed} 'rmf ./buidl'\nfalse; fuck --explain\nfalse; fuck -y\n"
        );
        let output = finish(
            workspace
                .program(&path, "")
                .env("TF_SHELL", shell)
                .env("PATH", system_path(&bin))
                .env("HISTFILE", workspace.0.join("history"))
                .env("NOTYPO_TRUSTED_COMPLETERS", "")
                .env("NOTYPO_TRUSTED_HELP", "eza")
                .env("NOTYPO_DISABLED_SOURCES", "man:history:legacy")
                .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
                .args(flags)
                .arg(&script)
                .spawn()
                .unwrap(),
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("1. ll --color=auto boom"),
            "{shell}: {stderr}"
        );
        assert!(
            stderr.contains("`ll` is an alias for `eza --icons`"),
            "{shell}: {stderr}"
        );
        let (_, rmf) = stderr
            .split_once("command:     rmf ./buidl")
            .unwrap_or_else(|| panic!("{shell}: {stderr}"));
        assert!(rmf.contains("rmf ./build"), "{shell}: {stderr}");
        assert!(rmf.contains("(through an alias)"), "{shell}: {stderr}");
        let calls = fs::read_to_string(bin.join("calls")).unwrap_or_default();
        assert_eq!(calls, "eza:--help\n", "{shell}");
        assert!(!bin.join("operation-marker").exists(), "{shell}: {stderr}");
        assert!(!marker.exists(), "{shell}: definitions are never run");
        assert!(workspace.0.join("build").exists());
    }
}

/// PowerShell's alias with arguments is a function that runs one command
/// with `@args`; such a wrapper takes its command's vocabulary, and the
/// safety gate judges what it runs.
#[test]
fn powershell_wrapper_functions_use_their_commands_vocabulary_and_safety_policy() {
    let Some(pwsh) = std::env::var_os("NOTYPO_TEST_PWSH")
        .map(PathBuf::from)
        .or_else(|| notypo::utils::which("pwsh"))
    else {
        eprintln!("skipped: PowerShell is not installed");
        return;
    };
    let Some(git) = notypo::utils::which("git") else {
        eprintln!("skipped: git is not installed");
        return;
    };
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::create_dir(workspace.0.join("build")).unwrap();
    // notypo's metadata queries reach git; other runs are recorded.
    for (name, body) in [
        (
            "git",
            format!(
                "case \"$*\" in --list-cmds=*|-h|'-C '*|'init -q --template= '*|'status -h') exec {} \"$@\";; esac\nprintf 'git %s\\n' \"$*\" >> \"$(dirname \"$0\")/calls\"\nexit 1\n",
                git.display()
            ),
        ),
        (
            "rm",
            "printf 'rm %s\\n' \"$*\" >> \"$(dirname \"$0\")/calls\"\nexit 1\n".to_owned(),
        ),
    ] {
        let path = bin.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let input = format!(
        "$env:TF_SHELL = 'powershell'\n\
         Invoke-Expression ((& '{}' --alias) -join \"`n\")\n\
         function gst {{ git status @args }}\n\
         function rmrf {{ rm -rf @args }}\n\
         GST --shrot\n\
         fuck --explain\n\
         rmrf ./buidl\n\
         fuck --explain\n\
         fuck -y\n\
         exit\n",
        env!("CARGO_BIN_EXE_notypo")
    );
    let mut child = workspace
        .program(&pwsh, "")
        .env("PATH", system_path(&bin))
        .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
        .args(["-NoProfile", "-NoLogo", "-Command", "-"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = finish_within(child, Duration::from_secs(60));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`GST` is an alias for `git status`; its words come from git"),
        "{stderr}"
    );
    assert!(stderr.contains("1. GST --short"), "{stderr}");
    let (_, rmrf) = stderr.split_once("command:     rmrf ./buidl").unwrap();
    assert!(rmrf.contains("1. rmrf ./build"), "{stderr}");
    assert!(rmrf.contains("uses rm -rf (through an alias)"), "{stderr}");
    assert_eq!(
        fs::read_to_string(bin.join("calls")).unwrap(),
        "git status --shrot\nrm -rf ./buidl\n",
        "only the typed lines ran; -y ran nothing"
    );
}

/// PowerShell's version: a completer registered by `Invoke-Expression` of a
/// generated script is passed with that script's namespaces and functions;
/// the probe runs it as Tab would, and nothing else from the script runs.
#[test]
fn powershell_passes_session_argument_completers_without_running_their_script() {
    let Some(pwsh) = std::env::var_os("NOTYPO_TEST_PWSH")
        .map(PathBuf::from)
        .or_else(|| notypo::utils::which("pwsh"))
    else {
        eprintln!("skipped: PowerShell is not installed");
        return;
    };
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let tool = bin.join("tool");
    fs::write(&tool, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let marker = workspace.0.join("script-marker");
    // What `tool completion powershell` might print: a namespace, helper
    // functions, a statement that must not run again, and the registration.
    let generated = workspace.0.join("generated.ps1");
    fs::write(
        &generated,
        format!(
            "using namespace System.Management.Automation\n\
             function __tool_words {{ 'build'; 'deploy' }}\n\
             function __tool_unrelated {{ Set-Content -Path '{marker}' -Value function }}\n\
             Register-ArgumentCompleter -Native -CommandName tool -ScriptBlock {{\n\
                 param($wordToComplete, $commandAst, $cursorPosition)\n\
                 __tool_words | ForEach-Object {{ [CompletionResult]::new($_, $_, [CompletionResultType]::ParameterValue, ('Run ' + $_)) }}\n\
             }}\n\
             if (Test-Path '{marker}') {{ Set-Content -Path '{marker}' -Value again }} else {{ Set-Content -Path '{marker}' -Value once }}\n",
            marker = marker.display()
        ),
    )
    .unwrap();
    let input = format!(
        "$env:TF_SHELL = 'powershell'\n\
         Invoke-Expression ((& '{}' --alias) -join \"`n\")\n\
         Get-Content -Raw -LiteralPath '{}' | Invoke-Expression\n\
         tool biuld\n\
         fuck --explain\n\
         $env:NOTYPO_TRUSTED_COMPLETERS = 'tool'\n\
         tool biuld\n\
         fuck --explain\n\
         exit\n",
        env!("CARGO_BIN_EXE_notypo"),
        generated.display()
    );
    let mut child = workspace
        .program(&pwsh, "")
        .env("PATH", system_path(&bin))
        .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
        .env_remove("NOTYPO_TRUSTED_COMPLETERS")
        .args(["-NoProfile", "-NoLogo", "-Command", "-"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = finish_within(child, Duration::from_secs(60));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let (untrusted, trusted) = stderr
        .split_once("probes:")
        .unwrap_or_else(|| panic!("{stdout}\n{stderr}"));
    assert!(
        untrusted.contains(
            "tool has an argument completer in your PowerShell session; add it to trusted_completers"
        ),
        "{stderr}"
    );
    assert!(
        trusted.contains("your PowerShell session's argument completer"),
        "{stderr}"
    );
    assert!(trusted.contains("1. tool build"), "{stderr}");
    assert!(trusted.contains("`build`: Run build"), "{stderr}");
    assert_eq!(
        fs::read_to_string(&marker).unwrap().trim(),
        "once",
        "the generated script ran only when the session loaded it"
    );
}

/// The alias hands the failed command's exit status (127) to the engine,
/// which makes the missing-executable diagnosis strong enough for `-y`.
#[test]
fn shell_aliases_pass_the_exit_status_to_the_native_engine() {
    for shell in ["bash", "zsh", "fish"] {
        let Some(path) = (if shell == "fish" {
            fish_with_history_append()
        } else {
            notypo::utils::which(shell)
        }) else {
            continue;
        };
        let workspace = Workspace::new();
        let bin = workspace.0.join("bin");
        fs::create_dir(&bin).unwrap();
        let git = bin.join("git");
        fs::write(
            &git,
            "#!/bin/sh\n[ \"$1\" = status ] && printf 'correction executed\\n'\n",
        )
        .unwrap();
        fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
        let output = finish(
            workspace
                .command("")
                .env("TF_SHELL", shell)
                .arg("--alias")
                .spawn()
                .unwrap(),
        );
        let alias = String::from_utf8(output.stdout).unwrap();
        let (flags, seed): (&[&str], _) = match shell {
            "bash" => (
                &["--norc", "--noprofile", "-i", "-c"],
                "set -o history\nhistory -s 'gti status'",
            ),
            "zsh" => (
                &["-f", "-i", "-c"],
                "HISTSIZE=50\nfc -p\nprint -s -- 'gti status'",
            ),
            "fish" => (
                &["--no-config", "--private", "-c"],
                "builtin history append -- 'gti status'",
            ),
            _ => unreachable!(),
        };
        let script = format!("{alias}\n{seed}\ngti status\nfuck -y\n");
        let output = finish(
            workspace
                .program(&path, "")
                .env("TF_SHELL", shell)
                .env("PATH", &bin)
                .env("HISTFILE", workspace.0.join("history"))
                .args(flags)
                .arg(&script)
                .spawn()
                .unwrap(),
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "correction executed\n",
            "{shell}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// A typo inside a loop is repaired in place through each shell's alias;
/// the loop around it (and fish's block around that) is unchanged, and the
/// corrected loop runs once.
#[test]
fn shell_aliases_repair_commands_inside_loops() {
    for shell in ["bash", "zsh", "fish"] {
        let Some(path) = (if shell == "fish" {
            fish_with_history_append()
        } else {
            notypo::utils::which(shell)
        }) else {
            continue;
        };
        let workspace = Workspace::new();
        let bin = workspace.0.join("bin");
        fs::create_dir(&bin).unwrap();
        let git = bin.join("git");
        fs::write(
            &git,
            "#!/bin/sh\n[ \"$1\" = status ] && printf 'status for %s\\n' \"$2\"\n",
        )
        .unwrap();
        fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
        let output = finish(
            workspace
                .command("")
                .env("TF_SHELL", shell)
                .arg("--alias")
                .spawn()
                .unwrap(),
        );
        let alias = String::from_utf8(output.stdout).unwrap();
        let (flags, typed, seed): (&[&str], _, _) = match shell {
            "bash" => (
                &["--norc", "--noprofile", "-i", "-c"],
                "for repo in a 'b c'; do gti status \"$repo\"; done",
                "set -o history\nhistory -s",
            ),
            "zsh" => (
                &["-f", "-i", "-c"],
                "for repo in a 'b c'; do gti status \"$repo\"; done",
                "HISTSIZE=50\nfc -p\nprint -s --",
            ),
            // fish 4's braces around the loop: `}` needs no `;`.
            "fish" => (
                &["--no-config", "--private", "-c"],
                "{ for repo in a 'b c'; gti status $repo; end }",
                "builtin history append --",
            ),
            _ => unreachable!(),
        };
        let quoted = Shell::Bash.quote(typed);
        let script = format!("{alias}\n{seed} {quoted}\n{typed}\nfuck -y\n");
        let output = finish(
            workspace
                .program(&path, "")
                .env("TF_SHELL", shell)
                .env("PATH", &bin)
                .env("HISTFILE", workspace.0.join("history"))
                .args(flags)
                .arg(&script)
                .spawn()
                .unwrap(),
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "status for a\nstatus for b c\n",
            "{shell}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// tcsh's alias hands over the previous event intact (quotes, globs, and
/// escaped `!` included) with its status, and runs the approved correction
/// once; a line tcsh would history-substitute again is never rerun.
#[test]
fn tcsh_alias_repairs_the_previous_event_without_reinterpreting_it() {
    let Some(tcsh) = notypo::utils::which("tcsh") else {
        return;
    };
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let git = bin.join("git");
    fs::write(
        &git,
        "#!/bin/sh\n[ \"$1\" = status ] && shift && printf 'status [%s]\\n' \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(workspace.0.join("one.txt"), "").unwrap();
    let output = finish(
        workspace
            .command("")
            .env("TF_SHELL", "tcsh")
            .arg("--alias")
            .spawn()
            .unwrap(),
    );
    let alias = String::from_utf8(output.stdout).unwrap();
    let script = format!(
        "set prompt=''\nset history=100\n{alias}\n\
         gti status 'a b' '*.txt' 'it'\\''s' 'x\\!y'\nfuck -y\n\
         echo \"left: [$?NOTYPO_EXIT_STATUS$?NOTYPO_CURRENT_COMMAND]\"\n\
         gti status 'p\\!q'\nfuck -y\nexit\n"
    );
    let mut child = workspace
        .program(&tcsh, "")
        .env("TF_SHELL", "tcsh")
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .args(["-f", "-i"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let output = finish(child);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The history keeps `x!y` without its backslash, so that line is
    // refused rather than rerun with a new history substitution.
    assert!(!stdout.contains("status ["), "{stdout}\n{stderr}");
    assert_eq!(stderr.matches("No fucks given").count(), 2, "{stderr}");
    assert!(stdout.contains("left: [00]"), "{stdout}");

    let script = format!(
        "set prompt=''\nset history=100\n{alias}\n\
         gti status 'a b' '*.txt' 'it'\\''s' >& log\nfuck -y\nexit\n"
    );
    let mut child = workspace
        .program(&tcsh, "")
        .env("TF_SHELL", "tcsh")
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .args(["-f", "-i"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let output = finish(child);
    assert_eq!(
        fs::read_to_string(workspace.0.join("log")).unwrap(),
        "status [a b]\nstatus [*.txt]\nstatus [it's]\n",
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The generic POSIX function (for ksh and other shells notypo doesn't
/// know) passes the status and recent history, skips its own line, and
/// forwards `-y` to notypo rather than to the corrected command.
#[test]
fn generic_function_repairs_the_previous_command_in_ksh() {
    let Some(ksh) = notypo::utils::which("ksh") else {
        return;
    };
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let git = bin.join("git");
    fs::write(
        &git,
        "#!/bin/sh\n[ \"$1\" = status ] && shift && printf 'status [%s]\\n' \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    let function = Shell::Generic.app_alias("fuck", env!("CARGO_BIN_EXE_notypo"), false);
    let script = format!(
        "{function}\ngti status 'a b'\nfuck -y\necho \"left=[${{notypo_status-unset}}]\"\n"
    );
    let mut child = workspace
        .program(&ksh, "")
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("ENV", "/dev/null")
        .arg("-i")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let output = finish(child);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("status [a b]\n") && stdout.matches("status [").count() == 1,
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("left=[unset]"), "{stdout}");
}

#[test]
fn fish_alias_preserves_pipeline_metadata_and_current_history_event() {
    let Some(fish) = fish_with_history_append() else {
        return;
    };
    let workspace = Workspace::new();
    let spy = workspace.0.join("spy");
    fs::write(
        &spy,
        "#!/bin/sh\nprintf '%s\\n' \"$NOTYPO_EXIT_STATUS\" \"$NOTYPO_PIPESTATUS\" \"$NOTYPO_CURRENT_COMMAND\" \"$NOTYPO_SHELL_FUNCTIONS\" \"$NOTYPO_FISH_COMPLETE_PATH\" \"$TF_SHELL:$TF_ALIAS\" > metadata\n",
    ).unwrap();
    fs::set_permissions(&spy, fs::Permissions::from_mode(0o755)).unwrap();
    let alias = Shell::Fish.app_alias("fix", spy.to_str().unwrap(), true);
    let script = format!(
        "{alias}\nfunction deploy_app; end\nset -g fish_complete_path /custom/completions /vendor/completions\nbuiltin history append -- 'false | true'\nfalse | true\nfix -y\n"
    );
    let output = finish(
        workspace
            .program(fish, "")
            .args(["--no-config", "--private", "-c", &script])
            .spawn()
            .unwrap(),
    );
    assert!(output.stdout.is_empty());
    let metadata = fs::read_to_string(workspace.0.join("metadata")).unwrap();
    let lines: Vec<_> = metadata.lines().collect();
    assert_eq!(&lines[..3], ["0", "1 0", "false | true"]);
    assert!(lines[3].split_whitespace().any(|word| word == "deploy_app"));
    assert!(lines[4].starts_with("/custom/completions:"));
    assert_eq!(lines[5], "fish:fix");
    assert!(!workspace.0.join("fish/fish_history").exists());
}

#[test]
fn fish_alias_executes_the_complete_multiline_selection_once() {
    let Some(fish) = fish_with_history_append() else {
        return;
    };
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let git = bin.join("git");
    fs::write(
        &git,
        "#!/bin/sh\n[ \"$1\" = status ] || exit 1\nprintf 'correction executed\\n'\nprintf 'once\\n' >> executions\n",
    ).unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    let alias = Shell::Fish.app_alias("fix", env!("CARGO_BIN_EXE_notypo"), true);
    let original = "gti status\nand printf 'second line\\n'";
    let script = format!(
        "{alias}\nbuiltin history append -- {}\n{original}\nfix -y\n",
        Shell::Fish.quote(original)
    );
    let output = finish(
        workspace
            .program(fish, "")
            .env("PATH", &bin)
            .args(["--no-config", "--private", "-c", &script])
            .spawn()
            .unwrap(),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "correction executed\nsecond line\n",
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(workspace.0.join("executions")).unwrap(),
        "once\n"
    );
    assert!(!workspace.0.join("fish/fish_history").exists());
}

#[test]
fn fish_alias_executes_cd_in_the_parent_shell() {
    let Some(fish) = fish_with_history_append() else {
        return;
    };
    let workspace = Workspace::new();
    fs::create_dir_all(workspace.0.join("project/src")).unwrap();
    let alias = Shell::Fish.app_alias("fix", env!("CARGO_BIN_EXE_notypo"), true);
    let script = format!(
        "{alias}\nbuiltin history append -- 'cd project/scr'\ncd project/scr\nfix -y\nprintf '%s' $PWD\n"
    );
    let output = finish(
        workspace
            .program(fish, "")
            .env("TF_SHELL", "fish")
            .args(["--no-config", "--private", "-c", &script])
            .spawn()
            .unwrap(),
    );
    let expected = fs::canonicalize(workspace.0.join("project/src")).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        expected.to_string_lossy(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!workspace.0.join("fish/fish_history").exists());
}

#[test]
fn fish_native_completion_repairs_nested_words_and_values_without_running_operations() {
    let Some(fish) = notypo::utils::which("fish") else {
        return;
    };
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let tool = bin.join("fishcli");
    fs::write(&tool, "#!/bin/sh\nprintf ran > operation-marker\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let completion_dir = workspace.0.join("fish/completions");
    fs::create_dir_all(&completion_dir).unwrap();
    fs::write(completion_dir.join("fishcli.fish"), r#"complete -c fishcli -f
complete -c fishcli -n 'not __fish_seen_subcommand_from nodes' -a nodes
complete -c fishcli -n '__fish_seen_subcommand_from nodes; and not __fish_seen_subcommand_from list' -a list
complete -c fishcli -n '__fish_seen_subcommand_from list' -l format -x -a "'alpha beta' json yaml"
"#).unwrap();
    // Neither command discovery nor function discovery may start an
    // interactive fish that sources the user's configuration.
    fs::write(
        workspace.0.join("fish/config.fish"),
        "printf loaded > config-marker\n",
    )
    .unwrap();
    let command = || {
        let mut command = workspace.command("");
        command
            .env("TF_SHELL", "fish")
            .env(
                "PATH",
                format!(
                    "{}:{}:/usr/bin:/bin",
                    bin.display(),
                    fish.parent().unwrap().display()
                ),
            )
            .env("NOTYPO_TRUSTED_COMPLETERS", "fishcli")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy");
        command
    };
    let source = "fishcli nodes lsit --format='alpha beat' 'private arg' 2>| cat";
    let output = finish(
        command()
            .args(["--json", "--force-command", source])
            .spawn()
            .unwrap(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["outcome"]["kind"], "ambiguous", "{report}");
    assert_eq!(
        report["candidates"][0]["command"],
        "fishcli nodes list --format='alpha beta' 'private arg' 2>| cat",
        "{report}"
    );
    assert_eq!(
        report["candidates"][0]["edits"].as_array().unwrap().len(),
        2
    );
    assert!(report["notes"].as_array().unwrap().iter().any(|note| {
        note.as_str()
            .is_some_and(|note| note.contains("fish protocol"))
    }));
    let denied = finish(
        command()
            .args(["-y", "--force-command", source])
            .spawn()
            .unwrap(),
    );
    assert!(!denied.status.success());
    assert!(denied.stdout.is_empty());
    for marker in ["operation-marker", "config-marker", "fish/fish_history"] {
        assert!(!workspace.0.join(marker).exists(), "unexpected {marker}");
    }
}

#[test]
fn zsh_alias_passes_completion_paths_without_losing_pipeline_status() {
    let Some(zsh) = notypo::utils::which("zsh") else {
        return;
    };
    let workspace = Workspace::new();
    let backend = workspace.0.join("backend");
    fs::write(&backend, "#!/bin/sh\nprintf '%s\\n' \"$NOTYPO_EXIT_STATUS\" \"$NOTYPO_PIPESTATUS\" \"$NOTYPO_ZSH_FPATH\" > metadata\n").unwrap();
    fs::set_permissions(&backend, fs::Permissions::from_mode(0o755)).unwrap();
    let alias = Shell::Zsh.app_alias("fix", backend.to_str().unwrap(), false);
    let path = workspace.0.join("completion directory");
    let script = format!(
        "{alias}\nfpath=( {} $fpath )\nfalse | true\nfix\n",
        notypo::shlex::quote(&path.to_string_lossy())
    );
    let output = finish(
        workspace
            .program(zsh, "")
            .args(["-f", "-i", "-c", &script])
            .spawn()
            .unwrap(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = fs::read_to_string(workspace.0.join("metadata")).unwrap();
    let mut lines = metadata.lines();
    assert_eq!(lines.next(), Some("0"));
    assert_eq!(lines.next(), Some("1 0"));
    assert!(
        lines
            .next()
            .unwrap()
            .starts_with(&format!("{}:", path.display()))
    );
}

#[test]
fn zsh_native_completion_repairs_nested_words_options_and_values_without_running_operations() {
    let Some(zsh) = notypo::utils::which("zsh") else {
        return;
    };
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    let completions = workspace.0.join("completions");
    fs::create_dir(&bin).unwrap();
    fs::create_dir(&completions).unwrap();
    let tool = bin.join("zshcli");
    fs::write(&tool, "#!/bin/sh\nprintf ran > operation-marker\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    // A shared handler registers an alias that differs from its filename.
    fs::write(completions.join("_shared_cli"), r#"#compdef sharedcli zshcli
case ${(Q)words[2]} in
  '') compadd -- nodes;;
  nodes)
    case ${(Q)words[3]} in
      '') compadd -- list;;
      list) _arguments '1:group:(nodes)' '2:command:(list)' '--format[Format]:format:(json yaml)' '*:argument:';;
    esac;;
esac
"#).unwrap();
    for config in [".zshenv", ".zshrc"] {
        fs::write(workspace.0.join(config), "print ran > config-marker\n").unwrap();
    }
    let command = || {
        let mut command = workspace.command("");
        command
            .env("TF_SHELL", "zsh")
            .env("ZDOTDIR", &workspace.0)
            .env("NOTYPO_ZSH_FPATH", &completions)
            .env(
                "PATH",
                format!(
                    "{}:{}:/usr/bin:/bin",
                    bin.display(),
                    zsh.parent().unwrap().display()
                ),
            )
            .env("NOTYPO_TRUSTED_COMPLETERS", "zshcli")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy");
        command
    };
    for (source, expected) in [
        (
            "zshcli nodes lsit --foramt=yaml 'private arg' 2> /dev/null | cat",
            "zshcli nodes list --format=yaml 'private arg' 2> /dev/null | cat",
        ),
        (
            "zshcli nodes list --format jsno",
            "zshcli nodes list --format json",
        ),
    ] {
        let output = finish(
            command()
                .args(["--json", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["outcome"]["kind"], "ambiguous", "{report}");
        assert_eq!(report["candidates"][0]["command"], expected, "{report}");
        assert!(
            report["notes"].as_array().unwrap().iter().any(|note| note
                .as_str()
                .is_some_and(|note| note.contains("zsh protocol"))),
            "{report}"
        );
        let denied = finish(
            command()
                .args(["-y", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        assert!(!denied.status.success());
        assert!(denied.stdout.is_empty());
    }
    let untrusted = finish(
        command()
            .env("NOTYPO_TRUSTED_COMPLETERS", "")
            .args(["--json", "--force-command", "zshcli nodes lsit"])
            .spawn()
            .unwrap(),
    );
    // A handler's protocol is independent of the shell editing the command.
    let fallback = finish(
        command()
            .env("TF_SHELL", "bash")
            .args(["--json", "--force-command", "zshcli nodes lsit"])
            .spawn()
            .unwrap(),
    );
    let fallback: serde_json::Value = serde_json::from_slice(&fallback.stdout).unwrap();
    assert_eq!(
        fallback["candidates"][0]["command"], "zshcli nodes list",
        "{fallback}"
    );
    let report: serde_json::Value = serde_json::from_slice(&untrusted.stdout).unwrap();
    assert!(
        report["notes"].as_array().unwrap().iter().any(|note| note
            .as_str()
            .is_some_and(|note| note.contains("trusted_completers"))),
        "{report}"
    );
    for marker in ["operation-marker", "config-marker", ".zcompdump", "history"] {
        assert!(!workspace.0.join(marker).exists(), "unexpected {marker}");
    }
}

#[test]
fn fish_substitutions_are_neither_replayed_nor_automatically_accepted() {
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let git = bin.join("git");
    fs::write(&git, "#!/bin/sh\nprintf 'status\\n'\n").unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    for source in [
        "gti (printf rerun > marker)",
        "gti $(printf rerun > marker)",
        "gti <(printf rerun > marker)",
    ] {
        let command = || {
            let mut command = workspace.command("");
            command
                .env("TF_SHELL", "fish")
                .env("PATH", system_path(&bin))
                .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "true");
            command
        };
        let output = finish(
            command()
                .args(["--json", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        assert!(output.status.success());
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["candidates"][0]["safety"]["decision"], "confirm",
            "{report}"
        );
        let output = finish(
            command()
                .args(["-y", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!workspace.0.join("marker").exists());
    }
}

#[test]
fn fish_history_uses_the_selected_data_session_and_decodes_multiline_commands() {
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let tool = bin.join("fishcli");
    fs::write(&tool, "#!/bin/sh\nprintf ran > marker\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let data = workspace.0.join("fish");
    fs::create_dir(&data).unwrap();
    let contents = "- cmd: printf 'a\\\\b'\\nfishcli publish\n  when: 1\n  paths:\n    - cmd: fishcli demolish\n";
    fs::write(data.join("work_history"), contents).unwrap();
    fs::write(data.join("fish_history"), contents).unwrap();
    let command = || {
        let mut command = workspace.command("");
        command
            .env("TF_SHELL", "fish")
            .env("PATH", system_path(&bin))
            .env("NOTYPO_DISABLED_SOURCES", "native:man:help:legacy")
            .args(["--json", "--force-command", "fishcli publsih"]);
        command
    };
    for session in ["work", "default"] {
        let output = finish(command().env("fish_history", session).spawn().unwrap());
        assert!(output.status.success());
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["candidates"][0]["command"], "fishcli publish",
            "{session}: {report}"
        );
    }
    let output = finish(
        command()
            .env("fish_history", "missing")
            .env("NOTYPO_FISH_HISTORY_SESSION", "work")
            .spawn()
            .unwrap(),
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["candidates"][0]["command"], "fishcli publish",
        "parent metadata takes precedence"
    );
    let output = finish(
        command()
            .env("NOTYPO_FISH_HISTORY_SESSION", "")
            .spawn()
            .unwrap(),
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["candidates"].as_array().unwrap().is_empty(),
        "private history is unavailable: {report}"
    );
    assert_eq!(
        fs::read_to_string(data.join("work_history")).unwrap(),
        contents
    );
    assert_eq!(
        fs::read_to_string(data.join("fish_history")).unwrap(),
        contents
    );
    assert!(!workspace.0.join("marker").exists());
}

#[test]
fn powershell_syntax_notypo_cannot_analyze_never_runs_under_yes() {
    let workspace = Workspace::new();
    for replay in ["false", "true"] {
        // Statements and expressions are PowerShell's, not notypo's to edit.
        let output = finish(
            workspace
                .command("cd_parent")
                .env("TF_SHELL", "powershell")
                .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", replay)
                .args(["-y", "--force-command", "foreach ($d in $dirs) { cd.. }"])
                .spawn()
                .unwrap(),
        );
        assert!(!output.status.success(), "replay={replay}");
        assert!(output.stdout.is_empty(), "replay={replay}");
    }
    // An analyzable PowerShell line is offered and gated like any other:
    // without a reported failure, -y still declines to run it.
    let output = finish(
        workspace
            .command("cd_parent")
            .env("TF_SHELL", "powershell")
            .args(["-y", "--force-command", "cd.."])
            .spawn()
            .unwrap(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.stdout.is_empty());
    assert!(stderr.contains("Not running `cd ..`"), "{stderr}");
    assert!(!stderr.contains("unsupported syntax"), "{stderr}");
}

#[test]
fn installed_click_script_selects_click_completion_without_sourcing_it() {
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let Some(bash) = notypo::utils::which("bash") else {
        eprintln!("skipped: bash is not installed");
        return;
    };
    std::os::unix::fs::symlink(bash, bin.join("bash")).unwrap();
    // A shell stand-in for Python running a click 8 console script.
    let python = bin.join("python3");
    fs::write(
        &python,
        r#"#!/bin/sh
root=$(dirname "$(dirname "$0")")
[ "$1" = -m ] && [ "$2" = clickfix ] || exit 8
shift 2
printf '[%s]%s|%s|%s\n' "$*" "$_CLICKFIX_COMPLETE" "$COMP_CWORD" "$COMP_WORDS" >> "$root/calls"
[ "$#" = 0 ] && [ "$_CLICKFIX_COMPLETE" = bash_complete ] || { touch "$root/operation-marker"; exit 2; }
[ "$HTTPS_PROXY" = http://127.0.0.1:9 ] && [ "$PYTHONDONTWRITEBYTECODE" = 1 ] || exit 8
case "$COMP_CWORD|$COMP_WORDS" in
  '2|clickfix deploy') printf 'plain,status\n';;
  *) printf 'plain,db:migrate\nplain,deploy\n';;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&python, fs::Permissions::from_mode(0o755)).unwrap();
    let app = bin.join("clickfix");
    fs::write(
        &app,
        format!("#!/bin/sh\nexec {} -m clickfix \"$@\"\n", python.display()),
    )
    .unwrap();
    fs::set_permissions(&app, fs::Permissions::from_mode(0o755)).unwrap();
    // As click 8 generates it, plus a line that would run if sourced.
    let completions = workspace.0.join("bash-completion/completions");
    fs::create_dir_all(&completions).unwrap();
    fs::write(
        completions.join("clickfix"),
        r#"touch sourced-marker
_clickfix_completion() {
    local IFS=$'\n'
    local response

    response=$(env COMP_WORDS="${COMP_WORDS[*]}" COMP_CWORD=$COMP_CWORD _CLICKFIX_COMPLETE=bash_complete $1)
}

_clickfix_completion_setup() {
    complete -o nosort -F _clickfix_completion clickfix
}

_clickfix_completion_setup;
"#,
    )
    .unwrap();
    let command = |trusted: &str| {
        let mut command = workspace.command("");
        command
            .env("PATH", system_path(&bin))
            .env(
                "BASH_COMPLETION_USER_DIR",
                workspace.0.join("bash-completion"),
            )
            .env("NOTYPO_TRUSTED_COMPLETERS", trusted)
            .env("NOTYPO_TRUSTED_HELP", "")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env("_CLICKFIX_COMPLETE", "bash_source");
        command
    };
    let report = |trusted: &str, source: &str| {
        let output = finish(
            command(trusted)
                .args(["--json", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let untrusted = report("", "clickfix deplyo");
    assert_eq!(untrusted["probes"], 0, "{untrusted}");
    assert!(!workspace.0.join("calls").exists());
    for (source, expected, decision) in [
        ("clickfix deplyo", "clickfix deploy", "allow"),
        ("clickfix deploy statsu", "clickfix deploy status", "allow"),
    ] {
        let report = report(r#"["python:clickfix"]"#, source);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{report}");
        assert_eq!(candidate["safety"]["decision"], decision, "{report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "click completion"),
            "{report}"
        );
    }
    let output = finish(
        command(r#"["python:clickfix"]"#)
            .args(["-y", "--force-command", "clickfix deplyo"])
            .spawn()
            .unwrap(),
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "clickfix deploy"
    );
    let calls = fs::read_to_string(workspace.0.join("calls")).unwrap();
    assert!(
        calls
            .lines()
            .all(|line| line.starts_with("[]bash_complete|")),
        "only completion requests, without arguments: {calls}"
    );
    assert!(!workspace.0.join("operation-marker").exists());
    assert!(!workspace.0.join("sourced-marker").exists());
}

#[test]
fn oclif_manifests_decide_repairs_without_running_the_app() {
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let package = workspace.0.join("lib/node_modules/odemo-cli");
    fs::create_dir_all(package.join("bin")).unwrap();
    fs::write(
        package.join("package.json"),
        r#"{"name": "odemo-cli", "version": "1.2.3", "bin": {"odemo": "bin/run"}, "oclif": {"bin": "odemo", "dirname": "notypo-cli-test-odemo"}}"#,
    )
    .unwrap();
    fs::write(
        package.join("oclif.manifest.json"),
        r#"{"version": "1.2.3", "commands": {
          "deploy": {"id": "deploy", "description": "Deploy the app", "flags": {
            "region": {"name": "region", "char": "r", "type": "option", "options": ["eu-west", "us-east"]}}, "args": {}},
          "deploy:list": {"id": "deploy:list", "flags": {}, "args": {}}}}"#,
    )
    .unwrap();
    let app = package.join("bin/run");
    fs::write(
        &app,
        format!(
            "#!/bin/sh\ntouch '{}'\n",
            workspace.0.join("operation-marker").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&app, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(&app, bin.join("odemo")).unwrap();
    let command = || {
        let mut command = workspace.command("");
        command
            .env("PATH", system_path(&bin))
            .env("XDG_DATA_HOME", workspace.0.join("data"))
            .env("NOTYPO_TRUSTED_COMPLETERS", "")
            .env("NOTYPO_TRUSTED_HELP", "")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false");
        command
    };
    for (source, expected) in [
        (
            "odemo deplyo --region eu-west",
            "odemo deploy --region eu-west",
        ),
        (
            "odemo deploy --regoin eu-west",
            "odemo deploy --region eu-west",
        ),
        ("odemo deploy:lsit", "odemo deploy:list"),
    ] {
        let output = finish(
            command()
                .args(["-y", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            expected,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let report = |source: &str| {
        let output = finish(
            command()
                .args(["--json", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let deploy = report("odemo deplyo");
    assert_eq!(deploy["probes"], 0, "{deploy}");
    assert_eq!(
        deploy["candidates"][0]["edits"][0]["via"],
        "oclif completion"
    );
    // Value lists are never treated as complete, so this one is asked.
    let value = report("odemo deploy -r eu-wset");
    assert_eq!(value["outcome"]["kind"], "ambiguous", "{value}");
    assert_eq!(value["candidates"][0]["command"], "odemo deploy -r eu-west");
    assert!(!workspace.0.join("operation-marker").exists());
}

#[test]
fn installed_yargs_script_selects_yargs_completion_without_sourcing_it() {
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    // A shell stand-in for a Node bin built on yargs, installed by npm.
    let package = workspace.0.join("lib/node_modules/ydemo");
    fs::create_dir_all(package.join("bin")).unwrap();
    fs::write(
        package.join("package.json"),
        r#"{"name": "ydemo", "bin": {"ydemo": "bin/ydemo.js"}, "dependencies": {"yargs": "^18.2.0"}}"#,
    )
    .unwrap();
    let app = package.join("bin/ydemo.js");
    fs::write(
        &app,
        format!(
            r#"#!/bin/sh
root='{}'
printf '%s|' "$ZSH_NAME" >> "$root/calls"
printf '<%s>' "$@" >> "$root/calls"
echo >> "$root/calls"
[ "$1" = --get-yargs-completions ] && [ "$2" = ydemo ] || {{ touch "$root/operation-marker"; exit 2; }}
[ "$HTTPS_PROXY" = http://127.0.0.1:9 ] || exit 8
shift 2
case "$ZSH_NAME|$*" in
  '|deploy '*) printf 'status\n';;
  'zsh|deploy '*) printf 'status:Show the status\n';;
  zsh*) printf 'deploy:Deploy the app\ndb:Database tasks\n';;
  *) printf 'deploy\ndb\n';;
esac
"#,
            workspace.0.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&app, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(&app, bin.join("ydemo")).unwrap();
    // As yargs 18.2.0 generates it, plus a line that would run if sourced.
    let completions = workspace.0.join("bash-completion/completions");
    fs::create_dir_all(&completions).unwrap();
    fs::write(
        completions.join("ydemo"),
        r#"###-begin-ydemo-completions-###
#
# yargs command completion script
#
touch sourced-marker
_ydemo_yargs_completions()
{
    local cur_word args type_list

    cur_word="${COMP_WORDS[COMP_CWORD]}"
    args=("${COMP_WORDS[@]}")
    mapfile -t type_list < <(ydemo --get-yargs-completions "${args[@]}")
    return 0
}
complete -o bashdefault -o default -F _ydemo_yargs_completions ydemo
###-end-ydemo-completions-###
"#,
    )
    .unwrap();
    let command = |trusted: &str| {
        let mut command = workspace.command("");
        command
            .env("PATH", system_path(&bin))
            .env(
                "BASH_COMPLETION_USER_DIR",
                workspace.0.join("bash-completion"),
            )
            .env("NOTYPO_TRUSTED_COMPLETERS", trusted)
            .env("NOTYPO_TRUSTED_HELP", "")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false");
        command
    };
    let report = |trusted: &str, source: &str| {
        let output = finish(
            command(trusted)
                .args(["--json", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let untrusted = report("", "ydemo deplyo");
    assert_eq!(untrusted["probes"], 0, "{untrusted}");
    assert!(!workspace.0.join("calls").exists());
    for (source, expected) in [
        ("ydemo deplyo", "ydemo deploy"),
        ("ydemo deploy statsu", "ydemo deploy status"),
    ] {
        let report = report(r#"["npm:ydemo"]"#, source);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{report}");
        assert_eq!(candidate["safety"]["decision"], "allow", "{report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "yargs completion"),
            "{report}"
        );
    }
    let output = finish(
        command(r#"["npm:ydemo"]"#)
            .args(["-y", "--force-command", "ydemo deplyo"])
            .spawn()
            .unwrap(),
    );
    // yargs lists no aliases or hidden commands, so a repair is asked.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.stdout.is_empty(), "{stderr}");
    assert!(
        stderr.contains("Not running `ydemo deploy` without confirmation"),
        "{stderr}"
    );
    let calls = fs::read_to_string(workspace.0.join("calls")).unwrap();
    assert!(
        calls
            .lines()
            .all(|line| line.contains("|<--get-yargs-completions><ydemo>")),
        "only completion requests, led by the program name: {calls}"
    );
    assert!(!workspace.0.join("operation-marker").exists());
    assert!(!workspace.0.join("sourced-marker").exists());
}

#[test]
fn aliases_use_their_target_programs_vocabulary_and_safety_policy() {
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    for (name, help) in [
        (
            "eza",
            "Usage: eza [options] [files...]\n\nOPTIONS\n  --color WHEN    when to use terminal colours\n  --icons         display icons\n",
        ),
        (
            "ls",
            "usage: ls [-l]\n  --classify      append indicators\n",
        ),
    ] {
        let path = bin.join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s:%s\\n' {name} \"$*\" >> \"$(dirname \"$0\")/calls\"\n[ \"$*\" = --help ] && {{ printf '%s' '{help}'; exit 0; }}\ntouch \"$(dirname \"$0\")/operation-marker\"\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let rm = bin.join("rm");
    fs::write(
        &rm,
        "#!/bin/sh\ntouch \"$(dirname \"$0\")/operation-marker\"\n",
    )
    .unwrap();
    fs::set_permissions(&rm, fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir(workspace.0.join("build")).unwrap();
    let command = |aliases: &str| {
        let mut command = workspace.command("");
        command
            .env("PATH", system_path(&bin))
            .env("TF_SHELL_ALIASES", aliases)
            .env("NOTYPO_TRUSTED_COMPLETERS", "")
            .env("NOTYPO_TRUSTED_HELP", "eza:ls")
            .env("NOTYPO_DISABLED_SOURCES", "man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env("NOTYPO_EXIT_STATUS", "1");
        command
    };
    let json = |aliases: &str, source: &str| {
        let output = finish(
            command(aliases)
                .args(["--json", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let report = json("alias ls='eza --icons'", "ls --colro=auto");
    assert_eq!(
        report["candidates"][0]["command"], "ls --color=auto",
        "{report}"
    );
    let calls = fs::read_to_string(bin.join("calls")).unwrap();
    assert!(calls.contains("eza:--help"), "{calls}");
    assert!(
        !calls.contains("ls:"),
        "the alias's own name is never asked: {calls}"
    );
    // Without the alias, ls answers for itself and has no --color.
    let report = json("", "ls --colro=auto");
    assert_ne!(
        report["candidates"][0]["command"], "ls --color=auto",
        "{report}"
    );
    // `rmf` runs `rm -rf`: a changed target needs approval, -y or not.
    // As the shell function passes it: the history and the failed status.
    let output = finish(
        command("alias rmf='rm -rf'")
            .env("TF_HISTORY", "rmf ./buidl\nfuck")
            .arg("--json")
            .spawn()
            .unwrap(),
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let candidate = &report["candidates"][0];
    assert_eq!(candidate["command"], "rmf ./build", "{report}");
    assert_eq!(candidate["safety"]["decision"], "confirm", "{report}");
    assert!(
        candidate["safety"]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|why| why.as_str().unwrap().contains("through an alias")),
        "{report}"
    );
    let output = finish(
        command("alias rmf='rm -rf'")
            .env("TF_HISTORY", "rmf ./buidl\nfuck -y")
            .arg("-y")
            .spawn()
            .unwrap(),
    );
    assert!(output.stdout.is_empty());
    assert!(!bin.join("operation-marker").exists());
    assert!(workspace.0.join("build").exists());
}

#[test]
fn powershell_function_repairs_programs_cmdlets_and_parameters() {
    let Some(pwsh) = std::env::var_os("NOTYPO_TEST_PWSH")
        .map(PathBuf::from)
        .or_else(|| notypo::utils::which("pwsh"))
    else {
        eprintln!("skipped: PowerShell is not installed");
        return;
    };
    let Some(git) = notypo::utils::which("git") else {
        eprintln!("skipped: git is not installed");
        return;
    };
    let workspace = Workspace::new();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::create_dir_all(workspace.0.join("one")).unwrap();
    fs::write(workspace.0.join("one/first.txt"), "").unwrap();
    fs::create_dir_all(workspace.0.join("two/deep")).unwrap();
    fs::write(workspace.0.join("two/deep/second.txt"), "").unwrap();
    // notypo's metadata queries reach git; anything else is recorded.
    let wrapper = bin.join("git");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\ncase \"$*\" in --list-cmds=*|-h|'-C '*|'init -q --template= '*) exec {} \"$@\";; esac\nprintf '%s\\n' \"$*\" >> \"$(dirname \"$0\")/calls\"\nexit 1\n",
            git.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    // Lines typed at an interactive prompt: each failure is a real history
    // entry, with PowerShell's own status and error records.
    let input = format!(
        "$env:TF_SHELL = 'powershell'\n\
         Invoke-Expression ((& '{}' --alias) -join \"`n\")\n\
         Get-ChildItme -Name -Path one\n\
         fuck -y\n\
         Get-ChildItem -Path two -Recrse -Name\n\
         fuck -y\n\
         git sttus\n\
         fuck -y\n\
         exit\n",
        env!("CARGO_BIN_EXE_notypo")
    );
    let mut child = workspace
        .program(&pwsh, "")
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
        .args(["-NoProfile", "-NoLogo", "-Command", "-"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = finish_within(child, Duration::from_secs(60));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The corrected cmdlet and parameter ran and listed the files.
    assert!(stdout.contains("first.txt"), "{stdout}\n{stderr}");
    assert!(
        stdout.contains(&format!("deep{}second.txt", std::path::MAIN_SEPARATOR)),
        "{stdout}\n{stderr}"
    );
    assert_eq!(
        fs::read_to_string(bin.join("calls")).unwrap_or_default(),
        "sttus\nstatus\n",
        "{stdout}\n{stderr}"
    );
}

#[test]
fn dotnet_completion_respects_project_trust_and_blocks_build_and_replay_execution() {
    let workspace = Workspace::new();
    let sdk = workspace.0.join("sdk-install");
    let version = sdk.join("sdk/10.0.401");
    fs::create_dir_all(&version).unwrap();
    fs::write(version.join(".version"), "commit\n10.0.401\n").unwrap();
    fs::write(
        version.join("dotnet.deps.json"),
        r#"{"libraries":{"dotnet.deps.json/10.0.401":{}}}"#,
    )
    .unwrap();
    fs::write(
        version.join("dotnet.dll"),
        "CompleteCommand GetCompletions PrintCliSchemaAction",
    )
    .unwrap();
    fs::write(sdk.join("schema"), r#"{
      "name":"dotnet","arguments":{"subcommand":{"hidden":true}},"options":{},"subcommands":{
        "build":{"arguments":{"project":{}},"subcommands":{},"options":{
          "--configuration":{"aliases":["-c"],"arity":{"minimum":1,"maximum":1}},
          "--verbosity":{"aliases":["/verbosity"],"arity":{"minimum":1,"maximum":1}},
          "--self-contained":{"valueType":"System.Boolean","arity":{"minimum":0,"maximum":1}}
        }},
        "nuget":{"arguments":{},"options":{},"subcommands":{"locals":{"arguments":{"folders":{}},"options":{},"subcommands":{}}}},
        "new":{"arguments":{"template":{}},"subcommands":{},"options":{"--name":{"arity":{"minimum":1,"maximum":1}}}}
      }
    }"#).unwrap();
    let host = sdk.join("dotnet");
    fs::write(&host, r#"#!/bin/sh
actual=$(readlink "$0" 2>/dev/null || printf '%s' "$0")
root=$(dirname "$actual")
[ "$HTTPS_PROXY" = http://127.0.0.1:9 ] && [ "$DOTNET_CLI_TELEMETRY_OPTOUT" = 1 ] || exit 8
touch "$DOTNET_CLI_HOME/probe-write"
printf '<%s>' "$@" >> "$root/calls"
printf '\n' >> "$root/calls"
case "$1" in
  --version) printf '10.0.401\n';;
  --info) printf 'Base Path: %s/sdk/10.0.401/\n' "$root";;
  --cli-schema) cat "$root/schema";;
  complete)
    [ "$2" = --position ] && [ "$#" = 4 ] || exit 8
    case "$4" in
      'dotnet new '*)
        [ -f "$DOTNET_CLI_HOME/.templateengine/installed-state" ] || exit 8
        touch "$DOTNET_CLI_HOME/.templateengine/completion-write"
        printf 'fixture\n--name\n';;
      'dotnet build -'|'dotnet build --self-contained true -') printf -- '--configuration\n--verbosity\n--self-contained\n-c\n';;
      'dotnet build /') printf '/verbosity\n';;
      'dotnet build --configuration '| 'dotnet build -c ') printf 'Debug\nRelease\n';;
      'dotnet build --verbosity '|'dotnet build /verbosity ') printf 'quiet\nnormal\n';;
      'dotnet nuget ') printf 'locals\n';;
      *) printf 'build\nnuget\nnew\n';;
    esac;;
  *) touch "$root/operation-marker"; exit 9;;
esac
"#).unwrap();
    fs::set_permissions(&host, fs::Permissions::from_mode(0o755)).unwrap();
    let bin = workspace.0.join("bin");
    fs::create_dir(&bin).unwrap();
    std::os::unix::fs::symlink(&host, bin.join("sdk-host")).unwrap();
    let home = workspace.0.join("user-home");
    fs::create_dir_all(home.join(".templateengine")).unwrap();
    fs::write(
        home.join(".templateengine/installed-state"),
        "original template state",
    )
    .unwrap();
    fs::write(workspace.0.join("app.csproj"), "<Project />").unwrap();
    let command = |trusted: bool, project: bool| {
        let mut command = workspace.command("");
        command
            .env("PATH", system_path(&bin))
            .env("DOTNET_CLI_HOME", &home)
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                if trusted { r#"["dotnet:sdk"]"# } else { "" },
            )
            .env(
                "NOTYPO_TRUSTED_WORKSPACES",
                if project {
                    workspace.0.to_str().unwrap()
                } else {
                    ""
                },
            )
            .env("NOTYPO_TRUSTED_HELP", "")
            .env(
                "NOTYPO_DISABLED_SOURCES",
                "help:man:history:filesystem:legacy",
            )
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false");
        command
    };
    for (trusted, project) in [(false, true), (true, false)] {
        let output = finish(
            command(trusted, project)
                .args(["--json", "--force-command", "sdk-host biuld"])
                .spawn()
                .unwrap(),
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            report["candidates"].as_array().unwrap().is_empty(),
            "{report}"
        );
        assert!(
            !sdk.join("calls").exists(),
            "refused workspace or SDK trust still probed"
        );
    }
    // Help trust does not bypass workspace trust through a renamed host.
    let output = finish(
        command(false, false)
            .env("NOTYPO_TRUSTED_HELP", "sdk-host")
            .env(
                "NOTYPO_DISABLED_SOURCES",
                "native:man:history:filesystem:legacy",
            )
            .args(["--json", "--force-command", "sdk-host biuld"])
            .spawn()
            .unwrap(),
    );
    assert!(output.status.success());
    assert!(!sdk.join("calls").exists());
    for (source, expected) in [
        ("sdk-host biuld", "sdk-host build"),
        ("sdk-host new fixtur", "sdk-host new fixture"),
        ("sdk-host nuget loclas", "sdk-host nuget locals"),
        (
            "sdk-host build --configuraton Release",
            "sdk-host build --configuration Release",
        ),
        (
            "sdk-host build --configuration Releae",
            "sdk-host build --configuration Release",
        ),
        (
            "sdk-host build --configuration=Releae",
            "sdk-host build --configuration=Release",
        ),
        (
            "sdk-host build --configuration:Releae",
            "sdk-host build --configuration:Release",
        ),
        (
            "sdk-host build /verbosoty:quiet",
            "sdk-host build /verbosity:quiet",
        ),
        (
            "sdk-host build /verbosity:quie",
            "sdk-host build /verbosity:quiet",
        ),
        (
            "sdk-host build --self-contained --configuraton Release",
            "sdk-host build --self-contained --configuration Release",
        ),
        (
            "sdk-host build --configuration Releae -- '$(touch operation-marker)'",
            "sdk-host build --configuration Release -- '$(touch operation-marker)'",
        ),
    ] {
        let output = finish(
            command(true, true)
                .args(["--json", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["candidates"][0]["command"], expected,
            "{source}: {report}"
        );
        assert_eq!(
            report["candidates"][0]["safety"]["decision"], "confirm",
            "{report}"
        );
        assert!(
            report["candidates"][0]["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "dotnet completion"),
            "{report}"
        );
        let output = finish(
            command(true, true)
                .args(["-y", "--force-command", source])
                .spawn()
                .unwrap(),
        );
        assert!(
            !output.status.success() && output.stdout.is_empty(),
            "{source}"
        );
    }
    let output = finish(
        command(false, false)
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "true")
            .env(
                "NOTYPO_DISABLED_SOURCES",
                "native:help:man:history:filesystem:legacy",
            )
            .args(["-y", "--force-command", "sdk-host build"])
            .spawn()
            .unwrap(),
    );
    assert!(!output.status.success());
    assert!(!sdk.join("operation-marker").exists());
    assert!(!workspace.0.join("operation-marker").exists());
    assert!(!home.join("probe-write").exists());
    assert_eq!(
        fs::read_to_string(home.join(".templateengine/installed-state")).unwrap(),
        "original template state"
    );
    let calls = fs::read_to_string(sdk.join("calls")).unwrap();
    assert!(
        calls.lines().all(|line| line.starts_with("<--version>")
            || line.starts_with("<--info>")
            || line.starts_with("<--cli-schema>")
            || line.starts_with("<complete><--position>")),
        "{calls}"
    );
}
