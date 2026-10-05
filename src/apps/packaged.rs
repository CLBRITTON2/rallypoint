//! Store apps. Their executables sit under WindowsApps and refuse a direct start, so each is saved by its AUMID and
//! reopened through the shell's `shell:AppsFolder`, the way the Start menu opens it.

use windows::Win32::Storage::Packaging::Appx::{
    APPLICATION_USER_MODEL_ID_MAX_LENGTH, GetApplicationUserModelId,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows_core::{Owned, PWSTR};

use crate::error::Error;
use crate::model::{ExePath, Owner};
use crate::plan::{Key, Launch, Start};

const LAUNCHER: &str = "explorer.exe";

/// The AUMID of packaged process `pid`, such as `Microsoft.WindowsNotepad_8wekyb3d8bbwe!App`.
pub fn aumid_of(pid: u32) -> Result<String, Error> {
    let os_error = |call: &'static str| {
        move |source| Error::Os {
            call,
            context: format!("reading the AUMID of process {pid}"),
            source,
        }
    };
    // SAFETY: plain call with flags and a pid, returning an owned handle wrapped below.
    let opened = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(os_error("OpenProcess"))?;
    // SAFETY: OpenProcess returned this handle, and nothing else closes it.
    let process = unsafe { Owned::new(opened) };
    let mut length = APPLICATION_USER_MODEL_ID_MAX_LENGTH;
    let mut wide = vec![0u16; length as usize];
    // SAFETY: `wide` holds `length` UTF-16 units, which bounds the write.
    unsafe { GetApplicationUserModelId(*process, &mut length, Some(PWSTR(wide.as_mut_ptr()))) }
        .ok()
        .map_err(os_error("GetApplicationUserModelId"))?;
    // `length` counts the terminating null.
    wide.truncate(length.saturating_sub(1) as usize);
    String::from_utf16(&wide).map_err(|source| Error::Aumid { pid, source })
}

/// The key of a Store app window, by who runs it and its app. Its windows are told apart only by order.
pub fn key(owner: &Owner, aumid: &str) -> Key {
    Key::Packaged {
        owner: owner.clone(),
        aumid: aumid.to_string(),
    }
}

/// Starts the Store app `aumid`.
pub fn launch(owner: &Owner, aumid: &str) -> Launch {
    Launch {
        owner: owner.clone(),
        program: ExePath::new(LAUNCHER.to_string()),
        arguments: format!(r"shell:AppsFolder\{aumid}"),
        start: Start::Detached,
    }
}
