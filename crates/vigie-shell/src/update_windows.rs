//! Updates from the app on Windows (decision 0012): HTTPS through WinHTTP, SHA-256 through
//! BCrypt, both part of Windows, so no crate and no TLS stack of Vigie's own.

use std::io::Write;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;

use windows_sys::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_DISABLE_REDIRECTS, WINHTTP_FLAG_SECURE,
    WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_LOCATION,
    WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen,
    WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData,
    WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts,
};
use windows_sys::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Largest download accepted: the installer is a few MB.
const MAX_BYTES: usize = 64 << 20;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// A WinHTTP handle, closed once.
struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: a handle WinHTTP gave us, closed once.
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn handle(raw: *mut core::ffi::c_void, what: &str) -> Result<Handle, String> {
    if raw.is_null() {
        Err(format!("{what} failed ({})", std::io::Error::last_os_error()))
    } else {
        Ok(Handle(raw))
    }
}

/// A sent request with its response headers received.
struct Response {
    request: Handle,
    // Kept alive while the request is used.
    _connection: Handle,
    _session: Handle,
}

/// Sends `GET url` (https only); `follow` says whether redirects are followed.
fn get(url: &str, follow: bool) -> Result<Response, String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| "only https is used".to_owned())?;
    let (host, path) = match rest.find('/') {
        Some(at) => rest.split_at(at),
        None => (rest, "/"),
    };
    let agent = wide(concat!("Vigie/", env!("CARGO_PKG_VERSION")));
    let (host, path, verb) = (wide(host), wide(path), wide("GET"));
    // SAFETY: NUL-terminated strings that outlive the calls; every handle is owned by a
    // `Handle` and closed once.
    unsafe {
        let session = handle(
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                std::ptr::null(),
                std::ptr::null(),
                0,
            ),
            "WinHttpOpen",
        )?;
        // Resolve, connect, send, receive: 15 s each at most.
        WinHttpSetTimeouts(session.0, 15_000, 15_000, 15_000, 15_000);
        let connection = handle(
            WinHttpConnect(session.0, host.as_ptr(), 443, 0),
            "WinHttpConnect",
        )?;
        let request = handle(
            WinHttpOpenRequest(
                connection.0,
                verb.as_ptr(),
                path.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ),
            "WinHttpOpenRequest",
        )?;
        if !follow {
            let feature: u32 = WINHTTP_DISABLE_REDIRECTS;
            WinHttpSetOption(
                request.0,
                WINHTTP_OPTION_DISABLE_FEATURE,
                (&raw const feature).cast(),
                4,
            );
        }
        if WinHttpSendRequest(request.0, std::ptr::null(), 0, std::ptr::null(), 0, 0, 0) == 0
            || WinHttpReceiveResponse(request.0, std::ptr::null_mut()) == 0
        {
            return Err(format!(
                "no answer from {} ({})",
                url,
                std::io::Error::last_os_error()
            ));
        }
        Ok(Response {
            request,
            _connection: connection,
            _session: session,
        })
    }
}

impl Response {
    fn status(&self) -> u32 {
        let mut status = 0u32;
        let mut len = 4u32;
        // SAFETY: a u32 out value of the size passed.
        unsafe {
            WinHttpQueryHeaders(
                self.request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                std::ptr::null(),
                (&raw mut status).cast(),
                &mut len,
                std::ptr::null_mut(),
            )
        };
        status
    }

