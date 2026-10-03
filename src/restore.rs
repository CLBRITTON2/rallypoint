//! Brings a saved session back: matches the windows already open, launches the rest, and moves each window to its
//! workspace.

use std::fmt;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::capture::{LiveWindow, Sources};
use crate::error::Error;
use crate::glazewm::{Client, Event};
use crate::launch::{keep_output_from_launches, spawn};
use crate::model::{Focus, SavedWindow, Session};
use crate::plan::{assign, launch_key, launches};
use crate::uncloak;

/// How long launched windows get to appear. A window started through `runas` takes a few seconds.
const WAIT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_secs(1);
/// Windows count as settled once GlazeWM has managed none for this long.
const QUIET: Duration = Duration::from_secs(5);
/// Restoring starts after this long even if windows keep appearing.
const SETTLE_LIMIT: Duration = Duration::from_secs(60);

#[derive(PartialEq, Debug)]
pub enum Outcome {
    AlreadyOpen,
    Launched,
    Skipped(&'static str),
    LaunchFailed(String),
    /// Launched, but no matching window appeared within [`WAIT`].
    NotSeen,
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
            Outcome::NotSeen => write!(
                f,
                "launched, but no window appeared in {} s",
                WAIT.as_secs()
            ),
        }
    }
}

/// Restores `session` and returns one outcome per saved window, in the session's order. `user` is the account
/// rallypoint runs as: windows of any other owner are launched through `runas /savecred`.
pub fn restore(session: &Session, user: &str) -> Result<Vec<Outcome>, Error> {
    keep_output_from_launches()?;
    settle()?;
    let saved = &session.windows;
    let mut sources = Sources::connect()?;
    if uncloak::adopt(&mut sources, saved)? > 0 {
        settle()?;
    }
    let open = assign(saved, &sources.live_windows()?);
    let mut failures: Vec<(usize, String)> = Vec::new();
    let mut expected: Vec<usize> = Vec::new();
    for (launch, windows) in launches(saved, &open) {
        match spawn(&launch, user) {
            Ok(()) => expected.extend(windows),
            Err(error) => {
                failures.extend(windows.into_iter().map(|index| (index, error.to_string())))
            }
        }
    }
    let live = wait(&mut sources, saved, &expected)?;
    let found = assign(saved, &live);
    for (window, index) in saved.iter().zip(&found) {
        if let Some(target) = index.and_then(|index| live.get(index)) {
            place(&mut sources, window, target)?;
        }
    }
    refocus(&mut sources, &session.focus)?;
    Ok(saved
        .iter()
        .enumerate()
        .map(|(index, window)| {
            let was_open = open.get(index).is_some_and(Option::is_some);
            let is_open = found.get(index).is_some_and(Option::is_some);
            let failure = failures.iter().find(|(failed, _)| *failed == index);
            match (was_open, is_open, launch_key(window), failure) {
                (true, _, _, _) => Outcome::AlreadyOpen,
                (_, true, _, _) => Outcome::Launched,
                (_, _, Err(reason), _) => Outcome::Skipped(reason),
                (_, _, _, Some((_, error))) => Outcome::LaunchFailed(error.clone()),
                _ => Outcome::NotSeen,
            }
        })
        .collect())
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
/// fails while a just launched wezterm has not opened its socket yet, so a failed read is retried until then.
fn wait(
    sources: &mut Sources,
    saved: &[SavedWindow],
    expected: &[usize],
) -> Result<Vec<LiveWindow>, Error> {
    let deadline = Instant::now() + WAIT;
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
            }
            Err(error) if Instant::now() >= deadline => return Err(error),
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
