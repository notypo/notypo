//! Getting the failed command's output, port of `thefuck/output_readers`.
//!
//! Captured output comes from the external shell logger or instant-mode log.
//! Otherwise Python always runs `/bin/sh -c <script>`,
//! which costs ~3 ms of shell startup on top of the command. We skip the
//! shell when it provably wouldn't matter:
//!   * a plain command (no quotes, globs, expansions, redirections, ...)
//!     whose first word is in `$PATH` is spawned directly;
//!   * a plain command whose first word is neither a builtin nor in `$PATH`
//!     gets the shell's "command not found" message without spawning;
//!   * `cd <dir>` (the `cd_mkdir` case) is answered with a `stat`.
//!
//! Everything else goes through `/bin/sh -c`, exactly like Python.

use crate::logs;
use crate::settings::Settings;
#[cfg(unix)]
use crate::utils;
use std::ffi::OsStr;
#[cfg(unix)]
use std::fs;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command as Process, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use std::{env, thread};

mod captured;
#[cfg(unix)]
pub(crate) use captured::LOG_SIZE;

#[cfg(unix)]
const SH: &str = "/bin/sh";
#[cfg(windows)]
const SH: &str = "cmd.exe";

/// How a script gets its output.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan<'a> {
    /// Output known without running anything.
    Known(String),
    /// Spawn the program directly with these arguments.
    Direct {
        program: PathBuf,
        args: Vec<&'a str>,
    },
    /// `/bin/sh -c script`.
    Shell,
}

/// sh builtins and reserved words: these always need a real shell.
#[cfg(unix)]
const SH_BUILTINS: &[&str] = &[
    ".",
    ":",
    "!",
    "{",
    "}",
    "[[",
    "]]",
    "alias",
    "bg",
    "bind",
    "break",
    "builtin",
    "caller",
    "case",
    "cd",
    "command",
    "compgen",
    "complete",
    "continue",
    "coproc",
    "declare",
    "dirs",
    "disown",
    "do",
    "done",
    "echo",
    "elif",
    "else",
    "enable",
    "esac",
    "eval",
    "exec",
    "exit",
    "export",
    "false",
    "fc",
    "fg",
    "fi",
    "for",
    "function",
    "getopts",
    "hash",
    "help",
    "history",
    "if",
    "in",
    "jobs",
    "kill",
    "let",
    "local",
    "logout",
    "mapfile",
    "popd",
    "printf",
    "pushd",
    "pwd",
    "read",
    "readarray",
    "readonly",
    "return",
    "select",
    "set",
    "shift",
    "shopt",
    "source",
    "suspend",
    "test",
    "then",
    "time",
    "times",
    "trap",
    "true",
    "type",
    "typeset",
    "ulimit",
    "umask",
    "unalias",
    "unset",
    "until",
    "wait",
    "while",
];

/// Characters that can't change meaning anywhere in a shell word.
fn is_plain(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '-' | '_' | '.' | '/' | ',' | ':' | '+' | '@' | '%' | '=')
        || !c.is_ascii()
}

/// The words of `script` if it is a plain simple command.
pub fn plain_words(script: &str) -> Option<Vec<&str>> {
    if !script.chars().all(|c| is_plain(c) || c == ' ' || c == '\t') {
        return None;
    }
    let words: Vec<&str> = script
        .split([' ', '\t'])
        .filter(|w| !w.is_empty())
        .collect();
    (!words.is_empty()).then_some(words)
}

#[cfg(windows)]
pub fn plan(_script: &str) -> Plan<'_> {
    Plan::Shell
}

