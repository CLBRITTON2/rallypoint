//! Brings a saved session back: matches the windows already open, launches the rest, and moves each window to its
//! workspace.

use std::fmt;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation};
use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};

use crate::error::Error;
use crate::glazewm::{Client, Event};
use crate::session::{Focus, LiveWindow, SavedWindow, Session, Sources};
use crate::uncloak;

/// How long launched windows get to appear. A Claude Code window started through `runas` takes a few seconds.
const WAIT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_secs(1);
/// Windows count as settled once GlazeWM has managed none for this long.
const QUIET: Duration = Duration::from_secs(5);
/// Restoring starts after this long even if windows keep appearing.
const SETTLE_LIMIT: Duration = Duration::from_secs(60);

/// What tells an open window apart as the one a saved window was.
#[derive(PartialEq, Debug)]
enum Key {
    /// A wezterm-gui window, by who runs it and the folder of its first pane.
    Terminal {
        executable_path: String,
        owner: String,
        cwd: PathBuf,
    },
    /// Any other window, by its program. Its windows are told apart only by order.
    Program { executable_path: String },
}

/// A program to start, as `owner`.
#[derive(PartialEq, Debug)]
struct Launch {
    owner: String,
    program: String,
    /// Passed verbatim, so the saved command line's quoting survives.
    arguments: String,
}

impl Launch {
    fn command_line(&self) -> String {
        format!("\"{}\" {}", self.program, self.arguments)
            .trim_end()
            .to_string()
    }
}

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

/// Makes rallypoint's stdout and stderr non-inheritable. `Command` spawns with handle inheritance on, so a launched
/// app would otherwise hold a pipe handed to rallypoint open for as long as it runs, and a shell redirecting the
/// report (the startup `restore *> log; watch`) would wait on it before starting `watch`.
fn keep_output_from_launches() -> Result<(), Error> {
    for id in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let handle = unsafe { GetStdHandle(id) }.map_err(|source| Error::Inherit {
            call: "GetStdHandle",
            source,
        })?;
        // No handle to leak when rallypoint runs without that stream.
        if handle.is_invalid() {
            continue;
        }
        unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) }.map_err(
            |source| Error::Inherit {
                call: "SetHandleInformation",
                source,
            },
        )?;
    }
    Ok(())
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
        sources.glazewm().set_state(&target.id, saved.state)?;
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

fn key(window: &SavedWindow) -> Result<Key, &'static str> {
    let executable_path = window
        .executable_path
        .as_ref()
        .ok_or("no executable path, a protected process")?;
    if window.process_name == "wezterm-gui" {
        let pane = window.panes.first().ok_or("wezterm window without panes")?;
        return Ok(Key::Terminal {
            executable_path: executable_path.clone(),
            owner: window.owner.clone(),
            cwd: pane.cwd.clone(),
        });
    }
    Ok(Key::Program {
        executable_path: executable_path.clone(),
    })
}

/// The key of a window restore can launch, or why it cannot. A packaged app is matched when open but never launched.
fn launch_key(window: &SavedWindow) -> Result<Key, &'static str> {
    let key = key(window)?;
    match &key {
        Key::Program { executable_path } if executable_path.contains(r"\WindowsApps\") => {
            Err("packaged app, its executable cannot be launched directly")
        }
        _ => Ok(key),
    }
}

/// For each saved window, the index of the open window it is, in the session's order. An open window is claimed
/// by the first saved window with its key.
fn assign(saved: &[SavedWindow], live: &[LiveWindow]) -> Vec<Option<usize>> {
    let live_keys: Vec<Option<Key>> = live.iter().map(|live| key(&live.window).ok()).collect();
    let mut claimed = vec![false; live.len()];
    saved
        .iter()
        .map(|window| {
            let wanted = key(window).ok()?;
            let index = live_keys
                .iter()
                .zip(&claimed)
                .position(|(live_key, taken)| !taken && live_key.as_ref() == Some(&wanted))?;
            if let Some(taken) = claimed.get_mut(index) {
                *taken = true;
            }
            Some(index)
        })
        .collect()
}

/// What to start for the saved windows not open yet, each with the indexes of the windows it brings back. Every
/// wezterm-gui window is its own process, so each gets a launch. Any other program is launched once, since a second
/// launch of a browser or chat app opens a stray window instead of restoring the saved ones.
fn launches(saved: &[SavedWindow], open: &[Option<usize>]) -> Vec<(Launch, Vec<usize>)> {
    let mut planned: Vec<(Key, Launch, Vec<usize>)> = Vec::new();
    for (index, window) in saved.iter().enumerate() {
        if open.get(index).is_some_and(Option::is_some) {
            continue;
        }
        let Ok(key) = launch_key(window) else {
            continue;
        };
        if let Some((_, _, windows)) = planned.iter_mut().find(|(planned_key, _, _)| {
            matches!(planned_key, Key::Program { .. }) && *planned_key == key
        }) {
            windows.push(index);
            continue;
        }
        let launch = launch(window, &key);
        planned.push((key, launch, vec![index]));
    }
    planned
        .into_iter()
        .map(|(_, launch, windows)| (launch, windows))
        .collect()
}

fn launch(window: &SavedWindow, key: &Key) -> Launch {
    match key {
        Key::Terminal {
            executable_path,
            owner,
            cwd,
        } => {
            let resume = window
                .panes
                .first()
                .and_then(|pane| pane.claude_session_id.as_ref())
                .map(|id| format!(" -- pwsh -NoLogo -Command claude --resume {id}"))
                .unwrap_or_default();
            Launch {
                owner: owner.clone(),
                program: executable_path.clone(),
                arguments: format!("start --cwd \"{}\"{resume}", cwd.display()),
            }
        }
        Key::Program { executable_path } => Launch {
            owner: window.owner.clone(),
            program: executable_path.clone(),
            arguments: window
                .command_line
                .as_deref()
                .map(arguments_of)
                .unwrap_or_default()
                .to_string(),
        },
    }
}

