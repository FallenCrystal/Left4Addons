pub mod downloader;
pub mod error;
pub mod provider;
pub mod providers;
pub mod resolver;

pub use downloader::{DownloadProgress, DownloadRequest, DownloadResult, ProgressSink, ResilientDownloader};
pub use error::WorkshopError;
pub use provider::{ProviderCapabilities, WorkshopProvider};
pub use providers::{
    CatalogBackendProvider, PlaceholderWebScraper, SteamBridgeProvider, SteamWebApiProvider,
    WebScraperPlaceholderProvider, WorkshopWebScraper,
};
pub use resolver::{SourceRoutingPolicy, WorkshopResolver};
