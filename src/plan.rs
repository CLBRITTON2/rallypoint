//! Decides what a restore does with each saved window: which open window it already is, or what to launch for it.

use std::fmt;
use std::path::PathBuf;

use crate::apps::{self, LaunchScope};
use crate::capture::LiveWindow;
use crate::model::{ExePath, Owner, SavedWindow};

/// What tells an open window apart as the one a saved window was.
#[derive(PartialEq, Debug)]
pub enum Key {
    /// A wezterm-gui window, by who runs it and the folder of its first pane.
    Wezterm {
        executable_path: ExePath,
        owner: Owner,
        cwd: PathBuf,
    },
    /// A console shell, by who runs it and its folder. None is a folder save could not read, matching any.
    Shell {
        executable_path: ExePath,
        owner: Owner,
        cwd: Option<PathBuf>,
    },
    /// A Windows Terminal window, by who runs it and its tabs. No tabs is a window save found no tab shells in,
    /// matching any.
    WindowsTerminal { owner: Owner, tabs: Vec<TabKey> },
    /// A Store app window, by who runs it and its AUMID. Its windows are told apart only by order.
    Packaged { owner: Owner, aumid: String },
    /// Any other window, by who runs it and its program. Its windows are told apart only by order.
    Program {
        executable_path: ExePath,
        owner: Owner,
    },
}

/// Why restore leaves a saved window alone.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SkipReason {
    Protected,
    NoPanes,
    StateMismatch,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SkipReason::Protected => "no executable path, a protected process",
            SkipReason::NoPanes => "terminal window without panes",
            SkipReason::StateMismatch => "saved app state does not match its program",
        })
    }
}

