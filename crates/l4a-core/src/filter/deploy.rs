use crate::config::AppPaths;
use crate::engine::EventSink;
use crate::error::CoreError;
use l4a_protocol::{
    CoreEvent, DeployLocalFilterRequest, FilterCommitInfo, FilterEnvReport, FilterModelParams,
    FilterProbabilities, LocalFilterDeploymentStatus, LocalFilterServiceState, FILTER_RUNNER_PY,
};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Deserialize;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio_util::sync::CancellationToken;

const REQUIRED_DEPLOY_FILES: &[&str] = &[
    "l4filter.py",
    "vision_encoder.onnx",
    "fusion_head.onnx",
    "text_vocab.json",
    "requirements.txt",
];

const MAX_READY_BANNER_BYTES: usize = 64 * 1024;
const MAX_STDERR_TAIL_BYTES: usize = 4096;
const CONFIGURE_HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const PREDICT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const AUTO_RESTART_COOLDOWN: Duration = Duration::from_secs(3);
const GIT_FAST_TIMEOUT: Duration = Duration::from_secs(30);
const GIT_NETWORK_TIMEOUT: Duration = Duration::from_secs(300);
const VENV_CREATE_TIMEOUT: Duration = Duration::from_secs(120);
const PIP_INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

const GIT_STALL_ENVS: &[(&str, &str)] = &[
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_HTTP_LOW_SPEED_LIMIT", "1024"),
    ("GIT_HTTP_LOW_SPEED_TIME", "20"),
];

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn configure_std_cmd(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd;
}

fn configure_tokio_cmd(cmd: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd;
}

fn clean_os_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{unc}");
        }
        if let Some(stripped) = raw.strip_prefix(r"\\?\") {
            return stripped.to_string();
        }
    }
    raw.into_owned()
}

fn is_unc_or_network_path(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        if path.components().any(|comp| {
            matches!(
                comp,
                Component::Prefix(p)
                    if matches!(
                        p.kind(),
                        Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _) | Prefix::DeviceNS(_)
                    )
            )
        }) {
            return true;
        }
    }
    let cleaned = clean_os_path(path);
    cleaned.starts_with(r"\\") || cleaned.starts_with("//")
}

fn venv_python_path(venv_dir: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        venv_dir.join("Scripts").join("python.exe")
    }
    #[cfg(not(windows))]
    {
        let py = venv_dir.join("bin").join("python");
        if py.is_file() {
            py
        } else {
            let py3 = venv_dir.join("bin").join("python3");
            if py3.is_file() {
                py3
            } else {
                py
            }
        }
    }
}

fn which_executable(program: &str) -> Option<String> {
    let path_var = std::env::var_os("PATH")?;
    #[cfg(windows)]
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
        .split(';')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_ascii_lowercase())
        .collect();

    for dir in std::env::split_paths(&path_var) {
        let direct = dir.join(program);
        if is_executable_file(&direct) {
            return Some(clean_os_path(&direct));
        }
        #[cfg(windows)]
        {
            for ext in &exts {
                let candidate = dir.join(format!("{program}{ext}"));
                if is_executable_file(&candidate) {
                    return Some(clean_os_path(&candidate));
                }
            }
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub fn probe_environment() -> FilterEnvReport {
    let mut report = FilterEnvReport::default();

    let mut git_cmd = std::process::Command::new("git");
    git_cmd
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    configure_std_cmd(&mut git_cmd);

    if let Ok(out) = git_cmd.output() {
        if out.status.success() {
            let raw_ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let clean_ver = raw_ver
                .strip_prefix("git version ")
                .unwrap_or(&raw_ver)
                .trim()
                .to_string();
            report.git_available = true;
            report.git_version = (!clean_ver.is_empty()).then_some(clean_ver);
            report.git_path = which_executable("git").or_else(|| Some("git".into()));
        }
    }

    let probe_script = "import sys\nif sys.version_info < (3, 9):\n    sys.exit(2)\nprint(sys.executable)\nprint(sys.version.split()[0])\ntry:\n    import venv, ensurepip\n    print('1')\nexcept Exception:\n    print('0')\n";

    #[cfg(windows)]
    let candidates: &[(&str, &[&str])] = &[("python3", &[]), ("python", &[]), ("py", &["-3"])];
    #[cfg(not(windows))]
    let candidates: &[(&str, &[&str])] = &[("python3", &[]), ("python", &[])];

    let mut fallback_no_venv: Option<(String, String)> = None;

    for &(prog, prefix_args) in candidates {
        let mut cmd = std::process::Command::new(prog);
        cmd.args(prefix_args)
            .args(["-c", probe_script])
            .env("PYTHONIOENCODING", "utf-8")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        configure_std_cmd(&mut cmd);

        let Ok(out) = cmd.output() else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        let Some(exe_path) = lines.next() else {
            continue;
        };
        let Some(ver) = lines.next() else {
            continue;
        };
        let has_venv = lines.next() == Some("1");
        if has_venv {
            report.python_available = true;
            report.python_path = Some(exe_path.to_string());
            report.python_version = Some(ver.to_string());
            report.venv_available = true;
            return report;
        }
        if fallback_no_venv.is_none() {
            fallback_no_venv = Some((exe_path.to_string(), ver.to_string()));
        }
    }

    if let Some((exe_path, ver)) = fallback_no_venv {
        report.python_available = true;
        report.python_path = Some(exe_path);
        report.python_version = Some(ver);
        report.venv_available = false;
    }

    report
}

pub fn validate_repo_url(raw: &str, https_only: bool) -> Result<String, CoreError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') || trimmed.chars().any(|c| c.is_control()) {
        return Err(CoreError::InvalidInput("无效的模型仓库地址".into()));
    }
    if trimmed.starts_with(r"\\") || trimmed.starts_with("//") {
        return Err(CoreError::InvalidInput(
            "不允许使用 UNC 网络共享或无协议仓库路径".into(),
        ));
    }

    let as_path = Path::new(trimmed);
    if is_unc_or_network_path(as_path) {
        return Err(CoreError::InvalidInput(
            "不允许使用 UNC 网络共享路径作为模型仓库地址".into(),
        ));
    }

    // Only touch local filesystem if `trimmed` is not a URL scheme (`scheme://...`)
    if !trimmed.contains("://") && as_path.is_dir() {
        let canon = fs::canonicalize(as_path)?;
        if is_unc_or_network_path(&canon) {
            return Err(CoreError::InvalidInput(
                "不允许使用指向 UNC 网络共享的本地仓库路径".into(),
            ));
        }
        return Ok(clean_os_path(&canon));
    }

    let parsed = url::Url::parse(trimmed)
        .map_err(|_| CoreError::InvalidInput("模型仓库地址必须是有效的 URL 或本地目录".into()))?;

    match parsed.scheme() {
        "file" => {
            if let Some(host) = parsed.host_str() {
                if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
                    return Err(CoreError::InvalidInput(
                        "file:// 模型仓库地址不允许指定远程主机".into(),
                    ));
                }
            }
            if parsed.path().starts_with("//") || parsed.path().starts_with(r"\\") {
                return Err(CoreError::InvalidInput(
                    "不允许使用 UNC 网络共享作为 file:// 仓库路径".into(),
                ));
            }
            let file_path = parsed
                .to_file_path()
                .map_err(|()| CoreError::InvalidInput("无效的 file:// 仓库路径".into()))?;
            if is_unc_or_network_path(&file_path) {
                return Err(CoreError::InvalidInput(
                    "不允许使用 UNC 网络共享路径作为模型仓库地址".into(),
                ));
            }
            if !file_path.is_dir() {
                return Err(CoreError::NotFound(format!(
                    "本地模型仓库目录不存在: {}",
                    file_path.display()
                )));
            }
            let canon = fs::canonicalize(file_path)?;
            if is_unc_or_network_path(&canon) {
                return Err(CoreError::InvalidInput(
                    "不允许使用指向 UNC 网络共享的本地仓库路径".into(),
                ));
            }
            Ok(clean_os_path(&canon))
        }
        "https" => {
            if parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
            {
                return Err(CoreError::InvalidInput(
                    "仓库 URL 缺少主机名或包含内联认证凭据".into(),
                ));
            }
            Ok(parsed.into())
        }
        "http" => {
            let Some(host) = parsed.host_str() else {
                return Err(CoreError::InvalidInput("仓库 URL 缺少主机名".into()));
            };
            if !parsed.username().is_empty() || parsed.password().is_some() {
                return Err(CoreError::InvalidInput(
                    "仓库 URL 不允许包含内联认证凭据".into(),
                ));
            }
            let is_loopback = host.eq_ignore_ascii_case("localhost")
                || matches!(host, "127.0.0.1" | "::1" | "[::1]");
            if https_only && !is_loopback {
                return Err(CoreError::InvalidInput(
                    "已启用仅 HTTPS 连接，请使用 HTTPS 模型仓库地址".into(),
                ));
            }
            Ok(parsed.into())
        }
        _ => Err(CoreError::InvalidInput(
            "仅支持 HTTPS（或本地测试路径）模型仓库地址".into(),
        )),
    }
}

