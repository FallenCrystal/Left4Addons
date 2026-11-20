use crate::db::LocalDb;
use crate::error::CoreError;
use chrono::Utc;
use l4a_protocol::{
    AppConfig, CollectionSyncDiff, DependencyDiff, InstallState, ItemSubmission, MetadataSource,
    ObsoleteDependencyHint, RemovedUpstreamItem, SubmissionBatchRequest, SyncReport,
    UpdateAvailableItem, WorkshopFileType, WorkshopFetchOutcome, WorkshopId, WorkshopItemStatus,
};
use l4a_workshop::{CatalogBackendProvider, WorkshopResolver};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// 远程生命周期同步：检测模组更新、上游下架/私密、父模组依赖新增/移除（含多父引用保护），
/// 以及追踪合集的新增/移除 Diff。
pub async fn sync_lifecycle(
    db: &LocalDb,
    config: &AppConfig,
    resolver: &WorkshopResolver,
    catalog_reporter: Option<Arc<CatalogBackendProvider>>,
) -> Result<SyncReport, CoreError> {
    let addons = db.list_addons()?;
    let collections = db.list_tracked_collections()?;

    let mut query_ids: Vec<WorkshopId> = addons
        .iter()
        .filter_map(|a| a.workshop_id)
        .collect();
    for col in &collections {
        for mid in &col.member_workshop_ids {
            query_ids.push(*mid);
        }
    }
    query_ids.sort_unstable();
    query_ids.dedup();

    let mut report = SyncReport {
        checked_items: query_ids.len(),
        ..Default::default()
    };

    if !query_ids.is_empty() {
        // 拉取新快照之前记录所有已知父模组的旧活跃依赖集合
        let mut old_deps_by_parent: HashMap<WorkshopId, HashSet<WorkshopId>> = HashMap::new();
        for wid in &query_ids {
            let deps = db.list_active_dependencies(*wid)?;
            if !deps.is_empty() {
                old_deps_by_parent.insert(*wid, deps.into_iter().map(|d| d.workshop_id).collect());
            }
        }

        let (outcomes, warnings) = resolver.fetch_items(&query_ids).await?;
        report.warnings.extend(warnings);

        let mut submissions = Vec::new();
        let addon_by_wid: HashMap<WorkshopId, _> = addons
            .iter()
            .filter_map(|a| a.workshop_id.map(|w| (w, a.clone())))
            .collect();

        // 先写入所有最新快照与依赖更新，以便随后计算全图残余入度
        let mut diff_candidates: Vec<(WorkshopId, String, Vec<WorkshopId>, Vec<WorkshopId>)> =
            Vec::new();

        for outcome in outcomes {
            match outcome {
                WorkshopFetchOutcome::Found { item } => {
                    let wid = item.workshop_id;
                    let can_diff_deps =
                        source_provides_dependencies(item.source) || !item.dependencies.is_empty();
                    let old_dep_set = old_deps_by_parent.get(&wid).cloned().unwrap_or_default();

                    if can_diff_deps {
                        let new_dep_set: HashSet<WorkshopId> =
                            item.dependencies.iter().map(|d| d.workshop_id).collect();
                        let added: Vec<WorkshopId> = new_dep_set
                            .difference(&old_dep_set)
                            .copied()
                            .collect();
                        let removed: Vec<WorkshopId> = old_dep_set
                            .difference(&new_dep_set)
                            .copied()
                            .collect();

                        if !added.is_empty() || !removed.is_empty() {
                            let mut added_sorted = added;
                            let mut removed_sorted = removed;
                            added_sorted.sort_unstable();
                            removed_sorted.sort_unstable();
                            diff_candidates.push((
                                wid,
                                item.title.clone(),
                                added_sorted,
                                removed_sorted,
                            ));
                        }
                    }

                    // 只有支持依赖信息的来源 (或非空依赖列表) 才替换现有依赖边。
                    // 仅用 Steam Web API（不暴露依赖字段）同步时绝不擦除历史依赖。
                    db.upsert_workshop_snapshot(&item, can_diff_deps)?;

                    if let Some(addon) = addon_by_wid.get(&wid) {
                        if let Some(remote_ts) = item.time_updated {
                            if addon.install_state == InstallState::Installed
                                && addon
                                    .local_time_updated
                                    .is_some_and(|local_ts| remote_ts > local_ts)
                            {
                                report.updates_available.push(UpdateAvailableItem {
                                    addon_id: addon.id.clone(),
                                    workshop_id: wid,
                                    title: item.title.clone(),
                                    local_time_updated: addon.local_time_updated,
                                    remote_time_updated: remote_ts,
                                    source: item.source,
                                });
                            }
                        }
                    }

                    if item.source != MetadataSource::CatalogServer {
                        submissions.push(ItemSubmission {
                            workshop_id: item.workshop_id,
                            file_type: item.file_type,
                            status: item.status,
                            title: item.title,
                            description: item.description,
                            creator_steam_id: item.creator_steam_id,
                            creator_name: item.creator_name,
                            preview_url: item.preview_url,
                            file_url: item.file_url,
                            file_size: item.file_size,
                            time_created: item.time_created,
                            time_updated: item.time_updated,
                            tags: item.tags,
                            dependencies: item.dependencies,
                            collection_children: item.collection_children,
                            subscriptions: item.subscriptions,
                            favorites: item.favorites,
                            observed_source: item.source,
                            observed_at: item.fetched_at,
                        });
                    }
                }
                WorkshopFetchOutcome::Removed {
                    workshop_id,
                    source,
                    observed_at,
                    ..
                } => {
                    db.mark_workshop_status(workshop_id, WorkshopItemStatus::Removed, source)?;
                    let title = if let Some(addon) = addon_by_wid.get(&workshop_id) {
                        report.removed_upstream.push(RemovedUpstreamItem {
                            addon_id: addon.id.clone(),
                            workshop_id,
                            title: addon.title.clone(),
                            status: WorkshopItemStatus::Removed,
                            local_file_preserved: addon.install_state == InstallState::Installed,
                            detected_at: observed_at,
                        });
                        addon.title.clone()
                    } else {
                        format!("Workshop #{workshop_id}")
                    };
                    if source != MetadataSource::CatalogServer {
                        submissions.push(ItemSubmission {
                            workshop_id,
                            file_type: WorkshopFileType::Item,
                            status: WorkshopItemStatus::Removed,
                            title,
                            description: None,
                            creator_steam_id: None,
                            creator_name: None,
                            preview_url: None,
                            file_url: None,
                            file_size: None,
                            time_created: None,
                            time_updated: None,
                            tags: Vec::new(),
                            dependencies: Vec::new(),
                            collection_children: Vec::new(),
                            subscriptions: None,
                            favorites: None,
                            observed_source: source,
                            observed_at,
                        });
                    }
                }
                WorkshopFetchOutcome::PrivateOrUnavailable {
                    workshop_id,
                    source,
                    observed_at,
                    ..
                } => {
                    db.mark_workshop_status(
                        workshop_id,
                        WorkshopItemStatus::PrivateOrUnavailable,
                        source,
                    )?;
                    let title = if let Some(addon) = addon_by_wid.get(&workshop_id) {
                        report.removed_upstream.push(RemovedUpstreamItem {
                            addon_id: addon.id.clone(),
                            workshop_id,
                            title: addon.title.clone(),
                            status: WorkshopItemStatus::PrivateOrUnavailable,
                            local_file_preserved: addon.install_state == InstallState::Installed,
                            detected_at: observed_at,
                        });
                        addon.title.clone()
                    } else {
                        format!("Workshop #{workshop_id}")
                    };
                    if source != MetadataSource::CatalogServer {
                        submissions.push(ItemSubmission {
                            workshop_id,
                            file_type: WorkshopFileType::Item,
                            status: WorkshopItemStatus::PrivateOrUnavailable,
                            title,
                            description: None,
                            creator_steam_id: None,
                            creator_name: None,
                            preview_url: None,
                            file_url: None,
                            file_size: None,
                            time_created: None,
                            time_updated: None,
                            tags: Vec::new(),
                            dependencies: Vec::new(),
                            collection_children: Vec::new(),
                            subscriptions: None,
                            favorites: None,
                            observed_source: source,
                            observed_at,
                        });
                    }
                }
            }
        }

        // 计算全局已安装父模组的剩余依赖引用（解决"某个地图作者不再需要某分包，但另一个地图还在用"的问题）
        let installed_wids: HashSet<WorkshopId> = addons
            .iter()
            .filter(|a| a.install_state == InstallState::Installed)
            .filter_map(|a| a.workshop_id)
            .collect();

        let all_edges = db.list_all_active_dependency_edges()?;
        let mut active_parents_by_dep: HashMap<WorkshopId, Vec<WorkshopId>> = HashMap::new();
        for (parent, dep) in all_edges {
            if installed_wids.contains(&parent) {
                active_parents_by_dep.entry(dep).or_default().push(parent);
            }
        }

        let mut removed_parents_map: HashMap<WorkshopId, Vec<WorkshopId>> = HashMap::new();
        for (parent_wid, parent_title, added, removed) in diff_candidates {
            for dep in &removed {
                removed_parents_map.entry(*dep).or_default().push(parent_wid);
            }
            report.dependency_diffs.push(DependencyDiff {
                parent_workshop_id: parent_wid,
                parent_title,
                added_dependencies: added,
                removed_dependencies: removed,
            });
        }

        let mut obsolete_list = Vec::new();
        for (dep_wid, mut removed_from_parents) in removed_parents_map {
            removed_from_parents.sort_unstable();
            removed_from_parents.dedup();
            let mut remaining = active_parents_by_dep
                .get(&dep_wid)
                .cloned()
                .unwrap_or_default();
            remaining.sort_unstable();
            remaining.dedup();

            let dep_addon = addon_by_wid.get(&dep_wid);
            let snap = db.get_workshop_snapshot(dep_wid)?;
            let dep_title = dep_addon
                .map(|a| a.title.clone())
                .or_else(|| snap.map(|s| s.title));
            let is_installed = dep_addon.is_some_and(|a| a.install_state == InstallState::Installed);

            obsolete_list.push(ObsoleteDependencyHint {
                dependency_workshop_id: dep_wid,
                dependency_addon_id: dep_addon.map(|a| a.id.clone()),
                dependency_title: dep_title,
                removed_from_parents,
                can_safely_remove: remaining.is_empty() && is_installed,
                remaining_active_parents: remaining,
            });
        }
        obsolete_list.sort_by_key(|o| o.dependency_workshop_id);
        report.obsolete_dependencies = obsolete_list;

        // 可选地将核验结果提交到自托管 l4a-server
        if let (Some(reporter), Some(token)) =
            (catalog_reporter, config.catalog_contributor_token.as_deref())
        {
            if !submissions.is_empty() && !token.trim().is_empty() {
                if let Err(err) = reporter
                    .submit_observations(token, &SubmissionBatchRequest { items: submissions })
                    .await
                {
                    report
                        .warnings
                        .push(format!("向 Catalog Server 上报观测数据失败: {err}"));
                }
            }
        }
    }

    // 同步已追踪合集的成员变更 Diff
    for mut col in collections {
        match resolver.fetch_collection(col.collection_id).await {
            Ok((remote, warnings)) => {
                report.warnings.extend(warnings);
                if matches!(
                    remote.status,
                    WorkshopItemStatus::Removed | WorkshopItemStatus::PrivateOrUnavailable
                ) {
                    report.warnings.push(format!(
                        "合集 {} ({}) 远端状态为 {:?}，已保留本地既有成员列表",
                        col.title, col.collection_id, remote.status
                    ));
                    col.last_synced_at = Some(Utc::now());
                    db.save_tracked_collection(&col)?;
                    continue;
                }

                let old_set: HashSet<WorkshopId> = col.member_workshop_ids.iter().copied().collect();
                let new_set: HashSet<WorkshopId> = remote.child_ids.iter().copied().collect();

                let mut added: Vec<WorkshopId> = new_set.difference(&old_set).copied().collect();
                let mut removed: Vec<WorkshopId> = old_set.difference(&new_set).copied().collect();
                added.sort_unstable();
                removed.sort_unstable();

                if !added.is_empty() || !removed.is_empty() {
                    report.collection_diffs.push(CollectionSyncDiff {
                        collection_id: col.collection_id,
                        title: remote.title.clone().unwrap_or_else(|| col.title.clone()),
                        added_items: added,
                        removed_items: removed.clone(),
                    });
                }

                for item in &remote.items {
                    db.upsert_workshop_snapshot(item, false)?;
                }

                if let Some(t) = remote.title {
                    if !t.trim().is_empty() {
                        col.title = t;
                    }
                }
                if remote.description.is_some() {
                    col.description = remote.description;
                }
                col.member_workshop_ids = remote.child_ids;
                col.removed_upstream_ids.retain(|id| !new_set.contains(id));
                col.removed_upstream_ids.extend(removed);
                col.removed_upstream_ids.sort_unstable();
                col.removed_upstream_ids.dedup();
                col.last_synced_at = Some(Utc::now());
                db.save_tracked_collection(&col)?;
            }
            Err(err) => {
                report
                    .warnings
                    .push(format!("同步合集 {} 失败: {err}", col.collection_id));
            }
        }
    }

    Ok(report)
}

fn source_provides_dependencies(source: MetadataSource) -> bool {
    matches!(
        source,
        MetadataSource::SteamworksSdk
            | MetadataSource::CatalogServer
            | MetadataSource::WebScraper
            | MetadataSource::AdminOverride
    )
}
