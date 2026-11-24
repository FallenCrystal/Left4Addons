mod admin_sync;
mod items;
mod schema;
mod search_query;
mod upstream_apply;

pub(crate) use search_query::{RankedLocalItem, RankedLocalSearch};

use chrono::{DateTime, Utc};
use l4a_protocol::{WorkshopId, WorkshopItemRecord, WorkshopItemStatus};
use rusqlite::{params, Connection, Transaction, TransactionBehavior};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, RwLock};

const READER_POOL_SIZE: usize = 4;
const MAX_NEGATIVE_CACHE_ENTRIES: usize = 4096;
pub(crate) const STATE_INCREMENTAL_SCAN: &str = "incremental_scan";
pub(crate) const STATE_BACKFILL_CURSOR: &str = "backfill_cursor";
pub(crate) const STATE_BACKFILL_SYNCED_TOTAL: &str = "backfill_synced_total";
pub(crate) const STATE_REMOTE_WORKSHOP_TOTAL: &str = "remote_workshop_total";
const STATE_SEARCH_INDEX_VERSION: &str = "search_index_version";
const INSERT_SEARCH_DOCUMENT: &str = "
    INSERT INTO workshop_items_fts (
        rowid, title_tokens, title_compact, title_pinyin, creator_tokens, desc_tokens, desc_aliases
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

pub struct ServerRepo {
    writer: Mutex<Connection>,
    readers: Vec<Mutex<Connection>>,
    reader_idx: AtomicUsize,
    negative_cache: RwLock<HashMap<WorkshopId, NegativeObservation>>,
}

#[derive(Clone, Copy)]
struct NegativeObservation {
    status: WorkshopItemStatus,
    observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct StoredItemMeta {
    pub record: WorkshopItemRecord,
    pub is_admin_override: bool,
    pub last_verified_at: DateTime<Utc>,
    pub removed_at: Option<DateTime<Utc>>,
    steam_version: Option<i64>,
    catalog_indexed: bool,
    pub is_listed: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogIndexPolicy {
    /// 仅写入或刷新按 ID 穿透缓存，不将未收录条目暴露到公开工坊首页与搜索。
    CacheOnly,
    /// 仅在本地尚无任何记录时以 `CacheOnly` 写入，避免非英文搜索预热覆盖英文主记录。
    InsertOnlyCache,
    /// 将公开条目纳入目录索引，供公开工坊首页与检索展示。
    IndexInCatalog,
}

#[derive(Debug, Default, Clone)]
pub struct UpstreamApplySummary {
    pub active_upserted: u64,
    /// Catalog promotions that preserved a newer cached snapshot.
    pub catalog_promoted: u64,
    pub tombstoned: u64,
    pub skipped_overrides: u64,
    pub skipped_stale: u64,
}

impl UpstreamApplySummary {
    pub(crate) fn active_written(&self) -> u64 {
        self.active_upserted + self.catalog_promoted
    }
}

impl ServerRepo {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if path == Path::new(":memory:") {
            return Self::open_in_memory();
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let writer = open_configured_conn(path)?;
        let mut repo = Self {
            writer: Mutex::new(writer),
            readers: Vec::with_capacity(READER_POOL_SIZE),
            reader_idx: AtomicUsize::new(0),
            negative_cache: RwLock::new(HashMap::new()),
        };
        repo.migrate()?;

        for _ in 0..READER_POOL_SIZE {
            let reader = open_configured_conn(path)?;
            reader.pragma_update(None, "query_only", "ON")?;
            repo.readers.push(Mutex::new(reader));
        }

        Ok(repo)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        let repo = Self {
            writer: Mutex::new(conn),
            readers: Vec::new(),
            reader_idx: AtomicUsize::new(0),
            negative_cache: RwLock::new(HashMap::new()),
        };
        repo.migrate()?;
        Ok(repo)
    }

    fn lock_writer(&self) -> anyhow::Result<MutexGuard<'_, Connection>> {
        self.writer
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库写入锁状态异常"))
    }

    fn lock_reader(&self) -> anyhow::Result<MutexGuard<'_, Connection>> {
        if self.readers.is_empty() {
            return self.lock_writer();
        }
        let idx = self.reader_idx.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        self.readers[idx]
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库读取锁状态异常"))
    }

    fn with_read_snapshot<T>(
        &self,
        read: impl FnOnce(&Transaction<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut conn = self.lock_reader()?;
        let snapshot = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let result = read(&snapshot)?;
        snapshot.commit()?;
        Ok(result)
    }

    pub fn with_transaction<T>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut conn = self.lock_writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }
}

fn open_configured_conn(path: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    Ok(conn)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> anyhow::Result<bool> {
    let mut stmt = conn.prepare("SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2")?;
    Ok(stmt.exists(params![table, column])?)
}

fn has_table(conn: &Connection, table: &str) -> anyhow::Result<bool> {
    let mut stmt =
        conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?;
    Ok(stmt.exists(params![table])?)
}

fn serialize_json_enum<T: serde::Serialize>(val: &T) -> String {
    serde_json::to_string(val)
        .unwrap_or_default()
        .trim_matches('"')
        .to_string()
}

fn parse_json_enum<T: serde::de::DeserializeOwned>(s: &str, fallback: T) -> T {
    serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap_or(fallback)
}

fn parse_rfc3339_or_now(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn parse_workshop_id_col(col: usize, raw: i64) -> rusqlite::Result<WorkshopId> {
    u64::try_from(raw)
        .ok()
        .and_then(|id| WorkshopId::new(id).ok())
        .ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                col,
                rusqlite::types::Type::Integer,
                format!("invalid workshop_id in SQLite row: {raw}").into(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_and_dependency_reads_share_a_wal_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let repo = ServerRepo::open(&dir.path().join("catalog.db")).unwrap();
        repo.with_transaction(|tx| {
            tx.execute_batch(
                "INSERT INTO workshop_items
                    (workshop_id, file_type, status, title, file_url, source, fetched_at, last_verified_at)
                 VALUES (1, 'item', 'active', 'Map', 'old-link', 'steam_kit', datetime('now'), datetime('now'));
                 INSERT INTO workshop_item_dependencies
                    (parent_workshop_id, dep_workshop_id, updated_at) VALUES (1, 10, datetime('now'));",
            )?;
            Ok(())
        }).unwrap();

        repo.with_read_snapshot(|snapshot| {
            let url: String = snapshot.query_row("SELECT file_url FROM workshop_items WHERE workshop_id = 1", [], |r| r.get(0))?;
            assert_eq!(url, "old-link");
            repo.with_transaction(|tx| {
                tx.execute_batch(
                    "UPDATE workshop_items SET file_url = 'new-link' WHERE workshop_id = 1;
                     UPDATE workshop_item_dependencies SET dep_workshop_id = 20 WHERE parent_workshop_id = 1;",
                )?;
                Ok(())
            })?;
            let item = ServerRepo::get_item_with_conn(snapshot, WorkshopId(1))?.unwrap().record;
            assert_eq!(item.file_url.as_deref(), Some("old-link"));
            assert_eq!(item.dependencies[0].workshop_id, WorkshopId(10));
            Ok(())
        }).unwrap();
        let item = repo.get_item(WorkshopId(1)).unwrap().unwrap().record;
        assert_eq!(item.file_url.as_deref(), Some("new-link"));
        assert_eq!(item.dependencies[0].workshop_id, WorkshopId(20));
    }
}
