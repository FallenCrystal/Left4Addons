# `l4a-server` REST API 规范 (`v1`)

默认监听：`127.0.0.1:8787`；Web 管理控制台：`GET /admin/`。

> 部署与 `catalog.toml` 配置说明请参见 [Catalog Server 介绍与部署指南](../guides/catalog-server-guide.md)，后端架构参见 [07. SteamKit CM 缓存后端设计](../architecture/07-catalog-backend.md)。

## 1. 公开查询接口

按 ID 查询默认支持 Read-Through 穿透缓存。本地未命中或超过 `sync.item_ttl_secs` 时，服务端会通过 SteamKit CM 核验。一次公开请求最多等待刷新 10 秒，合集和子条目共享该预算。刷新失败或超时后会尽量返回已有缓存。

### `GET /api/v1/health`
返回服务健康状态、缓存总条目数 (`items`) 与当前 Steam CM 连接状态 (`steamState`)。

### `GET /api/v1/items/{workshop_id}?refresh=false`
查询单个创意工坊条目（含前置依赖、画廊 `previews`、`fileUrl` 与上下架状态）。传入 `?refresh=true` 可强制绕过本地 TTL 向 Steam CM 重新核验。

### `POST /api/v1/items/batch`
请求体（单次最多 `200` 个 ID，自动去重；超限返回 `400 Bad Request`）：
```json
{
  "ids": ["2814104912", "2814105100"],
  "forceRefresh": false
}
```
响应：返回命中的 `items`（含已下架条目的墓碑状态）以及未能解析的 `missingIds`。

### `GET /api/v1/items/search?q=&tag=&status=&sort=&page=1&pageSize=30&lang=&searchTarget=&fileType=`
公开检索接口默认启用 **SteamKit CM + 本地 SQLite FTS5 混合搜索**（可在 `catalog.toml` `[search]` 配置关闭）。响应头 `X-L4A-Search-Mode` 会标明本次执行模式：`hybrid`、`local` 或 `local-fallback`（Steam CM 超时或失败时的优雅降级）。

- **混合检索与网页端一致性**：向 Steam CM `PublishedFile.QueryFiles` 发起检索时**不限制 `creator_appid`**（兼容《求生之路 2》Authoring Tools `563` 上传的绝大部分战役与地图），从而在服务端直接复刻创意工坊网页端的搜索相关性与 `file_url` / 依赖 / 画廊返回，无需爬取网页 HTML。
- **多语言检索 (`lang` / `Accept-Language`)**：按 `lang` 查询参数 → HTTP `Accept-Language` 请求头（按 `q` 权重）→ 配置项 `search.default_language` 的优先级映射 Steam `ELanguage`（如 `en`/`english`→`0`、`zh-CN`/`schinese`→`6`、`zh-TW`/`tchinese`→`7`、`ja`→`10`、`ko`→`4`、`ru`→`8`），复刻 Steam 网页端随语言切换搜索结果的体验。
- **扩展筛选与排序**：
  - `searchTarget`：支持 `all`（默认，全字段）、`title`（仅标题）、`description`（仅简介）；
  - `fileType`：支持 `item`（组件，默认）与 `collection`（合集）；
  - `sort`：支持 `relevance`、`trend`、`toprated`、`subscriptions`（及 `totalprofiles` / `popular`）、`updated`、`newest`、`updated_asc`、`title`（后两项及 `removed` / `private_or_unavailable` 状态自动走本地索引 `local` 模式）。
- **本地能力增强与隔离保护**：本地 SQLite FTS5 索引（中英文分词、驼峰/忽略空格复合词、中文标题拼音、`AdminOverride` 人工覆写与直链、下架墓碑）会与 Steam CM 结果智能去重融合；按需搜索暖缓存（英文走 `RefreshUnindexedCache`，非英文走 `InsertOnlyCache`）仅写入按 ID 缓存而不纳入公开目录，本地已知未列出 (`is_listed = 0`) 条目不会从公开搜索泄露，私密 (`private_or_unavailable`) 条目仅在本地墓碑中保留脱敏占位信息（不泄露原始私密标题、简介或直链），且 Steam 实时搜索中的私密/未列出条目会被过滤。
- **精确 ID / 链接查询**：开启 `sync.enable_read_through` 后，传入 Steam Workshop 链接或 6 位以上纯数字 ID 时，若本地未收录且未在本地标记为受限/覆写，会自动通过 `GetDetails` 穿透补全公开条目。

常规混合检索在去重后计算总数和分页。普通查询的 Steam 命中数超过 2400 时，整次查询使用 Steam 的排序、分页和总数，仍会应用本地覆写及可见性过滤。Steam 超时或失败时仍可回退到本地目录。