    fn location(&self) -> Option<String> {
        let mut buffer = [0u16; 1024];
        let mut len = (buffer.len() * 2) as u32;
        // SAFETY: the buffer and its size in bytes match.
        let ok = unsafe {
            WinHttpQueryHeaders(
                self.request.0,
                WINHTTP_QUERY_LOCATION,
                std::ptr::null(),
                buffer.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then(|| String::from_utf16_lossy(&buffer[..(len as usize / 2).min(buffer.len())]))
    }

    /// The body, up to `MAX_BYTES`, into `out`.
    fn read_into(&self, out: &mut impl Write) -> Result<usize, String> {
        let mut total = 0usize;
        let mut chunk = vec![0u8; 64 << 10];
        loop {
            let mut available = 0u32;
            // SAFETY: a received request; valid out pointer.
            if unsafe { WinHttpQueryDataAvailable(self.request.0, &mut available) } == 0 {
                return Err(format!("download interrupted ({})", std::io::Error::last_os_error()));
            }
            if available == 0 {
                return Ok(total);
            }
            let want = (available as usize).min(chunk.len());
            let mut read = 0u32;
            // SAFETY: `chunk` holds at least `want` bytes.
            if unsafe {
                WinHttpReadData(self.request.0, chunk.as_mut_ptr().cast(), want as u32, &mut read)
            } == 0
            {
                return Err(format!("download interrupted ({})", std::io::Error::last_os_error()));
            }
            total += read as usize;
            if total > MAX_BYTES {
                return Err("download larger than expected".into());
            }
            out.write_all(&chunk[..read as usize])
                .map_err(|e| e.to_string())?;
        }
    }
}

/// Where `url` redirects to, without following it.
pub(crate) fn redirect_of(url: &str) -> Result<String, String> {
    let response = get(url, false)?;
    match response.status() {
        301 | 302 | 303 | 307 | 308 => response
            .location()
            .ok_or_else(|| "a redirect without a location".to_owned()),
        status => Err(format!("unexpected answer {status} from {url}")),
    }
}

/// The body of `url` (redirects followed) as text.
pub(crate) fn text(url: &str) -> Result<String, String> {
    let response = get(url, true)?;
    match response.status() {
        200 => {
            let mut body = Vec::new();
            response.read_into(&mut body)?;
            String::from_utf8(body).map_err(|_| "not text".to_owned())
        }
        status => Err(format!("answer {status} for {url}")),
    }
}

/// Saves the body of `url` (redirects followed) to `path`.
pub(crate) fn download(url: &str, path: &Path) -> Result<(), String> {
    let response = get(url, true)?;
    match response.status() {
        200 => {
            let mut file = std::fs::File::create(path).map_err(|e| e.to_string())?;
            response.read_into(&mut file)?;
            file.sync_all().map_err(|e| e.to_string())
        }
        status => Err(format!("answer {status} for {url}")),
    }
}

/// The SHA-256 of a file.
pub(crate) fn sha256_file(path: &Path) -> Option<[u8; 32]> {
    let data = std::fs::read(path).ok()?;
    let mut digest = [0u8; 32];
    // SAFETY: the pseudo-handle of SHA-256; input and output sizes as passed.
    let status = unsafe {
        BCryptHash(
            BCRYPT_SHA256_ALG_HANDLE,
            std::ptr::null(),
            0,
            data.as_ptr(),
            u32::try_from(data.len()).ok()?,
            digest.as_mut_ptr(),
            32,
        )
    };
    (status == 0).then_some(digest)
}

/// This process runs with administrator rights (Ctrl+Shift+Esc may start Vigie so).
pub(crate) fn is_elevated() -> bool {
    let mut token = std::ptr::null_mut();
    // SAFETY: our own process; the token handle is owned right after.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    // SAFETY: a fresh handle nothing else owns.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: a TOKEN_ELEVATION out value of the size passed.
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&raw mut elevation).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    ok != 0 && elevation.TokenIsElevated != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_a_file() {
        let dir = std::env::temp_dir().join(format!("vigie-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("abc.txt");
        std::fs::write(&file, b"abc").unwrap();
        let digest = sha256_file(&file).expect("hashed");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Goes online, so only by hand: `cargo test -p vigie-shell -- --ignored latest`.
    #[test]
    #[ignore = "network"]
    fn latest_release_redirects_to_its_tag() {
        let to = redirect_of("https://github.com/EvanPluchart/vigie/releases/latest").unwrap();
        eprintln!("latest: {to}");
        assert!(to.contains("/releases/tag/v"), "{to}");
    }
}
