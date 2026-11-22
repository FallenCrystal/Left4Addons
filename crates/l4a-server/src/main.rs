use clap::Parser;
use l4a_server::{build_router, ServerState};
use std::path::PathBuf;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
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
    let _worker_token = state.sync.spawn_background_workers();

    if generated_password {
        tracing::info!(
            config = %cli.config.display(),
            username = %initial_user,
            password = %initial_pass,
            "已生成并保存初始管理员密码，请使用上述账号密码登录控制台"
        );
    }

    let listener = TcpListener::bind(&bind_addr).await?;
    tracing::info!(
        bind = %bind_addr,
        config = %cli.config.display(),
        db = %db_path.display(),
        console = %format!("http://{bind_addr}/admin/"),
        "l4a-server (SteamKit CM Catalog) 已启动"
    );

    axum::serve(listener, build_router(state)).await?;
    Ok(())
}
