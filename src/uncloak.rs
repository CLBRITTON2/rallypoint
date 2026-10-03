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

use crate::capture::Sources;
use crate::error::Error;
use crate::model::{ExePath, SavedWindow};

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
    executable_path: Option<ExePath>,
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
        .map_err(|_panic| Error::ThreadPanicked { thread: "uncloak" })??;
    for window in &chosen {
        eprintln!(
            "rallypoint: uncloaked window handle {} of {}",
            window.handle,
            window
                .executable_path
                .as_ref()
                .map(ExePath::as_str)
                .unwrap_or_default()
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
    // SAFETY: `collect_shell_cloaked` reads lparam as the `handles` it points to, which outlives the call.
    unsafe {
        EnumWindows(
            Some(collect_shell_cloaked),
            LPARAM(&raw mut handles as isize),
        )
    }
    .map_err(|source| Error::Os {
        call: "EnumWindows",
        context: "finding the cloaked windows".to_string(),
        source,
    })?;
    Ok(handles)
}

unsafe extern "system" fn collect_shell_cloaked(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `shell_cloaked` passes a live `Vec<isize>` as lparam, and EnumWindows calls back on its thread.
    let handles = unsafe { &mut *(lparam.0 as *mut Vec<isize>) };
    let mut cloaked: u32 = 0;
    // SAFETY: `cloaked` is a live local of the size passed, which bounds the write.
    let read = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast::<c_void>(),
            size_of::<u32>() as u32,
        )
    };
    // SAFETY: takes the handle EnumWindows passed, by value.
    let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
    if visible && read.is_ok() && cloaked == CLOAKED_SHELL {
        handles.push(hwnd.0 as isize);
    }
    BOOL(1)
}

/// Clears the shell cloak of each window in `handles`, on the calling thread, which must not be in the MTA yet.
fn uncloak(handles: &[isize]) -> Result<(), Error> {
    let com_error = |call: &'static str, context: String| {
        move |source| Error::Os {
            call,
            context,
            source,
        }
    };
    let setup = || "opening the shell's application views".to_string();
    let of_window = |handle: isize| format!("uncloaking window handle {handle}");
    // SAFETY: the caller runs this on a thread with no apartment yet, uninitialized below.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .map_err(com_error("CoInitializeEx", setup()))?;
    let result = (|| {
        // SAFETY: COM is initialized on this thread, and the CLSID is a static.
        let provider: IServiceProvider =
            unsafe { CoCreateInstance(&CLSID_IMMERSIVE_SHELL, None, CLSCTX_ALL) }
                .map_err(com_error("CoCreateInstance(ImmersiveShell)", setup()))?;
        // SAFETY: `provider` is a live interface, and the IID matches the interface asked for.
        let views: IApplicationViewCollection =
            unsafe { provider.QueryService(&IApplicationViewCollection::IID) }.map_err(
                com_error("QueryService(IApplicationViewCollection)", setup()),
            )?;
        for &handle in handles {
            let mut view: Option<IApplicationView> = None;
            // SAFETY: the vtable slot matches GlazeWM's declaration, and `view` is a live local it writes.
            unsafe { views.get_view_for_hwnd(handle, &raw mut view) }
                .ok()
                .map_err(com_error("GetViewForHwnd", of_window(handle)))?;
            let view = view.ok_or(Error::NoView { handle })?;
            // SAFETY: the vtable slot matches GlazeWM's declaration, and `view` is a live interface.
            unsafe { view.set_cloak(1, 0) }
                .ok()
                .map_err(com_error("SetCloak", of_window(handle)))?;
        }
        Ok(())
    })();
    // SAFETY: pairs the CoInitializeEx above, after every interface on this thread is dropped.
    unsafe { CoUninitialize() };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    fn hidden(handle: isize, executable_path: Option<&str>) -> Hidden {
        Hidden {
            handle,
            executable_path: executable_path.map(|path| ExePath::new(path.to_string())),
        }
    }

    #[test]
    fn to_uncloak_takes_unmanaged_windows_of_saved_programs_only() {
        let windows = vec![
            hidden(1, Some(r"C:\tools\app.exe")),
            hidden(2, Some(r"C:\tools\app.exe")),
            hidden(3, Some(r"C:\tools\other.exe")),
            hidden(4, None),
        ];
        let saved = vec![fixtures::window("app", Some(r"C:\tools\app.exe"))];
        let handles: Vec<isize> = to_uncloak(&windows, &[2], &saved)
            .iter()
            .map(|window| window.handle)
            .collect();
        assert_eq!(handles, vec![1]);
    }

    #[test]
    fn to_uncloak_never_matches_a_protected_process() {
        let protected = fixtures::window("app", None);
        assert!(to_uncloak(&[hidden(1, None)], &[], &[protected]).is_empty());
    }
}