/// A command line without its leading program, quoted or not.
fn arguments_of(command_line: &str) -> &str {
    let command_line = command_line.trim_start();
    let rest = match command_line.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"').map_or("", |(_, rest)| rest),
        None => command_line.split_once(' ').map_or("", |(_, rest)| rest),
    };
    rest.trim()
}

/// Starts `launch` without waiting for it. Another owner's program goes through `runas /savecred`, which is waited
/// for, because it reports a missing saved credential only in its exit code and output.
fn spawn(launch: &Launch, user: &str) -> Result<(), Error> {
    let command_line = launch.command_line();
    if launch.owner.eq_ignore_ascii_case(user) {
        // Electron apps log to an inherited console, which would bury the report.
        return Command::new(&launch.program)
            .raw_arg(&launch.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(drop)
            .map_err(|source| Error::Launch {
                command_line,
                source,
            });
    }
    let output = Command::new("runas")
        .arg(format!("/user:{}", launch.owner))
        .arg("/savecred")
        .arg(&command_line)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| Error::Spawn {
            program: "runas",
            source,
        })?;
    if !output.status.success() {
        return Err(Error::Runas {
            owner: launch.owner.clone(),
            command_line,
            code: output.status.code(),
            output: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glazewm::State;
    use crate::session::SavedPane;

    fn program(path: &str, command_line: &str) -> SavedWindow {
        SavedWindow {
            workspace: "1".to_string(),
            process_name: "app".to_string(),
            executable_path: Some(path.to_string()),
            command_line: Some(command_line.to_string()),
            owner: "owner".to_string(),
            title: String::new(),
            class_name: String::new(),
            state: State::Tiling,
            panes: Vec::new(),
        }
    }

    fn terminal(cwd: &str, claude_session_id: Option<&str>) -> SavedWindow {
        SavedWindow {
            workspace: "2".to_string(),
            process_name: "wezterm-gui".to_string(),
            executable_path: Some(r"C:\Program Files\WezTerm\wezterm-gui.exe".to_string()),
            command_line: None,
            owner: "owner".to_string(),
            title: String::new(),
            class_name: String::new(),
            state: State::Tiling,
            panes: vec![SavedPane {
                cwd: PathBuf::from(cwd),
                title: String::new(),
                claude_session_id: claude_session_id.map(str::to_string),
            }],
        }
    }

    fn live(window: SavedWindow) -> LiveWindow {
        LiveWindow {
            id: String::new(),
            window,
        }
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

    #[test]
    fn arguments_of_drops_a_quoted_or_bare_program() {
        assert_eq!(
            arguments_of(r#""C:\Program Files\x.exe" -a "b c""#),
            r#"-a "b c""#
        );
        assert_eq!(arguments_of(r"x.exe -a"), "-a");
        assert_eq!(arguments_of(r#""C:\x.exe" "#), "");
        assert_eq!(arguments_of("x.exe"), "");
    }

    #[test]
    fn assign_claims_each_open_window_once_in_order() {
        let saved = vec![
            program(r"C:\f.exe", ""),
            program(r"C:\f.exe", ""),
            terminal(r"C:\a", None),
            terminal(r"C:\b", None),
        ];
        let open = vec![
            live(terminal(r"C:\b", None)),
            live(program(r"C:\f.exe", "")),
        ];
        assert_eq!(assign(&saved, &open), vec![Some(1), None, None, Some(0)]);
    }

    #[test]
    fn launches_start_each_terminal_and_each_program_once() {
        let saved = vec![
            program(r"C:\f.exe", r#""C:\f.exe" -x"#),
            terminal(r"C:\a", Some("id-a")),
            program(r"C:\f.exe", r#""C:\f.exe" -x"#),
            terminal(r"C:\b", None),
            program(r"C:\m.exe", ""),
        ];
        let open = vec![None, None, None, None, Some(0)];
        let planned: Vec<(String, Vec<usize>)> = launches(&saved, &open)
            .into_iter()
            .map(|(launch, windows)| (launch.command_line(), windows))
            .collect();
        assert_eq!(
            planned,
            vec![
                (r#""C:\f.exe" -x"#.to_string(), vec![0, 2]),
                (
                    r#""C:\Program Files\WezTerm\wezterm-gui.exe" start --cwd "C:\a" -- pwsh -NoLogo -Command claude --resume id-a"#
                        .to_string(),
                    vec![1]
                ),
                (
                    r#""C:\Program Files\WezTerm\wezterm-gui.exe" start --cwd "C:\b""#.to_string(),
                    vec![3]
                ),
            ]
        );
    }

    #[test]
    fn launch_key_skips_what_cannot_be_launched() {
        let protected = SavedWindow {
            executable_path: None,
            ..program("", "")
        };
        assert!(launch_key(&protected).is_err());
        let packaged = program(
            r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1\wt.exe",
            "",
        );
        assert!(launch_key(&packaged).is_err());
    }

    #[test]
    fn assign_matches_an_open_packaged_app() {
        let packaged = r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1\wt.exe";
        let saved = vec![program(packaged, "")];
        let open = vec![live(program(packaged, ""))];
        assert_eq!(assign(&saved, &open), vec![Some(0)]);
    }
}
