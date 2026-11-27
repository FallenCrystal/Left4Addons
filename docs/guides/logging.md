# 日志系统指南 (`l4a-logging`)

桌面端（`src-tauri` + `l4a-core`）与目录服务（`l4a-server`）共用 `crates/l4a-logging`，统一输出带日期时间的日志、在启动时滚动并压缩旧日志，并对联网请求、外部命令、内容过滤与用户操作进行审计记录。

## 文件布局与滚动

| 进程 | 默认目录 | 当前会话 | 历史归档 |
| --- | --- | --- | --- |
| 桌面端 | `<运行目录>/l4a/logs/`（不可写时回退到系统 AppData） | `latest.log` | `l4a-YYYY-MM-DD_HH-MM-SS.tar.gz` |
| `l4a-server` | 配置文件所在目录下的 `logs/`（`--log-dir` / `L4A_SERVER_LOG_DIR` 可覆盖） | `latest.log` | `l4a-server-YYYY-MM-DD_HH-MM-SS.tar.gz` |

- 每个会话的首行是 `# l4a-log session_start=<RFC3339> app=... pid=... os=...` 会话头。
- 启动时，上一会话的 `latest.log` 先被**同步**重命名为 `<前缀>-<会话开始时间>.log`（若无会话头则使用文件修改时间），随后由后台线程 `l4a-log-archiver` 压缩为同名 `.tar.gz` 并删除原文件。同一秒内多次启动会追加 `-2`、`-3` 后缀。
- 压缩先写入 `.tar.gz.partial` 再原子重命名，中断不会留下损坏的归档；残留的待压缩 `.log` 会在下次启动时重试。
- 默认保留最近 **30** 个归档（服务端可用 `--log-keep` / `L4A_SERVER_LOG_KEEP` 调整，`0` 表示不限制）。
- 若 `latest.log` 被另一个仍在运行的实例占用而无法重命名，新实例改写入 `<前缀>-<时间>-<pid>.log`，不会覆盖对方日志。

## 日志格式与级别

```
2026-11-26 20:55:12.123 -08:00  INFO main l4a::net: HTTP request completed purpose="catalog.search" method=GET url=https://catalog.example/api/v1/search?q=... status=200 elapsed_ms=184 bytes=Some(5120)
```

- 时间戳为本地时间，包含日期、毫秒和时区偏移；文件中不含 ANSI 颜色码。
- 日志消息统一使用英文；字段值中来自界面或错误类型的文本（如部署阶段说明、错误信息）保持原文。
- 过滤规则读取 `L4A_LOG`，其次 `RUST_LOG`，默认 `info`。例如：`L4A_LOG=info,l4a::net=debug,l4a::process=debug`。
- 桌面端 debug 构建同时输出到 stderr，release 构建只写文件；服务端同时输出到 stdout。
- Tauri 等依赖通过 `log` crate 产生的记录会被桥接进同一日志；任何线程 panic 都会以 `l4a::panic` 记录位置与回溯后再交给默认处理。

## 审计内容（按 target 分类）

| Target | 内容 | 级别 |
| --- | --- | --- |
| `l4a::op` | 桌面端 IPC 操作：扫描、保存配置、下载/更新、移动/启停/重命名/删除模组、整合包、合集、分组、预设、部署/启停本地过滤服务、过滤评估、排除规则等。记录参数摘要、耗时与结果摘要（成功/失败/未变更计数等）。 | INFO / 失败 WARN |
| `l4a::net` | 所有出站 HTTP（Steam Web API、目录服务器、镜像下载、封面、过滤推理、LLM 复核、本地推理服务）：方法、脱敏后的 URL、状态码、响应头耗时、大小；Steam CM 连接与 GetDetails / QueryFiles / search_files 调用的耗时与条目数。封面、本地推理、健康探测等高频请求成功时为 DEBUG。 | INFO / DEBUG / 失败或 4xx·5xx WARN |
| `l4a::process` | 部署与探测执行的外部命令（`git`、`python -m venv`、`pip install`、推理守护进程等）：完整命令行、工作目录、注入的环境变量**名称**、退出码、耗时、失败时的 stderr/stdout 尾部；超时与取消。Python/Git 环境探测为 DEBUG。 | INFO / 失败 WARN |
| `l4a::filter_runner` | 本地/服务端 Python 推理守护进程的 stderr 逐行转发。 | INFO |
| `l4a::steam` | Steamworks bridge 库加载与每次 SDK 调用的耗时（下载进度轮询为 DEBUG）。 | INFO / 失败 WARN |
| `tower_http::trace::on_response`（`http{method path client}` span） | `l4a-server` HTTP 访问日志：方法、路径（不含查询串）、客户端地址、状态码与耗时。 | INFO / 失败 WARN |
| `l4a_core::filter::evaluator` | 内容过滤：每项的判定（flagged、决策来源、分数、图片数、是否完整）与耗时；每批次的命中缓存数、实际评估数、标记/安全/失败计数与总耗时。 | INFO |
| `l4a_core::filter::deploy` | 本地过滤部署的开始参数、各阶段切换、最终 commit 与总耗时。 | INFO / 失败 WARN |
| `l4a_server::filter` | 服务端每次过滤推理的工坊 ID、标题、图片数、融合分数、评分版本与耗时。 | INFO / 失败 WARN |

## 敏感信息处理

- URL 中的用户名密码被替换为 `***`，`key`、`token`、`access_token`、`password`、`secret`、`*_token`、`*_key` 等查询参数值被掩码。
- 命令行中 `--token xxx`、`--api-key=xxx` 等参数值被掩码，超长参数（内联脚本、JSON）被截断；环境变量只记录名称。
- 请求头与请求体从不记录（LLM Token、Runner Token 均通过请求头传递）。
- `set_filter_llm_token` 只记录是否提供了 Token。
- `l4a-server` 首次生成的管理员明文密码只打印到控制台，不写入日志文件。

## 在代码中使用

```rust
use l4a_logging::http::AuditedSend;
use l4a_logging::process::CommandAudit;

// 出站 HTTP：把 .send() 换成 .send_audited("用途")，高频请求用 .send_audited_quiet(...)
let resp = client.get(url).send_audited("catalog.search").await?;

// 外部命令
let audit = CommandAudit::start("filter.deploy", cmd.as_std());
let result = cmd.output().await;
audit.finish_output(&result);
```

初始化（进程入口调用一次）：

```rust
let mut options = l4a_logging::LogOptions::new(dir.join("logs"), "l4a");
options.app_label = format!("Left4Addons {}", env!("CARGO_PKG_VERSION"));
let _session = l4a_logging::init(options);
```
