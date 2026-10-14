//! Bounded discovery subprocesses, such as native completers.
//!
//! Programs and arguments form an explicit argument vector. Shell adapters
//! use fixed drivers with candidate text passed as literal arguments.
//! stdin is closed (ZLE has a private terminal), captured output is capped,
//! and a probe that outlives its
//! timeout is killed together with every process it started. A shared
//! [`Budget`] bounds the number of probes and their total running time.

use std::ffi::OsString;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Limits shared by every probe made while correcting one command.
#[derive(Debug)]
pub struct Budget {
    deadline: Instant,
    per_probe: Duration,
    subprocesses_left: usize,
    spawned: usize,
    /// The failed command's directory, shared by all discovery providers.
    cwd: Option<PathBuf>,
    /// Bytes read from one probe before its output counts as truncated.
    pub max_output: usize,
    /// Results kept from one probe.
    pub max_candidates: usize,
}

impl Budget {
    pub fn new(total: Duration, per_probe: Duration, subprocesses: usize) -> Budget {
        Budget {
            deadline: Instant::now() + total,
            per_probe,
            subprocesses_left: subprocesses,
            spawned: 0,
            cwd: None,
            max_output: 1024 * 1024,
            max_candidates: 5000,
        }
    }

    /// The default limits: `probe_timeout` per probe, three times that overall.
    pub fn for_timeout(probe_timeout: f64) -> Budget {
        let per_probe = Duration::from_secs_f64(probe_timeout.clamp(0.0, 60.0));
        Budget::new(per_probe * 3, per_probe, 16)
    }

    pub fn spawned(&self) -> usize {
        self.spawned
    }

    pub(crate) fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    /// Set the default directory for this request without changing the
    /// calling process's cwd (which would race other library users).
    pub fn with_cwd(mut self, cwd: Option<&Path>) -> Self {
        self.cwd = cwd.map(Path::to_owned);
        self
    }

    pub fn exhausted(&self) -> bool {
        self.subprocesses_left == 0 || Instant::now() >= self.deadline
    }

    /// Claims one probe; returns how long it may run.
    fn reserve(&mut self) -> Result<Duration, ProbeError> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if self.subprocesses_left == 0 || remaining.is_zero() {
            return Err(ProbeError::BudgetExhausted);
        }
        self.subprocesses_left -= 1;
        self.spawned += 1;
        Ok(self.per_probe.min(remaining))
    }
}

/// Where a probe's result is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capture {
    Stdout,
    /// Separate stdout and stderr, with one shared output-size limit.
    Separate,
    /// stdout and stderr through one pipe (help text goes to either).
    Combined,
    /// An extra inherited descriptor, such as argcomplete's fd 8. stdout and
    /// stderr are discarded.
    Descriptor(i32),
}

#[derive(Debug)]
pub struct Probe<'a> {
    pub program: &'a Path,
    pub args: Vec<OsString>,
    /// Variables to set (`Some`) or remove (`None`).
    pub env: Vec<(OsString, Option<OsString>)>,
    pub capture: Capture,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ProbeOutput {
    pub status: Option<i32>,
    pub data: Vec<u8>,
    /// Populated only for `Capture::Separate`.
    pub stderr: Vec<u8>,
    pub truncated: bool,
}

#[derive(Debug)]
pub enum ProbeError {
    BudgetExhausted,
    Spawn(io::Error),
    TimedOut,
    Unsupported(&'static str),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::BudgetExhausted => f.write_str("probe budget exhausted"),
            ProbeError::Spawn(error) => write!(f, "could not start: {error}"),
            ProbeError::TimedOut => f.write_str("timed out"),
            ProbeError::Unsupported(why) => f.write_str(why),
        }
    }
}

/// Only absolute executables are probed automatically. A relative path, or a
/// program found through a relative `$PATH` entry, would run whatever the
/// current directory happens to contain.
pub fn is_trusted_location(path: &Path) -> bool {
    path.is_absolute()
}

pub fn run(probe: &Probe, budget: &mut Budget) -> Result<ProbeOutput, ProbeError> {
    run_inner(probe, budget, false, None)
}

/// [`run`] in `dir`, for apps whose argument parsing can create files
/// relative to the working directory.
pub fn run_in(probe: &Probe, dir: &Path, budget: &mut Budget) -> Result<ProbeOutput, ProbeError> {
    run_inner(probe, budget, false, Some(dir))
}

/// A private controlling terminal for native ZLE widgets. No input is sent
/// to it: `vared` runs a completion widget from its line-init hook, and the
/// completion function itself receives closed stdin. Answers use the same
/// bounded capture pipe as every other probe.
pub(crate) fn run_completion_terminal(
    probe: &Probe,
    budget: &mut Budget,
) -> Result<ProbeOutput, ProbeError> {
    #[cfg(unix)]
    {
        run_inner(probe, budget, true, None)
    }
    #[cfg(not(unix))]
    {
        let _ = (probe, budget);
        Err(ProbeError::Unsupported(
            "ZLE completion requires a Unix terminal",
        ))
    }
}

