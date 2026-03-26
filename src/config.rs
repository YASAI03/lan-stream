use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

const CONFIG_PATH: &str = "config.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub capture: CaptureConfig,
    pub server: ServerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureConfig {
    pub window_title: String,
    pub target_fps: u32,
    #[serde(default = "default_capture_cursor")]
    pub capture_cursor: bool,
    #[serde(default = "default_keyframe_interval")]
    pub keyframe_interval: u32,
}

fn default_capture_cursor() -> bool {
    true
}

fn default_keyframe_interval() -> u32 {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            capture: CaptureConfig {
                window_title: String::new(),
                target_fps: 30,
                capture_cursor: true,
                keyframe_interval: 60,
            },
            server: ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 8080,
            },
        }
    }
}

pub type SharedConfig = Arc<RwLock<Config>>;

pub fn load_config() -> Config {
    let path = Path::new(CONFIG_PATH);
    if path.exists() {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        toml::from_str(&content).unwrap_or_default()
    } else {
        let config = Config::default();
        let _ = save_config_to_file(&config);
        config
    }
}

pub fn save_config_to_file(config: &Config) -> Result<(), String> {
    let content = toml::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(CONFIG_PATH, content).map_err(|e| e.to_string())
}
