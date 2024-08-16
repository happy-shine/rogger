use std::fs;
use serde::Deserialize;
use toml;

#[derive(Deserialize, Debug)]
pub struct Config {
    pub logs: Vec<LogConfig>,
}

#[derive(Deserialize, Debug)]
pub struct LogConfig {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub ssh_key: Option<String>,
    pub log_path: String,
}

pub fn read_config(path: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let config: Config = toml::from_str(&content)?;
    Ok(config)
}


mod tests {
    use super::*;

    #[test]
    fn test_read_config() {
        let config = read_config("config.toml").expect("Failed to read config");
        for log in config.logs {
            println!("{:?}", log);
        }
    }
}