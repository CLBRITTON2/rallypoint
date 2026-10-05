//! Keeps the newest session current: saves after GlazeWM window and workspace events and on a timer, and stops
//! saving while Windows shuts down, so the windows closing one by one never overwrite the real session. Shows a tray
//! icon while it runs, whose menu saves at once or stops watching.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CONTEXTMENU,
    WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW,
};
use windows::core::{PCWSTR, w};

use crate::capture::capture;
use crate::error::Error;
use crate::glazewm::{Client, Event};
use crate::lock;
use crate::model::{SavedWindow, same_windows};
use crate::store;
use crate::tray::{self, Choice};

/// A burst of events (a restore, a workspace switch) gives one save this long after its last event.
const DEBOUNCE: Duration = Duration::from_secs(2);
/// Claude title changes fire no GlazeWM event, so a save also runs this often.
const PERIOD: Duration = Duration::from_secs(60);
const KEEP: usize = 10;
const MAX_FAILURES: u32 = 5;
const EVENTS: [&str; 7] = [
    "window_managed",
    "window_unmanaged",
    "focused_container_moved",
    "workspace_activated",
    "workspace_deactivated",
    "workspace_updated",
    "application_exiting",
];

enum Signal {
    Changed,
    /// Windows asked to end the session.
    Freeze,
    /// The shutdown was cancelled.
    Thaw,
    Exiting,
    /// The tray menu's Save now.
    SaveNow,
    /// The tray menu's Quit.
    Quit,
    Failed(Error),
}

/// Saves into `folder` until GlazeWM exits or the tray menu's Quit. Returns the error that stopped it otherwise.
pub fn watch(folder: &Path) -> Result<(), Error> {
    let _lock = lock::take()?;
    let (sender, signals) = mpsc::channel::<Signal>();
    Client::connect()?
        .subscribe(&EVENTS)?
        .forward(sender.clone(), signal_of);
    let (opened, window) = mpsc::channel::<Result<isize, Error>>();
    let shown = Arc::new(AtomicBool::new(false));
    let icon_shown = Arc::clone(&shown);
    thread::spawn(move || listen(sender, opened, icon_shown));
    let window = window
        .recv()
        .map_err(|_| Error::ThreadGone { thread: "window" })??;
    let stopped = save_until_stopped(folder, &signals);
    // Otherwise the icon stays in the notification area until the pointer passes over it.
    if shown.load(Ordering::Relaxed)
        && let Err(error) = tray::remove(HWND(window as _))
    {
        eprintln!("rallypoint: removing the tray icon failed: {error}");
    }
    stopped
}

fn save_until_stopped(folder: &Path, signals: &Receiver<Signal>) -> Result<(), Error> {
    let mut frozen = false;
    let mut last: Option<Vec<SavedWindow>> = None;
    let mut failures: u32 = 0;
    let mut quiet: Option<Instant> = Some(Instant::now());
    let mut periodic = Instant::now() + PERIOD;
    loop {
        let due = quiet.map_or(periodic, |quiet| quiet.min(periodic));
        match signals.recv_timeout(due.saturating_duration_since(Instant::now())) {
            Ok(Signal::Changed) => quiet = Some(Instant::now() + DEBOUNCE),
            Ok(Signal::Freeze) => frozen = true,
            Ok(Signal::Thaw) => {
                frozen = false;
                quiet = Some(Instant::now() + DEBOUNCE);
            }
            Ok(Signal::SaveNow) => quiet = Some(Instant::now()),
            Ok(Signal::Exiting | Signal::Quit) => return Ok(()),
            Ok(Signal::Failed(error)) => return Err(error),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(Error::ThreadGone {
                    thread: "event and window",
                });
            }
            Err(RecvTimeoutError::Timeout) => {
                quiet = None;
                periodic = Instant::now() + PERIOD;
                if frozen {
                    continue;
                }
                match save(folder, last.as_deref()) {
                    Ok(saved) => {
                        failures = 0;
                        last = saved.or(last);
                    }
                    Err(error) => {
                        failures += 1;
                        if failures >= MAX_FAILURES {
                            return Err(Error::SavesFailing {
                                failures,
                                source: Box::new(error),
                            });
                        }
                        eprintln!("rallypoint: save failed, retrying: {error}");
                        quiet = Some(Instant::now() + DEBOUNCE);
                    }
                }
            }
        }
    }
}

/// Captures and writes a session unless it is empty or holds the same windows as `last`. Returns the windows written.
fn save(folder: &Path, last: Option<&[SavedWindow]>) -> Result<Option<Vec<SavedWindow>>, Error> {
    let captured = capture()?;
    if !worth_writing(&captured.windows, last) {
        return Ok(None);
    }
    let path = store::write(folder, &captured)?;
    store::prune(folder, KEEP)?;
    println!("{}", path.display());
    Ok(Some(captured.windows))
}

/// Whether `captured` is a new session: not empty, as while GlazeWM starts, and not the same windows as `last`.
fn worth_writing(captured: &[SavedWindow], last: Option<&[SavedWindow]>) -> bool {
    !captured.is_empty() && !last.is_some_and(|last| same_windows(last, captured))
}

fn signal_of(event: Result<Event, Error>) -> Signal {
    match event {
        Ok(Event::Changed) => Signal::Changed,
        Ok(Event::ApplicationExiting) => Signal::Exiting,
        Err(error) => Signal::Failed(error),
    }
}

