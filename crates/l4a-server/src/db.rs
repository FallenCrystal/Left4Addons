use chrono::{DateTime, Utc};
use l4a_protocol::{
    AuditLogEntry, CatalogStats, ConflictDecision, ContributorId, ContributorSummary,
    MetadataSource, SearchQuery, SearchResponse, TrustTier, WorkshopCollectionRecord,
    WorkshopDependency, WorkshopFileType, WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

pub struct ServerRepo {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone)]
pub struct StoredItemMeta {
    pub record: WorkshopItemRecord,
    pub trust_score: i32,
}

#[derive(Debug, Clone)]
pub struct ContributorAuthRecord {
    pub id: ContributorId,
    pub name: String,
    pub trust_tier: TrustTier,
    pub trust_weight: i32,
    pub enabled: bool,
}

impl ServerRepo {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        let repo = Self {
            conn: Mutex::new(conn),
        };
        repo.migrate()?;
        Ok(repo)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        let repo = Self {
            conn: Mutex::new(conn),
        };
        repo.migrate()?;
        Ok(repo)
    }

    fn lock_conn(&self) -> anyhow::Result<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow::anyhow!("数据库锁状态异常"))
    }

    pub fn with_transaction<T>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    fn migrate(&self) -> anyhow::Result<()> {
        let conn = self.lock_conn()?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS workshop_items (
                workshop_id INTEGER PRIMARY KEY,
                file_type TEXT NOT NULL,
                status TEXT NOT NULL,
                title TEXT NOT NULL,
                description TEXT,
                creator_steam_id TEXT,
                creator_name TEXT,
                preview_url TEXT,
                file_url TEXT,
                file_size INTEGER,
                time_created INTEGER,
                time_updated INTEGER,
                tags_json TEXT NOT NULL DEFAULT '[]',
                collection_children_json TEXT NOT NULL DEFAULT '[]',
                subscriptions INTEGER,
                favorites INTEGER,
                trust_score INTEGER NOT NULL DEFAULT 20,
                source TEXT NOT NULL,
                fetched_at TEXT NOT NULL,
                removed_at TEXT,
                last_verified_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_workshop_items_status_updated ON workshop_items(status, time_updated DESC);
            CREATE INDEX IF NOT EXISTS idx_workshop_items_file_type ON workshop_items(file_type);

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
                trust_score INTEGER NOT NULL,
                recorded_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_revisions_wid ON workshop_item_revisions(workshop_id, id DESC);

            CREATE TABLE IF NOT EXISTS contributors (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                steam_id TEXT,
                token_hash TEXT NOT NULL UNIQUE,
                trust_tier TEXT NOT NULL,
                trust_weight INTEGER NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                last_seen_at TEXT
            );

            CREATE TABLE IF NOT EXISTS submission_audit_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                workshop_id INTEGER NOT NULL,
                contributor_id TEXT,
                contributor_name TEXT,
                decision TEXT NOT NULL,
                reason TEXT NOT NULL,
                submitted_status TEXT NOT NULL,
                submitted_time_updated INTEGER,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_audit_log_wid ON submission_audit_log(workshop_id, id DESC);
            CREATE INDEX IF NOT EXISTS idx_audit_log_decision ON submission_audit_log(decision);

            CREATE TABLE IF NOT EXISTS scraper_jobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                workshop_id INTEGER,
                job_kind TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'queued',
                created_at TEXT NOT NULL
            );
            "#,
        )?;
        Ok(())
    }

    pub fn get_item(&self, workshop_id: WorkshopId) -> anyhow::Result<Option<StoredItemMeta>> {
        let conn = self.lock_conn()?;
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
            "SELECT workshop_id, file_type, status, title, description, creator_steam_id,
                    creator_name, preview_url, file_url, file_size, time_created, time_updated,
                    tags_json, collection_children_json, subscriptions, favorites,
                    trust_score, source, fetched_at
             FROM workshop_items WHERE workshop_id = ?1",
        )?;

        let row = stmt
            .query_row(params![workshop_id.as_u64() as i64], |r| {
                let tags_json: String = r.get(12)?;
                let children_json: String = r.get(13)?;
                let file_type_str: String = r.get(1)?;
                let status_str: String = r.get(2)?;
                let source_str: String = r.get(17)?;
                let fetched_str: String = r.get(18)?;
                let trust_score: i32 = r.get(16)?;

                let record = WorkshopItemRecord {
                    workshop_id,
                    file_type: parse_json_enum(&file_type_str, WorkshopFileType::Item),
                    status: parse_json_enum(&status_str, WorkshopItemStatus::Unknown),
                    title: r.get(3)?,
                    description: r.get(4)?,
                    creator_steam_id: r.get(5)?,
                    creator_name: r.get(6)?,
                    preview_url: r.get(7)?,
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
                    fetched_at: DateTime::parse_from_rfc3339(&fetched_str)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                };
                Ok(StoredItemMeta {
                    record,
                    trust_score,
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
    ) -> anyhow::Result<(Vec<WorkshopItemRecord>, Vec<WorkshopId>)> {
        let conn = self.lock_conn()?;
        let mut items = Vec::new();
        let mut missing = Vec::new();
        for id in ids {
            match Self::get_item_with_conn(&conn, *id)? {
                Some(meta) => items.push(meta.record),
                None => missing.push(*id),
            }
        }
        Ok((items, missing))
    }

    pub fn upsert_item(
        &self,
        record: &WorkshopItemRecord,
        trust_score: i32,
        replace_deps: bool,
    ) -> anyhow::Result<()> {
        self.with_transaction(|tx| Self::upsert_item_tx(tx, record, trust_score, replace_deps))
    }

    pub fn upsert_item_tx(
        tx: &Connection,
        record: &WorkshopItemRecord,
        trust_score: i32,
        replace_deps: bool,
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
                creator_name, preview_url, file_url, file_size, time_created, time_updated,
                tags_json, collection_children_json, subscriptions, favorites,
                trust_score, source, fetched_at, removed_at, last_verified_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
            ON CONFLICT(workshop_id) DO UPDATE SET
                file_type = excluded.file_type,
                status = excluded.status,
                title = excluded.title,
                description = COALESCE(excluded.description, workshop_items.description),
                creator_steam_id = COALESCE(excluded.creator_steam_id, workshop_items.creator_steam_id),
                creator_name = COALESCE(excluded.creator_name, workshop_items.creator_name),
                preview_url = COALESCE(excluded.preview_url, workshop_items.preview_url),
                file_url = COALESCE(excluded.file_url, workshop_items.file_url),
                file_size = COALESCE(excluded.file_size, workshop_items.file_size),
                time_created = COALESCE(excluded.time_created, workshop_items.time_created),
                time_updated = COALESCE(excluded.time_updated, workshop_items.time_updated),
                tags_json = CASE WHEN excluded.tags_json = '[]' THEN workshop_items.tags_json ELSE excluded.tags_json END,
                collection_children_json = CASE WHEN excluded.collection_children_json = '[]' THEN workshop_items.collection_children_json ELSE excluded.collection_children_json END,
                subscriptions = COALESCE(excluded.subscriptions, workshop_items.subscriptions),
                favorites = COALESCE(excluded.favorites, workshop_items.favorites),
                trust_score = excluded.trust_score,
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
                record.file_url,
                record.file_size.map(|v| v as i64),
                record.time_created,
                record.time_updated,
                serde_json::to_string(&record.tags)?,
                serde_json::to_string(&record.collection_children)?,
                record.subscriptions.map(|v| v as i64),
                record.favorites.map(|v| v as i64),
                trust_score,
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
                    dep.title,
                    if dep.required { 1 } else { 0 },
                    now,
                ],
            )?;
        }

        tx.execute(
            r#"
            INSERT INTO workshop_item_revisions (
                workshop_id, status, title, time_updated, dependencies_json, source, trust_score, recorded_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                wid,
                serialize_json_enum(&record.status),
                record.title,
                record.time_updated,
                serde_json::to_string(&record.dependencies)?,
                serialize_json_enum(&record.source),
                trust_score,
                now,
            ],
        )?;

        Ok(())
    }

    pub fn search(&self, query: &SearchQuery) -> anyhow::Result<SearchResponse> {
        let conn = self.lock_conn()?;
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
            _ => "ORDER BY COALESCE(subscriptions, 0) DESC, COALESCE(time_updated, 0) DESC, workshop_id ASC",
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
        let conn = self.lock_conn()?;
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

    pub fn create_contributor(
        &self,
        id: &ContributorId,
        name: &str,
        steam_id: Option<&str>,
        token_hash: &str,
        trust_tier: TrustTier,
        trust_weight: i32,
    ) -> anyhow::Result<ContributorSummary> {
        let conn = self.lock_conn()?;
        let now = Utc::now();
        conn.execute(
            r#"
            INSERT INTO contributors (id, name, steam_id, token_hash, trust_tier, trust_weight, enabled, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)
            "#,
            params![
                id.as_str(),
                name,
                steam_id,
                token_hash,
                serialize_json_enum(&trust_tier),
                trust_weight,
                now.to_rfc3339(),
            ],
        )?;

        Ok(ContributorSummary {
            id: id.clone(),
            name: name.to_string(),
            steam_id: steam_id.map(ToString::to_string),
            trust_tier,
            trust_weight,
            enabled: true,
            created_at: now,
            last_seen_at: None,
        })
    }

    pub fn find_contributor_by_token_hash(
        &self,
        token_hash: &str,
    ) -> anyhow::Result<Option<ContributorAuthRecord>> {
        self.with_transaction(|tx| Self::find_contributor_tx(tx, token_hash))
    }

    pub(crate) fn find_contributor_tx(
        conn: &Connection,
        token_hash: &str,
    ) -> anyhow::Result<Option<ContributorAuthRecord>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, trust_tier, trust_weight, enabled FROM contributors WHERE token_hash = ?1",
        )?;
        let row = stmt
            .query_row(params![token_hash], |r| {
                let id_str: String = r.get(0)?;
                let tier_str: String = r.get(2)?;
                Ok(ContributorAuthRecord {
                    id: ContributorId(id_str),
                    name: r.get(1)?,
                    trust_tier: parse_json_enum(&tier_str, TrustTier::Community),
                    trust_weight: r.get(3)?,
                    enabled: r.get::<_, i64>(4)? != 0,
                })
            })
            .optional()?;
        if let Some(ref c) = row {
            if c.enabled {
                conn.execute(
                    "UPDATE contributors SET last_seen_at = ?2 WHERE id = ?1",
                    params![c.id.as_str(), Utc::now().to_rfc3339()],
                )?;
            }
        }
        Ok(row)
    }

    pub fn list_contributors(&self) -> anyhow::Result<Vec<ContributorSummary>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, steam_id, trust_tier, trust_weight, enabled, created_at, last_seen_at
             FROM contributors ORDER BY trust_weight DESC, created_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_contributor_summary)?;
        let mut out = Vec::new();
        for c in rows {
            out.push(c?);
        }
        Ok(out)
    }

    pub fn update_contributor(
        &self,
        id: &ContributorId,
        trust_tier: Option<TrustTier>,
        trust_weight: Option<i32>,
        enabled: Option<bool>,
    ) -> anyhow::Result<Option<ContributorSummary>> {
        self.with_transaction(|tx| {
            let exists: Option<String> = tx
                .query_row(
                    "SELECT id FROM contributors WHERE id = ?1",
                    params![id.as_str()],
                    |r| r.get(0),
                )
                .optional()?;
            if exists.is_none() {
                return Ok(None);
            }

            if let Some(tier) = trust_tier {
                tx.execute(
                    "UPDATE contributors SET trust_tier = ?2 WHERE id = ?1",
                    params![id.as_str(), serialize_json_enum(&tier)],
                )?;
                if trust_weight.is_none() {
                    tx.execute(
                        "UPDATE contributors SET trust_weight = ?2 WHERE id = ?1",
                        params![id.as_str(), tier.default_weight()],
                    )?;
                }
            }
            if let Some(w) = trust_weight {
                tx.execute(
                    "UPDATE contributors SET trust_weight = ?2 WHERE id = ?1",
                    params![id.as_str(), w],
                )?;
            }
            if let Some(en) = enabled {
                tx.execute(
                    "UPDATE contributors SET enabled = ?2 WHERE id = ?1",
                    params![id.as_str(), if en { 1 } else { 0 }],
                )?;
            }

            let updated = tx
                .query_row(
                    "SELECT id, name, steam_id, trust_tier, trust_weight, enabled, created_at, last_seen_at
                     FROM contributors WHERE id = ?1",
                    params![id.as_str()],
                    row_to_contributor_summary,
                )
                .optional()?;
            Ok(updated)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append_audit_log(
        &self,
        workshop_id: WorkshopId,
        contributor_id: Option<&ContributorId>,
        contributor_name: Option<&str>,
        decision: ConflictDecision,
        reason: &str,
        submitted_status: WorkshopItemStatus,
        submitted_time_updated: Option<i64>,
    ) -> anyhow::Result<()> {
        let conn = self.lock_conn()?;
        Self::append_audit_log_tx(
            &conn,
            workshop_id,
            contributor_id,
            contributor_name,
            decision,
            reason,
            submitted_status,
            submitted_time_updated,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append_audit_log_tx(
        tx: &Connection,
        workshop_id: WorkshopId,
        contributor_id: Option<&ContributorId>,
        contributor_name: Option<&str>,
        decision: ConflictDecision,
        reason: &str,
        submitted_status: WorkshopItemStatus,
        submitted_time_updated: Option<i64>,
    ) -> anyhow::Result<()> {
        tx.execute(
            r#"
            INSERT INTO submission_audit_log (
                workshop_id, contributor_id, contributor_name, decision, reason,
                submitted_status, submitted_time_updated, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                workshop_id.as_u64() as i64,
                contributor_id.map(ContributorId::as_str),
                contributor_name,
                serialize_json_enum(&decision),
                reason,
                serialize_json_enum(&submitted_status),
                submitted_time_updated,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_audit_logs(&self, limit: usize) -> anyhow::Result<Vec<AuditLogEntry>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, workshop_id, contributor_id, contributor_name, decision, reason,
                    submitted_status, submitted_time_updated, created_at
             FROM submission_audit_log ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit.clamp(1, 500) as i64], |r| {
            let wid_raw: i64 = r.get(1)?;
            let cid: Option<String> = r.get(2)?;
            let decision_str: String = r.get(4)?;
            let status_str: String = r.get(6)?;
            let created_str: String = r.get(8)?;
            Ok(AuditLogEntry {
                id: r.get(0)?,
                workshop_id: parse_workshop_id_col(1, wid_raw)?,
                contributor_id: cid.and_then(|s| ContributorId::new(s).ok()),
                contributor_name: r.get(3)?,
                decision: parse_json_enum(&decision_str, ConflictDecision::Accepted),
                reason: r.get(5)?,
                submitted_status: parse_json_enum(&status_str, WorkshopItemStatus::Unknown),
                submitted_time_updated: r.get(7)?,
                created_at: DateTime::parse_from_rfc3339(&created_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
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
    ) -> anyhow::Result<Vec<(String, Option<i64>, String)>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT status, time_updated, dependencies_json FROM workshop_item_revisions WHERE workshop_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![workshop_id.as_u64() as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn list_removed_deps(&self, parent_id: WorkshopId) -> anyhow::Result<Vec<WorkshopId>> {
        let conn = self.lock_conn()?;
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

    pub fn stats(&self) -> anyhow::Result<CatalogStats> {
        let conn = self.lock_conn()?;
        let count = |sql: &str| -> anyhow::Result<u64> {
            let v: i64 = conn.query_row(sql, [], |r| r.get(0))?;
            Ok(v as u64)
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
            contributors_count: count("SELECT COUNT(*) FROM contributors")?,
            flagged_audits_count: count(
                "SELECT COUNT(*) FROM submission_audit_log WHERE decision = 'flagged_for_review'",
            )?,
            queued_scraper_jobs: count(
                "SELECT COUNT(*) FROM scraper_jobs WHERE status = 'queued'",
            )?,
        })
    }
}

fn row_to_contributor_summary(r: &Row<'_>) -> rusqlite::Result<ContributorSummary> {
    let id_str: String = r.get(0)?;
    let tier_str: String = r.get(3)?;
    let created_str: String = r.get(6)?;
    let last_seen_str: Option<String> = r.get(7)?;
    Ok(ContributorSummary {
        id: ContributorId(id_str),
        name: r.get(1)?,
        steam_id: r.get(2)?,
        trust_tier: parse_json_enum(&tier_str, TrustTier::Community),
        trust_weight: r.get(4)?,
        enabled: r.get::<_, i64>(5)? != 0,
        created_at: DateTime::parse_from_rfc3339(&created_str)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        last_seen_at: last_seen_str.and_then(|s| {
            DateTime::parse_from_rfc3339(&s)
                .map(|d| d.with_timezone(&Utc))
                .ok()
        }),
    })
}

fn parse_workshop_id_col(idx: usize, raw: i64) -> rusqlite::Result<WorkshopId> {
    u64::try_from(raw)
        .ok()
        .and_then(|v| WorkshopId::new(v).ok())
        .ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                idx,
                rusqlite::types::Type::Integer,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid workshop_id column value: {raw}"),
                )),
            )
        })
}

fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

fn serialize_json_enum<T: serde::Serialize>(val: &T) -> String {
    serde_json::to_string(val)
        .unwrap_or_default()
        .trim_matches('"')
        .to_string()
}

fn parse_json_enum<T: serde::de::DeserializeOwned>(raw: &str, fallback: T) -> T {
    serde_json::from_value(serde_json::Value::String(raw.to_string())).unwrap_or(fallback)
}
