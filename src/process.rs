//! The process behind a window. WMI answers for processes of other accounts, where `OpenProcess` from an
//! unelevated token is denied, and the agent account's windows are such processes.

use serde::Deserialize;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use wmi::WMIConnection;

use crate::error::Error;

pub struct Process {
    pub pid: u32,
    /// None for a protected process.
    pub executable_path: Option<String>,
    pub command_line: Option<String>,
    pub owner: String,
}

#[derive(Deserialize)]
#[serde(rename = "Win32_Process", rename_all = "PascalCase")]
struct Win32Process {
    #[serde(rename = "__Path")]
    path: String,
    executable_path: Option<String>,
    command_line: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct GetOwner {
    return_value: u32,
    user: Option<String>,
}

pub struct Processes {
    wmi: WMIConnection,
}

impl Processes {
    pub fn connect() -> Result<Processes, Error> {
        Ok(Processes {
            wmi: WMIConnection::new().map_err(Error::WmiConnect)?,
        })
    }

    pub fn of_window(&self, handle: isize) -> Result<Process, Error> {
        let (pid, process) = self.win32_process(handle)?;
        let owner: GetOwner = self
            .wmi
            .exec_instance_method::<Win32Process, _>(&process.path, "GetOwner", ())
            .map_err(|source| Error::Wmi {
                query: format!("GetOwner of {}", process.path),
                source,
            })?;
        let user = match (owner.return_value, owner.user) {
            (0, Some(user)) => user,
            (code, _) => return Err(Error::Owner { pid, code }),
        };
        Ok(Process {
            pid,
            executable_path: process.executable_path,
            command_line: process.command_line,
            owner: user,
        })
    }

    /// The executable of the process behind `handle`, without its owner: `GetOwner` fails for system processes,
    /// which own some of the windows this is asked about. None for a protected process.
    pub fn executable_path_of_window(&self, handle: isize) -> Result<Option<String>, Error> {
        Ok(self.win32_process(handle)?.1.executable_path)
    }

    fn win32_process(&self, handle: isize) -> Result<(u32, Win32Process), Error> {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(HWND(handle as _), Some(&mut pid)) };
        if pid == 0 {
            return Err(Error::NoProcess { handle });
        }
        let query = format!("SELECT * FROM Win32_Process WHERE ProcessId = {pid}");
        let found: Vec<Win32Process> = self.wmi.raw_query(&query).map_err(|source| Error::Wmi {
            query: query.clone(),
            source,
        })?;
        let process = found
            .into_iter()
            .next()
            .ok_or(Error::ProcessGone { pid, handle })?;
        Ok((pid, process))
    }
}
