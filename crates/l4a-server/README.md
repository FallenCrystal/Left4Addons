# l4a-server

`l4a-server` 是 Left4Addons v2 的创意工坊信息管理与缓存后端服务。

## 核心特性

- **高性能轻量架构**：基于 Axum 0.8 与 SQLite WAL 模式，内存开销极低。
- **创意工坊防 429 缓存**：缓存模组元数据、封面、直链与合集层级，减少直连 Steam 带来的限流。
- **模组版本与依赖存档**：记录模组历史修订（revisions）与依赖有向图，避免模组下架后变成“死链黑盒”。
- **可信贡献者与冲突裁决**：支持客户端上报元数据，通过分级信任权重（Trust Tiers）、Steam CDN 白名单校验与防恶意下架裁决机制确保数据安全。
- **自包含暗色管理后台 (`GET /admin`)**：开箱即用，支持条目检索、数据覆写、Token 签发与禁用、审计日志审阅。

## 快速运行 (本地调试)

```bash
# 启动本地开发服务 (默认监听 127.0.0.1:8787，开发模式弱口令)
cargo run -p l4a-server -- --allow-insecure-dev-token
```

访问 `http://127.0.0.1:8787/admin` 即可查看管理后台，默认测试令牌为 `change-me-admin-token`。

## 生产运行

```bash
# 生产环境必须提供非空强管理员令牌，且不可为默认弱口令
L4A_SERVER_ADMIN_TOKEN="$(openssl rand -hex 32)" cargo run --release -p l4a-server -- --bind 0.0.0.0:8787 --db-path /var/lib/l4a-server/catalog.db
```

## 完整文档

- [Catalog Server 介绍与部署指南 (Systemd / Docker / Nginx / 运维)](../../docs/guides/catalog-server-guide.md)
- [Catalog Server REST API 接口规范](../../docs/api/catalog-server-api.md)
- [架构设计与冲突裁决状态机](../../docs/architecture/07-catalog-backend.md)
