//! Shows the windows a GlazeWM `wm-exit` left cloaked, so the next GlazeWM manages them and restore moves them
//! instead of launching their programs again. A clean exit skips GlazeWM's watcher cleanup, so every window on an
//! unfocused workspace stays shell-cloaked, unseen and unmanaged.

use std::ffi::c_void;
use std::mem::size_of;
use std::thread;

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    IServiceProvider,
};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, IsWindowVisible};
use windows::core::{BOOL, GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, interface};

use crate::error::Error;
use crate::session::{SavedWindow, Sources};

/// `DWM_CLOAKED_SHELL`, the cloak GlazeWM puts on windows of a hidden workspace.
const CLOAKED_SHELL: u32 = 2;
const CLSID_IMMERSIVE_SHELL: GUID = GUID::from_u128(0xC2F03A33_21F5_47FA_B4BB_156362A2F239);

/// Undocumented shell interface, as GlazeWM declares it in `wm-platform/src/platform_impl/windows/com.rs`. The
/// filler methods keep the vtable layout.
#[interface("1841c6d7-4f9d-42c0-af41-8747538f10e5")]
unsafe trait IApplicationViewCollection: IUnknown {
    unsafe fn m1(&self);
    unsafe fn m2(&self);
    unsafe fn m3(&self);
    unsafe fn get_view_for_hwnd(
        &self,
        window: isize,
        view: *mut Option<IApplicationView>,
    ) -> HRESULT;
}

#[interface("372E1D3B-38D3-42E4-A15B-8AB2B178F513")]
unsafe trait IApplicationView: IUnknown {
    unsafe fn m1(&self);
    unsafe fn m2(&self);
    unsafe fn m3(&self);
    unsafe fn m4(&self);
    unsafe fn m5(&self);
    unsafe fn m6(&self);
    unsafe fn m7(&self);
    unsafe fn m8(&self);
    unsafe fn m9(&self);
    unsafe fn set_cloak(&self, cloak_type: u32, cloak_flag: i32) -> HRESULT;
}

/// A shell-cloaked top-level window and the program behind it.
#[derive(PartialEq, Debug)]
struct Hidden {
    handle: isize,
    executable_path: Option<String>,
}

/// Uncloaks every hidden window GlazeWM does not manage whose program a saved window runs, and returns how many it
/// uncloaked. Other cloaked windows (suspended UWP frames, other virtual desktops) are the shell's and stay hidden.
pub fn adopt(sources: &mut Sources, saved: &[SavedWindow]) -> Result<usize, Error> {
    let managed: Vec<isize> = sources
        .glazewm()
        .workspaces()?
        .iter()
        .flat_map(|workspace| workspace.windows())
        .map(|window| window.handle)
        .collect();
    let mut hidden = Vec::new();
    for handle in shell_cloaked()? {
        hidden.push(Hidden {
            handle,
            executable_path: sources.processes().executable_path_of_window(handle)?,
        });
    }
    let chosen = to_uncloak(&hidden, &managed, saved);
    if chosen.is_empty() {
        return Ok(0);
    }
    let handles: Vec<isize> = chosen.iter().map(|window| window.handle).collect();
    // The wmi crate puts the process in the MTA, and the shell's view collection wants an STA thread.
    thread::spawn(move || uncloak(&handles))
        .join()
        .map_err(|_| Error::ThreadGone { thread: "uncloak" })??;
    for window in &chosen {
        eprintln!(
            "rallypoint: uncloaked window handle {} of {}",
            window.handle,
            window.executable_path.as_deref().unwrap_or_default()
        );
    }
    Ok(chosen.len())
}

fn to_uncloak<'a>(
    hidden: &'a [Hidden],
    managed: &[isize],
    saved: &[SavedWindow],
) -> Vec<&'a Hidden> {
    hidden
        .iter()
        .filter(|window| !managed.contains(&window.handle))
        .filter(|window| {
            window.executable_path.is_some()
                && saved
                    .iter()
                    .any(|saved| saved.executable_path == window.executable_path)
        })
        .collect()
}

