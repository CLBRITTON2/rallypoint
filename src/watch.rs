//! Keeps the newest session current: saves after GlazeWM window and workspace events and on a timer, and stops
//! saving while Windows shuts down, so the windows closing one by one never overwrite the real session.

use std::cell::RefCell;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, PostQuitMessage,
    RegisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW,
};
use windows::core::{PCWSTR, w};

use crate::error::Error;
use crate::glazewm::{Client, Event};
use crate::session::{self, SavedWindow};

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
    Failed(Error),
}

/// Saves into `folder` until GlazeWM exits. Returns the error that stopped it otherwise.
pub fn watch(folder: &Path) -> Result<(), Error> {
    let (sender, signals) = mpsc::channel::<Signal>();
    Client::connect()?
        .subscribe(&EVENTS)?
        .forward(sender.clone(), signal_of);
    thread::spawn(move || listen_for_shutdown(sender));

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
            Ok(Signal::Exiting) => return Ok(()),
            Ok(Signal::Failed(error)) => return Err(error),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(Error::ThreadGone {
                    thread: "event and shutdown",
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
    let captured = session::capture()?;
    if captured.windows.is_empty()
        || last.is_some_and(|last| session::same_windows(last, &captured.windows))
    {
        return Ok(None);
    }
    let path = session::write(folder, &captured)?;
    session::prune(folder, KEEP)?;
    println!("{}", path.display());
    Ok(Some(captured.windows))
}

fn signal_of(event: Result<Event, Error>) -> Signal {
    match event {
        Ok(Event::Changed) => Signal::Changed,
        Ok(Event::ApplicationExiting) => Signal::Exiting,
        Err(error) => Signal::Failed(error),
    }
}

thread_local! {
    /// Where the window procedure sends shutdown signals. It runs on the thread that made the window.
    static SHUTDOWN: RefCell<Option<Sender<Signal>>> = const { RefCell::new(None) };
}

/// Runs a hidden top-level window for `WM_QUERYENDSESSION`, which a message-only window never receives.
fn listen_for_shutdown(signals: Sender<Signal>) {
    let failed = signals.clone();
    SHUTDOWN.with_borrow_mut(|shutdown| *shutdown = Some(signals));
    if let Err(error) = run_shutdown_window() {
        // The main loop being gone means rallypoint is exiting, so there is nobody left to tell.
        drop(failed.send(Signal::Failed(error)));
    }
}

fn run_shutdown_window() -> Result<(), Error> {
    let window_error = |call| move |source| Error::Window { call, source };
    // SAFETY: plain Win32 calls with valid arguments. The class name and title are static strings.
    unsafe {
        let instance =
            GetModuleHandleW(PCWSTR::null()).map_err(window_error("GetModuleHandleW"))?;
        let class = w!("rallypoint-watch");
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(on_message),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassW(&window_class) == 0 {
            return Err(window_error("RegisterClassW")(
                windows::core::Error::from_thread(),
            ));
        }
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
        .map_err(window_error("CreateWindowExW"))?;
        let mut message = MSG::default();
        loop {
            match GetMessageW(&mut message, None, 0, 0).0 {
                0 => return Ok(()),
                -1 => {
                    return Err(window_error("GetMessageW")(
                        windows::core::Error::from_thread(),
                    ));
                }
                _ => {
                    DispatchMessageW(&message);
                }
            }
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
        _ => None,
    };
    if let Some(signal) = signal {
        let sent = SHUTDOWN.with_borrow(|shutdown| {
            shutdown
                .as_ref()
                .is_some_and(|shutdown| shutdown.send(signal).is_ok())
        });
        if !sent {
            // SAFETY: called on the thread running the message loop.
            unsafe { PostQuitMessage(0) };
        }
    }
    // SAFETY: forwards the arguments Windows passed in. It answers TRUE to WM_QUERYENDSESSION.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}
