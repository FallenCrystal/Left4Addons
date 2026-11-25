mod admin_sync;
mod items;
mod schema;
mod search_query;
mod upstream_apply;

pub(crate) use search_query::RankedCatalogMatch;

use chrono::{DateTime, Utc};
use l4a_protocol::{WorkshopId, WorkshopItemRecord, WorkshopItemStatus};
use rusqlite::{params, Connection, InterruptHandle, Transaction, TransactionBehavior};
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock};

const READER_POOL_SIZE: usize = 8;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QueryAbortedError {
    #[error("查询已取消")]
    Cancelled,
    #[error("查询执行超时")]
    Timeout,
    #[error("搜索服务繁忙，请稍后重试")]
    Busy,
}

#[derive(Default)]
struct CancelState {
    cancelled: bool,
    interrupt: Option<InterruptHandle>,
}

/// Thread-safe cooperative and SQLite-interrupt cancellation token for long-running read queries.
#[derive(Clone, Default)]
pub struct QueryCancelToken {
    flag: Arc<AtomicBool>,
    state: Arc<Mutex<CancelState>>,
}

impl QueryCancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
        let mut state = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.cancelled = true;
        // Invoke `sqlite3_interrupt` while still holding `state` so `InterruptBinding::drop`
        // cannot return the underlying connection to the pool before the interrupt finishes.
        if let Some(handle) = state.interrupt.take() {
            handle.interrupt();
        }
    }

    fn bind(&self, conn: &Connection) -> anyhow::Result<InterruptBinding> {
        let mut state = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.cancelled || self.flag.load(Ordering::Acquire) {
            return Err(QueryAbortedError::Cancelled.into());
        }
        state.interrupt = Some(conn.get_interrupt_handle());
        Ok(InterruptBinding {
            state: Arc::clone(&self.state),
        })
    }
}

struct InterruptBinding {
    state: Arc<Mutex<CancelState>>,
}

impl Drop for InterruptBinding {
    fn drop(&mut self) {
        let mut state = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.interrupt = None;
    }
}

/// Registers `sqlite3_progress_handler` so a cancel signal that fires between
/// statements (when SQLite clears `sqlite3_interrupt`) still aborts the next VDBE run.
struct ProgressHandlerGuard<'a> {
    conn: &'a Connection,
}

impl<'a> ProgressHandlerGuard<'a> {
    fn install(conn: &'a Connection, cancel: &QueryCancelToken) -> Self {
        const PROGRESS_HANDLER_VM_STEPS: std::ffi::c_int = 1_000;
        let flag = Arc::clone(&cancel.flag);
        conn.progress_handler(
            PROGRESS_HANDLER_VM_STEPS,
            Some(move || flag.load(Ordering::Acquire)),
        );
        Self { conn }
    }
}

impl Drop for ProgressHandlerGuard<'_> {
    fn drop(&mut self) {
        self.conn.progress_handler(0, None::<fn() -> bool>);
    }
}

struct ReaderPool {
    idle: Mutex<Vec<Connection>>,
    available: Condvar,
}

impl ReaderPool {
    fn new(conns: Vec<Connection>) -> Self {
        Self {
            idle: Mutex::new(conns),
            available: Condvar::new(),
        }
    }

    fn checkout(&self) -> anyhow::Result<ReaderGuard<'_>> {
        let mut idle = self
            .idle
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库读取池锁状态异常"))?;
        loop {
            if let Some(conn) = idle.pop() {
                return Ok(ReaderGuard::Pooled {
                    pool: self,
                    conn: Some(conn),
                });
            }
            idle = self
                .available
                .wait(idle)
                .map_err(|_| anyhow::anyhow!("数据库读取池条件变量异常"))?;
        }
    }
}

enum ReaderGuard<'a> {
    Pooled {
        pool: &'a ReaderPool,
        conn: Option<Connection>,
    },
    Writer(MutexGuard<'a, Connection>),
}

