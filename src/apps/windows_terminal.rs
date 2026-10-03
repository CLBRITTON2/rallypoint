//! Windows Terminal windows: their tabs, and the launch that reopens one. Each tab's pseudo console has a hidden
//! top-level window owned by the Windows Terminal window holding the tab, and that window belongs to the tab's shell
//! process.

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetClassNameW, GetWindow, GetWindowThreadProcessId,
};
use windows::core::BOOL;

use crate::apps::{self, Kind, quoted};
use crate::cwd;
use crate::error::Error;
use crate::model::Pane;
use crate::process::Processes;

pub const PROCESS_NAME: &str = "WindowsTerminal";
const PSEUDO_CONSOLE: &str = "PseudoConsoleWindow";

/// A tab's shell and the Windows Terminal window it sits in.
#[derive(PartialEq, Debug)]
pub struct Tab {
    pub window: isize,
    pub pid: u32,
}

/// The tabs of the Windows Terminal window `handle`, among `tab_shells`, oldest first.
pub fn saved_tabs(
    processes: &Processes,
    tab_shells: &[Tab],
    handle: isize,
) -> Result<Vec<Pane>, Error> {
    let pids: Vec<u32> = tab_shells
        .iter()
        .filter(|tab| tab.window == handle)
        .map(|tab| tab.pid)
        .collect();
    processes
        .started(&pids)?
        .into_iter()
        .map(|process| {
            let is_shell_tab = process
                .executable_path
                .as_deref()
                .is_some_and(|path| apps::kind_of(&apps::program_name(path)) == Kind::Shell);
            Ok(Pane {
                cwd: match is_shell_tab {
                    true => cwd::of_process(process.process_id)?,
                    false => None,
                },
                program: process.executable_path,
                command_line: process.command_line,
                resume: None,
            })
        })
        .collect()
}

/// wt.exe arguments opening one new window holding `tabs`. A shell tab runs its program alone, so a tab opened to run
/// one command does not run it again. A tab with neither program nor command line is left out.
pub fn launch_arguments(tabs: &[Pane]) -> String {
    let opened: Vec<String> = tabs
        .iter()
        .filter_map(|tab| {
            let path = tab.program.as_deref();
            let command = match (path, tab.command_line.as_deref()) {
                (Some(path), _) if apps::kind_of(&apps::program_name(path)) == Kind::Shell => {
                    format!("\"{path}\"")
                }
                (_, Some(command_line)) => command_line.to_string(),
                (Some(path), None) => format!("\"{path}\""),
                (None, None) => return None,
            };
            let folder = tab
                .cwd
                .as_deref()
                .map(|cwd| format!("-d {} ", quoted(cwd)))
                .unwrap_or_default();
            // wt.exe splits its arguments into subcommands at every bare ;
            Some(format!("new-tab {folder}{command}").replace(';', r"\;"))
        })
        .collect();
    match opened.is_empty() {
        true => "-w new".to_string(),
        false => format!("-w new {}", opened.join(" ; ")),
    }
}

/// Every pseudo console window owned by another window, so the consoles of other hosts are left out.
pub fn tab_shells() -> Result<Vec<Tab>, Error> {
    let mut tabs: Vec<Tab> = Vec::new();
    unsafe { EnumWindows(Some(collect_tab), LPARAM(&raw mut tabs as isize)) }
        .map_err(Error::Tabs)?;
    Ok(tabs)
}

unsafe extern "system" fn collect_tab(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let tabs = unsafe { &mut *(lparam.0 as *mut Vec<Tab>) };
    let mut class = [0u16; 64];
    let length = unsafe { GetClassNameW(hwnd, &mut class) };
    let is_pseudo_console = class
        .get(..usize::try_from(length).unwrap_or_default())
        .is_some_and(|name| String::from_utf16_lossy(name) == PSEUDO_CONSOLE);
    if !is_pseudo_console {
        return BOOL(1);
    }
    let Ok(owner) = (unsafe { GetWindow(hwnd, GW_OWNER) }) else {
        return BOOL(1);
    };
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid != 0 {
        tabs.push(Tab {
            window: owner.0 as isize,
            pid,
        });
    }
    BOOL(1)
}
