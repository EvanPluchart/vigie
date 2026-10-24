//! One Vigie per user session (decision 0009): a named mutex says whether Vigie already runs,
//! and a hidden window of the first instance receives "show yourself" from later ones.
//!
//! A later launch sends the request and waits for the answer, which the first instance gives
//! from its UI thread once its window is shown. A launch that gets no answer (the first one
//! hangs, or something else holds the mutex) starts anyway: two Vigies are better than none,
//! above all when Ctrl+Shift+Esc opens Vigie. The answer travels back with the message itself,
//! not through a named object: Ctrl+Shift+Esc starts Vigie elevated, and an object an elevated
//! process creates is out of reach of the user's ordinary Vigie.

use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW,
    GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, GetWindowThreadProcessId, HWND_MESSAGE, MSG,
    RegisterClassW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, SetWindowLongPtrW, TranslateMessage,
    WM_APP, WNDCLASSW,
};

use crate::ShowRequest;

const WM_SHOW: u32 = WM_APP + 10;
/// How long a later launch waits for the running Vigie to show itself.
const ANSWER_WITHIN: Duration = Duration::from_millis(2500);
/// How long the listener waits for the UI thread, within `ANSWER_WITHIN`.
const UI_ANSWER_WITHIN: Duration = Duration::from_secs(2);

type Callback = Arc<dyn Fn(ShowRequest) + Send + Sync>;

/// The names that make one instance: the mutex and the listener's window class.
#[derive(Clone, Copy)]
pub(crate) struct Names {
    pub(crate) mutex: &'static str,
    pub(crate) class: &'static str,
}

pub(crate) const VIGIE: Names = Names {
    mutex: "Local\\Vigie.SingleInstance",
    class: "VigieInstanceWindow",
};

/// Tells the waiting launch that the window is shown.
pub(crate) fn confirm_shown(answer: Sender<()>) {
    let _ = answer.send(());
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
        if !self._mutex.is_null() {
            // SAFETY: our own mutex handle, closed once.
            unsafe { CloseHandle(self._mutex) };
        }
    }
}

/// `Some` for the first instance, which then gets `on_show` when another launch asks, and for
/// a launch the running instance did not answer; `None` when the running Vigie showed itself.
pub(crate) fn acquire(names: Names, on_show: Callback) -> Option<Instance> {
    let name = wide(names.mutex);
    // SAFETY: a named mutex owned by nobody; the handle lives as long as the instance.
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    // SAFETY: no arguments.
    let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if mutex.is_null() {
        // No mutex at all (one an elevated Vigie made is out of reach): run rather than
        // refuse to start.
        return Some(Instance { _mutex: mutex });
    }
    if exists {
        if signal_running(names) {
            // SAFETY: our handle to the existing mutex.
            unsafe { CloseHandle(mutex) };
            return None;
        }
        // No answer: run, without listening (the instance that holds the mutex keeps it).
        return Some(Instance { _mutex: mutex });
    }
    std::thread::Builder::new()
        .name("vigie-instance".into())
        .spawn(move || listen(names, on_show))
        .ok()?;
    Some(Instance { _mutex: mutex })
}

/// Asks the running instance to show its window, lets it come to the front, and waits for its
/// answer. `false`: it did not answer in time.
fn signal_running(names: Names) -> bool {
    let class = wide(names.class);
    let deadline = Instant::now() + ANSWER_WITHIN;
    // SAFETY: plain lookups, and a message sent with a timeout.
    unsafe {
        // The running instance may still be creating its window: look again for a while.
        let mut hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
        while hwnd.is_null() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
            hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
        }
        if hwnd.is_null() {
            return false;
        }
        // Only that process may take the foreground, not any.
        let mut owner = 0u32;
        GetWindowThreadProcessId(hwnd, &mut owner);
        AllowSetForegroundWindow(owner);
        let left = deadline.saturating_duration_since(Instant::now());
        let wait = u32::try_from(left.as_millis())
            .unwrap_or(u32::MAX)
            .max(500);
        let mut shown = 0usize;
        SendMessageTimeoutW(
            hwnd,
            WM_SHOW,
            0,
            0,
            SMTO_ABORTIFHUNG,
            wait,
            &mut shown,
        ) != 0
            && shown == 1
    }
}

fn listen(names: Names, on_show: Callback) {
    let class = wide(names.class);
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
        if callback.is_null() {
            return 0;
        }
        // The UI thread answers once the window is shown; a hung one never does. This thread
        // only listens, so waiting here blocks nothing else.
        let (answer, answered) = std::sync::mpsc::channel();
        // SAFETY: as above.
        unsafe { (*callback)(ShowRequest { answer }) };
        return LRESULT::from(answered.recv_timeout(UI_ANSWER_WITHIN).is_ok());
    }
    // SAFETY: default handling.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second launch whose running instance answers exits; one whose instance never answers
    /// (it hangs) starts. Names of their own, so the user's Vigie is never involved.
    #[test]
    fn a_launch_starts_when_the_running_instance_does_not_answer() {
        let answering = Names {
            mutex: "Local\\Vigie.Test.Answering",
            class: "VigieTestAnswering",
        };
        let first = acquire(answering, Arc::new(|request: ShowRequest| request.done()));
        assert!(first.is_some(), "the first launch runs");
        assert!(
            acquire(answering, Arc::new(|_| {})).is_none(),
            "shown by the running instance: this launch exits"
        );

        let hung = Names {
            mutex: "Local\\Vigie.Test.Hung",
            class: "VigieTestHung",
        };
        // Never answers, as a Vigie whose UI thread hangs.
        let first = acquire(hung, Arc::new(|request: ShowRequest| std::mem::forget(request)));
        assert!(first.is_some());
        let started = Instant::now();
        assert!(
            acquire(hung, Arc::new(|_| {})).is_some(),
            "no answer: this launch runs"
        );
        assert!(started.elapsed() < Duration::from_secs(4), "within the wait");
    }
}
