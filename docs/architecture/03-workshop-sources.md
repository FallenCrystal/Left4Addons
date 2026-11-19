# 03. 创意工坊多源管道与 Web Scraper 占位符

## 1. `WorkshopProvider` 抽象

在 `crates/l4a-workshop/src/provider.rs` 中，所有数据源统一实现 `WorkshopProvider` Trait：

```rust
#[async_trait]
pub trait WorkshopProvider: Send + Sync {
    fn name(&self) -> &str;
    fn source_kind(&self) -> MetadataSource;
    fn capabilities(&self) -> ProviderCapabilities;
    async fn fetch_items(&self, ids: &[WorkshopId]) -> Result<Vec<WorkshopFetchOutcome>, WorkshopError>;
    async fn fetch_collection(&self, id: WorkshopId) -> Result<WorkshopCollectionRecord, WorkshopError>;
    async fn search(&self, query: &SearchQuery) -> Result<SearchResponse, WorkshopError>;
}
```

## 2. 内置数据源与能力边界

| 数据源 | 实现类 | `dependencies` | `file_url` | 下架检测 | 说明 |
| --- | --- | --- | --- | --- | --- |
| 自托管后端 | `CatalogBackendProvider` | 是 | 是 | 是 | 默认优先（低延迟、聚合多玩家核验） |
| Steamworks SDK | `SteamBridgeProvider` | 是 | **否** | 部分 | 无 HTTP 限流，能准确给出依赖树，但会占用游戏进程身份触发 VAC 提示 |
| Steam Web API | `SteamWebApiProvider` | **否** | 是 | 是 (`result=9/15`) | 免鉴权公开接口，能拿到 CDN 直链与下架状态 |
| 网页爬虫 (占位) | `WebScraperPlaceholderProvider` | 预留 | 预留 | 预留 | **当前严格占位**，调用返回 `WorkshopError::ScraperNotImplemented` |

## 3. 智能合并路由 (`WorkshopResolver`)

v1 中 Steamworks SDK 查到模组但缺少 `file_url` 时不得不额外写散落的回落代码。v2 中 `WorkshopResolver` 按配置顺序依次查询：
1. 未命中的 ID 自动向下游 Provider 回落；
2. 已命中但缺少 `file_url` 或 `dependencies` 的记录，会向具备对应能力 (`provides_file_url` / `provides_dependencies`) 的后续 Provider 请求并**原地合并缺失字段**；
3. 遇 `WorkshopError::ScraperNotImplemented` 静默跳过，确保占位符接入路由器后不影响生产路径。

## 4. 未来接入 Web 抓取的规范接口

位于 `crates/l4a-workshop/src/providers/web_scraper_placeholder.rs` 的 `WorkshopWebScraper` Trait 已定义好：
- `scrape_item_page(WorkshopId)`
- `scrape_browse_page(&SearchQuery)`
- `scrape_collection_page(WorkshopId)`
- `scrape_tag_tree()`

未来只需在独立模块或实现类中实现 `WorkshopWebScraper` 并注入 `WebScraperPlaceholderProvider::new(...)`，即可在不改 `l4a-core` 和 `src-tauri` 任何逻辑的前提下启用精准网页搜索与分类树。
