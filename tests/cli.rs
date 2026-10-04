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
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
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
/// the loop around it is unchanged, and the corrected loop runs once.
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
            "fish" => (
                &["--no-config", "--private", "-c"],
                "for repo in a 'b c'; gti status $repo; end",
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
fn unsupported_dialects_cannot_bypass_the_gate_with_a_legacy_rule() {
    let workspace = Workspace::new();
    for shell in ["powershell"] {
        for replay in ["false", "true"] {
            let output = finish(
                workspace
                    .command("cd_parent")
                    .env("TF_SHELL", shell)
                    .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", replay)
                    .args(["-y", "--force-command", "cd.."])
                    .spawn()
                    .unwrap(),
            );
            assert!(!output.status.success(), "{shell}: replay={replay}");
            assert!(output.stdout.is_empty());
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("unsupported syntax"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
