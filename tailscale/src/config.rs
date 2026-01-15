//! Configuration file management for ~/.arb/config.toml

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const CONFIG_DIR: &str = ".arb";
const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Controller,
    Trader,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub role: Role,
    #[serde(default = "default_beacon_port")]
    pub beacon_port: u16,
    #[serde(default = "default_ws_port")]
    pub ws_port: u16,
}

fn default_beacon_port() -> u16 {
    9000
}

fn default_ws_port() -> u16 {
    9001
}

impl Default for Config {
    fn default() -> Self {
        Self {
            role: Role::Trader,
            beacon_port: default_beacon_port(),
            ws_port: default_ws_port(),
        }
    }
}

impl Config {
    /// Get the config file path (~/.arb/config.toml)
    pub fn path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Could not determine home directory")?;
        Ok(home.join(CONFIG_DIR).join(CONFIG_FILE))
    }

    /// Load config from ~/.arb/config.toml
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Could not read config file: {}", path.display()))?;
        let config: Config = toml::from_str(&contents)
            .with_context(|| format!("Could not parse config file: {}", path.display()))?;
        Ok(config)
    }

    /// Save config to ~/.arb/config.toml
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Could not create config directory: {}", parent.display()))?;
        }
        let contents = toml::to_string_pretty(self).context("Could not serialize config")?;
        fs::write(&path, contents)
            .with_context(|| format!("Could not write config file: {}", path.display()))?;
        Ok(())
    }

    /// Check if config file exists
    pub fn exists() -> bool {
        Self::path().map(|p| p.exists()).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_serialization() {
        let config = Config {
            role: Role::Controller,
            beacon_port: 9000,
            ws_port: 9001,
        };

        let toml_str = toml::to_string(&config).unwrap();
        assert!(toml_str.contains("role = \"controller\""));
        assert!(toml_str.contains("beacon_port = 9000"));
        assert!(toml_str.contains("ws_port = 9001"));

        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.role, Role::Controller);
    }

    #[test]
    fn test_config_deserialization() {
        let toml_str = r#"
            role = "trader"
            beacon_port = 8000
            ws_port = 8001
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.role, Role::Trader);
        assert_eq!(config.beacon_port, 8000);
        assert_eq!(config.ws_port, 8001);
    }
}
