use clap::Parser;
use l4a_server::{build_router, ServerConfig, ServerState};
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

const INSECURE_DEFAULT_TOKEN: &str = "change-me-admin-token";

#[derive(Debug, Parser)]
#[command(name = "l4a-server", about = "Left4Addons v2 创意工坊信息管理与缓存后端")]
struct Cli {
    #[arg(long, env = "L4A_SERVER_BIND", default_value = "127.0.0.1:8787")]
    bind: String,
    #[arg(long, env = "L4A_SERVER_DB", default_value = "./l4a-catalog.db")]
    db_path: PathBuf,
    #[arg(long, env = "L4A_SERVER_ADMIN_TOKEN")]
    admin_token: Option<String>,
    #[arg(long, default_value_t = false)]
    allow_insecure_dev_token: bool,
}

fn is_loopback_bind(bind: &str) -> bool {
    if let Ok(addr) = bind.parse::<SocketAddr>() {
        return addr.ip().is_loopback();
    }
    let lower = bind.trim().to_ascii_lowercase();
    lower == "localhost"
        || lower.starts_with("localhost:")
        || lower.starts_with("127.0.0.1:")
        || lower.starts_with("[::1]:")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let trimmed_token = cli
        .admin_token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    let admin_token = match trimmed_token {
        Some(t) if t != INSECURE_DEFAULT_TOKEN => t,
        other => {
            if cli.allow_insecure_dev_token && is_loopback_bind(&cli.bind) {
                tracing::warn!(
                    bind = %cli.bind,
                    "当前正在使用开发模式弱/默认管理员令牌，仅限本地回环调试，请勿用于生产环境"
                );
                other.unwrap_or_else(|| INSECURE_DEFAULT_TOKEN.to_string())
            } else {
                anyhow::bail!(
                    "拒绝启动：必须通过 --admin-token 或环境变量 L4A_SERVER_ADMIN_TOKEN 提供非空的强管理员令牌 (且不可为 change-me-admin-token)。本地调试可显式添加 --allow-insecure-dev-token 并绑定回环地址"
                );
            }
        }
    };

    let state = ServerState::open(&ServerConfig {
        bind_addr: cli.bind.clone(),
        db_path: cli.db_path.clone(),
        admin_token,
    })?;

    let listener = TcpListener::bind(&cli.bind).await?;
    tracing::info!(
        bind = %cli.bind,
        db = %cli.db_path.display(),
        admin_ui = %format!("http://{}/admin", cli.bind),
        "l4a-server 已启动"
    );

    axum::serve(listener, build_router(state)).await?;
    Ok(())
}
