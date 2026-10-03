//! A saved session: every GlazeWM window, the workspace it sits on, and what relaunching it needs.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::claude;
use crate::cwd;
use crate::error::Error;
use crate::glazewm::{Client, State, Window, Workspace};
use crate::process::{Process, Processes};
use crate::tabs::{self, Tab};
use crate::wezterm;

/// The session format `capture` writes and `read` accepts. Bump it on any change an older rallypoint could misread,
/// since a session of another version is refused rather than migrated.
pub const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    /// Unix milliseconds.
    pub saved_at: u64,
    pub focus: Focus,
    pub windows: Vec<SavedWindow>,
}

/// The part of a session file read before the rest, so a session of another version fails on its version instead
/// of on whichever field changed. Sessions saved before versioning have none.
#[derive(Deserialize)]
struct Header {
    version: Option<u32>,
}

/// Which workspaces were on screen.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Focus {
    /// One per monitor.
    pub displayed: Vec<String>,
    pub focused: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct SavedWindow {
    pub workspace: String,
    pub process_name: String,
    pub executable_path: Option<String>,
    pub command_line: Option<String>,
    pub owner: String,
    pub title: String,
    pub class_name: String,
    pub state: WindowState,
    pub app: AppState,
}

/// rallypoint's copy of a GlazeWM window state, so the session format does not change with GlazeWM's IPC types.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum WindowState {
    Tiling,
    Floating,
    Minimized,
    Fullscreen,
}

impl From<State> for WindowState {
    fn from(state: State) -> WindowState {
        match state {
            State::Tiling => WindowState::Tiling,
            State::Floating => WindowState::Floating,
            State::Minimized => WindowState::Minimized,
            State::Fullscreen => WindowState::Fullscreen,
        }
    }
}

impl From<WindowState> for State {
    fn from(state: WindowState) -> State {
        match state {
            WindowState::Tiling => State::Tiling,
            WindowState::Floating => State::Floating,
            WindowState::Minimized => State::Minimized,
            WindowState::Fullscreen => State::Fullscreen,
        }
    }
}

/// What reopening a window needs beyond its program.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppState {
    Program,
    /// A console shell. None is a folder save could not read.
    Shell {
        cwd: Option<PathBuf>,
    },
    /// A terminal window: a wezterm window's panes or a Windows Terminal window's tabs, oldest first.
    Terminal {
        panes: Vec<Pane>,
    },
}

/// One wezterm pane or Windows Terminal tab.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Pane {
    /// The program the pane runs. None for a wezterm pane, and for a tab running a protected process.
    pub program: Option<String>,
    pub command_line: Option<String>,
    /// None for a tab that is not a shell, and for a shell whose folder could not be read.
    pub cwd: Option<PathBuf>,
    pub resume: Option<Resume>,
}

/// A program session a pane reopens into.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Resume {
    ClaudeCode { session_id: String },
}

/// An open window: what a save records of it, and the GlazeWM container ID that commands it.
pub struct LiveWindow {
    pub id: String,
    pub window: SavedWindow,
}

/// The connections reading a session needs, opened once and reused across reads.
pub struct Sources {
    glazewm: Client,
    processes: Processes,
    profiles: PathBuf,
}

impl Sources {
    pub fn connect() -> Result<Sources, Error> {
        Ok(Sources {
            glazewm: Client::connect()?,
            processes: Processes::connect()?,
            profiles: profiles_folder()?,
        })
    }

    pub fn glazewm(&mut self) -> &mut Client {
        &mut self.glazewm
    }

    pub fn processes(&self) -> &Processes {
        &self.processes
    }

    /// Every open window, read from GlazeWM, WMI, wezterm and Claude Code's session files.
    pub fn live_windows(&mut self) -> Result<Vec<LiveWindow>, Error> {
        let mut windows = Vec::new();
        let tab_shells = tabs::all()?;
        for workspace in self.glazewm.workspaces()? {
            for window in workspace.windows() {
                let process = self.processes.of_window(window.handle)?;
                let app = match window.process_name.as_str() {
                    "wezterm-gui" => AppState::Terminal {
                        panes: wezterm_panes(&self.profiles.join(&process.owner), &process)?,
                    },
                    WINDOWS_TERMINAL => AppState::Terminal {
                        panes: self.tabs(&tab_shells, window.handle)?,
                    },
                    name if is_shell(name) => AppState::Shell {
                        cwd: cwd::of_process(process.pid)?,
                    },
                    _ => AppState::Program,
                };
                windows.push(LiveWindow {
                    id: window.id.clone(),
                    window: saved_window(&workspace.name, window, process, app),
                });
            }
        }
        Ok(windows)
    }