pub fn validate_target_commit(raw: &str) -> Result<String, CoreError> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.len() > 128
        || trimmed.starts_with('-')
        || trimmed.starts_with('/')
        || trimmed.ends_with('/')
        || trimmed.ends_with(".lock")
        || trimmed.contains("..")
        || trimmed.contains("@{")
        || trimmed.contains("//")
        || trimmed.contains('\\')
        || trimmed.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(CoreError::InvalidInput("目标 Commit 或分支名称无效".into()));
    }

    let is_hex_sha =
        (4..=40).contains(&trimmed.len()) && trimmed.bytes().all(|b| b.is_ascii_hexdigit());
    let is_safe_ref = trimmed
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'));

    if is_hex_sha || is_safe_ref {
        Ok(trimmed.to_string())
    } else {
        Err(CoreError::InvalidInput(
            "目标 Commit 仅允许 4-40 位十六进制哈希或合法引用名".into(),
        ))
    }
}

#[derive(Debug, Deserialize)]
struct ReadyBanner {
    port: u16,
}

struct RunningRunner {
    child: Child,
    stdin: Option<ChildStdin>,
    pid: Option<u32>,
    endpoint: String,
    port: u16,
    runner_token: String,
    stderr_tail: Arc<Mutex<String>>,
}

#[derive(Default)]
struct CachedStatusState {
    last_env: Option<FilterEnvReport>,
    service_state: LocalFilterServiceState,
    service_endpoint: Option<String>,
    running_pid: Option<u32>,
    current_commit: Option<FilterCommitInfo>,
    remote_latest_commit: Option<FilterCommitInfo>,
    update_available: bool,
    recent_commits: Vec<FilterCommitInfo>,
    stage_message: Option<String>,
    last_error: Option<String>,
    active_runner_info: Option<(u16, String)>,
}

enum PredictLocalError {
    Transport(CoreError),
    HttpStatus(CoreError),
}

pub struct LocalFilterManager {
    paths: AppPaths,
    events: Arc<dyn EventSink>,
    op_lock: tokio::sync::Mutex<Option<RunningRunner>>,
    configure_lock: tokio::sync::Mutex<()>,
    state: Mutex<CachedStatusState>,
    desired_params: RwLock<FilterModelParams>,
    applied_params: Mutex<Option<FilterModelParams>>,
    last_auto_restart_at: Mutex<Option<Instant>>,
    shutdown_token: CancellationToken,
    http_client: reqwest::Client,
}

impl LocalFilterManager {
    pub fn new(paths: AppPaths, events: Arc<dyn EventSink>) -> Self {
        let http_client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(PREDICT_HTTP_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            paths,
            events,
            op_lock: tokio::sync::Mutex::new(None),
            configure_lock: tokio::sync::Mutex::new(()),
            state: Mutex::new(CachedStatusState::default()),
            desired_params: RwLock::new(FilterModelParams::default()),
            applied_params: Mutex::new(None),
            last_auto_restart_at: Mutex::new(None),
            shutdown_token: CancellationToken::new(),
            http_client,
        }
    }

    pub fn set_desired_params(&self, params: &FilterModelParams) {
        *self.desired_params.write().unwrap() = params.sanitized();
    }

    pub fn is_deployed(&self) -> bool {
        let repo_dir = &self.paths.filter_repo_dir;
        if !repo_dir.join(".git").exists() {
            return false;
        }
        let deploy_dir = repo_dir.join("deploy");
        if !REQUIRED_DEPLOY_FILES
            .iter()
            .all(|name| deploy_dir.join(name).is_file())
        {
            return false;
        }
        venv_python_path(&self.paths.filter_venv_dir).is_file()
            && self.paths.filter_runner_path.is_file()
    }

    pub fn active_commit_sha(&self) -> Option<String> {
        if let Some(ref c) = self.state.lock().unwrap().current_commit {
            return Some(c.sha.clone());
        }
        read_git_head_sha_fast(&self.paths.filter_repo_dir)
    }

    pub fn active_endpoint(&self) -> Option<String> {
        if let Ok(mut guard) = self.op_lock.try_lock() {
            self.reap_exited_runner(&mut guard);
        }
        self.state.lock().unwrap().service_endpoint.clone()
    }

    pub async fn status(&self) -> LocalFilterDeploymentStatus {
        let op_busy = match self.op_lock.try_lock() {
            Ok(mut runner_guard) => {
                self.reap_exited_runner(&mut runner_guard);
                false
            }
            Err(_) => true,
        };

        if op_busy {
            let cached_env = self.state.lock().unwrap().last_env.clone();
            let env = match cached_env {
                Some(e) => e,
                None => tokio::task::spawn_blocking(probe_environment)
                    .await
                    .unwrap_or_default(),
            };
            let mut st = self.state.lock().unwrap();
            if st.last_env.is_none() {
                st.last_env = Some(env.clone());
            }
            return self.snapshot_from_state(env, &st);
        }

        let env = tokio::task::spawn_blocking(probe_environment)
            .await
            .unwrap_or_default();

        let local_git = if self.paths.filter_repo_dir.join(".git").exists() {
            inspect_local_repo_async(&self.shutdown_token, &self.paths.filter_repo_dir)
                .await
                .ok()
        } else {
            None
        };

        let mut st = self.state.lock().unwrap();
        st.last_env = Some(env.clone());
        // Never overwrite commit state if a concurrent deploy/start began while we probed env/git
        if !matches!(
            st.service_state,
            LocalFilterServiceState::Deploying | LocalFilterServiceState::Starting
        ) {
            if let Some((cur, rem, recent)) = local_git {
                st.current_commit = cur;
                if rem.is_some() {
                    st.remote_latest_commit = rem;
                }
                if !recent.is_empty() {
                    st.recent_commits = recent;
                }
                st.update_available = match (&st.current_commit, &st.remote_latest_commit) {
                    (Some(c), Some(r)) => c.sha != r.sha,
                    _ => false,
                };
            } else if !self.paths.filter_repo_dir.join(".git").exists() {
                st.current_commit = None;
            }
        }
        self.snapshot_from_state(env, &st)
    }

