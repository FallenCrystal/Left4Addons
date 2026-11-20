use crate::db::repo::PhysicalFileRow;
use crate::db::LocalDb;
use crate::error::CoreError;
use crate::file_changes::{occupied, regular_file, take_matching_dummy, FileChanges, StagedFile};
use crate::ops::{append_os_suffix, build_target_vpk_name, strip_suffix_ascii_case};
use crate::scanner::scan_and_commit;
use chrono::Utc;
use l4a_protocol::{
    AddonDirType, AddonId, AppConfig, FileRole, GameUpdateDummyPolicy, IssueId, ReconcileAction,
    ReconcileIssue, ReconcileIssueKind, WorkshopId,
};
use l4a_vpk::{inspect_dummy_vpk, DummyInspection, DummySpec};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

fn row_fingerprint_part(row: Option<&PhysicalFileRow>) -> String {
    match row {
        Some(r) => format!(
            "{}:{}:{}:{:?}:{:?}",
            r.record.path,
            r.record.file_size,
            r.record.modified_at,
            r.record.content_hash,
            r.record.dummy_original_hash
        ),
        None => "none".to_string(),
    }
}

fn issue_fingerprint(
    kind: ReconcileIssueKind,
    l_row: Option<&PhysicalFileRow>,
    w_row: Option<&PhysicalFileRow>,
) -> String {
    let raw = format!(
        "{kind:?}|{}|{}",
        row_fingerprint_part(l_row),
        row_fingerprint_part(w_row)
    );
    hex::encode(Sha256::digest(raw.as_bytes()))
}

/// Dummy Bypass v2 对账检测器：基于 `physical_files` 检出 6 类磁盘状态异常，
/// 尤其是「游戏更新覆写 Dummy」与「游戏自动下载新组件进入 workshop/」。
pub fn detect_reconcile_issues(
    db: &LocalDb,
    config: &AppConfig,
) -> Result<Vec<ReconcileIssue>, CoreError> {
    let detected = collect_issues(&db.list_physical_files()?, config);
    db.replace_open_reconcile_issues(&detected)?;
    db.list_open_reconcile_issues()
}

