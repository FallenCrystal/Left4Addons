use clap::Parser;
use l4a_server::{build_router, ServerState};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::net::TcpListener;

#[derive(Debug, Parser)]
#[command(
    name = "l4a-server",
    about = "Left4Addons v2 SteamKit CM 创意工坊缓存后端"
)]
struct Cli {
    /// TOML 配置文件路径（不存在时首次运行将自动生成默认配置与随机管理员密码）
    #[arg(short, long, default_value = "./catalog.toml")]
    config: PathBuf,

    /// 可选：覆盖配置文件中的监听地址
    #[arg(long)]
    bind: Option<String>,

    /// 可选：覆盖配置文件中的 SQLite 数据库路径
    #[arg(long)]
    db_path: Option<PathBuf>,

    /// 日志目录（默认为配置文件所在目录下的 `logs/`）。当前会话写入 `latest.log`，
    /// 启动时上一会话日志会被压缩为 `l4a-server-YYYY-MM-DD_HH-MM-SS.tar.gz`
    #[arg(long, env = "L4A_SERVER_LOG_DIR")]
    log_dir: Option<PathBuf>,

    /// 保留的历史日志归档数量（0 表示不限制）
    #[arg(long, env = "L4A_SERVER_LOG_KEEP", default_value_t = l4a_logging::DEFAULT_KEEP_ARCHIVES)]
    log_keep: usize,
}

fn default_log_dir(config: &Path) -> PathBuf {
    config
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("logs")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut log_options = l4a_logging::LogOptions::new(
        cli.log_dir
            .clone()
            .unwrap_or_else(|| default_log_dir(&cli.config)),
        "l4a-server",
    );
    log_options.app_label = format!("l4a-server {}", env!("CARGO_PKG_VERSION"));
    log_options.keep_archives = cli.log_keep;
    log_options.console = l4a_logging::ConsoleTarget::Stdout;
    let _log_session = l4a_logging::init(log_options);
    let (cfg, generated_password) = l4a_server::config::CatalogConfig::load_or_init(&cli.config)?;
    let bind_addr = cli.bind.unwrap_or_else(|| cfg.server.bind.clone());
    let db_path = cli.db_path.unwrap_or_else(|| cfg.server.db_path.clone());
    let initial_user = cfg.admin.username.clone();
    let initial_pass = cfg.admin.password.clone();

    let store = std::sync::Arc::new(l4a_server::config::CatalogConfigStore::new(
        Some(cli.config.clone()),
        cfg,
    ));
    let state = ServerState::from_config_store_with_db_path(store, &db_path)?;
    let worker_token = state.sync.spawn_background_workers();
    let filter_service = state.filter.clone();

    if generated_password {
        // The plaintext password goes to the console only, never into the persistent log files.
        println!(
            "已生成并保存初始管理员密码，请使用以下账号密码登录控制台: username={initial_user} password={initial_pass} (config={})",
            cli.config.display()
        );
        tracing::info!(
            config = %cli.config.display(),
            username = %initial_user,
            "Generated and saved initial admin password (plaintext printed to console only; also available in config [admin].password)"
        );
    }

    let listener = TcpListener::bind(&bind_addr).await?;
    tracing::info!(
        bind = %bind_addr,
        config = %cli.config.display(),
        db = %db_path.display(),
        console = %format!("http://{bind_addr}/admin/"),
        "l4a-server (SteamKit CM Catalog) started"
    );

    let serve_result = axum::serve(
        listener,
        build_router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await;
    worker_token.cancel();
    filter_service.shutdown().await;
    if let Err(err) = &serve_result {
        tracing::error!(error = %err, "l4a-server exited with error");
    } else {
        tracing::info!("l4a-server stopped");
    }
    serve_result?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %err, "Failed to register Ctrl+C signal handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => {
                tracing::error!(error = %err, "Failed to register SIGTERM signal handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("Shutdown signal received; gracefully stopping l4a-server and managed filter child process...");
}
