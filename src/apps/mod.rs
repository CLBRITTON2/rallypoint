//! What rallypoint knows about particular programs: which saved windows are terminals or shells, how each records
//! its state, and how each relaunches. Every program name rallypoint treats specially is here.

use std::path::Path;

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

/// The kind of a window whose process GlazeWM names `process_name`.
pub fn kind_of(process_name: &str) -> Kind {
    match process_name {
        wezterm::PROCESS_NAME => Kind::Wezterm,
        windows_terminal::PROCESS_NAME => Kind::WindowsTerminal,
        "pwsh" | "powershell" | "cmd" => Kind::Shell,
        _ => Kind::Program,
    }
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
    fn program_name_is_the_file_stem() {
        assert_eq!(
            program_name(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            "pwsh"
        );
        assert_eq!(program_name(r"C:\WINDOWS\system32\cmd.exe"), "cmd");
    }
}
