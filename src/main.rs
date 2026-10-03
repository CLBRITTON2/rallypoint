//! `rallypoint save` writes the current GlazeWM session to `%LOCALAPPDATA%\rallypoint\sessions\<unix ms>.json` and
//! prints the path. `rallypoint list` prints every saved session, newest first. `rallypoint restore` brings the newest
//! session back, or `rallypoint restore <path>` the one at that path, and prints one line per saved window.
//! `rallypoint watch` saves after window events until GlazeWM exits, printing each path written. Exits 1 when a
//! window was not restored, 2 on error.

use std::path::Path;
use std::process::ExitCode;

use rallypoint::error::Error;
use rallypoint::model::{SavedWindow, Session};
use rallypoint::{capture, restore, store, watch};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [command] if command == "save" => save(),
        [command] if command == "list" => list(),
        [command] if command == "restore" => store::sessions_folder()
            .and_then(|folder| store::latest(&folder))
            .and_then(restore),
        [command, path] if command == "restore" => store::read(Path::new(path)).and_then(restore),
        [command] if command == "watch" => watch(),
        _ => Err(Error::Usage(args)),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("rallypoint: {error}");
            ExitCode::from(2)
        }
    }
}

fn save() -> Result<ExitCode, Error> {
    let path = store::write(&store::sessions_folder()?, &capture::capture()?)?;
    println!("{}", path.display());
    Ok(ExitCode::SUCCESS)
}

fn list() -> Result<ExitCode, Error> {
    let now = store::now()?;
    for path in store::newest_first(&store::sessions_folder()?)? {
        let saved = store::read(&path)?;
        println!(
            "{}\t{}\t{} windows on {} workspaces",
            path.display(),
            age(saved.saved_at, now),
            saved.windows.len(),
            workspace_count(&saved.windows)
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn watch() -> Result<ExitCode, Error> {
    watch::watch(&store::sessions_folder()?)?;
    Ok(ExitCode::SUCCESS)
}

fn restore(saved: Session) -> Result<ExitCode, Error> {
    let user = std::env::var("USERNAME").map_err(|_| Error::Env { name: "USERNAME" })?;
    let outcomes = restore::restore(&saved, &user)?;
    for (window, outcome) in saved.windows.iter().zip(&outcomes) {
        println!(
            "{}\t{}\t{outcome}\t{}",
            window.workspace, window.process_name, window.title
        );
    }
    match outcomes.iter().all(restore::Outcome::is_restored) {
        true => Ok(ExitCode::SUCCESS),
        false => Ok(ExitCode::from(1)),
    }
}

/// How long before `now` a session saved at `saved_at` was written, in its largest whole unit, as `12 min ago`.
fn age(saved_at: u64, now: u64) -> String {
    let seconds = now.saturating_sub(saved_at) / 1000;
    match seconds {
        0..60 => format!("{seconds} s ago"),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86400),
    }
}

/// How many distinct workspaces `windows` sit on.
fn workspace_count(windows: &[SavedWindow]) -> usize {
    let mut names: Vec<&str> = windows
        .iter()
        .map(|window| window.workspace.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    names.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rallypoint::model::{AppState, WindowState};

    fn window(workspace: &str) -> SavedWindow {
        SavedWindow {
            workspace: workspace.to_string(),
            process_name: "app".to_string(),
            executable_path: None,
            command_line: None,
            owner: "owner".to_string(),
            title: String::new(),
            class_name: String::new(),
            state: WindowState::Tiling,
            app: AppState::Program,
        }
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
        let windows = [window("2"), window("1"), window("2")];
        assert_eq!(workspace_count(&windows), 2);
    }
}
