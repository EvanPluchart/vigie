//! The tray icon on Windows (decision 0008): `Shell_NotifyIconW` with `NOTIFYICON_VERSION_4`,
//! driven by a hidden window on a thread of its own. The thread sleeps in `GetMessageW`, so the
//! icon costs nothing between events. Notifications are balloons on the same icon, which
//! Windows 10 and 11 show as toasts.

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_LARGE_ICON, NIIF_USER, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_BALLOONUSERCLICK, NIN_SELECT, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
    DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, HICON,
    IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR, LoadIconW, LoadImageW, MF_SEPARATOR, MF_STRING,
    MSG, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SM_CXICON,
    SM_CXSMICON, SetForegroundWindow, SetWindowLongPtrW, TPM_BOTTOMALIGN, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_APP, WM_CLOSE, WM_CONTEXTMENU,
    WM_DESTROY, WM_ENDSESSION, WM_LBUTTONDBLCLK, WM_NULL, WNDCLASSW, WS_OVERLAPPED,
};

use crate::{Rect, TrayEvent, TrayMenu};

/// Messages the icon sends to the window, and the ones the app posts to it.
const WM_TRAY: u32 = WM_APP + 1;
const WM_UPDATE: u32 = WM_APP + 2;
const WM_NOTIFY_USER: u32 = WM_APP + 3;
/// Vigie removes its icon. Not WM_CLOSE: other programs send that one (`taskkill` without
/// `/F` does), and the icon must not vanish while Vigie runs hidden behind it.
const WM_SHUTDOWN: u32 = WM_APP + 4;
const ICON_ID: u32 = 1;
/// The application icon in the executable's resources: `winresource` gives it ID 1.
const ICON_RESOURCE: u16 = 1;
const MENU_OPEN: usize = 1;
const MENU_QUIT: usize = 2;

type Callback = Arc<dyn Fn(TrayEvent) + Send + Sync>;

/// What the app changes from its threads; the tray thread applies it on `WM_UPDATE`.
struct Shared {
    tooltip: String,
    menu: TrayMenu,
    notification: Option<(String, String)>,
    /// The icon is in the notification area (adding it fails while Explorer is down).
    shown: bool,
}

/// Lives on the tray thread, pointed to by the window's user data.
struct State {
    shared: Arc<Mutex<Shared>>,
    callback: Callback,
    icon: HICON,
    balloon_icon: HICON,
    taskbar_created: u32,
}

pub(crate) struct Tray {
    /// The window handle as an integer, so the handle can cross threads.
    hwnd: isize,
    shared: Arc<Mutex<Shared>>,
    thread: Option<JoinHandle<()>>,
}

fn wide_into<const N: usize>(text: &str, out: &mut [u16; N]) {
    out.fill(0);
    for (slot, unit) in out.iter_mut().take(N - 1).zip(text.encode_utf16()) {
        *slot = unit;
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

impl Tray {
    pub(crate) fn new(tooltip: &str, menu: TrayMenu, callback: Callback) -> Option<Tray> {
        let shared = Arc::new(Mutex::new(Shared {
            tooltip: tooltip.to_owned(),
            menu,
            notification: None,
            shown: false,
        }));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<isize>();
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("vigie-tray".into())
            .spawn(move || run(thread_shared, callback, ready_tx))
            .ok()?;
        let hwnd = ready_rx.recv().ok().filter(|&h| h != 0)?;
        Some(Tray {
            hwnd,
            shared,
            thread: Some(thread),
        })
    }

    fn post(&self, message: u32) {
        // SAFETY: posting to a window owned by the tray thread; harmless if it is gone.
        unsafe { PostMessageW(self.hwnd as HWND, message, 0, 0) };
    }

    pub(crate) fn set_tooltip(&self, tooltip: &str) {
        if let Ok(mut shared) = self.shared.lock() {
            if shared.tooltip == tooltip {
                return;
            }
            tooltip.clone_into(&mut shared.tooltip);
        }
        self.post(WM_UPDATE);
    }

    pub(crate) fn set_menu(&self, menu: TrayMenu) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.menu = menu;
        }
    }

    pub(crate) fn notify(&self, title: &str, body: &str) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.notification = Some((title.to_owned(), body.to_owned()));
        }
        self.post(WM_NOTIFY_USER);
    }
}

