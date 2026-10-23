//! Upkeep signals for the diagnostics (decision 0010): a restart Windows is waiting for, and
//! the free space on the system drive. Read from the registry and the file system only: no
//! network, no Windows Update query.

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
};

use crate::{DriveSpace, Maintenance};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// The key exists (its presence is the signal).
fn key_exists(path: &str) -> bool {
    let path = wide(path);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: valid null-terminated path and out pointer; the key is closed below.
    let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ, &mut key) };
    if status == ERROR_SUCCESS {
        // SAFETY: opened above.
        unsafe { RegCloseKey(key) };
        true
    } else {
        false
    }
}

pub(crate) fn read() -> Maintenance {
    // Windows Update installed updates that finish on restart; component servicing (features,
    // cumulative updates) waits for one too.
    let updates = key_exists(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired",
    );
    let servicing = key_exists(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending",
    );
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let root = wide(&format!("{drive}\\"));
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: valid root path and out pointers; the third one may be null.
    let ok =
        unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) };
    Maintenance {
        restart_for_updates: updates || servicing,
        system_drive: (ok != 0 && total > 0).then(|| DriveSpace {
            letter: drive.chars().next().unwrap_or('C'),
            free_bytes: free,
            total_bytes: total,
        }),
    }
}
