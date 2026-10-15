//! Bounded, explicitly authorized discovery for compatibility rules.
//!
//! The structured engine hands its remaining budget to this source. Replies
//! stay in request memory: they can depend on repository, project and local
//! resources, so neither process-global nor persistent caches are appropriate.

use super::{
    native,
    probe::{self, Budget, Capture, Probe},
};
use crate::types::Command;
use std::{collections::HashMap, path::PathBuf};

type Key = (PathBuf, Vec<String>, bool);

#[derive(Debug)]
pub(crate) struct Request {
    pub(crate) budget: Budget,
    cwd: PathBuf,
    memo: HashMap<Key, Option<String>>,
}

impl Request {
    pub(crate) fn new(budget: Budget) -> Self {
        let cwd = budget
            .cwd()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        Self {
            budget: budget.with_cwd(Some(&cwd)),
            cwd,
            memo: HashMap::new(),
        }
    }
}

pub(crate) fn stdout(command: &Command, program: &str, args: &[&str]) -> Option<String> {
    query(command, program, args, false)
}

pub(crate) fn combined(command: &Command, program: &str, args: &[&str]) -> Option<String> {
    query(command, program, args, true)
}

// Only the metadata entry points used by the rules belong here. A failed
// command's remaining arguments never become a discovery invocation.
fn metadata_entry(program: &str, args: &[&str]) -> bool {
    let name = std::path::Path::new(program)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    match (name, args) {
        (_, ["--help"]) => true,
        ("git", ["branch", "-a", "--no-color", "--no-column"]) => true,
        ("gem", ["help", "commands"]) => true,
        ("go" | "docker" | "yum", []) => true,
        ("brew", ["--prefix"]) => true,
        ("npm", ["run-script"]) => true,
        ("gulp", ["--tasks-simple"]) => true,
        ("gradle" | "gradlew", ["tasks"]) => true,
        ("goenv" | "nodenv" | "pyenv" | "rbenv", ["commands"]) => true,
        ("ifconfig", ["-a"]) => true,
        ("unzip", ["-Z1", file]) => !file.is_empty() && !file.starts_with('-'),
        ("lsof", ["-i", port]) => port
            .strip_prefix(':')
            .is_some_and(|port| port.parse::<u16>().is_ok()),
        ("apt-file", ["search", "--regexp", pattern]) => {
            pattern.starts_with("/bin/") && pattern.ends_with('$')
        }
        ("pkgfile", ["-b", "-v", executable]) => {
            !executable.is_empty() && !executable.starts_with('-')
        }
        _ => false,
    }
}

