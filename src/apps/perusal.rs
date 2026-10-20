//! perusal, a terminal reviewer of git working trees. A PowerShell tab running it reopens with it running again,
//! where restore would otherwise start the shell alone.

use crate::apps::{self, arguments_of};
use crate::error::Error;
use crate::model::{ExePath, Perusal};
use crate::process::Processes;

const EXECUTABLE: &str = "perusal.exe";

/// The shells whose `-Command` can start perusal in the reopened tab.
const SHELLS: [&str; 2] = ["pwsh", "powershell"];

/// Whether a tab whose shell is `program` reopens with the perusal it ran.
pub fn reopens_in(program: &ExePath) -> bool {
    let name = apps::program_name(program.as_str());
    SHELLS.iter().any(|shell| name.eq_ignore_ascii_case(shell))
}

/// The perusal that shell `pid` started, the oldest of several. None when it runs none, or only a protected one.
pub fn running_in(processes: &Processes, pid: u32) -> Result<Option<Perusal>, Error> {
    Ok(processes
        .children_named(pid, EXECUTABLE)?
        .into_iter()
        .find_map(|process| {
            let executable_path = process.executable_path?;
            let arguments = arguments_of(&process.command_line?, &executable_path).to_string();
            Some(Perusal {
                executable_path,
                arguments,
            })
        }))
}

/// PowerShell arguments that start `perusal` and keep the shell open once it exits. The saved arguments are read as
/// PowerShell, and one holding a double quote does not survive wt.exe splitting and rejoining the command line.
pub fn shell_arguments(perusal: &Perusal) -> String {
    let path = perusal.executable_path.as_str().replace('\'', "''");
    format!("-NoExit -Command \"& '{path}' {}\"", perusal.arguments)
}
