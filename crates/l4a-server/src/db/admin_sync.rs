use super::{parse_json_enum, parse_rfc3339_or_now, serialize_json_enum, ServerRepo};
use chrono::Utc;
use l4a_protocol::{
    AdminItemOverrideRequest, AuditLogEntry, CatalogStats, MetadataSource, WorkshopFileType,
    WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

impl ServerRepo {
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

            Self::upsert_item_tx(
                tx,
                &next_record,
                true,
                replace_deps,
                true,
                None,
                None,
                None,
                Some(true),
                None,
            )?;
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
        let wid_i64 = super::workshop_id_to_i64(wid)?;
        self.with_transaction(|tx| {
            let changed = tx.execute(
                "UPDATE workshop_items SET is_admin_override = 0, source = 'catalog_server' WHERE workshop_id = ?1",
                params![wid_i64],
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
        let wid_i64 = workshop_id.map(super::workshop_id_to_i64).transpose()?;
        tx.execute(
            r#"
            INSERT INTO audit_log (
                workshop_id, actor, action, reason, status, time_updated, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                wid_i64,
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

    pub fn get_sync_state(&self, key: &str) -> anyhow::Result<Option<String>> {
        let conn = self.lock_reader()?;
        Self::get_sync_state_with_conn(&conn, key)
    }

    pub(super) fn get_sync_state_with_conn(
        conn: &Connection,
        key: &str,
    ) -> anyhow::Result<Option<String>> {
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

    pub(super) fn set_sync_states_tx(
        tx: &Transaction<'_>,
        entries: &[(&str, &str)],
    ) -> anyhow::Result<()> {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO sync_state (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )?;
        for (key, value) in entries {
            stmt.execute(params![key, value])?;
        }
        Ok(())
    }

    pub fn total_items_count(&self) -> anyhow::Result<u64> {
        let conn = self.lock_reader()?;
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM workshop_items", [], |r| r.get(0))?;
        Ok(n.max(0) as u64)
    }

    pub fn stats(&self) -> anyhow::Result<CatalogStats> {
        self.with_read_snapshot(|conn| {
            let (
                total_items,
                active_items,
                removed_items,
                private_items,
                collections_count,
                overridden_items,
            ) = conn.query_row(
                "SELECT
                    COUNT(*),
                    COALESCE(SUM(CASE WHEN status = 'active' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status = 'removed' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status = 'private_or_unavailable' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN file_type = 'collection' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN is_admin_override = 1 THEN 1 ELSE 0 END), 0)
                 FROM workshop_items",
                [],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?.max(0) as u64,
                        r.get::<_, i64>(1)?.max(0) as u64,
                        r.get::<_, i64>(2)?.max(0) as u64,
                        r.get::<_, i64>(3)?.max(0) as u64,
                        r.get::<_, i64>(4)?.max(0) as u64,
                        r.get::<_, i64>(5)?.max(0) as u64,
                    ))
                },
            )?;
            let revisions_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM workshop_item_revisions", [], |r| {
                    r.get(0)
                })?;
            let audit_logs_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM audit_log", [], |r| r.get(0))?;
            Ok(CatalogStats {
                total_items,
                active_items,
                removed_items,
                private_items,
                collections_count,
                overridden_items,
                revisions_count: revisions_count.max(0) as u64,
                audit_logs_count: audit_logs_count.max(0) as u64,
            })
        })
    }
}
