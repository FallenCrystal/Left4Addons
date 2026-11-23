mod addons;
mod grouping;
mod metadata;
mod reconcile_tasks;
mod workshop;

pub use metadata::{
    clean_addon_title, clean_author_name, clean_workshop_title, collect_installed_addon_maps,
    derive_categories_from_info, is_remote_image_url, is_valid_local_cover_key, merge_categories,
    normalize_valid_remote_cover_url, parse_enum, serialize_enum,
};

use crate::db::migrations::run_migrations;
use crate::error::CoreError;
use l4a_protocol::{AddonId, PhysicalFileRecord};
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

#[derive(Debug, Clone)]
pub struct PhysicalFileRow {
    pub record: PhysicalFileRecord,
    pub addon_id: Option<AddonId>,
}

pub struct LocalDb {
    conn: Mutex<Connection>,
    // Serializes workspace operations with lifecycle writes to the dependency graph.
    workspace: Mutex<()>,
}

impl LocalDb {
    pub fn open(path: &Path) -> Result<Self, CoreError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        run_migrations(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            workspace: Mutex::new(()),
        })
    }

    pub fn open_in_memory() -> Result<Self, CoreError> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        run_migrations(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            workspace: Mutex::new(()),
        })
    }

    pub(super) fn lock_conn(&self) -> Result<MutexGuard<'_, Connection>, CoreError> {
        self.conn
            .lock()
            .map_err(|_| CoreError::InvalidInput("本地数据库互斥锁异常".into()))
    }

    pub(crate) fn lock_workspace(&self) -> Result<MutexGuard<'_, ()>, CoreError> {
        self.workspace
            .lock()
            .map_err(|_| CoreError::Conflict("工作区锁状态异常".into()))
    }

    pub(crate) fn write<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        let result = operation(&tx)?;
        tx.commit()?;
        Ok(result)
    }
}
