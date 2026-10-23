//! One Vigie per user session (decision 0009): a named mutex says whether Vigie already runs,
//! and a hidden window of the first instance receives "show yourself" from later ones.

use std::sync::Arc;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DispatchMessageW,
    FindWindowW, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, HWND_MESSAGE, MSG, PostMessageW,
    RegisterClassW, SetWindowLongPtrW, TranslateMessage, WM_APP, WNDCLASSW,
};

const MUTEX: &str = "Local\\Vigie.SingleInstance";
const CLASS: &str = "VigieInstanceWindow";
const WM_SHOW: u32 = WM_APP + 10;

type Callback = Arc<dyn Fn() + Send + Sync>;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// Held by the running instance; the mutex is released when the process ends.
pub(crate) struct Instance {
    _mutex: HANDLE,
}

// SAFETY: the mutex handle is only closed by the OS at exit; it is never used across threads.
unsafe impl Send for Instance {}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: our own mutex handle, closed once.
        unsafe { CloseHandle(self._mutex) };
    }
}

/// `Some` for the first instance, which then gets `on_show` when another launch asks;
/// `None` when Vigie already runs: the running one has been asked to show itself.
pub(crate) fn acquire(on_show: Callback) -> Option<Instance> {
    let name = wide(MUTEX);
    // SAFETY: a named mutex owned by nobody; the handle lives as long as the instance.
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    // SAFETY: no arguments.
    let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if mutex.is_null() {
        // No mutex at all: run rather than refuse to start.
        return Some(Instance { _mutex: mutex });
    }
    if exists {
        signal_running();
        // SAFETY: our handle to the existing mutex.
        unsafe { CloseHandle(mutex) };
        return None;
    }
    std::thread::Builder::new()
        .name("vigie-instance".into())
        .spawn(move || listen(on_show))
        .ok()?;
    Some(Instance { _mutex: mutex })
}

/// Asks the running instance to show its window, and lets it come to the front.
fn signal_running() {
    let class = wide(CLASS);
    // SAFETY: plain lookups and a posted message.
    unsafe {
        let hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
        if !hwnd.is_null() {
            AllowSetForegroundWindow(ASFW_ANY);
            PostMessageW(hwnd, WM_SHOW, 0, 0);
        }
    }
}

fn listen(on_show: Callback) {
    let class = wide(CLASS);
    // SAFETY: a message-only window with a static procedure; the callback box lives until
    // the thread ends with the process.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: module,
            lpszClassName: class.as_ptr(),
            ..WNDCLASSW::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            module,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        let callback = Box::into_raw(Box::new(on_show));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, callback as isize);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_SHOW {
        // SAFETY: set right after the window was created, never freed while it lives.
        let callback = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Callback;
        if !callback.is_null() {
            // SAFETY: as above.
            unsafe { (*callback)() };
        }
        return 0;
    }
    // SAFETY: default handling.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
