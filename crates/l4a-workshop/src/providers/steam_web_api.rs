use crate::error::WorkshopError;
use crate::provider::{ProviderCapabilities, WorkshopProvider};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use l4a_mirror::MirrorRouter;
use l4a_protocol::{
    MetadataSource, WorkshopCollectionRecord, WorkshopFetchOutcome, WorkshopFileType, WorkshopId,
    WorkshopItemRecord, WorkshopItemStatus,
};
use serde_json::Value;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_STEAM_API_BASE: &str = "https://api.steampowered.com";
const STEAM_WEB_API_BATCH_SIZE: usize = 100;

pub struct SteamWebApiProvider {
    base_url: String,
    mirror: Arc<MirrorRouter>,
    timeout: Duration,
}

impl SteamWebApiProvider {
    pub fn new(mirror: Arc<MirrorRouter>) -> Self {
        Self::with_base_url(DEFAULT_STEAM_API_BASE, mirror)
    }

    pub fn with_base_url(base_url: impl Into<String>, mirror: Arc<MirrorRouter>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            mirror,
            timeout: Duration::from_secs(10),
        }
    }
}

#[async_trait]
impl WorkshopProvider for SteamWebApiProvider {
    fn name(&self) -> &str {
        "steam-web-api"
    }

    fn source_kind(&self) -> MetadataSource {
        MetadataSource::SteamWebApi
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            fetch_items: true,
            fetch_collection: true,
            search: false,
            provides_file_url: true,
            provides_dependencies: false,
            supports_status_tombstone: true,
        }
    }

    async fn fetch_items(
        &self,
        ids: &[WorkshopId],
    ) -> Result<Vec<WorkshopFetchOutcome>, WorkshopError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let url = format!(
            "{}/ISteamRemoteStorage/GetPublishedFileDetails/v1/",
            self.base_url
        );
        let (client, _) = self
            .mirror
            .build_client_for(&url, Some(self.timeout))
            .map_err(|e| WorkshopError::Network(e.to_string()))?;

        let now = Utc::now();
        let mut outcomes = Vec::with_capacity(ids.len());

        for chunk in ids.chunks(STEAM_WEB_API_BATCH_SIZE) {
            let mut params = Vec::with_capacity(chunk.len() + 1);
            params.push(("itemcount".to_string(), chunk.len().to_string()));
            for (idx, id) in chunk.iter().enumerate() {
                params.push((format!("publishedfileids[{idx}]"), id.to_string()));
            }

            let (req, _) = self.mirror.prepare_post(&client, &url);
            let resp = req
                .form(&params)
                .send()
                .await
                .map_err(|e| WorkshopError::Network(e.to_string()))?;

            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(WorkshopError::HttpStatus {
                    status: status.as_u16(),
                    body,
                });
            }

            let json: Value = resp
                .json()
                .await
                .map_err(|e| WorkshopError::Parse(e.to_string()))?;
            let details = json
                .get("response")
                .and_then(|r| r.get("publishedfiledetails"))
                .and_then(Value::as_array)
                .ok_or_else(|| WorkshopError::Parse("缺少 publishedfiledetails 字段".into()))?;

            for entry in details {
                let Some(wid) = entry
                    .get("publishedfileid")
                    .and_then(Value::as_str)
                    .and_then(|s| WorkshopId::from_str(s).ok())
                else {
                    continue;
                };

                match entry.get("result").and_then(Value::as_i64) {
                    Some(1) => {
                        let title = entry
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let description = entry
                            .get("description")
                            .and_then(Value::as_str)
                            .map(ToString::to_string);
                        let creator_steam_id = entry
                            .get("creator")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(ToString::to_string);
                        let preview_url = entry
                            .get("preview_url")
                            .and_then(Value::as_str)
                            .and_then(crate::resolver::normalize_workshop_url);
                        let file_url = entry
                            .get("file_url")
                            .and_then(Value::as_str)
                            .and_then(crate::resolver::normalize_workshop_url);
                        let file_size =
                            entry.get("file_size").and_then(Value::as_u64).or_else(|| {
                                entry.get("file_size").and_then(Value::as_str)?.parse().ok()
                            });
                        let time_created = entry
                            .get("time_created")
                            .and_then(Value::as_i64)
                            .or_else(|| {
                                entry
                                    .get("time_created")
                                    .and_then(Value::as_str)?
                                    .parse()
                                    .ok()
                            });
                        let time_updated = entry
                            .get("time_updated")
                            .and_then(Value::as_i64)
                            .or_else(|| {
                                entry
                                    .get("time_updated")
                                    .and_then(Value::as_str)?
                                    .parse()
                                    .ok()
                            });
                        let tags = entry
                            .get("tags")
                            .and_then(Value::as_array)
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.get("tag").and_then(Value::as_str))
                                    .map(ToString::to_string)
                                    .collect()
                            })
                            .unwrap_or_default();

                        let fetched_at = Utc
                            .timestamp_opt(now.timestamp(), 0)
                            .single()
                            .unwrap_or(now);

                        outcomes.push(WorkshopFetchOutcome::Found {
                            item: WorkshopItemRecord {
                                workshop_id: wid,
                                file_type: WorkshopFileType::Item,
                                status: WorkshopItemStatus::Active,
                                title,
                                description,
                                creator_steam_id,
                                creator_name: None,
                                preview_url,
                                previews: Vec::new(),
                                file_url,
                                file_size,
                                time_created,
                                time_updated,
                                tags,
                                dependencies: Vec::new(),
                                collection_children: Vec::new(),
                                subscriptions: entry.get("subscriptions").and_then(Value::as_u64),
                                favorites: entry.get("favorited").and_then(Value::as_u64),
                                source: MetadataSource::SteamWebApi,
                                fetched_at,
                            },
                        });
                    }
                    Some(9) => outcomes.push(WorkshopFetchOutcome::Removed {
                        workshop_id: wid,
                        source: MetadataSource::SteamWebApi,
                        observed_at: now,
                        reason: Some("Steam Web API result=9 (FileNotFound)".into()),
                    }),
                    Some(15) => outcomes.push(WorkshopFetchOutcome::PrivateOrUnavailable {
                        workshop_id: wid,
                        source: MetadataSource::SteamWebApi,
                        observed_at: now,
                        reason: Some("Steam Web API result=15 (AccessDenied)".into()),
                    }),
                    // 超时或缺少状态的条目由其他数据源补查，或留待下次重试。
                    _ => {}
                }
            }
        }

        Ok(outcomes)
    }

    async fn fetch_collection(
        &self,
        collection_id: WorkshopId,
    ) -> Result<WorkshopCollectionRecord, WorkshopError> {
        let url = format!(
            "{}/ISteamRemoteStorage/GetCollectionDetails/v1/",
            self.base_url
        );
        let (client, _) = self
            .mirror
            .build_client_for(&url, Some(self.timeout))
            .map_err(|e| WorkshopError::Network(e.to_string()))?;

        let params = vec![
            ("collectioncount".to_string(), "1".to_string()),
            ("publishedfileids[0]".to_string(), collection_id.to_string()),
        ];

        let (req, _) = self.mirror.prepare_post(&client, &url);
        let resp = req
            .form(&params)
            .send()
            .await
            .map_err(|e| WorkshopError::Network(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(WorkshopError::HttpStatus {
                status: status.as_u16(),
                body,
            });
        }

        let json: Value = resp
            .json()
            .await
            .map_err(|e| WorkshopError::Parse(e.to_string()))?;
        let details = json
            .get("response")
            .and_then(|r| r.get("collectiondetails"))
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .ok_or_else(|| WorkshopError::Parse("缺少 collectiondetails".into()))?;

        let result_code = details
            .get("result")
            .and_then(Value::as_i64)
            .ok_or_else(|| WorkshopError::Parse("collectiondetails 缺少 result 字段".into()))?;
        let col_status = match result_code {
            1 => WorkshopItemStatus::Active,
            9 => WorkshopItemStatus::Removed,
            _ => WorkshopItemStatus::PrivateOrUnavailable,
        };

        let child_ids: Vec<WorkshopId> = details
            .get("children")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| c.get("publishedfileid").and_then(Value::as_str))
                    .filter_map(|s| WorkshopId::from_str(s).ok())
                    .collect()
            })
            .unwrap_or_default();

        let outcomes = if col_status == WorkshopItemStatus::Active {
            let mut query_ids = vec![collection_id];
            query_ids.extend(child_ids.iter().copied());
            self.fetch_items(&query_ids).await?
        } else {
            Vec::new()
        };
        let mut by_id: HashMap<WorkshopId, WorkshopItemRecord> = outcomes
            .into_iter()
            .filter_map(|o| match o {
                WorkshopFetchOutcome::Found { item } => Some((item.workshop_id, item)),
                _ => None,
            })
            .collect();

        let col_meta = by_id.remove(&collection_id);
        let items: Vec<WorkshopItemRecord> =
            child_ids.iter().filter_map(|id| by_id.remove(id)).collect();

        Ok(WorkshopCollectionRecord {
            collection_id,
            status: col_status,
            title: col_meta.as_ref().map(|m| m.title.clone()),
            description: col_meta.as_ref().and_then(|m| m.description.clone()),
            preview_url: col_meta.as_ref().and_then(|m| m.preview_url.clone()),
            child_ids,
            items,
            source: MetadataSource::SteamWebApi,
            fetched_at: Utc::now(),
        })
    }
}
