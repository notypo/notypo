//! Owned handles for process inspection and subprocess cancellation.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::process::Child;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use windows_sys::Win32::System::JobObjects::*;
use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

struct Handle(HANDLE);
impl Handle {
    fn new(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns and closes a valid handle exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

pub(crate) fn process_entry(pid: u32) -> Option<(String, u32)> {
    // SAFETY: the API takes scalar flags and returns a newly owned handle.
    let snapshot = Handle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }).ok()?;
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: entry is initialized with the required structure size.
    let mut found = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
    while found {
        if entry.th32ProcessID == pid {
            let len = entry
                .szExeFile
                .iter()
                .position(|ch| *ch == 0)
                .unwrap_or(entry.szExeFile.len());
            return Some((
                String::from_utf16_lossy(&entry.szExeFile[..len]),
                entry.th32ParentProcessID,
            ));
        }
        // SAFETY: the snapshot and entry remain valid.
        found = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
    }
    None
}

/// Closing the job cancels the entire subprocess tree. The child starts
/// suspended so it cannot create descendants before it belongs to the job.
pub(crate) struct Job(Handle);
impl Job {
    pub(crate) fn start(child: &Child) -> io::Result<Self> {
        // SAFETY: null attributes and name create an unnamed job.
        let handle = Handle::new(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: limits matches the indicated class and size. Both handles
        // remain valid throughout these API calls.
        unsafe {
            if SetInformationJobObject(
                handle.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(handle.0, child.as_raw_handle()) == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        resume(child.id())?;
        Ok(Self(handle))
    }
    pub(crate) fn terminate(&self) {
        // SAFETY: the job handle is valid and owned by this object.
        unsafe { TerminateJobObject(self.0.0, 1) };
    }
}

fn resume(pid: u32) -> io::Result<()> {
    // SAFETY: snapshot flags and pid are scalar inputs.
    let snapshot = Handle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    // SAFETY: entry is initialized with its required structure size.
    let mut found = unsafe { Thread32First(snapshot.0, &mut entry) } != 0;
    while found {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: OpenThread returns a new owned thread handle.
            let thread =
                Handle::new(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) })?;
            // SAFETY: this is the primary thread of our suspended child.
            if unsafe { ResumeThread(thread.0) } == u32::MAX {
                return Err(io::Error::last_os_error());
            }
            return Ok(());
        }
        // SAFETY: the snapshot and entry remain valid.
        found = unsafe { Thread32Next(snapshot.0, &mut entry) } != 0;
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "spawned process has no primary thread",
    ))
}