pub(crate) fn collect_issues(
    files: &[PhysicalFileRow],
    config: &AppConfig,
) -> Vec<(ReconcileIssue, String)> {
    let now = Utc::now();
    let mut detected: Vec<(ReconcileIssue, String)> = Vec::new();

    #[derive(Default)]
    struct Bucket<'a> {
        loading_real: Vec<&'a PhysicalFileRow>,
        workshop_real: Vec<&'a PhysicalFileRow>,
        workshop_dummies: Vec<&'a PhysicalFileRow>,
        loading_dummies: Vec<&'a PhysicalFileRow>,
    }

    let mut by_workshop: HashMap<WorkshopId, Bucket<'_>> = HashMap::new();
    let mut disabled_workshop_files = Vec::new();

    for row in files {
        let f = &row.record;
        if f.dir_type == AddonDirType::Workshop && !f.enabled {
            disabled_workshop_files.push(row);
        }
        let Some(wid) = f.workshop_id else {
            continue;
        };
        let bucket = by_workshop.entry(wid).or_default();
        match (f.dir_type, f.role) {
            (AddonDirType::Loading, FileRole::RealAddon) => bucket.loading_real.push(row),
            (AddonDirType::Workshop, FileRole::RealAddon) => bucket.workshop_real.push(row),
            (AddonDirType::Workshop, FileRole::DummyV1 | FileRole::DummyV2) => {
                bucket.workshop_dummies.push(row)
            }
            (AddonDirType::Loading, FileRole::DummyV1 | FileRole::DummyV2) => {
                bucket.loading_dummies.push(row)
            }
            _ => {}
        }
    }

    for (wid, bucket) in by_workshop {
        let addon_id = Some(AddonId::from_workshop_id(wid));
        let loading_first = bucket.loading_real.first().copied();
        let workshop_first = bucket.workshop_real.first().copied();
        let dummy_first = bucket.workshop_dummies.first().copied();

        match (loading_first, workshop_first, dummy_first) {
            // 情况 1：`loading/` 有托管真实 VPK，`workshop/` 却出现真实 VPK -> 极可能是游戏更新覆写了 Dummy！
            (Some(l_row), Some(w_row), _) => {
                let same_hash = l_row.record.content_hash.is_some()
                    && l_row.record.content_hash == w_row.record.content_hash;
                if same_hash {
                    let kind = ReconcileIssueKind::DuplicateIdenticalVpk;
                    let fp = issue_fingerprint(kind, Some(l_row), Some(w_row));
                    detected.push((
                        ReconcileIssue {
                            id: IssueId(format!("dup_{wid}")),
                            kind,
                            workshop_id: Some(wid),
                            addon_id: addon_id.clone(),
                            loading_path: Some(l_row.record.path.clone()),
                            workshop_path: Some(w_row.record.path.clone()),
                            summary: format!(
                                "Workshop {wid} 在 loading 与 workshop 目录存在相同指纹的重复 VPK"
                            ),
                            recommended_action: ReconcileAction::KeepLocalAndRegenerateDummy,
                            available_actions: vec![
                                ReconcileAction::KeepLocalAndRegenerateDummy,
                                ReconcileAction::KeepWorkshopDeleteLoading,
                                ReconcileAction::Ignore,
                            ],
                            resolved: false,
                            detected_at: now,
                        },
                        fp,
                    ));
                } else {
                    let kind = ReconcileIssueKind::DummyOverwrittenByGameUpdate;
                    let fp = issue_fingerprint(kind, Some(l_row), Some(w_row));
                    let recommended = match config.game_update_dummy_policy {
                        GameUpdateDummyPolicy::AutoKeepLocalRegenerateDummy => {
                            ReconcileAction::KeepLocalAndRegenerateDummy
                        }
                        _ => ReconcileAction::AdoptWorkshopUpdate,
                    };
                    detected.push((
                        ReconcileIssue {
                            id: IssueId(format!("overwritten_{wid}")),
                            kind,
                            workshop_id: Some(wid),
                            addon_id: addon_id.clone(),
                            loading_path: Some(l_row.record.path.clone()),
                            workshop_path: Some(w_row.record.path.clone()),
                            summary: format!(
                                "Workshop {wid} 的 Dummy 已被游戏下载的新版真实 VPK 覆写，建议合并更新至 loading/ 并保留自定义命名与分组"
                            ),
                            recommended_action: recommended,
                            available_actions: vec![
                                ReconcileAction::AdoptWorkshopUpdate,
                                ReconcileAction::KeepLocalAndRegenerateDummy,
                                ReconcileAction::KeepWorkshopDeleteLoading,
                                ReconcileAction::Ignore,
                            ],
                            resolved: false,
                            detected_at: now,
                        },
                        fp,
                    ));
                }
            }
            // 情况 2：仅 `workshop/` 出现真实 VPK 且启用了 Dummy Bypass -> 游戏自动下载了新组件！
            (None, Some(w_row), None) if config.enable_dummy_bypass => {
                let kind = ReconcileIssueKind::NewWorkshopAutoDownload;
                let fp = issue_fingerprint(kind, None, Some(w_row));
                detected.push((
                    ReconcileIssue {
                        id: IssueId(format!("new_ws_{wid}")),
                        kind,
                        workshop_id: Some(wid),
                        addon_id: addon_id.clone(),
                        loading_path: None,
                        workshop_path: Some(w_row.record.path.clone()),
                        summary: format!(
                            "检测到 workshop/ 中存在未托管的组件 {wid}（可能是游戏自动下载的新增组件）"
                        ),
                        recommended_action: ReconcileAction::ImportToLoadingAndCreateDummy,
                        available_actions: vec![
                            ReconcileAction::ImportToLoadingAndCreateDummy,
                            ReconcileAction::LeaveInWorkshop,
                            ReconcileAction::Ignore,
                        ],
                        resolved: false,
                        detected_at: now,
                    },
                    fp,
                ));
            }
            // 情况 3：`loading/` 已不存在真实 VPK，但 `workshop/` 还残留孤儿 Dummy
            (None, None, Some(d_row)) => {
                let kind = ReconcileIssueKind::OrphanDummy;
                let fp = issue_fingerprint(kind, None, Some(d_row));
                detected.push((
                    ReconcileIssue {
                        id: IssueId(format!("orphan_dummy_{wid}")),
                        kind,
                        workshop_id: Some(wid),
                        addon_id: addon_id.clone(),
                        loading_path: None,
                        workshop_path: Some(d_row.record.path.clone()),
                        summary: format!(
                            "workshop/ 存在孤儿 Dummy ({wid})，对应 loading/ 模组已被移除"
                        ),
                        recommended_action: ReconcileAction::RemoveOrphanDummy,
                        available_actions: vec![
                            ReconcileAction::RemoveOrphanDummy,
                            ReconcileAction::Ignore,
                        ],
                        resolved: false,
                        detected_at: now,
                    },
                    fp,
                ));
            }
            // 情况 4：Dummy 存在且 `loading/` 也存在真实 VPK，但 Dummy 仍是 v1 或指纹已过期
            (Some(l_row), None, Some(d_row)) => {
                let stale = d_row.record.role == FileRole::DummyV1
                    || d_row.record.dummy_original_hash != l_row.record.content_hash;
                if stale {
                    let kind = ReconcileIssueKind::StaleDummyMetadata;
                    let fp = issue_fingerprint(kind, Some(l_row), Some(d_row));
                    detected.push((
                        ReconcileIssue {
                            id: IssueId(format!("stale_dummy_{wid}")),
                            kind,
                            workshop_id: Some(wid),
                            addon_id: addon_id.clone(),
                            loading_path: Some(l_row.record.path.clone()),
                            workshop_path: Some(d_row.record.path.clone()),
                            summary: format!("Workshop {wid} 的 Dummy 元信息已过期或为旧版 v1，建议刷新为 v2 Dummy"),
                            recommended_action: ReconcileAction::RefreshDummy,
                            available_actions: vec![
                                ReconcileAction::RefreshDummy,
                                ReconcileAction::Ignore,
                            ],
                            resolved: false,
                            detected_at: now,
                        },
                        fp,
                    ));
                }
            }
            _ => {}
        }

        // 不应该出现在 loading/ 里的 Dummy 也算孤儿清理对象
        for ld in bucket.loading_dummies {
            let kind = ReconcileIssueKind::OrphanDummy;
            let fp = issue_fingerprint(kind, Some(ld), None);
            detected.push((
                ReconcileIssue {
                    id: IssueId(format!("orphan_loading_dummy_{wid}")),
                    kind,
                    workshop_id: Some(wid),
                    addon_id: addon_id.clone(),
                    loading_path: Some(ld.record.path.clone()),
                    workshop_path: None,
                    summary: format!(
                        "loading/ 目录内意外存在 Dummy 文件 ({})",
                        ld.record.file_name
                    ),
                    recommended_action: ReconcileAction::RemoveOrphanDummy,
                    available_actions: vec![
                        ReconcileAction::RemoveOrphanDummy,
                        ReconcileAction::Ignore,
                    ],
                    resolved: false,
                    detected_at: now,
                },
                fp,
            ));
        }
    }

    for row in disabled_workshop_files {
        let kind = ReconcileIssueKind::DisabledWorkshopConflict;
        let fp = issue_fingerprint(kind, None, Some(row));
        detected.push((
            ReconcileIssue {
                id: IssueId(format!("disabled_ws_{}", row.record.file_name)),
                kind,
                workshop_id: row.record.workshop_id,
                addon_id: row.addon_id.clone(),
                loading_path: None,
                workshop_path: Some(row.record.path.clone()),
                summary: format!(
                    "workshop/ 目录中的 {} 被标记为 .disabled，Steam 可能将其视为损坏并重新下载",
                    row.record.file_name
                ),
                recommended_action: ReconcileAction::EnableWorkshopFile,
                available_actions: vec![
                    ReconcileAction::EnableWorkshopFile,
                    ReconcileAction::Ignore,
                ],
                resolved: false,
                detected_at: now,
            },
            fp,
        ));
    }

    detected
}

