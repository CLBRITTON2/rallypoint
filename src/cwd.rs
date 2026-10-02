//! The working directory of another process, read from the process parameters in its memory. cmd's `cd` moves it,
//! but PowerShell's `Set-Location` leaves it alone, so a pwsh shell reports the folder it started in.

use std::ffi::OsString;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
use windows::Win32::Foundation::{E_ACCESSDENIED, HANDLE};
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Threading::{
    OpenProcess, PEB, PROCESS_BASIC_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};
use windows_core::Owned;

use crate::error::Error;

/// Offset of `CurrentDirectory.DosPath` in a 64-bit `RTL_USER_PROCESS_PARAMETERS`, inside the part the SDK declares
/// as reserved.
/// https://www.geoffchappell.com/studies/windows/km/ntoskrnl/inc/api/pebteb/rtl_user_process_parameters.htm
const CURRENT_DIRECTORY: usize = 0x38;

/// A `UNICODE_STRING` as it sits in the other process, its buffer an address there.
#[repr(C)]
#[derive(Default)]
struct RemoteString {
    length: u16,
    maximum_length: u16,
    buffer: usize,
}

/// The working directory of process `pid`, or None when this account may not read it: an elevated process, or one
/// of another account.
pub fn of_process(pid: u32) -> Result<Option<PathBuf>, Error> {
    let opened = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
            false,
            pid,
        )
    };
    let process = match opened {
        Ok(handle) => unsafe { Owned::new(handle) },
        Err(error) if error.code() == E_ACCESSDENIED => return Ok(None),
        Err(source) => {
            return Err(Error::Cwd {
                pid,
                call: "OpenProcess",
                source,
            });
        }
    };
    let mut basic = PROCESS_BASIC_INFORMATION::default();
    unsafe {
        NtQueryInformationProcess(
            *process,
            ProcessBasicInformation,
            (&raw mut basic).cast(),
            size_of::<PROCESS_BASIC_INFORMATION>() as u32,
            std::ptr::null_mut(),
        )
    }
    .ok()
    .map_err(|source| Error::Cwd {
        pid,
        call: "NtQueryInformationProcess",
        source,
    })?;
    let parameters: usize = read(
        *process,
        pid,
        basic.PebBaseAddress as usize + offset_of!(PEB, ProcessParameters),
    )?;
    let dos_path: RemoteString = read(*process, pid, parameters + CURRENT_DIRECTORY)?;
    let mut wide = vec![0u16; usize::from(dos_path.length) / 2];
    unsafe {
        ReadProcessMemory(
            *process,
            dos_path.buffer as *const _,
            wide.as_mut_ptr().cast(),
            usize::from(dos_path.length),
            None,
        )
    }
    .map_err(|source| Error::Cwd {
        pid,
        call: "ReadProcessMemory",
        source,
    })?;
    Ok(Some(folder(&OsString::from_wide(&wide).to_string_lossy())))
}

fn read<T: Default>(process: HANDLE, pid: u32, address: usize) -> Result<T, Error> {
    let mut value = T::default();
    unsafe {
        ReadProcessMemory(
            process,
            address as *const _,
            (&raw mut value).cast(),
            size_of::<T>(),
            None,
        )
    }
    .map_err(|source| Error::Cwd {
        pid,
        call: "ReadProcessMemory",
        source,
    })?;
    Ok(value)
}

/// A DOS path without the trailing backslash Windows keeps on a working directory, except a drive root's, since
/// `C:` alone means that drive's current folder.
fn folder(dos_path: &str) -> PathBuf {
    match dos_path.strip_suffix('\\') {
        Some(trimmed) if !trimmed.ends_with(':') => PathBuf::from(trimmed),
        _ => PathBuf::from(dos_path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_drops_the_trailing_backslash_but_a_root_keeps_it() {
        assert_eq!(folder(r"C:\Users\x\zet\"), PathBuf::from(r"C:\Users\x\zet"));
        assert_eq!(folder(r"C:\"), PathBuf::from(r"C:\"));
        assert_eq!(
            folder(r"\\server\share\x"),
            PathBuf::from(r"\\server\share\x")
        );
    }

    #[test]
    fn of_process_reads_this_process() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            of_process(std::process::id())?,
            Some(std::env::current_dir()?)
        );
        Ok(())
    }
}
