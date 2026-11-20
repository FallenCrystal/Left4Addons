# 01. 总体架构与 Crate 分层

## 1. 为何重构 v2

`v1` 在早期迭代中累积了大量职责耦合问题：`src-tauri/src/commands/handlers/mod.rs` 膨胀到超过 4600 行，同时混杂了文件操作、VPK 解析、Steamworks FFI 调用、网页 HTML 爬取策略、HTTP 断点下载、自动分组启发式和 JSON 数据库读写。前端也承担了本应属于本地引擎的任务队列与数据融合逻辑。

`v2` 采用**六边形架构（Hexagonal Architecture / Ports & Adapters）**与 **Cargo 多 Crate Workspace**，强迫领域规则与外部依赖解耦：

| v1 痛点 | v2 架构解法 |
| --- | --- |
| 所有逻辑堆在 `src-tauri` 内，难测试、难复用 | 拆分为 8 个职责单一 Crate (`src-tauri` 只负责桌面 IPC 和引擎初始化) |
| Steam SDK / Web API / 网页抓取源混乱交织 | `l4a-workshop` 定义 `WorkshopProvider` Trait 与多源优先级合并器 (网页抓取保留显式占位符) |
| 无远程数据缓存服务端，重复请求受限流折磨 | 新增 `l4a-server`（Axum + SQLite + `/admin` 管理页），支持可信玩家提交与冲突裁决 |
| 未建模远程下架、版本变更与依赖包退场 | `l4a-core::lifecycle` 维护状态快照与依赖边历史，支持多父模组引用安全检测 |
| Dummy Bypass 被游戏更新覆写或自动下新包时盲人摸象 | `l4a-vpk` 引入 Dummy v2 元信息。`l4a-core::reconcile` 在扫描时主动检出 6 类漂移并提供修复动作 |
| 分组模型混合自动聚类、合集同步与用户标签，双向引用易丢失 | 拆为正交 4 层：`Bundle` / `TrackedCollection` / `UserGroup` / `LoadoutProfile` |
| 单测仅作为事后补丁，依赖真实 Steam/网络/文件系统 | 所有外设边界均有 Trait 抽象与本地内存/Mock Server 测试，零外网依赖秒级跑通全量测试 |

## 2. Crate 依赖有向无环图 (DAG)

```mermaid
flowchart TD
    Proto["l4a-protocol (纯类型与 ID 契约)"]
    Vpk["l4a-vpk (VPK 读写 / KV 解析 / Dummy v2)"]
    Mirror["l4a-mirror (可注入镜像规则路由器)"]
    Steam["l4a-steam (路径发现 / SteamBridge Trait & FFI)"]
    Bridge["l4a-steam-bridge (独立 Steamworks cdylib)"]
    Workshop["l4a-workshop (Provider 管线 / 占位爬虫 / 续传下载)"]
    Core["l4a-core (本地 SQLite 引擎 / 对账 / 同步 / 分组)"]
    Server["l4a-server (Axum + SQLite 后端与管理页)"]
    Tauri["l4a-tauri (薄桌面 IPC 适配壳)"]

    Vpk --> Proto
    Steam --> Proto
    Workshop --> Proto
    Workshop --> Mirror
    Workshop --> Steam
    Core --> Proto
    Core --> Vpk
    Core --> Mirror
    Core --> Steam
    Core --> Workshop
    Server --> Proto
    Tauri --> Proto
    Tauri --> Steam
    Tauri --> Core
    Steam -.动态加载.-> Bridge
```

> 关键工程约束：根目录 `default-members` 仅包含前 7 个纯跨平台 Crate，因此无 GTK/WebKit 的服务器或 CI 环境也可以直接运行 `cargo test` 和 `l4a-server`。

## 3. 桌面本地管理接口

`get_dashboard_snapshot` 读取当前记录，`scan_workspace` 重新扫描并返回快照。界面可在收到 `snapshot_updated` 后读取新快照。应用层 IPC 错误包含 `code` 和 `message`，界面用 `code` 判断冲突、取消和恢复等状态，`message` 用于展示。

`move_addons`、`toggle_addons`、`apply_renames` 和 `delete_addons` 返回 `AddonBatchReport`。`items` 保留请求顺序，每个模组 ID 对应一条结果。移动、启停和删除取重复 ID 的首次出现位置，重命名要求每个 ID 只提交一个目标名称。

| `status` | 含义 |
| --- | --- |
| `applied` | 该项的文件和记录变更已提交 |
| `unchanged` | 该项无需修改 |
| `failed` | 该项失败，`error` 提供错误码和原因。`recovery_required` 表示回滚受阻，错误消息包含保留备份的位置（如有） |
| `not_attempted` | 校验或执行中止后，该项未执行 |

重命名先校验整批目标。执行阶段遇到首个错误即停止，先前提交的项目保留。只清除记录时要求模组处于未安装状态。文件删除针对当前主文件和匹配的 Dummy，剩余的真实副本会被后续扫描重新识别。

批次执行后统一刷新磁盘库存和对账告警。若返回 `refreshError`，可运行 `scan_workspace` 重试刷新，`items` 仍记录已提交的操作。`apply_profile` 在 `batch` 字段返回同一结果结构，启停数量按实际提交的项目统计。