> `GET /api/v1/admin/items/search` 保持**纯本地 SQLite 缓存检索**，供管理员检视已缓存条目与墓碑，不触发上游 Steam CM 请求。

### `GET /api/v1/collections/{collection_id}?refresh=false`
查询合集元数据及其子条目明细（若目标或子条目缺失会自动穿透拉取并补全 `items`；若目标 ID 非合集类型返回 `404`）。

### `GET /api/v1/filter/info`
查询内容过滤概率推理服务的公开能力信息 (`CatalogFilterPublicInfo`)：
```json
{
  "enabled": true,
  "ready": true,
  "modelName": "Left4FiltersModel",
  "modelVersion": "1.0.0",
  "maxImages": 2,
  "scoringRevision": "a1b2c3d4e5f60718"
}
```
- 当服务端关闭图片模态 (`enable_image = false`) 时，`maxImages` 返回 `0`。
- `scoringRevision` 为当前服务端评分配置与模型标识的摘要。外接节点可通过 `/health` 提供 `version` 或 `scoringRevision`。对于未报告版本的官方 Left4Filters 服务，Catalog 检查其是否提供尸体识别分数。后端未就绪或禁用时省略 `scoringRevision`。

### `POST /api/v1/filter/predict`
对创意工坊条目或给定多模态输入执行敏感度概率评估 (`FilterPredictRequest` -> `FilterProbabilities`)：
```json
{
  "workshopId": "2814104912",
  "title": "Optional Title",
  "description": "Optional Description",
  "tags": ["Survivors"],
  "imagesBase64": [],
  "imageUrls": ["https://images.steamusercontent.com/ugc/..."]
}
```
- **仅返回纯概率 (`FilterProbabilities`)**：返回 `prob`、`goreProb`、`nsfwProb`、`scoringRevision` 以及可选单模态分项 (`textProb`、`textGoreProb`、`textNsfwProb`、`titleProb`、`descriptionProb`、`imageProb`、`imageGoreProb`、`imageNsfwProb`、`perImageProbs`)；**绝不发起任何外部 LLM 请求**，也不返回二值封禁结论。
- 支持尸体识别的模型可返回 `corpseProb` 和 `corpsePileProb`。任意输入图片达到识别阈值时，`corpseReviewFloor` 给出整体路由分数 `prob` 的复核下限。`prob` 位于该下限时，`fusionProb` 给出直判采用的融合分数。外接节点若只报告等于下限的整体路由分数，直判采用保守值 `0`。`corpseImageIndices` 给出已定位的命中图片下标，从 0 开始，按服务实际评分的图片顺序排列。血腥权重为零时停用尸体复核。
- **参数严格受服务端管控**：公开请求体不接受任何模型参数或校准权重覆盖；参数仅可通过 `catalog.toml` `[filter]` 或管理员接口 `PATCH /api/v1/admin/config/filter` 修改。
- 公开预测和管理员试算共用并发配额，JSON 请求体上限为 48 MiB。读取超时返回 `408 Request Timeout`，配额已满时返回 `429 Too Many Requests`。管理员试算在读取请求体前验证身份。
- **按 `workshopId` 补全与可见性隔离**：当传入 `workshopId` 且请求未携带标题/简介/图片时，仅在本地缓存存在**公开且正常可用 (`status == active` 且非未列出 `is_listed != 0`)** 的记录时读取其元数据与 Steam 预览图（最多 `maxImages` 张）；私密 (`private_or_unavailable`)、未列出或不存在的条目统一返回 `404 Not Found`，绝不泄露受限元数据。
- **SSRF 与载荷限制**：`imageUrls` 允许受信任的 Steam CDN 域名白名单（`media.steampowered.com`、`*.steamusercontent.com`、`*.steamstatic.com`、`steamuserimages*.akamaihd.net`、`steamcdn*.akamaihd.net` 以及 `cloud*.steampowered.com/ugc/*`），支持 `https://` 并将历史 `http://` Steam CDN 链接自动升级为 `https://` 后抓取；最多跟随 3 次 HTTP 重定向且每跳均须通过 Steam CDN 白名单校验，单图上限 `16 MiB`、像素上限 `4096×4096`、超时 `8s`；任意非白名单 URL 或超限图片返回 `400 Bad Request`。未开启过滤服务或后端未就绪时返回 `503 Service Unavailable`。
- 当服务端无法读取请求中的预览图 URL 时，响应附带 `code: "image_fetch_failed"`。客户端可以读取预览图后用 `imagesBase64` 重试。

---

## 2. 管理员认证接口

