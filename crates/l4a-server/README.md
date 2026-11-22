# l4a-server

`l4a-server` 是 Left4Addons v2 的创意工坊缓存与同步后端服务。它基于纯协议层 **SteamKit CM (`steam-vent`)** 直接向 Steam Connection Manager 拉取《求生之路 2》(AppID `550`) 的模组元数据、前置依赖有向图、合集成员、画廊预览以及 `.vpk` 下载直链 (`file_url`)，使桌面客户端无需加载受 VAC 限制的 Steamworks SDK 即可完成高速查询与直链下载。

## 核心特性

- **纯协议层 SteamKit CM 上游**：无需在服务器上安装 Steam 客户端或 Steamworks SDK，也无需申请 Steam Web API Key；默认匿名 CM 会话即可拉取 `file_url`、依赖树与合集，同时支持登录普通 Steam 账号解锁创作者昵称解析与更高配额。
- **`file_url` 实时保鲜与穿透缓存**：通过 `PublishedFile.QueryFiles` (`RankedByLastUpdatedDate`) 定时增量同步 + 按需 Read-Through 穿透与 TTL 过期重刷，及时更新模组发版后的新版 CDN 直链。
- **下架墓碑与版本归档**：当上游条目变为 `Removed` (`result=9`) 或 `PrivateOrUnavailable` 时，自动清空失效下载直链并完整保留历史标题、简介、封面、画廊与依赖拓扑。
- **零环境变量 TOML 配置 (`catalog.toml`)**：首次启动自动生成带随机强管理员密码的 `catalog.toml`，控制台中的所有策略调整、密码修改与 Steam Refresh Token 均原子持久化回配置文件。
- **现代化 React 运维控制台 (`GET /admin/`)**：内置登录鉴权、Steam 手机扫码 / 账号密码 + Steam Guard / Refresh Token 登录、缓存检索与强刷、管理员覆写锁定及安全审计日志。

## 快速运行

```bash
# 构建前端控制台静态资源并启动服务（首次启动将在当前目录自动生成 ./catalog.toml 并打印初始密码）
npm run catalog:build
cargo run -p l4a-server -- --config ./catalog.toml
```

访问 `http://127.0.0.1:8787/admin/` 并使用启动日志（或 `catalog.toml` 中 `[admin].password`）的初始密码即可登录控制台。

## 完整文档

- [Catalog Server 介绍与部署指南 (TOML 配置 / Systemd / Docker / Nginx)](../../docs/guides/catalog-server-guide.md)
- [Catalog Server REST API 接口规范](../../docs/api/catalog-server-api.md)
- [07. SteamKit CM 缓存后端架构设计](../../docs/architecture/07-catalog-backend.md)
