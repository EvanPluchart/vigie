//! Who each process runs as, for the User name column (decision 0010). The Terminal Services
//! API names the owner of every process, services included, without opening any of them.

use std::collections::HashMap;

use windows_sys::Win32::Security::PSID;
use windows_sys::Win32::Security::{GetLengthSid, IsValidSid, LookupAccountSidW, SID_NAME_USE};
use windows_sys::Win32::System::RemoteDesktop::{
    WTS_CURRENT_SERVER_HANDLE, WTS_PROCESS_INFOW, WTSEnumerateProcessesW, WTSFreeMemory,
};

use crate::Pid;

/// The account name of every process, by PID. `cache` keeps names by SID between calls, so
/// each account is looked up once.
pub(crate) fn process_owners(
    cache: &mut HashMap<Vec<u8>, Option<String>>,
) -> HashMap<Pid, Option<String>> {
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
        // SAFETY: valid SID; its length bounds the copy.
        let bytes =
            unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), GetLengthSid(sid) as usize) }
                .to_vec();
        let name = cache
            .entry(bytes)
            .or_insert_with(|| account_name(sid))
            .clone();
        owners.insert(entry.ProcessId, name);
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
