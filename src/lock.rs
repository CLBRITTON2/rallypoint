//! Keeps a second `rallypoint watch` from starting while one runs, under any account.

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError, HANDLE,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::w;

use crate::error::Error;

/// Held for as long as `watch` runs. Windows drops the mutex when the last handle closes, killed process included.
pub struct WatchLock(HANDLE);

/// Takes the lock, or fails with `Error::WatchRunning` when another `watch` holds it.
pub fn take() -> Result<WatchLock, Error> {
    // Global, so a watch of the agent account and one of the owner see the same mutex.
    // SAFETY: no security attributes, and the name is a static string.
    let created = unsafe { CreateMutexW(None, false, w!("Global\\rallypoint-watch")) };
    match created {
        // SAFETY: read straight after the call that set it.
        Ok(handle) if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS => {
            drop(WatchLock(handle));
            Err(Error::WatchRunning)
        }
        Ok(handle) => Ok(WatchLock(handle)),
        // Another account's mutex, whose default security admits only that account.
        Err(error) if error.code() == ERROR_ACCESS_DENIED.to_hresult() => Err(Error::WatchRunning),
        Err(source) => Err(Error::Os {
            call: "CreateMutexW",
            context: "taking the watch lock".to_string(),
            source,
        }),
    }
}

impl Drop for WatchLock {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateMutexW and is closed only here.
        if let Err(error) = unsafe { CloseHandle(self.0) } {
            eprintln!("rallypoint: closing the watch lock failed: {error}");
        }
    }
}