fn query(command: &Command, program: &str, args: &[&str], combined: bool) -> Option<String> {
    let ctx = command.ctx();
    if !metadata_entry(program, args) {
        crate::logs::debug(format_args!(
            "Not probing {program}: not a legacy metadata entry point"
        ));
        return None;
    }
    let path = ctx
        .which(program)
        .filter(|path| probe::is_trusted_location(path))?;
    let identity = native::app_identity(&path);
    // Git's built-in branch listing is the existing audited local read.
    // Other compatibility probes require the same explicit permission as help.
    let audited = program == "git" && args == ["branch", "-a", "--no-color", "--no-column"];
    if !audited
        && !ctx.settings.trusted_help.iter().any(|entry| {
            entry == "*"
                || entry == program
                || identity.as_ref().is_some_and(|identity| entry == identity)
        })
    {
        crate::logs::debug(format_args!(
            "Not probing {program}: trusted_help does not authorize it"
        ));
        return None;
    }
    let mut discovery = command.discovery.borrow_mut();
    let request = discovery
        .get_or_insert_with(|| Request::new(Budget::for_timeout(ctx.settings.probe_timeout)));
    for name in [program, path.to_str()?] {
        if let Some(why) = crate::workspace::untrusted_project(
            name,
            &request.cwd,
            &ctx.settings.trusted_workspaces,
        ) {
            crate::logs::debug(format_args!("Not probing {program}: {why}"));
            return None;
        }
    }
    let key = (
        path.clone(),
        args.iter().map(|arg| (*arg).to_owned()).collect(),
        combined,
    );
    if let Some(answer) = request.memo.get(&key) {
        return answer.clone();
    }
    let mut env = native::help_env();
    if audited {
        let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
        env.extend([
            ("GIT_OPTIONAL_LOCKS".into(), Some("0".into())),
            ("GIT_TERMINAL_PROMPT".into(), Some("0".into())),
            ("GIT_CONFIG_NOSYSTEM".into(), Some("1".into())),
            ("GIT_CONFIG_GLOBAL".into(), Some(null.into())),
            ("GIT_CONFIG_COUNT".into(), Some("1".into())),
            ("GIT_CONFIG_KEY_0".into(), Some("core.fsmonitor".into())),
            ("GIT_CONFIG_VALUE_0".into(), Some("false".into())),
            ("GIT_CONFIG_PARAMETERS".into(), None),
        ]);
    }
    env.extend([
        ("HOMEBREW_NO_AUTO_UPDATE".into(), Some("1".into())),
        ("HOMEBREW_NO_ANALYTICS".into(), Some("1".into())),
        ("npm_config_ignore_scripts".into(), Some("true".into())),
        ("npm_config_offline".into(), Some("true".into())),
    ]);
    let result = probe::run(
        &Probe {
            program: &path,
            args: args.iter().map(std::ffi::OsString::from).collect(),
            env,
            capture: if combined {
                Capture::Combined
            } else {
                Capture::Stdout
            },
        },
        &mut request.budget,
    );
    let answer = match result {
        Ok(output)
            if !output.truncated
                && (output.status == Some(0)
                    || program == "go" && args.is_empty() && output.status == Some(2)) =>
        {
            String::from_utf8(output.data).ok().filter(|text| {
                text.lines()
                    .take(request.budget.max_candidates.saturating_add(1))
                    .count()
                    <= request.budget.max_candidates
            })
        }
        Ok(_) => None,
        Err(error) => {
            crate::logs::debug(format_args!("Legacy metadata probe of {program}: {error}"));
            None
        }
    };
    request.memo.insert(key, answer.clone());
    answer
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::engine::native::tests::Dir;
    use crate::{
        engine::{self, FailureContext},
        settings::Settings,
        shells::Shell,
        types::Context,
    };
    use std::{
        fs,
        time::{Duration, Instant},
    };

    fn context(path: &std::path::Path, program: &str, trusted: bool) -> Context {
        Context::new(
            Settings {
                trusted_help: if trusted {
                    vec![program.into()]
                } else {
                    Vec::new()
                },
                ..Settings::default()
            },
            Shell::Bash,
            "typo".into(),
        )
        .with_which(program, Some(path.to_str().unwrap()))
    }

    fn request(command: &Command, cwd: &std::path::Path) {
        *command.discovery.borrow_mut() = Some(Request::new(
            Budget::new(Duration::from_secs(3), Duration::from_secs(1), 8).with_cwd(Some(cwd)),
        ));
    }

    #[test]
    fn trust_and_fixed_metadata_entry_points_are_required_before_spawning() {
        let dir = Dir::new("legacy-trust");
        let path = dir.script(
            "bin/tool",
            "#!/bin/sh\ntouch \"$(dirname \"$0\")/ran\"\nprintf 'build\\n'\n",
        );
        let denied = context(&path, "tool", false);
        let command = Command::new("tool biuld", None, &denied);
        assert_eq!(stdout(&command, "tool", &["--help"]), None);
        assert!(command.discovery.borrow().is_none());
        let allowed = context(&path, "tool", true);
        let command = Command::new("tool biuld", None, &allowed);
        assert_eq!(stdout(&command, "tool", &["biuld"]), None);
        assert!(!dir.0.join("bin/ran").exists());
        assert_eq!(
            stdout(&command, "tool", &["--help"]).as_deref(),
            Some("build\n")
        );
    }

    #[test]
    fn requests_preserve_cwd_offline_controls_and_do_not_reuse_other_projects() {
        let dir = Dir::new("legacy-projects");
        let path = dir.script(
            "bin/tool",
            r#"#!/bin/sh
[ "$*" = --help ] || exit 99
[ "$HTTP_PROXY" = http://127.0.0.1:9 ] || exit 98
[ "$CLOUDSDK_AUTH_DISABLE_CREDENTIALS" = true ] || exit 97
printf 'probe\n' >> "$(dirname "$0")/probes"
cat "$PWD/words"
"#,
        );
        let ctx = context(&path, "tool", true);
        for name in ["west", "east"] {
            let cwd = dir.0.join(name);
            fs::create_dir(&cwd).unwrap();
            fs::write(cwd.join("words"), format!("{name}\n")).unwrap();
            let command = Command::new("tool typo", None, &ctx);
            request(&command, &cwd);
            for _ in 0..2 {
                assert_eq!(
                    stdout(&command, "tool", &["--help"]),
                    Some(format!("{name}\n"))
                );
            }
            assert_eq!(
                command
                    .discovery
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .budget
                    .spawned(),
                1
            );
        }
        assert_eq!(
            fs::read_to_string(dir.0.join("bin/probes"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn project_trust_is_checked_in_the_failure_directory() {
        let dir = Dir::new("legacy-workspace");
        let path = dir.script(
            "bin/gulp",
            "#!/bin/sh\ntouch \"$(dirname \"$0\")/ran\"\nprintf 'build\\n'\n",
        );
        let cwd = dir.0.join("project");
        fs::create_dir(&cwd).unwrap();
        fs::write(cwd.join("gulpfile.js"), "throw new Error('must not load')").unwrap();
        let mut ctx = context(&path, "gulp", true);
        let command = Command::new("gulp biuld", None, &ctx);
        request(&command, &cwd);
        assert_eq!(stdout(&command, "gulp", &["--tasks-simple"]), None);
        assert!(!dir.0.join("bin/ran").exists());
        drop(command);
        ctx.settings.trusted_workspaces = vec![cwd.to_str().unwrap().into()];
        let command = Command::new("gulp biuld", None, &ctx);
        request(&command, &cwd);
        assert_eq!(
            stdout(&command, "gulp", &["--tasks-simple"]).as_deref(),
            Some("build\n")
        );
    }

    #[test]
    fn limits_and_failures_never_yield_partial_metadata() {
        for (name, body) in [
            ("failed", "printf build; exit 1"),
            ("invalid", "printf '\\377'"),
            ("large", "printf 'too much output for this budget'"),
            ("candidates", "printf 'a\\nb\\nc\\nd\\n'"),
            ("slow", "sleep 2; printf build"),
        ] {
            let dir = Dir::new(name);
            let path = dir.script("tool", &format!("#!/bin/sh\n{body}\n"));
            let ctx = context(&path, "tool", true);
            let mut budget = Budget::new(Duration::from_secs(1), Duration::from_millis(50), 2);
            budget.max_output = 16;
            budget.max_candidates = 3;
            let command = Command::new("tool typo", None, &ctx);
            *command.discovery.borrow_mut() = Some(Request::new(budget));
            let started = Instant::now();
            assert_eq!(stdout(&command, "tool", &["--help"]), None, "{name}");
            assert_eq!(stdout(&command, "tool", &["--help"]), None);
            assert!(started.elapsed() < Duration::from_secs(1));
            assert_eq!(
                command
                    .discovery
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .budget
                    .spawned(),
                1
            );
        }
    }

    #[test]
    fn go_help_reads_stderr_without_replaying_the_failed_arguments() {
        let dir = Dir::new("legacy-go");
        let path = dir.script("go", "#!/bin/sh\n[ \"$#\" = 0 ] || exit 99\nprintf 'The commands are:\\n\\nbuild\\n' >&2\nexit 2\n");
        let ctx = context(&path, "go", true);
        let command = Command::new("go biuld", Some("unknown command"), &ctx);
        let rule = crate::rules::RULES
            .iter()
            .find(|rule| rule.name == "go_unknown_command")
            .unwrap();
        assert_eq!((rule.get_new_command)(&command), ["go build"]);
    }

    #[test]
    fn compatibility_probes_cannot_restart_an_exhausted_engine_budget() {
        let dir = Dir::new("legacy-shared-budget");
        let path = dir.script("tool", "#!/bin/sh\ntouch \"$(dirname \"$0\")/ran\"\n");
        let mut ctx = context(&path, "tool", true)
            .with_path_listing(&["tool"])
            .with_aliases(&[])
            .with_history::<&str>(&[]);
        ctx.settings.probe_timeout = 0.0;
        ctx.settings.disabled_sources =
            ["native", "help", "history", "man", "filesystem", "stderr"]
                .map(String::from)
                .into();
        let failed = FailureContext {
            source: "tool typo".into(),
            ..FailureContext::default()
        };
        let report = engine::correct(&failed, &ctx);
        assert_eq!(report.probes, 0);
        let command = Command::new(&failed.source, None, &ctx);
        *command.discovery.borrow_mut() = Some(Request::new(
            report.discovery_budget.lock().unwrap().take().unwrap(),
        ));
        assert_eq!(stdout(&command, "tool", &["--help"]), None);
        assert!(!dir.0.join("ran").exists());
    }
}
