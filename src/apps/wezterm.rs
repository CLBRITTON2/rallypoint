//! wezterm windows: their tabs and panes, and the launch that reopens one. Every wezterm-gui process runs its own mux
//! behind `<owner home>\.local\share\wezterm\gui-sock-<pid>`, so the CLI is pointed at that socket.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::num::ParseIntError;
use std::path::{Path, PathBuf};
use std::process::Command;

use percent_encoding::percent_decode_str;
use serde::Deserialize;

use crate::apps::{claude_code, quoted};
use crate::cwd;
use crate::error::Error;
use crate::model::{ExePath, Owner, PaneArea, Resume, WeztermPane, WeztermTab};
use crate::plan::{Key, Launch, SkipReason, Start};
use crate::process::Process;

pub const PROCESS_NAME: &str = "wezterm-gui";

/// A pane as `wezterm cli list` reports it. The title is Claude Code's session title when the pane runs it.
#[derive(Deserialize)]
struct ListedPane {
    window_id: u64,
    tab_id: u64,
    pane_id: u64,
    cwd: String,
    title: String,
    size: ListedSize,
    left_col: u32,
    top_row: u32,
    is_active: bool,
}

#[derive(Deserialize)]
struct ListedSize {
    rows: u32,
    cols: u32,
}

/// The tabs of the wezterm window `process` draws, each pane with the Claude Code session it shows.
pub fn saved_tabs(owner_home: &Path, process: &Process) -> Result<Vec<WeztermTab>, Error> {
    let listed = single_window(list(&socket(owner_home, process.pid))?, process.pid)?;
    by_tab(listed)
        .into_iter()
        .map(|panes| {
            let panes = panes
                .into_iter()
                .map(|pane| saved_pane(owner_home, pane))
                .collect::<Result<Vec<WeztermPane>, Error>>()?;
            Ok(WeztermTab { panes })
        })
        .collect()
}

fn saved_pane(owner_home: &Path, pane: ListedPane) -> Result<WeztermPane, Error> {
    let cwd = local_path(&pane.cwd)?;
    let resume = claude_code::session_id(owner_home, &cwd, &pane.title)?
        .map(|session_id| Resume::ClaudeCode { session_id });
    Ok(WeztermPane {
        cwd,
        resume,
        area: PaneArea {
            left: pane.left_col,
            top: pane.top_row,
            cols: pane.size.cols,
            rows: pane.size.rows,
        },
        active: pane.is_active,
    })
}

/// `listed` grouped by tab, tabs and panes in the order wezterm lists them.
fn by_tab(listed: Vec<ListedPane>) -> Vec<Vec<ListedPane>> {
    let mut tabs: Vec<(u64, Vec<ListedPane>)> = Vec::new();
    for pane in listed {
        match tabs.iter_mut().find(|(tab_id, _)| *tab_id == pane.tab_id) {
            Some((_, panes)) => panes.push(pane),
            None => tabs.push((pane.tab_id, vec![pane])),
        }
    }
    tabs.into_iter().map(|(_, panes)| panes).collect()
}

/// The key of a wezterm window, which the top left pane of its first tab alone tells apart.
pub fn key(
    executable_path: &ExePath,
    owner: &Owner,
    tabs: &[WeztermTab],
) -> Result<Key, SkipReason> {
    let pane = tabs.first().and_then(anchor).ok_or(SkipReason::NoPanes)?;
    Ok(Key::Wezterm {
        executable_path: executable_path.clone(),
        owner: owner.clone(),
        cwd: pane.cwd.clone(),
    })
}

/// The top left pane of `tab`, the one every split of the tab divides.
fn anchor(tab: &WeztermTab) -> Option<&WeztermPane> {
    tab.panes
        .iter()
        .min_by_key(|pane| (pane.area.top, pane.area.left))
}

