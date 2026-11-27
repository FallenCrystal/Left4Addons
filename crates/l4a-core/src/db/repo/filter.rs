use super::metadata::{parse_enum, serialize_enum};
use super::LocalDb;
use crate::error::CoreError;
use chrono::{DateTime, Utc};
use l4a_protocol::{
    AddonId, ExclusionDisplayLevel, FilterExclusionRecord, FilterItemVerdict, InstallState,
    UpsertFilterExclusionRequest,
};
use rusqlite::{params, Connection, OptionalExtension};

fn parse_rfc3339_utc(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn map_exclusion_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<FilterExclusionRecord> {
    let addon_raw: String = r.get(0)?;
    let level_raw: String = r.get(1)?;
    let bind_fingerprint = r.get::<_, i64>(2)? != 0;
    let bound_fingerprint: Option<String> = r.get(3)?;
    let note: Option<String> = r.get(4)?;
    let created_raw: String = r.get(5)?;
    let updated_raw: String = r.get(6)?;
    let install_state_raw: Option<String> = r.get(7)?;
    let addon_hash: Option<String> = r.get(8)?;

    let current_fingerprint = match install_state_raw.as_deref() {
        Some(state) if parse_enum(state, InstallState::MissingFile) == InstallState::Installed => {
            addon_hash.filter(|h| !h.trim().is_empty())
        }
        _ => None,
    };

    let fingerprint_valid = !bind_fingerprint
        || (bound_fingerprint.is_some() && bound_fingerprint == current_fingerprint);

    Ok(FilterExclusionRecord {
        addon_id: AddonId(addon_raw),
        display_level: parse_enum(&level_raw, ExclusionDisplayLevel::LazyLoad),
        bind_fingerprint,
        bound_fingerprint,
        current_fingerprint,
        fingerprint_valid,
        note,
        created_at: parse_rfc3339_utc(&created_raw),
        updated_at: parse_rfc3339_utc(&updated_raw),
    })
}

impl LocalDb {
    pub fn list_filter_exclusions(&self) -> Result<Vec<FilterExclusionRecord>, CoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT fe.addon_id, fe.display_level, fe.bind_fingerprint, fe.bound_fingerprint,
                    fe.note, fe.created_at, fe.updated_at, a.install_state, a.content_hash
             FROM filter_exclusions fe
             LEFT JOIN addons a ON a.id = fe.addon_id
             ORDER BY fe.updated_at DESC, fe.addon_id ASC",
        )?;
        let rows = stmt.query_map([], map_exclusion_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn get_filter_exclusion_tx(
        tx: &Connection,
        addon_id: &AddonId,
    ) -> Result<Option<FilterExclusionRecord>, CoreError> {
        let record = tx
            .query_row(
                "SELECT fe.addon_id, fe.display_level, fe.bind_fingerprint, fe.bound_fingerprint,
                        fe.note, fe.created_at, fe.updated_at, a.install_state, a.content_hash
                 FROM filter_exclusions fe
                 LEFT JOIN addons a ON a.id = fe.addon_id
                 WHERE fe.addon_id = ?1",
                params![addon_id.as_str()],
                map_exclusion_row,
            )
            .optional()?;
        Ok(record)
    }

    pub fn upsert_filter_exclusion(
        &self,
        req: &UpsertFilterExclusionRequest,
    ) -> Result<FilterExclusionRecord, CoreError> {
        if req.addon_id.as_str().trim().is_empty() {
            return Err(CoreError::InvalidInput("模组 ID 不能为空".into()));
        }
        self.write(|tx| {
            let addon_row: Option<(String, Option<String>)> = tx
                .query_row(
                    "SELECT install_state, content_hash FROM addons WHERE id = ?1",
                    params![req.addon_id.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;

            let Some((install_state_raw, content_hash)) = addon_row else {
                return Err(CoreError::NotFound(format!("未找到模组: {}", req.addon_id)));
            };

            let installed_hash = if parse_enum(&install_state_raw, InstallState::MissingFile)
                == InstallState::Installed
            {
                content_hash
                    .map(|h| h.trim().to_string())
                    .filter(|h| !h.is_empty())
            } else {
                None
            };

            let (bind_fingerprint, bound_fingerprint) = match req.bind_fingerprint {
                Some(true) => {
                    let Some(hash) = installed_hash else {
                        return Err(CoreError::InvalidInput(
                            "该模组暂无已安装的本地 VPK 指纹，无法启用指纹绑定排除".into(),
                        ));
                    };
                    (true, Some(hash))
                }
                Some(false) => (false, None),
                None => Self::get_filter_exclusion_tx(tx, &req.addon_id)?
                    .map(|existing| (existing.bind_fingerprint, existing.bound_fingerprint))
                    .unwrap_or_default(),
            };

            let note = req
                .note
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let now = Utc::now().to_rfc3339();

            tx.execute(
                r#"
                INSERT INTO filter_exclusions (
                    addon_id, display_level, bind_fingerprint, bound_fingerprint,
                    note, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                ON CONFLICT(addon_id) DO UPDATE SET
                    display_level = excluded.display_level,
                    bind_fingerprint = excluded.bind_fingerprint,
                    bound_fingerprint = excluded.bound_fingerprint,
                    note = excluded.note,
                    updated_at = excluded.updated_at
                "#,
                params![
                    req.addon_id.as_str(),
                    serialize_enum(&req.display_level),
                    i32::from(bind_fingerprint),
                    bound_fingerprint,
                    note,
                    now,
                ],
            )?;

            Self::get_filter_exclusion_tx(tx, &req.addon_id)?
                .ok_or_else(|| CoreError::NotFound(format!("未找到排除记录: {}", req.addon_id)))
        })
    }

    pub fn delete_filter_exclusion(&self, addon_id: &AddonId) -> Result<(), CoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "DELETE FROM filter_exclusions WHERE addon_id = ?1",
            params![addon_id.as_str()],
        )?;
        Ok(())
    }

    pub(crate) fn migrate_filter_exclusion_addon_id_tx(
        tx: &Connection,
        old_id: &AddonId,
        new_id: &AddonId,
    ) -> Result<(), CoreError> {
        if old_id == new_id {
            return Ok(());
        }
        let target_exists: bool = tx
            .query_row(
                "SELECT 1 FROM filter_exclusions WHERE addon_id = ?1",
                params![new_id.as_str()],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if target_exists {
            return Ok(());
        }
        tx.execute(
            "UPDATE filter_exclusions SET addon_id = ?2, updated_at = ?3 WHERE addon_id = ?1",
            params![old_id.as_str(), new_id.as_str(), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn get_cached_filter_verdict(
        &self,
        target_key: &str,
        content_sig: &str,
        policy_sig: &str,
        expected_addon_state_sig: Option<&str>,
        max_age: Option<chrono::TimeDelta>,
    ) -> Result<Option<FilterItemVerdict>, CoreError> {
        let conn = self.lock_conn()?;
        let row: Option<(Option<String>, String)> = conn
            .query_row(
                "SELECT addon_state_sig, verdict_json FROM filter_verdicts
                 WHERE target_key = ?1 AND policy_sig = ?2 AND content_sig = ?3",
                params![target_key, policy_sig, content_sig],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((addon_state_sig, json_str)) = row else {
            return Ok(None);
        };
        let Ok(verdict) = serde_json::from_str::<FilterItemVerdict>(&json_str) else {
            return Ok(None);
        };
        if !verdict.complete {
            return Ok(None);
        }
        if let Some(limit) = max_age {
            if Utc::now() - verdict.evaluated_at >= limit {
                return Ok(None);
            }
        }
        match expected_addon_state_sig {
            Some(expected) if addon_state_sig.as_deref() != Some(expected) => return Ok(None),
            None if addon_state_sig.is_some() => return Ok(None),
            _ => {}
        }
        Ok(Some(verdict))
    }

    pub fn list_filter_verdicts_for_policy(
        &self,
        policy_sig: &str,
        max_age: Option<chrono::TimeDelta>,
    ) -> Result<Vec<CachedFilterVerdictRow>, CoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT content_sig, addon_state_sig, verdict_json FROM filter_verdicts
             WHERE policy_sig = ?1
             ORDER BY target_key ASC",
        )?;
        let rows = stmt.query_map(params![policy_sig], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let now = Utc::now();
        let mut out = Vec::new();
        for row in rows {
            let (content_sig, addon_state_sig, raw) = row?;
            if let Ok(verdict) = serde_json::from_str::<FilterItemVerdict>(&raw) {
                if !verdict.complete {
                    continue;
                }
                if let Some(limit) = max_age {
                    if now - verdict.evaluated_at >= limit {
                        continue;
                    }
                }
                out.push(CachedFilterVerdictRow {
                    content_sig,
                    addon_state_sig,
                    verdict,
                });
            }
        }
        Ok(out)
    }

    pub fn put_cached_filter_verdict(
        &self,
        target_key: &str,
        content_sig: &str,
        addon_state_sig: Option<&str>,
        verdict_policy_sig: &str,
        active_policy_sig: &str,
        verdict: &FilterItemVerdict,
    ) -> Result<bool, CoreError> {
        if !verdict.complete || verdict_policy_sig != active_policy_sig {
            return Ok(false);
        }
        let verdict_json = serde_json::to_string(verdict)
            .map_err(|e| CoreError::Config(format!("序列化过滤判定结果失败: {e}")))?;
        let now = Utc::now().to_rfc3339();
        self.write(|tx| {
            tx.execute(
                r#"
                INSERT INTO filter_verdicts (
                    target_key, content_sig, policy_sig, verdict_json, updated_at, addon_state_sig
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(target_key, policy_sig) DO UPDATE SET
                    content_sig = excluded.content_sig,
                    verdict_json = excluded.verdict_json,
                    updated_at = excluded.updated_at,
                    addon_state_sig = excluded.addon_state_sig
                "#,
                params![
                    target_key,
                    content_sig,
                    verdict_policy_sig,
                    verdict_json,
                    now,
                    addon_state_sig,
                ],
            )?;
            Ok(true)
        })
    }

    pub fn prune_filter_verdicts_except(&self, active_policy_sig: &str) -> Result<u64, CoreError> {
        let conn = self.lock_conn()?;
        let removed = conn.execute(
            "DELETE FROM filter_verdicts WHERE policy_sig != ?1",
            params![active_policy_sig],
        )?;
        Ok(removed as u64)
    }

    pub fn clear_filter_verdicts(&self) -> Result<u64, CoreError> {
        let conn = self.lock_conn()?;
        let removed = conn.execute("DELETE FROM filter_verdicts", [])?;
        Ok(removed as u64)
    }

    pub fn clear_llm_filter_verdicts(&self) -> Result<(), CoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "DELETE FROM filter_verdicts
             WHERE CASE WHEN json_valid(verdict_json) THEN json_extract(verdict_json, '$.decisionSource') END
                   IN ('llm_flagged_true', 'llm_flagged_false')",
            [],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct CachedFilterVerdictRow {
    pub content_sig: String,
    pub addon_state_sig: Option<String>,
    pub verdict: FilterItemVerdict,
}