/// 根据配置自动处理可自动决策的对账问题（仅在用户显式配置了自动策略时触发）。
pub fn auto_resolve_by_policy(
    db: &LocalDb,
    config: &AppConfig,
    image_cache_dir: &Path,
    issues: &[ReconcileIssue],
) -> Result<usize, CoreError> {
    let mut resolved = 0usize;
    for issue in issues {
        let action = match issue.kind {
            ReconcileIssueKind::DummyOverwrittenByGameUpdate if config.enable_dummy_bypass => {
                match config.game_update_dummy_policy {
                    GameUpdateDummyPolicy::AutoAdoptKeepCustomizations => {
                        Some(ReconcileAction::AdoptWorkshopUpdate)
                    }
                    GameUpdateDummyPolicy::AutoKeepLocalRegenerateDummy => {
                        Some(ReconcileAction::KeepLocalAndRegenerateDummy)
                    }
                    GameUpdateDummyPolicy::Ask => None,
                }
            }
            ReconcileIssueKind::NewWorkshopAutoDownload
                if config.enable_dummy_bypass && config.auto_adopt_new_workshop_downloads =>
            {
                Some(ReconcileAction::ImportToLoadingAndCreateDummy)
            }
            _ => None,
        };
        if let Some(act) = action {
            resolve_reconcile_issue(db, config, image_cache_dir, &issue.id, act)?;
            resolved += 1;
        }
    }
    Ok(resolved)
}

