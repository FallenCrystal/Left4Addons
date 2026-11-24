use super::items::dependency_requirements;
use super::{
    parse_workshop_id_col, serialize_json_enum, CatalogIndexPolicy, NegativeObservation,
    ServerRepo, StoredItemMeta, UpstreamApplySummary, MAX_NEGATIVE_CACHE_ENTRIES,
    STATE_BACKFILL_CURSOR, STATE_BACKFILL_SYNCED_TOTAL, STATE_REMOTE_WORKSHOP_TOTAL,
};
use crate::upstream::WorkshopObservation;
use chrono::{DateTime, Utc};
use l4a_protocol::{MetadataSource, WorkshopFetchOutcome, WorkshopId, WorkshopItemStatus};
use rusqlite::{params, Connection, Transaction, TransactionBehavior};
use std::collections::HashMap;

impl ServerRepo {
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

    pub(super) fn scrub_private_details_tx(
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
}
