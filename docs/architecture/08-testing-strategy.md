# 08. 测试策略与质量保障

## 1. 设计原则：测试内建，而非事后补丁

v2 在设计每个 Crate 的第一步即把所有外部副作用抽象在可控接口之后：
- **Steamworks SDK**：抽象为 `SteamBridge` Trait；测试注入 `MockSteamBridge`，零 Steam 进程依赖验证所有同步、依赖图与合集场景；
- **HTTP 与 Steam Web API / 下载 CDN / Catalog Server**：全部接收可配置 Base URL 与 `MirrorRouter`；测试中使用本地 `axum` 临时监听端口或 `tower::ServiceExt::oneshot`，验证真实网络协议、`206 Partial Content` 续传、`ETag` 冲突校验、鉴权与冲突裁决；
- **文件系统与 VPK**：使用 `tempfile::tempdir()` 与 `l4a_vpk::write_vpk()` 实时构造真实二进制 VPK（包括 Dummy v1/v2、mission 文件和损坏包），绝不依赖固定路径 fixture；
- **Web 抓取占位符**：显式测试 `WebScraperPlaceholderProvider` 返回 `ScraperNotImplemented` 且不影响 `WorkshopResolver` 其它来源降级。

## 2. 校验命令矩阵

```bash
# 运行全部 7 个默认 Rust Workspace crate 单元测试与集成测试
cargo test

# 严格 Clippy 静态检查
cargo clippy --all-targets -- -D warnings

# 验证桌面薄壳在 Windows GNU 交叉目标下的编译连通性
npm run build
cargo check --target x86_64-pc-windows-gnu -p l4a-tauri

# 前端静态检查、单测与构建
npm run lint && npm test && npm run build
```
