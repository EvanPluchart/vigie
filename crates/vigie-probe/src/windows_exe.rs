//! What an executable file says about itself on Windows: its path, icon, publisher, description.
//! Called once per new process (path) or once per executable (the rest), never every tick.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetObjectW, HBITMAP,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetDriveTypeW, GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows_sys::Win32::System::LibraryLoader::{
    EnumResourceNamesW, FindResourceW, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE,
    LoadLibraryExW, LoadResource, LockResource, SizeofResource,
};
use windows_sys::Win32::System::Threading::{PROCESS_NAME_WIN32, QueryFullProcessImageNameW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DestroyIcon, GetIconInfo, HICON, ICONINFO, LR_DEFAULTCOLOR,
    LookupIconIdFromDirectoryEx, RT_GROUP_ICON, RT_ICON,
};
use windows_sys::core::{BOOL, PCWSTR};

use crate::{ExeInfo, Icon};

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// explorer.exe in the Windows folder. A bare `explorer.exe` would be looked up in Vigie's own
/// folder first, where anything named so would run instead.
pub(crate) fn explorer() -> PathBuf {
    let windows = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(windows).join("explorer.exe")
}

/// Full path of a process's executable, from a handle with query rights.
pub(crate) fn image_path(handle: &OwnedHandle) -> Option<PathBuf> {
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    // SAFETY: the handle is live; `buffer` is valid for `len` UTF-16 units.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            handle.as_raw_handle(),
            PROCESS_NAME_WIN32,
            buffer.as_mut_ptr(),
            &mut len,
        )
    };
    (ok != 0).then(|| OsString::from_wide(&buffer[..len as usize]).into())
}

/// `size` is the icon's side in physical pixels; the closest size the file offers is used.
pub(crate) fn exe_info(exe: &Path, size: u32) -> ExeInfo {
    // A file on a network share (a startup entry can name one) is not read: opening it would
    // connect to that machine and send it the user's Windows credentials.
    if is_remote(exe) {
        return ExeInfo::default();
    }
    let path = wide(exe);
    let (publisher, description) = version_strings(&path);
    ExeInfo {
        icon: icon(&path, size),
        publisher,
        description,
    }
}

/// A UNC path (`\\server\share\…`, not `\\?\C:\…`) or a mapped network drive.
fn is_remote(exe: &Path) -> bool {
    use std::path::{Component, Prefix};
    // GetDriveTypeW's answer for a network drive (in a windows-sys module not enabled here).
    const DRIVE_REMOTE: u32 = 4;
    match exe.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::UNC(..) | Prefix::VerbatimUNC(..) => true,
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                let root = [u16::from(letter), u16::from(b':'), u16::from(b'\\'), 0];
                // SAFETY: a NUL-terminated drive root.
                unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
            }
            _ => false,
        },
        _ => false,
    }
}

