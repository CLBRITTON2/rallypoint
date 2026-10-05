//! `rallypoint save` writes the current GlazeWM session to `%LOCALAPPDATA%\rallypoint\sessions\<unix ms>.json` and
//! prints the path. `rallypoint list` prints every saved session, newest first. `rallypoint restore` brings the newest
//! session back, or `rallypoint restore <path>` the one at that path, and prints one line per saved window.
//! `rallypoint watch` saves after window events until GlazeWM exits, printing each path written. Exits 1 when a
//! window was not restored, the saved workspaces could not be shown again, or `list` met a session it cannot read,
//! and 2 on any other error or a usage error.

use std::path::Path;
use std::process::ExitCode;

use rallypoint::error::Error;
use rallypoint::model::Session;
use rallypoint::{capture, list, restore, store, watch};

const USAGE: &str = "usage: rallypoint save | rallypoint list | rallypoint restore [<session path>] | rallypoint watch";

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
        _ => {
            eprintln!("{USAGE}, got {args:?}");
            return ExitCode::from(2);
        }
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
    let folder = store::sessions_folder()?;
    let session = capture::capture()?;
    let path = store::write(&folder, &session)?;
    println!("{}", path.display());
    Ok(ExitCode::SUCCESS)
}

/// Prints every readable session, and an error line for each one it cannot read (another format version).
fn list() -> Result<ExitCode, Error> {
    let now = store::now()?;
    let mut unreadable: usize = 0;
    for path in store::newest_first(&store::sessions_folder()?)? {
        match store::read(&path) {
            Ok(saved) => println!("{}", list::summary(&path, &saved, now)),
            Err(error) => {
                unreadable += 1;
                eprintln!("rallypoint: {error}");
            }
        }
    }
    match unreadable {
        0 => Ok(ExitCode::SUCCESS),
        _ => Ok(ExitCode::from(1)),
    }
}

fn watch() -> Result<ExitCode, Error> {
    watch::watch(&store::sessions_folder()?)?;
    Ok(ExitCode::SUCCESS)
}

fn restore(saved: Session) -> Result<ExitCode, Error> {
    let restored = restore::restore(&saved)?;
    for (window, outcome) in saved.windows.iter().zip(&restored.outcomes) {
        println!(
            "{}\t{}\t{outcome}\t{}",
            window.workspace, window.process_name, window.title
        );
    }
    if let Err(error) = &restored.refocus {
        eprintln!("rallypoint: {error}");
    }
    let all_restored = restored.outcomes.iter().all(restore::Outcome::is_restored);
    match all_restored && restored.refocus.is_ok() {
        true => Ok(ExitCode::SUCCESS),
        false => Ok(ExitCode::from(1)),
    }
}
