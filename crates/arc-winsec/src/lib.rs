//! Windows security queries behind safe functions (ADR-0017).
//!
//! The only crate of the workspace, besides the other FFI crates, that contains `unsafe` code. On
//! targets other than Windows it is empty.

#![cfg(windows)]

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSidToSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{EqualSid, OWNER_SECURITY_INFORMATION, PSID};

/// The administrative owners: the built-in Administrators group, Local System and the
/// TrustedInstaller service.
const ADMINISTRATIVE_SIDS: [&str; 3] = [
    "S-1-5-32-544",
    "S-1-5-18",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
];

/// A block allocated by the system with `LocalAlloc`, freed exactly once when dropped.
struct LocalBlock(*mut c_void);

impl Drop for LocalBlock {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer was allocated by the system through `LocalAlloc` (by
            // `GetNamedSecurityInfoW` or `ConvertStringSidToSidW`), is owned by this guard alone
            // and is freed only here.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// Whether the owner of the file or folder at `path` is the built-in Administrators group
/// (`S-1-5-32-544`), Local System (`S-1-5-18`) or the TrustedInstaller service.
///
/// The call reads the security descriptor of the object the path resolves to: for a symbolic
/// link or other reparse point, the owner of the link's target, not of the link. A caller that
/// must not accept a link checks that separately. An object without an owner is not
/// administrative. The access control list is not examined.
///
/// # Errors
/// The operating system error when the descriptor cannot be read (`NotFound` for a missing path,
/// `PermissionDenied` without read access to the descriptor), and `InvalidInput` when the path
/// contains a NUL character.
pub fn owner_is_administrative(path: &Path) -> io::Result<bool> {
    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    if units.contains(&0) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut name = units;
    name.push(0);

    let mut owner: PSID = ptr::null_mut();
    let mut descriptor: *mut c_void = ptr::null_mut();
    // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call; the owner and
    // descriptor out-pointers point to live locals; the group, DACL and SACL out-pointers are
    // null because their information is not requested. On success `descriptor` is a block
    // allocated by the system that holds the owner SID, and is released by the guard below.
    let status = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    // The function returns the error code itself; `GetLastError` is not set.
    let _descriptor = LocalBlock(descriptor);
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(
            i32::try_from(status).unwrap_or(i32::MAX),
        ));
    }
    if owner.is_null() {
        return Ok(false);
    }
    for text in ADMINISTRATIVE_SIDS {
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        let mut sid: PSID = ptr::null_mut();
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call and `sid`
        // points to a live local; on success `sid` is a block allocated by the system, released
        // by the guard below.
        let converted = unsafe { ConvertStringSidToSidW(wide.as_ptr(), &raw mut sid) };
        if converted == 0 {
            return Err(io::Error::last_os_error());
        }
        let sid = LocalBlock(sid);
        // SAFETY: both pointers are valid SIDs: `owner` points into `descriptor`, which is alive
        // until the end of the function, and `sid.0` is the converted SID.
        if unsafe { EqualSid(owner, sid.0) } != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_folder_is_administrative() {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let system32 = Path::new(&root).join("System32");
        assert!(owner_is_administrative(&system32).unwrap());
    }

    #[test]
    fn a_missing_path_is_an_error() {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let missing = Path::new(&root).join("arc-winsec-no-such-entry");
        let error = owner_is_administrative(&missing).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_path_with_nul_is_refused() {
        let error = owner_is_administrative(Path::new("C:\\a\0b")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
