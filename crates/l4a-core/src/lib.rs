pub mod config;
pub mod db;
pub mod engine;
pub mod error;
mod file_changes;
pub mod grouping;
pub mod lifecycle;
pub mod ops;
pub mod reconcile;
pub mod scanner;
pub mod tasks;
pub mod watcher;

pub use config::{load_config, save_config, AppPaths};
pub use engine::{EventSink, L4aEngine, NullEventSink};
pub use error::CoreError;
