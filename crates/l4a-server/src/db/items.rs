use super::{
    parse_json_enum, parse_rfc3339_or_now, parse_workshop_id_col, serialize_json_enum,
    workshop_id_to_i64, OverlayItemMeta, ServerRepo, StoredItemMeta,
};
use chrono::{DateTime, Utc};
use l4a_protocol::{
    AdminItemDetailResponse, ItemRevisionEntry, MetadataSource, WorkshopCollectionRecord,
    WorkshopDependency, WorkshopFileType, WorkshopId, WorkshopItemRecord, WorkshopItemStatus,
    WorkshopPreview,
};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};

impl ServerRepo {
    pub fn get_item(&self, workshop_id: WorkshopId) -> anyhow::Result<Option<StoredItemMeta>> {
        self.with_read_snapshot(|snapshot| Self::get_item_with_conn(snapshot, workshop_id))
    }

    pub fn get_item_tx(
        tx: &Connection,
        workshop_id: WorkshopId,
    ) -> anyhow::Result<Option<StoredItemMeta>> {
        Self::get_item_with_conn(tx, workshop_id)
    }

    pub(super) fn get_item_with_conn(
        conn: &Connection,
        workshop_id: WorkshopId,
    ) -> anyhow::Result<Option<StoredItemMeta>> {
        let Ok(wid_i64) = i64::try_from(workshop_id.as_u64()) else {
            return Ok(None);
        };
        let mut stmt = conn.prepare_cached(
            "SELECT file_type, status, title, description, creator_steam_id,
                    creator_name, preview_url, previews_json, file_url, file_size,
                    time_created, time_updated, tags_json, collection_children_json,
                    subscriptions, favorites, is_admin_override, source,
                    fetched_at, removed_at, last_verified_at, steam_version,
                    catalog_indexed, is_listed,
                    COALESCE(visibility_verified_at, last_verified_at)
             FROM workshop_items WHERE workshop_id = ?1",
        )?;

        let row = stmt
            .query_row(params![wid_i64], |r| {
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
                let vis_verified_str: String = r.get(24)?;

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
                    visibility_verified_at: vis_verified_str
                        .parse()
                        .unwrap_or(DateTime::UNIX_EPOCH),
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
        let Ok(parent_i64) = i64::try_from(parent_id.as_u64()) else {
            return Ok(Vec::new());
        };
        let mut stmt = conn.prepare_cached(
            "SELECT edge.dep_workshop_id,
                    CASE WHEN dep.status = 'private_or_unavailable' THEN NULL ELSE edge.dep_title END,
                    edge.required
             FROM workshop_item_dependencies edge
             LEFT JOIN workshop_items dep ON dep.workshop_id = edge.dep_workshop_id
             WHERE edge.parent_workshop_id = ?1 AND edge.status = 'active'
             ORDER BY edge.dep_workshop_id ASC",
        )?;
        let rows = stmt.query_map(params![parent_i64], |r| {
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

    /// Lightweight batched lookup for hybrid search local overlays; full records are loaded only for admin overrides.
    pub(crate) fn get_overlay_batch(
        &self,
        ids: &[WorkshopId],
    ) -> anyhow::Result<HashMap<WorkshopId, OverlayItemMeta>> {
        let mut seen = HashSet::with_capacity(ids.len());
        let unique_i64: Vec<i64> = ids
            .iter()
            .filter(|id| seen.insert(**id))
            .filter_map(|id| i64::try_from(id.as_u64()).ok())
            .collect();
        if unique_i64.is_empty() {
            return Ok(HashMap::new());
        }

        self.with_read_snapshot(|snapshot| {
            let mut out = HashMap::with_capacity(unique_i64.len());
            let mut override_ids = Vec::new();
            for chunk in unique_i64.chunks(500) {
                let placeholders = vec!["?"; chunk.len()].join(", ");
                let sql = format!(
                    "SELECT workshop_id, status, is_admin_override,
                            COALESCE(visibility_verified_at, last_verified_at),
                            steam_version, catalog_indexed, is_listed, creator_name
                     FROM workshop_items
                     WHERE workshop_id IN ({placeholders})"
                );
                let mut stmt = snapshot.prepare(&sql)?;
                let rows = stmt.query_map(params_from_iter(chunk.iter().copied()), |r| {
                    let wid = parse_workshop_id_col(0, r.get(0)?)?;
                    let status_str: String = r.get(1)?;
                    let is_override: i64 = r.get(2)?;
                    let verified_str: String = r.get(3)?;
                    let steam_version: Option<i64> = r.get(4)?;
                    let catalog_indexed: i64 = r.get(5)?;
                    let is_listed: Option<i64> = r.get(6)?;
                    let creator_name: Option<String> = r.get(7)?;
                    Ok((
                        wid,
                        OverlayItemMeta {
                            status: parse_json_enum(&status_str, WorkshopItemStatus::Unknown),
                            is_admin_override: is_override != 0,
                            visibility_verified_at: verified_str
                                .parse()
                                .unwrap_or(DateTime::UNIX_EPOCH),
                            steam_version,
                            catalog_indexed: catalog_indexed != 0,
                            is_listed: is_listed.map(|v| v != 0),
                            creator_name,
                            override_record: None,
                        },
                    ))
                })?;
                for row in rows {
                    let (wid, meta) = row?;
                    if meta.is_admin_override {
                        override_ids.push(wid);
                    }
                    out.insert(wid, meta);
                }
            }
            for wid in override_ids {
                if let Some(full) = Self::get_item_with_conn(snapshot, wid)? {
                    if let Some(meta) = out.get_mut(&wid) {
                        meta.override_record = Some(full.record);
                    }
                }
            }
            Ok(out)
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
                (!is_admin_override).then_some(record.fetched_at),
                (!is_admin_override)
                    .then_some(record.time_updated)
                    .flatten(),
                Some(true),
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn upsert_item_tx(
        tx: &Connection,
        record: &WorkshopItemRecord,
        is_admin_override: bool,
        replace_deps: bool,
        record_revision: bool,
        listed: Option<bool>,
        visibility_verified_at: Option<DateTime<Utc>>,
        steam_version: Option<i64>,
        catalog_indexed: Option<bool>,
    ) -> anyhow::Result<()> {
        let wid = workshop_id_to_i64(record.workshop_id)?;
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
                fetched_at, removed_at, last_verified_at, visibility_verified_at,
                is_listed, steam_version, catalog_indexed
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?20, COALESCE(?25, ?20),
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
                visibility_verified_at = COALESCE(?25, workshop_items.visibility_verified_at, excluded.last_verified_at),
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
                steam_version,
                catalog_indexed,
                visibility_verified_at.map(|dt| dt.to_rfc3339()),
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
            let dep_wid = workshop_id_to_i64(dep.workshop_id)?;
            let dep_meta = tx
                .query_row(
                    "SELECT title, status FROM workshop_items WHERE workshop_id = ?1",
                    params![dep_wid],
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
                    dep_wid,
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

    pub(super) fn insert_revision_tx(
        tx: &Connection,
        record: &WorkshopItemRecord,
        recorded_at: &str,
    ) -> anyhow::Result<()> {
        let wid = workshop_id_to_i64(record.workshop_id)?;
        tx.execute(
            r#"
            INSERT INTO workshop_item_revisions (
                workshop_id, status, title, time_updated, dependencies_json, source, recorded_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                wid,
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
        let Ok(wid) = i64::try_from(workshop_id.as_u64()) else {
            return Ok(Vec::new());
        };
        let mut stmt = conn.prepare(
            "SELECT id, status, title, time_updated, dependencies_json, source, recorded_at
             FROM workshop_item_revisions
             WHERE workshop_id = ?1
             ORDER BY id DESC
             LIMIT 100",
        )?;
        let rows = stmt.query_map(params![wid], |r| {
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
        let Ok(parent_i64) = i64::try_from(parent_id.as_u64()) else {
            return Ok(Vec::new());
        };
        let conn = self.lock_reader()?;
        let mut stmt = conn.prepare(
            "SELECT dep_workshop_id FROM workshop_item_dependencies WHERE parent_workshop_id = ?1 AND status = 'removed_upstream'",
        )?;
        let rows = stmt.query_map(params![parent_i64], |r| {
            let raw: i64 = r.get(0)?;
            parse_workshop_id_col(0, raw)
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

pub(super) fn dependency_requirements(
    dependencies: &[WorkshopDependency],
) -> HashMap<WorkshopId, bool> {
    dependencies
        .iter()
        .map(|dep| (dep.workshop_id, dep.required))
        .collect()
}