    pub async fn check_updates(
        &self,
        repo_url: &str,
        https_only: bool,
    ) -> Result<LocalFilterDeploymentStatus, CoreError> {
        let mut runner_guard = self.op_lock.lock().await;
        self.reap_exited_runner(&mut runner_guard);

        let validated_url = validate_repo_url(repo_url, https_only)?;
        let repo_dir = &self.paths.filter_repo_dir;

        let env = tokio::task::spawn_blocking(probe_environment)
            .await
            .map_err(|e| CoreError::Io(std::io::Error::other(e)))?;
        if !env.git_available {
            return Err(CoreError::Unsupported(
                "未检测到 Git，请先在系统中安装 Git 后再检查模型更新".into(),
            ));
        }

        let (current, remote, recent, update_available) = if repo_dir.join(".git").exists() {
            ensure_origin_url_async(&self.shutdown_token, repo_dir, &validated_url).await?;
            run_git_async(
                &self.shutdown_token,
                repo_dir,
                &["fetch", "--all", "--tags", "--prune"],
                GIT_NETWORK_TIMEOUT,
            )
            .await?;
            let _ = run_git_async(
                &self.shutdown_token,
                repo_dir,
                &["remote", "set-head", "origin", "-a"],
                GIT_NETWORK_TIMEOUT,
            )
            .await;

            let current = read_commit_info_async(&self.shutdown_token, repo_dir, "HEAD").await;
            let remote_ref = resolve_remote_default_ref_async(&self.shutdown_token, repo_dir)
                .await
                .unwrap_or_else(|| "HEAD".to_string());
            let remote = read_commit_info_async(&self.shutdown_token, repo_dir, &remote_ref).await;
            let recent =
                read_recent_commits_async(&self.shutdown_token, repo_dir, &remote_ref, 25).await;
            let update_available = match (&current, &remote) {
                (Some(c), Some(r)) => c.sha != r.sha,
                (None, Some(_)) => true,
                _ => false,
            };
            (current, remote, recent, update_available)
        } else {
            let remote = ls_remote_head_async(&self.shutdown_token, &validated_url).await?;
            let update_available = remote.is_some();
            (None, remote, Vec::new(), update_available)
        };

        let status = {
            let mut st = self.state.lock().unwrap();
            st.last_env = Some(env.clone());
            st.current_commit = current;
            st.remote_latest_commit = remote;
            st.recent_commits = recent;
            st.update_available = update_available;
            st.last_error = None;
            self.snapshot_from_state(env, &st)
        };
        self.events.emit(CoreEvent::LocalFilterStatusChanged {
            status: status.clone(),
        });
        Ok(status)
    }