/// 执行具体的对账修复动作。
pub fn resolve_reconcile_issue(
    db: &LocalDb,
    config: &AppConfig,
    image_cache_dir: &Path,
    issue_id: &IssueId,
    action: ReconcileAction,
) -> Result<(), CoreError> {
    let issue = db
        .list_open_reconcile_issues()?
        .into_iter()
        .find(|issue| &issue.id == issue_id)
        .ok_or_else(|| CoreError::Conflict(format!("对账问题已变化，请刷新后重试: {issue_id}")))?;
    if !issue.available_actions.contains(&action) {
        return Err(CoreError::InvalidInput("该对账问题不支持所选动作".into()));
    }

    let rows = db.list_physical_files()?;
    let checked = |path: Option<&str>, dir: &str| {
        path.map(|path| {
            let row = rows
                .iter()
                .find(|row| row.record.path == path)
                .ok_or_else(|| stale_file(Path::new(path)))?;
            let path = Path::new(path);
            if dir.is_empty() || path.parent() != Some(Path::new(dir)) {
                return Err(stale_file(path));
            }
            verify_row(row, path)?;
            Ok(row)
        })
        .transpose()
    };
    let loading = checked(issue.loading_path.as_deref(), &config.loading_dir)?;
    let workshop = checked(issue.workshop_path.as_deref(), &config.workshop_dir)?;
    if issue_id.as_str().rsplit_once('@').map(|(_, fp)| fp)
        != Some(issue_fingerprint(issue.kind, loading, workshop).as_str())
    {
        return Err(CoreError::Conflict("对账问题已变化，请刷新后重试".into()));
    }
    if matches!(
        action,
        ReconcileAction::Ignore | ReconcileAction::LeaveInWorkshop
    ) {
        return db.mark_issue_resolved(issue_id);
    }

    let require_row = |row: Option<&PhysicalFileRow>| -> Result<PhysicalFileRow, CoreError> {
        row.cloned()
            .ok_or_else(|| CoreError::InvalidInput("对账问题缺少文件记录".into()))
    };
    let require_wid = || {
        issue
            .workshop_id
            .ok_or_else(|| CoreError::InvalidInput("缺少 workshop_id".into()))
    };

    FileChanges::run(|changes| {
        let mut seed = None;
        let mut expected = None;
        match action {
            ReconcileAction::AdoptWorkshopUpdate
            | ReconcileAction::ImportToLoadingAndCreateDummy => {
                let workshop = require_row(workshop)?;
                let wid = require_wid()?;
                let adoption = action == ReconcileAction::AdoptWorkshopUpdate;
                let dest = if adoption {
                    Path::new(&require_row(loading)?.record.path).to_path_buf()
                } else {
                    if config.loading_dir.is_empty() {
                        return Err(CoreError::InvalidInput("未配置 loading_dir".into()));
                    }
                    let title = match inspect_dummy_vpk(&workshop.record.path) {
                        DummyInspection::RealVpk { title, .. } => {
                            title.unwrap_or_else(|| format!("Workshop_{wid}"))
                        }
                        _ => return Err(stale_file(Path::new(&workshop.record.path))),
                    };
                    let name = build_target_vpk_name(Some(wid), None, &title, &config.rename_rules);
                    let dest = Path::new(&config.loading_dir).join(name);
                    if occupied(&dest)? || occupied(&append_os_suffix(&dest, ".disabled"))? {
                        return Err(CoreError::Conflict(format!(
                            "目标文件已存在: {}",
                            dest.display()
                        )));
                    }
                    dest
                };
                let remote_ts = db
                    .get_workshop_snapshot(wid)?
                    .and_then(|item| item.time_updated);
                let backup = take_row(changes, &workshop)?;
                let staged = StagedFile::prepare(&backup, &dest)?;
                if let Some(loading) = loading.filter(|_| adoption) {
                    take_row(changes, loading)?;
                }
                changes.publish_staged(staged, &dest)?;
                let hash = workshop
                    .record
                    .content_hash
                    .clone()
                    .ok_or_else(|| stale_file(&backup))?;
                if config.enable_dummy_bypass {
                    publish_reconcile_dummy(
                        changes,
                        config,
                        &dest,
                        wid,
                        &hash,
                        workshop.record.file_size,
                        remote_ts,
                    )?;
                }
                expected = Some((AddonId::from_workshop_id(wid), dest.clone(), hash.clone()));
                if let Some(mut addon) = db.get_addon_by_workshop_id(wid)? {
                    addon.current_path = Some(dest.to_string_lossy().into_owned());
                    addon.content_hash = Some(hash);
                    if adoption {
                        addon.local_time_updated = remote_ts.or(addon.local_time_updated);
                    }
                    seed = Some(addon);
                }
            }
            ReconcileAction::KeepLocalAndRegenerateDummy | ReconcileAction::RefreshDummy => {
                let loading = require_row(loading)?;
                let wid = require_wid()?;
                let src = Path::new(&loading.record.path);
                let hash = loading
                    .record
                    .content_hash
                    .as_deref()
                    .ok_or_else(|| stale_file(src))?;
                let remote_ts = db
                    .get_workshop_snapshot(wid)?
                    .and_then(|item| item.time_updated);
                take_row(changes, &require_row(workshop)?)?;
                publish_reconcile_dummy(
                    changes,
                    config,
                    src,
                    wid,
                    hash,
                    loading.record.file_size,
                    remote_ts,
                )?;
                verify_row(&loading, src)?;
            }
            ReconcileAction::KeepWorkshopDeleteLoading => {
                take_row(changes, &require_row(loading)?)?;
                let row = require_row(workshop)?;
                verify_row(&row, Path::new(&row.record.path))?;
            }
            ReconcileAction::RemoveOrphanDummy => {
                for row in [loading, workshop].into_iter().flatten() {
                    take_row(changes, row)?;
                }
            }
            ReconcileAction::EnableWorkshopFile => {
                let row = require_row(workshop)?;
                let enabled = strip_suffix_ascii_case(&row.record.path, ".disabled")
                    .ok_or_else(|| CoreError::InvalidInput("文件未禁用".into()))?;
                let backup = take_row(changes, &row)?;
                changes.publish_copy(&backup, Path::new(enabled))?;
            }
            ReconcileAction::Ignore | ReconcileAction::LeaveInWorkshop => unreachable!(),
        }
        scan_and_commit(db, config, image_cache_dir, seed, |tx, addons| {
            changes.verify()?;
            if let Some((id, dest, hash)) = &expected {
                if !addons.get(id).is_some_and(|addon| {
                    addon.current_path.as_deref() == dest.to_str()
                        && addon.content_hash.as_ref() == Some(hash)
                }) {
                    return Err(stale_file(dest));
                }
            }
            if action == ReconcileAction::KeepWorkshopDeleteLoading {
                let wid = require_wid()?;
                LocalDb::mark_issue_resolved_tx(tx, &IssueId(format!("new_ws_{wid}")))?;
            }
            Ok(())
        })?;
        Ok(())
    })
}

