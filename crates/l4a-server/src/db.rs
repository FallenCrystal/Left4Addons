use chrono::{DateTime, Utc};
use l4a_protocol::{
    AdminItemDetailResponse, AdminItemOverrideRequest, AuditLogEntry, CatalogStats,
    ItemRevisionEntry, MetadataSource, SearchQuery, SearchResponse, WorkshopCollectionRecord,
    WorkshopDependency, WorkshopFetchOutcome, WorkshopFileType, WorkshopId, WorkshopItemRecord,
    WorkshopItemStatus, WorkshopPreview,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

const READER_POOL_SIZE: usize = 4;

pub struct ServerRepo {
    writer: Mutex<Connection>,
    readers: Vec<Mutex<Connection>>,
    reader_idx: AtomicUsize,
}

#[derive(Debug, Clone)]
pub struct StoredItemMeta {
    pub record: WorkshopItemRecord,
    pub is_admin_override: bool,
    pub last_verified_at: DateTime<Utc>,
    pub removed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Clone)]
pub struct UpstreamApplySummary {
    pub active_upserted: u64,
    pub tombstoned: u64,
    pub skipped_overrides: u64,
    pub max_time_updated: Option<i64>,
}

impl ServerRepo {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let writer = open_configured_conn(path)?;
        let repo_init = Self {
            writer: Mutex::new(writer),
            readers: Vec::new(),
            reader_idx: AtomicUsize::new(0),
        };
        repo_init.migrate()?;

        let mut readers = Vec::with_capacity(READER_POOL_SIZE);
        for _ in 0..READER_POOL_SIZE {
            let reader = open_configured_conn(path)?;
            let _ = reader.pragma_update(None, "query_only", "ON");
            readers.push(Mutex::new(reader));
        }

