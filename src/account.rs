//! The Windows accounts behind windows: the one rallypoint runs as, and the profile folder of any owner.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{LookupAccountNameW, PSID, SID_NAME_USE};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::WindowsProgramming::GetUserNameW;
use windows::core::{PCWSTR, PWSTR, w};

use crate::error::Error;
use crate::model::Owner;

const PROFILE_LIST: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList";
/// `UNLEN + 1`, the longest account name with its terminating null.
const NAME_CAPACITY: usize = 257;

/// The account rallypoint runs as.
pub fn current_user() -> Result<Owner, Error> {
    let mut name = [0u16; NAME_CAPACITY];
    let mut length = NAME_CAPACITY as u32;
    // SAFETY: `length` holds the capacity of `name`, which bounds the write.
    unsafe { GetUserNameW(Some(PWSTR(name.as_mut_ptr())), &mut length) }.map_err(|source| {
        Error::Os {
            call: "GetUserNameW",
            context: "reading the name of the account rallypoint runs as".to_string(),
            source,
        }
    })?;
    // The length counts the terminating null.
    let name = name
        .get(..(length as usize).saturating_sub(1))
        .unwrap_or_default();
    String::from_utf16(name)
        .map(Owner::new)
        .map_err(Error::AccountName)
}

/// The profile folder of `owner`, from its SID's entry in the registry profile list, so no user folder is assumed.
pub fn home_of(owner: &Owner) -> Result<PathBuf, Error> {
    let os_error = |call: &'static str| {
        move |source| Error::Os {
            call,
            context: format!("finding the profile folder of {owner}"),
            source,
        }
    };
    let subkey: Vec<u16> = PROFILE_LIST
        .encode_utf16()
        .chain([u16::from(b'\\')])
        .chain(sid_of(owner)?)
        .chain([0])
        .collect();
    let mut size: u32 = 0;
    // SAFETY: no data buffer, so the call only writes the value's size to `size`.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            w!("ProfileImagePath"),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    }
    .ok()
    .map_err(os_error("RegGetValueW"))?;
    let mut path = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: `path` holds `size` bytes, the size the first call reported, which bounds the write.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            w!("ProfileImagePath"),
            RRF_RT_REG_SZ,
            None,
            Some(path.as_mut_ptr().cast()),
            Some(&mut size),
        )
    }
    .ok()
    .map_err(os_error("RegGetValueW"))?;
    let end = path
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(path.len());
    Ok(PathBuf::from(OsString::from_wide(
        path.get(..end).unwrap_or_default(),
    )))
}

/// The SID of `owner` in string form, `S-1-5-21-...`, as UTF-16 without a terminating null.
fn sid_of(owner: &Owner) -> Result<Vec<u16>, Error> {
    let os_error = |call: &'static str| {
        move |source| Error::Os {
            call,
            context: format!("finding the SID of {owner}"),
            source,
        }
    };
    let name = wide(owner.as_str());
    let mut sid_size: u32 = 0;
    let mut domain_size: u32 = 0;
    let mut usage = SID_NAME_USE::default();
    // SAFETY: no buffers, so the call only writes the sizes it needs to the two locals.
    let sized = unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            PCWSTR(name.as_ptr()),
            None,
            &mut sid_size,
            None,
            &mut domain_size,
            &mut usage,
        )
    };
    if let Err(error) = sized
        && error.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult()
    {
        return Err(os_error("LookupAccountNameW")(error));
    }
    let mut sid = vec![0u8; sid_size as usize];
    let mut domain = vec![0u16; domain_size as usize];
    // SAFETY: each buffer holds the size the first call asked for, and the sizes are passed with them.
    unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            PCWSTR(name.as_ptr()),
            Some(PSID(sid.as_mut_ptr().cast())),
            &mut sid_size,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_size,
            &mut usage,
        )
    }
    .map_err(os_error("LookupAccountNameW"))?;
    let mut text = PWSTR::null();
    // SAFETY: `sid` holds the SID LookupAccountNameW wrote, and `text` receives a LocalAlloc string freed below.
    unsafe { ConvertSidToStringSidW(PSID(sid.as_mut_ptr().cast()), &mut text) }
        .map_err(os_error("ConvertSidToStringSidW"))?;
    // SAFETY: `text` is the null-terminated string ConvertSidToStringSidW returned, copied before it is freed.
    let converted = unsafe { text.as_wide() }.to_vec();
    // SAFETY: `text` came from LocalAlloc and is not used after this.
    unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    Ok(converted)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_of_the_current_user_is_its_profile() -> Result<(), Box<dyn std::error::Error>> {
        let profile = std::env::var_os("USERPROFILE").ok_or("USERPROFILE is not set")?;
        assert_eq!(home_of(&current_user()?)?, PathBuf::from(profile));
        Ok(())
    }
}
