pub mod auth;
pub mod config;
mod credentials;
pub mod db;
pub mod filter;
pub mod hybrid_search;
pub mod routes;
pub mod search;
pub mod sync;
pub mod upstream;
pub mod web_assets;

use auth::AdminAuthManager;
use config::{CatalogConfig, CatalogConfigStore};
use db::ServerRepo;
use filter::CatalogFilterService;
use hybrid_search::HybridSearchService;
use std::path::Path;
use std::sync::Arc;
use sync::CatalogSyncCoordinator;
use upstream::{MockWorkshopUpstream, SteamCmClient, WorkshopUpstream};

#[derive(Clone)]
pub struct ServerState {
    pub config_store: Arc<CatalogConfigStore>,
    pub repo: Arc<ServerRepo>,
    pub auth: Arc<AdminAuthManager>,
    pub upstream: Arc<dyn WorkshopUpstream>,
    pub sync: Arc<CatalogSyncCoordinator>,
    pub search: Arc<HybridSearchService>,
    pub filter: Arc<CatalogFilterService>,
}

impl ServerState {
    pub fn from_config_store(config_store: Arc<CatalogConfigStore>) -> anyhow::Result<Self> {
        let db_path = config_store.get().server.db_path;
        Self::from_config_store_with_db_path(config_store, &db_path)
    }

    pub fn from_config_store_with_db_path(
        config_store: Arc<CatalogConfigStore>,
        db_path: &Path,
    ) -> anyhow::Result<Self> {
        let repo = Arc::new(ServerRepo::open(db_path)?);
        let upstream: Arc<dyn WorkshopUpstream> =
            Arc::new(SteamCmClient::new(config_store.clone()));
        Ok(Self::with_components(config_store, repo, upstream))
    }

    pub fn open_config_file(config_path: &Path) -> anyhow::Result<(Self, bool)> {
        let (cfg, created) = CatalogConfig::load_or_init(config_path)?;
        let store = Arc::new(CatalogConfigStore::new(
            Some(config_path.to_path_buf()),
            cfg,
        ));
        let state = Self::from_config_store(store)?;
        Ok((state, created))
    }

    pub fn with_components(
        config_store: Arc<CatalogConfigStore>,
        repo: Arc<ServerRepo>,
        upstream: Arc<dyn WorkshopUpstream>,
    ) -> Self {
        let cfg = config_store.get();
        let auth = Arc::new(AdminAuthManager::new(
            cfg.admin.username,
            cfg.admin.password,
            cfg.admin.session_ttl_hours,
        ));
        let sync =
            CatalogSyncCoordinator::new(config_store.clone(), repo.clone(), upstream.clone());
        let search = Arc::new(HybridSearchService::new(
            config_store.clone(),
            repo.clone(),
            upstream.clone(),
        ));
        let filter = CatalogFilterService::new(config_store.clone(), repo.clone());
        Self {
            config_store,
            repo,
            auth,
            upstream,
            sync,
            search,
            filter,
        }
    }

    pub fn in_memory(admin_password: impl Into<String>) -> anyhow::Result<Self> {
        Self::in_memory_with_upstream(admin_password, Arc::new(MockWorkshopUpstream::new()))
    }

    pub fn in_memory_with_upstream(
        admin_password: impl Into<String>,
        upstream: Arc<dyn WorkshopUpstream>,
    ) -> anyhow::Result<Self> {
        let mut cfg = CatalogConfig::default();
        cfg.admin.password = admin_password.into();
        let store = Arc::new(CatalogConfigStore::new(None, cfg));
        let repo = Arc::new(ServerRepo::open_in_memory()?);
        Ok(Self::with_components(store, repo, upstream))
    }
}

pub fn build_router(state: ServerState) -> axum::Router {
    routes::router(state)
}
