use serde::Deserialize;
use std::fs;
use std::io;
use std::path::PathBuf;
use toml;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub logs: Vec<LogConfig>,
    // pub regexps: Vec<RegexConfig>,
    // pub global: GlobalConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct GlobalConfig {
    pub auto_wrapping: Option<bool>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RegexConfig {}

#[derive(Deserialize, Debug, Clone)]
pub struct LogConfig {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub log_path: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub ssh_key: Option<String>,
    pub max_history: Option<usize>,
}

pub fn read_config(path: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let config_path = expand_tilde(path)?;
    let content = fs::read_to_string(config_path)?;
    let config: Config = toml::from_str(&content)?;
    Ok(config)
}

fn expand_tilde(path: &str) -> io::Result<PathBuf> {
    if path.starts_with("~/") {
        let home = std::env::var("HOME").map_err(|_| {
            io::Error::new(io::ErrorKind::NotFound, "HOME environment variable not set")
        })?;
        Ok(PathBuf::from(home).join(&path[2..]))
    } else {
        Ok(PathBuf::from(path))
    }
}