impl Tray {
    /// Whether the icon is on screen: hiding Vigie behind a missing icon would lose it.
    pub(crate) fn is_shown(&self) -> bool {
        self.shared.lock().is_ok_and(|s| s.shown)
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.post(WM_SHUTDOWN);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn load_icon(metric: i32) -> HICON {
    // SAFETY: plain system calls; the resource ID is passed as MAKEINTRESOURCE.
    unsafe {
        let size = GetSystemMetricsForDpi(metric, GetDpiForSystem());
        let module = GetModuleHandleW(std::ptr::null());
        let icon = LoadImageW(
            module,
            ICON_RESOURCE as usize as *const u16,
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        );
        if icon.is_null() {
            // No icon resource (tests): the generic application icon.
            LoadIconW(std::ptr::null_mut(), IDI_APPLICATION)
        } else {
            icon
        }
    }
}

fn icon_data(hwnd: HWND, state: &State) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_TRAY,
        hIcon: state.icon,
        ..NOTIFYICONDATAW::default()
    };
    if let Ok(shared) = state.shared.lock() {
        wide_into(&shared.tooltip, &mut data.szTip);
    }
    data
}

fn add_icon(hwnd: HWND, state: &State) {
    let mut data = icon_data(hwnd, state);
    // SAFETY: a fully initialised structure for our own window.
    let shown = unsafe {
        let shown = Shell_NotifyIconW(NIM_ADD, &data) != 0;
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        Shell_NotifyIconW(NIM_SETVERSION, &data);
        shown
    };
    if let Ok(mut shared) = state.shared.lock() {
        shared.shown = shown;
    }
}

