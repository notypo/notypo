#[cfg(unix)]
#[path = "terminal/unix.rs"]
mod backend;
#[cfg(windows)]
#[path = "terminal/windows.rs"]
mod backend;

pub(crate) use backend::{RawMode, init_output, size};
