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
- **本地能力增强与隔离保护**：本地 SQLite FTS5 索引（中英文分词、驼峰/忽略空格复合词、中文标题拼音、`AdminOverride` 人工覆写与直链、下架墓碑）会与 Steam CM 结果智能去重融合；仅通过按需搜索暖缓存 (`CacheOnly`) 的条目不会污染公开首页，本地已知的私密 (`private_or_unavailable`) 与未列出 (`is_listed = 0`) 条目绝不会从公开搜索泄露。
- **精确 ID / 链接查询**：传入 Steam Workshop 链接或 6 位以上纯数字 ID 时，若本地未收录且未在本地标记为受限/覆写，会自动通过 `GetDetails` 穿透补全公开条目。

> `GET /api/v1/admin/items/search` 保持**纯本地 SQLite 缓存检索**，供管理员检视已缓存条目与墓碑，不触发上游 Steam CM 请求。

### `GET /api/v1/collections/{collection_id}?refresh=false`
查询合集元数据及其子条目明细（若目标或子条目缺失会自动穿透拉取并补全 `items`；若目标 ID 非合集类型返回 `404`）。

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
- `GET /api/v1/admin/overview` — 获取缓存统计、Steam CM 状态、同步引擎进度与实例配置路径
- `GET /api/v1/admin/stats` — 获取缓存分类计数 (`CatalogStats`)
- `GET /api/v1/admin/audit?limit=200` — 查询运维与下架墓碑审计日志
- `GET /api/v1/admin/sync/status` — 获取同步策略与游标进度
- `PATCH /api/v1/admin/config/sync` — 热更新 Read-Through / TTL / 增量轮询 / 全量回填参数并持久化至 `catalog.toml`
- `POST /api/v1/admin/sync/trigger` — 立即执行一轮 `RankedByLastUpdatedDate` 增量同步
- `POST /api/v1/admin/sync/reset-cursor` — 将全量创意工坊回填游标重置为 `*`
- `POST /api/v1/admin/sync/fetch` — 手动按指定 Workshop ID 列表强制从 Steam CM 抓取预热（若含合集自动展开抓取子项）

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
