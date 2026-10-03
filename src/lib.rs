//! Saves the GlazeWM session so a reboot can bring every workspace and window back.

pub mod apps;
pub mod capture;
pub mod cwd;
pub mod error;
#[cfg(test)]
mod fixtures;
pub mod glazewm;
pub mod launch;
pub mod model;
pub mod plan;
pub mod process;
pub mod restore;
pub mod store;
pub mod uncloak;
pub mod watch;
