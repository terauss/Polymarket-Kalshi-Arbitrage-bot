//! Tailscale verification and peer discovery for controller/trader communication.

pub mod verify;

pub use verify::{TailscaleError, TailscaleStatus};