fn run_inner(
    probe: &Probe,
    budget: &mut Budget,
    completion_terminal: bool,
    dir: Option<&Path>,
) -> Result<ProbeOutput, ProbeError> {
    #[cfg(windows)]
    if matches!(probe.capture, Capture::Descriptor(_)) {
        return Err(ProbeError::Unsupported(
            "descriptor-based completion protocols need Unix file descriptors",
        ));
    }
    let timeout = budget.reserve()?;
    let (reader, writer) = io::pipe().map_err(ProbeError::Spawn)?;
    let mut process = Process::new(probe.program);
    process.args(&probe.args).stdin(Stdio::null());
    if let Some(dir) = dir.or(budget.cwd.as_deref()) {
        process.current_dir(dir);
        #[cfg(unix)]
        let directory = std::fs::canonicalize(dir).or_else(|_| std::path::absolute(dir));
        #[cfg(not(unix))]
        let directory = std::path::absolute(dir);
        if let Ok(absolute) = directory {
            process.env("PWD", absolute);
        }
    }
    for (key, value) in &probe.env {
        match value {
            Some(value) => process.env(key, value),
            None => process.env_remove(key),
        };
    }
    if let Some(path) = super::native::startup_stubs(probe.program, &probe.env) {
        process.env("PATH", path);
    }
    // The child inherits the extra descriptor at fork; the parent's copy must
    // close right after the spawn so the reader sees EOF.
    let mut extra_writer = None;
    let mut stderr_reader = None;
    match probe.capture {
        Capture::Stdout => {
            process.stdout(writer).stderr(Stdio::null());
        }
        Capture::Separate => {
            let (reader, stderr_writer) = io::pipe().map_err(ProbeError::Spawn)?;
            stderr_reader = Some(reader);
            process.stdout(writer).stderr(stderr_writer);
        }
        Capture::Combined => {
            process
                .stdout(writer.try_clone().map_err(ProbeError::Spawn)?)
                .stderr(writer);
        }
        Capture::Descriptor(target) => {
            process.stdout(Stdio::null()).stderr(Stdio::null());
            #[cfg(unix)]
            {
                use std::os::fd::AsRawFd;
                let fd = writer.as_raw_fd();
                // SAFETY: dup2 and fcntl are async-signal-safe, and the
                // closure only touches descriptors owned by the child.
                unsafe {
                    process.pre_exec(move || {
                        if fd == target {
                            let flags = libc::fcntl(fd, libc::F_GETFD);
                            if flags < 0
                                || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                            {
                                return Err(io::Error::last_os_error());
                            }
                        } else if libc::dup2(fd, target) < 0 {
                            return Err(io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
            }
            #[cfg(windows)]
            let _ = target;
            extra_writer = Some(writer);
        }
    }
    #[cfg(unix)]
    let terminal = if completion_terminal {
        let size = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let (master, slave) = crate::platform::unix::open_pty(&size).map_err(ProbeError::Spawn)?;
        process.stdin(slave);
        crate::platform::unix::controlling_terminal(&mut process);
        Some(master)
    } else {
        crate::platform::unix::new_session(&mut process);
        None
    };
    #[cfg(not(unix))]
    let _ = completion_terminal;
    #[cfg(windows)]
    process.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    let spawned = process.spawn();
    drop(extra_writer);
    let mut child = spawned.map_err(ProbeError::Spawn)?;
    #[cfg(windows)]
    let job = match crate::platform::windows::Job::start(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ProbeError::Spawn(error));
        }
    };
    drop(process);

    #[cfg(unix)]
    if let Some(mut master) = terminal {
        // ZLE writes screen refreshes to its tty, independently of the
        // completion answer pipe. Drain them so rendering cannot hang it.
        thread::spawn(move || {
            let _ = io::copy(&mut master, &mut io::sink());
        });
    }

    let limit = budget.max_output;
    let read = |mut reader: io::PipeReader| {
        let (done, finished) = mpsc::channel();
        thread::spawn(move || {
            let mut data = Vec::new();
            let _ = (&mut reader).take(limit as u64 + 1).read_to_end(&mut data);
            // Keep draining so a chatty child cannot block on a full pipe.
            let _ = io::copy(&mut reader, &mut io::sink());
            let _ = done.send(data);
        });
        finished
    };
    let finished = read(reader);
    let stderr_finished = stderr_reader.map(read);
    let deadline = Instant::now() + timeout;
    let data = finished.recv_timeout(timeout).ok();
    let stderr = match stderr_finished {
        Some(finished) => finished
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok(),
        None => Some(Vec::new()),
    };
    let status = data.as_ref().zip(stderr.as_ref()).and_then(|_| {
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Err(_) => break None,
                Ok(None) if Instant::now() >= deadline => break None,
                Ok(None) => thread::sleep(Duration::from_micros(200)),
            }
        }
    });
    if status.is_none() {
        #[cfg(unix)]
        // SAFETY: signalling the process group created for this probe,
        // whose leader has not been reaped yet.
        unsafe {
            libc::killpg(child.id() as libc::pid_t, libc::SIGKILL)
        };
        let _ = child.wait();
    }
    #[cfg(windows)]
    job.terminate();
    match (data, stderr, status) {
        (Some(mut data), Some(mut stderr), Some(status)) => {
            let truncated = data.len().saturating_add(stderr.len()) > limit;
            data.truncate(limit);
            stderr.truncate(limit.saturating_sub(data.len()));
            Ok(ProbeOutput {
                status: status.code(),
                data,
                stderr,
                truncated,
            })
        }
        _ => Err(ProbeError::TimedOut),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sh(script: &str) -> (PathBuf, Vec<OsString>) {
        (
            PathBuf::from("/bin/sh"),
            vec!["-c".into(), script.into(), "probe".into()],
        )
    }

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(10), Duration::from_secs(5), 8)
    }

    /// A probe that asked for a passphrase (borg, ssh, gpg open /dev/tty)
    /// would print on the user's terminal and stall there. Run from a
    /// terminal, this proves probes have none.
    #[cfg(unix)]
    #[test]
    fn probes_have_no_controlling_terminal() {
        let (program, args) = sh("if (: </dev/tty) 2>/dev/null; then echo tty; else echo none; fi");
        let probe = Probe {
            program: &program,
            args,
            env: Vec::new(),
            capture: Capture::Stdout,
        };
        let output = run(&probe, &mut budget()).unwrap();
        assert_eq!(output.data, b"none\n");
    }

    #[test]
    fn captures_stdout_with_explicit_arguments_and_env() {
        let (program, mut args) = sh("printf '%s|%s' \"$1\" \"$NOTYPO_PROBE\"");
        args.push("$(touch injected) ; x".into());
        let probe = Probe {
            program: &program,
            args,
            env: vec![("NOTYPO_PROBE".into(), Some("set".into()))],
            capture: Capture::Stdout,
        };
        let output = run(&probe, &mut budget()).unwrap();
        assert_eq!(output.status, Some(0));
        assert_eq!(output.data, b"$(touch injected) ; x|set");
        assert!(!Path::new("injected").exists());
    }

    #[test]
    fn captures_an_extra_descriptor() {
        let (program, args) = sh("echo out; echo err >&2; printf 'a\\vb' >&8");
        let probe = Probe {
            program: &program,
            args,
            env: Vec::new(),
            capture: Capture::Descriptor(8),
        };
        let output = run(&probe, &mut budget()).unwrap();
        assert_eq!(output.data, b"a\x0bb");
    }

    #[test]
    fn separate_streams_share_one_size_limit_and_are_drained_concurrently() {
        let (program, args) = sh("printf stdout; printf stderr >&2");
        let probe = Probe {
            program: &program,
            args,
            env: Vec::new(),
            capture: Capture::Separate,
        };
        let output = run(&probe, &mut budget()).unwrap();
        assert_eq!(output.data, b"stdout");
        assert_eq!(output.stderr, b"stderr");
        assert!(!output.truncated);
        let mut limits = budget();
        limits.max_output = 8;
        let output = run(&probe, &mut limits).unwrap();
        assert_eq!(output.data.len() + output.stderr.len(), 8);
        assert!(output.truncated);
        let (program, args) = sh(
            "i=0; while [ $i -lt 10000 ]; do printf '1234567890\\n' >&2; i=$((i+1)); done; printf done",
        );
        let output = run(
            &Probe {
                program: &program,
                args,
                env: Vec::new(),
                capture: Capture::Separate,
            },
            &mut limits,
        )
        .unwrap();
        assert_eq!(output.data, b"done");
        assert!(output.truncated);
    }

    #[test]
    fn times_out_kills_the_tree_and_respects_the_budget() {
        let (program, args) = sh("sleep 5 & sleep 5");
        let probe = Probe {
            program: &program,
            args,
            env: Vec::new(),
            capture: Capture::Stdout,
        };
        let mut budget = Budget::new(Duration::from_secs(10), Duration::from_millis(200), 1);
        let started = Instant::now();
        assert!(matches!(
            run(&probe, &mut budget),
            Err(ProbeError::TimedOut)
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(matches!(
            run(&probe, &mut budget),
            Err(ProbeError::BudgetExhausted)
        ));
        assert_eq!(budget.spawned(), 1);
    }

    #[test]
    fn caps_output() {
        let (program, args) = sh("yes x | head -c 100000");
        let probe = Probe {
            program: &program,
            args,
            env: Vec::new(),
            capture: Capture::Stdout,
        };
        let mut budget = budget();
        budget.max_output = 1000;
        let output = run(&probe, &mut budget).unwrap();
        assert_eq!(output.data.len(), 1000);
        assert!(output.truncated);
    }

    #[test]
    fn trust_requires_an_absolute_location() {
        assert!(is_trusted_location(Path::new("/usr/bin/aws")));
        assert!(!is_trusted_location(Path::new("./aws")));
        assert!(!is_trusted_location(Path::new("bin/aws")));
    }
}
