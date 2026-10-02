//! `rallypoint save` writes the current GlazeWM session to `%LOCALAPPDATA%\rallypoint\sessions\<unix ms>.json` and
//! prints the path. `rallypoint list` prints every saved session, newest first. `rallypoint restore` brings the newest
//! session back, or `rallypoint restore <path>` the one at that path, and prints one line per saved window.
//! `rallypoint watch` saves after window events until GlazeWM exits, printing each path written. Exits 1 when a
//! window was not restored, 2 on error.

use std::path::Path;
use std::process::ExitCode;

use rallypoint::error::Error;
use rallypoint::session::Session;
use rallypoint::{restore, session, watch};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [command] if command == "save" => save(),
        [command] if command == "list" => list(),
        [command] if command == "restore" => session::sessions_folder()
            .and_then(|folder| session::latest(&folder))
            .and_then(restore),
        [command, path] if command == "restore" => session::read(Path::new(path)).and_then(restore),
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
    let path = session::write(&session::sessions_folder()?, &session::capture()?)?;
    println!("{}", path.display());
    Ok(ExitCode::SUCCESS)
}

fn list() -> Result<ExitCode, Error> {
    let now = session::now()?;
    for path in session::newest_first(&session::sessions_folder()?)? {
        let saved = session::read(&path)?;
        println!(
            "{}\t{}\t{} windows on {} workspaces",
            path.display(),
            session::age(saved.saved_at, now),
            saved.windows.len(),
            session::workspace_count(&saved.windows)
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn watch() -> Result<ExitCode, Error> {
    watch::watch(&session::sessions_folder()?)?;
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
