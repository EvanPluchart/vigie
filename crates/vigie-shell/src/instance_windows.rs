//! One Vigie per user session (decision 0009): a named mutex says whether Vigie already runs,
//! and a hidden window of the first instance receives "show yourself" from later ones.
//!
//! The first instance confirms from its UI thread, once its window is shown. A later launch
//! that gets no confirmation (the first one hangs, or something else holds the mutex) starts
//! anyway: two Vigies are better than none, above all when Ctrl+Shift+Esc opens Vigie.

use std::sync::Arc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WAIT_OBJECT_0,
    WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, GetCurrentProcessId, OpenEventW, SetEvent,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW,
    GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, GetWindowThreadProcessId, HWND_MESSAGE, MSG,
    PostMessageW, RegisterClassW, SetWindowLongPtrW, TranslateMessage, WM_APP, WNDCLASSW,
};

use crate::ShowRequest;

const MUTEX: &str = "Local\\Vigie.SingleInstance";
const CLASS: &str = "VigieInstanceWindow";
const WM_SHOW: u32 = WM_APP + 10;
/// How long a later launch waits for the running Vigie to show itself.
const ANSWER_WITHIN: Duration = Duration::from_millis(2500);

type Callback = Arc<dyn Fn(ShowRequest) + Send + Sync>;

/// The event a later launch waits on, named after its process.
fn event_name(pid: u32) -> Vec<u16> {
    wide(&format!("Local\\Vigie.Shown.{pid}"))
}

/// Tells the launch that asked (process `pid`) that the window is shown.
pub(crate) fn confirm_shown(pid: u32) {
    let name = event_name(pid);
    // SAFETY: opens an event that launch created; the handle is closed right after.
    unsafe {
        let event = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
        if !event.is_null() {
            SetEvent(event);
            CloseHandle(event);
        }
    }
}

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
        if signal_running() {
            // SAFETY: our handle to the existing mutex.
            unsafe { CloseHandle(mutex) };
            return None;
        }
        // No answer: run, without listening (the instance that holds the mutex keeps it).
        return Some(Instance { _mutex: mutex });
    }
    std::thread::Builder::new()
        .name("vigie-instance".into())
        .spawn(move || listen(on_show))
        .ok()?;
    Some(Instance { _mutex: mutex })
}

/// Asks the running instance to show its window, lets it come to the front, and waits for it
/// to confirm. `false`: it did not answer in time.
fn signal_running() -> bool {
    let class = wide(CLASS);
    // SAFETY: plain calls; this process owns the event and closes it before returning.
    unsafe {
        let pid = GetCurrentProcessId();
        let event = CreateEventW(std::ptr::null(), 1, 0, event_name(pid).as_ptr());
        if event.is_null() {
            return false;
        }
        let deadline = Instant::now() + ANSWER_WITHIN;
        // The running instance may still be creating its window: look again for a while.
        let mut hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
        while hwnd.is_null() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
            hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
        }
        let mut shown = false;
        if !hwnd.is_null() {
            // Only that process may take the foreground, not any.
            let mut owner = 0u32;
            GetWindowThreadProcessId(hwnd, &mut owner);
            AllowSetForegroundWindow(owner);
            if PostMessageW(hwnd, WM_SHOW, pid as WPARAM, 0) != 0 {
                let left = deadline.saturating_duration_since(Instant::now());
                let wait = u32::try_from(left.as_millis()).unwrap_or(u32::MAX).max(500);
                shown = WaitForSingleObject(event, wait) == WAIT_OBJECT_0;
            }
        }
        CloseHandle(event);
        shown
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
            unsafe {
                (*callback)(ShowRequest {
                    from: wparam as u32,
                })
            };
        }
        return 0;
    }
    // SAFETY: default handling.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
