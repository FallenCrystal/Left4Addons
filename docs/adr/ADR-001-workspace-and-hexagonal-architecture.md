# ADR-001: 多 Crate Workspace 与六边形架构

- **状态**: 已采纳 (Accepted)
- **背景**: v1 将 Tauri 命令、VPK 解析、Steamworks 调用、HTTP 下载、缓存 JSON 与分组算法全部耦合在单一 `src-tauri` crate 中，导致无 GUI/GTK 环境无法编译运行，且无法独立进行单元与集成测试。
- **决策**: 拆分为 `l4a-protocol`、`l4a-vpk`、`l4a-mirror`、`l4a-steam`、`l4a-workshop`、`l4a-core`、`l4a-server`、`l4a-steam-bridge` 与薄适配壳 `l4a-tauri`，并以 Trait (`SteamBridge`, `WorkshopProvider`, `WorkshopWebScraper`, `EventSink`) 隔离全部外部 IO。
- **后果**: 核心领域与服务端完全脱离 Tauri 运行；后续更换前端或增加 CLI/Headless 模式无需触碰业务代码。
