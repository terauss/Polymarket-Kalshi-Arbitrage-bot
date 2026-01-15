//! Tailscale verification and peer discovery for controller/trader communication.

pub mod beacon;
pub mod config;
pub mod verify;

pub use beacon::{BeaconListener, BeaconSender, ControllerInfo};
pub use config::Config;
pub use verify::{TailscaleError, TailscaleStatus};