impl Key {
    /// Whether an open window with key `live` is the saved window with this key.
    fn matches(&self, live: &Key) -> bool {
        match (self, live) {
            (
                Key::Shell {
                    executable_path,
                    owner,
                    cwd: None,
                },
                Key::Shell {
                    executable_path: live_path,
                    owner: live_owner,
                    ..
                },
            ) => executable_path == live_path && owner == live_owner,
            (
                Key::WindowsTerminal { owner, tabs },
                Key::WindowsTerminal {
                    owner: live_owner, ..
                },
            ) if tabs.is_empty() => owner == live_owner,
            _ => self == live,
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct TabKey {
    pub executable_path: Option<ExePath>,
    pub cwd: Option<PathBuf>,
}

/// A program to start, as `owner`.
#[derive(PartialEq, Debug)]
pub struct Launch {
    pub owner: Owner,
    pub program: ExePath,
    /// Passed verbatim, so the saved command line's quoting survives.
    pub arguments: String,
    pub start: Start,
}

#[derive(PartialEq, Debug)]
pub enum Start {
    /// With no console and no output, so an app's logging never buries the report.
    Detached,
    /// In a console of its own, in `cwd` when one was saved.
    Console { cwd: Option<PathBuf> },
}

impl Launch {
    pub fn command_line(&self) -> String {
        format!("\"{}\" {}", self.program, self.arguments)
            .trim_end()
            .to_string()
    }
}

/// For each saved window, the index of the open window it is, in the session's order. Exact keys claim first, so a
/// saved window whose key matches any (a shell with no folder) never takes the open window of one with a folder.
/// Within a pass an open window goes to the first saved window it fits.
pub fn assign(saved: &[SavedWindow], live: &[LiveWindow]) -> Vec<Option<usize>> {
    let saved_keys: Vec<Option<Key>> = saved.iter().map(|window| apps::key(window).ok()).collect();
    let live_keys: Vec<Option<Key>> = live
        .iter()
        .map(|live| apps::key(&live.window).ok())
        .collect();
    let exact = claim(
        &saved_keys,
        &live_keys,
        &vec![None; saved.len()],
        <Key as PartialEq>::eq,
    );
    claim(&saved_keys, &live_keys, &exact, Key::matches)
}

/// `assigned`, with each saved key not assigned yet given the first unclaimed open window that `fits` it.
fn claim(
    saved_keys: &[Option<Key>],
    live_keys: &[Option<Key>],
    assigned: &[Option<usize>],
    fits: fn(&Key, &Key) -> bool,
) -> Vec<Option<usize>> {
    let mut claimed: Vec<bool> = (0..live_keys.len())
        .map(|index| assigned.contains(&Some(index)))
        .collect();
    saved_keys
        .iter()
        .zip(assigned)
        .map(|(wanted, assigned)| {
            if assigned.is_some() {
                return *assigned;
            }
            let wanted = wanted.as_ref()?;
            let index = live_keys
                .iter()
                .zip(&claimed)
                .position(|(live_key, taken)| {
                    !taken
                        && live_key
                            .as_ref()
                            .is_some_and(|live_key| fits(wanted, live_key))
                })?;
            if let Some(taken) = claimed.get_mut(index) {
                *taken = true;
            }
            Some(index)
        })
        .collect()
}

/// What to start for the saved windows not open yet, each with the indexes of the windows it brings back, one launch
/// per window or per program as its [`LaunchScope`] says.
pub fn launches(saved: &[SavedWindow], open: &[Option<usize>]) -> Vec<(Launch, Vec<usize>)> {
    let mut planned: Vec<(Key, Launch, Vec<usize>)> = Vec::new();
    for (index, window) in saved.iter().enumerate() {
        if open.get(index).is_some_and(Option::is_some) {
            continue;
        }
        let Ok(key) = apps::key(window) else {
            continue;
        };
        let per_program =
            apps::kind_of(&window.process_name).launch_scope() == LaunchScope::PerProgram;
        if let Some((_, _, windows)) = planned
            .iter_mut()
            .find(|(planned_key, _, _)| per_program && *planned_key == key)
        {
            windows.push(index);
            continue;
        }
        let launch = apps::launch(window, &key);
        planned.push((key, launch, vec![index]));
    }
    planned
        .into_iter()
        .map(|(_, launch, windows)| (launch, windows))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::{wezterm, windows_terminal};
    use crate::fixtures;
    use crate::model::{AppState, Resume, Tab, WeztermPane, WeztermTab};

    fn program(path: &str, command_line: &str) -> SavedWindow {
        SavedWindow {
            command_line: Some(command_line.to_string()),
            ..fixtures::window("app", Some(path))
        }
    }

    fn terminal(cwd: &str, claude_session_id: Option<&str>) -> SavedWindow {
        let pane = WeztermPane {
            resume: claude_session_id.map(|session_id| Resume::ClaudeCode {
                session_id: session_id.to_string(),
            }),
            ..fixtures::pane(cwd, 0, 0, 80, 24)
        };
        SavedWindow {
            app: AppState::Wezterm {
                tabs: vec![WeztermTab { panes: vec![pane] }],
            },
            ..fixtures::window(
                wezterm::PROCESS_NAME,
                Some(r"C:\Program Files\WezTerm\wezterm-gui.exe"),
            )
        }
    }

    fn shell(cwd: Option<&str>) -> SavedWindow {
        SavedWindow {
            app: AppState::Shell {
                cwd: cwd.map(PathBuf::from),
            },
            command_line: Some("pwsh -NoExit -Command app".to_string()),
            ..fixtures::window("pwsh", Some(PWSH))
        }
    }

    fn windows_terminal(tabs: Vec<Tab>) -> SavedWindow {
        SavedWindow {
            app: AppState::WindowsTerminal { tabs },
            ..fixtures::window(
                windows_terminal::PROCESS_NAME,
                Some(
                    r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1\WindowsTerminal.exe",
                ),
            )
        }
    }

    fn tab(path: &str, command_line: &str, cwd: Option<&str>) -> Tab {
        Tab {
            program: Some(ExePath::new(path.to_string())),
            command_line: Some(command_line.to_string()),
            cwd: cwd.map(PathBuf::from),
        }
    }

    fn packaged() -> SavedWindow {
        SavedWindow {
            app: AppState::Packaged {
                aumid: "Example.App_0123456789abc!App".to_string(),
            },
            ..fixtures::window("app", Some(PACKAGED))
        }
    }

    fn live(window: SavedWindow) -> LiveWindow {
        LiveWindow {
            id: String::new(),
            pid: 0,
            window,
        }
    }

    const PWSH: &str = r"C:\Program Files\PowerShell\7\pwsh.exe";
    const PACKAGED: &str = r"C:\Program Files\WindowsApps\Example_1\app.exe";

    #[test]
    fn launches_open_each_windows_terminal_window_with_its_tabs() {
        let saved = vec![
            windows_terminal(vec![
                tab(
                    PWSH,
                    r#""C:\Program Files\PowerShell\7\pwsh.exe" -c x"#,
                    Some(r"C:\a;b"),
                ),
                tab(r"C:\WINDOWS\system32\cmd.exe", "cmd", None),
                tab(r"C:\tools\app.exe", "app.exe a;b", None),
            ]),
            windows_terminal(Vec::new()),
        ];
        let planned: Vec<(String, Vec<usize>)> = launches(&saved, &[None, None])
            .into_iter()
            .map(|(launch, windows)| (launch.command_line(), windows))
            .collect();
        assert_eq!(
            planned,
            vec![
                (
                    r#""wt.exe" -w new new-tab -d "C:\a\;b" "C:\Program Files\PowerShell\7\pwsh.exe" ; new-tab "C:\WINDOWS\system32\cmd.exe" ; new-tab app.exe a\;b"#
                        .to_string(),
                    vec![0]
                ),
                (r#""wt.exe" -w new"#.to_string(), vec![1]),
            ]
        );
    }

    #[test]
    fn assign_matches_windows_terminal_by_tabs_and_a_tabless_window_by_owner() {
        let at = |cwd: &str| windows_terminal(vec![tab(PWSH, "pwsh", Some(cwd))]);
        let saved = vec![at(r"C:\a"), windows_terminal(Vec::new()), at(r"C:\c")];
        let open = vec![live(at(r"C:\b")), live(at(r"C:\a"))];
        assert_eq!(assign(&saved, &open), vec![Some(1), Some(0), None]);
    }

    #[test]
    fn assign_matches_a_shell_by_folder_and_an_unread_folder_by_program() {
        let saved = vec![
            shell(Some(r"C:\work\project")),
            shell(None),
            shell(Some(r"C:\a")),
        ];
        let open = vec![
            live(shell(Some(r"C:\b"))),
            live(shell(Some(r"C:\work\project"))),
        ];
        assert_eq!(assign(&saved, &open), vec![Some(1), Some(0), None]);
    }

    #[test]
    fn launches_open_each_shell_in_its_folder_without_its_arguments() {
        let saved = vec![shell(Some(r"C:\work\project")), shell(None)];
        let planned: Vec<(Launch, Vec<usize>)> = launches(&saved, &[None, None]);
        let program = ExePath::new(PWSH.to_string());
        let expected = |cwd: Option<&str>, index: usize| {
            (
                Launch {
                    owner: Owner::new("owner".to_string()),
                    program: program.clone(),
                    arguments: String::new(),
                    start: Start::Console {
                        cwd: cwd.map(PathBuf::from),
                    },
                },
                vec![index],
            )
        };
        assert_eq!(
            planned,
            vec![expected(Some(r"C:\work\project"), 0), expected(None, 1)]
        );
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
    fn launches_start_a_program_once_per_owner() {
        let other = SavedWindow {
            owner: Owner::new("other".to_string()),
            ..program(r"C:\f.exe", "")
        };
        let saved = vec![program(r"C:\f.exe", ""), other.clone()];
        let owners: Vec<(Owner, Vec<usize>)> = launches(&saved, &[None, None])
            .into_iter()
            .map(|(launch, windows)| (launch.owner, windows))
            .collect();
        assert_eq!(
            owners,
            vec![
                (Owner::new("owner".to_string()), vec![0]),
                (other.owner.clone(), vec![1])
            ]
        );
        assert_eq!(
            assign(&[other], &[live(program(r"C:\f.exe", ""))]),
            vec![None]
        );
    }

    #[test]
    fn key_skips_what_cannot_be_launched() {
        let protected = SavedWindow {
            executable_path: None,
            ..program("", "")
        };
        assert!(matches!(apps::key(&protected), Err(SkipReason::Protected)));
        let paneless = SavedWindow {
            app: AppState::Wezterm { tabs: Vec::new() },
            ..terminal(r"C:\a", None)
        };
        assert!(matches!(apps::key(&paneless), Err(SkipReason::NoPanes)));
    }

    #[test]
    fn launches_start_a_packaged_app_once_by_its_aumid() {
        let planned: Vec<(String, Vec<usize>)> = launches(&[packaged(), packaged()], &[None, None])
            .into_iter()
            .map(|(launch, windows)| (launch.command_line(), windows))
            .collect();
        assert_eq!(
            planned,
            vec![(
                r#""explorer.exe" shell:AppsFolder\Example.App_0123456789abc!App"#.to_string(),
                vec![0, 1]
            )]
        );
    }

    #[test]
    fn assign_gives_exact_keys_their_window_before_wildcards() {
        let saved = vec![shell(None), shell(Some(r"C:\work\project"))];
        let open = vec![live(shell(Some(r"C:\work\project")))];
        assert_eq!(assign(&saved, &open), vec![None, Some(0)]);
    }

    #[test]
    fn assign_ignores_the_case_of_executable_paths() {
        let saved = vec![program(r"C:\Tools\App.exe", "")];
        let open = vec![live(program(r"c:\tools\app.exe", ""))];
        assert_eq!(assign(&saved, &open), vec![Some(0)]);
    }

    #[test]
    fn assign_matches_an_open_packaged_app() {
        let saved = vec![packaged()];
        let open = vec![live(packaged())];
        assert_eq!(assign(&saved, &open), vec![Some(0)]);
    }
}
