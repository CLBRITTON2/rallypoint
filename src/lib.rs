//! Saves the GlazeWM session so a reboot can bring every workspace and window back.

pub mod claude;
pub mod cwd;
pub mod error;
#[cfg(test)]
mod fixtures;
pub mod glazewm;
pub mod process;
pub mod restore;
pub mod session;
pub mod tabs;
pub mod uncloak;
pub mod watch;
pub mod wezterm;