fn stale_file(path: &Path) -> CoreError {
    CoreError::Conflict(format!("文件已变化，请重新扫描: {}", path.display()))
}

fn verify_row(row: &PhysicalFileRow, path: &Path) -> Result<(), CoreError> {
    let record = &row.record;
    let metadata = regular_file(path)?;
    if metadata.len() != record.file_size
        || crate::scanner::modified_at(&metadata) != record.modified_at
    {
        return Err(stale_file(path));
    }
    let matches = match (record.role, inspect_dummy_vpk(path)) {
        (FileRole::RealAddon, DummyInspection::RealVpk { content_hash, .. }) => {
            record.content_hash.as_deref() == Some(content_hash.as_str())
        }
        (FileRole::DummyV1 | FileRole::DummyV2, DummyInspection::Dummy(meta)) => {
            (record.role == FileRole::DummyV2) == (meta.schema_version >= 2)
                && meta
                    .workshop_id
                    .is_none_or(|wid| record.workshop_id == Some(wid))
                && record.dummy_original_hash == meta.original_hash
                && record.dummy_original_time_updated == meta.original_time_updated
        }
        (FileRole::Corrupted, DummyInspection::Corrupted(_)) => true,
        _ => false,
    };
    if !matches {
        return Err(stale_file(path));
    }
    Ok(())
}

fn take_row(
    changes: &mut FileChanges,
    row: &PhysicalFileRow,
) -> Result<std::path::PathBuf, CoreError> {
    let backup = changes.take(Path::new(&row.record.path))?;
    verify_row(row, &backup)?;
    Ok(backup)
}

fn publish_reconcile_dummy(
    changes: &mut FileChanges,
    config: &AppConfig,
    src: &Path,
    wid: WorkshopId,
    hash: &str,
    size: u64,
    time: Option<i64>,
) -> Result<(), CoreError> {
    if config.workshop_dir.is_empty() {
        return Err(CoreError::InvalidInput("未配置 workshop_dir".into()));
    }
    let dst = Path::new(&config.workshop_dir).join(format!("{wid}.vpk"));
    take_matching_dummy(changes, &dst, wid)?;
    changes.publish_dummy(
        src,
        &dst,
        &DummySpec {
            workshop_id: wid,
            original_hash: hash.to_owned(),
            original_size: size,
            original_time_updated: time,
        },
    )
}