thread_local! {
    /// Where the window procedure sends its signals. It runs on the thread that made the window.
    static SIGNALS: RefCell<Option<Sender<Signal>>> = const { RefCell::new(None) };
    /// The message id Explorer broadcasts when it starts a new taskbar.
    static TASKBAR_CREATED: Cell<Option<u32>> = const { Cell::new(None) };
    /// Whether the tray icon is up, which `watch` reads to remove it only then.
    static ICON_SHOWN: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Runs a hidden top-level window with the tray icon on it, since a message-only window never receives
/// `WM_QUERYENDSESSION` or `TaskbarCreated`. Sends the window on `opened` once it is up, or why it is not.
fn listen(signals: Sender<Signal>, opened: Sender<Result<isize, Error>>, shown: Arc<AtomicBool>) {
    let failed = signals.clone();
    SIGNALS.with_borrow_mut(|slot| *slot = Some(signals));
    ICON_SHOWN.with_borrow_mut(|slot| *slot = Some(shown));
    let window = match open_window() {
        Ok(window) => window,
        Err(error) => {
            // `watch` waits on `opened` until this send, so it is there to receive it.
            drop(opened.send(Err(error)));
            return;
        }
    };
    show_icon(window);
    drop(opened.send(Ok(window.0 as isize)));
    if let Err(error) = pump_messages() {
        // The main loop being gone means rallypoint is exiting, so there is nobody left to tell.
        drop(failed.send(Signal::Failed(error)));
    }
}

fn window_error(call: &'static str) -> impl Fn(windows::core::Error) -> Error {
    move |source| Error::Os {
        call,
        context: "opening the watch window".to_string(),
        source,
    }
}

fn open_window() -> Result<HWND, Error> {
    // So the tray icon is loaded at the taskbar's size, not scaled up from 96 dpi. Only this thread changes.
    // SAFETY: takes a predefined context.
    if unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
        .0
        .is_null()
    {
        return Err(window_error("SetThreadDpiAwarenessContext")(
            windows::core::Error::from_thread(),
        ));
    }
    // SAFETY: the name is a static string.
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    if taskbar_created == 0 {
        return Err(window_error("RegisterWindowMessageW")(
            windows::core::Error::from_thread(),
        ));
    }
    TASKBAR_CREATED.set(Some(taskbar_created));
    // SAFETY: a null name asks for this executable's module, which is never freed.
    let instance =
        unsafe { GetModuleHandleW(PCWSTR::null()) }.map_err(window_error("GetModuleHandleW"))?;
    let class = w!("rallypoint-watch");
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(on_message),
        hInstance: instance.into(),
        lpszClassName: class,
        ..Default::default()
    };
    // SAFETY: the class name is a static string, and `on_message` has the window procedure signature.
    if unsafe { RegisterClassW(&window_class) } == 0 {
        return Err(window_error("RegisterClassW")(
            windows::core::Error::from_thread(),
        ));
    }
    // SAFETY: the class was registered above, and the class name and title are static strings.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("rallypoint watch"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
    .map_err(window_error("CreateWindowExW"))
}

fn pump_messages() -> Result<(), Error> {
    let mut message = MSG::default();
    loop {
        // SAFETY: `message` is a live local the call writes.
        match unsafe { GetMessageW(&mut message, None, 0, 0) }.0 {
            0 => return Ok(()),
            -1 => {
                return Err(window_error("GetMessageW")(
                    windows::core::Error::from_thread(),
                ));
            }
            _ => {
                // SAFETY: `message` is the message GetMessageW just filled in.
                unsafe { DispatchMessageW(&message) };
            }
        }
    }
}

/// Adds the tray icon, warning rather than stopping `watch`, which saves just as well without it.
fn show_icon(window: HWND) {
    let added = tray::add(window);
    if let Err(error) = &added {
        eprintln!("rallypoint: showing the tray icon failed: {error}");
    }
    ICON_SHOWN.with_borrow(|shown| {
        if let Some(shown) = shown {
            shown.store(added.is_ok(), Ordering::Relaxed);
        }
    });
}

/// What the tray menu's pick asks of the main loop, warning when the menu fails.
fn picked(window: HWND, wparam: WPARAM) -> Option<Signal> {
    match tray::choose(window, wparam) {
        Ok(choice) => choice.map(|choice| match choice {
            Choice::SaveNow => Signal::SaveNow,
            Choice::Quit => Signal::Quit,
        }),
        Err(error) => {
            eprintln!("rallypoint: the tray menu failed: {error}");
            None
        }
    }
}

unsafe extern "system" fn on_message(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let signal = match message {
        WM_QUERYENDSESSION => Some(Signal::Freeze),
        WM_ENDSESSION if wparam.0 == 0 => Some(Signal::Thaw),
        tray::CALLBACK if (lparam.0 & 0xFFFF) as u32 == WM_CONTEXTMENU => picked(window, wparam),
        _ if TASKBAR_CREATED.get() == Some(message) => {
            show_icon(window);
            None
        }
        _ => None,
    };
    if let Some(signal) = signal {
        let sent = SIGNALS.with_borrow(|signals| {
            signals
                .as_ref()
                .is_some_and(|signals| signals.send(signal).is_ok())
        });
        if !sent {
            // SAFETY: called on the thread running the message loop.
            unsafe { PostQuitMessage(0) };
        }
    }
    // SAFETY: forwards the arguments Windows passed in. It answers TRUE to WM_QUERYENDSESSION.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    #[test]
    fn worth_writing_skips_an_empty_or_unchanged_capture() {
        let one = vec![fixtures::window("app", None)];
        let two = vec![
            fixtures::window("app", None),
            fixtures::window("other", None),
        ];
        assert!(!worth_writing(&[], None));
        assert!(worth_writing(&one, None));
        assert!(!worth_writing(&one, Some(&one)));
        assert!(worth_writing(&two, Some(&one)));
    }
}
