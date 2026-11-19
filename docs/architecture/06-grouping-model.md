# 06. 新分组模型 (Bundles / Collections / UserGroups / Loadouts)

## 1. v1 分组混乱的根因

v1 同时存在 `auto-group`、`workshop-import`、`manual` 与 `MasterCollection`，且 `Group.masterCollectionIds` 与 `MasterCollection.groupIds` 双向冗余存储在 JSON 中，容易出现：
- 同一个插件只能属于单个 `Group`（执行 `create` 或 `add-addons` 会把该插件从原组强行踢出）；
- 重跑自动分组时会删掉之前已分好的组；
- 无法区分「同一个地图被作者拆成的 Part 1 + Part 2（物理必须一起启用）」和「我周五常玩的摸鱼娱乐合集（纯标签分类）」。

## 2. v2 正交四层模型

| 层次 | 实体 | 关系基数 | 职责与保护机制 |
| --- | --- | --- | --- |
| 第 1 层：多包整合 | `BundleRecord` | 1 对多（聚合地图与封面） | 把 `Part 1/2` 或主包+依赖分包聚合成逻辑包；支持 **`locked` 锁定保护**（锁定组与 `Manual` 组绝不被自动重建破坏） |
| 第 2 层：上游合集 | `TrackedCollectionRecord` | 引用 WorkshopID | 专门跟踪 Steam 合集订阅与上游增删 Diff，不强行独占本地模组分类 |
| 第 3 层：自定义分组 | `UserGroup` | **多对多** (`user_group_addons`) | 玩家自由分类与打标，一个模组可同时属于「战役」「高画质」「周五开黑」等多个组 |
| 第 4 层：启用预设 | `LoadoutProfile` | 多对多 (`profile_addons`) | 一键切换整套启用方案，仅操作 `loading/` 托管目录并汇报跳过的纯工坊条目 |