    /// The tabs of the Windows Terminal window `handle`, among `tab_shells`, oldest first.
    fn tabs(&self, tab_shells: &[Tab], handle: isize) -> Result<Vec<Pane>, Error> {
        let pids: Vec<u32> = tab_shells
            .iter()
            .filter(|tab| tab.window == handle)
            .map(|tab| tab.pid)
            .collect();
        self.processes
            .started(&pids)?
            .into_iter()
            .map(|process| {
                let is_shell_tab = process
                    .executable_path
                    .as_deref()
                    .is_some_and(|path| is_shell(&program_name(path)));
                Ok(Pane {
                    cwd: match is_shell_tab {
                        true => cwd::of_process(process.process_id)?,
                        false => None,
                    },
                    program: process.executable_path,
                    command_line: process.command_line,
                    resume: None,
                })
            })
            .collect()
    }

    pub fn focus(&mut self) -> Result<Focus, Error> {
        Ok(focus_of(&self.glazewm.workspaces()?))
    }
}

pub fn capture() -> Result<Session, Error> {
    let mut sources = Sources::connect()?;
    let windows = sources.live_windows()?;
    Ok(Session {
        version: VERSION,
        saved_at: now()?,
        focus: sources.focus()?,
        windows: windows.into_iter().map(|live| live.window).collect(),
    })
}

fn focus_of(workspaces: &[Workspace]) -> Focus {
    Focus {
        displayed: workspaces
            .iter()
            .filter(|workspace| workspace.is_displayed)
            .map(|workspace| workspace.name.clone())
            .collect(),
        focused: workspaces
            .iter()
            .find(|workspace| workspace.has_focus)
            .map(|workspace| workspace.name.clone()),
    }
}

/// The current time in Unix milliseconds, the unit of `saved_at`.
pub fn now() -> Result<u64, Error> {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(Error::Clock)?;
    u64::try_from(since_epoch.as_millis()).map_err(Error::ClockRange)
}

/// Every session file in `folder`, oldest first.
fn saved_paths(folder: &Path) -> Result<Vec<PathBuf>, Error> {
    let read_error = |source| Error::Read {
        path: folder.to_path_buf(),
        source,
    };
    let mut saved: Vec<(u64, PathBuf)> = Vec::new();
    for entry in fs::read_dir(folder).map_err(read_error)? {
        let path = entry.map_err(read_error)?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let saved_at = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<u64>().ok())
            .ok_or_else(|| Error::SessionName { path: path.clone() })?;
        saved.push((saved_at, path));
    }
    saved.sort_by_key(|(saved_at, _)| *saved_at);
    Ok(saved.into_iter().map(|(_, path)| path).collect())
}

/// Every session file in `folder`, newest first.
pub fn newest_first(folder: &Path) -> Result<Vec<PathBuf>, Error> {
    Ok(saved_paths(folder)?.into_iter().rev().collect())
}

/// The newest session in `folder`, the one with the largest `saved_at`.
pub fn latest(folder: &Path) -> Result<Session, Error> {
    let path = saved_paths(folder)?.pop().ok_or_else(|| Error::NoSession {
        folder: folder.to_path_buf(),
    })?;
    read(&path)
}

pub fn read(path: &Path) -> Result<Session, Error> {
    let text = fs::read_to_string(path).map_err(|source| Error::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let decode_error = |source| Error::Decode {
        path: path.to_path_buf(),
        source,
    };
    let header: Header = serde_json::from_str(&text).map_err(decode_error)?;
    if header.version != Some(VERSION) {
        return Err(Error::SessionVersion {
            path: path.to_path_buf(),
            found: header.version,
            expected: VERSION,
        });
    }
    serde_json::from_str(&text).map_err(decode_error)
}

/// How long before `now` a session saved at `saved_at` was written, in its largest whole unit, as `12 min ago`.
pub fn age(saved_at: u64, now: u64) -> String {
    let seconds = now.saturating_sub(saved_at) / 1000;
    match seconds {
        0..60 => format!("{seconds} s ago"),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86400),
    }
}

