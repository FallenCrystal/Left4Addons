# ADR-004: Dummy Bypass v2 显式对账与自愈引擎

- **状态**: 已采纳 (Accepted)
- **背景**: v1 生成 Dummy VPK 后便在扫描器中 `continue` 跳过它，一旦游戏更新覆写 Dummy，或游戏自动往 `workshop/` 下载新组件，管理器完全不知情。
- **决策**: 在 Dummy v2 中写入原包 SHA-256 指纹与时间戳 (`l4aDummy = "2"`)；扫描器完整建模 `physical_files` 并通过 `reconcile` 模块检出 6 类漂移，提供保留用户自定义文件名/启用状态/分组的 `AdoptWorkshopUpdate` 与 `ImportToLoadingAndCreateDummy` 修复路径。
- **后果**: 既能享受 Dummy Bypass 免除重复更新检查，又能在游戏悄悄改动 `workshop/` 时可靠感知并无损合并。