/// Every visible top-level window with a shell cloak.
fn shell_cloaked() -> Result<Vec<isize>, Error> {
    let mut handles: Vec<isize> = Vec::new();
    unsafe {
        EnumWindows(
            Some(collect_shell_cloaked),
            LPARAM(&raw mut handles as isize),
        )
    }
    .map_err(|source| Error::Uncloak {
        call: "EnumWindows",
        handle: None,
        source,
    })?;
    Ok(handles)
}

unsafe extern "system" fn collect_shell_cloaked(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let handles = unsafe { &mut *(lparam.0 as *mut Vec<isize>) };
    let mut cloaked: u32 = 0;
    let read = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast::<c_void>(),
            size_of::<u32>() as u32,
        )
    };
    if unsafe { IsWindowVisible(hwnd) }.as_bool() && read.is_ok() && cloaked == CLOAKED_SHELL {
        handles.push(hwnd.0 as isize);
    }
    BOOL(1)
}

/// Clears the shell cloak of each window in `handles`, on the calling thread, which must not be in the MTA yet.
fn uncloak(handles: &[isize]) -> Result<(), Error> {
    let com_error = |call: &'static str, handle: Option<isize>| {
        move |source| Error::Uncloak {
            call,
            handle,
            source,
        }
    };
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .map_err(com_error("CoInitializeEx", None))?;
    let result = (|| {
        let provider: IServiceProvider =
            unsafe { CoCreateInstance(&CLSID_IMMERSIVE_SHELL, None, CLSCTX_ALL) }
                .map_err(com_error("CoCreateInstance(ImmersiveShell)", None))?;
        let views: IApplicationViewCollection =
            unsafe { provider.QueryService(&IApplicationViewCollection::IID) }
                .map_err(com_error("QueryService(IApplicationViewCollection)", None))?;
        for &handle in handles {
            let mut view: Option<IApplicationView> = None;
            unsafe { views.get_view_for_hwnd(handle, &raw mut view) }
                .ok()
                .map_err(com_error("GetViewForHwnd", Some(handle)))?;
            let view = view.ok_or(Error::NoView { handle })?;
            unsafe { view.set_cloak(1, 0) }
                .ok()
                .map_err(com_error("SetCloak", Some(handle)))?;
        }
        Ok(())
    })();
    unsafe { CoUninitialize() };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glazewm::State;

    fn saved(executable_path: &str) -> SavedWindow {
        SavedWindow {
            workspace: "1".to_string(),
            process_name: "app".to_string(),
            executable_path: Some(executable_path.to_string()),
            command_line: None,
            owner: "owner".to_string(),
            title: String::new(),
            class_name: String::new(),
            state: State::Tiling,
            cwd: None,
            panes: Vec::new(),
            tabs: Vec::new(),
        }
    }

    fn hidden(handle: isize, executable_path: Option<&str>) -> Hidden {
        Hidden {
            handle,
            executable_path: executable_path.map(str::to_string),
        }
    }

    #[test]
    fn to_uncloak_takes_unmanaged_windows_of_saved_programs_only() {
        let windows = vec![
            hidden(1, Some(r"C:\wezterm-gui.exe")),
            hidden(2, Some(r"C:\wezterm-gui.exe")),
            hidden(3, Some(r"C:\tools\other.exe")),
            hidden(4, None),
        ];
        let saved = vec![saved(r"C:\wezterm-gui.exe")];
        let handles: Vec<isize> = to_uncloak(&windows, &[2], &saved)
            .iter()
            .map(|window| window.handle)
            .collect();
        assert_eq!(handles, vec![1]);
    }

    #[test]
    fn to_uncloak_never_matches_a_protected_process() {
        let protected = SavedWindow {
            executable_path: None,
            ..saved("")
        };
        assert!(to_uncloak(&[hidden(1, None)], &[], &[protected]).is_empty());
    }
}