    pub async fn deploy(
        &self,
        request: &DeployLocalFilterRequest,
        default_repo_url: &str,
        params: &FilterModelParams,
        https_only: bool,
    ) -> Result<LocalFilterDeploymentStatus, CoreError> {
        let mut runner_guard = self.op_lock.lock().await;

        let raw_url = request
            .repo_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(default_repo_url);
        let validated_url = validate_repo_url(raw_url, https_only)?;
        let target_commit = if request.track_latest {
            None
        } else {
            let raw_commit = request
                .target_commit
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    CoreError::InvalidInput(
                        "关闭跟随最新版本时必须指定非空的目标 Commit 或分支名称".into(),
                    )
                })?;
            Some(validate_target_commit(raw_commit)?)
        };

        self.set_desired_params(params);
        *self.last_auto_restart_at.lock().unwrap() = None;

        // Stop any running Python process before touching git checkout or venv files
        self.stop_runner_inner(&mut runner_guard).await;
        self.emit_stage(
            LocalFilterServiceState::Deploying,
            Some("正在检查本地 Git 与 Python 环境...".into()),
            None,
        );

        let deploy_res = self
            .run_deploy_steps(
                &validated_url,
                request.track_latest,
                target_commit.as_deref(),
            )
            .await;

        let (env, cur_commit, rem_commit, recent_commits) = match deploy_res {
            Ok(tuple) => tuple,
            Err(err) => {
                self.emit_stage(LocalFilterServiceState::Error, None, Some(err.to_string()));
                return Err(err);
            }
        };

        {
            let mut st = self.state.lock().unwrap();
            st.last_env = Some(env.clone());
            st.update_available = match (&cur_commit, &rem_commit) {
                (Some(c), Some(r)) => c.sha != r.sha,
                _ => false,
            };
            st.current_commit = cur_commit;
            st.remote_latest_commit = rem_commit;
            st.recent_commits = recent_commits;
            st.last_error = None;
        }

        if request.start_after_deploy {
            let latest_params = self.desired_params.read().unwrap().clone();
            if let Err(err) = self
                .start_runner_locked(&mut runner_guard, &latest_params)
                .await
            {
                self.emit_stage(LocalFilterServiceState::Error, None, Some(err.to_string()));
                return Err(err);
            }
        } else {
            let mut st = self.state.lock().unwrap();
            st.service_state = LocalFilterServiceState::Stopped;
            st.service_endpoint = None;
            st.running_pid = None;
            st.active_runner_info = None;
            st.stage_message = None;
            st.last_error = None;
        }

        let status = {
            let st = self.state.lock().unwrap();
            self.snapshot_from_state(env, &st)
        };
        self.events.emit(CoreEvent::LocalFilterStatusChanged {
            status: status.clone(),
        });
        Ok(status)
    }

    async fn run_deploy_steps(
        &self,
        validated_url: &str,
        track_latest: bool,
        target_commit: Option<&str>,
    ) -> Result<
        (
            FilterEnvReport,
            Option<FilterCommitInfo>,
            Option<FilterCommitInfo>,
            Vec<FilterCommitInfo>,
        ),
        CoreError,
    > {
        let env = tokio::task::spawn_blocking(probe_environment)
            .await
            .map_err(|e| CoreError::Io(std::io::Error::other(e)))?;

        if !env.git_available || !env.python_available || !env.venv_available {
            let mut missing = Vec::new();
            if !env.git_available {
                missing.push("Git");
            }
            if !env.python_available {
                missing.push("Python 3.9+");
            } else if !env.venv_available {
                missing.push("Python venv / ensurepip 模块 (如 Debian/Ubuntu 请安装 python3-venv)");
            }
            return Err(CoreError::Unsupported(format!(
                "缺少本地部署前置环境：{}。Left4Addons 不会自动安装系统级前置依赖，请先手动安装后重试。",
                missing.join("、")
            )));
        }

        let sys_python = env
            .python_path
            .clone()
            .ok_or_else(|| CoreError::Unsupported("未找到可用的 Python 3.9+ 可执行文件".into()))?;

        self.emit_stage(
            LocalFilterServiceState::Deploying,
            Some("正在同步模型仓库与检出目标版本...".into()),
            None,
        );

        fs::create_dir_all(&self.paths.filters_dir)?;
        let repo_dir = &self.paths.filter_repo_dir;

        if !repo_dir.join(".git").exists() {
            if repo_dir.exists() {
                fs::remove_dir_all(repo_dir)?;
            }
            let repo_str = clean_os_path(repo_dir);
            run_git_async(
                &self.shutdown_token,
                &self.paths.filters_dir,
                &["clone", "--no-checkout", "--", validated_url, &repo_str],
                GIT_NETWORK_TIMEOUT,
            )
            .await?;
        } else {
            ensure_origin_url_async(&self.shutdown_token, repo_dir, validated_url).await?;
            run_git_async(
                &self.shutdown_token,
                repo_dir,
                &["fetch", "--all", "--tags", "--prune"],
                GIT_NETWORK_TIMEOUT,
            )
            .await?;
        }
        let _ = run_git_async(
            &self.shutdown_token,
            repo_dir,
            &["remote", "set-head", "origin", "-a"],
            GIT_NETWORK_TIMEOUT,
        )
        .await;

        if track_latest {
            let remote_ref = resolve_remote_default_ref_async(&self.shutdown_token, repo_dir)
                .await
                .ok_or_else(|| CoreError::InvalidInput("无法解析模型仓库默认远端分支".into()))?;
            let sha = rev_parse_async(&self.shutdown_token, repo_dir, &remote_ref).await?;
            run_git_async(
                &self.shutdown_token,
                repo_dir,
                &["checkout", "--force", "--detach", &sha],
                GIT_FAST_TIMEOUT,
            )
            .await?;
        } else {
            let target = target_commit.ok_or_else(|| {
                CoreError::InvalidInput("关闭跟随最新版本时必须指定目标 Commit".into())
            })?;
            let verify_spec = format!("{target}^{{commit}}");
            if run_git_async(
                &self.shutdown_token,
                repo_dir,
                &["cat-file", "-e", &verify_spec],
                GIT_FAST_TIMEOUT,
            )
            .await
            .is_err()
            {
                run_git_async(
                    &self.shutdown_token,
                    repo_dir,
                    &["fetch", "origin", target],
                    GIT_NETWORK_TIMEOUT,
                )
                .await?;
            }
            run_git_async(
                &self.shutdown_token,
                repo_dir,
                &["checkout", "--force", "--detach", target],
                GIT_FAST_TIMEOUT,
            )
            .await?;
        }

        let deploy_dir = repo_dir.join("deploy");
        for req_file in REQUIRED_DEPLOY_FILES {
            if !deploy_dir.join(req_file).is_file() {
                return Err(CoreError::InvalidInput(format!(
                    "模型仓库缺少必要的部署文件: deploy/{req_file}"
                )));
            }
        }

        let (cur_commit, rem_commit, recent_commits) =
            inspect_local_repo_async(&self.shutdown_token, repo_dir).await?;

        self.emit_stage(
            LocalFilterServiceState::Deploying,
            Some("正在配置 Python 虚拟环境与推理依赖...".into()),
            None,
        );

        let venv_py = venv_python_path(&self.paths.filter_venv_dir);
        if !venv_py.is_file() {
            let venv_str = clean_os_path(&self.paths.filter_venv_dir);
            let out = run_cancellable_command(
                &self.shutdown_token,
                OsStr::new(&sys_python),
                None,
                &["-m", "venv", &venv_str],
                &[("PYTHONIOENCODING", "utf-8")],
                VENV_CREATE_TIMEOUT,
            )
            .await?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                return Err(CoreError::Unsupported(format!(
                    "创建 Python 虚拟环境失败: {stderr}"
                )));
            }
        }

        let venv_py = venv_python_path(&self.paths.filter_venv_dir);
        if !venv_py.is_file() {
            return Err(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Python 虚拟环境创建后未找到解释器",
            )));
        }

        let req_path = repo_dir.join("deploy").join("requirements.txt");
        let req_content = fs::read_to_string(&req_path)?;
        let has_packages = req_content
            .lines()
            .map(str::trim)
            .any(|line| !line.is_empty() && !line.starts_with('#'));

        if has_packages {
            let req_str = clean_os_path(&req_path);
            let pip_out = run_cancellable_command(
                &self.shutdown_token,
                venv_py.as_os_str(),
                None,
                &[
                    "-m",
                    "pip",
                    "install",
                    "--disable-pip-version-check",
                    "--timeout",
                    "20",
                    "--retries",
                    "2",
                    "-r",
                    &req_str,
                ],
                &[("PIP_NO_INPUT", "1"), ("PYTHONIOENCODING", "utf-8")],
                PIP_INSTALL_TIMEOUT,
            )
            .await?;
            if !pip_out.status.success()
                && !venv_deps_importable_async(&self.shutdown_token, &venv_py).await
            {
                let stderr = String::from_utf8_lossy(&pip_out.stderr).trim().to_string();
                return Err(CoreError::Network(format!(
                    "安装模型 Python 依赖失败: {stderr}"
                )));
            }
        }

        fs::write(&self.paths.filter_runner_path, FILTER_RUNNER_PY)?;

        Ok((env, cur_commit, rem_commit, recent_commits))
    }

    pub async fn start_service(
        &self,
        params: &FilterModelParams,
    ) -> Result<LocalFilterDeploymentStatus, CoreError> {
        self.set_desired_params(params);
        *self.last_auto_restart_at.lock().unwrap() = None;
        let mut runner_guard = self.op_lock.lock().await;
        self.reap_exited_runner(&mut runner_guard);
        if runner_guard.is_none() {
            let latest = self.desired_params.read().unwrap().clone();
            if let Err(err) = self.start_runner_locked(&mut runner_guard, &latest).await {
                self.emit_stage(LocalFilterServiceState::Error, None, Some(err.to_string()));
                return Err(err);
            }
        }
        drop(runner_guard);
        let _ = self.reconfigure_params(params).await;
        let status = self.status().await;
        self.events.emit(CoreEvent::LocalFilterStatusChanged {
            status: status.clone(),
        });
        Ok(status)
    }

    pub async fn ensure_started_endpoint(
        &self,
        params: &FilterModelParams,
    ) -> Result<String, CoreError> {
        self.set_desired_params(params);
        let mut runner_guard = self.op_lock.lock().await;
        self.reap_exited_runner(&mut runner_guard);
        if let Some(ref running) = *runner_guard {
            let endpoint = running.endpoint.clone();
            drop(runner_guard);
            let _ = self.ensure_params_synced().await;
            return Ok(endpoint);
        }

        let recovering_from_crash =
            self.state.lock().unwrap().service_state == LocalFilterServiceState::Error;
        if recovering_from_crash {
            let mut last_restart = self.last_auto_restart_at.lock().unwrap();
            if last_restart.is_some_and(|t| t.elapsed() < AUTO_RESTART_COOLDOWN) {
                return Err(CoreError::Network(
                    "本地过滤守护进程异常退出，自动重启冷却中 (3s)，请稍后重试".into(),
                ));
            }
            *last_restart = Some(Instant::now());
        }

        let latest = self.desired_params.read().unwrap().clone();
        self.start_runner_locked(&mut runner_guard, &latest).await?;
        let endpoint = runner_guard
            .as_ref()
            .map(|r| r.endpoint.clone())
            .ok_or_else(|| CoreError::Network("本地过滤服务启动后无可用地址".into()))?;
        let st_copy = {
            let st = self.state.lock().unwrap();
            let env = st.last_env.clone().unwrap_or_default();
            self.snapshot_from_state(env, &st)
        };
        drop(runner_guard);
        self.events
            .emit(CoreEvent::LocalFilterStatusChanged { status: st_copy });
        Ok(endpoint)
    }

    pub async fn stop_service(&self) -> Result<LocalFilterDeploymentStatus, CoreError> {
        let mut runner_guard = self.op_lock.lock().await;
        self.stop_runner_inner(&mut runner_guard).await;
        {
            let mut st = self.state.lock().unwrap();
            st.service_state = LocalFilterServiceState::Stopped;
            st.service_endpoint = None;
            st.running_pid = None;
            st.active_runner_info = None;
            st.stage_message = None;
        }
        drop(runner_guard);
        let status = self.status().await;
        self.events.emit(CoreEvent::LocalFilterStatusChanged {
            status: status.clone(),
        });
        Ok(status)
    }

    pub async fn shutdown(&self) {
        self.shutdown_token.cancel();
        if let Ok(mut runner_guard) =
            tokio::time::timeout(Duration::from_secs(5), self.op_lock.lock()).await
        {
            self.stop_runner_inner(&mut runner_guard).await;
        }
        let mut st = self.state.lock().unwrap();
        st.service_state = LocalFilterServiceState::Stopped;
        st.service_endpoint = None;
        st.running_pid = None;
        st.active_runner_info = None;
        st.stage_message = None;
    }

    pub async fn reconfigure_params(&self, params: &FilterModelParams) -> Result<(), CoreError> {
        self.set_desired_params(params);
        self.ensure_params_synced().await
    }

    async fn ensure_params_synced(&self) -> Result<(), CoreError> {
        let _cfg_guard = self.configure_lock.lock().await;
        if let Ok(mut rg) = self.op_lock.try_lock() {
            self.reap_exited_runner(&mut rg);
        }
        let info = self.state.lock().unwrap().active_runner_info.clone();
        let Some((port, token)) = info else {
            return Ok(());
        };
        let target_params = self.desired_params.read().unwrap().clone();
        if self.applied_params.lock().unwrap().as_ref() == Some(&target_params) {
            return Ok(());
        }

        match self.post_configure_http(port, &token, &target_params).await {
            Ok(()) => {
                *self.applied_params.lock().unwrap() = Some(target_params);
                Ok(())
            }
            Err(PredictLocalError::Transport(err)) => {
                if let Ok(mut rg) = self.op_lock.try_lock() {
                    self.reap_exited_runner(&mut rg);
                }
                Err(err)
            }
            Err(PredictLocalError::HttpStatus(err)) => Err(err),
        }
    }

    async fn post_configure_http(
        &self,
        port: u16,
        token: &str,
        params: &FilterModelParams,
    ) -> Result<(), PredictLocalError> {
        let url = format!("http://127.0.0.1:{port}/configure");
        let resp = self
            .http_client
            .post(&url)
            .timeout(CONFIGURE_HTTP_TIMEOUT)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
            .json(&params.sanitized())
            .send()
            .await
            .map_err(|e| {
                PredictLocalError::Transport(CoreError::Network(format!(
                    "同步本地过滤运行参数失败: {e}"
                )))
            })?;
        if !resp.status().is_success() {
            return Err(PredictLocalError::HttpStatus(CoreError::Network(format!(
                "同步本地过滤运行参数返回 HTTP {}",
                resp.status()
            ))));
        }
        Ok(())
    }

    pub async fn predict_local(
        &self,
        title: &str,
        description: Option<&str>,
        tags: &[String],
        images_base64: &[String],
        auto_restart: bool,
        params: &FilterModelParams,
    ) -> Result<FilterProbabilities, CoreError> {
        self.set_desired_params(params);
        let _ = self.ensure_params_synced().await;

        let endpoint = match self.active_endpoint() {
            Some(ep) => ep,
            None => {
                if auto_restart && self.is_deployed() {
                    self.ensure_started_endpoint(params).await?
                } else {
                    return Err(CoreError::InvalidInput(
                        "本地过滤守护进程未运行，请先部署或启动服务".into(),
                    ));
                }
            }
        };

        let payload = serde_json::json!({
            "title": title,
            "description": description.unwrap_or(""),
            "tags": tags,
            "imagesBase64": images_base64,
        });

        match self.send_predict_http(&endpoint, &payload).await {
            Ok(probs) => Ok(probs),
            Err(PredictLocalError::HttpStatus(err)) => Err(err),
            Err(PredictLocalError::Transport(err)) => {
                let mut guard = self.op_lock.lock().await;
                self.reap_exited_runner(&mut guard);

                // If the runner bound to `endpoint` suffered a transport failure, terminate it before recovery
                if guard.as_ref().is_some_and(|r| r.endpoint == endpoint) {
                    self.stop_runner_inner(&mut guard).await;
                    let mut st = self.state.lock().unwrap();
                    st.service_state = LocalFilterServiceState::Error;
                    st.service_endpoint = None;
                    st.running_pid = None;
                    st.active_runner_info = None;
                    st.last_error = Some(err.to_string());
                }

                if !auto_restart || !self.is_deployed() {
                    return Err(err);
                }

                // Reuse if another concurrent caller already restarted a fresh runner
                let retry_endpoint = if let Some(ref running) = *guard {
                    running.endpoint.clone()
                } else {
                    {
                        let mut last_restart = self.last_auto_restart_at.lock().unwrap();
                        if last_restart.is_some_and(|t| t.elapsed() < AUTO_RESTART_COOLDOWN) {
                            return Err(err);
                        }
                        *last_restart = Some(Instant::now());
                    }
                    let latest = self.desired_params.read().unwrap().clone();
                    self.start_runner_locked(&mut guard, &latest).await?;
                    let ep = guard
                        .as_ref()
                        .map(|r| r.endpoint.clone())
                        .ok_or_else(|| CoreError::Network("本地过滤服务重启后无可用地址".into()))?;
                    let st_copy = {
                        let st = self.state.lock().unwrap();
                        let env = st.last_env.clone().unwrap_or_default();
                        self.snapshot_from_state(env, &st)
                    };
                    self.events
                        .emit(CoreEvent::LocalFilterStatusChanged { status: st_copy });
                    ep
                };
                drop(guard);

                match self.send_predict_http(&retry_endpoint, &payload).await {
                    Ok(probs) => Ok(probs),
                    Err(PredictLocalError::Transport(e) | PredictLocalError::HttpStatus(e)) => {
                        Err(e)
                    }
                }
            }
        }
    }

    async fn send_predict_http(
        &self,
        endpoint: &str,
        payload: &serde_json::Value,
    ) -> Result<FilterProbabilities, PredictLocalError> {
        let url = format!("{}/predict", endpoint.trim_end_matches('/'));
        let resp = self
            .http_client
            .post(&url)
            .timeout(PREDICT_HTTP_TIMEOUT)
            .json(payload)
            .send()
            .await
            .map_err(|e| {
                PredictLocalError::Transport(CoreError::Network(format!(
                    "调用本地过滤模型推理失败: {e}"
                )))
            })?;
        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().await.unwrap_or_default();
            return Err(PredictLocalError::HttpStatus(CoreError::Network(format!(
                "本地过滤模型推理返回 HTTP {status}: {detail}"
            ))));
        }
        resp.json::<FilterProbabilities>().await.map_err(|e| {
            PredictLocalError::HttpStatus(CoreError::Network(format!(
                "解析本地过滤模型输出失败: {e}"
            )))
        })
    }

    async fn start_runner_locked(
        &self,
        runner_guard: &mut Option<RunningRunner>,
        params: &FilterModelParams,
    ) -> Result<(), CoreError> {
        let deploy_dir = self.paths.filter_repo_dir.join("deploy");
        let venv_py = venv_python_path(&self.paths.filter_venv_dir);
        if !REQUIRED_DEPLOY_FILES
            .iter()
            .all(|f| deploy_dir.join(f).is_file())
            || !venv_py.is_file()
        {
            return Err(CoreError::InvalidInput(
                "本地过滤模型尚未完成部署，请先执行一键部署".into(),
            ));
        }

        fs::create_dir_all(&self.paths.filters_dir)?;
        let need_write_runner = fs::read_to_string(&self.paths.filter_runner_path)
            .map(|existing| existing != FILTER_RUNNER_PY)
            .unwrap_or(true);
        if need_write_runner {
            fs::write(&self.paths.filter_runner_path, FILTER_RUNNER_PY)?;
        }

        self.emit_stage(
            LocalFilterServiceState::Starting,
            Some("正在启动本地过滤推理守护进程...".into()),
            None,
        );

        let rng = SystemRandom::new();
        let mut token_bytes = [0u8; 32];
        rng.fill(&mut token_bytes)
            .map_err(|_| CoreError::Config("生成守护进程鉴权令牌失败".into()))?;
        let runner_token = hex::encode(token_bytes);

        let boot_params = params.sanitized();
        let params_json =
            serde_json::to_string(&boot_params).map_err(|e| CoreError::Config(e.to_string()))?;

        let mut cmd = tokio::process::Command::new(&venv_py);
        cmd.args([
            &clean_os_path(&self.paths.filter_runner_path),
            "--host",
            "127.0.0.1",
            "--port",
            "0",
            "--deploy-dir",
            &clean_os_path(&self.paths.filter_repo_dir),
            "--params-json",
            &params_json,
            "--watch-stdin",
        ])
        .env("L4A_RUNNER_TOKEN", &runner_token)
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
        configure_tokio_cmd(&mut cmd);

        let mut child = cmd.spawn()?;
        let pid = child.id();
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| CoreError::Io(std::io::Error::other("无法打开守护进程标准输入管道")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CoreError::Io(std::io::Error::other("无法捕获守护进程标准输出")))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| CoreError::Io(std::io::Error::other("无法捕获守护进程错误输出")))?;

        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let stderr_tail_writer = stderr_tail.clone();
        tokio::spawn(async move {
            let mut stderr = stderr;
            let mut buf = [0u8; 4096];
            loop {
                match stderr.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(&buf[..n]);
                        let mut guard = stderr_tail_writer.lock().unwrap();
                        guard.push_str(&chunk);
                        if guard.len() > MAX_STDERR_TAIL_BYTES {
                            let excess = guard.len() - MAX_STDERR_TAIL_BYTES;
                            let cut = guard
                                .char_indices()
                                .find(|&(idx, _)| idx >= excess)
                                .map(|(idx, _)| idx)
                                .unwrap_or(guard.len());
                            guard.drain(..cut);
                        }
                    }
                }
            }
        });

        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<ReadyBanner, String>>();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut total_scanned: usize = 0;
            let mut line_buf: Vec<u8> = Vec::with_capacity(512);
            let mut tx_opt = Some(ready_tx);

            while tx_opt.is_some() {
                line_buf.clear();
                let remaining = MAX_READY_BANNER_BYTES.saturating_sub(total_scanned);
                if remaining == 0 {
                    if let Some(tx) = tx_opt.take() {
                        let _ = tx.send(Err(
                            "守护进程启动输出超出 64 KiB 上限仍未收到就绪信标".into()
                        ));
                    }
                    break;
                }
                let mut bounded = (&mut reader).take(remaining as u64);
                match bounded.read_until(b'\n', &mut line_buf).await {
                    Ok(0) => {
                        if let Some(tx) = tx_opt.take() {
                            let _ = tx.send(Err("本地过滤守护进程在就绪前提前退出".into()));
                        }
                        return;
                    }
                    Ok(n) => {
                        total_scanned += n;
                        let line = String::from_utf8_lossy(&line_buf);
                        if let Some(json_part) = line.trim().strip_prefix("L4A_FILTER_READY ") {
                            if let Some(tx) = tx_opt.take() {
                                let parsed = serde_json::from_str::<ReadyBanner>(json_part)
                                    .map_err(|e| format!("解析守护进程就绪信标失败: {e}"));
                                let _ = tx.send(parsed);
                            }
                            break;
                        }
                    }
                    Err(e) => {
                        if let Some(tx) = tx_opt.take() {
                            let _ = tx.send(Err(format!("读取守护进程标准输出失败: {e}")));
                        }
                        return;
                    }
                }
            }

            // Drain post-ready stdout using raw byte chunk reads so non-UTF-8 output never causes SIGPIPE
            let mut drain_buf = [0u8; 4096];
            loop {
                match reader.read(&mut drain_buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });

        let banner_res = tokio::select! {
            () = self.shutdown_token.cancelled() => {
                drop(stdin);
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(CoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "应用正在关闭，已取消本地过滤服务启动",
                )));
            }
            res = tokio::time::timeout(Duration::from_secs(30), ready_rx) => res,
        };

        let banner = match banner_res {
            Ok(Ok(Ok(b))) => b,
            Ok(Ok(Err(msg))) => {
                drop(stdin);
                let _ = child.kill().await;
                let _ = child.wait().await;
                let err_detail = stderr_tail.lock().unwrap().trim().to_string();
                let full_msg = if err_detail.is_empty() {
                    msg
                } else {
                    format!("{msg}: {err_detail}")
                };
                return Err(CoreError::Unsupported(full_msg));
            }
            Ok(Err(_)) => {
                drop(stdin);
                let _ = child.kill().await;
                let _ = child.wait().await;
                let err_detail = stderr_tail.lock().unwrap().trim().to_string();
                return Err(CoreError::Unsupported(format!(
                    "本地过滤守护进程启动中断{}",
                    if err_detail.is_empty() {
                        String::new()
                    } else {
                        format!(": {err_detail}")
                    }
                )));
            }
            Err(_) => {
                drop(stdin);
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(CoreError::Network(
                    "等待本地过滤守护进程就绪超时 (30s)".into(),
                ));
            }
        };

        if banner.port == 0 {
            drop(stdin);
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(CoreError::Unsupported(
                "本地过滤守护进程返回了无效监听端口 0".into(),
            ));
        }

        let endpoint = format!("http://127.0.0.1:{}", banner.port);

        // Re-check latest desired_params in case save_config updated them while Python was booting
        let latest_desired = self.desired_params.read().unwrap().clone();
        let active_applied = if latest_desired != boot_params {
            match self
                .post_configure_http(banner.port, &runner_token, &latest_desired)
                .await
            {
                Ok(()) => latest_desired,
                Err(PredictLocalError::Transport(err) | PredictLocalError::HttpStatus(err)) => {
                    tracing::warn!(error = %err, "守护进程启动后同步最新过滤参数失败");
                    boot_params
                }
            }
        } else {
            boot_params
        };
        *self.applied_params.lock().unwrap() = Some(active_applied);

        {
            let mut st = self.state.lock().unwrap();
            st.service_state = LocalFilterServiceState::Running;
            st.service_endpoint = Some(endpoint.clone());
            st.running_pid = pid;
            st.active_runner_info = Some((banner.port, runner_token.clone()));
            st.stage_message = None;
            st.last_error = None;
        }

        *runner_guard = Some(RunningRunner {
            child,
            stdin: Some(stdin),
            pid,
            endpoint,
            port: banner.port,
            runner_token,
            stderr_tail,
        });
        Ok(())
    }

    fn reap_exited_runner(&self, runner_guard: &mut Option<RunningRunner>) {
        let exited = if let Some(ref mut r) = *runner_guard {
            match r.child.try_wait() {
                Ok(Some(status)) => {
                    let _ = (r.pid, r.port, &r.runner_token);
                    let tail = r.stderr_tail.lock().unwrap().trim().to_string();
                    Some(if tail.is_empty() {
                        format!("本地过滤守护进程已退出 ({status})")
                    } else {
                        format!("本地过滤守护进程异常退出 ({status}): {tail}")
                    })
                }
                Ok(None) => None,
                Err(err) => Some(format!("检测本地过滤守护进程状态失败: {err}")),
            }
        } else {
            None
        };

        if let Some(err_msg) = exited {
            *runner_guard = None;
            *self.applied_params.lock().unwrap() = None;
            let status_snapshot = {
                let mut st = self.state.lock().unwrap();
                st.service_state = LocalFilterServiceState::Error;
                st.service_endpoint = None;
                st.running_pid = None;
                st.active_runner_info = None;
                st.stage_message = None;
                st.last_error = Some(err_msg);
                let env = st.last_env.clone().unwrap_or_default();
                self.snapshot_from_state(env, &st)
            };
            self.events.emit(CoreEvent::LocalFilterStatusChanged {
                status: status_snapshot,
            });
        }
    }

    async fn stop_runner_inner(&self, runner_guard: &mut Option<RunningRunner>) {
        *self.applied_params.lock().unwrap() = None;
        if let Some(mut running) = runner_guard.take() {
            drop(running.stdin.take());
            let _ = running.child.kill().await;
            let _ = tokio::time::timeout(Duration::from_secs(3), running.child.wait()).await;
        }
    }

    fn emit_stage(
        &self,
        service_state: LocalFilterServiceState,
        stage_message: Option<String>,
        last_error: Option<String>,
    ) {
        let status = {
            let mut st = self.state.lock().unwrap();
            st.service_state = service_state;
            if !matches!(service_state, LocalFilterServiceState::Running) {
                st.service_endpoint = None;
                st.running_pid = None;
                st.active_runner_info = None;
            }
            st.stage_message = stage_message;
            if last_error.is_some() {
                st.last_error = last_error;
            }
            let env = st.last_env.clone().unwrap_or_default();
            self.snapshot_from_state(env, &st)
        };
        self.events
            .emit(CoreEvent::LocalFilterStatusChanged { status });
    }

    fn snapshot_from_state(
        &self,
        env: FilterEnvReport,
        st: &CachedStatusState,
    ) -> LocalFilterDeploymentStatus {
        LocalFilterDeploymentStatus {
            env,
            deployed: self.is_deployed(),
            repo_dir: clean_os_path(&self.paths.filter_repo_dir),
            venv_dir: clean_os_path(&self.paths.filter_venv_dir),
            service_state: st.service_state,
            service_endpoint: st.service_endpoint.clone(),
            running_pid: st.running_pid,
            current_commit: st.current_commit.clone(),
            remote_latest_commit: st.remote_latest_commit.clone(),
            update_available: st.update_available,
            recent_commits: st.recent_commits.clone(),
            stage_message: st.stage_message.clone(),
            last_error: st.last_error.clone(),
        }
    }
}

