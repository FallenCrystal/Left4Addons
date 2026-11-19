use crate::db::repo::PhysicalFileRow;
use crate::db::LocalDb;
use crate::error::CoreError;
use crate::ops::{
    append_os_suffix, build_target_vpk_name, strip_suffix_ascii_case, write_dummy_atomically,
};
use chrono::Utc;
use l4a_protocol::{
    AddonDirType, AddonId, AppConfig, FileRole, GameUpdateDummyPolicy, IssueId, ReconcileAction,
    ReconcileIssue, ReconcileIssueKind, WorkshopId,
};
use l4a_vpk::{extract_addon_metadata, DummySpec};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

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
pub fn detect_reconcile_issues(db: &LocalDb, config: &AppConfig) -> Result<Vec<ReconcileIssue>, CoreError> {
    let files = db.list_physical_files()?;
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

    for row in &files {
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
                        summary: format!("workshop/ 存在孤儿 Dummy ({wid})，对应 loading/ 模组已被移除"),
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
                    summary: format!("loading/ 目录内意外存在 Dummy 文件 ({})", ld.record.file_name),
                    recommended_action: ReconcileAction::RemoveOrphanDummy,
                    available_actions: vec![ReconcileAction::RemoveOrphanDummy, ReconcileAction::Ignore],
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

    db.replace_open_reconcile_issues(&detected)?;
    db.list_open_reconcile_issues()
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
            ReconcileIssueKind::DummyOverwrittenByGameUpdate => match config.game_update_dummy_policy {
                GameUpdateDummyPolicy::AutoAdoptKeepCustomizations => {
                    Some(ReconcileAction::AdoptWorkshopUpdate)
                }
                GameUpdateDummyPolicy::AutoKeepLocalRegenerateDummy => {
                    Some(ReconcileAction::KeepLocalAndRegenerateDummy)
                }
                GameUpdateDummyPolicy::Ask => None,
            },
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
    let issues = db.list_open_reconcile_issues()?;
    let issue = issues
        .into_iter()
        .find(|i| &i.id == issue_id)
        .ok_or_else(|| CoreError::NotFound(format!("未找到对账问题: {issue_id}")))?;

    match action {
        ReconcileAction::Ignore => {
            db.mark_issue_resolved(issue_id)?;
            return Ok(());
        }
        ReconcileAction::LeaveInWorkshop => {
            db.mark_issue_resolved(issue_id)?;
            return Ok(());
        }
        ReconcileAction::AdoptWorkshopUpdate => {
            let loading_path = issue
                .loading_path
                .as_deref()
                .map(PathBuf::from)
                .ok_or_else(|| CoreError::InvalidInput("缺少 loading_path".into()))?;
            let workshop_path = issue
                .workshop_path
                .as_deref()
                .map(PathBuf::from)
                .ok_or_else(|| CoreError::InvalidInput("缺少 workshop_path".into()))?;
            let wid = issue
                .workshop_id
                .ok_or_else(|| CoreError::InvalidInput("缺少 workshop_id".into()))?;

            // 1. 先校验并计算新版 VPK 的真实指纹与大小（损坏则拒绝覆盖旧文件）
            let meta = extract_addon_metadata(&workshop_path, image_cache_dir);
            if let Some(err) = meta.error {
                return Err(CoreError::Vpk(err));
            }
            let new_size = fs::metadata(&workshop_path)?.len();
            let remote_ts = db
                .get_workshop_snapshot(wid)?
                .and_then(|s| s.time_updated);

            // 2. 用 workshop/ 新版 VPK 原子替换 loading/ 路径（保留原自定义文件名及 .disabled 状态！）
            move_or_copy_replace(&workshop_path, &loading_path)?;

            // 3. 在 workshop/ 原子生成带新指纹的 v2 Dummy
            if config.enable_dummy_bypass {
                if config.workshop_dir.trim().is_empty() {
                    return Err(CoreError::InvalidInput("已启用 Dummy Bypass，但未配置 workshop_dir".into()));
                }
                let dummy_path = Path::new(&config.workshop_dir).join(format!("{wid}.vpk"));
                write_dummy_atomically(
                    &loading_path,
                    &dummy_path,
                    &DummySpec {
                        workshop_id: wid,
                        original_hash: meta.content_hash.clone(),
                        original_size: new_size,
                        original_time_updated: remote_ts,
                    },
                )?;
            }

            if let Some(ref aid) = issue.addon_id {
                if let Some(mut existing_addon) = db.get_addon(aid)? {
                    existing_addon.content_hash = Some(meta.content_hash);
                    existing_addon.file_size = new_size;
                    existing_addon.local_time_updated = remote_ts;
                    existing_addon.has_dummy_bypass = config.enable_dummy_bypass;
                    db.upsert_addon(&existing_addon)?;
                }
            }
        }
        ReconcileAction::KeepLocalAndRegenerateDummy | ReconcileAction::RefreshDummy => {
            let loading_path = issue
                .loading_path
                .as_deref()
                .map(PathBuf::from)
                .ok_or_else(|| CoreError::InvalidInput("缺少 loading_path".into()))?;
            let wid = issue
                .workshop_id
                .ok_or_else(|| CoreError::InvalidInput("缺少 workshop_id".into()))?;
            if config.workshop_dir.trim().is_empty() {
                return Err(CoreError::InvalidInput("未配置 workshop_dir".into()));
            }
            if !loading_path.is_file() {
                return Err(CoreError::NotFound(format!(
                    "本地 loading 文件不存在: {}",
                    loading_path.display()
                )));
            }
            let meta = extract_addon_metadata(&loading_path, image_cache_dir);
            if let Some(err) = meta.error {
                return Err(CoreError::Vpk(err));
            }
            let size = fs::metadata(&loading_path)?.len();
            let remote_ts = db
                .get_workshop_snapshot(wid)?
                .and_then(|s| s.time_updated);
            let dummy_path = Path::new(&config.workshop_dir).join(format!("{wid}.vpk"));
            write_dummy_atomically(
                &loading_path,
                &dummy_path,
                &DummySpec {
                    workshop_id: wid,
                    original_hash: meta.content_hash,
                    original_size: size,
                    original_time_updated: remote_ts,
                },
            )?;
            if let Some(ref w_path) = issue.workshop_path {
                let old_path = Path::new(w_path);
                if old_path != dummy_path && old_path.exists() {
                    fs::remove_file(old_path)?;
                }
            }
        }
        ReconcileAction::KeepWorkshopDeleteLoading => {
            if let Some(ref l_path) = issue.loading_path {
                let p = Path::new(l_path);
                if p.exists() {
                    fs::remove_file(p)?;
                }
            }
        }
        ReconcileAction::ImportToLoadingAndCreateDummy => {
            let workshop_path = issue
                .workshop_path
                .as_deref()
                .map(PathBuf::from)
                .ok_or_else(|| CoreError::InvalidInput("缺少 workshop_path".into()))?;
            let wid = issue
                .workshop_id
                .ok_or_else(|| CoreError::InvalidInput("缺少 workshop_id".into()))?;
            if config.loading_dir.trim().is_empty() {
                return Err(CoreError::InvalidInput("未配置 loading_dir".into()));
            }

            let meta = extract_addon_metadata(&workshop_path, image_cache_dir);
            if let Some(err) = meta.error {
                return Err(CoreError::Vpk(err));
            }
            let title = meta
                .addon_info
                .get("addontitle")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("Workshop_{wid}"));
            let target_name = build_target_vpk_name(Some(wid), None, &title, &config.rename_rules);
            let dest_path = Path::new(&config.loading_dir).join(&target_name);
            let disabled_dest_path = append_os_suffix(&dest_path, ".disabled");
            if dest_path.exists() || disabled_dest_path.exists() {
                return Err(CoreError::Conflict(format!(
                    "目标 loading 目录已存在同名文件: {}",
                    dest_path.display()
                )));
            }
            let size = fs::metadata(&workshop_path)?.len();
            let remote_ts = db
                .get_workshop_snapshot(wid)?
                .and_then(|s| s.time_updated);

            move_or_copy_replace(&workshop_path, &dest_path)?;

            if config.enable_dummy_bypass && !config.workshop_dir.trim().is_empty() {
                let dummy_path = Path::new(&config.workshop_dir).join(format!("{wid}.vpk"));
                write_dummy_atomically(
                    &dest_path,
                    &dummy_path,
                    &DummySpec {
                        workshop_id: wid,
                        original_hash: meta.content_hash,
                        original_size: size,
                        original_time_updated: remote_ts,
                    },
                )?;
            }
        }
        ReconcileAction::RemoveOrphanDummy => {
            for p in [&issue.workshop_path, &issue.loading_path].into_iter().flatten() {
                let path = Path::new(p);
                if path.exists() {
                    fs::remove_file(path)?;
                }
            }
        }
        ReconcileAction::EnableWorkshopFile => {
            let w_path = issue
                .workshop_path
                .as_deref()
                .ok_or_else(|| CoreError::InvalidInput("缺少 workshop_path".into()))?;
            if let Some(enabled_str) = strip_suffix_ascii_case(w_path, ".disabled") {
                let target = Path::new(enabled_str);
                if target.exists() {
                    return Err(CoreError::Conflict(format!(
                        "启用文件冲突，已存在 {}",
                        target.display()
                    )));
                }
                fs::rename(w_path, target)?;
            }
        }
    }

    db.mark_issue_resolved(issue_id)?;
    Ok(())
}

