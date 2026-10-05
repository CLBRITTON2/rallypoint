//! The notification area icon `watch` shows while it runs, with a menu to save at once or stop watching.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION,
    NOTIFY_ICON_MESSAGE, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, NOTIFYICONDATAW_0,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, HMENU, IDI_APPLICATION, LoadIconW, MF_STRING,
    PostMessageW, SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TrackPopupMenu, WM_APP, WM_NULL,
};
use windows::core::w;

use crate::error::Error;

/// The message the icon sends its window on input, the event in the low word of its `lparam`.
pub const CALLBACK: u32 = WM_APP + 1;
const ID: u32 = 1;
const TIP: &str = "rallypoint watch";
const SAVE_NOW: usize = 1;
const QUIT: usize = 2;

pub enum Choice {
    SaveNow,
    Quit,
}

/// Shows the icon on `window`. A restarted Explorer forgets every icon, so this runs again on `TaskbarCreated`.
pub fn add(window: HWND) -> Result<(), Error> {
    // SAFETY: a stock icon id with no instance loads a shared icon, which is never freed.
    let icon = unsafe { LoadIconW(None, IDI_APPLICATION) }.map_err(|source| Error::Os {
        call: "LoadIconW",
        context: "loading the tray icon".to_string(),
        source,
    })?;
    let data = NOTIFYICONDATAW {
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: CALLBACK,
        hIcon: icon,
        szTip: tip(),
        // Version 4 sends WM_CONTEXTMENU for a right click or the menu key, with the point in `wparam`.
        Anonymous: NOTIFYICONDATAW_0 {
            uVersion: NOTIFYICON_VERSION_4,
        },
        ..identity(window)
    };
    notify(NIM_ADD, "NIM_ADD", &data)?;
    notify(NIM_SETVERSION, "NIM_SETVERSION", &data)
}

pub fn remove(window: HWND) -> Result<(), Error> {
    notify(NIM_DELETE, "NIM_DELETE", &identity(window))
}

/// Shows the menu at the screen point a version 4 icon packs into `wparam`. None when nothing was picked.
pub fn choose(window: HWND, wparam: WPARAM) -> Result<Option<Choice>, Error> {
    let (x, y) = point_of(wparam);
    // SAFETY: takes no arguments.
    let menu = unsafe { CreatePopupMenu() }.map_err(menu_error("CreatePopupMenu"))?;
    let picked = pick(window, menu, x, y);
    // SAFETY: `menu` was created above and is no longer shown.
    unsafe { DestroyMenu(menu) }.map_err(menu_error("DestroyMenu"))?;
    picked
}

/// The signed x and y in the low and high words of `wparam`, negative on a monitor left of or above the primary.
fn point_of(wparam: WPARAM) -> (i32, i32) {
    (
        i32::from(wparam.0 as u16 as i16),
        i32::from((wparam.0 >> 16) as u16 as i16),
    )
}

fn pick(window: HWND, menu: HMENU, x: i32, y: i32) -> Result<Option<Choice>, Error> {
    // SAFETY: `menu` is live and the item texts are static strings.
    unsafe { AppendMenuW(menu, MF_STRING, SAVE_NOW, w!("Save now")) }
        .map_err(menu_error("AppendMenuW"))?;
    // SAFETY: as above.
    unsafe { AppendMenuW(menu, MF_STRING, QUIT, w!("Quit")) }.map_err(menu_error("AppendMenuW"))?;
    // The menu closes on a click elsewhere only while its window is in the foreground, which the shell lets an icon's
    // window take. A refusal only keeps the menu open until a pick or Escape. See the TrackPopupMenu remarks.
    // SAFETY: `window` is a live window of this thread.
    let _ = unsafe { SetForegroundWindow(window) };
    // SAFETY: `menu` is live and `window` is a live window of this thread.
    let command = unsafe {
        TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            x,
            y,
            None,
            window,
            None,
        )
    }
    .0;
    // Without a message after the menu, the next one opens and closes at once. See the TrackPopupMenu remarks.
    // SAFETY: posts to a live window of this thread.
    unsafe { PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0)) }
        .map_err(menu_error("PostMessageW"))?;
    Ok(match usize::try_from(command) {
        Ok(SAVE_NOW) => Some(Choice::SaveNow),
        Ok(QUIT) => Some(Choice::Quit),
        _ => None,
    })
}

fn menu_error(call: &'static str) -> impl Fn(windows::core::Error) -> Error {
    move |source| Error::Os {
        call,
        context: "showing the tray menu".to_string(),
        source,
    }
}

/// The fields that name the icon, which every call about it carries.
fn identity(window: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: ID,
        ..Default::default()
    }
}

fn tip() -> [u16; 128] {
    let mut tip = [0u16; 128];
    for (slot, unit) in tip.iter_mut().zip(TIP.encode_utf16()) {
        *slot = unit;
    }
    tip
}

fn notify(
    message: NOTIFY_ICON_MESSAGE,
    name: &'static str,
    data: &NOTIFYICONDATAW,
) -> Result<(), Error> {
    // SAFETY: `data` is a live NOTIFYICONDATAW whose cbSize is its own size.
    match unsafe { Shell_NotifyIconW(message, data) }.as_bool() {
        true => Ok(()),
        false => Err(Error::Tray { message: name }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_of_keeps_negative_coordinates() {
        assert_eq!(point_of(WPARAM(0x0002_0001)), (1, 2));
        assert_eq!(point_of(WPARAM(0xFFFE_FFFF)), (-1, -2));
    }

    #[test]
    fn tip_is_null_terminated() {
        let tip = tip();
        let length = TIP.encode_utf16().count();
        assert_eq!(
            tip.get(..length),
            Some(TIP.encode_utf16().collect::<Vec<u16>>().as_slice())
        );
        assert_eq!(tip.get(length), Some(&0));
    }
}
