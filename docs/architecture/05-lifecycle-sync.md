# 05. 远程生命周期同步与依赖淘汰检测

## 1. 更新检测与 Tombstone 保护

当 `L4aEngine::sync_workshop()` 通过 `WorkshopResolver` 拉取远端元数据后：
- **模组更新**：比较 `workshop_snapshots.time_updated` 与 `addons.local_time_updated`，生成 `UpdateAvailableItem`；
- **作者下架或设为私密 (`Removed` / `PrivateOrUnavailable`)**：
  - Steam Web API 返回 `result=9` (FileNotFound) 或 `result=15` (AccessDenied)，或者 `l4a-server` 标记为已下架；
  - 客户端写入 `removed_detected_at` 形成 Tombstone 记录，**绝对不自动删除本地磁盘上的 `.vpk` 文件**，而是在 `SyncReport.removed_upstream` 中提示用户「远端已下架，已为你保留本地副本」。

## 2. 依赖图历史与「不再需要某分包」安全判定

当某地图作者更新模组，不再依赖某个材质/模型分包时，v1 会丢失这层关系且无法判断别的地图是否还在用它。v2 在本地数据库维护 `workshop_dependencies(parent_workshop_id, dep_workshop_id, status)`：

1. 同步前记录每个父模组的旧 `active` 依赖集 $D_{\text{old}}$；
2. 同步后对比新依赖集 $D_{\text{new}}$：
   - $D_{\text{new}} \setminus D_{\text{old}} \rightarrow$ `added_dependencies`（需下载的新增依赖）
   - $D_{\text{old}} \setminus D_{\text{new}} \rightarrow$ `removed_dependencies`（数据库标为 `removed_upstream`）
3. 对每个被移除的依赖 $d$，扫描本地**所有仍处于 `installed` 状态的父模组**是否还存在 `active` 依赖边指向 $d$：
   - 若剩余活跃父模组列表为空：`ObsoleteDependencyHint { can_safely_remove: true }`（提示可安全卸载）；
   - 若仍有其他已安装地图依赖 $d$：`can_safely_remove: false`，并列出 `remaining_active_parents`，彻底防止误删共享分包！

## 3. 追踪合集 Diff

对于纳入追踪的创意工坊合集 (`tracked_collections`)，每次同步计算 `added_items` 与 `removed_items`，形成 `CollectionSyncDiff` 并在数据库记录上游移除的历史 ID，用户可自主决定是否从本地清理或保留。
