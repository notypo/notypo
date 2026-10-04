#![cfg(unix)]

use notypo::logs::USER_COMMAND_MARK;
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

fn finish(mut child: Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("CLI did not exit within 10 seconds");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn corrects_commands_through_the_cli() {
    let workspace = Workspace::new();
    let output = finish(
        workspace
            .command("cd_parent")
            .args(["-y", "--force-command", "cd.."])
            .spawn()
            .unwrap(),
    );
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "cd ..");
    let output = finish(
        workspace
            .command("mkdir_p")
            .args(["-y", "--force-command", "mkdir nested/dir"])
            .spawn()
            .unwrap(),
    );
    assert!(output.status.success());
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
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "sudo sh -c \"printf rerun > marker\""
    );
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
        "#!/bin/sh\ncase $1 in\n  sttus) printf \"git: 'sttus' is not a git command. See 'git --help'.\\n\\nThe most similar command is\\n\\tstatus\\n\"; exit 1;;\n  status) printf 'correction executed\\n';;\n  *) exit 1;;\nesac\n",
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

#[test]
fn native_engine_never_replays_the_failed_command() {
    let workspace = Workspace::new();
    for replay in ["false", "true"] {
        let output = finish(
            workspace
                .command("")
                .env("NOTYPO_ENGINE", "native")
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
            .env("NOTYPO_ENGINE", "native")
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
            .env("NOTYPO_ENGINE", "native")
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

/// The alias hands the failed command's exit status (127) to the engine,
/// which makes the missing-executable diagnosis strong enough for `-y`.
#[test]
fn shell_aliases_pass_the_exit_status_to_the_native_engine() {
    for shell in ["bash", "zsh"] {
        let Some(path) = notypo::utils::which(shell) else {
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
        let (flags, seed): (&[&str], _) = if shell == "bash" {
            (
                &["--norc", "--noprofile", "-i", "-c"],
                "set -o history\nhistory -s 'gti status'",
            )
        } else {
            (
                &["-f", "-i", "-c"],
                "HISTSIZE=50\nfc -p\nprint -s -- 'gti status'",
            )
        };
        let script = format!("{alias}\n{seed}\ngti status\nfuck -y\n");
        let output = finish(
            workspace
                .program(&path, "")
                .env("TF_SHELL", shell)
                .env("NOTYPO_ENGINE", "native")
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