/// Reopens a wezterm window in `cwd` with the top left pane of its first tab. [`rebuild`] opens the rest.
pub fn launch(executable_path: &ExePath, owner: &Owner, cwd: &Path, tabs: &[WeztermTab]) -> Launch {
    let resume = tabs
        .first()
        .and_then(anchor)
        .and_then(|pane| pane.resume.as_ref());
    Launch {
        owner: owner.clone(),
        program: executable_path.clone(),
        arguments: launch_arguments(cwd, resume),
        start: Start::Detached,
    }
}

/// `wezterm-gui.exe` arguments opening a window in `cwd` that resumes `resume`.
fn launch_arguments(cwd: &Path, resume: Option<&Resume>) -> String {
    let program = pane_program(resume);
    let program = match program.is_empty() {
        true => String::new(),
        false => format!(" -- {}", program.join(" ")),
    };
    format!("start --cwd {}{program}", quoted(cwd))
}

/// What a pane runs: its Claude Code session, or wezterm's default program when it resumes none.
fn pane_program(resume: Option<&Resume>) -> Vec<String> {
    resume
        .map(|Resume::ClaudeCode { session_id }| claude_code::resume_command(session_id))
        .unwrap_or_default()
}

/// Opens the panes of `tabs` beyond the one [`launch`] started, in the window wezterm-gui process `pid` draws, and
/// focuses each tab's saved active pane. wezterm reports no active tab, so the first tab is shown.
pub fn rebuild(owner_home: &Path, pid: u32, tabs: &[WeztermTab]) -> Result<(), Error> {
    let socket = socket(owner_home, pid);
    let listed = single_window(list(&socket)?, pid)?;
    let started = listed.first().ok_or(Error::WeztermEmpty { pid })?;
    let mut active: Vec<u64> = Vec::new();
    for (index, tab) in tabs.iter().enumerate() {
        let layout = layout(tab.panes.iter().collect())?;
        let pane_id = match index {
            0 => started.pane_id,
            _ => {
                let head = vec![
                    "spawn".to_string(),
                    "--window-id".to_string(),
                    started.window_id.to_string(),
                ];
                new_pane(&socket, &with_pane(head, layout.first_pane()))?
            }
        };
        let placed = split(&socket, &layout, pane_id)?;
        active.extend(
            placed
                .iter()
                .find(|(pane, _)| pane.active)
                .map(|(_, pane_id)| *pane_id),
        );
    }
    // Focusing a pane also shows its tab, so the first tab's goes last and stays shown.
    for pane_id in active.iter().rev() {
        cli(
            &socket,
            &[
                "activate-pane".to_string(),
                "--pane-id".to_string(),
                pane_id.to_string(),
            ]
            .map(OsString::from),
        )?;
    }
    Ok(())
}

/// How a tab's panes split, rebuilt from where each pane sits.
#[derive(PartialEq, Debug)]
enum Layout<'a> {
    Pane(&'a WeztermPane),
    /// `first` keeps the place of the pane split, and `second` opens on `side` of it with `percent` of the room.
    Split {
        side: Side,
        percent: u32,
        first: Box<Layout<'a>>,
        second: Box<Layout<'a>>,
    },
}

