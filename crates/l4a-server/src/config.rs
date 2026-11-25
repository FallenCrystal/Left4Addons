use crate::credentials::write_private_file;
use l4a_protocol::{
    CatalogFilterBackendKind, CatalogFilterConfigDto, CatalogSyncConfigDto,
    FilterGalleryAggregation, FilterModelParams, SteamAuthMode,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogConfig {
    pub server: ServerSection,
    pub admin: AdminSection,
    pub steam: SteamSection,
    pub sync: SyncSection,
    pub search: SearchSection,
    pub filter: FilterSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSection {
    pub bind: String,
    pub db_path: PathBuf,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8787".to_string(),
            db_path: PathBuf::from("./l4a-catalog.db"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AdminSection {
    pub username: String,
    pub password: String,
    pub session_ttl_hours: u64,
    pub allow_insecure_http: bool,
}

impl Default for AdminSection {
    fn default() -> Self {
        Self {
            username: "admin".to_string(),
            password: generate_initial_password(),
            session_ttl_hours: 168,
            allow_insecure_http: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamSection {
    pub app_id: u32,
    pub mode: SteamAuthMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub guard_data_path: PathBuf,
    pub request_concurrency: usize,
}

impl Default for SteamSection {
    fn default() -> Self {
        Self {
            app_id: 550,
            mode: SteamAuthMode::Anonymous,
            account_name: None,
            refresh_token: None,
            guard_data_path: PathBuf::from("./steam-guard.json"),
            request_concurrency: 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncSection {
    pub enable_read_through: bool,
    pub item_ttl_secs: u64,
    pub enable_incremental_sync: bool,
    pub incremental_interval_secs: u64,
    pub incremental_pages_per_tick: u32,
    pub enable_backfill: bool,
    pub backfill_interval_secs: u64,
    pub backfill_batch_size: u32,
}

impl Default for SyncSection {
    fn default() -> Self {
        Self {
            enable_read_through: true,
            item_ttl_secs: 900,
            enable_incremental_sync: true,
            incremental_interval_secs: 180,
            incremental_pages_per_tick: 2,
            enable_backfill: false,
            backfill_interval_secs: 15,
            backfill_batch_size: 50,
        }
    }
}

impl SyncSection {
    pub fn to_dto(&self) -> CatalogSyncConfigDto {
        CatalogSyncConfigDto {
            enable_read_through: self.enable_read_through,
            item_ttl_secs: self.item_ttl_secs,
            enable_incremental_sync: self.enable_incremental_sync,
            incremental_interval_secs: self.incremental_interval_secs,
            incremental_pages_per_tick: self.incremental_pages_per_tick,
            enable_backfill: self.enable_backfill,
            backfill_interval_secs: self.backfill_interval_secs,
            backfill_batch_size: self.backfill_batch_size,
        }
    }

    pub fn apply_dto(&mut self, dto: &CatalogSyncConfigDto) {
        self.enable_read_through = dto.enable_read_through;
        self.item_ttl_secs = dto.item_ttl_secs.max(30);
        self.enable_incremental_sync = dto.enable_incremental_sync;
        self.incremental_interval_secs = dto.incremental_interval_secs.max(30);
        self.incremental_pages_per_tick = dto.incremental_pages_per_tick.clamp(1, 20);
        self.enable_backfill = dto.enable_backfill;
        self.backfill_interval_secs = dto.backfill_interval_secs.max(2);
        self.backfill_batch_size = dto.backfill_batch_size.clamp(1, 100);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchSection {
    pub enable_hybrid_search: bool,
    pub default_language: String,
    pub steam_timeout_ms: u64,
    pub cache_ttl_secs: u64,
    pub cache_steam_results: bool,
}

impl Default for SearchSection {
    fn default() -> Self {
        Self {
            enable_hybrid_search: true,
            default_language: "english".to_string(),
            steam_timeout_ms: 4_500,
            cache_ttl_secs: 60,
            cache_steam_results: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterSection {
    pub enabled: bool,
    pub backend: CatalogFilterBackendKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deploy_dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub python_bin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_url: Option<String>,
    pub cache_ttl_secs: u64,
    pub threads: u32,
    pub enable_text: bool,
    pub enable_image: bool,
    pub max_images: u32,
    pub gallery_aggregation: FilterGalleryAggregation,
    pub gore_weight: f32,
    pub nsfw_weight: f32,
    pub temperature: f32,
    pub bias: f32,
}

impl Default for FilterSection {
    fn default() -> Self {
        let p = FilterModelParams::default();
        Self {
            enabled: false,
            backend: CatalogFilterBackendKind::ManagedPython,
            deploy_dir: None,
            python_bin: None,
            upstream_url: None,
            cache_ttl_secs: 3_600,
            threads: p.threads,
            enable_text: p.enable_text,
            enable_image: p.enable_image,
            max_images: p.max_images,
            gallery_aggregation: p.gallery_aggregation,
            gore_weight: p.gore_weight,
            nsfw_weight: p.nsfw_weight,
            temperature: p.temperature,
            bias: p.bias,
        }
    }
}

impl FilterSection {
    pub fn params(&self) -> FilterModelParams {
        FilterModelParams {
            threads: self.threads,
            enable_text: self.enable_text,
            enable_image: self.enable_image,
            max_images: self.max_images,
            gallery_aggregation: self.gallery_aggregation,
            gore_weight: self.gore_weight,
            nsfw_weight: self.nsfw_weight,
            temperature: self.temperature,
            bias: self.bias,
        }
        .sanitized()
    }

    pub fn normalize(&mut self) {
        self.deploy_dir = self
            .deploy_dir
            .as_ref()
            .map(|p| p.to_string_lossy().trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        self.python_bin = self
            .python_bin
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        self.upstream_url = self
            .upstream_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        self.cache_ttl_secs = self.cache_ttl_secs.min(86_400);

        let p = self.params();
        self.threads = p.threads;
        self.enable_text = p.enable_text;
        self.enable_image = p.enable_image;
        self.max_images = p.max_images;
        self.gallery_aggregation = p.gallery_aggregation;
        self.gore_weight = p.gore_weight;
        self.nsfw_weight = p.nsfw_weight;
        self.temperature = p.temperature;
        self.bias = p.bias;
    }

    pub fn to_dto(&self) -> CatalogFilterConfigDto {
        let mut normalized = self.clone();
        normalized.normalize();
        let params = normalized.params();
        CatalogFilterConfigDto {
            enabled: normalized.enabled,
            backend: normalized.backend,
            deploy_dir: normalized.deploy_dir.map(|p| p.display().to_string()),
            python_bin: normalized.python_bin,
            upstream_url: normalized.upstream_url,
            cache_ttl_secs: normalized.cache_ttl_secs,
            params,
        }
    }

    pub fn apply_dto(&mut self, dto: &CatalogFilterConfigDto) {
        self.enabled = dto.enabled;
        self.backend = dto.backend;
        self.deploy_dir = dto
            .deploy_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        self.python_bin = dto
            .python_bin
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        self.upstream_url = dto
            .upstream_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        self.cache_ttl_secs = dto.cache_ttl_secs.min(86_400);

        let p = dto.params.sanitized();
        self.threads = p.threads;
        self.enable_text = p.enable_text;
        self.enable_image = p.enable_image;
        self.max_images = p.max_images;
        self.gallery_aggregation = p.gallery_aggregation;
        self.gore_weight = p.gore_weight;
        self.nsfw_weight = p.nsfw_weight;
        self.temperature = p.temperature;
        self.bias = p.bias;
    }
}

pub fn generate_initial_password() -> String {
    format!("cat_{}", uuid::Uuid::new_v4().simple())
}

impl CatalogConfig {
    /// 读取或初始化配置。返回值中的布尔值表示是否生成并保存了初始管理员密码。
    pub fn load_or_init(path: &Path) -> anyhow::Result<(Self, bool)> {
        if path.exists() {
            let raw = fs::read_to_string(path)?;
            let mut document: toml::Table = toml::from_str(&raw)?;
            let mut parsed: Self = toml::Value::Table(document.clone()).try_into()?;
            parsed.normalize();
            let admin = document
                .entry("admin")
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            // Deserializing the config above already rejects a non-table [admin].
            let admin = admin
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("admin 必须为 TOML 表"))?;
            let generated_password = !admin.contains_key("password");
            if generated_password {
                admin.insert("password".into(), parsed.admin.password.clone().into());
                write_private_file(path, toml::to_string_pretty(&document)?.as_bytes())?;
            }
            Ok((parsed, generated_password))
        } else {
            let mut default_cfg = Self::default();
            default_cfg.normalize();
            default_cfg.save_to(path)?;
            Ok((default_cfg, true))
        }
    }

    pub fn normalize(&mut self) {
        if self.admin.username.trim().is_empty() {
            self.admin.username = "admin".to_string();
        }
        self.admin.session_ttl_hours = self.admin.session_ttl_hours.clamp(1, 24 * 90);
        if self.steam.app_id == 0 {
            self.steam.app_id = 550;
        }
        self.steam.request_concurrency = self.steam.request_concurrency.clamp(1, 16);
        self.sync.item_ttl_secs = self.sync.item_ttl_secs.max(30);
        self.sync.incremental_interval_secs = self.sync.incremental_interval_secs.max(30);
        self.sync.incremental_pages_per_tick = self.sync.incremental_pages_per_tick.clamp(1, 20);
        self.sync.backfill_interval_secs = self.sync.backfill_interval_secs.max(2);
        self.sync.backfill_batch_size = self.sync.backfill_batch_size.clamp(1, 100);
        self.search.default_language = self.search.default_language.trim().to_string();
        if self.search.default_language.is_empty() {
            self.search.default_language = "english".to_string();
        }
        self.search.steam_timeout_ms = self.search.steam_timeout_ms.clamp(500, 15_000);
        self.search.cache_ttl_secs = self.search.cache_ttl_secs.min(3_600);
        self.filter.normalize();
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        let body = format!(
            "# Left4Addons Catalog Server 配置文件\n# 修改将由控制台自动持久化，也可手动编辑后重启服务。\n\n{}",
            toml::to_string_pretty(self)?
        );
        write_private_file(path, body.as_bytes())?;
        Ok(())
    }
}

pub struct CatalogConfigStore {
    path: Option<PathBuf>,
    inner: RwLock<CatalogConfig>,
}

impl CatalogConfigStore {
    pub fn new(path: Option<PathBuf>, mut config: CatalogConfig) -> Self {
        config.normalize();
        Self {
            path,
            inner: RwLock::new(config),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn get(&self) -> CatalogConfig {
        self.inner.read().unwrap().clone()
    }

    pub fn update(&self, f: impl FnOnce(&mut CatalogConfig)) -> anyhow::Result<CatalogConfig> {
        let mut guard = self.inner.write().unwrap();
        let mut next = guard.clone();
        f(&mut next);
        next.normalize();
        if let Some(ref path) = self.path {
            next.save_to(path)?;
        }
        *guard = next.clone();
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_and_persists_missing_admin_password_in_existing_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.toml");
        fs::write(&path, "[server]\nbind = '127.0.0.1:9555'\n[admin]\nusername = 'operator'\n[custom]\nkeep = 'yes'\n").unwrap();
        let (first, generated) = CatalogConfig::load_or_init(&path).unwrap();
        assert!(generated);
        let (second, generated) = CatalogConfig::load_or_init(&path).unwrap();
        assert!(!generated);
        assert_eq!(first.admin.password, second.admin.password);
        assert!(!second.admin.password.is_empty());
        assert_eq!(second.admin.username, "operator");
        assert_eq!(second.server.bind, "127.0.0.1:9555");
        let document: toml::Table = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(document["custom"]["keep"].as_str(), Some("yes"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
