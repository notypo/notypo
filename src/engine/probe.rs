//! Bounded discovery subprocesses, such as native completers.
//!
//! Probes never run through a shell: the program and its arguments form an
//! explicit argument vector, so candidate text cannot become shell syntax.
//! stdin is closed, captured output is capped, and a probe that outlives its
//! timeout is killed together with every process it started. A shared
//! [`Budget`] bounds the number of probes and their total running time.

use std::ffi::OsString;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::Path;
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
    #[cfg(windows)]
    if matches!(probe.capture, Capture::Descriptor(_)) {
        return Err(ProbeError::Unsupported(
            "descriptor-based completion protocols need Unix file descriptors",
        ));
    }
    let timeout = budget.reserve()?;
    let (mut reader, writer) = io::pipe().map_err(ProbeError::Spawn)?;
    let mut process = Process::new(probe.program);
    process.args(&probe.args).stdin(Stdio::null());
    for (key, value) in &probe.env {
        match value {
            Some(value) => process.env(key, value),
            None => process.env_remove(key),
        };
    }
    // The child inherits the extra descriptor at fork; the parent's copy must
    // close right after the spawn so the reader sees EOF.
    let mut extra_writer = None;
    match probe.capture {
        Capture::Stdout => {
            process.stdout(writer).stderr(Stdio::null());
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
    process.process_group(0);
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

    let limit = budget.max_output;
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let mut data = Vec::new();
        let _ = (&mut reader).take(limit as u64 + 1).read_to_end(&mut data);
        // Keep draining so a chatty child cannot block on a full pipe.
        let _ = io::copy(&mut reader, &mut io::sink());
        let _ = done.send(data);
    });
    let deadline = Instant::now() + timeout;
    let data = finished.recv_timeout(timeout).ok();
    let status = data.as_ref().and_then(|_| {
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
    match (data, status) {
        (Some(mut data), Some(status)) => {
            let truncated = data.len() > limit;
            data.truncate(limit);
            Ok(ProbeOutput {
                status: status.code(),
                data,
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
