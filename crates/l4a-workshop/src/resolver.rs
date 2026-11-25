use crate::error::WorkshopError;
use crate::provider::WorkshopProvider;
use l4a_protocol::{
    MetadataSource, SearchQuery, SearchResponse, WorkshopCollectionRecord, WorkshopFetchOutcome,
    WorkshopId, WorkshopItemRecord,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct SourceRoutingPolicy {
    pub enrich_missing_file_url: bool,
    pub enrich_missing_dependencies: bool,
}

impl Default for SourceRoutingPolicy {
    fn default() -> Self {
        Self {
            enrich_missing_file_url: true,
            enrich_missing_dependencies: true,
        }
    }
}

/// 统一创意工坊元数据路由器：按有序 Provider 回落，并智能合并缺失字段（例如 SDK 依赖 + WebAPI 直链）。
pub struct WorkshopResolver {
    providers: Vec<Arc<dyn WorkshopProvider>>,
    policy: SourceRoutingPolicy,
}

impl WorkshopResolver {
    pub fn new(providers: Vec<Arc<dyn WorkshopProvider>>, policy: SourceRoutingPolicy) -> Self {
        Self { providers, policy }
    }

    pub fn providers(&self) -> &[Arc<dyn WorkshopProvider>] {
        &self.providers
    }

    pub fn without_steamworks_sdk(&self) -> Self {
        let providers = self
            .providers
            .iter()
            .filter(|provider| provider.source_kind() != MetadataSource::SteamworksSdk)
            .cloned()
            .collect();
        Self::new(providers, self.policy.clone())
    }

    pub async fn fetch_items(
        &self,
        ids: &[WorkshopId],
    ) -> Result<(Vec<WorkshopFetchOutcome>, Vec<String>), WorkshopError> {
        if ids.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }

        let mut remaining: Vec<WorkshopId> = ids.to_vec();
        let mut results: HashMap<WorkshopId, WorkshopFetchOutcome> = HashMap::new();
        let mut deps_authoritative: HashSet<WorkshopId> = HashSet::new();
        let mut warnings = Vec::new();

        for (idx, provider) in self.providers.iter().enumerate() {
            let caps = provider.capabilities();
            if remaining.is_empty()
                && !self.needs_enrichment(&results, &deps_authoritative, &self.providers[idx..])
            {
                break;
            }

            let mut query_ids: HashSet<WorkshopId> = remaining.iter().copied().collect();
            if self.policy.enrich_missing_file_url && caps.provides_file_url {
                for (id, outcome) in &results {
                    if let WorkshopFetchOutcome::Found { item } = outcome {
                        if item.file_url.is_none() {
                            query_ids.insert(*id);
                        }
                    }
                }
            }
            if self.policy.enrich_missing_dependencies && caps.provides_dependencies {
                for (id, outcome) in &results {
                    if let WorkshopFetchOutcome::Found { item } = outcome {
                        if item.dependencies.is_empty() && !deps_authoritative.contains(id) {
                            query_ids.insert(*id);
                        }
                    }
                }
            }
            if provider.source_kind() == MetadataSource::SteamworksSdk {
                for (id, outcome) in &results {
                    if matches!(outcome, WorkshopFetchOutcome::PrivateOrUnavailable { .. }) {
                        query_ids.insert(*id);
                    }
                }
            }

            if query_ids.is_empty() {
                continue;
            }

            let batch: Vec<WorkshopId> = query_ids.into_iter().collect();
            match provider.fetch_items(&batch).await {
                Ok(outcomes) => {
                    for outcome in outcomes {
                        let id = outcome.workshop_id();
                        if caps.provides_dependencies
                            && matches!(outcome, WorkshopFetchOutcome::Found { .. })
                        {
                            deps_authoritative.insert(id);
                        }
                        match (results.get_mut(&id), outcome) {
                            (
                                Some(WorkshopFetchOutcome::Found { item: existing }),
                                WorkshopFetchOutcome::Found { item: incoming },
                            ) => {
                                merge_item_fields(existing, incoming);
                            }
                            (
                                Some(slot @ WorkshopFetchOutcome::PrivateOrUnavailable { .. }),
                                found @ WorkshopFetchOutcome::Found { .. },
                            ) => {
                                // 登录态 Steamworks SDK 对仅好友可见或私密条目可返回真实元数据，覆盖未登录来源的不可用结果。
                                *slot = found;
                            }
                            (Some(_), WorkshopFetchOutcome::Found { .. }) => {
                                // 已有明确 Removed 时不随意用其它来源的 Found 覆盖
                            }
                            (None, other) => {
                                results.insert(id, other);
                            }
                            (
                                Some(slot @ WorkshopFetchOutcome::Found { .. }),
                                tombstone @ WorkshopFetchOutcome::Removed { .. },
                            ) => {
                                let from_catalog_cache = matches!(
                                    slot,
                                    WorkshopFetchOutcome::Found { item }
                                        if item.source == MetadataSource::CatalogServer
                                );
                                if from_catalog_cache {
                                    *slot = tombstone;
                                }
                            }
                            (
                                Some(WorkshopFetchOutcome::Found { .. }),
                                WorkshopFetchOutcome::PrivateOrUnavailable { .. },
                            ) => {}
                            (Some(slot), other) => {
                                *slot = other;
                            }
                        }
                    }
                    remaining.retain(|id| !results.contains_key(id));
                }
                Err(WorkshopError::ScraperNotImplemented(_)) => continue,
                Err(err) => {
                    warnings.push(format!("{}: {err}", provider.name()));
                }
            }
        }

        let ordered = ids
            .iter()
            .filter_map(|id| {
                let mut outcome = results.remove(id)?;
                if let WorkshopFetchOutcome::Found { item } = &mut outcome {
                    normalize_item_urls(item);
                }
                Some(outcome)
            })
            .collect();
        Ok((ordered, warnings))
    }

    pub async fn fetch_collection(
        &self,
        collection_id: WorkshopId,
    ) -> Result<(WorkshopCollectionRecord, Vec<String>), WorkshopError> {
        let mut warnings = Vec::new();
        for provider in &self.providers {
            if !provider.capabilities().fetch_collection {
                continue;
            }
            match provider.fetch_collection(collection_id).await {
                Ok(mut record) => {
                    record.preview_url = record
                        .preview_url
                        .as_deref()
                        .and_then(normalize_workshop_url);
                    for item in &mut record.items {
                        normalize_item_urls(item);
                    }
                    return Ok((record, warnings));
                }
                Err(WorkshopError::ScraperNotImplemented(_)) => continue,
                Err(err) => warnings.push(format!("{}: {err}", provider.name())),
            }
        }
        Err(WorkshopError::Provider(format!(
            "所有数据源均无法查询合集 {collection_id}: {}",
            warnings.join("; ")
        )))
    }

    pub async fn search(
        &self,
        query: &SearchQuery,
    ) -> Result<(SearchResponse, Vec<String>), WorkshopError> {
        let mut warnings = Vec::new();
        let mut attempted = false;
        let mut last_error: Option<WorkshopError> = None;
        for provider in &self.providers {
            if !provider.capabilities().search {
                continue;
            }
            match provider.search(query).await {
                Ok(mut resp) => {
                    for item in &mut resp.items {
                        normalize_item_urls(item);
                    }
                    return Ok((resp, warnings));
                }
                Err(WorkshopError::ScraperNotImplemented(_)) => continue,
                Err(err) => {
                    attempted = true;
                    warnings.push(format!("{}: {err}", provider.name()));
                    last_error = Some(err);
                }
            }
        }
        if !attempted {
            return Err(WorkshopError::Unsupported(
                "当前启用数据源不支持搜索".to_string(),
            ));
        }
        if warnings.len() == 1 {
            if let Some(err) = last_error {
                return Err(err);
            }
        }
        Err(WorkshopError::Provider(format!(
            "所有搜索数据源均失败: {}",
            warnings.join("; ")
        )))
    }

    fn needs_enrichment(
        &self,
        results: &HashMap<WorkshopId, WorkshopFetchOutcome>,
        deps_authoritative: &HashSet<WorkshopId>,
        remaining_providers: &[Arc<dyn WorkshopProvider>],
    ) -> bool {
        let can_fill_url = self.policy.enrich_missing_file_url
            && remaining_providers
                .iter()
                .any(|p| p.capabilities().provides_file_url);
        let can_fill_deps = self.policy.enrich_missing_dependencies
            && remaining_providers
                .iter()
                .any(|p| p.capabilities().provides_dependencies);
        let has_sdk = remaining_providers
            .iter()
            .any(|p| p.source_kind() == MetadataSource::SteamworksSdk);

        results.iter().any(|(id, outcome)| match outcome {
            WorkshopFetchOutcome::Found { item } => {
                (can_fill_url && item.file_url.is_none())
                    || (can_fill_deps
                        && item.dependencies.is_empty()
                        && !deps_authoritative.contains(id))
            }
            WorkshopFetchOutcome::PrivateOrUnavailable { .. } => has_sdk,
            WorkshopFetchOutcome::Removed { .. } => false,
        })
    }
}

