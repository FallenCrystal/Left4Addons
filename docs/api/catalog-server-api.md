# `l4a-server` REST API 规范 (`v1`)

默认监听：`127.0.0.1:8787`；Web 管理控制台：`GET /admin/`。

> 部署与 `catalog.toml` 配置说明请参见 [Catalog Server 介绍与部署指南](../guides/catalog-server-guide.md)，后端架构参见 [07. SteamKit CM 缓存后端设计](../architecture/07-catalog-backend.md)。

## 1. 公开查询接口

公开接口默认支持 Read-Through 穿透缓存：当本地未命中或超过 `sync.item_ttl_secs` 时，服务端会自动通过 SteamKit CM 拉取最新元数据与 `.vpk` 下载直链 (`fileUrl`)。

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
在本地 SQLite 缓存库中按标题/描述/ID、标签（大小写不敏感匹配）与状态 (`active` / `removed` / `private_or_unavailable`) 分页检索。`sort` 支持 `relevance`、`updated`、`updated_asc`、`newest`、`subscriptions`、`title`。

### `GET /api/v1/collections/{collection_id}?refresh=false`
查询合集元数据及其子条目明细（若目标或子条目缺失会自动穿透拉取并补全 `items`；若目标 ID 非合集类型返回 `404`）。

---

## 2. 管理员认证接口

管理接口支持 `HttpOnly` 会话 Cookie (`l4a_admin_session`)、`Authorization: Bearer <token>` 或 `X-Admin-Token: <admin_password>`。

- `POST /api/v1/admin/auth/login` — 验证 `{ "username", "password" }`，签发会话 Token 并写入 `HttpOnly; SameSite=Strict` Cookie
- `POST /api/v1/admin/auth/logout` — 吊销当前会话并清除 Cookie
- `GET /api/v1/admin/auth/me` — 查询当前登录态 (`{ "authenticated", "username", "expiresAt" }`)
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
- `GET /api/v1/admin/items/{workshop_id}` — 查看条目完整缓存详情、覆写锁状态、墓碑时间与历史 `revisions`
- `POST /api/v1/admin/items/{workshop_id}/refresh` — 强制从 Steam CM 重刷单条记录（若处于覆写锁定则返回 `409 Conflict`）
- `POST /api/v1/admin/items/{workshop_id}/override` — 强制覆写并锁定条目状态、标题、备用 `fileUrl` 或前置依赖
- `DELETE /api/v1/admin/items/{workshop_id}/override` — 解除管理员覆写锁定并恢复上游同步
