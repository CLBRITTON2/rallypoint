//! The shells behind Windows Terminal tabs. Each tab's pseudo console has a hidden top-level window owned by the
//! Windows Terminal window holding the tab, and that window belongs to the tab's shell process.

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetClassNameW, GetWindow, GetWindowThreadProcessId,
};
use windows::core::BOOL;

use crate::error::Error;

const PSEUDO_CONSOLE: &str = "PseudoConsoleWindow";

/// A tab's shell and the Windows Terminal window it sits in.
#[derive(PartialEq, Debug)]
pub struct Tab {
    pub window: isize,
    pub pid: u32,
}

/// Every pseudo console window owned by another window, so the consoles of other hosts are left out.
pub fn all() -> Result<Vec<Tab>, Error> {
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
