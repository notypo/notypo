#[cfg(windows)]
pub(crate) mod windows;

#[cfg(unix)]
pub(crate) fn parent_id() -> u32 {
    std::os::unix::process::parent_id()
}

#[cfg(windows)]
pub(crate) fn parent_id() -> u32 {
    windows::process_entry(std::process::id()).map_or(0, |(_, parent)| parent)
}