#[cfg(unix)]
pub fn plan(script: &str) -> Plan<'_> {
    let Some(words) = plain_words(script) else {
        return Plan::Shell;
    };
    let (first, args) = (words[0], words[1..].to_vec());
    if first.contains('=') {
        return Plan::Shell; // a variable assignment prefix
    }
    if SH_BUILTINS.contains(&first) {
        return match (first, args.as_slice()) {
            ("cd", [target]) => emulate_cd(target).map_or(Plan::Shell, Plan::Known),
            _ => Plan::Shell,
        };
    }
    if first.contains('/') {
        let program = PathBuf::from(first);
        // Missing or not executable: let sh word the error message.
        return if utils::is_executable_file(&program) {
            Plan::Direct { program, args }
        } else {
            Plan::Shell
        };
    }
    match utils::which(first) {
        Some(program) => Plan::Direct { program, args },
        None => Plan::Known(format!("{SH}: {first}: command not found\n")),
    }
}

/// What `sh -c 'cd <target>'` prints, when a `stat` can tell.
#[cfg(unix)]
fn emulate_cd(target: &str) -> Option<String> {
    if target.starts_with('-') || env::var("CDPATH").is_ok_and(|v| !v.is_empty()) {
        return None;
    }
    let error = |why: &str| Some(format!("{SH}: line 0: cd: {target}: {why}\n"));
    match fs::metadata(target) {
        Ok(m) if m.is_dir() => {
            if utils::access(target.as_ref(), libc::X_OK) {
                Some(String::new())
            } else {
                error("Permission denied")
            }
        }
        Ok(_) => error("Not a directory"),
        Err(e) => match e.raw_os_error() {
            Some(libc::ENOENT) => error("No such file or directory"),
            Some(libc::ENOTDIR) => error("Not a directory"),
            Some(libc::EACCES) => error("Permission denied"),
            _ => None,
        },
    }
}

/// Output already recorded for `script` by the shell logger or the instant
/// mode log. Never runs anything.
pub fn captured_output(
    script: &str,
    settings: &Settings,
) -> Option<(String, crate::engine::OutputOrigin)> {
    use crate::engine::OutputOrigin;
    if let Some(socket) = captured::logger_socket() {
        return captured::from_logger(script, &socket)
            .map(|text| (text, OutputOrigin::ShellLogger));
    }
    if settings.instant_mode {
        return captured::from_log(script).map(|text| (text, OutputOrigin::InstantModeLog));
    }
    None
}

/// `rerun.get_output(script, expanded)`; `None` when it timed out.
pub fn get_output(script: &str, expanded: &str, settings: &Settings) -> Option<String> {
    if let Some(socket) = captured::logger_socket() {
        return captured::from_logger(script, &socket);
    }
    if settings.instant_mode {
        return captured::from_log(script);
    }
    let started = Instant::now();
    let is_slow = crate::shlex::split(expanded)
        .ok()
        .and_then(|parts| parts.into_iter().next())
        .is_some_and(|first| settings.slow_commands.iter().any(|s| *s == *first));
    let timeout = Duration::from_secs_f64(
        if is_slow {
            settings.wait_slow_command
        } else {
            settings.wait_command
        }
        .max(0.0),
    );
    let plan = plan(expanded);
    let output = match &plan {
        Plan::Known(text) => Some(text.clone()),
        Plan::Direct { program, args } => run(program.as_os_str(), args, settings, timeout)
            .unwrap_or_else(|_| run_shell(expanded, settings, timeout).ok().flatten()),
        Plan::Shell => run_shell(expanded, settings, timeout).ok().flatten(),
    };
    logs::debug(format_args!(
        "Call: {script}; with env: {:?}; is slow: {is_slow}; via: {} took: {:?}",
        settings.env,
        match &plan {
            Plan::Known(_) => "no process".to_owned(),
            Plan::Direct { program, .. } => program.display().to_string(),
            Plan::Shell => format!("{SH} -c"),
        },
        started.elapsed()
    ));
    match &output {
        Some(text) => logs::debug(format_args!("Received output: {text}")),
        None => logs::debug("Execution timed out!"),
    }
    output
}

fn run_shell(
    script: &str,
    settings: &Settings,
    timeout: Duration,
) -> std::io::Result<Option<String>> {
    #[cfg(unix)]
    {
        run(SH.as_ref(), &["-c", script], settings, timeout)
    }
    #[cfg(windows)]
    {
        let shell = env::var_os("COMSPEC").unwrap_or_else(|| SH.into());
        run(&shell, &["/D", "/S", "/C", script], settings, timeout)
    }
}

