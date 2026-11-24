use crate::credentials::write_private_file;
use l4a_protocol::{CatalogSyncConfigDto, SteamAuthMode};
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
        if self.search.default_language.trim().is_empty() {
            self.search.default_language = "english".to_string();
        }
        self.search.steam_timeout_ms = self.search.steam_timeout_ms.clamp(500, 15_000);
        self.search.cache_ttl_secs = self.search.cache_ttl_secs.min(3_600);
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
    pub fn new(path: Option<PathBuf>, config: CatalogConfig) -> Self {
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
