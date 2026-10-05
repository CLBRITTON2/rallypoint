//! Brings a saved session back: matches the windows already open, launches the rest, and moves each window to its
//! workspace.

use std::fmt;
use std::rc::Rc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::account;
use crate::capture::{LiveWindow, Sources};
use crate::error::Error;
use crate::glazewm::{Client, Event};
use crate::launch::{keep_output_from_launches, spawn};
use crate::model::{Focus, SavedWindow, Session, WindowState};
use crate::plan::{SkipReason, assign, launch_key, launches};
use crate::uncloak;

/// How long launched windows get to appear. A window started through `runas` takes a few seconds.
const WAIT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_secs(1);
/// Windows count as settled once GlazeWM has managed none for this long.
const QUIET: Duration = Duration::from_secs(5);
/// Restoring starts after this long even if windows keep appearing.
const SETTLE_LIMIT: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub enum Outcome {
    AlreadyOpen,
    Launched,
    /// Open after the launches, though rallypoint launched nothing for it: a packaged app or a program another
    /// program started.
    Appeared,
    Skipped(SkipReason),
    /// Shared, since one launch can bring back several windows.
    LaunchFailed(Rc<Error>),
    /// Open, but moving it to its saved workspace or state failed.
    PlaceFailed(Error),
    /// Launched, but no matching window appeared within [`WAIT`].
    NotSeen,
}

pub struct Restored {
    /// One per saved window, in the session's order.
    pub outcomes: Vec<Outcome>,
    /// Showing the saved workspaces again, which runs after every window is placed.
    pub refocus: Result<(), Error>,
}

impl Outcome {
    pub fn is_restored(&self) -> bool {
        matches!(
            self,
            Outcome::AlreadyOpen | Outcome::Launched | Outcome::Appeared
        )
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Outcome::AlreadyOpen => write!(f, "already open"),
            Outcome::Launched => write!(f, "launched"),
            Outcome::Appeared => write!(f, "opened on its own"),
            Outcome::Skipped(reason) => write!(f, "skipped, {reason}"),
            Outcome::LaunchFailed(error) => write!(f, "launch failed, {error}"),
            Outcome::PlaceFailed(error) => write!(f, "open, but placing it failed, {error}"),
            Outcome::NotSeen => write!(
                f,
                "launched, but no window appeared in {} s",
                WAIT.as_secs()
            ),
        }
    }
}

/// Restores `session`. Windows of an owner other than the account rallypoint runs as are launched through
/// `runas /savecred`. A window that fails to launch or move gets that error as its outcome, and the rest go on.
pub fn restore(session: &Session) -> Result<Restored, Error> {
    let user = account::current_user()?;
    keep_output_from_launches()?;
    settle()?;
    let saved = &session.windows;
    let mut sources = Sources::connect()?;
    if uncloak::adopt(&mut sources, saved)? > 0 {
        settle()?;
    }
    let open = assign(saved, &wait(&mut sources, saved, &[])?);
    let mut launch_failures: Vec<(usize, Rc<Error>)> = Vec::new();
    let mut expected: Vec<usize> = Vec::new();
    for (launch, windows) in launches(saved, &open) {
        match spawn(&launch, &user) {
            Ok(()) => expected.extend(windows),
            Err(error) => {
                let error = Rc::new(error);
                launch_failures.extend(windows.into_iter().map(|index| (index, Rc::clone(&error))));
            }
        }
    }
    let live = wait(&mut sources, saved, &expected)?;
    let found = assign(saved, &live);
    let outcomes: Vec<Outcome> = saved
        .iter()
        .enumerate()
        .zip(&found)
        .map(|((index, window), found)| {
            let target = found.and_then(|found| live.get(found));
            let steps = Steps {
                was_open: open.get(index).is_some_and(Option::is_some),
                launched: expected.contains(&index),
                launch_failure: launch_failures
                    .iter()
                    .find(|(failed, _)| *failed == index)
                    .map(|(_, error)| Rc::clone(error)),
                is_open: target.is_some(),
                place_failure: target.and_then(|target| place(&mut sources, window, target).err()),
            };
            outcome(window, steps)
        })
        .collect();
    Ok(Restored {
        outcomes,
        refocus: refocus(&mut sources, &session.focus),
    })
}

