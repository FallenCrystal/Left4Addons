use super::metadata::{
    is_valid_local_cover_key, normalize_valid_remote_cover_url, parse_enum, serialize_enum,
};
use super::LocalDb;
use crate::error::CoreError;
use chrono::{DateTime, Utc};
use l4a_protocol::{
    AddonId, BundleId, BundleKind, BundleRecord, GroupId, LoadoutProfile, ProfileId,
    TrackedCollectionRecord, UserGroup, WorkshopId,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;

impl LocalDb {
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
                cover_image: r.get::<_, Option<String>>(5)?.and_then(|c| {
                    normalize_valid_remote_cover_url(&c).or_else(|| {
                        let trimmed = c.trim();
                        is_valid_local_cover_key(trimmed).then(|| trimmed.to_owned())
                    })
                }),
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
        Self::write_bundle_tx(&tx, bundle)?;
        tx.commit()?;
        Ok(())
    }

    /// 仅用于自动重建分包：若同 ID 的 Bundle 已被用户锁定或为手动分包，拒绝覆盖以严格保护锁定保证。
    pub fn save_auto_bundle(&self, bundle: &BundleRecord) -> Result<bool, CoreError> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        let existing_protected: Option<bool> = tx
            .query_row(
                "SELECT locked, kind FROM bundles WHERE id = ?1",
                params![bundle.id.as_str()],
                |r| {
                    let locked = r.get::<_, i64>(0)? != 0;
                    let kind: String = r.get(1)?;
                    Ok(locked || kind == "manual")
                },
            )
            .optional()?;
        if existing_protected == Some(true) {
            return Ok(false);
        }
        Self::write_bundle_tx(&tx, bundle)?;
        tx.commit()?;
        Ok(true)
    }

    fn write_bundle_tx(tx: &Connection, bundle: &BundleRecord) -> Result<(), CoreError> {
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
}