fn move_or_copy_replace(src: &Path, dst: &Path) -> Result<(), CoreError> {
    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let tmp = append_os_suffix(dst, ".l4a_tmp");
    let bak = append_os_suffix(dst, ".l4a_bak");
    if tmp.exists() {
        let _ = fs::remove_file(&tmp);
    }
    if bak.exists() {
        let _ = fs::remove_file(&bak);
    }

    let moved_in_stage = fs::rename(src, &tmp).is_ok();
    if !moved_in_stage {
        if let Err(err) = fs::copy(src, &tmp) {
            let _ = fs::remove_file(&tmp);
            return Err(err.into());
        }
    }

    let had_dst = dst.exists();
    if had_dst {
        if let Err(err) = fs::rename(dst, &bak) {
            if moved_in_stage && tmp.exists() && !src.exists() {
                let _ = fs::rename(&tmp, src);
            } else {
                let _ = fs::remove_file(&tmp);
            }
            return Err(err.into());
        }
    }

    if let Err(err) = fs::rename(&tmp, dst) {
        if had_dst && bak.exists() {
            let _ = fs::rename(&bak, dst);
        }
        if moved_in_stage && tmp.exists() && !src.exists() {
            let _ = fs::rename(&tmp, src);
        } else {
            let _ = fs::remove_file(&tmp);
        }
        return Err(err.into());
    }

    if had_dst && bak.exists() {
        let _ = fs::remove_file(&bak);
    }
    if !moved_in_stage && src.exists() {
        fs::remove_file(src)?;
    }
    Ok(())
}
