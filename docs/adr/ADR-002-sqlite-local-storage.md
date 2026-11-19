# ADR-002: 客户端状态迁至本地 SQLite 与 TOML 配置分离

- **状态**: 已采纳 (Accepted)
- **背景**: v1 使用多个手写 JSON 文件 (`settings.json`, `groups.json`, `known_addons.json`, `workshop_cache.json`, `background_tasks.json`) 并通过互斥锁整体覆写，随着依赖图、物理文件快照与历史修订增加，整包序列化写回既易产生并发撕裂又难表达多对多关系。
- **决策**: 使用 `rusqlite` (bundled WAL 模式) 持久化所有运行态关系表 (`l4a_v2.db`)，同时将用户可直接编辑的静态配置留在 `config/settings.toml` 与 `config/mirrors.json`。
- **后果**: 获得事务 ACID、外键约束与毫秒级增量查询能力，彻底避免整库 JSON 读写竞争。
