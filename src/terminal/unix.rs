//! Terminal state shared by command selection and the shell logger.

pub(crate) fn init_output() {}

/// Restores the caller's terminal attributes on every normal exit path.
pub(crate) struct RawMode(Option<libc::termios>);

impl RawMode {
    pub(crate) fn enter() -> Self {
        let mut saved = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: fd 0 and a correctly sized output buffer.
        if unsafe { libc::tcgetattr(0, saved.as_mut_ptr()) } != 0 {
            return Self(None);
        }
        // SAFETY: tcgetattr initialized the structure.
        let saved = unsafe { saved.assume_init() };
        let mut raw = saved;
        // SAFETY: both termios structures are initialized.
        unsafe {
            libc::cfmakeraw(&mut raw);
            if libc::tcsetattr(0, libc::TCSANOW, &raw) != 0 {
                return Self(None);
            }
        }
        Self(Some(saved))
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        if let Some(saved) = &self.0 {
            // SAFETY: restoring the valid structure read by tcgetattr.
            unsafe { libc::tcsetattr(0, libc::TCSADRAIN, saved) };
        }
    }
}

pub(crate) fn size() -> libc::winsize {
    let mut size = std::mem::MaybeUninit::<libc::winsize>::uninit();
    for fd in [libc::STDOUT_FILENO, libc::STDERR_FILENO, libc::STDIN_FILENO] {
        // SAFETY: the output buffer has the size expected by TIOCGWINSZ.
        if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, size.as_mut_ptr()) } == 0 {
            // SAFETY: ioctl initialized size.
            let size = unsafe { size.assume_init() };
            if size.ws_row > 0 && size.ws_col > 0 {
                return size;
            }
        }
    }
    libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}
