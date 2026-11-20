<div style="text-align: center;">

  # Left 4 Addons v2

  **《求生之路 2》(Left 4 Dead 2) 创意工坊模组与本地插件管理器（v2 架构重构分支）**

</div>

> 原 `v1` 代码与发行逻辑完整保留在 `main` 分支；当前 `v2` 分支专注于可长期演进的 **多 Crate Workspace + 本地 SQLite 核心引擎 + 独立信息缓存后端 (`l4a-server`) + 全链路自动化测试底座**。

---

## v2 核心架构改进

1. **清晰的职责分层（8 个 Workspace Crates + 瘦身 Tauri 壳）**
   - `l4a-protocol`: 跨端共享的纯数据结构、强类型 ID（`WorkshopId`, `AddonId`）与事件契约。
   - `l4a-vpk`: VPK v1/v2 读写、KeyValues 解析、SHA-256 内容指纹、地图/VTF 封面提取与 **Dummy v2 自描述元信息**。
   - `l4a-mirror`: 无全局状态的 `MirrorRouter`，100% 兼容 `mirrors.json` 三种规则语法。
   - `l4a-steam` & `l4a-steam-bridge`: 路径自动探测 + `SteamBridge` Trait 隔离（含 `NullSteamBridge` / `DynamicSteamBridge` / `MockSteamBridge`）。
   - `l4a-workshop`: 多源 `WorkshopResolver`、断点续传下载器 (`ResilientDownloader`)，以及 **显式保留占位符的 `WorkshopWebScraper` Trait / `WebScraperPlaceholderProvider`**。
   - `l4a-core`: 本地 SQLite 引擎、物理/逻辑分离扫描器、Dummy Bypass v2 对账状态机、远程生命周期同步与四层正交分组模型。
   - `l4a-server`: 独立可自托管的创意工坊信息管理与缓存服务（Axum + SQLite + 可信玩家 Token + 冲突裁决 + 内置 `/admin` 管理页 + 服务端爬虫占位符）。
   - `src-tauri`: 仅负责初始化 `L4aEngine` 与 IPC 转发的薄壳。

2. **Dummy Bypass v2 显式对账**
   - 游戏更新覆写 `workshop/<id>.vpk` 时自动检出 `DummyOverwrittenByGameUpdate`，支持 **`AdoptWorkshopUpdate`**（保留用户自定义文件名、禁用状态与分组，替换包体并重建 Dummy v2）。
   - 游戏自动下载新组件进入 `workshop/` 时自动检出 `NewWorkshopAutoDownload`，支持一键按命名规则迁入 `loading/` 并生成 Dummy v2。

3. **远程生命周期同步与多父依赖安全判定**
   - 检测上游更新、下架/私密 Tombstone（不误删用户本地 `.vpk`）。
   - 父模组更新移除分包时，跨全部已安装父模组计算剩余入度，仅在无其他地图依赖时标记 `can_safely_remove = true`。

4. **四层正交分组模型**
   - `BundleRecord`（多包聚合与地图/封面跨包补全，支持 `locked` 防覆盖）
   - `TrackedCollectionRecord`（上游合集追踪与增删 Diff）
   - `UserGroup`（多对多自由分类与标签）
   - `LoadoutProfile`（一键切换启用方案，仅操作 `loading/` 托管文件）

---

## 文档蓝图

完整架构设计、状态机、API 契约与 ADR 决策记录位于 [docs/README.md](docs/README.md)：
- [01. 总体架构与 Crate 分层](docs/architecture/01-overview.md)
- [02. 领域模型与本地 SQLite 设计](docs/architecture/02-domain-model.md)
- [03. 创意工坊多源管道与 Web Scraper 占位符](docs/architecture/03-workshop-sources.md)
- [04. Dummy Bypass v2 对账状态机](docs/architecture/04-dummy-bypass-v2.md)
- [05. 远程生命周期同步与依赖淘汰检测](docs/architecture/05-lifecycle-sync.md)
- [06. 新分组模型](docs/architecture/06-grouping-model.md)
- [07. `l4a-server` 后端设计与管理控制台](docs/architecture/07-catalog-backend.md)
- [08. 测试策略与校验命令](docs/architecture/08-testing-strategy.md)
- [Catalog Server REST API 规范](docs/api/catalog-server-api.md)

---

## 快速验证与开发

验证环境使用 Node.js 24、Rust 1.100.0 和随附的 `rustfmt`。

```bash
# 1. 检查 Rust 格式并运行默认工作区成员的测试（零外部 Steam/网络依赖）
cargo fmt --all -- --check
cargo test --locked

# 2. 启动独立的创意工坊信息管理/缓存后端（管理页位于 http://127.0.0.1:8787/admin）
L4A_SERVER_ADMIN_TOKEN=dev-secret cargo run -p l4a-server

# 3. 安装前端依赖并运行前端校验/测试/构建
npm ci
npm run lint
npm test
npm run build

# 4. Linux 上交叉检查 Windows GNU 目标的 Tauri 薄壳
cargo check --target x86_64-pc-windows-gnu -p l4a-tauri
```

## 许可证

基于 [MIT License](LICENSE) 开源。