/// Runs `program args` with stdin from /dev/null and stdout+stderr merged.
/// `Err` only if it couldn't be spawned; `Ok(None)` if it timed out (then
/// its whole process group is killed, like psutil's recursive kill).
pub fn run(
    program: &OsStr,
    args: &[&str],
    settings: &Settings,
    timeout: Duration,
) -> std::io::Result<Option<String>> {
    let (mut reader, writer) = std::io::pipe()?;
    let mut process = Process::new(program);
    #[cfg(unix)]
    process.args(args).process_group(0);
    #[cfg(windows)]
    {
        process.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
        if args.len() == 4
            && args[2] == "/C"
            && program
                .to_string_lossy()
                .rsplit(['/', '\\'])
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("cmd.exe"))
        {
            // cmd /S /C strips the outer quotes. Preserve the existing script's
            // quoting inside them, instead of escaping it as a single argv word.
            process.args(&args[..3]).raw_arg(format!("\"{}\"", args[3]));
        } else {
            process.args(args);
        }
    }
    process
        .stdin(Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(writer)
        .envs(settings.env.iter().map(|(k, v)| (k, v)));
    let mut child = process.spawn()?;
    #[cfg(windows)]
    let job = match crate::platform::windows::Job::start(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    // `process` holds our copies of the pipe's write end: close them so the
    // reader sees EOF once the child (and its children) are done.
    drop(process);

    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        let _ = done.send(buf);
    });
    let deadline = Instant::now() + timeout;
    let output = finished.recv_timeout(timeout).ok();
    // EOF usually means the child has exited; give it until the deadline.
    let exited = output.is_some()
        && loop {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => break true,
                Ok(None) if Instant::now() >= deadline => break false,
                Ok(None) => thread::sleep(Duration::from_micros(100)),
            }
        };
    if !exited {
        #[cfg(unix)]
        // SAFETY: signalling the process group we created.
        unsafe {
            libc::killpg(child.id() as libc::pid_t, libc::SIGKILL)
        };
        #[cfg(windows)]
        job.terminate();
        let _ = child.wait();
        return Ok(None);
    }
    Ok(output.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn plans() {
        assert_eq!(
            plan("gti status"),
            Plan::Known("/bin/sh: gti: command not found\n".into())
        );
        assert!(
            matches!(plan("ls -la /tmp"), Plan::Direct { args, .. } if args == ["-la", "/tmp"])
        );
        for script in [
            "ls | wc",
            "echo $HOME",
            "git commit -m \"x\"",
            "FOO=1 ls",
            "export X=1",
            "ls *.rs",
            "cd ~",
            "cd",
        ] {
            assert_eq!(plan(script), Plan::Shell, "{script}");
        }
        assert_eq!(
            plan("cd /definitely/not/here"),
            Plan::Known(
                "/bin/sh: line 0: cd: /definitely/not/here: No such file or directory\n".into()
            )
        );
        assert_eq!(plan("cd /"), Plan::Known(String::new()));
        assert_eq!(
            plan("cd /bin/sh"),
            Plan::Known("/bin/sh: line 0: cd: /bin/sh: Not a directory\n".into())
        );
    }

    #[test]
    fn merges_output_and_sets_env() {
        let s = Settings::default();
        assert_eq!(
            get_output("x", "sh -c 'echo out; echo err >&2'", &s).as_deref(),
            Some("out\nerr\n")
        );
        assert_eq!(get_output("x", "printenv LANG", &s).as_deref(), Some("C\n"));
        assert_eq!(
            get_output("x", "cat", &s).as_deref(),
            Some(""),
            "stdin must be /dev/null"
        );
    }

    #[test]
    fn times_out_and_kills_the_process_group() {
        let s = Settings {
            wait_command: 0.2,
            ..Settings::default()
        };
        let started = Instant::now();
        assert_eq!(get_output("x", "sh -c 'sleep 5 & sleep 5'", &s), None);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
