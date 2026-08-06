mod bridge;
pub mod discovery;
mod service;
mod types;

pub use discovery::detect_l4d2_addons_path;
pub use service::{fetch_collection_children_web, fetch_steam_details_web, WorkshopService};
pub use types::*;