fn read_git_head_sha_fast(repo_dir: &Path) -> Option<String> {
    let git_dir = repo_dir.join(".git");
    let head_raw = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let trimmed = head_raw.trim();
    if trimmed.len() == 40 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(trimmed.to_ascii_lowercase());
    }
    if let Some(rel_ref) = trimmed.strip_prefix("ref:") {
        let rel_ref = rel_ref.trim();
        if rel_ref.is_empty() || rel_ref.contains("..") {
            return None;
        }
        if let Ok(ref_sha) = fs::read_to_string(git_dir.join(rel_ref)) {
            let s = ref_sha.trim();
            if s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Some(s.to_ascii_lowercase());
            }
        }
        if let Ok(packed) = fs::read_to_string(git_dir.join("packed-refs")) {
            for line in packed.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') || line.starts_with('^') {
                    continue;
                }
                if let Some((sha, rname)) = line.split_once(' ') {
                    if rname.trim() == rel_ref
                        && sha.len() == 40
                        && sha.bytes().all(|b| b.is_ascii_hexdigit())
                    {
                        return Some(sha.to_ascii_lowercase());
                    }
                }
            }
        }
    }
    None
}

async fn run_cancellable_command(
    cancel: &CancellationToken,
    program: &OsStr,
    cwd: Option<&Path>,
    args: &[&str],
    envs: &[(&str, &str)],
    timeout_dur: Duration,
) -> Result<std::process::Output, CoreError> {
    if cancel.is_cancelled() {
        return Err(CoreError::Io(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "操作已取消",
        )));
    }

    let mut cmd = tokio::process::Command::new(program);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for &(k, v) in envs {
        cmd.env(k, v);
    }
    configure_tokio_cmd(&mut cmd);

    let child = cmd.spawn()?;
    tokio::select! {
        () = cancel.cancelled() => {
            Err(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "操作在应用退出时被取消",
            )))
        }
        res = tokio::time::timeout(timeout_dur, child.wait_with_output()) => {
            match res {
                Ok(Ok(output)) => Ok(output),
                Ok(Err(err)) => Err(CoreError::Io(err)),
                Err(_) => Err(CoreError::Network(format!(
                    "外部命令执行超时 ({}s)",
                    timeout_dur.as_secs()
                ))),
            }
        }
    }
}

