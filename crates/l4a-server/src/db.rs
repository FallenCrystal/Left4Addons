use crate::search::{self, MatchRank, ParsedQuery, SEARCH_INDEX_VERSION};
use crate::upstream::WorkshopObservation;
use chrono::{DateTime, Utc};
use l4a_protocol::{
    AdminItemDetailResponse, AdminItemOverrideRequest, AuditLogEntry, CatalogStats,
    ItemRevisionEntry, MetadataSource, SearchQuery, SearchResponse, WorkshopCollectionRecord,
    WorkshopDependency, WorkshopFetchOutcome, WorkshopFileType, WorkshopId, WorkshopItemRecord,
    WorkshopItemStatus, WorkshopPreview,
};
use rusqlite::{
    params, Connection, OptionalExtension, Row, Rows, Transaction, TransactionBehavior,
};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
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

    fn migrate(&self) -> anyhow::Result<()> {
        self.with_transaction(Self::migrate_tx)
    }

    fn migrate_tx(conn: &Transaction<'_>) -> anyhow::Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS workshop_items (
                workshop_id INTEGER PRIMARY KEY,
                file_type TEXT NOT NULL,
                status TEXT NOT NULL,
                title TEXT NOT NULL,
                description TEXT,
                creator_steam_id TEXT,
                creator_name TEXT,
                preview_url TEXT,
                previews_json TEXT NOT NULL DEFAULT '[]',
                file_url TEXT,
                file_size INTEGER,
                time_created INTEGER,
                time_updated INTEGER,
                tags_json TEXT NOT NULL DEFAULT '[]',
                collection_children_json TEXT NOT NULL DEFAULT '[]',
                subscriptions INTEGER,
                favorites INTEGER,
                is_admin_override INTEGER NOT NULL DEFAULT 0,
                source TEXT NOT NULL,
                fetched_at TEXT NOT NULL,
                removed_at TEXT,
                last_verified_at TEXT NOT NULL,
                is_listed INTEGER,
                steam_version INTEGER,
                catalog_indexed INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_workshop_items_status_updated ON workshop_items(status, time_updated DESC);
            CREATE INDEX IF NOT EXISTS idx_workshop_items_file_type ON workshop_items(file_type);
            CREATE INDEX IF NOT EXISTS idx_workshop_items_subscriptions ON workshop_items(subscriptions DESC);

            CREATE TABLE IF NOT EXISTS workshop_item_dependencies (
                parent_workshop_id INTEGER NOT NULL,
                dep_workshop_id INTEGER NOT NULL,
                dep_title TEXT,
                required INTEGER NOT NULL DEFAULT 1,
                status TEXT NOT NULL DEFAULT 'active',
                updated_at TEXT NOT NULL,
                PRIMARY KEY (parent_workshop_id, dep_workshop_id),
                FOREIGN KEY (parent_workshop_id) REFERENCES workshop_items(workshop_id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_dependencies_child ON workshop_item_dependencies(dep_workshop_id);

            CREATE TABLE IF NOT EXISTS workshop_item_revisions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                workshop_id INTEGER NOT NULL,
                status TEXT NOT NULL,
                title TEXT NOT NULL,
                time_updated INTEGER,
                dependencies_json TEXT NOT NULL,
                source TEXT NOT NULL,
                recorded_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_revisions_wid ON workshop_item_revisions(workshop_id, id DESC);

            CREATE TABLE IF NOT EXISTS audit_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                workshop_id INTEGER,
                actor TEXT NOT NULL,
                action TEXT NOT NULL,
                reason TEXT NOT NULL,
                status TEXT,
                time_updated INTEGER,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_audit_log_created ON audit_log(id DESC);

            CREATE TABLE IF NOT EXISTS sync_state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;

        if !has_column(conn, "workshop_items", "previews_json")? {
            conn.execute(
                "ALTER TABLE workshop_items ADD COLUMN previews_json TEXT NOT NULL DEFAULT '[]'",
                [],
            )?;
        }
        if !has_column(conn, "workshop_items", "is_admin_override")? {
            conn.execute(
                "ALTER TABLE workshop_items ADD COLUMN is_admin_override INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        conn.execute(
            "UPDATE workshop_items SET is_admin_override = 1
             WHERE source = 'admin_override' AND is_admin_override = 0",
            [],
        )?;
        if !has_column(conn, "workshop_items", "is_listed")? {
            conn.execute(
                "ALTER TABLE workshop_items ADD COLUMN is_listed INTEGER",
                [],
            )?;
            conn.execute(
                "UPDATE workshop_items SET is_listed = 1 WHERE is_admin_override = 1",
                [],
            )?;
        }
        if !has_column(conn, "workshop_items", "steam_version")? {
            conn.execute(
                "ALTER TABLE workshop_items ADD COLUMN steam_version INTEGER",
                [],
            )?;
            conn.execute(
                "UPDATE workshop_items SET steam_version = time_updated WHERE is_admin_override = 0",
                [],
            )?;
        }
        if !has_column(conn, "workshop_items", "catalog_indexed")? {
            conn.execute(
                "ALTER TABLE workshop_items ADD COLUMN catalog_indexed INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
            // The legacy cache recorded no per-item catalog provenance beyond admin overrides.
            conn.execute(
                "UPDATE workshop_items SET catalog_indexed = 1 WHERE is_admin_override = 1",
                [],
            )?;
            conn.execute(
                "DELETE FROM sync_state WHERE key IN (?1, ?2, ?3)",
                params![
                    STATE_INCREMENTAL_SCAN,
                    STATE_BACKFILL_CURSOR,
                    STATE_BACKFILL_SYNCED_TOTAL,
                ],
            )?;
        }
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_workshop_items_catalog_public
             ON workshop_items(catalog_indexed, is_listed)",
            [],
        )?;
        if has_column(conn, "workshop_item_revisions", "trust_score")? {
            conn.execute(
                "ALTER TABLE workshop_item_revisions DROP COLUMN trust_score",
                [],
            )?;
        }

        let index_current = Self::search_index_is_current(conn)?;
        // Older CM builds may have cached account-visible details as full private rows.
        Self::scrub_private_details_tx(conn, None, index_current)?;
        if !index_current {
            Self::rebuild_search_index_tx(conn)?;
        }
        Ok(())
    }

    fn search_index_is_current(tx: &Transaction<'_>) -> anyhow::Result<bool> {
        let current_version: Option<String> = tx
            .query_row(
                "SELECT value FROM sync_state WHERE key = ?1",
                params![STATE_SEARCH_INDEX_VERSION],
                |r| r.get(0),
            )
            .optional()?;
        Ok(current_version.as_deref() == Some(SEARCH_INDEX_VERSION)
            && has_table(tx, "workshop_items_fts")?)
    }

    fn rebuild_search_index_tx(tx: &Transaction<'_>) -> anyhow::Result<()> {
        tx.execute_batch(
            r#"
            DROP TABLE IF EXISTS workshop_items_fts;
            CREATE VIRTUAL TABLE workshop_items_fts USING fts5(
                title_tokens,
                title_compact,
                title_pinyin,
                creator_tokens,
                desc_tokens,
                desc_aliases,
                tokenize = 'unicode61 remove_diacritics 0'
            );
            "#,
        )?;

        let mut select_stmt =
            tx.prepare("SELECT workshop_id, title, creator_name, description FROM workshop_items")?;
        let rows = select_stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut insert_stmt = tx.prepare(INSERT_SEARCH_DOCUMENT)?;
        for row in rows {
            let (wid, title, creator_name, description) = row?;
            let doc = search::build_indexed_document(
                &title,
                creator_name.as_deref(),
                description.as_deref(),
            );
            insert_stmt.execute(params![
                wid,
                doc.title_tokens,
                doc.title_compact,
                doc.title_pinyin,
                doc.creator_tokens,
                doc.desc_tokens,
                doc.desc_aliases,
            ])?;
        }
        drop(insert_stmt);
        drop(select_stmt);

        tx.execute(
            "INSERT INTO sync_state (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![STATE_SEARCH_INDEX_VERSION, SEARCH_INDEX_VERSION],
        )?;
        Ok(())
    }

    fn upsert_search_index_tx(
        tx: &Connection,
        wid: i64,
        title: &str,
        creator_name: Option<&str>,
        description: Option<&str>,
    ) -> anyhow::Result<()> {
        let doc = search::build_indexed_document(title, creator_name, description);
        tx.prepare_cached("DELETE FROM workshop_items_fts WHERE rowid = ?1")?
            .execute(params![wid])?;
        tx.prepare_cached(INSERT_SEARCH_DOCUMENT)?.execute(params![
            wid,
            doc.title_tokens,
            doc.title_compact,
            doc.title_pinyin,
            doc.creator_tokens,
            doc.desc_tokens,
            doc.desc_aliases,
        ])?;
        Ok(())
    }

    pub fn get_item(&self, workshop_id: WorkshopId) -> anyhow::Result<Option<StoredItemMeta>> {
        self.with_read_snapshot(|snapshot| Self::get_item_with_conn(snapshot, workshop_id))
    }

    pub fn get_item_tx(
        tx: &Connection,
        workshop_id: WorkshopId,
    ) -> anyhow::Result<Option<StoredItemMeta>> {
        Self::get_item_with_conn(tx, workshop_id)
    }

    fn get_item_with_conn(
        conn: &Connection,
        workshop_id: WorkshopId,
    ) -> anyhow::Result<Option<StoredItemMeta>> {
        let mut stmt = conn.prepare(
            "SELECT file_type, status, title, description, creator_steam_id,
                    creator_name, preview_url, previews_json, file_url, file_size,
                    time_created, time_updated, tags_json, collection_children_json,
                    subscriptions, favorites, is_admin_override, source,
                    fetched_at, removed_at, last_verified_at, steam_version,
                    catalog_indexed, is_listed
             FROM workshop_items WHERE workshop_id = ?1",
        )?;

        let row = stmt
            .query_row(params![workshop_id.as_u64() as i64], |r| {
                let file_type_str: String = r.get(0)?;
                let status_str: String = r.get(1)?;
                let previews_json: String = r.get(7)?;
                let tags_json: String = r.get(12)?;
                let children_json: String = r.get(13)?;
                let is_override: i64 = r.get(16)?;
                let source_str: String = r.get(17)?;
                let fetched_str: String = r.get(18)?;
                let removed_str: Option<String> = r.get(19)?;
                let verified_str: String = r.get(20)?;
                let catalog_indexed: i64 = r.get(22)?;
                let is_listed: Option<i64> = r.get(23)?;

                let record = WorkshopItemRecord {
                    workshop_id,
                    file_type: parse_json_enum(&file_type_str, WorkshopFileType::Item),
                    status: parse_json_enum(&status_str, WorkshopItemStatus::Unknown),
                    title: r.get(2)?,
                    description: r.get(3)?,
                    creator_steam_id: r.get(4)?,
                    creator_name: r.get(5)?,
                    preview_url: r.get(6)?,
                    previews: serde_json::from_str::<Vec<WorkshopPreview>>(&previews_json)
                        .unwrap_or_default(),
                    file_url: r.get(8)?,
                    file_size: r.get::<_, Option<i64>>(9)?.map(|v| v as u64),
                    time_created: r.get(10)?,
                    time_updated: r.get(11)?,
                    tags: serde_json::from_str(&tags_json).unwrap_or_default(),
                    dependencies: Vec::new(),
                    collection_children: serde_json::from_str(&children_json).unwrap_or_default(),
                    subscriptions: r.get::<_, Option<i64>>(14)?.map(|v| v as u64),
                    favorites: r.get::<_, Option<i64>>(15)?.map(|v| v as u64),
                    source: parse_json_enum(&source_str, MetadataSource::CatalogServer),
                    fetched_at: parse_rfc3339_or_now(&fetched_str),
                };
                Ok(StoredItemMeta {
                    record,
                    is_admin_override: is_override != 0,
                    last_verified_at: verified_str.parse().unwrap_or(DateTime::UNIX_EPOCH),
                    removed_at: removed_str.as_deref().map(parse_rfc3339_or_now),
                    steam_version: r.get(21)?,
                    catalog_indexed: catalog_indexed != 0,
                    is_listed: is_listed.map(|v| v != 0),
                })
            })
            .optional()?;

        let Some(mut meta) = row else {
            return Ok(None);
        };
        meta.record.dependencies = Self::load_active_deps(conn, workshop_id)?;
        Ok(Some(meta))
    }

    fn load_active_deps(
        conn: &Connection,
        parent_id: WorkshopId,
    ) -> anyhow::Result<Vec<WorkshopDependency>> {
        let mut stmt = conn.prepare(
            "SELECT edge.dep_workshop_id,
                    CASE WHEN dep.status = 'private_or_unavailable' THEN NULL ELSE edge.dep_title END,
                    edge.required
             FROM workshop_item_dependencies edge
             LEFT JOIN workshop_items dep ON dep.workshop_id = edge.dep_workshop_id
             WHERE edge.parent_workshop_id = ?1 AND edge.status = 'active'
             ORDER BY edge.dep_workshop_id ASC",
        )?;
        let rows = stmt.query_map(params![parent_id.as_u64() as i64], |r| {
            let dep_raw: i64 = r.get(0)?;
            Ok(WorkshopDependency {
                workshop_id: parse_workshop_id_col(0, dep_raw)?,
                title: r.get(1)?,
                required: r.get::<_, i64>(2)? != 0,
            })
        })?;
        let mut out = Vec::new();
        for dep in rows {
            out.push(dep?);
        }
        Ok(out)
    }

    pub fn get_batch(
        &self,
        ids: &[WorkshopId],
    ) -> anyhow::Result<(Vec<StoredItemMeta>, Vec<WorkshopId>)> {
        self.with_read_snapshot(|snapshot| {
            let mut found = Vec::new();
            let mut missing = Vec::new();
            for id in ids {
                match Self::get_item_with_conn(snapshot, *id)? {
                    Some(meta) => found.push(meta),
                    None => missing.push(*id),
                }
            }
            Ok((found, missing))
        })
    }

    pub fn get_admin_item_detail(
        &self,
        workshop_id: WorkshopId,
    ) -> anyhow::Result<Option<AdminItemDetailResponse>> {
        self.with_read_snapshot(|snapshot| {
            let Some(meta) = Self::get_item_with_conn(snapshot, workshop_id)? else {
                return Ok(None);
            };
            let revisions = Self::list_revisions_with_conn(snapshot, workshop_id)?;
            Ok(Some(AdminItemDetailResponse {
                item: meta.record,
                is_admin_override: meta.is_admin_override,
                last_verified_at: meta.last_verified_at,
                removed_at: meta.removed_at,
                revisions,
            }))
        })
    }

    pub fn upsert_item(
        &self,
        record: &WorkshopItemRecord,
        is_admin_override: bool,
        replace_deps: bool,
    ) -> anyhow::Result<()> {
        self.with_transaction(|tx| {
            Self::upsert_item_tx(
                tx,
                record,
                is_admin_override,
                replace_deps,
                true,
                (!is_admin_override).then_some(true),
                Some(true),
            )
        })
    }

    fn upsert_item_tx(
        tx: &Connection,
        record: &WorkshopItemRecord,
        is_admin_override: bool,
        replace_deps: bool,
        record_revision: bool,
        listed: Option<bool>,
        catalog_indexed: Option<bool>,
    ) -> anyhow::Result<()> {
        let wid = record.workshop_id.as_u64() as i64;
        let now = Utc::now().to_rfc3339();
        let removed_at = match record.status {
            WorkshopItemStatus::Removed | WorkshopItemStatus::PrivateOrUnavailable => {
                Some(now.clone())
            }
            _ => None,
        };

        let searchable_fields_unchanged = tx
            .prepare_cached(
                "SELECT 1 FROM workshop_items WHERE workshop_id = ?1 AND title IS ?2
                    AND creator_name IS COALESCE(?3, creator_name)
                    AND description IS COALESCE(?4, description)",
            )?
            .exists(params![
                wid,
                record.title,
                record.creator_name,
                record.description,
            ])?;
        let mut upsert = tx.prepare_cached(
            r#"
            INSERT INTO workshop_items (
                workshop_id, file_type, status, title, description, creator_steam_id,
                creator_name, preview_url, previews_json, file_url, file_size,
                time_created, time_updated, tags_json, collection_children_json,
                subscriptions, favorites, is_admin_override, source,
                fetched_at, removed_at, last_verified_at, is_listed, steam_version,
                catalog_indexed
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?20,
                COALESCE(?22, ?18), ?23, COALESCE(?24, ?18)
            )
            ON CONFLICT(workshop_id) DO UPDATE SET
                file_type = excluded.file_type,
                status = excluded.status,
                title = excluded.title,
                description = COALESCE(excluded.description, workshop_items.description),
                creator_steam_id = COALESCE(excluded.creator_steam_id, workshop_items.creator_steam_id),
                creator_name = COALESCE(excluded.creator_name, workshop_items.creator_name),
                preview_url = COALESCE(excluded.preview_url, workshop_items.preview_url),
                previews_json = excluded.previews_json,
                file_url = excluded.file_url,
                file_size = COALESCE(excluded.file_size, workshop_items.file_size),
                time_created = COALESCE(excluded.time_created, workshop_items.time_created),
                time_updated = COALESCE(excluded.time_updated, workshop_items.time_updated),
                tags_json = CASE WHEN excluded.tags_json = '[]' THEN workshop_items.tags_json ELSE excluded.tags_json END,
                collection_children_json = CASE WHEN excluded.collection_children_json = '[]' THEN workshop_items.collection_children_json ELSE excluded.collection_children_json END,
                subscriptions = COALESCE(excluded.subscriptions, workshop_items.subscriptions),
                favorites = COALESCE(excluded.favorites, workshop_items.favorites),
                is_admin_override = excluded.is_admin_override,
                source = excluded.source,
                fetched_at = excluded.fetched_at,
                removed_at = CASE WHEN excluded.status = 'active' THEN NULL ELSE COALESCE(workshop_items.removed_at, excluded.removed_at) END,
                last_verified_at = excluded.last_verified_at,
                is_listed = COALESCE(?22, workshop_items.is_listed),
                steam_version = COALESCE(excluded.steam_version, workshop_items.steam_version),
                catalog_indexed = CASE
                    WHEN ?24 = 1 THEN 1
                    ELSE workshop_items.catalog_indexed
                END
            RETURNING title, creator_name, description
            "#,
        )?;
        let updated_search_fields = upsert.query_row(
            params![
                wid,
                serialize_json_enum(&record.file_type),
                serialize_json_enum(&record.status),
                record.title,
                record.description,
                record.creator_steam_id,
                record.creator_name,
                record.preview_url,
                serde_json::to_string(&record.previews)?,
                record.file_url,
                record.file_size.map(|v| v as i64),
                record.time_created,
                record.time_updated,
                serde_json::to_string(&record.tags)?,
                serde_json::to_string(&record.collection_children)?,
                record.subscriptions.map(|v| v as i64),
                record.favorites.map(|v| v as i64),
                if is_admin_override { 1 } else { 0 },
                serialize_json_enum(&record.source),
                record.fetched_at.to_rfc3339(),
                removed_at,
                listed,
                listed.and(record.time_updated),
                catalog_indexed,
            ],
            |r| {
                if searchable_fields_unchanged {
                    Ok(None)
                } else {
                    Ok(Some((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    )))
                }
            },
        )?;
        drop(upsert);

        if let Some((title, creator, description)) = updated_search_fields {
            Self::upsert_search_index_tx(
                tx,
                wid,
                &title,
                creator.as_deref(),
                description.as_deref(),
            )?;
        }

        if replace_deps {
            tx.execute(
                "UPDATE workshop_item_dependencies SET status = 'removed_upstream', updated_at = ?2 WHERE parent_workshop_id = ?1",
                params![wid, now],
            )?;
        }

        for dep in &record.dependencies {
            let dep_meta = tx
                .query_row(
                    "SELECT title, status FROM workshop_items WHERE workshop_id = ?1",
                    params![dep.workshop_id.as_u64() as i64],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            let resolved_dep_title = match dep_meta {
                Some((_, status)) if status == "private_or_unavailable" => None,
                Some((title, _)) => dep.title.clone().or(Some(title)),
                None => dep.title.clone(),
            };
            tx.execute(
                r#"
                INSERT INTO workshop_item_dependencies (
                    parent_workshop_id, dep_workshop_id, dep_title, required, status, updated_at
                ) VALUES (?1, ?2, ?3, ?4, 'active', ?5)
                ON CONFLICT(parent_workshop_id, dep_workshop_id) DO UPDATE SET
                    dep_title = COALESCE(excluded.dep_title, workshop_item_dependencies.dep_title),
                    required = excluded.required,
                    status = 'active',
                    updated_at = excluded.updated_at
                "#,
                params![
                    wid,
                    dep.workshop_id.as_u64() as i64,
                    resolved_dep_title,
                    if dep.required { 1 } else { 0 },
                    now,
                ],
            )?;
        }

        // 同时反哺所有将该条目作为前置依赖的父条目 dep_title
        if record.status != WorkshopItemStatus::PrivateOrUnavailable
            && !record.title.starts_with("Workshop #")
        {
            tx.execute(
                "UPDATE workshop_item_dependencies SET dep_title = ?2 WHERE dep_workshop_id = ?1 AND dep_title IS NULL",
                params![wid, record.title],
            )?;
        }

        if record_revision {
            Self::insert_revision_tx(tx, record, &now)?;
        }

        Ok(())
    }

    fn insert_revision_tx(
        tx: &Connection,
        record: &WorkshopItemRecord,
        recorded_at: &str,
    ) -> anyhow::Result<()> {
        tx.execute(
            r#"
            INSERT INTO workshop_item_revisions (
                workshop_id, status, title, time_updated, dependencies_json, source, recorded_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                record.workshop_id.as_u64() as i64,
                serialize_json_enum(&record.status),
                record.title,
                record.time_updated,
                serde_json::to_string(&record.dependencies)?,
                serialize_json_enum(&record.source),
                recorded_at,
            ],
        )?;
        Ok(())
    }

    /// 将 SteamKit CM 上游拉取结果原子写入缓存库（默认纳入公开目录索引）。
    pub fn apply_upstream_outcomes(
        &self,
        observations: &[WorkshopObservation],
    ) -> anyhow::Result<UpstreamApplySummary> {
        self.apply_upstream_outcomes_with_policy(observations, CatalogIndexPolicy::IndexInCatalog)
    }

    /// 将 SteamKit CM 上游拉取结果原子写入缓存库：
    /// - 跳过 `is_admin_override = 1` 的管理员锁定条目；
    /// - 按 `index_policy` 区分公开目录同步 (`IndexInCatalog`) 与客户端按 ID 穿透缓存 (`CacheOnly`)；
    /// - 对更新后的组件刷新 `file_url`、依赖与元数据，并在关键字段变化时追加历史修订；
    /// - 对上游变为 `Removed` 或 `PrivateOrUnavailable` 的组件执行墓碑归档（清空失效 `file_url`，私密条目清理公开缓存中的详细信息，已下架条目保留历史元数据）。
    pub fn apply_upstream_outcomes_with_policy(
        &self,
        observations: &[WorkshopObservation],
        index_policy: CatalogIndexPolicy,
    ) -> anyhow::Result<UpstreamApplySummary> {
        if observations.is_empty() {
            return Ok(UpstreamApplySummary::default());
        }
        self.apply_upstream_batch(observations, index_policy, |_, summary| Ok(summary))
    }

    pub(crate) fn apply_backfill_page(
        &self,
        observations: &[WorkshopObservation],
        next_cursor: &str,
        remote_total: u64,
    ) -> anyhow::Result<u64> {
        // Checkpoint the page with its writes so a retry can still count membership-only updates.
        self.apply_upstream_batch(
            observations,
            CatalogIndexPolicy::IndexInCatalog,
            |tx, summary| {
                let count = summary.active_written();
                let prev_total = Self::get_sync_state_with_conn(tx, STATE_BACKFILL_SYNCED_TOTAL)?
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0);
                Self::set_sync_states_tx(
                    tx,
                    &[
                        (STATE_BACKFILL_CURSOR, next_cursor),
                        (
                            STATE_BACKFILL_SYNCED_TOTAL,
                            &prev_total.saturating_add(count).to_string(),
                        ),
                    ],
                )?;
                if remote_total > 0 {
                    Self::set_sync_states_tx(
                        tx,
                        &[(STATE_REMOTE_WORKSHOP_TOTAL, &remote_total.to_string())],
                    )?;
                }
                Ok(count)
            },
        )
    }

    fn apply_upstream_batch<T>(
        &self,
        observations: &[WorkshopObservation],
        index_policy: CatalogIndexPolicy,
        finish: impl FnOnce(&Transaction<'_>, UpstreamApplySummary) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let promote_catalog = index_policy == CatalogIndexPolicy::IndexInCatalog;
        // Negative observations for unknown IDs must take part in the same write ordering.
        let mut writer = self.lock_writer()?;
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut pending_negatives: HashMap<WorkshopId, Option<NegativeObservation>> =
            HashMap::new();
        let summary = {
            let negatives = self.negative_cache.read().unwrap();
            let mut summary = UpstreamApplySummary::default();
            for observation in observations {
                let wid = observation.outcome.workshop_id();
                let observed_at = observation.observed_at();
                let existing = Self::get_item_tx(&tx, wid)?;
                if index_policy == CatalogIndexPolicy::InsertOnlyCache && existing.is_some() {
                    summary.skipped_stale += 1;
                    continue;
                }
                if existing.as_ref().is_some_and(|prev| prev.is_admin_override) {
                    summary.skipped_overrides += 1;
                    continue;
                }
                let incoming_version = match &observation.outcome {
                    WorkshopFetchOutcome::Found { item }
                        if item.status != WorkshopItemStatus::PrivateOrUnavailable =>
                    {
                        item.time_updated
                    }
                    _ => None,
                };
                let stale_version = existing
                    .as_ref()
                    .and_then(|prev| prev.steam_version)
                    .zip(incoming_version)
                    .is_some_and(|(cached, incoming)| incoming < cached);
                let latest_negative = pending_negatives
                    .get(&wid)
                    .copied()
                    .unwrap_or_else(|| negatives.get(&wid).copied());
                if existing
                    .as_ref()
                    .is_some_and(|prev| observed_at <= prev.last_verified_at)
                    || latest_negative.is_some_and(|prev| observed_at <= prev.observed_at)
                    || stale_version
                {
                    let can_promote_existing = promote_catalog
                        && observation.listed
                        && matches!(
                            &observation.outcome,
                            WorkshopFetchOutcome::Found { item }
                                if item.status == WorkshopItemStatus::Active
                        )
                        && existing.as_ref().is_some_and(|prev| {
                            !prev.catalog_indexed
                                && prev.record.status == WorkshopItemStatus::Active
                        });
                    let promoted = can_promote_existing
                        && tx.execute(
                            "UPDATE workshop_items SET catalog_indexed = 1
                             WHERE workshop_id = ?1 AND catalog_indexed = 0 AND is_listed = 1",
                            params![wid.as_u64() as i64],
                        )? > 0;
                    if promoted {
                        summary.catalog_promoted += 1;
                    } else {
                        summary.skipped_stale += 1;
                    }
                    continue;
                }

                let (status, source, reason) = match &observation.outcome {
                    WorkshopFetchOutcome::Found { item }
                        if item.status != WorkshopItemStatus::PrivateOrUnavailable =>
                    {
                        let changed = existing.as_ref().is_none_or(|prev| {
                            prev.record.status != item.status
                                || prev.record.title != item.title
                                || prev.record.time_updated != item.time_updated
                                || prev.record.file_url != item.file_url
                                || dependency_requirements(&prev.record.dependencies)
                                    != dependency_requirements(&item.dependencies)
                        });
                        Self::upsert_item_tx(
                            &tx,
                            item,
                            false,
                            true,
                            changed,
                            Some(observation.listed),
                            (promote_catalog && observation.listed).then_some(true),
                        )?;
                        pending_negatives.insert(wid, None);
                        summary.active_upserted += 1;
                        continue;
                    }
                    WorkshopFetchOutcome::Found { item } => {
                        (item.status, item.source, "Steam CM 上游标记私密或不可用")
                    }
                    WorkshopFetchOutcome::Removed { source, reason, .. } => (
                        WorkshopItemStatus::Removed,
                        *source,
                        reason.as_deref().unwrap_or("Steam CM 上游标记已下架"),
                    ),
                    WorkshopFetchOutcome::PrivateOrUnavailable { source, reason, .. } => (
                        WorkshopItemStatus::PrivateOrUnavailable,
                        *source,
                        reason.as_deref().unwrap_or("Steam CM 上游标记私密或不可用"),
                    ),
                };
                if let Some(prev) = existing {
                    Self::apply_tombstone_tx(&tx, &prev, observed_at, status, source, reason)?;
                    pending_negatives.insert(wid, None);
                    summary.tombstoned += 1;
                } else {
                    if status == WorkshopItemStatus::PrivateOrUnavailable {
                        tx.execute(
                            "UPDATE workshop_item_dependencies SET dep_title = NULL WHERE dep_workshop_id = ?1",
                            params![wid.as_u64() as i64],
                        )?;
                    }
                    pending_negatives.insert(
                        wid,
                        Some(NegativeObservation {
                            status,
                            observed_at,
                        }),
                    );
                }
            }
            summary
        };
        let result = finish(&tx, summary)?;
        tx.commit()?;

        let mut negatives = self.negative_cache.write().unwrap();
        for (wid, update) in pending_negatives {
            if let Some(observation) = update {
                if negatives.len() >= MAX_NEGATIVE_CACHE_ENTRIES {
                    negatives.clear();
                }
                negatives.insert(wid, observation);
            } else {
                negatives.remove(&wid);
            }
        }
        Ok(result)
    }

    pub(crate) fn negative_status(
        &self,
        wid: WorkshopId,
        ttl_secs: u64,
    ) -> Option<WorkshopItemStatus> {
        let entries = self.negative_cache.read().unwrap();
        let entry = entries.get(&wid)?;
        let age = (Utc::now() - entry.observed_at).num_seconds().max(0) as u64;
        (age < ttl_secs).then_some(entry.status)
    }

    pub(crate) fn negative_observed_at(
        &self,
        wid: WorkshopId,
    ) -> Option<(WorkshopItemStatus, DateTime<Utc>)> {
        let entries = self.negative_cache.read().unwrap();
        entries
            .get(&wid)
            .map(|entry| (entry.status, entry.observed_at))
    }

    pub(crate) fn listing_recheck_ids(
        &self,
        after: Option<WorkshopId>,
        verified_before: i64,
        limit: usize,
    ) -> anyhow::Result<Vec<WorkshopId>> {
        let conn = self.lock_reader()?;
        let mut stmt = conn.prepare(
            "SELECT workshop_id FROM workshop_items
             WHERE workshop_id > ?1 AND status = 'active' AND is_admin_override = 0
               AND catalog_indexed = 1
               AND (is_listed IS NULL OR (is_listed = 1 AND unixepoch(last_verified_at) <= ?2))
             ORDER BY workshop_id LIMIT ?3",
        )?;
        let after = after.map_or(0, |id| id.as_u64() as i64);
        let rows = stmt.query_map(params![after, verified_before, limit], |row| {
            parse_workshop_id_col(0, row.get(0)?)
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn apply_tombstone_tx(
        tx: &Connection,
        prev: &StoredItemMeta,
        observed_at: DateTime<Utc>,
        next_status: WorkshopItemStatus,
        source: MetadataSource,
        reason: &str,
    ) -> anyhow::Result<()> {
        let wid = prev.record.workshop_id;
        let observed_at = observed_at.to_rfc3339();
        tx.execute(
            "UPDATE workshop_items
             SET status = ?2, file_url = NULL, source = ?3,
                 fetched_at = ?4, removed_at = COALESCE(removed_at, ?4),
                 last_verified_at = ?4
             WHERE workshop_id = ?1",
            params![
                wid.as_u64() as i64,
                serialize_json_enum(&next_status),
                serialize_json_enum(&source),
                observed_at,
            ],
        )?;
        if next_status == WorkshopItemStatus::PrivateOrUnavailable {
            Self::scrub_private_details_tx(tx, Some(wid), true)?;
        }
        if prev.record.status != next_status {
            let rev_record = Self::get_item_tx(tx, wid)?
                .ok_or_else(|| anyhow::anyhow!("更新墓碑后找不到工坊条目 {wid}"))?
                .record;
            Self::insert_revision_tx(tx, &rev_record, &Utc::now().to_rfc3339())?;
            Self::append_audit_log_tx(
                tx,
                Some(wid),
                "steamkit-sync",
                "tombstone_detected",
                reason,
                Some(next_status),
                prev.record.time_updated,
            )?;
        }
        Ok(())
    }

    fn scrub_private_details_tx(
        tx: &Connection,
        wid: Option<WorkshopId>,
        update_index: bool,
    ) -> anyhow::Result<()> {
        let wid = wid.map(|id| id.as_u64() as i64);
        tx.execute(
            "UPDATE workshop_item_dependencies SET dep_title = NULL
             WHERE dep_title IS NOT NULL AND dep_workshop_id IN (
                 SELECT workshop_id FROM workshop_items
                 WHERE status = 'private_or_unavailable'
                   AND (?1 IS NULL OR workshop_id = ?1)
             )",
            params![wid],
        )?;
        tx.execute(
            "DELETE FROM workshop_item_dependencies WHERE parent_workshop_id IN (
                SELECT workshop_id FROM workshop_items
                WHERE status = 'private_or_unavailable' AND is_admin_override = 0
                  AND (?1 IS NULL OR workshop_id = ?1)
            )",
            params![wid],
        )?;
        let mut changed_ids = Vec::new();
        if update_index {
            let mut stmt = tx.prepare_cached(
                "SELECT workshop_id FROM workshop_items
                 WHERE status = 'private_or_unavailable' AND is_admin_override = 0
                   AND (?1 IS NULL OR workshop_id = ?1)
                   AND (title IS NOT 'Workshop #' || workshop_id
                        OR description IS NOT NULL OR creator_name IS NOT NULL)",
            )?;
            let rows = stmt.query_map(params![wid], |r| r.get::<_, i64>(0))?;
            changed_ids = rows.collect::<Result<Vec<_>, _>>()?;
        }
        tx.execute(
            "UPDATE workshop_items
             SET title = 'Workshop #' || workshop_id, description = NULL,
                 creator_steam_id = NULL, creator_name = NULL, preview_url = NULL,
                 previews_json = '[]', file_url = NULL, file_size = NULL,
                 time_created = NULL, time_updated = NULL, tags_json = '[]',
                 collection_children_json = '[]', subscriptions = NULL, favorites = NULL
             WHERE status = 'private_or_unavailable' AND is_admin_override = 0
               AND (?1 IS NULL OR workshop_id = ?1)",
            params![wid],
        )?;
        for scrub_wid in changed_ids {
            Self::upsert_search_index_tx(
                tx,
                scrub_wid,
                &format!("Workshop #{scrub_wid}"),
                None,
                None,
            )?;
        }
        Ok(())
    }

    pub fn apply_admin_override(
        &self,
        wid: WorkshopId,
        actor: &str,
        req: AdminItemOverrideRequest,
    ) -> anyhow::Result<WorkshopItemRecord> {
        self.with_transaction(|tx| {
            let existing = Self::get_item_tx(tx, wid)?;
            let replace_deps = req.dependencies.is_some();
            let now = Utc::now();

            let next_record = if let Some(cur) = existing {
                let mut r = cur.record;
                r.status = req.status;
                if let Some(title) = req.title.filter(|s| !s.trim().is_empty()) {
                    r.title = title;
                }
                if let Some(desc) = req.description {
                    r.description = Some(desc);
                }
                if let Some(url) = req.file_url {
                    r.file_url = if url.trim().is_empty() {
                        None
                    } else {
                        Some(url.trim().to_string())
                    };
                }
                if let Some(preview) = req.preview_url {
                    r.preview_url = if preview.trim().is_empty() {
                        None
                    } else {
                        Some(preview.trim().to_string())
                    };
                }
                if let Some(deps) = req.dependencies {
                    r.dependencies = deps;
                }
                r.source = MetadataSource::AdminOverride;
                r.fetched_at = now;
                r
            } else {
                WorkshopItemRecord {
                    workshop_id: wid,
                    file_type: WorkshopFileType::Item,
                    status: req.status,
                    title: req
                        .title
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| format!("Workshop #{wid}")),
                    description: req.description,
                    creator_steam_id: None,
                    creator_name: None,
                    preview_url: req.preview_url.filter(|s| !s.trim().is_empty()),
                    previews: Vec::new(),
                    file_url: req.file_url.filter(|s| !s.trim().is_empty()),
                    file_size: None,
                    time_created: None,
                    time_updated: Some(now.timestamp()),
                    tags: Vec::new(),
                    dependencies: req.dependencies.unwrap_or_default(),
                    collection_children: Vec::new(),
                    subscriptions: None,
                    favorites: None,
                    source: MetadataSource::AdminOverride,
                    fetched_at: now,
                }
            };

            Self::upsert_item_tx(tx, &next_record, true, replace_deps, true, None, Some(true))?;
            let reason = req
                .reason
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "管理员手动覆写并锁定条目状态".to_string());
            Self::append_audit_log_tx(
                tx,
                Some(wid),
                actor,
                "admin_override",
                &reason,
                Some(next_record.status),
                next_record.time_updated,
            )?;
            Ok(next_record)
        })
    }

    pub fn clear_admin_override(&self, wid: WorkshopId, actor: &str) -> anyhow::Result<bool> {
        self.with_transaction(|tx| {
            let changed = tx.execute(
                "UPDATE workshop_items SET is_admin_override = 0, source = 'catalog_server' WHERE workshop_id = ?1",
                params![wid.as_u64() as i64],
            )?;
            if changed > 0 {
                Self::append_audit_log_tx(
                    tx,
                    Some(wid),
                    actor,
                    "clear_override",
                    "管理员解除条目覆写锁定",
                    None,
                    None,
                )?;
            }
            Ok(changed > 0)
        })
    }

    pub fn search(&self, query: &SearchQuery, public_only: bool) -> anyhow::Result<SearchResponse> {
        let page = query.page.max(1);
        let page_size = query.page_size.clamp(1, 100);
        let offset = u64::from(page - 1) * u64::from(page_size);
        let ranked = self.search_ranked(query, public_only, offset, page_size)?;
        Ok(SearchResponse {
            items: ranked.items.into_iter().map(|item| item.record).collect(),
            total: ranked.total,
            page,
            page_size,
            source: MetadataSource::CatalogServer,
        })
    }

    pub(crate) fn search_ranked(
        &self,
        query: &SearchQuery,
        public_only: bool,
        offset: u64,
        limit: u32,
    ) -> anyhow::Result<RankedLocalSearch> {
        let limit = limit.clamp(1, 500);
        let filters = SearchFilters {
            tag: query
                .tag
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_ascii_lowercase),
            status: query.status.map(|status| serialize_json_enum(&status)),
            public_only,
            file_type: query.file_type.map(|ft| serialize_json_enum(&ft)),
        };
        let sort = SearchSort::from_name(query.sort.as_deref());
        let target = LocalSearchTarget::from_name(query.search_target.as_deref());
        let parsed = query.query.as_deref().and_then(search::parse_query);

        self.with_read_snapshot(|conn| {
            let (total, page_entries) = match &parsed {
                None => Self::browse_items(conn, &filters, sort, limit, offset)?,
                Some(parsed) => {
                    if let Some(id) = parsed
                        .exact_workshop_id
                        .filter(|_| parsed.numeric_prefix.is_none())
                    {
                        Self::search_exact_id(conn, &filters, id, offset)?
                    } else {
                        let mut selection = SearchPage::new(sort, limit, offset);
                        if parsed.terms.is_empty() {
                            Self::search_symbol_only(
                                conn,
                                parsed,
                                &filters,
                                target,
                                &mut selection,
                            )?;
                        } else {
                            Self::search_indexed(conn, parsed, &filters, target, &mut selection)?;
                        }
                        selection.finish()
                    }
                }
            };

            let mut items = Vec::with_capacity(page_entries.len());
            for (id, rank) in page_entries {
                if let Some(meta) = Self::get_item_with_conn(conn, id)? {
                    items.push(RankedLocalItem {
                        record: meta.record,
                        rank,
                        is_admin_override: meta.is_admin_override,
                        last_verified_at: meta.last_verified_at,
                    });
                }
            }

            Ok(RankedLocalSearch { total, items })
        })
    }

    fn browse_items(
        conn: &Connection,
        filters: &SearchFilters,
        sort: SearchSort,
        page_size: u32,
        offset: u64,
    ) -> anyhow::Result<RankedIdPage> {
        let total: u64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM workshop_items w WHERE {SEARCH_ITEM_FILTERS}"),
            params![
                filters.tag,
                filters.status,
                filters.public_only,
                filters.file_type
            ],
            |r| r.get(0),
        )?;

        let order_by = sort.browse_order();
        let sql = format!(
            "SELECT w.workshop_id FROM workshop_items w WHERE {SEARCH_ITEM_FILTERS}
             ORDER BY {order_by} LIMIT ?5 OFFSET ?6"
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(
            params![
                filters.tag,
                filters.status,
                filters.public_only,
                filters.file_type,
                page_size,
                offset
            ],
            |row| Ok((parse_workshop_id_col(0, row.get(0)?)?, None)),
        )?;
        Ok((total, rows.collect::<Result<Vec<_>, _>>()?))
    }

    fn search_exact_id(
        conn: &Connection,
        filters: &SearchFilters,
        id: u64,
        offset: u64,
    ) -> anyhow::Result<RankedIdPage> {
        let Ok(id) = i64::try_from(id) else {
            return Ok((0, Vec::new()));
        };
        let matched = conn
            .prepare_cached(&format!(
                "SELECT 1 FROM workshop_items w WHERE w.workshop_id = ?5 AND {SEARCH_ITEM_FILTERS}"
            ))?
            .exists(params![
                filters.tag,
                filters.status,
                filters.public_only,
                filters.file_type,
                id
            ])?;
        let ids = if matched && offset == 0 {
            vec![(
                WorkshopId(id as u64),
                Some(MatchRank {
                    tier: 0,
                    sub_rank: 0,
                }),
            )]
        } else {
            Vec::new()
        };
        Ok((u64::from(matched), ids))
    }

    fn search_indexed(
        conn: &Connection,
        parsed: &ParsedQuery,
        filters: &SearchFilters,
        target: LocalSearchTarget,
        page: &mut SearchPage,
    ) -> anyhow::Result<()> {
        if target != LocalSearchTarget::DescriptionOnly {
            if let Some(fts_query) = search::build_primary_fts_query(parsed) {
                let (source, match_filter) = if parsed.numeric_prefix.is_some() {
                    (
                        "FROM (
                             SELECT rowid AS workshop_id FROM workshop_items_fts WHERE workshop_items_fts MATCH ?5
                             UNION
                             SELECT workshop_id FROM workshop_items
                             WHERE workshop_id = ?6 OR CAST(workshop_id AS TEXT) LIKE ?7
                         ) candidates JOIN workshop_items w ON w.workshop_id = candidates.workshop_id",
                        "",
                    )
                } else {
                    (SEARCH_FTS_SOURCE, "AND workshop_items_fts MATCH ?5")
                };
                let mut stmt = conn.prepare_cached(&format!(
                    "SELECT {SEARCH_ORDER_COLUMNS}, w.creator_name
                     {source} WHERE {SEARCH_ITEM_FILTERS} {match_filter}"
                ))?;
                let rows = if let Some(prefix) = &parsed.numeric_prefix {
                    let exact = parsed
                        .exact_workshop_id
                        .and_then(|id| i64::try_from(id).ok());
                    stmt.query(params![
                        filters.tag,
                        filters.status,
                        filters.public_only,
                        filters.file_type,
                        fts_query,
                        exact,
                        format!("{prefix}%")
                    ])?
                } else {
                    stmt.query(params![
                        filters.tag,
                        filters.status,
                        filters.public_only,
                        filters.file_type,
                        fts_query
                    ])?
                };
                page.collect(rows, |row, id| {
                    let title = row.get_ref(1)?.as_str()?;
                    let creator = row.get_ref(5)?.as_str_or_null()?;
                    Ok(match target {
                        LocalSearchTarget::TitleOnly => {
                            search::score_title_only_candidate(parsed, id.as_u64(), title)
                        }
                        LocalSearchTarget::All => {
                            search::score_primary_candidate(parsed, id.as_u64(), title, creator)
                        }
                        LocalSearchTarget::DescriptionOnly => unreachable!(),
                    })
                })?;
            }
        }

        if target == LocalSearchTarget::DescriptionOnly
            || (target == LocalSearchTarget::All && page.total == 0)
        {
            if let Some(desc_query) = search::build_desc_fts_query(parsed) {
                let mut stmt = conn.prepare_cached(&format!(
                    "SELECT {SEARCH_ORDER_COLUMNS}, w.description {SEARCH_FTS_SOURCE}
                     WHERE workshop_items_fts MATCH ?5 AND {SEARCH_ITEM_FILTERS}"
                ))?;
                let rows = stmt.query(params![
                    filters.tag,
                    filters.status,
                    filters.public_only,
                    filters.file_type,
                    desc_query
                ])?;
                page.collect(rows, |row, _| {
                    let description = row.get_ref(5)?.as_str_or_null()?;
                    Ok(description.and_then(|desc| search::score_desc_candidate(parsed, desc)))
                })?;
            }
        }

        Ok(())
    }

    fn search_symbol_only(
        conn: &Connection,
        parsed: &ParsedQuery,
        filters: &SearchFilters,
        target: LocalSearchTarget,
        page: &mut SearchPage,
    ) -> anyhow::Result<()> {
        if target != LocalSearchTarget::DescriptionOnly {
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT {SEARCH_ORDER_COLUMNS} FROM workshop_items w WHERE {SEARCH_ITEM_FILTERS}"
            ))?;
            let rows = stmt.query(params![
                filters.tag,
                filters.status,
                filters.public_only,
                filters.file_type
            ])?;
            page.collect(rows, |row, _| {
                let title = search::fold_text(row.get_ref(1)?.as_str()?);
                let rank = if title == parsed.raw_folded {
                    Some(MatchRank {
                        tier: 0,
                        sub_rank: 1,
                    })
                } else if title.starts_with(&parsed.raw_folded) {
                    Some(MatchRank {
                        tier: 1,
                        sub_rank: 0,
                    })
                } else if title.contains(&parsed.raw_folded) {
                    Some(MatchRank {
                        tier: 2,
                        sub_rank: 0,
                    })
                } else {
                    None
                };
                Ok(rank)
            })?;
        }

        if target == LocalSearchTarget::DescriptionOnly
            || (target == LocalSearchTarget::All && page.total == 0)
        {
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT {SEARCH_ORDER_COLUMNS}, w.description FROM workshop_items w
                 WHERE {SEARCH_ITEM_FILTERS} AND w.description IS NOT NULL"
            ))?;
            let rows = stmt.query(params![
                filters.tag,
                filters.status,
                filters.public_only,
                filters.file_type
            ])?;
            page.collect(rows, |row, _| {
                let description = row.get_ref(5)?.as_str()?;
                let cleaned = search::strip_bbcode_and_urls(description);
                let matches = search::fold_text(&cleaned).contains(&parsed.raw_folded);
                Ok(matches.then_some(MatchRank {
                    tier: 6,
                    sub_rank: 0,
                }))
            })?;
        }

        Ok(())
    }

    pub fn get_collection(
        &self,
        collection_id: WorkshopId,
    ) -> anyhow::Result<Option<WorkshopCollectionRecord>> {
        self.with_read_snapshot(|snapshot| {
            let Some(meta) = Self::get_item_with_conn(snapshot, collection_id)? else {
                return Ok(None);
            };
            if meta.record.file_type != WorkshopFileType::Collection {
                return Ok(None);
            }
            let child_ids = meta.record.collection_children.clone();
            let mut items = Vec::new();
            for cid in &child_ids {
                if let Some(child_meta) = Self::get_item_with_conn(snapshot, *cid)? {
                    items.push(child_meta.record);
                }
            }
            Ok(Some(WorkshopCollectionRecord {
                collection_id,
                status: meta.record.status,
                title: Some(meta.record.title),
                description: meta.record.description,
                preview_url: meta.record.preview_url,
                child_ids,
                items,
                source: MetadataSource::CatalogServer,
                fetched_at: meta.record.fetched_at,
            }))
        })
    }

    pub fn append_audit_log(
        &self,
        workshop_id: Option<WorkshopId>,
        actor: &str,
        action: &str,
        reason: &str,
        status: Option<WorkshopItemStatus>,
        time_updated: Option<i64>,
    ) -> anyhow::Result<()> {
        let conn = self.lock_writer()?;
        Self::append_audit_log_tx(
            &conn,
            workshop_id,
            actor,
            action,
            reason,
            status,
            time_updated,
        )
    }

    pub fn append_audit_log_tx(
        tx: &Connection,
        workshop_id: Option<WorkshopId>,
        actor: &str,
        action: &str,
        reason: &str,
        status: Option<WorkshopItemStatus>,
        time_updated: Option<i64>,
    ) -> anyhow::Result<()> {
        tx.execute(
            r#"
            INSERT INTO audit_log (
                workshop_id, actor, action, reason, status, time_updated, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                workshop_id.map(|id| id.as_u64() as i64),
                actor,
                action,
                reason,
                status.map(|s| serialize_json_enum(&s)),
                time_updated,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_audit_logs(&self, limit: usize) -> anyhow::Result<Vec<AuditLogEntry>> {
        let conn = self.lock_reader()?;
        let mut stmt = conn.prepare(
            "SELECT id, workshop_id, actor, action, reason, status, time_updated, created_at
             FROM audit_log ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit.clamp(1, 500) as i64], |r| {
            let wid_raw: Option<i64> = r.get(1)?;
            let status_str: Option<String> = r.get(5)?;
            let created_str: String = r.get(7)?;
            Ok(AuditLogEntry {
                id: r.get(0)?,
                workshop_id: wid_raw
                    .and_then(|v| u64::try_from(v).ok())
                    .and_then(|v| WorkshopId::new(v).ok()),
                actor: r.get(2)?,
                action: r.get(3)?,
                reason: r.get(4)?,
                status: status_str
                    .as_deref()
                    .map(|s| parse_json_enum(s, WorkshopItemStatus::Unknown)),
                time_updated: r.get(6)?,
                created_at: parse_rfc3339_or_now(&created_str),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn list_revisions(
        &self,
        workshop_id: WorkshopId,
    ) -> anyhow::Result<Vec<ItemRevisionEntry>> {
        let conn = self.lock_reader()?;
        Self::list_revisions_with_conn(&conn, workshop_id)
    }

    fn list_revisions_with_conn(
        conn: &Connection,
        workshop_id: WorkshopId,
    ) -> anyhow::Result<Vec<ItemRevisionEntry>> {
        let mut stmt = conn.prepare(
            "SELECT id, status, title, time_updated, dependencies_json, source, recorded_at
             FROM workshop_item_revisions
             WHERE workshop_id = ?1
             ORDER BY id DESC
             LIMIT 100",
        )?;
        let rows = stmt.query_map(params![workshop_id.as_u64() as i64], |r| {
            let status_str: String = r.get(1)?;
            let deps_json: String = r.get(4)?;
            let source_str: String = r.get(5)?;
            let rec_str: String = r.get(6)?;
            Ok(ItemRevisionEntry {
                id: r.get(0)?,
                status: parse_json_enum(&status_str, WorkshopItemStatus::Unknown),
                title: r.get(2)?,
                time_updated: r.get(3)?,
                dependencies: serde_json::from_str(&deps_json).unwrap_or_default(),
                source: parse_json_enum(&source_str, MetadataSource::SteamKit),
                recorded_at: parse_rfc3339_or_now(&rec_str),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn list_removed_deps(&self, parent_id: WorkshopId) -> anyhow::Result<Vec<WorkshopId>> {
        let conn = self.lock_reader()?;
        let mut stmt = conn.prepare(
            "SELECT dep_workshop_id FROM workshop_item_dependencies WHERE parent_workshop_id = ?1 AND status = 'removed_upstream'",
        )?;
        let rows = stmt.query_map(params![parent_id.as_u64() as i64], |r| {
            let raw: i64 = r.get(0)?;
            parse_workshop_id_col(0, raw)
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_sync_state(&self, key: &str) -> anyhow::Result<Option<String>> {
        let conn = self.lock_reader()?;
        Self::get_sync_state_with_conn(&conn, key)
    }

    fn get_sync_state_with_conn(conn: &Connection, key: &str) -> anyhow::Result<Option<String>> {
        Ok(conn
            .query_row(
                "SELECT value FROM sync_state WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_sync_state(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.set_sync_states(&[(key, value)])
    }

    pub(crate) fn set_sync_states(&self, entries: &[(&str, &str)]) -> anyhow::Result<()> {
        self.with_transaction(|tx| Self::set_sync_states_tx(tx, entries))
    }

    fn set_sync_states_tx(tx: &Transaction<'_>, entries: &[(&str, &str)]) -> anyhow::Result<()> {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO sync_state (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )?;
        for (key, value) in entries {
            stmt.execute(params![key, value])?;
        }
        Ok(())
    }

    pub fn stats(&self) -> anyhow::Result<CatalogStats> {
        let conn = self.lock_reader()?;
        let count = |sql: &str| -> anyhow::Result<u64> {
            let n: i64 = conn.query_row(sql, [], |r| r.get(0))?;
            Ok(n.max(0) as u64)
        };
        Ok(CatalogStats {
            total_items: count("SELECT COUNT(*) FROM workshop_items")?,
            active_items: count("SELECT COUNT(*) FROM workshop_items WHERE status = 'active'")?,
            removed_items: count("SELECT COUNT(*) FROM workshop_items WHERE status = 'removed'")?,
            private_items: count(
                "SELECT COUNT(*) FROM workshop_items WHERE status = 'private_or_unavailable'",
            )?,
            collections_count: count(
                "SELECT COUNT(*) FROM workshop_items WHERE file_type = 'collection'",
            )?,
            overridden_items: count(
                "SELECT COUNT(*) FROM workshop_items WHERE is_admin_override = 1",
            )?,
            revisions_count: count("SELECT COUNT(*) FROM workshop_item_revisions")?,
            audit_logs_count: count("SELECT COUNT(*) FROM audit_log")?,
        })
    }
}

fn dependency_requirements(dependencies: &[WorkshopDependency]) -> HashMap<WorkshopId, bool> {
    dependencies
        .iter()
        .map(|dep| (dep.workshop_id, dep.required))
        .collect()
}

fn open_configured_conn(path: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    Ok(conn)
}

const SEARCH_ITEM_FILTERS: &str = "
    (?1 IS NULL OR EXISTS (SELECT 1 FROM json_each(w.tags_json) WHERE LOWER(value) = ?1))
    AND (?2 IS NULL OR w.status = ?2)
    AND (?3 = 0 OR (w.catalog_indexed = 1 AND w.is_listed = 1))
    AND (?4 IS NULL OR w.file_type = ?4)";

const SEARCH_ORDER_COLUMNS: &str = "
    w.workshop_id, w.title, COALESCE(w.subscriptions, 0),
    COALESCE(w.time_updated, 0), COALESCE(w.time_created, 0)";
const SEARCH_FTS_SOURCE: &str = "
    FROM workshop_items_fts JOIN workshop_items w ON w.workshop_id = workshop_items_fts.rowid";

#[derive(Debug, Clone)]
pub(crate) struct RankedLocalItem {
    pub record: WorkshopItemRecord,
    pub rank: Option<MatchRank>,
    pub is_admin_override: bool,
    pub last_verified_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub(crate) struct RankedLocalSearch {
    pub total: u64,
    pub items: Vec<RankedLocalItem>,
}

type RankedIdPage = (u64, Vec<(WorkshopId, Option<MatchRank>)>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalSearchTarget {
    All,
    TitleOnly,
    DescriptionOnly,
}

impl LocalSearchTarget {
    fn from_name(raw: Option<&str>) -> Self {
        match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("title" | "1") => Self::TitleOnly,
            Some("description" | "desc" | "2") => Self::DescriptionOnly,
            _ => Self::All,
        }
    }
}

struct SearchFilters {
    tag: Option<String>,
    status: Option<String>,
    public_only: bool,
    file_type: Option<String>,
}

#[derive(Clone, Copy)]
enum SearchSort {
    Updated,
    UpdatedAscending,
    Title,
    Subscriptions,
    Newest,
    Relevance,
}

impl SearchSort {
    fn from_name(sort: Option<&str>) -> Self {
        match sort.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("updated" | "lastupdated") => Self::Updated,
            Some("updated_asc") => Self::UpdatedAscending,
            Some("title") => Self::Title,
            Some("subscriptions" | "totalprofiles" | "popular" | "trend" | "toprated") => {
                Self::Subscriptions
            }
            Some("newest" | "mostrecent") => Self::Newest,
            _ => Self::Relevance,
        }
    }

    fn browse_order(self) -> &'static str {
        match self {
            Self::Updated => "COALESCE(w.time_updated, 0) DESC, w.workshop_id DESC",
            Self::UpdatedAscending => "COALESCE(w.time_updated, 0) ASC, w.workshop_id ASC",
            Self::Title => "w.title COLLATE NOCASE ASC, w.workshop_id ASC",
            Self::Subscriptions => "COALESCE(w.subscriptions, 0) DESC, w.workshop_id ASC",
            Self::Newest => "COALESCE(w.time_created, 0) DESC, w.workshop_id DESC",
            Self::Relevance => {
                "COALESCE(w.subscriptions, 0) DESC, COALESCE(w.time_updated, 0) DESC, w.workshop_id ASC"
            }
        }
    }

    fn entry(self, row: &Row<'_>, id: WorkshopId, rank: MatchRank) -> rusqlite::Result<PageEntry> {
        Ok(match self {
            Self::Updated => PageEntry::TimeDescending(Reverse((row.get(3)?, id))),
            Self::UpdatedAscending => PageEntry::TimeAscending(row.get(3)?, id),
            Self::Title => PageEntry::Title(row.get_ref(1)?.as_str()?.to_ascii_lowercase(), id),
            Self::Subscriptions => PageEntry::Subscriptions(Reverse(row.get(2)?), id),
            Self::Newest => PageEntry::TimeDescending(Reverse((row.get(4)?, id))),
            Self::Relevance => {
                PageEntry::Relevance(rank, Reverse(row.get(2)?), Reverse(row.get(3)?), id)
            }
        })
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum PageEntry {
    TimeDescending(Reverse<(i64, WorkshopId)>),
    TimeAscending(i64, WorkshopId),
    Title(String, WorkshopId),
    Subscriptions(Reverse<i64>, WorkshopId),
    Relevance(MatchRank, Reverse<i64>, Reverse<i64>, WorkshopId),
}

impl PageEntry {
    fn id(self) -> WorkshopId {
        match self {
            Self::TimeDescending(Reverse((_, id)))
            | Self::TimeAscending(_, id)
            | Self::Title(_, id)
            | Self::Subscriptions(_, id)
            | Self::Relevance(_, _, _, id) => id,
        }
    }
}

#[derive(PartialEq, Eq)]
struct RankedEntry {
    entry: PageEntry,
    rank: Option<MatchRank>,
}

impl Ord for RankedEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.entry.cmp(&other.entry)
    }
}

impl PartialOrd for RankedEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

struct SearchPage {
    sort: SearchSort,
    offset: u64,
    limit: u64,
    total: u64,
    best: BinaryHeap<RankedEntry>,
}

impl SearchPage {
    fn new(sort: SearchSort, page_size: u32, offset: u64) -> Self {
        Self {
            sort,
            offset,
            limit: offset + u64::from(page_size),
            total: 0,
            best: BinaryHeap::new(),
        }
    }

    fn collect(
        &mut self,
        mut rows: Rows<'_>,
        score: impl Fn(&Row<'_>, WorkshopId) -> rusqlite::Result<Option<MatchRank>>,
    ) -> rusqlite::Result<()> {
        while let Some(row) = rows.next()? {
            let id = parse_workshop_id_col(0, row.get(0)?)?;
            if let Some(rank) = score(row, id)? {
                self.total += 1;
                let entry = RankedEntry {
                    entry: self.sort.entry(row, id, rank)?,
                    rank: Some(rank),
                };
                // Count every validated match and keep only the best entries through this page.
                if (self.best.len() as u64) < self.limit {
                    self.best.push(entry);
                } else if let Some(mut last) = self.best.peek_mut() {
                    if entry < *last {
                        *last = entry;
                    }
                }
            }
        }
        Ok(())
    }

    fn finish(mut self) -> RankedIdPage {
        let offset = usize::try_from(self.offset).unwrap_or(usize::MAX);
        let page_len = self.best.len().saturating_sub(offset);
        // The requested page is at the end of the retained entries in sort order.
        let mut pairs: Vec<_> = std::iter::from_fn(|| self.best.pop())
            .take(page_len)
            .map(|ranked| (ranked.entry.id(), ranked.rank))
            .collect();
        pairs.reverse();
        (self.total, pairs)
    }
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
