# ADR-005: 四层正交分组与预设模型

- **状态**: 已采纳 (Accepted)
- **背景**: v1 把多包自动分组、Steam 合集导入、用户分组与 MasterCollection 混在一处，双向引用数组易失同步，且单个模组被排他性地限制在一个 Group 里。
- **决策**: 拆分为 `BundleRecord`（含 `locked` 保护与跨包地图聚合）、`TrackedCollectionRecord`（上游追踪）、`UserGroup`（多对多自由分类）与 `LoadoutProfile`（一键启用方案，仅操作 `loading/` 托管文件）。
- **后果**: 互不干扰、无双向冗余字段、自动聚合不再破坏玩家辛苦整理好的分类。
