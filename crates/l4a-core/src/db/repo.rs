use crate::db::migrations::run_migrations;
use crate::error::CoreError;
use chrono::{DateTime, Utc};
use l4a_protocol::{
    AddonDirType, AddonId, AddonSummary, BundleId, BundleKind, BundleRecord, FileRole, GroupId,
    InstallState, IssueId, LoadoutProfile, MapEntry, MetadataSource, PhysicalFileRecord, ProfileId,
    ReconcileAction, ReconcileIssue, ReconcileIssueKind, TaskId, TaskKind, TaskRecord, TaskStatus,
    TrackedCollectionRecord, UserGroup, WorkshopDependency, WorkshopFileType, WorkshopId,
    WorkshopItemRecord, WorkshopItemStatus,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeMap, HashMap, HashSet};
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

    fn lock_conn(&self) -> Result<MutexGuard<'_, Connection>, CoreError> {
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

    pub fn list_physical_files(&self) -> Result<Vec<PhysicalFileRow>, CoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT path, dir_type, file_name, enabled, role, workshop_id, content_hash,
                    dummy_original_hash, dummy_original_time_updated, file_size, modified_at, addon_id
             FROM physical_files",
        )?;
        let rows = stmt.query_map([], |r| {
            let dir_str: String = r.get(1)?;
            let role_str: String = r.get(4)?;
            let wid_raw: Option<i64> = r.get(5)?;
            let addon_raw: Option<String> = r.get(11)?;
            Ok(PhysicalFileRow {
                record: PhysicalFileRecord {
                    path: r.get(0)?,
                    dir_type: parse_enum(&dir_str, AddonDirType::Loading),
                    file_name: r.get(2)?,
                    enabled: r.get::<_, i64>(3)? != 0,
                    role: parse_enum(&role_str, FileRole::RealAddon),
                    workshop_id: wid_raw.and_then(|v| WorkshopId::new(v as u64).ok()),
                    content_hash: r.get(6)?,
                    dummy_original_hash: r.get(7)?,
                    dummy_original_time_updated: r.get(8)?,
                    file_size: r.get::<_, i64>(9)? as u64,
                    modified_at: r.get(10)?,
                },
                addon_id: addon_raw.map(AddonId),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn replace_physical_files(&self, rows: &[PhysicalFileRow]) -> Result<(), CoreError> {
        self.write(|tx| Self::replace_physical_files_tx(tx, rows))
    }

    pub(crate) fn replace_physical_files_tx(
        tx: &Connection,
        rows: &[PhysicalFileRow],
    ) -> Result<(), CoreError> {
        tx.execute("DELETE FROM physical_files", [])?;
        for row in rows {
            let f = &row.record;
            tx.execute(
                r#"
                INSERT INTO physical_files (
                    path, dir_type, file_name, enabled, role, workshop_id, content_hash,
                    dummy_original_hash, dummy_original_time_updated, file_size, modified_at, addon_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                "#,
                params![
                    f.path,
                    serialize_enum(&f.dir_type),
                    f.file_name,
                    if f.enabled { 1 } else { 0 },
                    serialize_enum(&f.role),
                    f.workshop_id.map(|w| w.as_u64() as i64),
                    f.content_hash,
                    f.dummy_original_hash,
                    f.dummy_original_time_updated,
                    f.file_size as i64,
                    f.modified_at,
                    row.addon_id.as_ref().map(AddonId::as_str),
                ],
            )?;
        }
        Ok(())
    }

    pub fn list_addons(&self) -> Result<Vec<AddonSummary>, CoreError> {
        let conn = self.lock_conn()?;
        let snapshots = Self::load_snapshots_map(&conn)?;
        let mut stmt = conn.prepare(
            "SELECT id, workshop_id, title, author, version, install_state, primary_dir,
                    enabled, current_file_name, current_path, content_hash, file_size,
                    files_count, cover_image, categories_json, maps_json, addon_info_json,
                    has_dummy_bypass, local_time_updated
             FROM addons ORDER BY title COLLATE NOCASE ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            let id_str: String = r.get(0)?;
            let wid_raw: Option<i64> = r.get(1)?;
            let wid = wid_raw.and_then(|v| WorkshopId::new(v as u64).ok());
            let state_str: String = r.get(5)?;
            let dir_str: Option<String> = r.get(6)?;
            let cats_json: String = r.get(14)?;
            let maps_json: String = r.get(15)?;
            let info_json: String = r.get(16)?;
            let local_ts: Option<i64> = r.get(18)?;

            let install_state = parse_enum(&state_str, InstallState::Installed);
            let snap = wid.and_then(|w| snapshots.get(&w));
            let remote_status = snap
                .map(|s| s.status)
                .unwrap_or(WorkshopItemStatus::Unknown);
            let remote_ts = snap.and_then(|s| s.time_updated);
            let update_available = match (install_state, local_ts, remote_ts) {
                (InstallState::Installed, Some(l), Some(rt)) => rt > l,
                _ => false,
            };
            let cover = r
                .get::<_, Option<String>>(13)?
                .or_else(|| snap.and_then(|s| s.preview_url.clone()));

            Ok(AddonSummary {
                id: AddonId(id_str),
                workshop_id: wid,
                title: r.get(2)?,
                author: r.get(3)?,
                version: r.get(4)?,
                install_state,
                primary_dir: dir_str.map(|s| parse_enum(&s, AddonDirType::Loading)),
                enabled: r.get::<_, i64>(7)? != 0,
                current_file_name: r.get(8)?,
                current_path: r.get(9)?,
                content_hash: r.get(10)?,
                file_size: r.get::<_, i64>(11)? as u64,
                files_count: r.get::<_, i64>(12)? as usize,
                cover_image: cover,
                categories: serde_json::from_str(&cats_json).unwrap_or_default(),
                maps: serde_json::from_str(&maps_json).unwrap_or_default(),
                has_dummy_bypass: r.get::<_, i64>(17)? != 0,
                remote_status,
                local_time_updated: local_ts,
                remote_time_updated: remote_ts,
                update_available,
                addon_info: serde_json::from_str(&info_json).unwrap_or_default(),
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_addon(&self, id: &AddonId) -> Result<Option<AddonSummary>, CoreError> {
        Ok(self.list_addons()?.into_iter().find(|a| &a.id == id))
    }

    pub fn get_addon_by_workshop_id(
        &self,
        wid: WorkshopId,
    ) -> Result<Option<AddonSummary>, CoreError> {
        Ok(self
            .list_addons()?
            .into_iter()
            .find(|a| a.workshop_id == Some(wid)))
    }

    pub fn upsert_addon(&self, addon: &AddonSummary) -> Result<(), CoreError> {
        let conn = self.lock_conn()?;
        Self::upsert_addon_tx(&conn, addon)
    }

    pub(crate) fn upsert_addon_tx(
        conn: &Connection,
        addon: &AddonSummary,
    ) -> Result<(), CoreError> {
        let now = Utc::now().to_rfc3339();
        conn.execute(
            r#"
            INSERT INTO addons (
                id, workshop_id, title, author, version, install_state, primary_dir,
                enabled, current_file_name, current_path, content_hash, file_size,
                files_count, cover_image, categories_json, maps_json, addon_info_json,
                has_dummy_bypass, local_time_updated, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
            ON CONFLICT(id) DO UPDATE SET
                workshop_id = COALESCE(excluded.workshop_id, addons.workshop_id),
                title = excluded.title,
                author = COALESCE(excluded.author, addons.author),
                version = COALESCE(excluded.version, addons.version),
                install_state = excluded.install_state,
                primary_dir = excluded.primary_dir,
                enabled = excluded.enabled,
                current_file_name = excluded.current_file_name,
                current_path = excluded.current_path,
                content_hash = COALESCE(excluded.content_hash, addons.content_hash),
                file_size = excluded.file_size,
                files_count = excluded.files_count,
                cover_image = COALESCE(excluded.cover_image, addons.cover_image),
                categories_json = excluded.categories_json,
                maps_json = excluded.maps_json,
                addon_info_json = excluded.addon_info_json,
                has_dummy_bypass = excluded.has_dummy_bypass,
                local_time_updated = COALESCE(excluded.local_time_updated, addons.local_time_updated),
                updated_at = excluded.updated_at
            "#,
            params![
                addon.id.as_str(),
                addon.workshop_id.map(|w| w.as_u64() as i64),
                addon.title,
                addon.author,
                addon.version,
                serialize_enum(&addon.install_state),
                addon.primary_dir.as_ref().map(serialize_enum),
                if addon.enabled { 1 } else { 0 },
                addon.current_file_name,
                addon.current_path,
                addon.content_hash,
                addon.file_size as i64,
                addon.files_count as i64,
                addon.cover_image,
                serde_json::to_string(&addon.categories).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(&addon.maps).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(&addon.addon_info).unwrap_or_else(|_| "{}".into()),
                if addon.has_dummy_bypass { 1 } else { 0 },
                addon.local_time_updated,
                now,
            ],
        )?;
        Ok(())
    }

    pub fn mark_addon_uninstalled(&self, id: &AddonId) -> Result<(), CoreError> {
        let conn = self.lock_conn()?;
        Self::mark_addon_uninstalled_tx(&conn, id)
    }

    pub(crate) fn mark_addon_uninstalled_tx(
        conn: &Connection,
        id: &AddonId,
    ) -> Result<(), CoreError> {
        conn.execute(
            "UPDATE addons SET install_state = 'uninstalled_known', primary_dir = NULL, current_path = NULL, enabled = 0, has_dummy_bypass = 0, updated_at = ?2 WHERE id = ?1",
            params![id.as_str(), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn delete_addon_record(&self, id: &AddonId) -> Result<(), CoreError> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM user_group_addons WHERE addon_id = ?1",
            params![id.as_str()],
        )?;
        tx.execute(
            "DELETE FROM profile_addons WHERE addon_id = ?1",
            params![id.as_str()],
        )?;
        tx.execute(
            "DELETE FROM bundle_members WHERE addon_id = ?1",
            params![id.as_str()],
        )?;
        tx.execute("DELETE FROM addons WHERE id = ?1", params![id.as_str()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn update_addon_local_timestamp(
        &self,
        id: &AddonId,
        local_time_updated: Option<i64>,
    ) -> Result<(), CoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE addons SET local_time_updated = ?2, updated_at = ?3 WHERE id = ?1",
            params![id.as_str(), local_time_updated, Utc::now().to_rfc3339()],
        )?;
        Ok(())
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
        // Ordinary upserts retain missing fields. A replacement's metadata belongs to the new VPK.
        tx.execute(
            "UPDATE addons SET cover_image = ?2, local_time_updated = ?3 WHERE id = ?1",
            params![addon.id.as_str(), addon.cover_image, item.time_updated],
        )?;
        tx.execute(
            "UPDATE workshop_snapshots SET preview_url = ?2, time_updated = ?3 WHERE workshop_id = ?1",
            params![item.workshop_id.as_u64() as i64, item.preview_url, item.time_updated],
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
                item.preview_url,
                item.file_url,
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

    fn load_snapshots_map(
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

    pub fn replace_open_reconcile_issues(
        &self,
        detected: &[(ReconcileIssue, String)],
    ) -> Result<(), CoreError> {
        self.write(|tx| Self::replace_open_reconcile_issues_tx(tx, detected))
    }

    pub(crate) fn replace_open_reconcile_issues_tx(
        tx: &Connection,
        detected: &[(ReconcileIssue, String)],
    ) -> Result<(), CoreError> {
        let mut resolved_fps: HashMap<String, String> = HashMap::new();
        {
            let mut stmt = tx
                .prepare("SELECT id, state_fingerprint FROM reconcile_issues WHERE resolved = 1")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (id, fp) = row?;
                resolved_fps.insert(id, fp);
            }
        }

        let mut current_ids: HashSet<&str> = HashSet::with_capacity(detected.len());
        tx.execute("DELETE FROM reconcile_issues WHERE resolved = 0", [])?;

        for (issue, fp) in detected {
            current_ids.insert(issue.id.as_str());
            if let Some(prev_fp) = resolved_fps.get(issue.id.as_str()) {
                if prev_fp == fp {
                    continue;
                }
            }
            tx.execute(
                r#"
                INSERT OR REPLACE INTO reconcile_issues (
                    id, kind, workshop_id, addon_id, loading_path, workshop_path,
                    summary, recommended_action, available_actions_json, resolved, detected_at, state_fingerprint
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                "#,
                params![
                    issue.id.as_str(),
                    serialize_enum(&issue.kind),
                    issue.workshop_id.map(|w| w.as_u64() as i64),
                    issue.addon_id.as_ref().map(AddonId::as_str),
                    issue.loading_path,
                    issue.workshop_path,
                    issue.summary,
                    serialize_enum(&issue.recommended_action),
                    serde_json::to_string(&issue.available_actions).unwrap_or_else(|_| "[]".into()),
                    if issue.resolved { 1 } else { 0 },
                    issue.detected_at.to_rfc3339(),
                    fp,
                ],
            )?;
        }

        for old_id in resolved_fps.keys() {
            if !current_ids.contains(old_id.as_str()) {
                tx.execute(
                    "DELETE FROM reconcile_issues WHERE id = ?1",
                    params![old_id],
                )?;
            }
        }

        Ok(())
    }

    pub fn list_open_reconcile_issues(&self) -> Result<Vec<ReconcileIssue>, CoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, workshop_id, addon_id, loading_path, workshop_path, summary,
                    recommended_action, available_actions_json, resolved, detected_at, state_fingerprint
             FROM reconcile_issues WHERE resolved = 0 ORDER BY detected_at DESC, id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            let kind_str: String = r.get(1)?;
            let wid_raw: Option<i64> = r.get(2)?;
            let addon_raw: Option<String> = r.get(3)?;
            let rec_str: String = r.get(7)?;
            let avail_json: String = r.get(8)?;
            let det_str: String = r.get(10)?;
            Ok(ReconcileIssue {
                id: IssueId(format!(
                    "{}@{}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(11)?
                )),
                kind: parse_enum(&kind_str, ReconcileIssueKind::OrphanDummy),
                workshop_id: wid_raw.and_then(|v| WorkshopId::new(v as u64).ok()),
                addon_id: addon_raw.map(AddonId),
                loading_path: r.get(4)?,
                workshop_path: r.get(5)?,
                summary: r.get(6)?,
                recommended_action: parse_enum(&rec_str, ReconcileAction::Ignore),
                available_actions: serde_json::from_str(&avail_json).unwrap_or_default(),
                resolved: r.get::<_, i64>(9)? != 0,
                detected_at: DateTime::parse_from_rfc3339(&det_str)
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

    pub fn mark_issue_resolved(&self, id: &IssueId) -> Result<(), CoreError> {
        self.write(|tx| Self::mark_issue_resolved_tx(tx, id))
    }

    pub(crate) fn mark_issue_resolved_tx(conn: &Connection, id: &IssueId) -> Result<(), CoreError> {
        if let Some((key, fingerprint)) = id.as_str().rsplit_once('@') {
            let affected = conn.execute(
                "UPDATE reconcile_issues SET resolved = 1 WHERE id = ?1 AND state_fingerprint = ?2 AND resolved = 0",
                params![key, fingerprint],
            )?;
            if affected == 0 {
                return Err(CoreError::Conflict("对账问题已变化，请刷新后重试".into()));
            }
        } else {
            conn.execute(
                "UPDATE reconcile_issues SET resolved = 1 WHERE id = ?1",
                params![id.as_str()],
            )?;
        }
        Ok(())
    }

    pub fn list_bundles(&self) -> Result<Vec<BundleRecord>, CoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, kind, locked, maps_json, cover_image FROM bundles ORDER BY name COLLATE NOCASE ASC",
        )?;
        let mut bundles = Vec::new();
        let rows = stmt.query_map([], |r| {
            let kind_str: String = r.get(2)?;
            let maps_json: String = r.get(4)?;
            Ok(BundleRecord {
                id: BundleId(r.get(0)?),
                name: r.get(1)?,
                kind: parse_enum(&kind_str, BundleKind::Manual),
                locked: r.get::<_, i64>(3)? != 0,
                member_addon_ids: Vec::new(),
                maps: serde_json::from_str(&maps_json).unwrap_or_default(),
                cover_image: r.get(5)?,
            })
        })?;
        for row in rows {
            bundles.push(row?);
        }

        let mut mem_stmt = conn.prepare(
            "SELECT addon_id FROM bundle_members WHERE bundle_id = ?1 ORDER BY position ASC, addon_id ASC",
        )?;
        for b in &mut bundles {
            let members: Vec<AddonId> = mem_stmt
                .query_map(params![b.id.as_str()], |r| Ok(AddonId(r.get(0)?)))?
                .filter_map(Result::ok)
                .collect();
            b.member_addon_ids = members;
        }
        Ok(bundles)
    }

    pub fn save_bundle(&self, bundle: &BundleRecord) -> Result<(), CoreError> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            r#"
            INSERT INTO bundles (id, name, kind, locked, maps_json, cover_image)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                kind = excluded.kind,
                locked = excluded.locked,
                maps_json = excluded.maps_json,
                cover_image = COALESCE(excluded.cover_image, bundles.cover_image)
            "#,
            params![
                bundle.id.as_str(),
                bundle.name,
                serialize_enum(&bundle.kind),
                if bundle.locked { 1 } else { 0 },
                serde_json::to_string(&bundle.maps).unwrap_or_else(|_| "[]".into()),
                bundle.cover_image,
            ],
        )?;
        tx.execute(
            "DELETE FROM bundle_members WHERE bundle_id = ?1",
            params![bundle.id.as_str()],
        )?;
        let mut seen = HashSet::new();
        let mut pos = 0i64;
        for aid in &bundle.member_addon_ids {
            if !seen.insert(aid) {
                continue;
            }
            tx.execute(
                "INSERT OR IGNORE INTO bundle_members (bundle_id, addon_id, position) VALUES (?1, ?2, ?3)",
                params![bundle.id.as_str(), aid.as_str(), pos],
            )?;
            pos += 1;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_unlocked_auto_bundles(&self) -> Result<(), CoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "DELETE FROM bundles WHERE locked = 0 AND kind != 'manual'",
            [],
        )?;
        Ok(())
    }

    pub fn delete_bundle(&self, id: &BundleId) -> Result<(), CoreError> {
        let conn = self.lock_conn()?;
        let affected = conn.execute("DELETE FROM bundles WHERE id = ?1", params![id.as_str()])?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Bundle {id}")));
        }
        Ok(())
    }

    pub fn set_bundle_locked(&self, id: &BundleId, locked: bool) -> Result<(), CoreError> {
        let conn = self.lock_conn()?;
        let affected = conn.execute(
            "UPDATE bundles SET locked = ?2 WHERE id = ?1",
            params![id.as_str(), if locked { 1 } else { 0 }],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Bundle {id}")));
        }
        Ok(())
    }

    pub fn list_tracked_collections(&self) -> Result<Vec<TrackedCollectionRecord>, CoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT collection_id, title, description, auto_sync, removed_upstream_json, last_synced_at
             FROM tracked_collections ORDER BY title COLLATE NOCASE ASC",
        )?;
        let mut list = Vec::new();
        let rows = stmt.query_map([], |r| {
            let cid_raw: i64 = r.get(0)?;
            let rem_json: String = r.get(4)?;
            let synced: Option<String> = r.get(5)?;
            Ok(TrackedCollectionRecord {
                collection_id: WorkshopId::new(cid_raw as u64).unwrap_or(WorkshopId(1)),
                title: r.get(1)?,
                description: r.get(2)?,
                auto_sync: r.get::<_, i64>(3)? != 0,
                member_workshop_ids: Vec::new(),
                removed_upstream_ids: serde_json::from_str(&rem_json).unwrap_or_default(),
                last_synced_at: synced.and_then(|s| {
                    DateTime::parse_from_rfc3339(&s)
                        .map(|d| d.with_timezone(&Utc))
                        .ok()
                }),
            })
        })?;
        for r in rows {
            list.push(r?);
        }

        let mut mem_stmt = conn.prepare(
            "SELECT workshop_id FROM collection_items WHERE collection_id = ?1 ORDER BY position ASC",
        )?;
        for c in &mut list {
            c.member_workshop_ids = mem_stmt
                .query_map(params![c.collection_id.as_u64() as i64], |r| {
                    let raw: i64 = r.get(0)?;
                    Ok(WorkshopId::new(raw as u64).unwrap_or(WorkshopId(1)))
                })?
                .filter_map(Result::ok)
                .collect();
        }
        Ok(list)
    }

    pub fn save_tracked_collection(&self, col: &TrackedCollectionRecord) -> Result<(), CoreError> {
        self.write(|tx| Self::save_tracked_collection_tx(tx, col))
    }

    pub(crate) fn save_tracked_collection_tx(
        tx: &Connection,
        col: &TrackedCollectionRecord,
    ) -> Result<(), CoreError> {
        let cid = col.collection_id.as_u64() as i64;
        tx.execute(
            r#"
            INSERT INTO tracked_collections (collection_id, title, description, auto_sync, removed_upstream_json, last_synced_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(collection_id) DO UPDATE SET
                title = excluded.title,
                description = COALESCE(excluded.description, tracked_collections.description),
                auto_sync = excluded.auto_sync,
                removed_upstream_json = excluded.removed_upstream_json,
                last_synced_at = excluded.last_synced_at
            "#,
            params![
                cid,
                col.title,
                col.description,
                if col.auto_sync { 1 } else { 0 },
                serde_json::to_string(&col.removed_upstream_ids).unwrap_or_else(|_| "[]".into()),
                col.last_synced_at.map(|d| d.to_rfc3339()),
            ],
        )?;
        tx.execute(
            "DELETE FROM collection_items WHERE collection_id = ?1",
            params![cid],
        )?;
        let mut seen = HashSet::new();
        let mut pos = 0i64;
        for wid in &col.member_workshop_ids {
            if !seen.insert(wid) {
                continue;
            }
            tx.execute(
                "INSERT OR IGNORE INTO collection_items (collection_id, workshop_id, position) VALUES (?1, ?2, ?3)",
                params![cid, wid.as_u64() as i64, pos],
            )?;
            pos += 1;
        }
        Ok(())
    }

    pub fn delete_tracked_collection(&self, collection_id: WorkshopId) -> Result<(), CoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "DELETE FROM tracked_collections WHERE collection_id = ?1",
            params![collection_id.as_u64() as i64],
        )?;
        Ok(())
    }

    pub fn list_user_groups(&self) -> Result<Vec<UserGroup>, CoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, tags_json, color FROM user_groups ORDER BY name COLLATE NOCASE ASC",
        )?;
        let mut groups = Vec::new();
        let rows = stmt.query_map([], |r| {
            let tags_json: String = r.get(2)?;
            Ok(UserGroup {
                id: GroupId(r.get(0)?),
                name: r.get(1)?,
                tags: serde_json::from_str(&tags_json).unwrap_or_default(),
                color: r.get(3)?,
                addon_ids: Vec::new(),
            })
        })?;
        for r in rows {
            groups.push(r?);
        }

        let mut mem_stmt = conn.prepare(
            "SELECT addon_id FROM user_group_addons WHERE group_id = ?1 ORDER BY addon_id ASC",
        )?;
        for g in &mut groups {
            g.addon_ids = mem_stmt
                .query_map(params![g.id.as_str()], |r| Ok(AddonId(r.get(0)?)))?
                .filter_map(Result::ok)
                .collect();
        }
        Ok(groups)
    }

    pub fn save_user_group(&self, group: &UserGroup) -> Result<(), CoreError> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            r#"
            INSERT INTO user_groups (id, name, tags_json, color) VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(id) DO UPDATE SET name = excluded.name, tags_json = excluded.tags_json, color = excluded.color
            "#,
            params![
                group.id.as_str(),
                group.name,
                serde_json::to_string(&group.tags).unwrap_or_else(|_| "[]".into()),
                group.color,
            ],
        )?;
        tx.execute(
            "DELETE FROM user_group_addons WHERE group_id = ?1",
            params![group.id.as_str()],
        )?;
        for aid in &group.addon_ids {
            tx.execute(
                "INSERT OR IGNORE INTO user_group_addons (group_id, addon_id) VALUES (?1, ?2)",
                params![group.id.as_str(), aid.as_str()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_user_group(&self, id: &GroupId) -> Result<(), CoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "DELETE FROM user_groups WHERE id = ?1",
            params![id.as_str()],
        )?;
        Ok(())
    }

    pub fn list_profiles(&self) -> Result<Vec<LoadoutProfile>, CoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, description FROM loadout_profiles ORDER BY name COLLATE NOCASE ASC",
        )?;
        let mut profiles = Vec::new();
        let rows = stmt.query_map([], |r| {
            Ok(LoadoutProfile {
                id: ProfileId(r.get(0)?),
                name: r.get(1)?,
                description: r.get(2)?,
                enabled_addon_ids: Vec::new(),
            })
        })?;
        for r in rows {
            profiles.push(r?);
        }

        let mut mem_stmt = conn.prepare(
            "SELECT addon_id FROM profile_addons WHERE profile_id = ?1 ORDER BY addon_id ASC",
        )?;
        for p in &mut profiles {
            p.enabled_addon_ids = mem_stmt
                .query_map(params![p.id.as_str()], |r| Ok(AddonId(r.get(0)?)))?
                .filter_map(Result::ok)
                .collect();
        }
        Ok(profiles)
    }

    pub fn save_profile(&self, profile: &LoadoutProfile) -> Result<(), CoreError> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            r#"
            INSERT INTO loadout_profiles (id, name, description) VALUES (?1, ?2, ?3)
            ON CONFLICT(id) DO UPDATE SET name = excluded.name, description = excluded.description
            "#,
            params![profile.id.as_str(), profile.name, profile.description],
        )?;
        tx.execute(
            "DELETE FROM profile_addons WHERE profile_id = ?1",
            params![profile.id.as_str()],
        )?;
        for aid in &profile.enabled_addon_ids {
            tx.execute(
                "INSERT OR IGNORE INTO profile_addons (profile_id, addon_id) VALUES (?1, ?2)",
                params![profile.id.as_str(), aid.as_str()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_profile(&self, id: &ProfileId) -> Result<(), CoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "DELETE FROM loadout_profiles WHERE id = ?1",
            params![id.as_str()],
        )?;
        Ok(())
    }

    pub fn save_task(&self, task: &TaskRecord) -> Result<(), CoreError> {
        let conn = self.lock_conn()?;
        Self::save_task_tx(&conn, task)
    }

    pub(crate) fn save_task_tx(conn: &Connection, task: &TaskRecord) -> Result<(), CoreError> {
        conn.execute(
            r#"
            INSERT INTO tasks (id, kind, status, target_id, progress_percent, message, error, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(id) DO UPDATE SET
                status = excluded.status,
                progress_percent = excluded.progress_percent,
                message = excluded.message,
                error = excluded.error,
                updated_at = excluded.updated_at
            "#,
            params![
                task.id.as_str(),
                serialize_enum(&task.kind),
                serialize_enum(&task.status),
                task.target_id,
                task.progress_percent as i64,
                task.message,
                task.error,
                task.created_at.to_rfc3339(),
                task.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_tasks(&self) -> Result<Vec<TaskRecord>, CoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, status, target_id, progress_percent, message, error, created_at, updated_at
             FROM tasks ORDER BY created_at DESC LIMIT 500",
        )?;
        let rows = stmt.query_map([], |r| {
            let kind_s: String = r.get(1)?;
            let status_s: String = r.get(2)?;
            let c_s: String = r.get(7)?;
            let u_s: String = r.get(8)?;
            Ok(TaskRecord {
                id: TaskId(r.get(0)?),
                kind: parse_enum(&kind_s, TaskKind::Scan),
                status: parse_enum(&status_s, TaskStatus::Queued),
                target_id: r.get(3)?,
                progress_percent: r.get::<_, i64>(4)?.clamp(0, 100) as u8,
                message: r.get(5)?,
                error: r.get(6)?,
                created_at: DateTime::parse_from_rfc3339(&c_s)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                updated_at: DateTime::parse_from_rfc3339(&u_s)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn lookup_addon_workshop_id_by_hash(
        &self,
        hash: &str,
    ) -> Result<Option<WorkshopId>, CoreError> {
        let conn = self.lock_conn()?;
        let raw: Option<i64> = conn
            .query_row(
                "SELECT workshop_id FROM addons WHERE content_hash = ?1 AND workshop_id IS NOT NULL LIMIT 1",
                params![hash],
                |r| r.get(0),
            )
            .optional()?;
        Ok(raw.and_then(|v| WorkshopId::new(v as u64).ok()))
    }

    pub fn lookup_addon_title_and_groups(
        &self,
    ) -> Result<HashMap<AddonId, (String, Vec<String>)>, CoreError> {
        let addons = self.list_addons()?;
        let groups = self.list_user_groups()?;
        let mut group_names: HashMap<AddonId, Vec<String>> = HashMap::new();
        for g in groups {
            for aid in g.addon_ids {
                group_names.entry(aid).or_default().push(g.name.clone());
            }
        }
        let mut out = HashMap::new();
        for a in addons {
            let names = group_names.remove(&a.id).unwrap_or_default();
            out.insert(a.id, (a.title, names));
        }
        Ok(out)
    }
}

pub fn serialize_enum<T: serde::Serialize>(val: &T) -> String {
    serde_json::to_string(val)
        .unwrap_or_default()
        .trim_matches('"')
        .to_string()
}

pub fn parse_enum<T: serde::de::DeserializeOwned>(raw: &str, fallback: T) -> T {
    serde_json::from_value(serde_json::Value::String(raw.to_string())).unwrap_or(fallback)
}

pub fn derive_categories_from_info(addon_info: &BTreeMap<String, String>) -> Vec<String> {
    let flag = |keys: &[&str]| -> bool {
        keys.iter().any(|k| {
            addon_info
                .get(&k.to_ascii_lowercase())
                .is_some_and(|v| v.trim() == "1" || v.eq_ignore_ascii_case("true"))
        })
    };
    let mut cats = Vec::new();
    if flag(&["addoncontent_campaign"]) {
        cats.push("Campaign".into());
    }
    if flag(&["addoncontent_map"]) {
        cats.push("Map".into());
    }
    if flag(&["addoncontent_survivor"]) {
        cats.push("Survivor".into());
    }
    if flag(&[
        "addoncontent_weaponmodel",
        "content_weaponmodel",
        "content_weapon",
    ]) {
        cats.push("Weapon".into());
    }
    if flag(&["addoncontent_skin"]) {
        cats.push("Skin".into());
    }
    if flag(&["addoncontent_script"]) {
        cats.push("Script".into());
    }
    if flag(&["addoncontent_music", "addoncontent_sound"]) {
        cats.push("Audio".into());
    }
    if flag(&["addoncontent_bossinfected", "addoncontent_commoninfected"]) {
        cats.push("Infected".into());
    }
    if flag(&["addoncontent_ui"]) {
        cats.push("UI".into());
    }
    cats
}

pub fn collect_installed_addon_maps(addon: &AddonSummary) -> Vec<MapEntry> {
    addon.maps.clone()
}