/// How many distinct workspaces `windows` sit on.
pub fn workspace_count(windows: &[SavedWindow]) -> usize {
    let mut names: Vec<&str> = windows
        .iter()
        .map(|window| window.workspace.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    names.len()
}

fn saved_window(workspace: &str, window: Window, process: Process, app: AppState) -> SavedWindow {
    SavedWindow {
        workspace: workspace.to_string(),
        process_name: window.process_name,
        executable_path: process.executable_path,
        command_line: process.command_line,
        owner: process.owner,
        title: window.title,
        class_name: window.class_name,
        state: window.state.into(),
        app,
    }
}

/// Whether a window is a console shell, which restore reopens in its saved folder, one launch per window.
pub fn is_shell(process_name: &str) -> bool {
    matches!(process_name, "pwsh" | "powershell" | "cmd")
}

/// The process name of an executable path, as GlazeWM names a window's process: `pwsh` for `C:\x\pwsh.exe`.
pub fn program_name(executable_path: &str) -> String {
    Path::new(executable_path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default()
}

pub const WINDOWS_TERMINAL: &str = "WindowsTerminal";

fn wezterm_panes(owner_home: &Path, process: &Process) -> Result<Vec<Pane>, Error> {
    let socket = wezterm::socket(owner_home, process.pid);
    wezterm::panes(&socket, process.pid)?
        .into_iter()
        .map(|pane| {
            let resume = claude::session_id(owner_home, &pane.cwd, &pane.title)?
                .map(|session_id| Resume::ClaudeCode { session_id });
            Ok(Pane {
                program: None,
                command_line: None,
                cwd: Some(pane.cwd),
                resume,
            })
        })
        .collect()
}

/// The folder holding every account's profile, so an owner's home is found without naming a user folder.
fn profiles_folder() -> Result<PathBuf, Error> {
    let profile = std::env::var_os("USERPROFILE").ok_or(Error::Env {
        name: "USERPROFILE",
    })?;
    let profile = PathBuf::from(profile);
    profile.parent().map(Path::to_path_buf).ok_or(Error::Env {
        name: "USERPROFILE",
    })
}

/// `%LOCALAPPDATA%\rallypoint\sessions`, where every save lands.
pub fn sessions_folder() -> Result<PathBuf, Error> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or(Error::Env {
        name: "LOCALAPPDATA",
    })?;
    Ok(PathBuf::from(local).join(r"rallypoint\sessions"))
}

/// Writes `session` to `<folder>\<saved_at>.json` through a temp file, so a crash mid-write never leaves a torn
/// session behind. Returns the path written.
pub fn write(folder: &Path, session: &Session) -> Result<PathBuf, Error> {
    let write_error = |path: &Path| {
        let path = path.to_path_buf();
        move |source| Error::Write { path, source }
    };
    fs::create_dir_all(folder).map_err(write_error(folder))?;
    let path = folder.join(format!("{}.json", session.saved_at));
    let temp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(session).map_err(Error::Encode)?;
    fs::write(&temp, json).map_err(write_error(&temp))?;
    fs::rename(&temp, &path).map_err(write_error(&path))?;
    Ok(path)
}

/// Deletes all but the newest `keep` sessions in `folder`.
pub fn prune(folder: &Path, keep: usize) -> Result<(), Error> {
    let saved = saved_paths(folder)?;
    let stale = saved.len().saturating_sub(keep);
    for path in saved.into_iter().take(stale) {
        fs::remove_file(&path).map_err(|source| Error::Remove { path, source })?;
    }
    Ok(())
}

/// Whether two saves hold the same windows, ignoring titles: a title changes with every page or command and needs
/// no new save, while a Claude session change shows up in a pane's `resume`.
pub fn same_windows(a: &[SavedWindow], b: &[SavedWindow]) -> bool {
    untitled(a) == untitled(b)
}

fn untitled(windows: &[SavedWindow]) -> Vec<SavedWindow> {
    windows
        .iter()
        .map(|window| SavedWindow {
            title: String::new(),
            ..window.clone()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    fn window(title: &str, workspace: &str) -> SavedWindow {
        SavedWindow {
            workspace: workspace.to_string(),
            title: title.to_string(),
            ..fixtures::window("app", None)
        }
    }

    fn sessions_saved_at(folder: &Path, saved_at: &[u64]) -> Result<(), Error> {
        for &saved_at in saved_at {
            write(
                folder,
                &Session {
                    version: VERSION,
                    saved_at,
                    focus: Focus::default(),
                    windows: Vec::new(),
                },
            )?;
        }
        Ok(())
    }

    #[test]
    fn same_windows_ignores_titles_only() {
        assert!(same_windows(&[window("a", "1")], &[window("b", "1")]));
        assert!(!same_windows(&[window("a", "1")], &[window("a", "2")]));
    }

    #[test]
    fn focus_of_takes_displayed_and_focused_workspaces() -> Result<(), serde_json::Error> {
        let workspaces: Vec<Workspace> = serde_json::from_str(
            r#"[{"name":"1","hasFocus":false,"isDisplayed":false,"children":[]},
                {"name":"2","hasFocus":false,"isDisplayed":true,"children":[]},
                {"name":"11","hasFocus":true,"isDisplayed":true,"children":[]}]"#,
        )?;
        let expected = Focus {
            displayed: vec!["2".to_string(), "11".to_string()],
            focused: Some("11".to_string()),
        };
        assert_eq!(focus_of(&workspaces), expected);
        Ok(())
    }

    #[test]
    fn read_refuses_a_session_of_another_version() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let unversioned = folder.path().join("1.json");
        fs::write(&unversioned, r#"{"saved_at":1,"windows":[]}"#)?;
        let newer = folder.path().join("2.json");
        fs::write(
            &newer,
            r#"{"version":99,"saved_at":2,"focus":{},"windows":[{}]}"#,
        )?;
        assert!(matches!(
            read(&unversioned),
            Err(Error::SessionVersion {
                found: None,
                expected: VERSION,
                ..
            })
        ));
        assert!(matches!(
            read(&newer),
            Err(Error::SessionVersion {
                found: Some(99),
                expected: VERSION,
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn a_written_session_reads_back() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let pane = Pane {
            program: Some(r"C:\tools\shell.exe".to_string()),
            command_line: None,
            cwd: Some(PathBuf::from(r"C:\work\project")),
            resume: Some(Resume::ClaudeCode {
                session_id: "id".to_string(),
            }),
        };
        let windows = vec![
            fixtures::window("app", Some(r"C:\tools\app.exe")),
            SavedWindow {
                app: AppState::Shell { cwd: None },
                state: WindowState::Floating,
                ..fixtures::window("shell", None)
            },
            SavedWindow {
                app: AppState::Terminal { panes: vec![pane] },
                ..fixtures::window("terminal", None)
            },
        ];
        let session = Session {
            version: VERSION,
            saved_at: 7,
            focus: Focus::default(),
            windows: windows.clone(),
        };
        assert_eq!(read(&write(folder.path(), &session)?)?.windows, windows);
        Ok(())
    }

    #[test]
    fn program_name_is_the_file_stem() {
        assert_eq!(
            program_name(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            "pwsh"
        );
        assert_eq!(program_name(r"C:\WINDOWS\system32\cmd.exe"), "cmd");
    }

    #[test]
    fn age_uses_the_largest_whole_unit() {
        assert_eq!(age(0, 59_999), "59 s ago");
        assert_eq!(age(0, 60_000), "1 min ago");
        assert_eq!(age(0, 3_599_999), "59 min ago");
        assert_eq!(age(0, 7_200_000), "2 h ago");
        assert_eq!(age(0, 86_400_000), "1 d ago");
        assert_eq!(age(5_000, 1_000), "0 s ago");
    }

    #[test]
    fn workspace_count_counts_each_name_once() {
        let windows = [window("a", "2"), window("b", "1"), window("c", "2")];
        assert_eq!(workspace_count(&windows), 2);
    }

    #[test]
    fn newest_first_reverses_save_order() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        sessions_saved_at(folder.path(), &[3, 10, 2])?;
        let names = ["10.json", "3.json", "2.json"].map(|name| folder.path().join(name));
        assert_eq!(newest_first(folder.path())?, names);
        Ok(())
    }

    #[test]
    fn prune_keeps_the_newest() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        sessions_saved_at(folder.path(), &[3, 10, 2])?;
        prune(folder.path(), 2)?;
        let left = vec![folder.path().join("3.json"), folder.path().join("10.json")];
        assert_eq!(saved_paths(folder.path())?, left);
        Ok(())
    }
}