/// The executable's first icon group, as Explorer shows it, read straight from its resources.
/// This avoids the shell icon APIs, which load shell32 and its dependencies (about 20 MB of
/// images in the working set).
fn icon(path: &[u16], size: u32) -> Option<Icon> {
    struct Search {
        size: i32,
        icon: HICON,
    }

    unsafe extern "system" fn first_group(
        module: HMODULE,
        kind: PCWSTR,
        name: PCWSTR,
        param: isize,
    ) -> BOOL {
        // SAFETY: `param` is the `&mut Search` passed below, alive during the enumeration.
        let search = unsafe { &mut *(param as *mut Search) };
        // SAFETY: `module` is a data-file module; `name` is valid during this callback; every
        // pointer and size comes from the resource functions themselves.
        unsafe {
            let Some(group) = resource(module, name, kind) else {
                return 1;
            };
            let id = LookupIconIdFromDirectoryEx(
                group.as_ptr(),
                1,
                search.size,
                search.size,
                LR_DEFAULTCOLOR,
            );
            if id == 0 {
                return 1;
            }
            let Some(bits) = resource(module, id as u16 as usize as PCWSTR, RT_ICON) else {
                return 1;
            };
            search.icon = CreateIconFromResourceEx(
                bits.as_ptr(),
                bits.len() as u32,
                1,
                0x0003_0000,
                search.size,
                search.size,
                LR_DEFAULTCOLOR,
            );
        }
        // Stop after the first group, found or not.
        0
    }

    let size = size.clamp(16, 256) as i32;
    // SAFETY: `path` is NUL-terminated; the module is loaded as data only (no code runs) and
    // freed below.
    let module = unsafe {
        LoadLibraryExW(
            path.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
        )
    };
    if module.is_null() {
        return None;
    }
    let mut search = Search {
        size,
        icon: std::ptr::null_mut(),
    };
    // SAFETY: `first_group` matches ENUMRESNAMEPROCW and uses `search` as documented above.
    unsafe {
        EnumResourceNamesW(
            module,
            RT_GROUP_ICON,
            Some(first_group),
            &mut search as *mut Search as isize,
        );
        FreeLibrary(module);
    }
    if search.icon.is_null() {
        return None;
    }
    let pixels = icon_pixels(search.icon);
    // SAFETY: the icon was created for us by CreateIconFromResourceEx.
    unsafe { DestroyIcon(search.icon) };
    pixels
}

/// The bytes of one resource of a data-file module. They live as long as the module.
///
/// # Safety
/// `module` must be loaded and stay loaded while the slice is used.
unsafe fn resource<'a>(module: HMODULE, name: PCWSTR, kind: PCWSTR) -> Option<&'a [u8]> {
    // SAFETY: per the contract above; null results are checked.
    unsafe {
        let info = FindResourceW(module, name, kind);
        if info.is_null() {
            return None;
        }
        let size = SizeofResource(module, info);
        let data = LockResource(LoadResource(module, info));
        (!data.is_null() && size > 0)
            .then(|| std::slice::from_raw_parts(data.cast::<u8>(), size as usize))
    }
}

fn icon_pixels(icon: HICON) -> Option<Icon> {
    // SAFETY: an all-zero ICONINFO is a valid out value.
    let mut info: ICONINFO = unsafe { std::mem::zeroed() };
    // SAFETY: `icon` is live; GetIconInfo creates two bitmaps we must delete.
    if unsafe { GetIconInfo(icon, &mut info) } == 0 {
        return None;
    }
    let color = bitmap_bgra(info.hbmColor);
    let mask = bitmap_bgra(info.hbmMask);
    // SAFETY: both bitmaps were created by GetIconInfo for us.
    unsafe {
        DeleteObject(info.hbmColor);
        DeleteObject(info.hbmMask);
    }
    let (width, height, mut bgra) = color?;
    // Old icons have no alpha channel: take transparency from the mask (white is transparent).
    if bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        let (_, _, mask) = mask?;
        for (pixel, m) in bgra
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(mask.as_chunks::<4>().0.iter())
        {
            pixel[3] = if m[0] == 0 { 255 } else { 0 };
        }
    }
    for pixel in bgra.as_chunks_mut::<4>().0.iter_mut() {
        pixel.swap(0, 2);
    }
    Some(Icon {
        width,
        height,
        rgba: bgra,
    })
}

