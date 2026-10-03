//! Starts the programs a restore plans, as their owner, without tying them to rallypoint's console or output.

use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use windows::Win32::Foundation::{HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation};
use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
use windows::Win32::System::Threading::{
    CREATE_NEW_CONSOLE, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows_core::{Owned, PCWSTR, PWSTR};

use crate::error::Error;
use crate::model::Owner;
use crate::plan::{Launch, Start};

/// Makes rallypoint's stdout and stderr non-inheritable. `Command` spawns with handle inheritance on, so a launched
/// app would otherwise hold a pipe handed to rallypoint open for as long as it runs, and a shell redirecting the
/// report (the startup `restore *> log; watch`) would wait on it before starting `watch`.
pub fn keep_output_from_launches() -> Result<(), Error> {
    let inherit_error = |call: &'static str| {
        move |source| Error::Os {
            call,
            context: "keeping rallypoint's output from launched programs".to_string(),
            source,
        }
    };
    for id in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: plain call with a standard handle id. The handle it returns is borrowed, never closed.
        let handle = unsafe { GetStdHandle(id) }.map_err(inherit_error("GetStdHandle"))?;
        // No handle to leak when rallypoint runs without that stream.
        if handle.is_invalid() {
            continue;
        }
        // SAFETY: `handle` is this process's live standard handle, checked valid above.
        unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) }
            .map_err(inherit_error("SetHandleInformation"))?;
    }
    Ok(())
}

/// Starts `launch` without waiting for it. `user` is the account rallypoint runs as, so another owner's launch goes
/// through `runas`.
pub fn spawn(launch: &Launch, user: &Owner) -> Result<(), Error> {
    if launch.owner != *user {
        return spawn_as(launch);
    }
    match &launch.start {
        Start::Detached => spawn_detached(launch),
        Start::Console { cwd } => spawn_console(launch, cwd.as_deref()),
    }
}

fn spawn_detached(launch: &Launch) -> Result<(), Error> {
    // Electron apps log to an inherited console, which would bury the report.
    Command::new(&launch.program)
        .raw_arg(&launch.arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|source| Error::Launch {
            command_line: launch.command_line(),
            source,
        })
}

/// Opens `launch` in a new console. `Command` always hands the child standard handles, so a shell started through it
/// would read and write rallypoint's (hidden at startup) instead of its own console.
fn spawn_console(launch: &Launch, cwd: Option<&Path>) -> Result<(), Error> {
    let command_line = launch.command_line();
    let mut wide_command_line: Vec<u16> = command_line.encode_utf16().chain([0]).collect();
    let wide_cwd: Option<Vec<u16>> =
        cwd.map(|cwd| cwd.as_os_str().encode_wide().chain([0]).collect());
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut started = PROCESS_INFORMATION::default();
    // SAFETY: the command line is a mutable null-terminated buffer, as CreateProcessW requires, and the folder is
    // null-terminated. Every buffer outlives the call.
    unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(wide_command_line.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NEW_CONSOLE,
            None,
            wide_cwd
                .as_ref()
                .map_or(PCWSTR::null(), |wide_cwd| PCWSTR(wide_cwd.as_ptr())),
            &startup,
            &mut started,
        )
    }
    .map_err(|source| Error::Launch {
        command_line,
        source: source.into(),
    })?;
    // SAFETY: CreateProcessW returned this handle, and nothing else closes it.
    drop(unsafe { Owned::new(started.hProcess) });
    // SAFETY: CreateProcessW returned this handle, and nothing else closes it.
    drop(unsafe { Owned::new(started.hThread) });
    Ok(())
}

/// Starts another owner's program through `runas /savecred`, which opens a console program in a console of its own
/// but cannot set its folder. It is waited for, because it reports a missing saved credential only in its exit code
/// and output.
fn spawn_as(launch: &Launch) -> Result<(), Error> {
    let command_line = launch.command_line();
    let output = Command::new("runas")
        .arg(format!("/user:{}", launch.owner))
        .arg("/savecred")
        .arg(&command_line)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| Error::Spawn {
            program: "runas",
            source,
        })?;
    if !output.status.success() {
        return Err(Error::Runas {
            owner: launch.owner.clone(),
            command_line,
            code: output.status.code(),
            output: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        });
    }
    Ok(())
}
