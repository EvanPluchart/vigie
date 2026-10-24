//! Small desktop actions on Windows: the clipboard, the file manager, a file's Properties dialog
//! and the default browser.

use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{GlobalFree, HWND};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_INVOKEIDLIST, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW, ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// explorer.exe in the Windows folder. A bare `explorer.exe` would be looked up in Vigie's own
/// folder first, where anything named so would run instead.
fn explorer() -> PathBuf {
    let windows = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(windows).join("explorer.exe")
}

pub(crate) fn copy_text(text: &str) -> bool {
    let units = wide(text);
    // SAFETY: standard clipboard sequence. The global memory belongs to the clipboard once
    // SetClipboardData succeeds; it is freed here otherwise.
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();
        let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2);
        if memory.is_null() {
            CloseClipboard();
            return false;
        }
        let target = GlobalLock(memory).cast::<u16>();
        if target.is_null() {
            GlobalFree(memory);
            CloseClipboard();
            return false;
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), target, units.len());
        GlobalUnlock(memory);
        let placed = !SetClipboardData(u32::from(CF_UNICODETEXT), memory).is_null();
        if !placed {
            GlobalFree(memory);
        }
        CloseClipboard();
        placed
    }
}

pub(crate) fn reveal(path: &Path) -> bool {
    // `/select,` opens the folder with the file highlighted.
    std::process::Command::new(explorer())
        .arg(format!("/select,{}", path.display()))
        .spawn()
        .is_ok()
}

pub(crate) fn properties(path: &Path) -> bool {
    let file = wide(&path.to_string_lossy());
    let verb = wide("properties");
    // SAFETY: an all-zero SHELLEXECUTEINFOW is valid; the strings outlive the call.
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    // The Properties dialog is a shell verb, reached through the item's context menu.
    info.fMask = SEE_MASK_INVOKEIDLIST | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.nShow = SW_SHOWNORMAL;
    // SAFETY: `info` is initialised as the call requires.
    unsafe { ShellExecuteExW(&mut info) != 0 }
}

pub(crate) fn open_url(url: &str) -> bool {
    let (verb, target) = (wide("open"), wide(url));
    // SAFETY: valid null-terminated strings; null parameters and directory are allowed.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut::<core::ffi::c_void>() as HWND,
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success.
    result as usize > 32
}