/// The pixels of a bitmap as top-down 32-bit BGRA.
fn bitmap_bgra(bitmap: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
    if bitmap.is_null() {
        return None;
    }
    // SAFETY: an all-zero BITMAP is a valid out value; the size matches.
    let mut bm: BITMAP = unsafe { std::mem::zeroed() };
    // SAFETY: `bitmap` is live; `bm` is valid for its size.
    if unsafe {
        GetObjectW(
            bitmap,
            size_of::<BITMAP>() as i32,
            (&mut bm as *mut BITMAP).cast(),
        )
    } == 0
    {
        return None;
    }
    let (width, height) = (bm.bmWidth.unsigned_abs(), bm.bmHeight.unsigned_abs());
    if width == 0 || height == 0 || width > 256 || height > 256 {
        return None;
    }
    // SAFETY: an all-zero BITMAPINFO is valid once its header is filled in below.
    let mut header: BITMAPINFO = unsafe { std::mem::zeroed() };
    header.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width as i32,
        // Negative height: rows top-down.
        biHeight: -(height as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..header.bmiHeader
    };
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    // SAFETY: plain calls; the DC is deleted below and `pixels` holds width*height 32-bit pixels.
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            height,
            pixels.as_mut_ptr().cast(),
            &mut header,
            DIB_RGB_COLORS,
        );
        DeleteDC(dc);
        (lines == height as i32).then_some((width, height, pixels))
    }
}

/// CompanyName and FileDescription from the version resource.
fn version_strings(path: &[u16]) -> (Option<String>, Option<String>) {
    let mut ignored = 0u32;
    // SAFETY: `path` is NUL-terminated.
    let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), &mut ignored) };
    if size == 0 {
        return (None, None);
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: `data` is valid for `size` bytes.
    if unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return (None, None);
    }
    // The first language and code page the file declares; US English Unicode otherwise.
    let translation = query(&data, "\\VarFileInfo\\Translation")
        .filter(|bytes| bytes.len() >= 4)
        .map(|bytes| {
            let language = u16::from_le_bytes([bytes[0], bytes[1]]);
            let code_page = u16::from_le_bytes([bytes[2], bytes[3]]);
            format!("{language:04x}{code_page:04x}")
        })
        .unwrap_or_else(|| "040904b0".into());
    let string = |name: &str| {
        let bytes = query(&data, &format!("\\StringFileInfo\\{translation}\\{name}"))?;
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        let text = String::from_utf16_lossy(&units).trim().to_owned();
        (!text.is_empty()).then_some(text)
    };
    (string("CompanyName"), string("FileDescription"))
}

/// One value of a version resource, as raw bytes borrowed from `data`.
fn query<'a>(data: &'a [u8], sub_block: &str) -> Option<&'a [u8]> {
    let sub: Vec<u16> = sub_block.encode_utf16().chain(Some(0)).collect();
    let mut value: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut len = 0u32;
    // SAFETY: `data` holds a version resource; `sub` is NUL-terminated; outputs are valid.
    let ok = unsafe { VerQueryValueW(data.as_ptr().cast(), sub.as_ptr(), &mut value, &mut len) };
    if ok == 0 || value.is_null() || len == 0 {
        return None;
    }
    let start = (value as usize).checked_sub(data.as_ptr() as usize)?;
    // String values report their length in UTF-16 units, binary ones in bytes; read as bytes,
    // bounded by the resource itself.
    let bytes = if sub_block.starts_with("\\StringFileInfo") {
        len as usize * 2
    } else {
        len as usize
    };
    data.get(start..start.checked_add(bytes)?.min(data.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_explorer() {
        let windows = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .expect("SystemRoot");
        let info = exe_info(&windows.join("explorer.exe"), 32);
        let icon = info.icon.expect("explorer has an icon");
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
        assert!(
            icon.rgba.as_chunks::<4>().0.iter().any(|p| p[3] == 255),
            "some opaque pixels"
        );
        assert!(
            icon.rgba.as_chunks::<4>().0.iter().any(|p| p[3] == 0),
            "some transparent pixels"
        );
        assert_eq!(info.publisher.as_deref(), Some("Microsoft Corporation"));
        assert!(info.description.is_some());
    }

    #[test]
    fn missing_file_has_nothing() {
        let info = exe_info(Path::new("C:\\does\\not\\exist.exe"), 32);
        assert!(info.icon.is_none() && info.publisher.is_none());
    }
}
