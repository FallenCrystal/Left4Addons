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
    pub llm_token_path: PathBuf,
    pub db_path: PathBuf,
    pub cache_dir: PathBuf,
    pub image_cache_dir: PathBuf,
    pub download_cache_dir: PathBuf,
    pub filters_dir: PathBuf,
    pub filter_repo_dir: PathBuf,
    pub filter_venv_dir: PathBuf,
    pub filter_runner_path: PathBuf,
}

impl AppPaths {
    pub fn from_runtime_dir(runtime_dir: impl AsRef<Path>) -> Self {
        let root = runtime_dir.as_ref().to_path_buf();
        let config_dir = root.join("config");
        let cache_dir = root.join("cache");
        let filters_dir = root.join("filters");
        Self {
            runtime_dir: root.clone(),
            settings_path: config_dir.join("settings.toml"),
            mirrors_path: config_dir.join("mirrors.json"),
            llm_token_path: config_dir.join("llm_token.sealed"),
            db_path: root.join("l4a_v2.db"),
            image_cache_dir: cache_dir.join("images"),
            download_cache_dir: cache_dir.join("downloading"),
            filter_repo_dir: filters_dir.join("Left4Filters"),
            filter_venv_dir: filters_dir.join("venv"),
            filter_runner_path: filters_dir.join("l4a_filter_runner.py"),
            config_dir,
            cache_dir,
            filters_dir,
        }
    }

    pub fn ensure_dirs(&self) -> Result<(), CoreError> {
        fs::create_dir_all(&self.runtime_dir)?;
        fs::create_dir_all(&self.config_dir)?;
        fs::create_dir_all(&self.cache_dir)?;
        fs::create_dir_all(&self.image_cache_dir)?;
        fs::create_dir_all(&self.download_cache_dir)?;
        fs::create_dir_all(&self.filters_dir)?;
        Ok(())
    }
}

pub(crate) fn normalize_filter_preset_params(cfg: &mut l4a_protocol::ClientFilterConfig) {
    use l4a_protocol::{FilterModelParams, FilterPresetId};

    cfg.local_params = cfg.local_params.sanitized();
    if cfg.preset != FilterPresetId::Custom {
        let preset_defaults = cfg.preset.default_params().sanitized();
        let base_defaults = FilterModelParams::default();
        if cfg.local_params == preset_defaults {
            // Already matches preset defaults
        } else if cfg.local_params == base_defaults && preset_defaults != base_defaults {
            cfg.local_params = preset_defaults;
        } else {
            cfg.preset = FilterPresetId::Custom;
        }
    }
}

pub fn load_config(path: &Path) -> Result<AppConfig, CoreError> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let content = fs::read_to_string(path)?;
    let mut cfg =
        toml::from_str::<AppConfig>(&content).map_err(|e| CoreError::Config(e.to_string()))?;
    normalize_filter_preset_params(&mut cfg.filter);
    Ok(cfg)
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