impl<'a> Layout<'a> {
    /// The pane the layout starts from, which every split in it divides.
    fn first_pane(&self) -> &'a WeztermPane {
        match self {
            Layout::Pane(pane) => pane,
            Layout::Split { first, .. } => first.first_pane(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Side {
    Right,
    Bottom,
}

impl Side {
    fn flag(self) -> &'static str {
        match self {
            Side::Right => "--right",
            Side::Bottom => "--bottom",
        }
    }

    /// The first cell of `area` along the axis this side splits, and the cell past its last.
    fn span(self, area: &PaneArea) -> (u32, u32) {
        match self {
            Side::Right => (area.left, area.left + area.cols),
            Side::Bottom => (area.top, area.top + area.rows),
        }
    }
}

/// The splits that put `panes` where they sit, trying a cut into left and right before one into top and bottom.
fn layout(panes: Vec<&WeztermPane>) -> Result<Layout<'_>, Error> {
    if let [only] = panes.as_slice() {
        return Ok(Layout::Pane(only));
    }
    for side in [Side::Right, Side::Bottom] {
        let spans: Vec<(u32, u32)> = panes.iter().map(|pane| side.span(&pane.area)).collect();
        if let Some((at, percent)) = cut(&spans) {
            let (first, second): (Vec<&WeztermPane>, Vec<&WeztermPane>) =
                panes.iter().partition(|pane| side.span(&pane.area).0 < at);
            return Ok(Layout::Split {
                side,
                percent,
                first: Box::new(layout(first)?),
                second: Box::new(layout(second)?),
            });
        }
    }
    Err(Error::PaneLayout {
        areas: panes.iter().map(|pane| pane.area).collect(),
    })
}

/// Where a line across every span in `spans` can cut them into two parts, as the first cell of the second part and
/// that part's share in percent of the room, the split line's one cell aside. The first such line, if any.
fn cut(spans: &[(u32, u32)]) -> Option<(u32, u32)> {
    let start = spans.iter().map(|(from, _)| *from).min()?;
    let end = spans.iter().map(|(_, to)| *to).max()?;
    let mut starts: Vec<u32> = spans
        .iter()
        .map(|(from, _)| *from)
        .filter(|from| *from > start)
        .collect();
    starts.sort_unstable();
    let at = starts
        .into_iter()
        .find(|at| spans.iter().all(|(from, to)| to < at || from >= at))?;
    let room = (end - start).saturating_sub(1).max(1);
    let percent = ((end - at) * 100 + room / 2) / room;
    // wezterm takes 1 to 99.
    Some((at, percent.clamp(1, 99)))
}

/// Splits pane `pane_id` as `layout` says, and pairs each saved pane with the id of the pane it became.
fn split<'a>(
    socket: &Path,
    layout: &Layout<'a>,
    pane_id: u64,
) -> Result<Vec<(&'a WeztermPane, u64)>, Error> {
    match layout {
        Layout::Pane(pane) => Ok(vec![(*pane, pane_id)]),
        Layout::Split {
            side,
            percent,
            first,
            second,
        } => {
            let head = vec![
                "split-pane".to_string(),
                "--pane-id".to_string(),
                pane_id.to_string(),
                side.flag().to_string(),
                "--percent".to_string(),
                percent.to_string(),
            ];
            let opened = new_pane(socket, &with_pane(head, second.first_pane()))?;
            let mut placed = split(socket, first, pane_id)?;
            placed.extend(split(socket, second, opened)?);
            Ok(placed)
        }
    }
}

/// `wezterm cli` arguments `head`, then the folder and program of `pane`.
fn with_pane(head: Vec<String>, pane: &WeztermPane) -> Vec<OsString> {
    let program = pane_program(pane.resume.as_ref());
    let separator = (!program.is_empty()).then(|| "--".to_string());
    head.into_iter()
        .map(OsString::from)
        .chain([OsString::from("--cwd"), pane.cwd.clone().into_os_string()])
        .chain(separator.into_iter().chain(program).map(OsString::from))
        .collect()
}

fn socket(owner_home: &Path, pid: u32) -> PathBuf {
    owner_home.join(format!(r".local\share\wezterm\gui-sock-{pid}"))
}

