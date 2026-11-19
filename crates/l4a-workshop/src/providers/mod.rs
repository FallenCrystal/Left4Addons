pub mod catalog_backend;
pub mod steam_bridge;
pub mod steam_web_api;
pub mod web_scraper_placeholder;

pub use catalog_backend::CatalogBackendProvider;
pub use steam_bridge::SteamBridgeProvider;
pub use steam_web_api::SteamWebApiProvider;
pub use web_scraper_placeholder::{
    PlaceholderWebScraper, WebScraperPlaceholderProvider, WorkshopWebScraper,
};
