//! What rallypoint knows about particular programs: which saved windows are terminals or shells, how each records
//! its state, and how each relaunches. Every program name rallypoint treats specially is here.

use std::path::Path;

use crate::model::{AppState, ExePath, SavedWindow, Tab, WeztermPane};
use crate::plan::{Key, Launch, SkipReason, Start};

pub mod claude_code;
pub mod wezterm;
pub mod windows_terminal;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Wezterm,
    WindowsTerminal,
    /// A console shell, which restore reopens in its saved folder.
    Shell,
    Program,
}

/// How many launches the saved windows of one program take.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LaunchScope {
    PerWindow,
    /// One launch however many windows it had, since a second launch of a browser or chat app opens a stray window
    /// instead of restoring the saved ones.
    PerProgram,
}

impl Kind {
    pub fn launch_scope(self) -> LaunchScope {
        match self {
            Kind::Wezterm | Kind::WindowsTerminal | Kind::Shell => LaunchScope::PerWindow,
            Kind::Program => LaunchScope::PerProgram,
        }
    }
}

const SHELLS: [&str; 3] = ["pwsh", "powershell", "cmd"];

/// The kind of a window whose process GlazeWM names `process_name`, regardless of case, as Windows names files.
pub fn kind_of(process_name: &str) -> Kind {
    let is = |name: &str| process_name.eq_ignore_ascii_case(name);
    if is(wezterm::PROCESS_NAME) {
        Kind::Wezterm
    } else if is(windows_terminal::PROCESS_NAME) {
        Kind::WindowsTerminal
    } else if SHELLS.into_iter().any(is) {
        Kind::Shell
    } else {
        Kind::Program
    }
}

/// The key of `window`, or why it has none.
pub fn key(window: &SavedWindow) -> Result<Key, SkipReason> {
    let executable_path = window
        .executable_path
        .as_ref()
        .ok_or(SkipReason::Protected)?;
    match (kind_of(&window.process_name), &window.app) {
        (Kind::WindowsTerminal, AppState::WindowsTerminal { tabs }) => {
            Ok(windows_terminal::key(&window.owner, tabs))
        }
        (Kind::Wezterm, AppState::Wezterm { panes }) => {
            wezterm::key(executable_path, &window.owner, panes)
        }
        (Kind::Shell, AppState::Shell { cwd }) => Ok(Key::Shell {
            executable_path: executable_path.clone(),
            owner: window.owner.clone(),
            cwd: cwd.clone(),
        }),
        (Kind::Program, AppState::Program) => Ok(Key::Program {
            executable_path: executable_path.clone(),
        }),
        // Every variant by name, so a new one fails to compile here instead of being skipped.
        (
            Kind::Wezterm | Kind::WindowsTerminal | Kind::Shell | Kind::Program,
            AppState::Program
            | AppState::Shell { .. }
            | AppState::Wezterm { .. }
            | AppState::WindowsTerminal { .. },
        ) => Err(SkipReason::StateMismatch),
    }
}

/// What to start to bring back `window`, whose key is `key`.
pub fn launch(window: &SavedWindow, key: &Key) -> Launch {
    match key {
        Key::Wezterm {
            executable_path,
            owner,
            cwd,
        } => wezterm::launch(executable_path, owner, cwd, wezterm_panes(&window.app)),
        Key::WindowsTerminal { owner, .. } => windows_terminal::launch(owner, tabs(&window.app)),
        // Without the saved arguments, so a window opened to run one command (a -Command) does not run it again.
        Key::Shell {
            executable_path,
            owner,
            cwd,
        } => Launch {
            owner: owner.clone(),
            program: executable_path.clone(),
            arguments: String::new(),
            start: Start::Console { cwd: cwd.clone() },
        },
        Key::Program { executable_path } => Launch {
            owner: window.owner.clone(),
            program: executable_path.clone(),
            arguments: window
                .command_line
                .as_deref()
                .map(|command_line| arguments_of(command_line, executable_path))
                .unwrap_or_default()
                .to_string(),
            start: Start::Detached,
        },
    }
}

fn wezterm_panes(app: &AppState) -> &[WeztermPane] {
    match app {
        AppState::Wezterm { panes } => panes,
        AppState::WindowsTerminal { .. } | AppState::Shell { .. } | AppState::Program => &[],
    }
}

fn tabs(app: &AppState) -> &[Tab] {
    match app {
        AppState::WindowsTerminal { tabs } => tabs,
        AppState::Wezterm { .. } | AppState::Shell { .. } | AppState::Program => &[],
    }
}

/// A command line without its leading program, quoted or not. An unquoted program is `executable_path` when the
/// command line starts with it, since that path can hold spaces, and otherwise ends at the first space.
fn arguments_of<'a>(command_line: &'a str, executable_path: &ExePath) -> &'a str {
    let command_line = command_line.trim_start();
    let rest = match command_line.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"').map_or("", |(_, rest)| rest),
        None => match executable_path.strip_from(command_line) {
            Some(rest) if rest.is_empty() || rest.starts_with(' ') => rest,
            _ => command_line.split_once(' ').map_or("", |(_, rest)| rest),
        },
    };
    rest.trim()
}

/// The process name of an executable path, as GlazeWM names a window's process: `pwsh` for `C:\x\pwsh.exe`.
pub fn program_name(executable_path: &str) -> String {
    Path::new(executable_path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// `folder` as one command line argument. A trailing backslash is doubled, since `\"` would escape the closing quote.
pub fn quoted(folder: &Path) -> String {
    let folder = folder.display().to_string();
    if folder.ends_with('\\') {
        format!("\"{folder}\\\"")
    } else {
        format!("\"{folder}\"")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_of_ignores_case() {
        assert_eq!(kind_of("PWSH"), Kind::Shell);
        assert_eq!(kind_of("Cmd"), Kind::Shell);
        assert_eq!(kind_of("windowsterminal"), Kind::WindowsTerminal);
        assert_eq!(kind_of("WezTerm-GUI"), Kind::Wezterm);
        assert_eq!(kind_of("app"), Kind::Program);
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
    fn arguments_of_drops_a_quoted_or_bare_program() {
        let path = ExePath::new(r"C:\tools\x.exe".to_string());
        assert_eq!(
            arguments_of(r#""C:\tools\x.exe" -a "b c""#, &path),
            r#"-a "b c""#
        );
        assert_eq!(arguments_of(r"x.exe -a", &path), "-a");
        assert_eq!(arguments_of(r#""C:\tools\x.exe" "#, &path), "");
        assert_eq!(arguments_of("x.exe", &path), "");
    }

    #[test]
    fn arguments_of_keeps_an_unquoted_program_path_with_spaces_whole() {
        let path = ExePath::new(r"C:\Program Files\x\app.exe".to_string());
        assert_eq!(arguments_of(r"c:\program files\x\app.exe -a", &path), "-a");
        assert_eq!(arguments_of(r"C:\Program Files\x\app.exe", &path), "");
        assert_eq!(
            arguments_of(r"C:\Program Files\x\app.exe2 -a", &path),
            r"Files\x\app.exe2 -a"
        );
    }
}
