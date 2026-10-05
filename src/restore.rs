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
use crate::model::{Focus, SavedWindow, Session};
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
        matches!(self, Outcome::AlreadyOpen | Outcome::Launched)
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Outcome::AlreadyOpen => write!(f, "already open"),
            Outcome::Launched => write!(f, "launched"),
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
    let place_failures: Vec<Option<Error>> = saved
        .iter()
        .zip(&found)
        .map(|(window, index)| {
            let target = index.and_then(|index| live.get(index))?;
            place(&mut sources, window, target).err()
        })
        .collect();
    Ok(Restored {
        outcomes: outcomes(saved, &open, &found, &launch_failures, place_failures),
        refocus: refocus(&mut sources, &session.focus),
    })
}

/// What became of each saved window: `open` and `found` are the open windows assigned before and after the launches,
/// `launch_failures` the windows whose launch failed, and `place_failures` one entry per saved window.
fn outcomes(
    saved: &[SavedWindow],
    open: &[Option<usize>],
    found: &[Option<usize>],
    launch_failures: &[(usize, Rc<Error>)],
    place_failures: Vec<Option<Error>>,
) -> Vec<Outcome> {
    saved
        .iter()
        .zip(place_failures)
        .enumerate()
        .map(|(index, (window, place_failure))| {
            let was_open = open.get(index).is_some_and(Option::is_some);
            let is_open = found.get(index).is_some_and(Option::is_some);
            let launch_failure = launch_failures.iter().find(|(failed, _)| *failed == index);
            match (
                place_failure,
                was_open,
                is_open,
                launch_key(window),
                launch_failure,
            ) {
                (Some(error), ..) => Outcome::PlaceFailed(error),
                (None, true, ..) => Outcome::AlreadyOpen,
                (None, _, true, ..) => Outcome::Launched,
                (None, _, _, Err(reason), _) => Outcome::Skipped(reason),
                (None, _, _, _, Some((_, error))) => Outcome::LaunchFailed(Rc::clone(error)),
                (None, ..) => Outcome::NotSeen,
            }
        })
        .collect()
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

/// Moves `target` to the saved window's workspace and state, leaving alone what already matches, since a move to the
/// workspace a window is on would still reorder it.
fn place(sources: &mut Sources, saved: &SavedWindow, target: &LiveWindow) -> Result<(), Error> {
    if target.window.workspace != saved.workspace {
        sources
            .glazewm()
            .move_to_workspace(&target.id, &saved.workspace)?;
    }
    if target.window.state != saved.state {
        sources
            .glazewm()
            .set_state(&target.id, saved.state.into())?;
    }
    Ok(())
}

/// Shows each saved displayed workspace on its monitor and focuses the saved focused one.
fn refocus(sources: &mut Sources, focus: &Focus) -> Result<(), Error> {
    for workspace in focus_order(focus) {
        if sources.focus()?.focused.as_deref() != Some(workspace) {
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
    fn outcomes_report_each_window_by_what_happened_to_it() {
        let app = |name: &str| fixtures::window(name, Some(r"C:\tools\app.exe"));
        let saved = vec![
            app("open"),
            app("launched"),
            fixtures::window("protected", None),
            app("failed"),
            app("unseen"),
            app("misplaced"),
        ];
        let open = [Some(0), None, None, None, None, Some(1)];
        let found = [Some(0), Some(2), None, None, None, Some(1)];
        let launch_error = Rc::new(Error::ThreadGone { thread: "launch" });
        let place_failures = vec![
            None,
            None,
            None,
            None,
            None,
            Some(Error::ThreadGone { thread: "place" }),
        ];
        let outcomes = outcomes(&saved, &open, &found, &[(3, launch_error)], place_failures);
        assert!(matches!(
            outcomes.as_slice(),
            [
                Outcome::AlreadyOpen,
                Outcome::Launched,
                Outcome::Skipped(SkipReason::Protected),
                Outcome::LaunchFailed(_),
                Outcome::NotSeen,
                Outcome::PlaceFailed(Error::ThreadGone { thread: "place" }),
            ]
        ));
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
