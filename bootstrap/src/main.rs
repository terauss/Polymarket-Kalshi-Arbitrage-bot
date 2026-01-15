//! Bootstrap CLI for arbitrage bot Tailscale setup.
//!
//! Guides user through:
//! 1. Tailscale verification
//! 2. Role selection (controller/trader)
//! 3. Config file creation
//! 4. Optionally launching the appropriate binary

use anyhow::Result;
use std::io::{self, Write};
use std::process::Command;
use tailscale::config::{Config, Role};
use tailscale::verify;

fn main() -> Result<()> {
    println!("=== Arbitrage Bot Setup ===\n");

    // Step 1: Verify Tailscale
    print!("Checking Tailscale... ");
    io::stdout().flush()?;

    let status = match verify::verify() {
        Ok(s) => {
            println!("Connected as {}", s.self_ip);
            s
        }
        Err(e) => {
            println!("FAILED");
            eprintln!("\nError: {}", e);
            eprintln!("\nPlease ensure Tailscale is installed and connected:");
            eprintln!("  1. Install: brew install tailscale");
            eprintln!("  2. Start daemon: sudo tailscaled");
            eprintln!("  3. Connect: tailscale up");
            std::process::exit(1);
        }
    };

    // Step 2: Show peers
    if status.peers.is_empty() {
        println!("\nNo peers found on your Tailnet.");
        println!("Make sure the other machine has joined your Tailnet.");
        println!("You can continue setup, but the beacon won't find anyone yet.\n");
    } else {
        println!("Found {} peer(s): {:?}\n", status.peers.len(), status.peers);
    }

    // Step 3: Prompt for role
    println!("What role is this machine?");
    println!("  1) Controller (runs arbitrage detection, hosts WebSocket server)");
    println!("  2) Trader (executes trades, connects to controller)");
    print!("\nEnter choice [1/2]: ");
    io::stdout().flush()?;

    let role = loop {
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        match input.trim() {
            "1" => break Role::Controller,
            "2" => break Role::Trader,
            _ => {
                print!("Invalid choice. Enter 1 or 2: ");
                io::stdout().flush()?;
            }
        }
    };

    // Step 4: Write config
    let config = Config {
        role: role.clone(),
        beacon_port: 9000,
        ws_port: 9001,
    };

    config.save()?;
    let config_path = Config::path()?;
    println!("\nConfig written to {}", config_path.display());

    // Step 5: Offer to launch
    let binary_name = match role {
        Role::Controller => "controller",
        Role::Trader => "remote-trader",
    };

    println!("\nStart {} now? [Y/n]: ", binary_name);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim().to_lowercase();

    if input.is_empty() || input == "y" || input == "yes" {
        println!("\nLaunching {}...\n", binary_name);

        // Try cargo run first (development), fall back to direct binary
        let status = Command::new("cargo")
            .args(["run", "-p", binary_name, "--release"])
            .status();

        match status {
            Ok(s) => std::process::exit(s.code().unwrap_or(1)),
            Err(_) => {
                // Try direct binary
                let status = Command::new(binary_name).status();
                match status {
                    Ok(s) => std::process::exit(s.code().unwrap_or(1)),
                    Err(e) => {
                        eprintln!("Failed to launch {}: {}", binary_name, e);
                        eprintln!("Run manually: cargo run -p {} --release", binary_name);
                        std::process::exit(1);
                    }
                }
            }
        }
    } else {
        println!("\nSetup complete. Run manually:");
        println!("  cargo run -p {} --release", binary_name);
    }

    Ok(())
}
