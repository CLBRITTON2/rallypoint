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

#[cfg(not(target_pointer_width = "64"))]
compile_error!(
    "cwd reads the 64-bit process parameters layout, so rallypoint builds for 64-bit only"
);

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

/// A type [`read`] may fill from another process's memory.
///
/// # Safety
///
/// Every bit pattern of `size_of::<Self>()` bytes must be a valid `Self`: no references, enums, bools or niches.
unsafe trait PlainData: Default {}

// SAFETY: any bytes are a valid usize.
unsafe impl PlainData for usize {}

// SAFETY: two u16 and a usize, each valid for any bytes, and padding holds no value.
unsafe impl PlainData for RemoteString {}

/// The working directory of process `pid`, or None when this account may not read it: an elevated process, or one
/// of another account.
pub fn of_process(pid: u32) -> Result<Option<PathBuf>, Error> {
    // SAFETY: plain call with flags and a pid, returning an owned handle wrapped below.
    let opened = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
            false,
            pid,
        )
    };
    let process = match opened {
        // SAFETY: OpenProcess returned this handle, and nothing else closes it.
        Ok(handle) => unsafe { Owned::new(handle) },
        Err(error) if error.code() == E_ACCESSDENIED => return Ok(None),
        Err(source) => return Err(cwd_error(pid, "OpenProcess")(source)),
    };
    let mut basic = PROCESS_BASIC_INFORMATION::default();
    // SAFETY: `basic` is a live local of the size passed, which bounds the write.
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
    .map_err(cwd_error(pid, "NtQueryInformationProcess"))?;
    let parameters: usize = read(
        *process,
        pid,
        basic.PebBaseAddress as usize + offset_of!(PEB, ProcessParameters),
    )?;
    let dos_path: RemoteString = read(*process, pid, parameters + CURRENT_DIRECTORY)?;
    let mut wide = vec![0u16; usize::from(dos_path.length) / 2];
    // SAFETY: `wide` holds `length` bytes, which bounds the write. The source address is in the other process.
    unsafe {
        ReadProcessMemory(
            *process,
            dos_path.buffer as *const _,
            wide.as_mut_ptr().cast(),
            usize::from(dos_path.length),
            None,
        )
    }
    .map_err(cwd_error(pid, "ReadProcessMemory"))?;
    Ok(Some(folder(&wide)))
}

/// Reads a `T` at `address` in `process`.
fn read<T: PlainData>(process: HANDLE, pid: u32, address: usize) -> Result<T, Error> {
    let mut value = T::default();
    // SAFETY: `value` is a live local of `size_of::<T>()` bytes, which bounds the write, and `PlainData` makes any
    // bytes written a valid `T`.
    unsafe {
        ReadProcessMemory(
            process,
            address as *const _,
            (&raw mut value).cast(),
            size_of::<T>(),
            None,
        )
    }
    .map_err(cwd_error(pid, "ReadProcessMemory"))?;
    Ok(value)
}

fn cwd_error(pid: u32, call: &'static str) -> impl FnOnce(windows::core::Error) -> Error {
    move |source| Error::Os {
        call,
        context: format!("reading the working directory of process {pid}"),
        source,
    }
}

/// A UTF-16 DOS path without the trailing backslash Windows keeps on a working directory, except a drive root's,
/// since `C:` alone means that drive's current folder.
fn folder(dos_path: &[u16]) -> PathBuf {
    let backslash = u16::from(b'\\');
    let colon = u16::from(b':');
    match dos_path.split_last() {
        Some((&last, trimmed)) if last == backslash && trimmed.last() != Some(&colon) => {
            PathBuf::from(OsString::from_wide(trimmed))
        }
        _ => PathBuf::from(OsString::from_wide(dos_path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder_of(dos_path: &str) -> PathBuf {
        folder(&dos_path.encode_utf16().collect::<Vec<u16>>())
    }

    #[test]
    fn folder_drops_the_trailing_backslash_but_a_root_keeps_it() {
        assert_eq!(
            folder_of(r"C:\work\project\"),
            PathBuf::from(r"C:\work\project")
        );
        assert_eq!(folder_of(r"C:\"), PathBuf::from(r"C:\"));
        assert_eq!(
            folder_of(r"\\server\share\x"),
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