管理接口支持 `HttpOnly` 会话 Cookie (`l4a_admin_session`)、`Authorization: Bearer <token>` 或 `X-Admin-Token: <admin_password>`。

使用 Cookie 认证时，写请求须通过 `X-L4A-CSRF` 携带 `GET /api/v1/admin/auth/me` 返回的 `csrfToken`。CLI 可以继续使用显式凭证。

- `POST /api/v1/admin/auth/login` — 验证 `{ "username", "password" }`，签发会话 Token 并写入 `HttpOnly; SameSite=Strict` Cookie
- `POST /api/v1/admin/auth/logout` — 吊销当前会话并清除 Cookie
- `GET /api/v1/admin/auth/me` — 查询当前登录态。使用有效会话 Cookie 时返回该会话的 `csrfToken`
- `POST /api/v1/admin/auth/password` — 修改管理员密码并自动写回 `catalog.toml`

---

## 3. 控制台运维与 SteamKit CM 接口

### 总览、同步与审计
- `GET /api/v1/admin/overview` — 获取缓存统计、Steam CM 状态、同步引擎进度、内容过滤服务状态 (`filter`) 与实例配置路径
- `GET /api/v1/admin/stats` — 获取缓存分类计数 (`CatalogStats`)
- `GET /api/v1/admin/audit?limit=200` — 查询运维与下架墓碑审计日志
- `GET /api/v1/admin/sync/status` — 获取同步策略与游标进度
- `PATCH /api/v1/admin/config/sync` — 热更新 Read-Through / TTL / 增量轮询 / 全量回填参数并持久化至 `catalog.toml`
- `POST /api/v1/admin/sync/trigger` — 立即执行一轮 `RankedByLastUpdatedDate` 增量同步
- `POST /api/v1/admin/sync/reset-cursor` — 将全量创意工坊回填游标重置为 `*`
- `POST /api/v1/admin/sync/fetch` — 手动按指定 Workshop ID 列表强制从 Steam CM 抓取预热（若含合集自动展开抓取子项）

### 内容过滤模型服务
- `GET /api/v1/admin/filter/status` — 获取内容过滤后端配置、就绪态、运行 PID / 活动端点、请求与缓存命中计数及最近错误 (`CatalogFilterStatus`)
- `PATCH /api/v1/admin/config/filter` — 更新 `[filter]` 后端模式与模型校准参数，持久化写回 `catalog.toml`，热更新 `ManagedPython` 参数或切换后端，并写入 `filter_config_update` 审计日志。`deployDir` 和 `pythonBin` 只能通过主机上的 `catalog.toml` 修改，控制台显示当前值。
- `POST /api/v1/admin/filter/restart` — 重启托管 Python 过滤子进程或重新探测 `UpstreamHttp` 健康状态，清空推理缓存并写入 `filter_restart` 审计日志
- `POST /api/v1/admin/filter/test` — 管理员在线概率试算沙盒接口（输入 `FilterPredictRequest`，返回 `FilterProbabilities`）

### SteamKit CM 账号会话
- `GET /api/v1/admin/steam/status` — 查看当前 CM 连接详情与进行中的认证流状态
- `POST /api/v1/admin/steam/reconnect` — 强制重连 Steam CM 节点
- `POST /api/v1/admin/steam/auth/qr/start` — 发起 Steam 手机扫码登录（返回内联 `qrSvg` 与 `challengeUrl`）
- `POST /api/v1/admin/steam/auth/credentials` — 发起账号密码登录流程
- `POST /api/v1/admin/steam/auth/guard` — 提交邮箱或手机令牌 5 位 Steam Guard 验证码
- `POST /api/v1/admin/steam/auth/token` — 直接验证并绑定 Steam CM Refresh Token（写回 `catalog.toml`）
- `POST /api/v1/admin/steam/auth/anonymous` — 清除已绑定账号并切回匿名 CM 会话

### 单条目检视与人工覆写
- `GET /api/v1/admin/items/search` — 使用公开搜索的参数查询完整管理缓存
- `GET /api/v1/admin/items/{workshop_id}` — 查看条目完整缓存详情、覆写锁状态、墓碑时间与历史 `revisions`
- `POST /api/v1/admin/items/{workshop_id}/refresh` — 强制从 Steam CM 重刷单条记录（若处于覆写锁定则返回 `409 Conflict`）
- `POST /api/v1/admin/items/{workshop_id}/override` — 强制覆写并锁定条目状态、标题、备用 `fileUrl` 或前置依赖
- `DELETE /api/v1/admin/items/{workshop_id}/override` — 解除管理员覆写锁定并恢复上游同步
