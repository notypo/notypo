//! Runs an interactive shell in a PTY and records its terminal output.
//!
//! File descriptors and terminal attributes are restored by their owners.
//! The log retains the newest megabyte, matching the instant reader's bound.

#![cfg(unix)]

use crate::output_readers::LOG_SIZE;
use crate::terminal::{self, RawMode};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::Path;
use std::process::{Child, Command, ExitStatus};

const CLEAN_SIZE: usize = 10 * 1024;

struct RollingLog {
    file: File,
    position: usize,
}

impl RollingLog {
    fn new(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.set_len(LOG_SIZE as u64)?;
        Ok(Self { file, position: 0 })
    }

    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        let bytes = &bytes[bytes.len().saturating_sub(LOG_SIZE)..];
        if self.position + bytes.len() > LOG_SIZE {
            let discard = CLEAN_SIZE
                .max(self.position + bytes.len() - LOG_SIZE)
                .min(self.position);
            let mut remaining = vec![0; self.position - discard];
            self.file.read_exact_at(&mut remaining, discard as u64)?;
            self.file.write_all_at(&remaining, 0)?;
            self.position = remaining.len();
            self.file
                .write_all_at(&vec![0; LOG_SIZE - self.position], self.position as u64)?;
        }
        self.file.write_all_at(bytes, self.position as u64)?;
        self.position += bytes.len();
        Ok(())
    }
}

/// Kills and reaps the PTY process if relaying terminates with an I/O error.
struct ShellProcess(Child);

impl Drop for ShellProcess {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            // SAFETY: the child created a session whose process group is its pid.
            unsafe { libc::killpg(self.0.id() as libc::pid_t, libc::SIGKILL) };
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

pub fn log_shell(path: &Path) -> io::Result<ExitStatus> {
    let shell = std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "SHELL is not set"))?;
    let mut log = RollingLog::new(path)?;
    let size = terminal::size();
    let (mut master, slave) = crate::platform::unix::open_pty(&size)?;
    let mut process = Command::new(shell);
    process
        .stdin(slave.try_clone()?)
        .stdout(slave.try_clone()?)
        .stderr(slave);
    crate::platform::unix::controlling_terminal(&mut process);
    let mut child = ShellProcess(process.spawn()?);
    drop(process);
    let _raw = RawMode::enter();
    relay(&mut master, &mut log, size)?;
    child.0.wait()
}

fn relay(master: &mut File, log: &mut RollingLog, mut size: libc::winsize) -> io::Result<()> {
    let mut input_open = true;
    let mut output = std::io::stdout().lock();
    let mut buf = [0; 4096];
    loop {
        let mut fds = [
            libc::pollfd {
                fd: master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if input_open { libc::STDIN_FILENO } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: fds is initialized and its length matches the supplied count.
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 100) } == -1 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        let resized = terminal::size();
        if (
            resized.ws_row,
            resized.ws_col,
            resized.ws_xpixel,
            resized.ws_ypixel,
        ) != (size.ws_row, size.ws_col, size.ws_xpixel, size.ws_ypixel)
        {
            // SAFETY: master is a valid PTY and resized is a valid winsize.
            unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &resized) };
            size = resized;
        }
        if fds[0].revents != 0 {
            match master.read(&mut buf) {
                Ok(0) => break,
                Ok(len) => {
                    log.append(&buf[..len])?;
                    output.write_all(&buf[..len])?;
                    output.flush()?;
                }
                // Linux PTYs report EIO after the last slave is closed.
                Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        if fds[1].revents != 0 {
            // SAFETY: buf is valid for its length. Read the fd directly to avoid
            // mixing buffered stdin with poll's readiness notifications.
            let len = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr().cast(), buf.len()) };
            if len > 0 {
                master.write_all(&buf[..len as usize])?;
            } else if len == 0 || fds[1].revents & libc::POLLNVAL != 0 {
                input_open = false;
                master.write_all(b"\x04")?;
            } else {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_log_retains_newest_bytes_and_zeroes_unused_tail() {
        let path = std::env::temp_dir().join(format!("notypo-rolling-log-{}", std::process::id()));
        let mut log = RollingLog::new(&path).unwrap();
        log.append(&vec![b'a'; LOG_SIZE - 2]).unwrap();
        log.append(b"new!").unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), LOG_SIZE);
        assert_eq!(&bytes[log.position - 4..log.position], b"new!");
        assert!(bytes[log.position..].iter().all(|byte| *byte == 0));
        log.append(&vec![b'b'; LOG_SIZE + 3]).unwrap();
        assert!(
            std::fs::read(&path)
                .unwrap()
                .iter()
                .all(|byte| *byte == b'b')
        );
        drop(log);
        std::fs::remove_file(path).unwrap();
    }
}
