use crate::error::WorkshopError;
use crate::provider::{ProviderCapabilities, WorkshopProvider};
use async_trait::async_trait;
use chrono::Utc;
use l4a_logging::http;
use l4a_mirror::{MirrorRouter, RedirectRequestOptions};
use l4a_protocol::{
    BatchItemsRequest, BatchItemsResponse, MetadataSource, SearchQuery, SearchResponse,
    WorkshopCollectionRecord, WorkshopFetchOutcome, WorkshopId, WorkshopItemStatus,
};
use reqwest::Method;
use std::sync::Arc;
use std::time::Duration;

// At the maximum 15s Steam timeout, the catalog server can spend up to 2s waiting for a
// local search slot and 5s executing the SQLite fallback, plus response transfer margin.
pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(30);

/// 接入自托管 `l4a-server` 创意工坊信息管理后端的客户端 Provider。
pub struct CatalogBackendProvider {
    base_url: String,
    mirror: Arc<MirrorRouter>,
    timeout: Duration,
    search_timeout: Duration,
}

impl CatalogBackendProvider {
    pub fn new(base_url: impl Into<String>, mirror: Arc<MirrorRouter>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            mirror,
            timeout: Duration::from_secs(15),
            search_timeout: SEARCH_TIMEOUT,
        }
    }

    pub fn search_timeout(&self) -> Duration {
        self.search_timeout
    }
}

#[async_trait]
impl WorkshopProvider for CatalogBackendProvider {
    fn name(&self) -> &str {
        "catalog-server"
    }

    fn source_kind(&self) -> MetadataSource {
        MetadataSource::CatalogServer
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            fetch_items: true,
            fetch_collection: true,
            search: true,
            provides_file_url: true,
            provides_dependencies: true,
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
        let url = format!("{}/api/v1/items/batch", self.base_url);
        let payload = BatchItemsRequest {
            ids: ids.to_vec(),
            force_refresh: false,
        };
        let resp = self
            .mirror
            .send_with_redirects(
                Method::POST,
                &url,
                RedirectRequestOptions {
                    timeout: Some(self.timeout),
                    ..Default::default()
                },
                |req, _, method| {
                    if *method == Method::POST {
                        req.json(&payload)
                    } else {
                        req
                    }
                },
                |_| Ok::<(), WorkshopError>(()),
                |req, hop| http::send_redirect_hop(req, "catalog.items.batch", hop),
            )
            .await
            .map_err(|e| WorkshopError::Network(e.to_string()))?
            .response;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(WorkshopError::HttpStatus {
                status: status.as_u16(),
                body,
            });
        }

        let batch: BatchItemsResponse = resp
            .json()
            .await
            .map_err(|e| WorkshopError::Parse(e.to_string()))?;
        let now = Utc::now();

        let outcomes = batch
            .items
            .into_iter()
            .map(|mut item| {
                item.source = MetadataSource::CatalogServer;
                match item.status {
                    WorkshopItemStatus::Removed => WorkshopFetchOutcome::Removed {
                        workshop_id: item.workshop_id,
                        source: MetadataSource::CatalogServer,
                        observed_at: now,
                        reason: Some("Catalog Server 标记该条目已下架".to_string()),
                    },
                    WorkshopItemStatus::PrivateOrUnavailable => {
                        WorkshopFetchOutcome::PrivateOrUnavailable {
                            workshop_id: item.workshop_id,
                            source: MetadataSource::CatalogServer,
                            observed_at: now,
                            reason: Some("Catalog Server 标记该条目为私密或不可用".to_string()),
                        }
                    }
                    WorkshopItemStatus::Active | WorkshopItemStatus::Unknown => {
                        WorkshopFetchOutcome::Found { item }
                    }
                }
            })
            .collect();

        Ok(outcomes)
    }

    async fn fetch_collection(
        &self,
        collection_id: WorkshopId,
    ) -> Result<WorkshopCollectionRecord, WorkshopError> {
        let url = format!("{}/api/v1/collections/{collection_id}", self.base_url);
        let resp = self
            .mirror
            .send_with_redirects(
                Method::GET,
                &url,
                RedirectRequestOptions {
                    timeout: Some(self.timeout),
                    ..Default::default()
                },
                |req, _, _| req,
                |_| Ok::<(), WorkshopError>(()),
                |req, hop| http::send_redirect_hop(req, "catalog.collection", hop),
            )
            .await
            .map_err(|e| WorkshopError::Network(e.to_string()))?
            .response;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(WorkshopError::HttpStatus {
                status: status.as_u16(),
                body,
            });
        }

        resp.json::<WorkshopCollectionRecord>()
            .await
            .map_err(|e| WorkshopError::Parse(e.to_string()))
    }

    async fn search(&self, query: &SearchQuery) -> Result<SearchResponse, WorkshopError> {
        let url = format!("{}/api/v1/items/search", self.base_url);

        let page = query.page.max(1);
        let page_size = if query.page_size == 0 {
            30
        } else {
            query.page_size.clamp(1, 100)
        };
        let mut params = vec![
            ("page", page.to_string()),
            ("pageSize", page_size.to_string()),
        ];
        if let Some(ref q) = query.query {
            let trimmed = q.trim();
            if !trimmed.is_empty() {
                params.push(("q", trimmed.to_string()));
            }
        }
        if let Some(ref tag) = query.tag {
            let trimmed = tag.trim();
            if !trimmed.is_empty() {
                params.push(("tag", trimmed.to_string()));
            }
        }
        if let Some(status) = query.status {
            let status_str = serde_json::to_string(&status)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string();
            if !status_str.is_empty() {
                params.push(("status", status_str));
            }
        }
        if let Some(ref sort) = query.sort {
            if !sort.trim().is_empty() {
                params.push(("sort", sort.clone()));
            }
        }
        if let Some(ref lang) = query.language {
            let trimmed = lang.trim();
            if !trimmed.is_empty() {
                params.push(("lang", trimmed.to_string()));
            }
        }
        if let Some(ref target) = query.search_target {
            let trimmed = target.trim();
            if !trimmed.is_empty() {
                params.push(("searchTarget", trimmed.to_string()));
            }
        }
        if let Some(file_type) = query.file_type {
            let ft_str = serde_json::to_string(&file_type)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string();
            if !ft_str.is_empty() {
                params.push(("fileType", ft_str));
            }
        }

        let resp = self
            .mirror
            .send_with_redirects(
                Method::GET,
                &url,
                RedirectRequestOptions {
                    timeout: Some(self.search_timeout),
                    ..Default::default()
                },
                |req, hop, _| if hop == 0 { req.query(&params) } else { req },
                |_| Ok::<(), WorkshopError>(()),
                |req, hop| http::send_redirect_hop(req, "catalog.search", hop),
            )
            .await
            .map_err(|e| WorkshopError::Network(e.to_string()))?
            .response;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(WorkshopError::HttpStatus {
                status: status.as_u16(),
                body,
            });
        }

        resp.json::<SearchResponse>()
            .await
            .map_err(|e| WorkshopError::Parse(e.to_string()))
    }
}
