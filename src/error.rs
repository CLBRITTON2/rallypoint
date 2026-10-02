use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "usage: rallypoint save | rallypoint list | rallypoint restore [<session path>] | rallypoint watch, got {0:?}"
    )]
    Usage(Vec<String>),
    #[error("connecting to GlazeWM at {url} failed, is it running? {source}")]
    GlazeConnect {
        url: &'static str,
        #[source]
        source: Box<tungstenite::Error>,
    },
    #[error("talking to GlazeWM failed while sending {message:?}: {source}")]
    GlazeSocket {
        message: String,
        #[source]
        source: Box<tungstenite::Error>,
    },
    #[error("GlazeWM sent a reply to {message:?} that is not valid: {source}, reply: {reply}")]
    GlazeReply {
        message: String,
        reply: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("GlazeWM refused {message:?}: {reason:?}")]
    GlazeRefused {
        message: String,
        reason: Option<String>,
    },
    #[error("GlazeWM exited before the restore started")]
    GlazeExiting,
    #[error("window handle {handle} has no process")]
    NoProcess { handle: isize },
    #[error("connecting to WMI failed: {0}")]
    WmiConnect(#[source] wmi::WMIError),
    #[error("WMI {query} failed: {source}")]
    Wmi {
        query: String,
        #[source]
        source: wmi::WMIError,
    },
    #[error("process {pid} of window handle {handle} exited during the save")]
    ProcessGone { pid: u32, handle: isize },
    #[error("GetOwner of process {pid} returned {code}")]
    Owner { pid: u32, code: u32 },
    #[error("the environment variable {name} is not set")]
    Env { name: &'static str },
    #[error("running {program} failed: {source}")]
    Spawn {
        program: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("{call} failed while keeping rallypoint's output from launched programs: {source}")]
    Inherit {
        call: &'static str,
        #[source]
        source: windows::core::Error,
    },
    #[error("launching {command_line} failed: {source}")]
    Launch {
        command_line: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "runas /user:{owner} /savecred {command_line} exited {code:?}: {output}. If no password is saved for {owner}, \
         run it once in a terminal to save one"
    )]
    Runas {
        owner: String,
        command_line: String,
        code: Option<i32>,
        output: String,
    },
    #[error("wezterm cli list against {socket:?} exited {code:?}: {stderr}")]
    Wezterm {
        socket: PathBuf,
        code: Option<i32>,
        stderr: String,
    },
    #[error("wezterm cli list against {socket:?} printed panes that are not valid: {source}")]
    WeztermParse {
        socket: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("wezterm process {pid} holds {windows} windows, so its panes cannot be told apart")]
    WeztermWindows { pid: u32, windows: usize },
    #[error("the pane cwd {cwd:?} is not a local file URI")]
    PaneCwd { cwd: String },
    #[error("reading {path:?} failed: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the title record in {path:?} is not valid: {source}")]
    SessionRecord {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("writing {path:?} failed: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("removing {path:?} failed: {source}")]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{call} failed while opening the shutdown window: {source}")]
    Window {
        call: &'static str,
        #[source]
        source: windows::core::Error,
    },
    #[error("{call} failed while uncloaking window handle {handle:?}: {source}")]
    Uncloak {
        call: &'static str,
        handle: Option<isize>,
        #[source]
        source: windows::core::Error,
    },
    #[error(
        "the shell has no application view for window handle {handle}, so it cannot be uncloaked"
    )]
    NoView { handle: isize },
    #[error("the {thread} thread stopped without reporting why")]
    ThreadGone { thread: &'static str },
    #[error("{failures} saves in a row failed, the last with: {source}")]
    SavesFailing {
        failures: u32,
        #[source]
        source: Box<Error>,
    },
    #[error("{path:?} is not named <unix ms>.json, so it cannot be ordered among the sessions")]
    SessionName { path: PathBuf },
    #[error("{folder:?} holds no session, run rallypoint save first")]
    NoSession { folder: PathBuf },
    #[error("the session {path:?} is not valid: {source}")]
    Decode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("encoding the session failed: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("the system clock is before 1970: {0}")]
    Clock(#[source] std::time::SystemTimeError),
}
