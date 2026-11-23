use super::metadata::{
    local_cover_for_db, normalize_valid_remote_cover_url, parse_enum, serialize_enum,
};
use super::LocalDb;
use crate::error::CoreError;
use chrono::{DateTime, Utc};
use l4a_protocol::{
    AddonSummary, MetadataSource, WorkshopDependency, WorkshopFileType, WorkshopId,
    WorkshopItemRecord, WorkshopItemStatus,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

impl LocalDb {
    pub fn has_workshop_snapshot(&self, workshop_id: WorkshopId) -> Result<bool, CoreError> {
        let conn = self.lock_conn()?;
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM workshop_snapshots WHERE workshop_id = ?1 LIMIT 1",
                params![workshop_id.as_u64() as i64],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    pub fn upsert_workshop_snapshot(
        &self,
        item: &WorkshopItemRecord,
        replace_deps: bool,
    ) -> Result<(), CoreError> {
        self.write(|tx| Self::upsert_workshop_snapshot_tx(tx, item, replace_deps))
    }

    pub(crate) fn save_workshop_update_tx(
        tx: &Connection,
        addon: &AddonSummary,
        item: &WorkshopItemRecord,
        replace_deps: bool,
    ) -> Result<(), CoreError> {
        Self::upsert_workshop_snapshot_tx(tx, item, replace_deps)?;
        let preview_url = item
            .preview_url
            .as_deref()
            .and_then(normalize_valid_remote_cover_url);
        // Ordinary upserts retain missing fields. A replacement's metadata belongs to the new VPK.
        tx.execute(
            "UPDATE addons SET cover_image = ?2, local_time_updated = ?3 WHERE id = ?1",
            params![
                addon.id.as_str(),
                local_cover_for_db(addon),
                item.time_updated
            ],
        )?;
        tx.execute(
            "UPDATE workshop_snapshots SET preview_url = ?2, time_updated = ?3 WHERE workshop_id = ?1",
            params![item.workshop_id.as_u64() as i64, preview_url, item.time_updated],
        )?;
        Ok(())
    }

    pub(crate) fn upsert_workshop_snapshot_tx(
        tx: &Connection,
        item: &WorkshopItemRecord,
        replace_deps: bool,
    ) -> Result<(), CoreError> {
        let wid = item.workshop_id.as_u64() as i64;
        let now = Utc::now().to_rfc3339();
        let removed_at = matches!(
            item.status,
            WorkshopItemStatus::Removed | WorkshopItemStatus::PrivateOrUnavailable
        )
        .then(|| now.clone());
        let preview_url = item
            .preview_url
            .as_deref()
            .and_then(normalize_valid_remote_cover_url);
        let file_url = item
            .file_url
            .as_deref()
            .map(l4a_workshop::upgrade_steam_cdn_https)
            .filter(|s| !s.is_empty());

        tx.execute(
            r#"
            INSERT INTO workshop_snapshots (
                workshop_id, file_type, status, title, description, creator_steam_id, creator_name,
                preview_url, file_url, file_size, time_created, time_updated, tags_json,
                collection_children_json, source, fetched_at, removed_detected_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
            ON CONFLICT(workshop_id) DO UPDATE SET
                file_type = excluded.file_type,
                status = excluded.status,
                title = CASE WHEN excluded.title = '' THEN workshop_snapshots.title ELSE excluded.title END,
                description = COALESCE(excluded.description, workshop_snapshots.description),
                creator_steam_id = COALESCE(excluded.creator_steam_id, workshop_snapshots.creator_steam_id),
                creator_name = COALESCE(excluded.creator_name, workshop_snapshots.creator_name),
                preview_url = COALESCE(excluded.preview_url, workshop_snapshots.preview_url),
                file_url = COALESCE(excluded.file_url, workshop_snapshots.file_url),
                file_size = COALESCE(excluded.file_size, workshop_snapshots.file_size),
                time_created = COALESCE(excluded.time_created, workshop_snapshots.time_created),
                time_updated = COALESCE(excluded.time_updated, workshop_snapshots.time_updated),
                tags_json = CASE WHEN excluded.tags_json = '[]' THEN workshop_snapshots.tags_json ELSE excluded.tags_json END,
                collection_children_json = CASE WHEN excluded.collection_children_json = '[]' THEN workshop_snapshots.collection_children_json ELSE excluded.collection_children_json END,
                source = excluded.source,
                fetched_at = excluded.fetched_at,
                removed_detected_at = CASE WHEN excluded.status = 'active' THEN NULL ELSE COALESCE(workshop_snapshots.removed_detected_at, excluded.removed_detected_at) END
            "#,
            params![
                wid,
                serialize_enum(&item.file_type),
                serialize_enum(&item.status),
                item.title,
                item.description,
                item.creator_steam_id,
                item.creator_name,
                preview_url,
                file_url,
                item.file_size.map(|v| v as i64),
                item.time_created,
                item.time_updated,
                serde_json::to_string(&item.tags).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(&item.collection_children).unwrap_or_else(|_| "[]".into()),
                serialize_enum(&item.source),
                item.fetched_at.to_rfc3339(),
                removed_at,
            ],
        )?;

        if replace_deps {
            tx.execute(
                "UPDATE workshop_dependencies SET status = 'removed_upstream', updated_at = ?2 WHERE parent_workshop_id = ?1",
                params![wid, now],
            )?;
            for dep in &item.dependencies {
                tx.execute(
                    r#"
                    INSERT INTO workshop_dependencies (
                        parent_workshop_id, dep_workshop_id, dep_title, required, status, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, 'active', ?5)
                    ON CONFLICT(parent_workshop_id, dep_workshop_id) DO UPDATE SET
                        dep_title = COALESCE(excluded.dep_title, workshop_dependencies.dep_title),
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
        }

        Ok(())
    }

    pub fn mark_workshop_status(
        &self,
        workshop_id: WorkshopId,
        status: WorkshopItemStatus,
        source: MetadataSource,
    ) -> Result<(), CoreError> {
        let conn = self.lock_conn()?;
        let now = Utc::now().to_rfc3339();
        let wid = workshop_id.as_u64() as i64;
        let exists: Option<i64> = conn
            .query_row(
                "SELECT workshop_id FROM workshop_snapshots WHERE workshop_id = ?1",
                params![wid],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_some() {
            conn.execute(
                "UPDATE workshop_snapshots SET status = ?2, source = ?3, fetched_at = ?4, removed_detected_at = COALESCE(removed_detected_at, ?4) WHERE workshop_id = ?1",
                params![wid, serialize_enum(&status), serialize_enum(&source), now],
            )?;
        } else {
            conn.execute(
                "INSERT INTO workshop_snapshots (workshop_id, file_type, status, title, source, fetched_at, removed_detected_at) VALUES (?1, 'item', ?2, ?3, ?4, ?5, ?5)",
                params![wid, serialize_enum(&status), format!("Workshop #{workshop_id}"), serialize_enum(&source), now],
            )?;
        }
        Ok(())
    }

    pub fn list_workshop_snapshots(
        &self,
    ) -> Result<HashMap<WorkshopId, WorkshopItemRecord>, CoreError> {
        let conn = self.lock_conn()?;
        Self::load_snapshots_map(&conn)
    }

    pub fn get_workshop_snapshot(
        &self,
        wid: WorkshopId,
    ) -> Result<Option<WorkshopItemRecord>, CoreError> {
        let conn = self.lock_conn()?;
        let mut map = Self::load_snapshots_map(&conn)?;
        Ok(map.remove(&wid))
    }

    pub fn list_active_dependencies(
        &self,
        parent: WorkshopId,
    ) -> Result<Vec<WorkshopDependency>, CoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT dep_workshop_id, dep_title, required FROM workshop_dependencies WHERE parent_workshop_id = ?1 AND status = 'active' ORDER BY dep_workshop_id ASC",
        )?;
        let rows = stmt.query_map(params![parent.as_u64() as i64], |r| {
            let raw: i64 = r.get(0)?;
            Ok(WorkshopDependency {
                workshop_id: WorkshopId::new(raw as u64).unwrap_or(WorkshopId(1)),
                title: r.get(1)?,
                required: r.get::<_, i64>(2)? != 0,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn list_all_active_dependency_edges(
        &self,
    ) -> Result<Vec<(WorkshopId, WorkshopId)>, CoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT parent_workshop_id, dep_workshop_id FROM workshop_dependencies WHERE status = 'active'",
        )?;
        let rows = stmt.query_map([], |r| {
            let p: i64 = r.get(0)?;
            let d: i64 = r.get(1)?;
            Ok((
                WorkshopId::new(p as u64).unwrap_or(WorkshopId(1)),
                WorkshopId::new(d as u64).unwrap_or(WorkshopId(1)),
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub(super) fn load_snapshots_map(
        conn: &Connection,
    ) -> Result<HashMap<WorkshopId, WorkshopItemRecord>, CoreError> {
        let mut stmt = conn.prepare(
            "SELECT workshop_id, file_type, status, title, description, creator_steam_id, creator_name,
                    preview_url, file_url, file_size, time_created, time_updated, tags_json,
                    collection_children_json, source, fetched_at
             FROM workshop_snapshots",
        )?;
        let mut map = HashMap::new();
        let rows = stmt.query_map([], |r| {
            let wid_raw: i64 = r.get(0)?;
            let wid = WorkshopId::new(wid_raw as u64).unwrap_or(WorkshopId(1));
            let ft: String = r.get(1)?;
            let st: String = r.get(2)?;
            let tags_json: String = r.get(12)?;
            let children_json: String = r.get(13)?;
            let src: String = r.get(14)?;
            let fetched: String = r.get(15)?;
            Ok(WorkshopItemRecord {
                workshop_id: wid,
                file_type: parse_enum(&ft, WorkshopFileType::Item),
                status: parse_enum(&st, WorkshopItemStatus::Unknown),
                title: r.get(3)?,
                description: r.get(4)?,
                creator_steam_id: r.get(5)?,
                creator_name: r.get(6)?,
                preview_url: r.get(7)?,
                previews: Vec::new(),
                file_url: r.get(8)?,
                file_size: r.get::<_, Option<i64>>(9)?.map(|v| v as u64),
                time_created: r.get(10)?,
                time_updated: r.get(11)?,
                tags: serde_json::from_str(&tags_json).unwrap_or_default(),
                dependencies: Vec::new(),
                collection_children: serde_json::from_str(&children_json).unwrap_or_default(),
                subscriptions: None,
                favorites: None,
                source: parse_enum(&src, MetadataSource::SteamWebApi),
                fetched_at: DateTime::parse_from_rfc3339(&fetched)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?;
        for row in rows {
            let item = row?;
            map.insert(item.workshop_id, item);
        }

        let mut dep_stmt = conn.prepare(
            "SELECT parent_workshop_id, dep_workshop_id, dep_title, required FROM workshop_dependencies WHERE status = 'active'",
        )?;
        let dep_rows = dep_stmt.query_map([], |r| {
            let p: i64 = r.get(0)?;
            let d: i64 = r.get(1)?;
            Ok((
                WorkshopId::new(p as u64).unwrap_or(WorkshopId(1)),
                WorkshopDependency {
                    workshop_id: WorkshopId::new(d as u64).unwrap_or(WorkshopId(1)),
                    title: r.get(2)?,
                    required: r.get::<_, i64>(3)? != 0,
                },
            ))
        })?;
        for row in dep_rows {
            let (parent, dep) = row?;
            if let Some(item) = map.get_mut(&parent) {
                item.dependencies.push(dep);
            }
        }
        Ok(map)
    }
}
