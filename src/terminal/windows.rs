//! Console modes are restored when interactive selection finishes.

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Console::*;

pub(crate) struct RawMode(Option<(HANDLE, u32)>);

impl RawMode {
    pub(crate) fn enter() -> Self {
        // SAFETY: standard handles are borrowed; mode is a valid output buffer.
        unsafe {
            let input = GetStdHandle(STD_INPUT_HANDLE);
            let mut mode = 0;
            if GetConsoleMode(input, &mut mode) == 0 {
                return Self(None);
            }
            let raw = (mode & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT))
                | ENABLE_VIRTUAL_TERMINAL_INPUT;
            if SetConsoleMode(input, raw) == 0 {
                return Self(None);
            }
            Self(Some((input, mode)))
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        if let Some((handle, mode)) = self.0 {
            // SAFETY: restoring the mode of a borrowed console handle.
            unsafe { SetConsoleMode(handle, mode) };
        }
    }
}

pub(crate) fn init_output() {
    // SAFETY: handles are borrowed, and each mode output is initialized.
    unsafe {
        for kind in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let handle = GetStdHandle(kind);
            let mut mode = 0;
            if GetConsoleMode(handle, &mut mode) != 0 {
                SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}

pub(crate) struct WindowSize {
    pub ws_col: u16,
}

pub(crate) fn size() -> WindowSize {
    let mut info = std::mem::MaybeUninit::<CONSOLE_SCREEN_BUFFER_INFO>::uninit();
    // SAFETY: info has the layout and size required by the console API.
    if unsafe { GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), info.as_mut_ptr()) }
        != 0
    {
        // SAFETY: the API initialized info.
        let info = unsafe { info.assume_init() };
        return WindowSize {
            ws_col: (i32::from(info.srWindow.Right) - i32::from(info.srWindow.Left) + 1)
                .clamp(1, 4096) as u16,
        };
    }
    WindowSize { ws_col: 80 }
}
