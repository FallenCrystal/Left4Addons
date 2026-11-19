use crate::error::CoreError;
use l4a_protocol::AppConfig;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub runtime_dir: PathBuf,
    pub config_dir: PathBuf,
    pub settings_path: PathBuf,
    pub mirrors_path: PathBuf,
    pub db_path: PathBuf,
    pub cache_dir: PathBuf,
    pub image_cache_dir: PathBuf,
    pub download_cache_dir: PathBuf,
}

impl AppPaths {
    pub fn from_runtime_dir(runtime_dir: impl AsRef<Path>) -> Self {
        let root = runtime_dir.as_ref().to_path_buf();
        let config_dir = root.join("config");
        let cache_dir = root.join("cache");
        Self {
            runtime_dir: root.clone(),
            settings_path: config_dir.join("settings.toml"),
            mirrors_path: config_dir.join("mirrors.json"),
            db_path: root.join("l4a_v2.db"),
            image_cache_dir: cache_dir.join("images"),
            download_cache_dir: cache_dir.join("downloading"),
            config_dir,
            cache_dir,
        }
    }

    pub fn ensure_dirs(&self) -> Result<(), CoreError> {
        fs::create_dir_all(&self.runtime_dir)?;
        fs::create_dir_all(&self.config_dir)?;
        fs::create_dir_all(&self.cache_dir)?;
        fs::create_dir_all(&self.image_cache_dir)?;
        fs::create_dir_all(&self.download_cache_dir)?;
        Ok(())
    }
}

pub fn load_config(path: &Path) -> Result<AppConfig, CoreError> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let content = fs::read_to_string(path)?;
    toml::from_str::<AppConfig>(&content).map_err(|e| CoreError::Config(e.to_string()))
}

pub fn save_config(path: &Path, config: &AppConfig) -> Result<(), CoreError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let encoded = toml::to_string_pretty(config).map_err(|e| CoreError::Config(e.to_string()))?;
    fs::write(path, encoded)?;
    Ok(())
}
