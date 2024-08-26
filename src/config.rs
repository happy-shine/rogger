use std::fs;
use serde::Deserialize;
use toml;
use std::path::PathBuf;
use std::io;

#[derive(Deserialize, Debug)]
pub struct Config {
    pub logs: Vec<LogConfig>,
}

#[derive(Deserialize, Debug)]
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
        let home = std::env::var("HOME").map_err(|_| io::Error::new(io::ErrorKind::NotFound, "HOME environment variable not set"))?;
        Ok(PathBuf::from(home).join(&path[2..]))
    } else {
        Ok(PathBuf::from(path))
    }
}
