use super::metadata::{
    clean_addon_title, clean_author_name, clean_workshop_title, is_valid_local_cover_key,
    local_cover_for_db, merge_categories, normalize_valid_remote_cover_url, parse_enum,
    serialize_enum, strip_vpk_file_stem,
};
use super::{LocalDb, PhysicalFileRow};
use crate::error::CoreError;
use chrono::Utc;
use l4a_protocol::{
    AddonDirType, AddonId, AddonSummary, FileRole, InstallState, MapEntry, PhysicalFileRecord,
    WorkshopId, WorkshopItemStatus,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeMap, HashMap};

impl LocalDb {
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
            let stored_cover = r.get::<_, Option<String>>(13)?;
            let local_cover_image = stored_cover
                .as_deref()
                .map(str::trim)
                .filter(|c| is_valid_local_cover_key(c))
                .map(str::to_owned);
            let workshop_cover = snap
                .and_then(|s| s.preview_url.as_deref())
                .and_then(normalize_valid_remote_cover_url);
            let cover = workshop_cover
                .or_else(|| {
                    stored_cover
                        .as_deref()
                        .and_then(normalize_valid_remote_cover_url)
                })
                .or_else(|| local_cover_image.clone());

            let stored_title: String = r.get(2)?;
            let stored_author: Option<String> = r.get(3)?;
            let current_file_name: Option<String> = r.get(8)?;
            let stored_cats: Vec<String> = serde_json::from_str(&cats_json).unwrap_or_default();
            let maps: Vec<MapEntry> = serde_json::from_str(&maps_json).unwrap_or_default();
            let addon_info: BTreeMap<String, String> =
                serde_json::from_str(&info_json).unwrap_or_default();

            let addon_title = clean_addon_title(addon_info.get("addontitle").map(String::as_str));
            let workshop_title = snap.and_then(|s| clean_workshop_title(Some(s.title.as_str())));
            let title = workshop_title
                .clone()
                .or_else(|| addon_title.clone())
                .or_else(|| clean_addon_title(Some(stored_title.as_str())))
                .or_else(|| {
                    current_file_name
                        .as_deref()
                        .map(strip_vpk_file_stem)
                        .filter(|s| !s.is_empty())
                })
                .unwrap_or(stored_title);
            let author = clean_author_name(stored_author.as_deref())
                .or_else(|| {
                    clean_author_name(
                        addon_info
                            .get("addonauthor")
                            .or_else(|| addon_info.get("author"))
                            .map(String::as_str),
                    )
                })
                .or_else(|| snap.and_then(|s| clean_author_name(s.creator_name.as_deref())));
            let categories = merge_categories(
                &stored_cats,
                &addon_info,
                &maps,
                snap.map(|s| s.tags.as_slice()).unwrap_or(&[]),
            );

            Ok(AddonSummary {
                id: AddonId(id_str),
                workshop_id: wid,
                title,
                addon_title,
                workshop_title,
                author,
                version: r.get(4)?,
                install_state,
                primary_dir: dir_str.map(|s| parse_enum(&s, AddonDirType::Loading)),
                enabled: r.get::<_, i64>(7)? != 0,
                current_file_name,
                current_path: r.get(9)?,
                content_hash: r.get(10)?,
                file_size: r.get::<_, i64>(11)? as u64,
                files_count: r.get::<_, i64>(12)? as usize,
                cover_image: cover,
                local_cover_image,
                categories,
                maps,
                has_dummy_bypass: r.get::<_, i64>(17)? != 0,
                remote_status,
                local_time_updated: local_ts,
                remote_time_updated: remote_ts,
                update_available,
                addon_info,
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

    pub(crate) fn cached_workshop_title(
        &self,
        wid: WorkshopId,
    ) -> Result<Option<String>, CoreError> {
        let conn = self.lock_conn()?;
        let (addon_title, snapshot_title): (Option<String>, Option<String>) = conn
            .prepare_cached(
                "SELECT
                    (SELECT title FROM addons WHERE workshop_id = ?1 ORDER BY title COLLATE NOCASE, id LIMIT 1),
                    (SELECT title FROM workshop_snapshots WHERE workshop_id = ?1)",
            )?
            .query_row([wid.as_u64() as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok([addon_title, snapshot_title]
            .iter()
            .flatten()
            .map(|title| title.trim())
            .find(|title| !title.is_empty())
            .map(str::to_owned))
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
                local_cover_for_db(addon),
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

    pub(crate) fn list_stored_addon_categories(
        &self,
    ) -> Result<HashMap<AddonId, Vec<String>>, CoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare("SELECT id, categories_json FROM addons")?;
        let rows = stmt.query_map([], |r| {
            let id: String = r.get(0)?;
            let raw: String = r.get(1)?;
            let cats: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
            Ok((AddonId(id), cats))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, cats) = row?;
            out.insert(id, cats);
        }
        Ok(out)
    }

    pub(crate) fn update_addon_file_state(&self, addon: &AddonSummary) -> Result<(), CoreError> {
        let conn = self.lock_conn()?;
        let updated = conn.execute(
            r#"
            UPDATE addons SET
                install_state = ?2,
                primary_dir = ?3,
                enabled = ?4,
                current_file_name = ?5,
                current_path = ?6,
                has_dummy_bypass = ?7,
                updated_at = ?8
            WHERE id = ?1
            "#,
            params![
                addon.id.as_str(),
                serialize_enum(&addon.install_state),
                addon.primary_dir.as_ref().map(serialize_enum),
                if addon.enabled { 1 } else { 0 },
                addon.current_file_name,
                addon.current_path,
                if addon.has_dummy_bypass { 1 } else { 0 },
                Utc::now().to_rfc3339(),
            ],
        )?;
        if updated == 0 {
            Self::upsert_addon_tx(&conn, addon)?;
        }
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
        tx.execute(
            "DELETE FROM filter_exclusions WHERE addon_id = ?1",
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
