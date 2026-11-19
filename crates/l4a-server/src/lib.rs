pub mod admin_ui;
pub mod auth;
pub mod db;
pub mod ingest;
pub mod routes;
pub mod scraper;

use db::ServerRepo;
use scraper::{CatalogScraper, PlaceholderCatalogScraper};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind_addr: String,
    pub db_path: PathBuf,
    pub admin_token: String,
}

#[derive(Clone)]
pub struct ServerState {
    pub repo: Arc<ServerRepo>,
    pub admin_token: String,
    pub scraper: Arc<dyn CatalogScraper>,
}

impl ServerState {
    pub fn open(config: &ServerConfig) -> anyhow::Result<Self> {
        let repo = ServerRepo::open(&config.db_path)?;
        Ok(Self {
            repo: Arc::new(repo),
            admin_token: config.admin_token.clone(),
            scraper: Arc::new(PlaceholderCatalogScraper),
        })
    }

    pub fn in_memory(admin_token: impl Into<String>) -> anyhow::Result<Self> {
        let repo = ServerRepo::open_in_memory()?;
        Ok(Self {
            repo: Arc::new(repo),
            admin_token: admin_token.into(),
            scraper: Arc::new(PlaceholderCatalogScraper),
        })
    }
}

pub fn build_router(state: ServerState) -> axum::Router {
    routes::router(state)
}
