//! Shared PTY setup for the shell logger and native ZLE completion probes.

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::Command;

pub(crate) fn open_pty(size: &libc::winsize) -> io::Result<(File, File)> {
    let mut master = -1;
    let mut slave = -1;
    let mut size = *size;
    // SAFETY: valid fd output pointers and window size; unused name and
    // termios pointers are null as permitted by openpty.
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut size,
        )
    } == -1
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty returned distinct, newly owned descriptors.
    let (master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
    for file in [&master, &slave] {
        // SAFETY: the File owns a valid descriptor. In particular, the
        // master must not leak into the child and prevent terminal EOF.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((master, slave))
}

pub(crate) fn controlling_terminal(process: &mut Command) {
    // SAFETY: only async-signal-safe calls run between fork and exec. The
    // caller must attach the PTY slave to the child's stdin before spawn.
    unsafe {
        process.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}
