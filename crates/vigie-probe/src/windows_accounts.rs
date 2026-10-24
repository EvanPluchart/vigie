//! Who each process runs as, for the User name column (decision 0010). A process Vigie can
//! open names its account through its token, which is cheap. The Terminal Services API names
//! the owner of every process, services included, without opening any, but lists them all
//! (about 12 ms): it is asked only for processes whose token is out of reach.

use std::collections::HashMap;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Security::{
    GetLengthSid, GetTokenInformation, IsValidSid, LookupAccountSidW, PSID, SID_NAME_USE,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::RemoteDesktop::{
    WTS_CURRENT_SERVER_HANDLE, WTS_PROCESS_INFOW, WTSEnumerateProcessesW, WTSFreeMemory,
};
use windows_sys::Win32::System::Threading::OpenProcessToken;

use crate::Pid;

/// Names by SID, each account looked up once.
pub(crate) type NameCache = HashMap<Vec<u8>, Option<String>>;

/// The account of an opened process, from its token. `None` when the token is out of reach
/// (another account's process, without administrator rights).
pub(crate) fn token_owner(process: &OwnedHandle, cache: &mut NameCache) -> Option<Option<String>> {
    let mut token = std::ptr::null_mut();
    // SAFETY: a live process handle; the token handle is owned right after.
    if unsafe { OpenProcessToken(process.as_raw_handle(), TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    // SAFETY: a fresh handle nothing else owns.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    // TOKEN_USER and its SID: well under 512 bytes, 8-byte aligned.
    let mut buffer = [0u64; 64];
    let mut len = 0u32;
    // SAFETY: the buffer and its size in bytes match.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
            &mut len,
        )
    } == 0
    {
        return None;
    }
    // SAFETY: on success the buffer starts with a TOKEN_USER whose SID lies inside it.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    Some(cached_name(sid, cache))
}

/// The name of a valid `sid`, through `cache`.
fn cached_name(sid: PSID, cache: &mut NameCache) -> Option<String> {
    // SAFETY: valid SID; its length bounds the copy.
    let bytes =
        unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), GetLengthSid(sid) as usize) }
            .to_vec();
    cache
        .entry(bytes)
        .or_insert_with(|| account_name(sid))
        .clone()
}

/// The account name of every process, by PID. `cache` keeps names by SID between calls, so
/// each account is looked up once.
pub(crate) fn process_owners(cache: &mut NameCache) -> HashMap<Pid, Option<String>> {
    let mut list: *mut WTS_PROCESS_INFOW = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: valid out pointers; the list is freed below with WTSFreeMemory.
    if unsafe { WTSEnumerateProcessesW(WTS_CURRENT_SERVER_HANDLE, 0, 1, &mut list, &mut count) }
        == 0
        || list.is_null()
    {
        return HashMap::new();
    }
    // SAFETY: the API wrote `count` entries at `list`.
    let entries = unsafe { std::slice::from_raw_parts(list, count as usize) };
    let mut owners = HashMap::with_capacity(entries.len());
    for entry in entries {
        let sid = entry.pUserSid;
        // SAFETY: a SID from the API, valid until WTSFreeMemory; checked before use.
        if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
            continue;
        }
        owners.insert(entry.ProcessId, cached_name(sid, cache));
    }
    // SAFETY: allocated by WTSEnumerateProcessesW.
    unsafe { WTSFreeMemory(list.cast()) };
    owners
}

/// "pluch", "SYSTEM", "LOCAL SERVICE": the account part, as Task Manager shows it.
fn account_name(sid: PSID) -> Option<String> {
    let mut name = [0u16; 256];
    let mut domain = [0u16; 256];
    let (mut name_len, mut domain_len) = (name.len() as u32, domain.len() as u32);
    let mut kind: SID_NAME_USE = 0;
    // SAFETY: buffers and their lengths match; `sid` is valid.
    let ok = unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            sid,
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut kind,
        )
    };
    (ok != 0).then(|| String::from_utf16_lossy(&name[..name_len as usize]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The token of this test process names the same account as the full listing.
    #[test]
    fn token_and_listing_agree() {
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: plain calls; the handle is owned right after.
        let pid = unsafe { GetCurrentProcessId() };
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        assert!(!raw.is_null());
        // SAFETY: a fresh handle nothing else owns.
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut cache = NameCache::new();
        let by_token = token_owner(&handle, &mut cache).expect("own token readable");
        assert!(by_token.is_some(), "own account has a name");
        let listed = process_owners(&mut cache).remove(&pid).flatten();
        assert_eq!(by_token, listed);
    }

    #[test]
    #[ignore = "prints, for manual checks"]
    fn print_owners() {
        let owners = super::process_owners(&mut Default::default());
        let named = owners.values().filter(|n| n.is_some()).count();
        eprintln!("{} processes, {named} named", owners.len());
        for pid in [4u32, 8748, 3552, 21944] {
            eprintln!("{pid}: {:?}", owners.get(&pid));
        }
    }
}
