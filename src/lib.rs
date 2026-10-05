//! Saves the GlazeWM session so a reboot can bring every workspace and window back.

mod account;
mod apps;
pub mod capture;
mod cwd;
pub mod error;
#[cfg(test)]
mod fixtures;
mod glazewm;
mod launch;
pub mod list;
pub mod model;
mod plan;
mod process;
pub mod restore;
pub mod status;
pub mod store;
mod tray;
mod uncloak;
pub mod watch;
