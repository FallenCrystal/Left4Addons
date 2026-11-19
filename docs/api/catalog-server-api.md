# `l4a-server` REST API 规范 (`v1`)

默认监听：`127.0.0.1:8787`；管理控制台：`GET /admin`。

## 1. 公开查询接口

### `GET /api/v1/health`
返回服务健康状态与当前缓存条目数。

### `GET /api/v1/items/{workshop_id}`
查询单个已缓存的创意工坊条目（包含当前活跃依赖列表与上下架状态）。

### `POST /api/v1/items/batch`
请求体（单次最多 `200` 个 ID，自动按顺序去重；超限返回 `400 Bad Request`）：
```json
{
  "ids": ["1001", "9999"]
}
```
响应：返回命中的 `items` 以及未缓存的 `missingIds`。

### `GET /api/v1/items/search?q=&tag=&status=&sort=&page=1&pageSize=30`
按标题/描述/ID、标签（通过 SQLite `json_each` 精确大小写不敏感匹配）与状态 (`active` / `removed` / `private_or_unavailable`) 分页检索。`sort` 支持 `updated_asc`、`title`、`subscriptions`，默认按 `time_updated DESC` 排序。

### `GET /api/v1/collections/{collection_id}`
查询缓存的合集元数据及其子条目明细（若目标 ID 非合集类型返回 `404`）。

---

## 2. 贡献者上报接口 (需要 `Authorization: Bearer <contributor_token>`)

### `POST /api/v1/submissions`
请求体（单次最多 `200` 条；`fileUrl`/`previewUrl` 须为 `https://` 且匹配 Steam CDN 白名单）：
```json
{
  "items": [
    {
      "workshopId": "1001",
      "fileType": "item",
      "status": "active",
      "title": "Dark Carnival Remix",
      "fileUrl": "https://cdn.example/1001.vpk",
      "timeUpdated": 1700001000,
      "dependencies": [{ "workshopId": "2001", "required": true }],
      "observedSource": "steamworks_sdk",
      "observedAt": "2026-11-19T12:00:00Z"
    }
  ]
}
```
响应中每个条目返回 `decision`：`accepted` / `merged` / `rejected_lower_trust` / `rejected_stale` / `flagged_for_review`。

单次请求按输入顺序裁决，业务拒收仍返回逐条回执。数据库错误返回 `500`，该请求的条目、依赖、修订、审计和贡献者最近活动时间一并回滚。

---

## 3. 管理员接口 (需要 `X-Admin-Token: <admin_token>`)

- `GET /api/v1/admin/stats` — 概览统计（条目数、下架数、待审阅冲突数等）
- `GET /api/v1/admin/contributors` — 列出全部贡献者（脱敏，不含哈希）
- `POST /api/v1/admin/contributors` — 创建新贡献者并返回一次性明文 Token
- `PATCH /api/v1/admin/contributors/{id}` — 更新贡献者信任等级、权重或启用/禁用状态
- `GET /api/v1/admin/audit` — 查看最近 200 条提交与冲突裁决审计日志
- `POST /api/v1/admin/items/{workshop_id}/override` — 强制覆写某条目的状态/标题/下载链接/依赖
- `GET /api/v1/admin/scraper/status` — 查看服务端爬虫占位状态 (`implemented: false`)

管理员覆写与审计日志在同一个事务中保存。省略 `dependencies` 时保留现有依赖，显式传入空数组时清空依赖。
