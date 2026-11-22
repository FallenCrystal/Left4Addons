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

### `GET /api/v1/items/search?q=&tag=&status=&sort=&page=1&pageSize=30`
在本地 SQLite FTS5 缓存索引中按标题（支持中英文分词、驼峰/忽略空格复合匹配与中文标题拼音检索）、作者名、Workshop ID/链接、标签（大小写不敏感匹配）与状态 (`active` / `removed` / `private_or_unavailable`) 分页检索；仅当主字段无命中时自动回退至去除 BBCode 与链接后的简介检索，避免简介版权声明/致谢名单干扰结果。`sort` 支持 `relevance`、`updated`、`updated_asc`、`newest`、`subscriptions`、`title`。

搜索会排除未列出、尚未核验可见性和已过期的活跃条目。未列出的 Steam 条目仍可按 ID 查询。此前已确认公开的墓碑和管理员锁定的公开覆写可以继续显示。

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