/// What each step of a restore found for one saved window.
#[derive(Default)]
struct Steps {
    /// Open before the launches.
    was_open: bool,
    /// Brought back by a launch that started.
    launched: bool,
    launch_failure: Option<Rc<Error>>,
    /// Open after the launches.
    is_open: bool,
    place_failure: Option<Error>,
}

/// What became of `window`, given what each restore step found for it.
fn outcome(window: &SavedWindow, steps: Steps) -> Outcome {
    match steps {
        Steps {
            place_failure: Some(error),
            ..
        } => Outcome::PlaceFailed(error),
        Steps { was_open: true, .. } => Outcome::AlreadyOpen,
        Steps {
            is_open: true,
            launched: true,
            ..
        } => Outcome::Launched,
        Steps { is_open: true, .. } => Outcome::Appeared,
        Steps { launch_failure, .. } => match (launch_key(window), launch_failure) {
            (Err(reason), _) => Outcome::Skipped(reason),
            (Ok(_), Some(error)) => Outcome::LaunchFailed(error),
            (Ok(_), None) => Outcome::NotSeen,
        },
    }
}

/// Blocks until GlazeWM has managed no new window for [`QUIET`], so the apps still starting at login are matched as
/// open instead of launched a second time.
fn settle() -> Result<(), Error> {
    let (sender, events) = mpsc::channel::<Result<Event, Error>>();
    Client::connect()?
        .subscribe(&["window_managed", "application_exiting"])?
        .forward(sender, |event| event);
    let deadline = Instant::now() + SETTLE_LIMIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(QUIET.min(left)) {
            Ok(Ok(Event::Changed)) => continue,
            Ok(Ok(Event::ApplicationExiting)) => return Err(Error::GlazeExiting),
            Ok(Err(error)) => return Err(error),
            Err(RecvTimeoutError::Timeout) if left <= QUIET => {
                eprintln!(
                    "rallypoint: windows kept appearing for {} s, restoring anyway",
                    SETTLE_LIMIT.as_secs()
                );
                return Ok(());
            }
            Err(RecvTimeoutError::Timeout) => return Ok(()),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(Error::ThreadGone {
                    thread: "GlazeWM event",
                });
            }
        }
    }
}

/// One change that puts an open window where its saved window was.
#[derive(PartialEq, Debug)]
enum Placement<'a> {
    Workspace(&'a str),
    State(WindowState),
}

/// The changes that give open window `live` the workspace and state of `saved`, leaving alone what already matches,
/// since a move to the workspace a window is on would still reorder it.
fn placement<'a>(saved: &'a SavedWindow, live: &SavedWindow) -> Vec<Placement<'a>> {
    let workspace =
        (live.workspace != saved.workspace).then_some(Placement::Workspace(&saved.workspace));
    let state = (live.state != saved.state).then_some(Placement::State(saved.state));
    workspace.into_iter().chain(state).collect()
}

fn place(sources: &mut Sources, saved: &SavedWindow, target: &LiveWindow) -> Result<(), Error> {
    for change in placement(saved, &target.window) {
        match change {
            Placement::Workspace(workspace) => {
                sources.glazewm().move_to_workspace(&target.id, workspace)?
            }
            Placement::State(state) => sources.glazewm().set_state(&target.id, state.into())?,
        }
    }
    Ok(())
}

/// Shows each saved displayed workspace on its monitor and focuses the saved focused one.
fn refocus(sources: &mut Sources, focus: &Focus) -> Result<(), Error> {
    for workspace in focus_order(focus) {
        let live = sources.focus()?;
        if live.focused.as_deref() != Some(workspace) {
            sources.glazewm().focus_workspace(workspace)?;
        }
    }
    Ok(())
}

/// The workspaces to focus in turn: every displayed one, the focused one last so it keeps the focus.
fn focus_order(focus: &Focus) -> Vec<&str> {
    let focused = focus.focused.as_deref();
    focus
        .displayed
        .iter()
        .map(String::as_str)
        .filter(|workspace| Some(*workspace) != focused)
        .chain(focused)
        .collect()
}

