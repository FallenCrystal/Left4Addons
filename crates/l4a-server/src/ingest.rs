use crate::db::{ContributorAuthRecord, ServerRepo};
use l4a_protocol::{
    ConflictDecision, ItemSubmission, MetadataSource, SubmissionReceipt, TrustTier,
    WorkshopFileType, WorkshopItemRecord, WorkshopItemStatus,
};

const STEAM_CDN_HOST_ALLOWLIST: &[&str] = &[
    "steamusercontent.com",
    "steamstatic.com",
    "steampowered.com",
    "steamcommunity.com",
    "akamaihd.net",
];

/// 校验入库 URL 是否为合法 HTTPS 且属于受信任的 Steam CDN 域名白名单。
pub fn validate_steam_cdn_url(url_str: &str) -> Result<(), String> {
    let trimmed = url_str.trim();
    if trimmed.is_empty() {
        return Err("URL 不能为空".to_string());
    }
    let parsed = url::Url::parse(trimmed).map_err(|e| format!("URL 格式非法: {e}"))?;
    if !parsed.scheme().eq_ignore_ascii_case("https") {
        return Err("只允许 https:// 协议的 Steam CDN URL".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("URL 不允许包含认证信息".to_string());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "URL 缺少有效主机名".to_string())?
        .trim_end_matches('.')
        .to_ascii_lowercase();

    let allowed = STEAM_CDN_HOST_ALLOWLIST
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")));
    if !allowed {
        return Err(format!("主机名 {host} 不在 Steam CDN 白名单内"));
    }
    Ok(())
}

/// 冲突裁决与可信数据入库引擎（全程单事务原子执行，消除 TOCTOU 竞态）。
pub fn ingest_submission(
    repo: &ServerRepo,
    contributor: &ContributorAuthRecord,
    sub: &ItemSubmission,
) -> anyhow::Result<SubmissionReceipt> {
    repo.with_transaction(|tx| ingest_submission_tx(tx, contributor, sub))
}

pub(crate) fn ingest_submission_tx(
    tx: &rusqlite::Connection,
    contributor: &ContributorAuthRecord,
    sub: &ItemSubmission,
) -> anyhow::Result<SubmissionReceipt> {
    // 规则 0：校验直链与封面图 URL，拦截非白名单域名投毒
    for (field, maybe_url) in [
        ("file_url", sub.file_url.as_deref()),
        ("preview_url", sub.preview_url.as_deref()),
    ] {
        if let Some(u) = maybe_url {
            if let Err(err_msg) = validate_steam_cdn_url(u) {
                let reason = format!(
                    "贡献者 {} 提交的 {} 未通过安全校验 ({})，已拦截并记入审计队列",
                    contributor.name, field, err_msg
                );
                ServerRepo::append_audit_log_tx(
                    tx,
                    sub.workshop_id,
                    Some(&contributor.id),
                    Some(&contributor.name),
                    ConflictDecision::FlaggedForReview,
                    &reason,
                    sub.status,
                    sub.time_updated,
                )?;
                return Ok(SubmissionReceipt {
                    workshop_id: sub.workshop_id,
                    decision: ConflictDecision::FlaggedForReview,
                    reason,
                });
            }
        }
    }

    let existing = ServerRepo::get_item_tx(tx, sub.workshop_id)?;
    let contributor_weight = contributor.trust_weight;

    let Some(current) = existing else {
        let record = submission_to_record(sub);
        ServerRepo::upsert_item_tx(tx, &record, contributor_weight, true)?;
        let reason = format!(
            "新录入创意工坊条目 (来源: {:?}, 贡献者权重: {})",
            sub.observed_source, contributor_weight
        );
        ServerRepo::append_audit_log_tx(
            tx,
            sub.workshop_id,
            Some(&contributor.id),
            Some(&contributor.name),
            ConflictDecision::Accepted,
            &reason,
            sub.status,
            sub.time_updated,
        )?;
        return Ok(SubmissionReceipt {
            workshop_id: sub.workshop_id,
            decision: ConflictDecision::Accepted,
            reason,
        });
    };

    let current_item = current.record;
    let current_weight = current.trust_score;

    // 规则 1：防低信任（含同级别 Community）恶意标下架/私密，或用 Unknown 覆盖 Active
    if current_item.status == WorkshopItemStatus::Active {
        if sub.status == WorkshopItemStatus::Unknown {
            let reason = format!(
                "拒绝以 Unknown 状态降级活跃条目 (提交者: {})",
                contributor.name
            );
            ServerRepo::append_audit_log_tx(
                tx,
                sub.workshop_id,
                Some(&contributor.id),
                Some(&contributor.name),
                ConflictDecision::RejectedLowerTrust,
                &reason,
                sub.status,
                sub.time_updated,
            )?;
            return Ok(SubmissionReceipt {
                workshop_id: sub.workshop_id,
                decision: ConflictDecision::RejectedLowerTrust,
                reason,
            });
        }

        if matches!(
            sub.status,
            WorkshopItemStatus::Removed | WorkshopItemStatus::PrivateOrUnavailable
        ) && contributor.trust_tier < TrustTier::Maintainer
        {
            let reason = format!(
                    "非维护者贡献者 ({}, {:?}, 权重 {}) 尝试将活跃条目改为 {:?}，已标记待管理员审阅而未立即下架",
                    contributor.name, contributor.trust_tier, contributor_weight, sub.status
                );
            ServerRepo::append_audit_log_tx(
                tx,
                sub.workshop_id,
                Some(&contributor.id),
                Some(&contributor.name),
                ConflictDecision::FlaggedForReview,
                &reason,
                sub.status,
                sub.time_updated,
            )?;
            return Ok(SubmissionReceipt {
                workshop_id: sub.workshop_id,
                decision: ConflictDecision::FlaggedForReview,
                reason,
            });
        }
    }

    // 规则 2：如果新提交拥有严格更低的信任权重，只允许无损补全无害展示字段，禁止注入 file_url / dependencies
    if contributor_weight < current_weight {
        if let Some(merged_item) = merge_missing_only(&current_item, sub) {
            ServerRepo::upsert_item_tx(tx, &merged_item, current_weight, false)?;
            let reason = format!(
                    "低权重 ({}) 补充了缺失展示字段（description/creator/tags/preview），保留原权威字段与权重 ({})",
                    contributor_weight, current_weight
                );
            ServerRepo::append_audit_log_tx(
                tx,
                sub.workshop_id,
                Some(&contributor.id),
                Some(&contributor.name),
                ConflictDecision::Merged,
                &reason,
                sub.status,
                sub.time_updated,
            )?;
            return Ok(SubmissionReceipt {
                workshop_id: sub.workshop_id,
                decision: ConflictDecision::Merged,
                reason,
            });
        }

        let reason = format!(
            "提交权重 ({}) 低于现有权威记录权重 ({})，拒绝覆盖高信任字段",
            contributor_weight, current_weight
        );
        ServerRepo::append_audit_log_tx(
            tx,
            sub.workshop_id,
            Some(&contributor.id),
            Some(&contributor.name),
            ConflictDecision::RejectedLowerTrust,
            &reason,
            sub.status,
            sub.time_updated,
        )?;
        return Ok(SubmissionReceipt {
            workshop_id: sub.workshop_id,
            decision: ConflictDecision::RejectedLowerTrust,
            reason,
        });
    }

    // 规则 3：同级权重时防范旧快照覆盖、缺失时间戳绕过及旧 Active 复活已 Removed 条目
    if contributor_weight == current_weight {
        if let (Some(cur_ts), Some(sub_ts)) = (current_item.time_updated, sub.time_updated) {
            if sub_ts < cur_ts {
                let reason = format!("提交时间戳 ({sub_ts}) 早于当前快照 ({cur_ts})，跳过旧快照");
                ServerRepo::append_audit_log_tx(
                    tx,
                    sub.workshop_id,
                    Some(&contributor.id),
                    Some(&contributor.name),
                    ConflictDecision::RejectedStale,
                    &reason,
                    sub.status,
                    sub.time_updated,
                )?;
                return Ok(SubmissionReceipt {
                    workshop_id: sub.workshop_id,
                    decision: ConflictDecision::RejectedStale,
                    reason,
                });
            }
        }

        if current_item.time_updated.is_some()
            && sub.time_updated.is_none()
            && sub.status == WorkshopItemStatus::Active
        {
            let reason =
                "现有记录含更新时间而本次 Active 提交缺少 time_updated，视为陈旧快照".to_string();
            ServerRepo::append_audit_log_tx(
                tx,
                sub.workshop_id,
                Some(&contributor.id),
                Some(&contributor.name),
                ConflictDecision::RejectedStale,
                &reason,
                sub.status,
                sub.time_updated,
            )?;
            return Ok(SubmissionReceipt {
                workshop_id: sub.workshop_id,
                decision: ConflictDecision::RejectedStale,
                reason,
            });
        }

        if matches!(
            current_item.status,
            WorkshopItemStatus::Removed | WorkshopItemStatus::PrivateOrUnavailable
        ) && sub.status == WorkshopItemStatus::Active
            && sub.time_updated.unwrap_or(0) <= current_item.time_updated.unwrap_or(0)
        {
            let reason = "不允许同权重旧/同时间戳 Active 快照复活已下架条目".to_string();
            ServerRepo::append_audit_log_tx(
                tx,
                sub.workshop_id,
                Some(&contributor.id),
                Some(&contributor.name),
                ConflictDecision::RejectedStale,
                &reason,
                sub.status,
                sub.time_updated,
            )?;
            return Ok(SubmissionReceipt {
                workshop_id: sub.workshop_id,
                decision: ConflictDecision::RejectedStale,
                reason,
            });
        }
    }

    // 规则 4：保留既有 Collection 类型，且仅当来源具备依赖能力或提交非空依赖时才替换依赖边
    let mut next_record = submission_to_record(sub);
    if current_item.file_type == WorkshopFileType::Collection {
        next_record.file_type = WorkshopFileType::Collection;
    }
    let replace_deps = !sub.dependencies.is_empty()
        || (source_provides_deps(sub.observed_source)
            && sub.time_updated.unwrap_or(0) >= current_item.time_updated.unwrap_or(0));
    ServerRepo::upsert_item_tx(tx, &next_record, contributor_weight, replace_deps)?;

    let reason = format!(
        "接受来自 {} (权重 {}) 的快照更新",
        contributor.name, contributor_weight
    );
    ServerRepo::append_audit_log_tx(
        tx,
        sub.workshop_id,
        Some(&contributor.id),
        Some(&contributor.name),
        ConflictDecision::Accepted,
        &reason,
        sub.status,
        sub.time_updated,
    )?;

    Ok(SubmissionReceipt {
        workshop_id: sub.workshop_id,
        decision: ConflictDecision::Accepted,
        reason,
    })
}

fn source_provides_deps(source: MetadataSource) -> bool {
    matches!(
        source,
        MetadataSource::SteamworksSdk | MetadataSource::WebScraper | MetadataSource::AdminOverride
    )
}

fn submission_to_record(sub: &ItemSubmission) -> WorkshopItemRecord {
    WorkshopItemRecord {
        workshop_id: sub.workshop_id,
        file_type: sub.file_type,
        status: sub.status,
        title: sub.title.clone(),
        description: sub.description.clone(),
        creator_steam_id: sub.creator_steam_id.clone(),
        creator_name: sub.creator_name.clone(),
        preview_url: sub.preview_url.clone(),
        file_url: sub.file_url.clone(),
        file_size: sub.file_size,
        time_created: sub.time_created,
        time_updated: sub.time_updated,
        tags: sub.tags.clone(),
        dependencies: sub.dependencies.clone(),
        collection_children: sub.collection_children.clone(),
        subscriptions: sub.subscriptions,
        favorites: sub.favorites,
        source: sub.observed_source,
        fetched_at: sub.observed_at,
    }
}

fn merge_missing_only(
    current: &WorkshopItemRecord,
    sub: &ItemSubmission,
) -> Option<WorkshopItemRecord> {
    let mut merged = current.clone();
    let mut changed = false;

    if merged.description.is_none() && sub.description.is_some() {
        merged.description = sub.description.clone();
        changed = true;
    }
    if merged.creator_name.is_none() && sub.creator_name.is_some() {
        merged.creator_name = sub.creator_name.clone();
        changed = true;
    }
    if merged.preview_url.is_none() && sub.preview_url.is_some() {
        merged.preview_url = sub.preview_url.clone();
        changed = true;
    }
    if merged.tags.is_empty() && !sub.tags.is_empty() {
        merged.tags = sub.tags.clone();
        changed = true;
    }

    changed.then_some(merged)
}