async fn venv_deps_importable_async(cancel: &CancellationToken, venv_py: &Path) -> bool {
    run_cancellable_command(
        cancel,
        venv_py.as_os_str(),
        None,
        &["-c", "import onnxruntime, numpy, PIL"],
        &[("PYTHONIOENCODING", "utf-8")],
        Duration::from_secs(20),
    )
    .await
    .is_ok_and(|out| out.status.success())
}

async fn run_git_async(
    cancel: &CancellationToken,
    cwd: &Path,
    args: &[&str],
    timeout_dur: Duration,
) -> Result<String, CoreError> {
    let out = run_cancellable_command(
        cancel,
        OsStr::new("git"),
        Some(cwd),
        args,
        GIT_STALL_ENVS,
        timeout_dur,
    )
    .await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(CoreError::Network(format!("Git 命令执行失败: {stderr}")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn ensure_origin_url_async(
    cancel: &CancellationToken,
    repo_dir: &Path,
    url: &str,
) -> Result<(), CoreError> {
    if run_git_async(
        cancel,
        repo_dir,
        &["remote", "set-url", "origin", "--", url],
        GIT_FAST_TIMEOUT,
    )
    .await
    .is_err()
    {
        run_git_async(
            cancel,
            repo_dir,
            &["remote", "add", "origin", "--", url],
            GIT_FAST_TIMEOUT,
        )
        .await?;
    }
    Ok(())
}

async fn rev_parse_async(
    cancel: &CancellationToken,
    repo_dir: &Path,
    rev: &str,
) -> Result<String, CoreError> {
    let out = run_git_async(
        cancel,
        repo_dir,
        &["rev-parse", "--verify", rev],
        GIT_FAST_TIMEOUT,
    )
    .await?;
    let sha = out.trim().to_string();
    if sha.is_empty() {
        return Err(CoreError::InvalidInput(format!("无法解析 Git 引用: {rev}")));
    }
    Ok(sha)
}

async fn resolve_remote_default_ref_async(
    cancel: &CancellationToken,
    repo_dir: &Path,
) -> Option<String> {
    if let Ok(sym) = run_git_async(
        cancel,
        repo_dir,
        &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"],
        GIT_FAST_TIMEOUT,
    )
    .await
    {
        let trimmed = sym.trim();
        if !trimmed.is_empty() && rev_parse_async(cancel, repo_dir, trimmed).await.is_ok() {
            return Some(trimmed.to_string());
        }
    }
    for candidate in ["origin/main", "origin/master", "FETCH_HEAD", "HEAD"] {
        if rev_parse_async(cancel, repo_dir, candidate).await.is_ok() {
            return Some(candidate.to_string());
        }
    }
    None
}

fn parse_commit_line(line: &str) -> Option<FilterCommitInfo> {
    let mut parts = line.trim().splitn(4, '\x1f');
    let sha = parts.next()?.trim();
    let short_sha = parts.next()?.trim();
    let subject = parts.next()?.trim();
    let committed_at = parts.next()?.trim();
    if sha.is_empty() {
        return None;
    }
    Some(FilterCommitInfo {
        sha: sha.to_string(),
        short_sha: if short_sha.is_empty() {
            sha.chars().take(7).collect()
        } else {
            short_sha.to_string()
        },
        subject: subject.to_string(),
        committed_at: committed_at.to_string(),
    })
}

async fn read_commit_info_async(
    cancel: &CancellationToken,
    repo_dir: &Path,
    rev: &str,
) -> Option<FilterCommitInfo> {
    let out = run_git_async(
        cancel,
        repo_dir,
        &["log", rev, "-n", "1", "--format=%H%x1f%h%x1f%s%x1f%cI"],
        GIT_FAST_TIMEOUT,
    )
    .await
    .ok()?;
    out.lines().find_map(parse_commit_line)
}

async fn read_recent_commits_async(
    cancel: &CancellationToken,
    repo_dir: &Path,
    rev: &str,
    limit: usize,
) -> Vec<FilterCommitInfo> {
    let limit_str = limit.to_string();
    let Ok(out) = run_git_async(
        cancel,
        repo_dir,
        &[
            "log",
            rev,
            "-n",
            &limit_str,
            "--format=%H%x1f%h%x1f%s%x1f%cI",
        ],
        GIT_FAST_TIMEOUT,
    )
    .await
    else {
        return Vec::new();
    };
    out.lines().filter_map(parse_commit_line).collect()
}

async fn inspect_local_repo_async(
    cancel: &CancellationToken,
    repo_dir: &Path,
) -> Result<
    (
        Option<FilterCommitInfo>,
        Option<FilterCommitInfo>,
        Vec<FilterCommitInfo>,
    ),
    CoreError,
> {
    if cancel.is_cancelled() {
        return Err(CoreError::Io(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "已取消读取仓库状态",
        )));
    }
    let cur = read_commit_info_async(cancel, repo_dir, "HEAD").await;
    let rem_ref = resolve_remote_default_ref_async(cancel, repo_dir).await;
    let rem = match rem_ref.as_deref() {
        Some(r) => read_commit_info_async(cancel, repo_dir, r).await,
        None => None,
    };
    let recent = match rem_ref.as_deref() {
        Some(r) => read_recent_commits_async(cancel, repo_dir, r, 25).await,
        None => Vec::new(),
    };
    Ok((cur, rem, recent))
}

async fn ls_remote_head_async(
    cancel: &CancellationToken,
    repo_url: &str,
) -> Result<Option<FilterCommitInfo>, CoreError> {
    let out = run_cancellable_command(
        cancel,
        OsStr::new("git"),
        None,
        &["ls-remote", "--", repo_url, "HEAD"],
        GIT_STALL_ENVS,
        GIT_FAST_TIMEOUT,
    )
    .await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(CoreError::Network(format!(
            "读取远端模型仓库 HEAD 失败: {stderr}"
        )));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        if let Some((sha, _)) = line.trim().split_once('\t') {
            let sha = sha.trim();
            if sha.len() >= 7 && sha.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Ok(Some(FilterCommitInfo {
                    sha: sha.to_string(),
                    short_sha: sha[..7].to_string(),
                    subject: "Remote HEAD".to_string(),
                    committed_at: String::new(),
                }));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_repo_url_rejects_unc_remote_file_hosts_and_http_in_https_only() {
        for bad in [
            r"\\attacker.example.com\share\repo",
            r"\\?\UNC\attacker.example.com\share\repo",
            "//attacker.example.com/share/repo",
            "file://attacker.example.com/share/repo",
            "file://192.168.1.10/share/repo",
            "file:////attacker.example.com/share/repo",
            "-oProxyCommand=evil",
            "ssh://git@github.com/FallenCrystal/Left4Filters.git",
        ] {
            assert!(
                validate_repo_url(bad, false).is_err(),
                "expected {bad:?} to be rejected"
            );
        }

        assert!(validate_repo_url("http://example.com/repo.git", true).is_err());
        assert!(validate_repo_url("http://127.0.0.1:8080/repo.git", true).is_ok());
        assert!(
            validate_repo_url("https://github.com/FallenCrystal/Left4Filters.git", true).is_ok()
        );
    }
}