/// Reads the open windows until every window at an index in `expected` has a match, or [`WAIT`] runs out. A read
/// fails while a just launched wezterm has not opened its socket yet, so a failed read is retried until then. At the
/// deadline a failed read falls back to the last read that worked, so the windows it found are still placed and
/// reported, and errors only when no read worked.
fn wait(
    sources: &mut Sources,
    saved: &[SavedWindow],
    expected: &[usize],
) -> Result<Vec<LiveWindow>, Error> {
    let deadline = Instant::now() + WAIT;
    let mut last_read: Option<Vec<LiveWindow>> = None;
    loop {
        match sources.live_windows() {
            Ok(live) => {
                let found = assign(saved, &live);
                let all_found = expected
                    .iter()
                    .all(|index| found.get(*index).is_some_and(Option::is_some));
                if all_found || Instant::now() >= deadline {
                    return Ok(live);
                }
                last_read = Some(live);
            }
            Err(error) if Instant::now() >= deadline => {
                return match last_read {
                    Some(live) => {
                        eprintln!(
                            "rallypoint: reading the open windows failed, using the last read: {error}"
                        );
                        Ok(live)
                    }
                    None => Err(error),
                };
            }
            Err(error) => {
                eprintln!("rallypoint: reading the open windows failed, retrying: {error}")
            }
        }
        thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    #[test]
    fn outcome_reports_a_window_by_what_happened_to_it() {
        let app = fixtures::window("app", Some(r"C:\tools\app.exe"));
        let protected = fixtures::window("protected", None);
        let packaged = fixtures::window(
            "packaged",
            Some(r"C:\Program Files\WindowsApps\Example_1\app.exe"),
        );
        let open = || Steps {
            was_open: true,
            is_open: true,
            ..Steps::default()
        };
        assert!(matches!(outcome(&app, open()), Outcome::AlreadyOpen));
        assert!(matches!(
            outcome(
                &app,
                Steps {
                    place_failure: Some(Error::ThreadGone { thread: "place" }),
                    ..open()
                }
            ),
            Outcome::PlaceFailed(Error::ThreadGone { thread: "place" })
        ));
        let launched = || Steps {
            launched: true,
            ..Steps::default()
        };
        assert!(matches!(
            outcome(
                &app,
                Steps {
                    is_open: true,
                    ..launched()
                }
            ),
            Outcome::Launched
        ));
        assert!(matches!(outcome(&app, launched()), Outcome::NotSeen));
        assert!(matches!(
            outcome(
                &app,
                Steps {
                    launch_failure: Some(Rc::new(Error::ThreadGone { thread: "launch" })),
                    ..Steps::default()
                }
            ),
            Outcome::LaunchFailed(_)
        ));
        assert!(matches!(
            outcome(&protected, Steps::default()),
            Outcome::Skipped(SkipReason::Protected)
        ));
        assert!(matches!(
            outcome(
                &packaged,
                Steps {
                    is_open: true,
                    ..Steps::default()
                }
            ),
            Outcome::Appeared
        ));
    }

    #[test]
    fn placement_changes_only_what_differs() {
        let saved = SavedWindow {
            workspace: "2".to_string(),
            state: WindowState::Floating,
            ..fixtures::window("app", None)
        };
        let moved = SavedWindow {
            workspace: "1".to_string(),
            ..saved.clone()
        };
        let tiled = SavedWindow {
            state: WindowState::Tiling,
            ..moved.clone()
        };
        assert_eq!(placement(&saved, &saved), Vec::new());
        assert_eq!(placement(&saved, &moved), vec![Placement::Workspace("2")]);
        assert_eq!(
            placement(&saved, &tiled),
            vec![
                Placement::Workspace("2"),
                Placement::State(WindowState::Floating)
            ]
        );
    }

    #[test]
    fn focus_order_ends_on_the_focused_workspace() {
        let focus = Focus {
            displayed: vec!["11".to_string(), "3".to_string()],
            focused: Some("11".to_string()),
        };
        assert_eq!(focus_order(&focus), vec!["3", "11"]);
        assert_eq!(focus_order(&Focus::default()), Vec::<&str>::new());
    }
}