fn run(shared: Arc<Mutex<Shared>>, callback: Callback, ready: std::sync::mpsc::Sender<isize>) {
    let class = wide("VigieTrayWindow");
    // SAFETY: registering a class with a static window procedure; failing again (already
    // registered by an earlier tray in this process) is fine.
    let hwnd = unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: module,
            lpszClassName: class.as_ptr(),
            ..WNDCLASSW::default()
        };
        RegisterClassW(&wc);
        // A hidden top-level window, not a message-only one: only top-level windows get the
        // TaskbarCreated broadcast sent when Explorer restarts.
        CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            module,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        let _ = ready.send(0);
        return;
    }
    let taskbar = wide("TaskbarCreated");
    let state = Box::new(State {
        shared,
        callback,
        icon: load_icon(SM_CXSMICON),
        balloon_icon: load_icon(SM_CXICON),
        // SAFETY: a NUL-terminated name.
        taskbar_created: unsafe { RegisterWindowMessageW(taskbar.as_ptr()) },
    });
    add_icon(hwnd, &state);
    let state = Box::into_raw(state);
    // SAFETY: the window proc reads this pointer until WM_DESTROY, on this thread only.
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize) };
    let _ = ready.send(hwnd as isize);
    let mut msg = MSG::default();
    // SAFETY: a standard message loop for this thread's window.
    unsafe {
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // The window is destroyed: free what it pointed to.
        let state = Box::from_raw(state);
        DestroyIcon(state.icon);
        DestroyIcon(state.balloon_icon);
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: set by `run` before any of our messages can arrive; null before that.
    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const State;
    if state.is_null() {
        // SAFETY: default handling.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    // SAFETY: the state outlives the window (freed after the loop ends).
    let state = unsafe { &*state };
    match msg {
        WM_TRAY => {
            // Version 4: the event in the low word of lparam, the anchor point in wparam.
            let event = (lparam as u32) & 0xffff;
            let x = (wparam & 0xffff) as u16 as i16 as i32;
            let y = ((wparam >> 16) & 0xffff) as u16 as i16 as i32;
            match event {
                NIN_SELECT | 0x401 /* NIN_KEYSELECT */ => (state.callback)(TrayEvent::Clicked { x, y }),
                WM_LBUTTONDBLCLK => (state.callback)(TrayEvent::Open),
                WM_CONTEXTMENU => show_menu(hwnd, state, x, y),
                NIN_BALLOONUSERCLICK => (state.callback)(TrayEvent::NotificationClicked),
                _ => {}
            }
            0
        }
        WM_UPDATE => {
            let mut data = icon_data(hwnd, state);
            data.uFlags = NIF_TIP | NIF_SHOWTIP;
            // SAFETY: modifying our own icon.
            unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
            0
        }
        WM_NOTIFY_USER => {
            let notification = state
                .shared
                .lock()
                .ok()
                .and_then(|mut s| s.notification.take());
            if let Some((title, body)) = notification {
                let mut data = icon_data(hwnd, state);
                data.uFlags = NIF_INFO;
                wide_into(&title, &mut data.szInfoTitle);
                wide_into(&body, &mut data.szInfo);
                data.dwInfoFlags = NIIF_USER | NIIF_LARGE_ICON;
                data.hBalloonIcon = state.balloon_icon;
                // SAFETY: modifying our own icon.
                unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
            }
            0
        }
        // Someone else's request to close this hidden window: Vigie decides when its icon goes.
        WM_CLOSE => 0,
        // Windows is signing out, or an installer asks Vigie to close (Restart Manager).
        WM_ENDSESSION if wparam != 0 => {
            (state.callback)(TrayEvent::Quit);
            0
        }
        WM_SHUTDOWN => {
            if let Ok(mut shared) = state.shared.lock() {
                shared.shown = false;
            }
            let data = icon_data(hwnd, state);
            // SAFETY: removing our icon, then destroying our window.
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &data);
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            0
        }
        // Explorer restarted: the icon is gone, add it again.
        m if m == state.taskbar_created => {
            add_icon(hwnd, state);
            0
        }
        // SAFETY: default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn show_menu(hwnd: HWND, state: &State, x: i32, y: i32) {
    let Ok(labels) = state.shared.lock().map(|s| s.menu.clone()) else {
        return;
    };
    let (open, quit) = (wide(&labels.open), wide(&labels.quit));
    // SAFETY: a menu owned and destroyed here. Our window must be in the foreground for the
    // menu to close when the user clicks elsewhere, and WM_NULL afterwards lets it close
    // cleanly (documented TrackPopupMenu behaviour).
    let command = unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        AppendMenuW(menu, MF_STRING, MENU_OPEN, open.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(menu, MF_STRING, MENU_QUIT, quit.as_ptr());
        SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            x,
            y,
            0,
            hwnd,
            std::ptr::null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        command
    };
    match command as usize {
        MENU_OPEN => (state.callback)(TrayEvent::Open),
        MENU_QUIT => (state.callback)(TrayEvent::Quit),
        _ => {}
    }
}

/// The user's default locale name ("fr-FR").
pub(crate) fn user_language() -> Option<String> {
    let mut name = [0u16; 85];
    // SAFETY: a buffer of LOCALE_NAME_MAX_LENGTH characters.
    let len = unsafe {
        windows_sys::Win32::Globalization::GetUserDefaultLocaleName(
            name.as_mut_ptr(),
            name.len() as i32,
        )
    };
    (len > 1).then(|| String::from_utf16_lossy(&name[..len as usize - 1]))
}

/// The work area (screen minus taskbar) of the monitor holding (`x`, `y`).
pub(crate) fn work_area(x: i32, y: i32) -> Option<Rect> {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    // SAFETY: plain queries.
    let ok = unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(monitor, &mut info)
    };
    (ok != 0).then(|| Rect {
        x: info.rcWork.left,
        y: info.rcWork.top,
        width: info.rcWork.right - info.rcWork.left,
        height: info.rcWork.bottom - info.rcWork.top,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Adds an icon for a moment and removes it: the tray thread starts and stops cleanly.
    #[test]
    fn starts_and_stops() {
        let tray = Tray::new(
            "Vigie test",
            TrayMenu {
                open: "Open".into(),
                quit: "Quit".into(),
            },
            Arc::new(|_| {}),
        )
        .expect("a tray icon");
        tray.set_tooltip("Vigie test · 1 %");
        drop(tray);
        assert!(work_area(0, 0).is_some_and(|a| a.width > 0 && a.height > 0));
    }
}