        Ok(Self {
            writer: repo_init.writer,
            readers,
            reader_idx: AtomicUsize::new(0),
        })
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        let repo = Self {
            writer: Mutex::new(conn),
            readers: Vec::new(),
            reader_idx: AtomicUsize::new(0),
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

    pub fn with_transaction<T>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut conn = self.lock_writer()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    fn migrate(&self) -> anyhow::Result<()> {
        let conn = self.lock_writer()?;
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
                last_verified_at TEXT NOT NULL
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

        ensure_column(
            &conn,
            "workshop_items",
            "previews_json",
            "TEXT NOT NULL DEFAULT '[]'",
        )?;
        ensure_column(
            &conn,
            "workshop_items",
            "is_admin_override",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        Ok(())
    }

    pub fn get_item(&self, workshop_id: WorkshopId) -> anyhow::Result<Option<StoredItemMeta>> {
        let conn = self.lock_reader()?;
        Self::get_item_with_conn(&conn, workshop_id)
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
                    fetched_at, removed_at, last_verified_at
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
                    last_verified_at: parse_rfc3339_or_now(&verified_str),
                    removed_at: removed_str.as_deref().map(parse_rfc3339_or_now),
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
            "SELECT dep_workshop_id, dep_title, required
             FROM workshop_item_dependencies
             WHERE parent_workshop_id = ?1 AND status = 'active'
             ORDER BY dep_workshop_id ASC",
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
        let conn = self.lock_reader()?;
        let mut found = Vec::new();
        let mut missing = Vec::new();
        for id in ids {
            match Self::get_item_with_conn(&conn, *id)? {
                Some(meta) => found.push(meta),
                None => missing.push(*id),
            }
        }
        Ok((found, missing))
    }

    pub fn get_admin_item_detail(
        &self,
        workshop_id: WorkshopId,
    ) -> anyhow::Result<Option<AdminItemDetailResponse>> {
        let conn = self.lock_reader()?;
        let Some(meta) = Self::get_item_with_conn(&conn, workshop_id)? else {
            return Ok(None);
        };
        let revisions = Self::list_revisions_with_conn(&conn, workshop_id)?;
        Ok(Some(AdminItemDetailResponse {
            item: meta.record,
            is_admin_override: meta.is_admin_override,
            last_verified_at: meta.last_verified_at,
            removed_at: meta.removed_at,
            revisions,
        }))
    }

    pub fn upsert_item(
        &self,
        record: &WorkshopItemRecord,
        is_admin_override: bool,
        replace_deps: bool,
    ) -> anyhow::Result<()> {
        self.with_transaction(|tx| {
            Self::upsert_item_tx(tx, record, is_admin_override, replace_deps, true)
        })
    }

    pub fn upsert_item_tx(
        tx: &Connection,
        record: &WorkshopItemRecord,
        is_admin_override: bool,
        replace_deps: bool,
        record_revision: bool,
    ) -> anyhow::Result<()> {
        let wid = record.workshop_id.as_u64() as i64;
        let now = Utc::now().to_rfc3339();
        let removed_at = match record.status {
            WorkshopItemStatus::Removed | WorkshopItemStatus::PrivateOrUnavailable => {
                Some(now.clone())
            }
            _ => None,
        };

        tx.execute(
            r#"
            INSERT INTO workshop_items (
                workshop_id, file_type, status, title, description, creator_steam_id,
                creator_name, preview_url, previews_json, file_url, file_size,
                time_created, time_updated, tags_json, collection_children_json,
                subscriptions, favorites, is_admin_override, source,
                fetched_at, removed_at, last_verified_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22
            )
            ON CONFLICT(workshop_id) DO UPDATE SET
                file_type = excluded.file_type,
                status = excluded.status,
                title = excluded.title,
                description = COALESCE(excluded.description, workshop_items.description),
                creator_steam_id = COALESCE(excluded.creator_steam_id, workshop_items.creator_steam_id),
                creator_name = COALESCE(excluded.creator_name, workshop_items.creator_name),
                preview_url = COALESCE(excluded.preview_url, workshop_items.preview_url),
                previews_json = CASE WHEN excluded.previews_json = '[]' THEN workshop_items.previews_json ELSE excluded.previews_json END,
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
                last_verified_at = excluded.last_verified_at
            "#,
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
                now,
            ],
        )?;

        if replace_deps {
            tx.execute(
                "UPDATE workshop_item_dependencies SET status = 'removed_upstream', updated_at = ?2 WHERE parent_workshop_id = ?1",
                params![wid, now],
            )?;
        }

        for dep in &record.dependencies {
            let resolved_dep_title: Option<String> = match &dep.title {
                Some(t) => Some(t.clone()),
                None => tx
                    .query_row(
                        "SELECT title FROM workshop_items WHERE workshop_id = ?1",
                        params![dep.workshop_id.as_u64() as i64],
                        |r| r.get(0),
                    )
                    .optional()?,
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
        if !record.title.starts_with("Workshop #") {
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

    /// 将 SteamKit CM 上游拉取结果原子写入缓存库：
    /// - 跳过 `is_admin_override = 1` 的管理员锁定条目；
    /// - 对更新后的组件刷新 `file_url`、依赖与元数据，并在关键字段变化时追加历史修订；
    /// - 对上游变为 `Removed` 或 `PrivateOrUnavailable` 的组件执行墓碑归档（清空失效 `file_url`，完整保留历史标题、简介、封面、画廊与依赖关系）。
    pub fn apply_upstream_outcomes(
        &self,
        outcomes: &[WorkshopFetchOutcome],
    ) -> anyhow::Result<UpstreamApplySummary> {
        if outcomes.is_empty() {
            return Ok(UpstreamApplySummary::default());
        }
        self.with_transaction(|tx| {
            let mut summary = UpstreamApplySummary::default();
            for outcome in outcomes {
                match outcome {
                    WorkshopFetchOutcome::Found { item } => {
                        let existing = Self::get_item_tx(tx, item.workshop_id)?;
                        if existing.as_ref().is_some_and(|e| e.is_admin_override) {
                            summary.skipped_overrides += 1;
                            continue;
                        }
                        let changed = existing.as_ref().is_none_or(|prev| {
                            prev.record.status != item.status
                                || prev.record.title != item.title
                                || prev.record.time_updated != item.time_updated
                                || prev.record.file_url != item.file_url
                                || prev.record.dependencies.len() != item.dependencies.len()
                                || prev
                                    .record
                                    .dependencies
                                    .iter()
                                    .map(|d| d.workshop_id)
                                    .ne(item.dependencies.iter().map(|d| d.workshop_id))
                        });
                        Self::upsert_item_tx(tx, item, false, true, changed)?;
                        summary.active_upserted += 1;
                        if let Some(ts) = item.time_updated {
                            summary.max_time_updated =
                                Some(summary.max_time_updated.map_or(ts, |cur| cur.max(ts)));
                        }
                    }
                    WorkshopFetchOutcome::Removed {
                        workshop_id,
                        source,
                        reason,
                        ..
                    } => {
                        Self::apply_tombstone_tx(
                            tx,
                            *workshop_id,
                            WorkshopItemStatus::Removed,
                            *source,
                            reason.as_deref().unwrap_or("Steam CM 上游标记已下架"),
                            &mut summary,
                        )?;
                    }
                    WorkshopFetchOutcome::PrivateOrUnavailable {
                        workshop_id,
                        source,
                        reason,
                        ..
                    } => {
                        Self::apply_tombstone_tx(
                            tx,
                            *workshop_id,
                            WorkshopItemStatus::PrivateOrUnavailable,
                            *source,
                            reason.as_deref().unwrap_or("Steam CM 上游标记私密或不可用"),
                            &mut summary,
                        )?;
                    }
                }
            }
            Ok(summary)
        })
    }

    fn apply_tombstone_tx(
        tx: &Connection,
        wid: WorkshopId,
        next_status: WorkshopItemStatus,
        source: MetadataSource,
        reason: &str,
        summary: &mut UpstreamApplySummary,
    ) -> anyhow::Result<()> {
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339();
        let existing = Self::get_item_tx(tx, wid)?;
        if let Some(prev) = existing {
            if prev.is_admin_override {
                summary.skipped_overrides += 1;
                return Ok(());
            }
            tx.execute(
                r#"
                UPDATE workshop_items
                SET status = ?2,
                    file_url = NULL,
                    source = ?3,
                    fetched_at = ?4,
                    removed_at = COALESCE(removed_at, ?4),
                    last_verified_at = ?4
                WHERE workshop_id = ?1
                "#,
                params![
                    wid.as_u64() as i64,
                    serialize_json_enum(&next_status),
                    serialize_json_enum(&source),
                    now,
                ],
            )?;
            if prev.record.status != next_status {
                let mut rev_record = prev.record;
                rev_record.status = next_status;
                rev_record.file_url = None;
                rev_record.source = source;
                Self::insert_revision_tx(tx, &rev_record, &now)?;
                Self::append_audit_log_tx(
                    tx,
                    Some(wid),
                    "steamkit-sync",
                    "tombstone_detected",
                    reason,
                    Some(next_status),
                    rev_record.time_updated,
                )?;
            }
            summary.tombstoned += 1;
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

            Self::upsert_item_tx(tx, &next_record, true, replace_deps, true)?;
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

    pub fn search(&self, query: &SearchQuery) -> anyhow::Result<SearchResponse> {
        let conn = self.lock_reader()?;
        let page = query.page.max(1);
        let page_size = query.page_size.clamp(1, 100);
        let offset_u64 = u64::from(page - 1) * u64::from(page_size);
        let offset = i64::try_from(offset_u64).unwrap_or(i64::MAX);

        let q_text = query
            .query
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase);
        let q_filter = q_text
            .as_deref()
            .map(|text| format!("%{}%", escape_like(text)));
        let tag_filter = query
            .tag
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase);
        let status_filter = query.status.map(|s| serialize_json_enum(&s));

        let total_i64: i64 = conn.query_row(
            r#"
            SELECT COUNT(*)
            FROM workshop_items
            WHERE (?1 IS NULL OR LOWER(title) LIKE ?1 ESCAPE '\' OR LOWER(COALESCE(description, '')) LIKE ?1 ESCAPE '\' OR CAST(workshop_id AS TEXT) LIKE ?1 ESCAPE '\')
              AND (?2 IS NULL OR EXISTS (SELECT 1 FROM json_each(tags_json) WHERE LOWER(value) = ?2))
              AND (?3 IS NULL OR status = ?3)
            "#,
            params![q_filter, tag_filter, status_filter],
            |r| r.get(0),
        )?;
        let total = u64::try_from(total_i64).unwrap_or(0);

        let limit = i64::from(page_size);
        let mut page_params: Vec<&dyn rusqlite::ToSql> =
            vec![&q_filter, &tag_filter, &status_filter, &limit, &offset];
        let order_by = match query.sort.as_deref() {
            Some("updated_asc") => "ORDER BY COALESCE(time_updated, 0) ASC, workshop_id ASC",
            Some("title") => "ORDER BY title COLLATE NOCASE ASC, workshop_id ASC",
            Some("subscriptions") => "ORDER BY COALESCE(subscriptions, 0) DESC, workshop_id ASC",
            Some("newest") => "ORDER BY COALESCE(time_created, 0) DESC, workshop_id DESC",
            Some("updated") => "ORDER BY COALESCE(time_updated, 0) DESC, workshop_id DESC",
            _ if q_text.is_some() => {
                page_params.push(&q_text);
                "ORDER BY CASE WHEN LOWER(title) = ?6 THEN 0
                               WHEN SUBSTR(LOWER(title), 1, LENGTH(?6)) = ?6 THEN 1
                               ELSE 2 END,
                          COALESCE(subscriptions, 0) DESC, COALESCE(time_updated, 0) DESC, workshop_id ASC"
            }
            _ => {
                "ORDER BY COALESCE(subscriptions, 0) DESC, COALESCE(time_updated, 0) DESC, workshop_id ASC"
            }
        };
        let page_sql = format!(
            r#"
            SELECT workshop_id
            FROM workshop_items
            WHERE (?1 IS NULL OR LOWER(title) LIKE ?1 ESCAPE '\' OR LOWER(COALESCE(description, '')) LIKE ?1 ESCAPE '\' OR CAST(workshop_id AS TEXT) LIKE ?1 ESCAPE '\')
              AND (?2 IS NULL OR EXISTS (SELECT 1 FROM json_each(tags_json) WHERE LOWER(value) = ?2))
              AND (?3 IS NULL OR status = ?3)
            {order_by}
            LIMIT ?4 OFFSET ?5
            "#
        );

        let mut stmt = conn.prepare(&page_sql)?;
        let page_ids: Vec<WorkshopId> = stmt
            .query_map(page_params.as_slice(), |r| {
                let raw: i64 = r.get(0)?;
                parse_workshop_id_col(0, raw)
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut items = Vec::with_capacity(page_ids.len());
        for id in page_ids {
            if let Some(meta) = Self::get_item_with_conn(&conn, id)? {
                items.push(meta.record);
            }
        }

        Ok(SearchResponse {
            items,
            total,
            page,
            page_size,
            source: MetadataSource::CatalogServer,
        })
    }

    pub fn get_collection(
        &self,
        collection_id: WorkshopId,
    ) -> anyhow::Result<Option<WorkshopCollectionRecord>> {
        let conn = self.lock_reader()?;
        let Some(meta) = Self::get_item_with_conn(&conn, collection_id)? else {
            return Ok(None);
        };
        if meta.record.file_type != WorkshopFileType::Collection {
            return Ok(None);
        }
        let child_ids = meta.record.collection_children.clone();
        let mut items = Vec::new();
        for cid in &child_ids {
            if let Some(child_meta) = Self::get_item_with_conn(&conn, *cid)? {
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
        Ok(conn
            .query_row(
                "SELECT value FROM sync_state WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_sync_state(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let conn = self.lock_writer()?;
        conn.execute(
            "INSERT INTO sync_state (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
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

fn open_configured_conn(path: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    Ok(conn)
}

fn ensure_column(conn: &Connection, table: &str, column: &str, def: &str) -> anyhow::Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !cols.iter().any(|c| c == column) {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {def}"),
            [],
        )?;
    }
    Ok(())
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

fn escape_like(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}
