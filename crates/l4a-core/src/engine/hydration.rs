use super::L4aEngine;
use crate::error::CoreError;
use crate::lifecycle::source_provides_dependencies;
use l4a_protocol::{
    CoreEvent, WorkshopFetchOutcome, WorkshopFileType, WorkshopId, WorkshopItemStatus,
};
use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

impl L4aEngine {
    #[doc(hidden)]
    pub fn expire_hydration_backoff_for_test(&self) {
        let now = Instant::now();
        let mut guard = self.hydration_backoff.lock().unwrap();
        for (_, retry_at) in guard.values_mut() {
            *retry_at = now;
        }
    }

    /// 自动为尚未缓存工坊快照的本地/工坊模组补齐 Steam 创意工坊元数据（标题、封面 `preview_url`、作者、标签与依赖关系），
    /// 与 v1 启动/扫描时自动查询工坊元数据的行为保持一致，且使用单飞保护与有界退避重试避免并发及离线循环请求。
    pub async fn hydrate_missing_workshop_metadata(&self) -> Result<usize, CoreError> {
        const HYDRATE_BATCH_SIZE: usize = 50;
        const HYDRATION_RETRY_BASE: Duration = Duration::from_secs(15);
        const HYDRATION_RETRY_MAX: Duration = Duration::from_secs(300);

        struct HydrationGuard<'a>(&'a AtomicBool);
        impl Drop for HydrationGuard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }

        let record_unresolved = |ids: &[WorkshopId]| {
            if ids.is_empty() {
                return;
            }
            let now = Instant::now();
            let mut guard = self.hydration_backoff.lock().unwrap();
            for &wid in ids {
                let attempts = guard.get(&wid).map_or(0, |(n, _)| n.saturating_add(1));
                let factor = 1u32.checked_shl(attempts.min(5)).unwrap_or(32);
                let delay = HYDRATION_RETRY_BASE
                    .saturating_mul(factor)
                    .min(HYDRATION_RETRY_MAX);
                guard.insert(wid, (attempts, now + delay));
            }
        };

        self.hydration_pending.store(true, Ordering::SeqCst);
        let mut total_hydrated = 0usize;

        loop {
            if self
                .hydrating_workshop
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return Ok(total_hydrated);
            }
            let guard = HydrationGuard(&self.hydrating_workshop);

            while self.hydration_pending.swap(false, Ordering::SeqCst) {
                let resolver = self.resolver.read().unwrap().clone();
                // 即使只是读取元数据，初始化 Steamworks 也会影响 VAC 游戏启动。
                let background_resolver = resolver.without_steamworks_sdk();
                if background_resolver.providers().is_empty() {
                    break;
                }

                let snapshots = self.db.list_workshop_snapshots()?;
                let in_backoff: HashSet<WorkshopId> = {
                    let now = Instant::now();
                    let backoff = self.hydration_backoff.lock().unwrap();
                    backoff
                        .iter()
                        .filter_map(|(&wid, &(_, retry_at))| (now < retry_at).then_some(wid))
                        .collect()
                };
                let mut missing: BTreeSet<WorkshopId> = self
                    .db
                    .list_addons()?
                    .into_iter()
                    .filter(|a| a.remote_status == WorkshopItemStatus::Unknown)
                    .filter_map(|a| a.workshop_id)
                    .filter(|wid| !snapshots.contains_key(wid) && !in_backoff.contains(wid))
                    .collect();
                for col in self.db.list_tracked_collections()? {
                    for wid in col.member_workshop_ids {
                        if !snapshots.contains_key(&wid) && !in_backoff.contains(&wid) {
                            missing.insert(wid);
                        }
                    }
                }

                if missing.is_empty() {
                    break;
                }

                let missing_ids: Vec<WorkshopId> = missing.into_iter().collect();
                for chunk in missing_ids.chunks(HYDRATE_BATCH_SIZE) {
                    let fetched = background_resolver.fetch_items(chunk).await;
                    let workspace = self.lock_workspace()?;
                    if !Arc::ptr_eq(&resolver, &self.resolver.read().unwrap()) {
                        self.hydration_pending.store(true, Ordering::SeqCst);
                        break;
                    }

                    let Ok((outcomes, _warnings)) = fetched else {
                        record_unresolved(chunk);
                        continue;
                    };

                    let resolved_ids: HashSet<_> = outcomes
                        .iter()
                        .map(WorkshopFetchOutcome::workshop_id)
                        .collect();
                    if !resolved_ids.is_empty() {
                        let mut backoff = self.hydration_backoff.lock().unwrap();
                        for wid in &resolved_ids {
                            backoff.remove(wid);
                        }
                    }
                    let unresolved: Vec<WorkshopId> = chunk
                        .iter()
                        .copied()
                        .filter(|wid| !resolved_ids.contains(wid))
                        .collect();
                    record_unresolved(&unresolved);

                    let mut chunk_changed = false;
                    for outcome in outcomes {
                        let workshop_id = outcome.workshop_id();
                        if self.db.has_workshop_snapshot(workshop_id)? {
                            continue;
                        }
                        match outcome {
                            WorkshopFetchOutcome::Found { item } => {
                                if item.file_type == WorkshopFileType::Collection {
                                    self.db.mark_workshop_status(
                                        workshop_id,
                                        WorkshopItemStatus::PrivateOrUnavailable,
                                        item.source,
                                    )?;
                                } else {
                                    let replace_deps = source_provides_dependencies(item.source)
                                        || !item.dependencies.is_empty();
                                    self.db.upsert_workshop_snapshot(&item, replace_deps)?;
                                    total_hydrated += 1;
                                }
                            }
                            WorkshopFetchOutcome::Removed { source, .. } => {
                                self.db.mark_workshop_status(
                                    workshop_id,
                                    WorkshopItemStatus::Removed,
                                    source,
                                )?;
                            }
                            WorkshopFetchOutcome::PrivateOrUnavailable { source, .. } => {
                                self.db.mark_workshop_status(
                                    workshop_id,
                                    WorkshopItemStatus::PrivateOrUnavailable,
                                    source,
                                )?;
                            }
                        }
                        chunk_changed = true;
                    }
                    if chunk_changed {
                        self.refresh_bundles_logged();
                    }
                    drop(workspace);

                    if chunk_changed {
                        self.events.emit(CoreEvent::SnapshotUpdated {
                            external_change: false,
                        });
                    }
                }
            }

            drop(guard);
            if !self.hydration_pending.load(Ordering::SeqCst) {
                break;
            }
        }

        Ok(total_hydrated)
    }
}
