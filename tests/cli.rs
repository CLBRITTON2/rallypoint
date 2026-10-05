//! Runs the rallypoint binary against a temporary `LOCALAPPDATA`, so no GlazeWM or desktop is needed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rallypoint::model::VERSION;
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::w;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn run(local_app_data: &Path, args: &[&str]) -> Result<Output, std::io::Error> {
    Command::new(env!("CARGO_BIN_EXE_rallypoint"))
        .args(args)
        .env("LOCALAPPDATA", local_app_data)
        .output()
}

fn sessions_folder(local_app_data: &Path) -> Result<PathBuf, std::io::Error> {
    let folder = local_app_data.join("rallypoint").join("sessions");
    fs::create_dir_all(&folder)?;
    Ok(folder)
}

#[test]
fn an_unknown_command_is_a_usage_error() -> TestResult {
    let local_app_data = tempfile::tempdir()?;
    let output = run(local_app_data.path(), &["unknown"])?;
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr)?,
        "rallypoint: unknown command 'unknown'\nRun 'rallypoint --help' to see the commands.\n"
    );
    Ok(())
}

#[test]
fn help_lists_every_command() -> TestResult {
    let local_app_data = tempfile::tempdir()?;
    let output = run(local_app_data.path(), &["--help"])?;
    assert_eq!(output.status.code(), Some(0));
    let help = String::from_utf8(output.stdout)?;
    for command in ["save", "list", "restore [<path>]", "watch", "status"] {
        assert!(
            help.contains(&format!("\n  {command} ")),
            "{command} missing from {help}"
        );
    }
    Ok(())
}

#[test]
fn no_command_prints_the_help_as_a_usage_error() -> TestResult {
    let local_app_data = tempfile::tempdir()?;
    let output = run(local_app_data.path(), &[])?;
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr)?.contains("Usage: rallypoint <command>"));
    Ok(())
}

#[test]
fn watch_refuses_to_start_while_another_holds_the_lock() -> TestResult {
    // SAFETY: no security attributes, and the name is a static string. The handle closes when the test exits.
    unsafe { CreateMutexW(None, false, w!("Global\\rallypoint-watch")) }?;
    let local_app_data = tempfile::tempdir()?;
    let output = run(local_app_data.path(), &["watch"])?;
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr)?,
        "rallypoint: another rallypoint watch is already running, rallypoint status shows its pid\n"
    );
    Ok(())
}

#[test]
fn list_prints_nothing_before_the_first_save() -> TestResult {
    let local_app_data = tempfile::tempdir()?;
    let output = run(local_app_data.path(), &["list"])?;
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    Ok(())
}

#[test]
fn restore_before_the_first_save_names_the_missing_session() -> TestResult {
    let local_app_data = tempfile::tempdir()?;
    let output = run(local_app_data.path(), &["restore"])?;
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)?.contains("holds no session, run rallypoint save first")
    );
    Ok(())
}

#[test]
fn list_goes_on_past_a_session_it_cannot_read() -> TestResult {
    let local_app_data = tempfile::tempdir()?;
    let folder = sessions_folder(local_app_data.path())?;
    fs::write(folder.join("1.json"), r#"{"saved_at":1,"windows":[]}"#)?;
    let readable = format!(
        r#"{{"version":{VERSION},"saved_at":2,"focus":{{"displayed":[],"focused":null}},"windows":[]}}"#
    );
    fs::write(folder.join("2.json"), readable)?;
    let output = run(local_app_data.path(), &["list"])?;
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout)?;
    assert_eq!(stdout.lines().count(), 1);
    assert!(stdout.contains("2.json"));
    assert!(String::from_utf8(output.stderr)?.contains("1.json"));
    Ok(())
}