impl Deref for ReaderGuard<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Pooled { conn, .. } => conn.as_ref().expect("pooled reader missing"),
            Self::Writer(guard) => guard,
        }
    }
}

impl DerefMut for ReaderGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Pooled { conn, .. } => conn.as_mut().expect("pooled reader missing"),
            Self::Writer(guard) => guard,
        }
    }
}

impl Drop for ReaderGuard<'_> {
    fn drop(&mut self) {
        if let Self::Pooled { pool, conn } = self {
            if let Some(conn) = conn.take() {
                let mut idle = match pool.idle.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                idle.push(conn);
                pool.available.notify_one();
            }
        }
    }
}

pub struct ServerRepo {
    writer: Mutex<Connection>,
    readers: Option<ReaderPool>,
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

impl StoredItemMeta {
    pub(crate) fn is_in_public_catalog(&self) -> bool {
        self.catalog_indexed && self.is_listed == Some(true)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OverlayItemMeta {
    pub status: WorkshopItemStatus,
    pub is_admin_override: bool,
    pub last_verified_at: DateTime<Utc>,
    pub catalog_indexed: bool,
    pub is_listed: Option<bool>,
    pub creator_name: Option<String>,
    pub override_record: Option<WorkshopItemRecord>,
}

impl OverlayItemMeta {
    pub(crate) fn is_in_public_catalog(&self) -> bool {
        self.catalog_indexed && self.is_listed == Some(true)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogIndexPolicy {
    /// 仅写入或刷新按 ID 穿透缓存，不将未收录条目暴露到公开工坊首页与搜索。
    CacheOnly,
    /// Search can warm the by-ID cache while indexed metadata remains owned by catalog sync.
    RefreshUnindexedCache,
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
            readers: None,
            negative_cache: RwLock::new(HashMap::new()),
        };
        repo.migrate()?;

        let mut reader_conns = Vec::with_capacity(READER_POOL_SIZE);
        for _ in 0..READER_POOL_SIZE {
            let reader = open_configured_conn(path)?;
            reader.pragma_update(None, "query_only", "ON")?;
            reader_conns.push(reader);
        }
        repo.readers = Some(ReaderPool::new(reader_conns));

        Ok(repo)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        let repo = Self {
            writer: Mutex::new(conn),
            readers: None,
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

    fn lock_reader(&self) -> anyhow::Result<ReaderGuard<'_>> {
        match &self.readers {
            Some(pool) => pool.checkout(),
            None => self.lock_writer().map(ReaderGuard::Writer),
        }
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

    pub(crate) fn with_cancellable_read_snapshot<T>(
        &self,
        cancel: &QueryCancelToken,
        read: impl FnOnce(&Transaction<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        if cancel.is_cancelled() {
            return Err(QueryAbortedError::Cancelled.into());
        }
        let mut conn = self.lock_reader()?;
        if cancel.is_cancelled() {
            return Err(QueryAbortedError::Cancelled.into());
        }
        let snapshot = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
        // Drop order is binding -> progress_guard -> snapshot -> conn, ensuring both
        // `sqlite3_interrupt` and `sqlite3_progress_handler` are detached before
        // rollback/commit and before `conn` returns to `ReaderPool`.
        let progress_guard = ProgressHandlerGuard::install(&snapshot, cancel);
        let binding = cancel.bind(&snapshot)?;
        let result = read(&snapshot);
        drop(binding);
        drop(progress_guard);
        match result {
            Ok(value) => {
                if cancel.is_cancelled() {
                    drop(snapshot);
                    return Err(QueryAbortedError::Cancelled.into());
                }
                snapshot.commit()?;
                Ok(value)
            }
            Err(err) => {
                drop(snapshot);
                if cancel.is_cancelled() || is_sqlite_interrupted(&err) {
                    return Err(QueryAbortedError::Cancelled.into());
                }
                Err(err)
            }
        }
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

fn is_sqlite_interrupted(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(sqlite_err, _))
                if sqlite_err.code == rusqlite::ErrorCode::OperationInterrupted
        )
    })
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

pub(super) fn workshop_id_to_i64(wid: WorkshopId) -> anyhow::Result<i64> {
    i64::try_from(wid.as_u64())
        .map_err(|_| anyhow::anyhow!("WorkshopId 超出 SQLite 整数范围: {wid}"))
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

    #[test]
    fn reader_pool_isolates_slow_query_and_interrupt_recovers_connection() {
        use std::sync::atomic::AtomicUsize;
        use std::time::{Duration, Instant};

        let dir = tempfile::tempdir().unwrap();
        let repo = Arc::new(ServerRepo::open(&dir.path().join("catalog.db")).unwrap());
        repo.with_transaction(|tx| {
            tx.execute(
                "INSERT INTO workshop_items
                    (workshop_id, file_type, status, title, source, fetched_at, last_verified_at, catalog_indexed, is_listed)
                 VALUES (42, 'item', 'active', 'Fast Item', 'steam_kit', datetime('now'), datetime('now'), 1, 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        let started = Arc::new(AtomicUsize::new(0));
        let mut tokens = Vec::new();
        let mut handles = Vec::new();

        for _ in 0..4 {
            let repo = Arc::clone(&repo);
            let started = Arc::clone(&started);
            let token = QueryCancelToken::new();
            tokens.push(token.clone());
            handles.push(std::thread::spawn(move || {
                repo.with_cancellable_read_snapshot(&token, |snapshot| {
                    started.fetch_add(1, Ordering::Release);
                    let count: i64 = snapshot.query_row(
                        "WITH RECURSIVE cnt(x) AS (
                             SELECT 1
                             UNION ALL
                             SELECT x + 1 FROM cnt WHERE x < 100000000
                         )
                         SELECT COUNT(*) FROM cnt",
                        [],
                        |r| r.get(0),
                    )?;
                    Ok(count)
                })
            }));
        }

        let wait_start = Instant::now();
        while started.load(Ordering::Acquire) < 4 {
            assert!(
                wait_start.elapsed() < Duration::from_secs(5),
                "timed out waiting for slow reader threads to start"
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        // Fast reads must run immediately on the remaining idle readers in the pool.
        let probe_start = Instant::now();
        assert_eq!(repo.total_items_count().unwrap(), 1);
        let item = repo.get_item(WorkshopId(42)).unwrap().unwrap().record;
        assert_eq!(item.title, "Fast Item");
        let stats = repo.stats().unwrap();
        assert_eq!(stats.total_items, 1);
        assert_eq!(stats.active_items, 1);
        assert!(
            probe_start.elapsed() < Duration::from_millis(500),
            "fast reads stalled behind slow readers: {:?}",
            probe_start.elapsed()
        );

        // Cancel all 4 long-running queries and confirm immediate sqlite3_interrupt unwinding.
        let cancel_start = Instant::now();
        for token in &tokens {
            token.cancel();
        }
        for handle in handles {
            let err = handle
                .join()
                .expect("slow reader thread panicked")
                .expect_err("expected slow query to be interrupted");
            assert_eq!(
                err.downcast_ref::<QueryAbortedError>(),
                Some(&QueryAbortedError::Cancelled)
            );
        }
        assert!(
            cancel_start.elapsed() < Duration::from_secs(2),
            "interrupted queries took too long to exit: {:?}",
            cancel_start.elapsed()
        );

        // Reuse every reader in the pool to confirm no lingering interrupt or transaction state.
        for _ in 0..(READER_POOL_SIZE * 2) {
            assert_eq!(repo.total_items_count().unwrap(), 1);
            assert_eq!(
                repo.get_item(WorkshopId(42)).unwrap().unwrap().record.title,
                "Fast Item"
            );
        }
    }

    #[test]
    fn cancellable_search_and_unbounded_sort_match_bounded_results() {
        let repo = ServerRepo::open_in_memory().unwrap();
        let now = Utc::now();
        for i in 1..=20u64 {
            let record = WorkshopItemRecord {
                workshop_id: WorkshopId(1000 + i),
                file_type: l4a_protocol::WorkshopFileType::Item,
                status: WorkshopItemStatus::Active,
                title: format!("BroadKeyword Map {i}"),
                description: None,
                creator_steam_id: None,
                creator_name: None,
                preview_url: None,
                previews: Vec::new(),
                file_url: None,
                file_size: None,
                time_created: Some(1_700_000_000 + i as i64),
                time_updated: Some(1_700_000_100 + ((21 - i) as i64)),
                tags: Vec::new(),
                dependencies: Vec::new(),
                collection_children: Vec::new(),
                subscriptions: Some(i * 10),
                favorites: None,
                source: l4a_protocol::MetadataSource::SteamKit,
                fetched_at: now,
            };
            repo.upsert_item(&record, false, true).unwrap();
        }

        for sort_name in ["subscriptions", "updated", "newest", "relevance", "title"] {
            let query = l4a_protocol::SearchQuery {
                query: Some("BroadKeyword".to_string()),
                tag: None,
                status: None,
                sort: Some(sort_name.to_string()),
                page: 1,
                page_size: 20,
                language: None,
                search_target: None,
                file_type: None,
            };

            let cancelled = QueryCancelToken::new();
            cancelled.cancel();
            let err = repo
                .search_cancellable(&query, true, &cancelled)
                .expect_err("pre-cancelled search must abort");
            assert_eq!(
                err.downcast_ref::<QueryAbortedError>(),
                Some(&QueryAbortedError::Cancelled)
            );

            let bounded = repo.search(&query, true).unwrap();
            let unbounded = repo
                .catalog_search_matches_cancellable(&query, &QueryCancelToken::new())
                .unwrap();
            let bounded_ids: Vec<_> = bounded.items.iter().map(|it| it.workshop_id).collect();
            let unbounded_ids: Vec<_> = unbounded.iter().map(|m| m.id).collect();
            assert_eq!(bounded_ids.len(), 20, "sort={sort_name}");
            assert_eq!(bounded_ids, unbounded_ids, "sort={sort_name}");

            let mut page2_query = query.clone();
            page2_query.page = 2;
            page2_query.page_size = 5;
            let page2 = repo.search(&page2_query, true).unwrap();
            let page2_ids: Vec<_> = page2.items.iter().map(|it| it.workshop_id).collect();
            assert_eq!(page2.total, 20, "sort={sort_name}");
            assert_eq!(page2_ids, unbounded_ids[5..10], "sort={sort_name} page 2");
        }
    }

    #[test]
    fn progress_handler_aborts_statement_started_after_cancel_signal() {
        use std::time::{Duration, Instant};

        let repo = ServerRepo::open_in_memory().unwrap();
        let token = QueryCancelToken::new();
        let started = Instant::now();
        let err = repo
            .with_cancellable_read_snapshot(&token, |snapshot| {
                // Cancel AFTER snapshot binding and BEFORE starting the SQLite statement:
                // SQLite clears `sqlite3_interrupt` when a new statement starts, so this
                // directly verifies `sqlite3_progress_handler` catches the cancellation.
                token.cancel();
                let count: i64 = snapshot.query_row(
                    "WITH RECURSIVE cnt(x) AS (
                         SELECT 1
                         UNION ALL
                         SELECT x + 1 FROM cnt WHERE x < 100000000
                     )
                     SELECT COUNT(*) FROM cnt",
                    [],
                    |r| r.get(0),
                )?;
                Ok(count)
            })
            .expect_err("progress handler must abort statement started after cancel");

        assert_eq!(
            err.downcast_ref::<QueryAbortedError>(),
            Some(&QueryAbortedError::Cancelled)
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "progress handler took too long to abort: {:?}",
            started.elapsed()
        );
        // Verify the progress handler was uninstalled and subsequent reads succeed cleanly.
        assert_eq!(repo.total_items_count().unwrap(), 0);
    }
}