/// Runs `wezterm cli <arguments>` against `socket` and returns what it printed. A refused socket fails instead of
/// starting a mux server, which would stay running after each retry.
fn cli(socket: &Path, arguments: &[OsString]) -> Result<Vec<u8>, Error> {
    let output = Command::new("wezterm")
        .args(["cli", "--no-auto-start"])
        .args(arguments)
        .env("WEZTERM_UNIX_SOCKET", socket)
        .output()
        .map_err(|source| Error::Spawn {
            program: "wezterm",
            source,
        })?;
    if !output.status.success() {
        return Err(Error::Wezterm {
            arguments: arguments
                .iter()
                .map(|argument| argument.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
            socket: socket.to_path_buf(),
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(output.stdout)
}

fn list(socket: &Path) -> Result<Vec<ListedPane>, Error> {
    let stdout = cli(socket, &["list", "--format", "json"].map(OsString::from))?;
    serde_json::from_slice(&stdout).map_err(|source| Error::WeztermParse {
        socket: socket.to_path_buf(),
        source,
    })
}

/// Runs a `wezterm cli` command that opens a pane, and returns the id it prints for it.
fn new_pane(socket: &Path, arguments: &[OsString]) -> Result<u64, Error> {
    let stdout = cli(socket, arguments)?;
    let printed = String::from_utf8_lossy(&stdout);
    printed
        .trim()
        .parse()
        .map_err(|source: ParseIntError| Error::WeztermPaneId {
            socket: socket.to_path_buf(),
            printed: printed.to_string(),
            source,
        })
}

/// `listed`, the panes of wezterm-gui process `pid`, when they all belong to one window.
fn single_window(listed: Vec<ListedPane>, pid: u32) -> Result<Vec<ListedPane>, Error> {
    let windows: BTreeSet<u64> = listed.iter().map(|pane| pane.window_id).collect();
    if windows.len() > 1 {
        return Err(Error::WeztermWindows {
            pid,
            windows: windows.len(),
        });
    }
    Ok(listed)
}

/// The path of a `file:///C:/...` URI, without the trailing separator wezterm reports for a folder unless it is a
/// drive root's.
fn local_path(cwd: &str) -> Result<PathBuf, Error> {
    let not_local = || Error::PaneCwd {
        cwd: cwd.to_string(),
    };
    let encoded = cwd.strip_prefix("file:///").ok_or_else(not_local)?;
    let path = percent_decode_str(encoded)
        .decode_utf8()
        .map_err(|source| Error::PaneCwdEncoding {
            cwd: cwd.to_string(),
            source,
        })?
        .replace('/', "\\");
    Ok(cwd::folder(&path.encode_utf16().collect::<Vec<u16>>()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::pane;

    fn listed(window_id: u64, tab_id: u64, pane_id: u64) -> ListedPane {
        ListedPane {
            window_id,
            tab_id,
            pane_id,
            cwd: "file:///C:/work/project/".to_string(),
            title: String::new(),
            size: ListedSize { rows: 1, cols: 1 },
            left_col: 0,
            top_row: 0,
            is_active: false,
        }
    }

    #[test]
    fn local_path_decodes_and_drops_the_trailing_separator() -> Result<(), Error> {
        assert_eq!(
            local_path("file:///C:/Program%20Files/x/")?,
            PathBuf::from(r"C:\Program Files\x")
        );
        assert_eq!(local_path("file:///C:/")?, PathBuf::from(r"C:\"));
        Ok(())
    }

    #[test]
    fn local_path_rejects_a_remote_uri() {
        assert!(matches!(
            local_path("file://server/share/x"),
            Err(Error::PaneCwd { .. })
        ));
    }

    #[test]
    fn local_path_rejects_a_path_that_is_not_utf8() {
        assert!(matches!(
            local_path("file:///C:/%FF"),
            Err(Error::PaneCwdEncoding { .. })
        ));
    }

    #[test]
    fn single_window_rejects_two_windows() {
        let panes = vec![listed(0, 0, 0), listed(1, 1, 1)];
        assert!(matches!(
            single_window(panes, 7),
            Err(Error::WeztermWindows { pid: 7, windows: 2 })
        ));
    }

    #[test]
    fn by_tab_groups_panes_in_listed_order() {
        let tabs: Vec<Vec<u64>> = by_tab(vec![listed(3, 5, 0), listed(3, 2, 1), listed(3, 5, 2)])
            .into_iter()
            .map(|panes| panes.into_iter().map(|pane| pane.pane_id).collect())
            .collect();
        assert_eq!(tabs, vec![vec![0, 2], vec![1]]);
    }

    #[test]
    fn layout_rebuilds_nested_splits_from_pane_areas() -> Result<(), Error> {
        // A 158 by 40 tab: a left pane, and on the right a pane above a smaller one, as wezterm reported it.
        let left = pane(r"C:\a", 0, 0, 110, 40);
        let top_right = pane(r"C:\b", 111, 0, 47, 29);
        let bottom_right = pane(r"C:\c", 111, 30, 47, 10);
        let built = layout(vec![&top_right, &bottom_right, &left])?;
        let expected = Layout::Split {
            side: Side::Right,
            percent: 30,
            first: Box::new(Layout::Pane(&left)),
            second: Box::new(Layout::Split {
                side: Side::Bottom,
                percent: 26,
                first: Box::new(Layout::Pane(&top_right)),
                second: Box::new(Layout::Pane(&bottom_right)),
            }),
        };
        assert_eq!(built, expected);
        assert_eq!(built.first_pane(), &left);
        Ok(())
    }

    #[test]
    fn layout_cuts_three_columns_at_the_first_line() -> Result<(), Error> {
        let a = pane(r"C:\a", 0, 0, 9, 10);
        let b = pane(r"C:\b", 10, 0, 9, 10);
        let c = pane(r"C:\c", 20, 0, 9, 10);
        let expected = Layout::Split {
            side: Side::Right,
            percent: 68,
            first: Box::new(Layout::Pane(&a)),
            second: Box::new(Layout::Split {
                side: Side::Right,
                percent: 50,
                first: Box::new(Layout::Pane(&b)),
                second: Box::new(Layout::Pane(&c)),
            }),
        };
        assert_eq!(layout(vec![&a, &b, &c])?, expected);
        Ok(())
    }

    #[test]
    fn layout_rejects_panes_no_line_cuts_apart() {
        // A pinwheel: every straight line crosses one of the four panes.
        let panes = [
            pane(r"C:\a", 0, 0, 19, 9),
            pane(r"C:\b", 20, 0, 9, 19),
            pane(r"C:\c", 10, 20, 19, 9),
            pane(r"C:\d", 0, 10, 9, 19),
        ];
        assert!(matches!(
            layout(panes.iter().collect()),
            Err(Error::PaneLayout { .. })
        ));
    }

    #[test]
    fn with_pane_runs_the_claude_session_in_the_pane_folder() {
        let resumed = WeztermPane {
            resume: Some(Resume::ClaudeCode {
                session_id: "id".to_string(),
            }),
            ..pane(r"C:\work\project", 0, 0, 1, 1)
        };
        let arguments = |pane: &WeztermPane| -> Vec<String> {
            with_pane(vec!["spawn".to_string()], pane)
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(
            arguments(&resumed),
            [
                "spawn",
                "--cwd",
                r"C:\work\project",
                "--",
                "pwsh",
                "-NoLogo",
                "-Command",
                "claude",
                "--resume",
                "id"
            ]
        );
        assert_eq!(
            arguments(&pane(r"C:\a", 0, 0, 1, 1)),
            ["spawn", "--cwd", r"C:\a"]
        );
    }

    #[test]
    fn key_takes_the_top_left_pane_of_the_first_tab() {
        let tabs = vec![
            WeztermTab {
                panes: vec![pane(r"C:\b", 11, 0, 9, 10), pane(r"C:\a", 0, 0, 10, 10)],
            },
            WeztermTab {
                panes: vec![pane(r"C:\c", 0, 0, 20, 10)],
            },
        ];
        let path = ExePath::new(r"C:\tools\wezterm-gui.exe".to_string());
        let owner = Owner::new("owner".to_string());
        assert_eq!(
            key(&path, &owner, &tabs),
            Ok(Key::Wezterm {
                executable_path: path.clone(),
                owner: owner.clone(),
                cwd: PathBuf::from(r"C:\a"),
            })
        );
    }
}