fn merge_item_fields(existing: &mut WorkshopItemRecord, incoming: WorkshopItemRecord) {
    let incoming_newer = match (existing.time_updated, incoming.time_updated) {
        (Some(e), Some(i)) => i > e,
        (None, Some(_)) => true,
        _ => false,
    };
    if incoming_newer {
        if !incoming.title.trim().is_empty() {
            existing.title = incoming.title.clone();
        }
        if incoming.time_updated.is_some() {
            existing.time_updated = incoming.time_updated;
        }
        if incoming.file_size.is_some() {
            existing.file_size = incoming.file_size;
        }
        if incoming.file_url.is_some() {
            existing.file_url = incoming.file_url.clone();
        }
        if matches!(
            incoming.source,
            MetadataSource::SteamworksSdk
                | MetadataSource::SteamKit
                | MetadataSource::CatalogServer
        ) && !incoming.dependencies.is_empty()
        {
            existing.dependencies = incoming.dependencies.clone();
        }
    }

    if existing.title.trim().is_empty() && !incoming.title.trim().is_empty() {
        existing.title = incoming.title;
    }
    if existing.description.is_none() {
        existing.description = incoming.description;
    }
    if existing.creator_steam_id.is_none() {
        existing.creator_steam_id = incoming.creator_steam_id;
    }
    if existing.creator_name.is_none() {
        existing.creator_name = incoming.creator_name;
    }
    if existing.preview_url.is_none() {
        existing.preview_url = incoming.preview_url;
    }
    if existing.previews.is_empty() && !incoming.previews.is_empty() {
        existing.previews = incoming.previews;
    }
    if existing.file_url.is_none() {
        existing.file_url = incoming.file_url;
    }
    if existing.file_size.is_none() {
        existing.file_size = incoming.file_size;
    }
    if existing.time_created.is_none() {
        existing.time_created = incoming.time_created;
    }
    if existing.time_updated.is_none() {
        existing.time_updated = incoming.time_updated;
    }
    if existing.tags.is_empty() && !incoming.tags.is_empty() {
        existing.tags = incoming.tags;
    }
    if existing.dependencies.is_empty() && !incoming.dependencies.is_empty() {
        existing.dependencies = incoming.dependencies;
    }
    if existing.collection_children.is_empty() && !incoming.collection_children.is_empty() {
        existing.collection_children = incoming.collection_children;
    }
}

/// 清理工坊链接，保留显式协议供请求端按镜像和 HTTPS 设置处理。
pub fn normalize_workshop_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.starts_with("//") {
        Some(format!("https:{trimmed}"))
    } else {
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }
}

fn normalize_item_urls(item: &mut WorkshopItemRecord) {
    item.preview_url = item.preview_url.as_deref().and_then(normalize_workshop_url);
    for preview in &mut item.previews {
        preview.url = preview.url.as_deref().and_then(normalize_workshop_url);
    }
    item.file_url = item.file_url.as_deref().and_then(normalize_workshop_url);
}
