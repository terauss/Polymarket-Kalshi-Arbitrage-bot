//! Tailscale verification and peer discovery for controller/trader communication.

pub mod config;
pub mod verify;

pub use config::Config;
pub use verify::{TailscaleError, TailscaleStatus};
