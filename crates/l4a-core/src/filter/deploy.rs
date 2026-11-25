use crate::config::AppPaths;
use crate::engine::EventSink;
use crate::error::CoreError;
use l4a_protocol::{
    CoreEvent, DeployLocalFilterRequest, FilterCommitInfo, FilterEnvReport, FilterModelParams,
    FilterProbabilities, LocalFilterDeploymentStatus, LocalFilterServiceState, FILTER_RUNNER_PY,
};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Deserialize;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;

const REQUIRED_DEPLOY_FILES: &[&str] = &[
    "l4filter.py",
    "vision_encoder.onnx",
    "fusion_head.onnx",
    "text_vocab.json",
    "requirements.txt",
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

    let as_path = Path::new(trimmed);
    if as_path.is_dir() {
        let canon = fs::canonicalize(as_path)?;
        return Ok(clean_os_path(&canon));
    }

    let parsed = url::Url::parse(trimmed)
        .map_err(|_| CoreError::InvalidInput("模型仓库地址必须是有效的 URL 或本地目录".into()))?;

    match parsed.scheme() {
        "file" => {
            let file_path = parsed
                .to_file_path()
                .map_err(|()| CoreError::InvalidInput("无效的 file:// 仓库路径".into()))?;
            if !file_path.is_dir() {
                return Err(CoreError::NotFound(format!(
                    "本地模型仓库目录不存在: {}",
                    file_path.display()
                )));
            }
            Ok(clean_os_path(&fs::canonicalize(file_path)?))
        }
        "https" => {
            if parsed.host_str().is_none() {
                return Err(CoreError::InvalidInput("仓库 URL 缺少主机名".into()));
            }
            Ok(parsed.into())
        }
        "http" => {
            let Some(host) = parsed.host_str() else {
                return Err(CoreError::InvalidInput("仓库 URL 缺少主机名".into()));
            };
            let is_loopback = matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]");
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
    #[serde(default)]
    host: Option<String>,
}

struct RunningRunner {
    child: Child,
    pid: Option<u32>,
    endpoint: String,
    port: u16,
    runner_token: String,
    stderr_tail: Arc<Mutex<String>>,
}

#[derive(Default)]
struct CachedStatusState {
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

pub struct LocalFilterManager {
    paths: AppPaths,
    events: Arc<dyn EventSink>,
    op_lock: tokio::sync::Mutex<Option<RunningRunner>>,
    state: Mutex<CachedStatusState>,
    http_client: reqwest::Client,
}

impl LocalFilterManager {
    pub fn new(paths: AppPaths, events: Arc<dyn EventSink>) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            paths,
            events,
            op_lock: tokio::sync::Mutex::new(None),
            state: Mutex::new(CachedStatusState::default()),
            http_client,
        }
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
        if self.paths.filter_repo_dir.join(".git").exists() {
            return read_commit_info_sync(&self.paths.filter_repo_dir, "HEAD").map(|c| c.sha);
        }
        None
    }

    pub fn active_endpoint(&self) -> Option<String> {
        self.state.lock().unwrap().service_endpoint.clone()
    }

    pub async fn status(&self) -> LocalFilterDeploymentStatus {
        {
            let mut runner_guard = self.op_lock.lock().await;
            self.reap_exited_runner(&mut runner_guard);
        }
        let paths = self.paths.clone();
        let (env, local_git) = tokio::task::spawn_blocking(move || {
            let env = probe_environment();
            let git = if paths.filter_repo_dir.join(".git").exists() {
                Some(inspect_local_repo_sync(&paths.filter_repo_dir))
            } else {
                None
            };
            (env, git)
        })
        .await
        .unwrap_or_default();

        let mut st = self.state.lock().unwrap();
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
        } else {
            st.current_commit = None;
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
        let repo_dir = self.paths.filter_repo_dir.clone();

        let (env, current, remote, recent, update_available) =
            tokio::task::spawn_blocking(move || -> Result<_, CoreError> {
                let env = probe_environment();
                if !env.git_available {
                    return Err(CoreError::Unsupported(
                        "未检测到 Git，请先在系统中安装 Git 后再检查模型更新".into(),
                    ));
                }

                if repo_dir.join(".git").exists() {
                    ensure_origin_url_sync(&repo_dir, &validated_url)?;
                    run_git_sync(&repo_dir, &["fetch", "--all", "--tags", "--prune"])?;
                    let _ = run_git_sync(&repo_dir, &["remote", "set-head", "origin", "-a"]);

                    let current = read_commit_info_sync(&repo_dir, "HEAD");
                    let remote_ref = resolve_remote_default_ref_sync(&repo_dir)
                        .unwrap_or_else(|| "HEAD".to_string());
                    let remote = read_commit_info_sync(&repo_dir, &remote_ref);
                    let recent = read_recent_commits_sync(&repo_dir, &remote_ref, 25);
                    let update_available = match (&current, &remote) {
                        (Some(c), Some(r)) => c.sha != r.sha,
                        (None, Some(_)) => true,
                        _ => false,
                    };
                    Ok((env, current, remote, recent, update_available))
                } else {
                    let remote = ls_remote_head_sync(&validated_url)?;
                    let update_available = remote.is_some();
                    Ok((env, None, remote, Vec::new(), update_available))
                }
            })
            .await
            .map_err(|e| CoreError::Io(std::io::Error::other(e)))??;

        let status = {
            let mut st = self.state.lock().unwrap();
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
            match request
                .target_commit
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(raw_commit) => Some(validate_target_commit(raw_commit)?),
                None => None,
            }
        };

        // Stop any running Python process before touching git checkout or venv files
        Self::stop_runner_inner(&mut runner_guard).await;
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
            match self.start_runner_locked(&mut runner_guard, params).await {
                Ok(()) => {}
                Err(err) => {
                    self.emit_stage(LocalFilterServiceState::Error, None, Some(err.to_string()));
                    return Err(err);
                }
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

        let paths = self.paths.clone();
        let url_owned = validated_url.to_string();
        let commit_owned = target_commit.map(str::to_owned);
        let (cur_commit, rem_commit, recent_commits) =
            tokio::task::spawn_blocking(move || -> Result<_, CoreError> {
                fs::create_dir_all(&paths.filters_dir)?;
                let repo_dir = &paths.filter_repo_dir;

                if !repo_dir.join(".git").exists() {
                    if repo_dir.exists() {
                        fs::remove_dir_all(repo_dir)?;
                    }
                    let repo_str = clean_os_path(repo_dir);
                    run_git_raw_sync(
                        &paths.filters_dir,
                        &["clone", "--no-checkout", "--", &url_owned, &repo_str],
                    )?;
                } else {
                    ensure_origin_url_sync(repo_dir, &url_owned)?;
                    run_git_sync(repo_dir, &["fetch", "--all", "--tags", "--prune"])?;
                }
                let _ = run_git_sync(repo_dir, &["remote", "set-head", "origin", "-a"]);

                if track_latest || commit_owned.is_none() {
                    let remote_ref =
                        resolve_remote_default_ref_sync(repo_dir).ok_or_else(|| {
                            CoreError::InvalidInput("无法解析模型仓库默认远端分支".into())
                        })?;
                    let sha = rev_parse_sync(repo_dir, &remote_ref)?;
                    run_git_sync(repo_dir, &["checkout", "--force", "--detach", &sha])?;
                } else if let Some(ref target) = commit_owned {
                    let verify_spec = format!("{target}^{{commit}}");
                    if run_git_sync(repo_dir, &["cat-file", "-e", &verify_spec]).is_err() {
                        run_git_sync(repo_dir, &["fetch", "origin", target])?;
                    }
                    run_git_sync(repo_dir, &["checkout", "--force", "--detach", target])?;
                }

                let deploy_dir = repo_dir.join("deploy");
                for req_file in REQUIRED_DEPLOY_FILES {
                    if !deploy_dir.join(req_file).is_file() {
                        return Err(CoreError::InvalidInput(format!(
                            "模型仓库缺少必要的部署文件: deploy/{req_file}"
                        )));
                    }
                }

                let (cur, rem, recent) = inspect_local_repo_sync(repo_dir);
                Ok((cur, rem, recent))
            })
            .await
            .map_err(|e| CoreError::Io(std::io::Error::other(e)))??;

        self.emit_stage(
            LocalFilterServiceState::Deploying,
            Some("正在配置 Python 虚拟环境与推理依赖...".into()),
            None,
        );

        let paths = self.paths.clone();
        tokio::task::spawn_blocking(move || -> Result<(), CoreError> {
            let venv_py = venv_python_path(&paths.filter_venv_dir);
            if !venv_py.is_file() {
                let mut cmd = std::process::Command::new(&sys_python);
                cmd.args(["-m", "venv", &clean_os_path(&paths.filter_venv_dir)])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                configure_std_cmd(&mut cmd);
                let out = cmd.output()?;
                if !out.status.success() {
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    return Err(CoreError::Unsupported(format!(
                        "创建 Python 虚拟环境失败: {stderr}"
                    )));
                }
            }

            let venv_py = venv_python_path(&paths.filter_venv_dir);
            if !venv_py.is_file() {
                return Err(CoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Python 虚拟环境创建后未找到解释器",
                )));
            }

            let req_path = paths
                .filter_repo_dir
                .join("deploy")
                .join("requirements.txt");
            let req_content = fs::read_to_string(&req_path)?;
            let has_packages = req_content
                .lines()
                .map(str::trim)
                .any(|line| !line.is_empty() && !line.starts_with('#'));

            if has_packages {
                let mut pip_cmd = std::process::Command::new(&venv_py);
                pip_cmd
                    .args([
                        "-m",
                        "pip",
                        "install",
                        "--disable-pip-version-check",
                        "-r",
                        &clean_os_path(&req_path),
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                configure_std_cmd(&mut pip_cmd);
                let pip_out = pip_cmd.output()?;
                if !pip_out.status.success() && !venv_deps_importable(&venv_py) {
                    let stderr = String::from_utf8_lossy(&pip_out.stderr).trim().to_string();
                    return Err(CoreError::Network(format!(
                        "安装模型 Python 依赖失败: {stderr}"
                    )));
                }
            }

            fs::write(&paths.filter_runner_path, FILTER_RUNNER_PY)?;
            Ok(())
        })
        .await
        .map_err(|e| CoreError::Io(std::io::Error::other(e)))??;

        Ok((env, cur_commit, rem_commit, recent_commits))
    }

    pub async fn start_service(
        &self,
        params: &FilterModelParams,
    ) -> Result<LocalFilterDeploymentStatus, CoreError> {
        let mut runner_guard = self.op_lock.lock().await;
        self.reap_exited_runner(&mut runner_guard);
        if runner_guard.is_none() {
            if let Err(err) = self.start_runner_locked(&mut runner_guard, params).await {
                self.emit_stage(LocalFilterServiceState::Error, None, Some(err.to_string()));
                return Err(err);
            }
        }
        drop(runner_guard);
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
        let mut runner_guard = self.op_lock.lock().await;
        self.reap_exited_runner(&mut runner_guard);
        if let Some(ref running) = *runner_guard {
            return Ok(running.endpoint.clone());
        }
        self.start_runner_locked(&mut runner_guard, params).await?;
        let endpoint = runner_guard
            .as_ref()
            .map(|r| r.endpoint.clone())
            .ok_or_else(|| CoreError::Network("本地过滤服务启动后无可用地址".into()))?;
        let st_copy = {
            let st = self.state.lock().unwrap();
            self.snapshot_from_state(FilterEnvReport::default(), &st)
        };
        drop(runner_guard);
        self.events
            .emit(CoreEvent::LocalFilterStatusChanged { status: st_copy });
        Ok(endpoint)
    }

    pub async fn stop_service(&self) -> Result<LocalFilterDeploymentStatus, CoreError> {
        let mut runner_guard = self.op_lock.lock().await;
        Self::stop_runner_inner(&mut runner_guard).await;
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

    pub async fn reconfigure_params(&self, params: &FilterModelParams) -> Result<(), CoreError> {
        let info = self.state.lock().unwrap().active_runner_info.clone();
        let Some((port, token)) = info else {
            return Ok(());
        };
        let url = format!("http://127.0.0.1:{port}/configure");
        let resp = self
            .http_client
            .post(&url)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
            .json(&params.sanitized())
            .send()
            .await
            .map_err(|e| CoreError::Network(format!("同步本地过滤运行参数失败: {e}")))?;
        if !resp.status().is_success() {
            return Err(CoreError::Network(format!(
                "同步本地过滤运行参数返回 HTTP {}",
                resp.status()
            )));
        }
        Ok(())
    }

    pub fn reconfigure_params_sync(&self, params: &FilterModelParams) -> Result<(), CoreError> {
        let info = self.state.lock().unwrap().active_runner_info.clone();
        let Some((port, token)) = info else {
            return Ok(());
        };
        let body = serde_json::to_vec(&params.sanitized())
            .map_err(|e| CoreError::Config(e.to_string()))?;
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
            .map_err(|e| CoreError::Network(format!("连接本地过滤服务失败: {e}")))?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));

        let req_head = format!(
            "POST /configure HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(req_head.as_bytes())?;
        stream.write_all(&body)?;
        stream.flush()?;

        let mut resp_buf = [0u8; 512];
        let n = stream.read(&mut resp_buf)?;
        let head = String::from_utf8_lossy(&resp_buf[..n]);
        if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
            return Err(CoreError::Network(format!(
                "更新本地过滤运行参数失败: {}",
                head.lines().next().unwrap_or("unknown")
            )));
        }
        Ok(())
    }

    pub async fn predict_local(
        &self,
        title: &str,
        description: Option<&str>,
        tags: &[String],
        images_base64: &[String],
    ) -> Result<FilterProbabilities, CoreError> {
        let endpoint = self
            .active_endpoint()
            .ok_or_else(|| CoreError::InvalidInput("本地过滤服务尚未运行".into()))?;
        let url = format!("{}/predict", endpoint.trim_end_matches('/'));
        let payload = serde_json::json!({
            "title": title,
            "description": description.unwrap_or(""),
            "tags": tags,
            "imagesBase64": images_base64,
        });
        let resp = self
            .http_client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| CoreError::Network(format!("调用本地过滤模型推理失败: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().await.unwrap_or_default();
            return Err(CoreError::Network(format!(
                "本地过滤模型推理返回 HTTP {status}: {detail}"
            )));
        }
        resp.json::<FilterProbabilities>()
            .await
            .map_err(|e| CoreError::Network(format!("解析本地过滤模型输出失败: {e}")))
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

        let params_json = serde_json::to_string(&params.sanitized())
            .map_err(|e| CoreError::Config(e.to_string()))?;

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
        ])
        .env("L4A_RUNNER_TOKEN", &runner_token)
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
        configure_tokio_cmd(&mut cmd);

        let mut child = cmd.spawn()?;
        let pid = child.id();
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
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut guard = stderr_tail_writer.lock().unwrap();
                if guard.len() > 2048 {
                    guard.clear();
                }
                if !guard.is_empty() {
                    guard.push('\n');
                }
                guard.push_str(&line);
            }
        });

        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<ReadyBanner, String>>();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            let mut tx_opt = Some(ready_tx);
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(json_part) = line.trim().strip_prefix("L4A_FILTER_READY ") {
                    if let Some(tx) = tx_opt.take() {
                        let parsed = serde_json::from_str::<ReadyBanner>(json_part)
                            .map_err(|e| format!("解析守护进程就绪信标失败: {e}"));
                        let _ = tx.send(parsed);
                    }
                }
            }
            if let Some(tx) = tx_opt.take() {
                let _ = tx.send(Err("本地过滤守护进程在就绪前提前退出".into()));
            }
        });

        let banner_res = tokio::time::timeout(Duration::from_secs(30), ready_rx).await;
        let banner = match banner_res {
            Ok(Ok(Ok(b))) => b,
            Ok(Ok(Err(msg))) => {
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
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(CoreError::Network(
                    "等待本地过滤守护进程就绪超时 (30s)".into(),
                ));
            }
        };

        let host = banner
            .host
            .as_deref()
            .filter(|h| !h.is_empty())
            .unwrap_or("127.0.0.1");
        let endpoint = format!("http://{host}:{}", banner.port);

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
                    Some((status, tail))
                }
                Ok(None) => None,
                Err(_) => Some((std::process::ExitStatus::default(), String::new())),
            }
        } else {
            None
        };

        if let Some((exit_status, tail)) = exited {
            *runner_guard = None;
            let mut st = self.state.lock().unwrap();
            st.service_state = LocalFilterServiceState::Error;
            st.service_endpoint = None;
            st.running_pid = None;
            st.active_runner_info = None;
            st.stage_message = None;
            st.last_error = Some(if tail.is_empty() {
                format!("本地过滤守护进程已退出 ({exit_status})")
            } else {
                format!("本地过滤守护进程异常退出 ({exit_status}): {tail}")
            });
        }
    }

    async fn stop_runner_inner(runner_guard: &mut Option<RunningRunner>) {
        if let Some(mut running) = runner_guard.take() {
            let _ = running.child.kill().await;
            let _ = running.child.wait().await;
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
            self.snapshot_from_state(FilterEnvReport::default(), &st)
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

fn venv_deps_importable(venv_py: &Path) -> bool {
    let mut cmd = std::process::Command::new(venv_py);
    cmd.args(["-c", "import onnxruntime, numpy, PIL"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_std_cmd(&mut cmd);
    cmd.status().is_ok_and(|s| s.success())
}

fn run_git_raw_sync(cwd: &Path, args: &[&str]) -> Result<String, CoreError> {
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_std_cmd(&mut cmd);

    let out = cmd.output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(CoreError::Network(format!("Git 命令执行失败: {stderr}")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run_git_sync(repo_dir: &Path, args: &[&str]) -> Result<String, CoreError> {
    run_git_raw_sync(repo_dir, args)
}

fn ensure_origin_url_sync(repo_dir: &Path, url: &str) -> Result<(), CoreError> {
    if run_git_sync(repo_dir, &["remote", "set-url", "origin", "--", url]).is_err() {
        run_git_sync(repo_dir, &["remote", "add", "origin", "--", url])?;
    }
    Ok(())
}

fn rev_parse_sync(repo_dir: &Path, rev: &str) -> Result<String, CoreError> {
    let out = run_git_sync(repo_dir, &["rev-parse", "--verify", rev])?;
    let sha = out.trim().to_string();
    if sha.is_empty() {
        return Err(CoreError::InvalidInput(format!("无法解析 Git 引用: {rev}")));
    }
    Ok(sha)
}

fn resolve_remote_default_ref_sync(repo_dir: &Path) -> Option<String> {
    if let Ok(sym) = run_git_sync(
        repo_dir,
        &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"],
    ) {
        let trimmed = sym.trim();
        if !trimmed.is_empty() && rev_parse_sync(repo_dir, trimmed).is_ok() {
            return Some(trimmed.to_string());
        }
    }
    for candidate in ["origin/main", "origin/master", "FETCH_HEAD", "HEAD"] {
        if rev_parse_sync(repo_dir, candidate).is_ok() {
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

fn read_commit_info_sync(repo_dir: &Path, rev: &str) -> Option<FilterCommitInfo> {
    let out = run_git_sync(
        repo_dir,
        &["log", rev, "-n", "1", "--format=%H%x1f%h%x1f%s%x1f%cI"],
    )
    .ok()?;
    out.lines().find_map(parse_commit_line)
}

fn read_recent_commits_sync(repo_dir: &Path, rev: &str, limit: usize) -> Vec<FilterCommitInfo> {
    let limit_str = limit.to_string();
    let Ok(out) = run_git_sync(
        repo_dir,
        &[
            "log",
            rev,
            "-n",
            &limit_str,
            "--format=%H%x1f%h%x1f%s%x1f%cI",
        ],
    ) else {
        return Vec::new();
    };
    out.lines().filter_map(parse_commit_line).collect()
}

fn inspect_local_repo_sync(
    repo_dir: &Path,
) -> (
    Option<FilterCommitInfo>,
    Option<FilterCommitInfo>,
    Vec<FilterCommitInfo>,
) {
    let cur = read_commit_info_sync(repo_dir, "HEAD");
    let rem_ref = resolve_remote_default_ref_sync(repo_dir);
    let rem = rem_ref
        .as_deref()
        .and_then(|r| read_commit_info_sync(repo_dir, r));
    let recent = rem_ref
        .as_deref()
        .map(|r| read_recent_commits_sync(repo_dir, r, 25))
        .unwrap_or_default();
    (cur, rem, recent)
}

fn ls_remote_head_sync(repo_url: &str) -> Result<Option<FilterCommitInfo>, CoreError> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(["ls-remote", "--", repo_url, "HEAD"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_std_cmd(&mut cmd);
    let out = cmd.output()?;
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
