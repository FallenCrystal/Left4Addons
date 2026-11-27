# Left4Addons v2 架构蓝图与文档总览

> `v2` 分支是一次针对核心底座、领域模型与可维护性的彻底重构；原 `v1` 代码完整保留在 `main` 分支，供历史查阅与行为参考。

## 阅读路线

### 1. 核心架构与领域设计 (`docs/architecture/`)
1. [01-overview.md](./architecture/01-overview.md) — 总体分层、Hexagonal 端口与适配器、Crate 依赖 DAG 与 v1/v2 问题对照表
2. [02-domain-model.md](./architecture/02-domain-model.md) — 核心领域实体（逻辑 Addon 与物理文件分离）与本地 SQLite ER 图
3. [03-workshop-sources.md](./architecture/03-workshop-sources.md) — 创意工坊多源管道、优先级路由与 Web Scraper 占位符契约
4. [04-dummy-bypass-v2.md](./architecture/04-dummy-bypass-v2.md) — Dummy Bypass v2 对账状态机（解决游戏更新覆写 Dummy 与游戏自动下载新组件）
5. [05-lifecycle-sync.md](./architecture/05-lifecycle-sync.md) — 远程生命周期同步（更新、下架/私密 Tombstone、多父依赖淘汰提示与合集 Diff）
6. [06-grouping-model.md](./architecture/06-grouping-model.md) — 新四层分组模型（Bundles / Tracked Collections / User Groups / Loadout Profiles）
7. [07-catalog-backend.md](./architecture/07-catalog-backend.md) — `l4a-server` 创意工坊信息管理/缓存后端、可信玩家 Token 与冲突裁决
8. [08-testing-strategy.md](./architecture/08-testing-strategy.md) — 测试金字塔、Mock 注入边界与全量验证指令

### 2. 架构决策记录 (`docs/adr/`)
- [ADR-001](./adr/ADR-001-workspace-and-hexagonal-architecture.md): 多 Crate Workspace 与六边形架构
- [ADR-002](./adr/ADR-002-sqlite-local-storage.md): 客户端状态迁至本地 SQLite 与 TOML 配置分离
- [ADR-003](./adr/ADR-003-workshop-provider-pipeline-and-catalog-backend.md): 创意工坊多源管道与自托管后端协同
- [ADR-004](./adr/ADR-004-dummy-bypass-reconciliation-engine.md): Dummy Bypass v2 显式对账与自愈引擎
- [ADR-005](./adr/ADR-005-grouping-and-loadout-model.md): 四层正交分组与预设模型

### 3. 接口与实用指南 (`docs/api/` & `docs/guides/`)
- [Catalog Server 介绍与部署指南 (`l4a-server`)](./guides/catalog-server-guide.md)
- [Catalog Server REST API 规范](./api/catalog-server-api.md)
- [镜像代理配置指南 (`mirrors.json`)](./guides/mirrors-configuration.md)
- [Steamworks SDK 实测细节与限制](./guides/steamworks-sdk-details.md)
- [日志系统指南 (`l4a-logging`)](./guides/logging.md) — 日志位置、启动滚动与 tar.gz 归档、审计范围与脱敏规则
