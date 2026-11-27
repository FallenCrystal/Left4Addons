use super::FILTER_RUNNER_PY;
use crate::config::AppPaths;
use crate::engine::EventSink;
use crate::error::CoreError;
use l4a_logging::http::AuditedSend;
use l4a_logging::process::CommandAudit;
use l4a_protocol::{
    merge_managed_predictions, CoreEvent, DeployLocalFilterRequest, FilterCommitInfo,
    FilterEnvReport, FilterModelParams, FilterProbabilities, LocalFilterDeploymentStatus,
    LocalFilterServiceState, ManagedFilterPrediction, RawFilterPredictRequest,
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

pub(crate) const MAX_RAW_PREDICT_IMAGE_BATCH_BYTES: usize = 30 * 1024 * 1024;

const REQUIRED_DEPLOY_FILES: &[&str] = &[
    "l4filter.py",
    "vision_encoder.onnx",
    "fusion_head.onnx",
    "text_vocab.json",
    "requirements.txt",
];
const WEBGPU_REQUIREMENTS: &str = "requirements-webgpu.txt";
const WEBGPU_PROVIDER: &str = "WebGpuExecutionProvider";
const VISION_PROVIDER_ENV: &str = "LEFT4FILTERS_VISION_PROVIDER";
const CPU_PYTHON_MIN_MINOR: u8 = 9;
const WEBGPU_PYTHON_MIN_MINOR: u8 = 11;
const PYTHON_VERSION_CHECK: &str =
    "import sys\nif sys.version_info < (3, int(sys.argv[1])):\n    sys.exit(2)\n";

fn validate_deploy_assets(deploy_dir: &Path, web_gpu_enabled: bool) -> Result<(), CoreError> {
    for name in REQUIRED_DEPLOY_FILES {
        if !deploy_dir.join(name).is_file() {
            return Err(CoreError::InvalidInput(format!(
                "模型仓库缺少必要的部署文件: deploy/{name}"
            )));
        }
    }
    if web_gpu_enabled && !deploy_dir.join(WEBGPU_REQUIREMENTS).is_file() {
        return Err(CoreError::Unsupported(format!(
            "所选 Left4Filters 版本缺少 deploy/{WEBGPU_REQUIREMENTS}。请更新模型版本或关闭 WebGPU 选项。"
        )));
    }

    let config_path = deploy_dir.join("model_config.json");
    let has_corpse_head = deploy_dir.join("corpse_head.onnx").is_file();
    if config_path.is_file() {
        let config: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&fs::read(&config_path)?)
                .map_err(|err| CoreError::InvalidInput(format!("模型部署配置无效: {err}")))?;
        if config
            .get("corpse_detection")
            .is_some_and(|value| !value.is_null())
            && !has_corpse_head
        {
            return Err(CoreError::InvalidInput(
                "模型仓库缺少必要的部署文件: deploy/corpse_head.onnx".into(),
            ));
        }
    } else if has_corpse_head {
        return Err(CoreError::InvalidInput(
            "尸体识别模型缺少部署配置: deploy/model_config.json".into(),
        ));
    }
    Ok(())
}

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

fn configure_runner_vision_provider(
    cmd: &mut tokio::process::Command,
    web_gpu_enabled: bool,
    inherited: Option<&str>,
) {
    if web_gpu_enabled {
        cmd.env(VISION_PROVIDER_ENV, "webgpu");
    } else if inherited.is_some_and(|provider| provider.trim().eq_ignore_ascii_case("webgpu")) {
        cmd.env_remove(VISION_PROVIDER_ENV);
    }
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

fn make_absolute_preserving_symlinks(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
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

pub(crate) fn venv_python_path(venv_dir: &Path) -> PathBuf {
    let abs_venv = make_absolute_preserving_symlinks(venv_dir);
    #[cfg(windows)]
    {
        abs_venv.join("Scripts").join("python.exe")
    }
    #[cfg(not(windows))]
    {
        let py = abs_venv.join("bin").join("python");
        if is_executable_file(&py) {
            py
        } else {
            let py3 = abs_venv.join("bin").join("python3");
            if is_executable_file(&py3) {
                py3
            } else {
                py
            }
        }
    }
}

fn which_executable_in_paths(program: &str, path_var: &OsStr) -> Option<PathBuf> {
    let prog_path = Path::new(program);
    if prog_path.components().count() > 1 {
        let abs = make_absolute_preserving_symlinks(prog_path);
        return is_executable_file(&abs).then_some(abs);
    }

    #[cfg(windows)]
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
        .split(';')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_ascii_lowercase())
        .collect();

    for dir in std::env::split_paths(path_var) {
        let direct = dir.join(program);
        if is_executable_file(&direct) {
            return Some(make_absolute_preserving_symlinks(&direct));
        }
        #[cfg(windows)]
        {
            for ext in &exts {
                let candidate = dir.join(format!("{program}{ext}"));
                if is_executable_file(&candidate) {
                    return Some(make_absolute_preserving_symlinks(&candidate));
                }
            }
        }
    }
    None
}

fn which_executable(program: &str) -> Option<String> {
    let path_var = std::env::var_os("PATH")?;
    which_executable_in_paths(program, &path_var).map(|p| clean_os_path(&p))
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

pub(crate) fn probe_python_candidates(
    candidates: &[(&str, &[&str])],
    path_override: Option<&OsStr>,
    min_python_minor: u8,
) -> Option<(String, String, bool)> {
    let probe_script = format!(
        "{PYTHON_VERSION_CHECK}print(sys.executable)\nprint(sys.version.split()[0])\ntry:\n    import venv, ensurepip\n    print('1')\nexcept Exception:\n    print('0')\n"
    );
    let min_python_minor = min_python_minor.to_string();
    let effective_path = path_override
        .map(OsStr::to_os_string)
        .or_else(|| std::env::var_os("PATH"));

    let mut fallback_no_venv: Option<(String, String)> = None;

    for &(prog, prefix_args) in candidates {
        let resolved_bin = if prefix_args.is_empty() {
            effective_path
                .as_deref()
                .and_then(|pv| which_executable_in_paths(prog, pv))
        } else {
            None
        };

        let mut cmd = match resolved_bin {
            Some(ref p) => std::process::Command::new(p),
            None => std::process::Command::new(prog),
        };
        if let Some(ref pv) = effective_path {
            cmd.env("PATH", pv);
        }
        cmd.args(prefix_args)
            .args(["-c", &probe_script, &min_python_minor])
            .env("PYTHONIOENCODING", "utf-8")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        configure_std_cmd(&mut cmd);

        let audit = CommandAudit::start_quiet("filter.probe.python", &cmd);
        let result = cmd.output();
        audit.finish_output(&result);
        let Ok(out) = result else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        let Some(reported_exe) = lines.next() else {
            continue;
        };
        let Some(ver) = lines.next() else {
            continue;
        };
        let has_venv = lines.next() == Some("1");

        // Preserve any virtualenv symlink path rather than replacing it with a canonicalized target.
        let preserved_exe = if let Some(bin_path) = resolved_bin {
            clean_os_path(&bin_path)
        } else {
            let reported_path = make_absolute_preserving_symlinks(Path::new(reported_exe));
            if is_executable_file(&reported_path) {
                clean_os_path(&reported_path)
            } else {
                reported_exe.to_string()
            }
        };

        if has_venv {
            return Some((preserved_exe, ver.to_string(), true));
        }
        if fallback_no_venv.is_none() {
            fallback_no_venv = Some((preserved_exe, ver.to_string()));
        }
    }

    fallback_no_venv.map(|(exe, ver)| (exe, ver, false))
}

fn probe_deployment_python(
    web_gpu_enabled: bool,
    path_override: Option<&OsStr>,
) -> Option<(String, String, bool)> {
    #[cfg(windows)]
    let candidates: &[(&str, &[&str])] = &[("python3", &[]), ("python", &[]), ("py", &["-3"])];
    #[cfg(not(windows))]
    let candidates: &[(&str, &[&str])] = &[("python3", &[]), ("python", &[])];

    let min_python_minor = if web_gpu_enabled {
        WEBGPU_PYTHON_MIN_MINOR
    } else {
        CPU_PYTHON_MIN_MINOR
    };
    let selected = probe_python_candidates(candidates, path_override, min_python_minor);
    if !web_gpu_enabled || selected.as_ref().is_some_and(|(_, _, has_venv)| *has_venv) {
        return selected;
    }

    let Some(path) = path_override
        .map(OsStr::to_os_string)
        .or_else(|| std::env::var_os("PATH"))
    else {
        return selected;
    };
    let fallback_paths = webgpu_python_fallbacks(&path);
    let fallback_candidates: Vec<(&str, &[&str])> = fallback_paths
        .iter()
        .map(|path| (path.as_str(), &[][..]))
        .collect();
    match probe_python_candidates(&fallback_candidates, Some(&path), min_python_minor) {
        Some(candidate) if candidate.2 || selected.is_none() => Some(candidate),
        _ => selected,
    }
}

fn webgpu_python_fallbacks(path: &OsStr) -> Vec<String> {
    let mut candidates = Vec::new();
    for dir in std::env::split_paths(path) {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut interpreters: Vec<_> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                #[cfg(windows)]
                let name = {
                    if !path.extension()?.eq_ignore_ascii_case("exe") {
                        return None;
                    }
                    path.file_stem()?.to_str()?.to_ascii_lowercase()
                };
                #[cfg(windows)]
                let name = name.as_str();
                #[cfg(not(windows))]
                let name = path.file_name()?.to_str()?;
                let minor = match name {
                    "python3" | "python" => 0,
                    _ => {
                        let minor = name.strip_prefix("python3.")?.parse::<u8>().ok()?;
                        if minor < WEBGPU_PYTHON_MIN_MINOR {
                            return None;
                        }
                        minor
                    }
                };
                is_executable_file(&path).then_some((std::cmp::Reverse(minor), path))
            })
            .collect();
        interpreters.sort_unstable();
        candidates.extend(
            interpreters
                .into_iter()
                .map(|(_, path)| clean_os_path(&make_absolute_preserving_symlinks(&path))),
        );
    }
    candidates
}

pub fn probe_environment() -> FilterEnvReport {
    probe_environment_for_backend(false)
}

fn probe_environment_for_backend(web_gpu_enabled: bool) -> FilterEnvReport {
    let mut report = FilterEnvReport::default();

    let mut git_cmd = std::process::Command::new("git");
    git_cmd
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    configure_std_cmd(&mut git_cmd);

    let audit = CommandAudit::start_quiet("filter.probe.git", &git_cmd);
    let git_result = git_cmd.output();
    audit.finish_output(&git_result);
    if let Ok(out) = git_result {
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

    if let Some((exe_path, ver, has_venv)) = probe_deployment_python(web_gpu_enabled, None) {
        report.python_available = true;
        report.python_path = Some(exe_path);
        report.python_version = Some(ver);
        report.venv_available = has_venv;
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
    vision_provider: Option<String>,
}

struct RunningRunner {
    child: Child,
    stdin: Option<ChildStdin>,
    pid: Option<u32>,
    endpoint: String,
    port: u16,
    runner_token: String,
    stderr_tail: Arc<Mutex<String>>,
    synced_params: Arc<Mutex<Option<FilterModelParams>>>,
}

struct PreparedDeployment {
    env: FilterEnvReport,
    current_commit: Option<FilterCommitInfo>,
    remote_latest_commit: Option<FilterCommitInfo>,
    recent_commits: Vec<FilterCommitInfo>,
    runner: Option<RunningRunner>,
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
    active_runner_synced: Option<Arc<Mutex<Option<FilterModelParams>>>>,
}

enum PredictLocalError {
    Transport(CoreError),
    HttpStatus(CoreError),
}

pub struct LocalFilterDeployOutcome {
    pub status: LocalFilterDeploymentStatus,
    pub validated_repo_url: String,
    pub start_error: Option<CoreError>,
}

pub struct LocalFilterManager {
    paths: AppPaths,
    events: Arc<dyn EventSink>,
    /// Lock ordering: always acquire `op_lock` before `predict_execution.write()`.
    /// Never acquire `op_lock` while holding `predict_execution.read()`.
    op_lock: tokio::sync::Mutex<Option<RunningRunner>>,
    predict_execution: tokio::sync::RwLock<()>,
    state: Mutex<CachedStatusState>,
    desired_params: RwLock<FilterModelParams>,
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
            predict_execution: tokio::sync::RwLock::new(()),
            state: Mutex::new(CachedStatusState::default()),
            desired_params: RwLock::new(FilterModelParams::default()),
            last_auto_restart_at: Mutex::new(None),
            shutdown_token: CancellationToken::new(),
            http_client,
        }
    }

    pub fn set_desired_params(&self, params: &FilterModelParams) {
        let sanitized = params.sanitized();
        *self.desired_params.write().unwrap() = sanitized.clone();
        if let Some(ref synced_arc) = self.state.lock().unwrap().active_runner_synced {
            let mut synced = synced_arc.lock().unwrap();
            if synced.as_ref() != Some(&sanitized) {
                *synced = None;
            }
        }
    }

    pub fn is_deployed(&self) -> bool {
        self.is_backend_deployed(self.web_gpu_enabled())
    }

    fn is_backend_deployed(&self, web_gpu_enabled: bool) -> bool {
        let repo_dir = &self.paths.filter_repo_dir;
        if !repo_dir.join(".git").exists() {
            return false;
        }
        let deploy_dir = repo_dir.join("deploy");
        validate_deploy_assets(&deploy_dir, web_gpu_enabled).is_ok()
            && is_executable_file(&venv_python_path(self.venv_dir(web_gpu_enabled)))
            && self.paths.filter_runner_path.is_file()
    }

    fn venv_dir(&self, web_gpu_enabled: bool) -> &Path {
        if web_gpu_enabled {
            &self.paths.filter_webgpu_venv_dir
        } else {
            &self.paths.filter_venv_dir
        }
    }

    fn web_gpu_marker(&self) -> PathBuf {
        self.paths.filters_dir.join("webgpu-enabled")
    }

    fn web_gpu_enabled(&self) -> bool {
        self.web_gpu_marker().is_file()
    }

    fn set_web_gpu_enabled(&self, enabled: bool) -> Result<(), CoreError> {
        let marker = self.web_gpu_marker();
        if enabled {
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(marker)?;
        } else if let Err(err) = fs::remove_file(marker) {
            if err.kind() != std::io::ErrorKind::NotFound {
                return Err(err.into());
            }
        }
        Ok(())
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
        let web_gpu_enabled = self.web_gpu_enabled();
        let probe = move || probe_environment_for_backend(web_gpu_enabled);

        if op_busy {
            let cached_env = self.state.lock().unwrap().last_env.clone();
            let env = match cached_env {
                Some(e) => e,
                None => tokio::task::spawn_blocking(probe).await.unwrap_or_default(),
            };
            let mut st = self.state.lock().unwrap();
            if st.last_env.is_none() {
                st.last_env = Some(env.clone());
            }
            return self.snapshot_from_state(env, &st);
        }

        let env = tokio::task::spawn_blocking(probe).await.unwrap_or_default();

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

        let web_gpu_enabled = self.web_gpu_enabled();
        let env =
            tokio::task::spawn_blocking(move || probe_environment_for_backend(web_gpu_enabled))
                .await
                .map_err(|e| CoreError::Io(std::io::Error::other(e)))?;
        if !env.git_available {
            return Err(CoreError::Unsupported(
                "未检测到 Git，请先在系统中安装 Git 后再检查模型更新".into(),
            ));
        }

        let has_valid_repo = repo_dir.join(".git").exists()
            && is_valid_git_repo_async(&self.shutdown_token, repo_dir).await?;
        let (current, remote, recent, update_available) = if has_valid_repo {
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
        https_only: bool,
    ) -> Result<LocalFilterDeployOutcome, CoreError> {
        let mut runner_guard = self.op_lock.lock().await;
        let web_gpu_enabled = request
            .install_web_gpu
            .unwrap_or_else(|| self.web_gpu_enabled());

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

        *self.last_auto_restart_at.lock().unwrap() = None;
        let deploy_started = Instant::now();
        tracing::info!(
            repo_url = %l4a_logging::redact::redact_url_str(&validated_url),
            target_commit = target_commit.as_deref().unwrap_or("latest"),
            web_gpu_enabled,
            start_after_deploy = request.start_after_deploy,
            "Starting local filter model deployment"
        );

        self.reap_exited_runner(&mut runner_guard);
        let restart_previous_runner =
            web_gpu_enabled && request.start_after_deploy && runner_guard.is_some();
        // Stop any running Python process before touching git checkout or venv files
        {
            let _exec = self.predict_execution.write().await;
            self.stop_runner_inner(&mut runner_guard).await;
        }
        self.emit_stage(
            LocalFilterServiceState::Deploying,
            Some("正在检查本地 Git 与 Python 环境...".into()),
            None,
        );

        let deploy_res = self
            .run_deploy_steps(
                &validated_url,
                target_commit.as_deref(),
                web_gpu_enabled,
                request.start_after_deploy,
            )
            .await;

        let PreparedDeployment {
            env,
            current_commit,
            remote_latest_commit,
            recent_commits,
            runner,
        } = match deploy_res {
            Ok(deployment) => deployment,
            Err(mut err) => {
                if restart_previous_runner
                    && !matches!(err, CoreError::RecoveryRequired(_))
                    && !self.shutdown_token.is_cancelled()
                {
                    let params = self.desired_params.read().unwrap().clone();
                    match self.start_runner_locked(&mut runner_guard, &params).await {
                        Ok(()) => {
                            self.emit_stage(
                                LocalFilterServiceState::Running,
                                None,
                                Some(err.to_string()),
                            );
                            return Err(err);
                        }
                        Err(restart_err) => {
                            err = CoreError::RecoveryRequired(format!(
                                "部署失败（{err}），重启原推理服务失败（{restart_err}）"
                            ));
                        }
                    }
                }
                tracing::warn!(
                    elapsed_ms = deploy_started.elapsed().as_millis() as u64,
                    error = %err,
                    "Local filter model deployment failed"
                );
                self.emit_stage(LocalFilterServiceState::Error, None, Some(err.to_string()));
                return Err(err);
            }
        };
        tracing::info!(
            elapsed_ms = deploy_started.elapsed().as_millis() as u64,
            commit = current_commit
                .as_ref()
                .map(|c| c.sha.as_str())
                .unwrap_or("-"),
            remote_latest = remote_latest_commit
                .as_ref()
                .map(|c| c.sha.as_str())
                .unwrap_or("-"),
            "Local filter model deployment steps completed"
        );

        {
            let mut st = self.state.lock().unwrap();
            st.last_env = Some(env.clone());
            st.update_available = match (&current_commit, &remote_latest_commit) {
                (Some(c), Some(r)) => c.sha != r.sha,
                _ => false,
            };
            st.current_commit = current_commit;
            st.remote_latest_commit = remote_latest_commit;
            st.recent_commits = recent_commits;
            st.last_error = None;
        }

        let start_error = if let Some(runner) = runner {
            self.set_running_runner(&mut runner_guard, runner);
            None
        } else if request.start_after_deploy {
            let latest_params = self.desired_params.read().unwrap().clone();
            self.start_runner_locked(&mut runner_guard, &latest_params)
                .await
                .err()
        } else {
            let mut st = self.state.lock().unwrap();
            st.service_state = LocalFilterServiceState::Stopped;
            st.service_endpoint = None;
            st.running_pid = None;
            st.active_runner_synced = None;
            st.stage_message = None;
            st.last_error = None;
            None
        };

        let status = {
            let st = self.state.lock().unwrap();
            self.snapshot_from_state(env, &st)
        };
        if start_error.is_none() {
            self.events.emit(CoreEvent::LocalFilterStatusChanged {
                status: status.clone(),
            });
        }
        Ok(LocalFilterDeployOutcome {
            status,
            validated_repo_url: validated_url,
            start_error,
        })
    }

    async fn run_deploy_steps(
        &self,
        validated_url: &str,
        target_commit: Option<&str>,
        web_gpu_enabled: bool,
        start_after_deploy: bool,
    ) -> Result<PreparedDeployment, CoreError> {
        let env =
            tokio::task::spawn_blocking(move || probe_environment_for_backend(web_gpu_enabled))
                .await
                .map_err(|e| CoreError::Io(std::io::Error::other(e)))?;

        let required_python = if web_gpu_enabled {
            "Python 3.11+ (WebGPU)"
        } else {
            "Python 3.9+"
        };

        if !env.git_available || !env.python_available || !env.venv_available {
            let mut missing = Vec::new();
            if !env.git_available {
                missing.push("Git");
            }
            if !env.python_available {
                missing.push(required_python);
            } else if !env.venv_available {
                missing.push("Python venv / ensurepip 模块 (如 Debian/Ubuntu 请安装 python3-venv)");
            }
            return Err(CoreError::Unsupported(format!(
                "缺少本地部署前置环境：{}。Left4Addons 不会自动安装系统级前置依赖，请先手动安装后重试。",
                missing.join("、")
            )));
        }

        let sys_python = env.python_path.clone().ok_or_else(|| {
            CoreError::Unsupported(format!("未找到可用的 {required_python} 可执行文件"))
        })?;

        self.emit_stage(
            LocalFilterServiceState::Deploying,
            Some("正在同步模型仓库与检出目标版本...".into()),
            None,
        );

        fs::create_dir_all(&self.paths.filters_dir)?;
        let repo_dir = &self.paths.filter_repo_dir;

        let has_valid_repo = repo_dir.join(".git").exists()
            && is_valid_git_repo_async(&self.shutdown_token, repo_dir).await?;
        if !has_valid_repo {
            if repo_dir.exists() {
                fs::remove_dir_all(repo_dir)?;
            }
            let repo_str = clean_os_path(repo_dir);
            if let Err(err) = run_git_async(
                &self.shutdown_token,
                &self.paths.filters_dir,
                &["clone", "--no-checkout", "--", validated_url, &repo_str],
                GIT_NETWORK_TIMEOUT,
            )
            .await
            {
                let _ = fs::remove_dir_all(repo_dir);
                return Err(err);
            }
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

        let prev_head = if has_valid_repo {
            rev_parse_async(&self.shutdown_token, repo_dir, "HEAD")
                .await
                .ok()
        } else {
            None
        };

        let sha = if let Some(target) = target_commit {
            resolve_target_commit_sha_async(&self.shutdown_token, repo_dir, target).await?
        } else {
            let remote_ref = resolve_remote_default_ref_async(&self.shutdown_token, repo_dir)
                .await
                .ok_or_else(|| CoreError::InvalidInput("无法解析模型仓库默认远端分支".into()))?;
            rev_parse_async(&self.shutdown_token, repo_dir, &remote_ref).await?
        };

        let post_checkout = async {
            run_git_async(
                &self.shutdown_token,
                repo_dir,
                &["checkout", "--force", "--detach", &sha],
                GIT_FAST_TIMEOUT,
            )
            .await?;
            let deploy_dir = repo_dir.join("deploy");
            validate_deploy_assets(&deploy_dir, web_gpu_enabled)?;

            let req_path = deploy_dir.join(if web_gpu_enabled {
                WEBGPU_REQUIREMENTS
            } else {
                "requirements.txt"
            });
            let req_content = fs::read_to_string(&req_path)?;
            let has_packages = req_content
                .lines()
                .map(str::trim)
                .any(|line| !line.is_empty() && !line.starts_with('#'));
            if web_gpu_enabled && !has_packages {
                return Err(CoreError::InvalidInput(format!(
                    "WebGPU 依赖清单为空: deploy/{WEBGPU_REQUIREMENTS}"
                )));
            }

            let (current_commit, remote_latest_commit, recent_commits) =
                inspect_local_repo_async(&self.shutdown_token, repo_dir).await?;

            self.emit_stage(
                LocalFilterServiceState::Deploying,
                Some(if web_gpu_enabled {
                    "正在配置 Python 虚拟环境与 WebGPU 推理依赖...".into()
                } else {
                    "正在配置 Python 虚拟环境与推理依赖...".into()
                }),
                None,
            );

            let venv_dir = self.venv_dir(web_gpu_enabled);
            let venv_py = venv_python_path(venv_dir);
            let has_valid_venv = if is_executable_file(&venv_py) {
                match venv_is_usable_async(&self.shutdown_token, &venv_py, web_gpu_enabled).await {
                    Ok(valid) => valid,
                    Err(CoreError::Io(err)) if err.kind() == std::io::ErrorKind::Interrupted => {
                        return Err(CoreError::Io(err));
                    }
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "Existing Python venv interpreter is not executable or is corrupted; recreating the venv"
                        );
                        false
                    }
                }
            } else {
                false
            };
            if !has_valid_venv {
                if venv_dir.exists() {
                    fs::remove_dir_all(venv_dir)?;
                }
                let venv_str = clean_os_path(&make_absolute_preserving_symlinks(venv_dir));
                let out = match run_cancellable_command(
                    &self.shutdown_token,
                    OsStr::new(&sys_python),
                    None,
                    &["-m", "venv", &venv_str],
                    &[("PYTHONIOENCODING", "utf-8")],
                    VENV_CREATE_TIMEOUT,
                )
                .await
                {
                    Ok(out) => out,
                    Err(err) => {
                        let _ = fs::remove_dir_all(venv_dir);
                        return Err(err);
                    }
                };
                if !out.status.success() {
                    let _ = fs::remove_dir_all(venv_dir);
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    return Err(CoreError::Unsupported(format!(
                        "创建 Python 虚拟环境失败: {stderr}"
                    )));
                }
            }

            let venv_py = venv_python_path(venv_dir);
            if !is_executable_file(&venv_py) {
                return Err(CoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Python 虚拟环境创建后未找到解释器",
                )));
            }

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
                    && (web_gpu_enabled
                        || !venv_deps_importable_async(&self.shutdown_token, &venv_py).await?)
                {
                    let stderr = String::from_utf8_lossy(&pip_out.stderr).trim().to_string();
                    let dependencies = if web_gpu_enabled { "WebGPU" } else { "模型" };
                    return Err(CoreError::Network(format!(
                        "安装{dependencies} Python 依赖失败: {stderr}"
                    )));
                }
            }

            fs::write(&self.paths.filter_runner_path, FILTER_RUNNER_PY)?;
            // Keep the existing backend selected until the requested WebGPU runner is ready.
            let runner = if web_gpu_enabled && start_after_deploy {
                let params = self.desired_params.read().unwrap().clone();
                Some(self.spawn_runner(&params, web_gpu_enabled).await?)
            } else {
                None
            };
            self.set_web_gpu_enabled(web_gpu_enabled)?;

            Ok(PreparedDeployment {
                env,
                current_commit,
                remote_latest_commit,
                recent_commits,
                runner,
            })
        }
        .await;

        if let Err(mut err) = post_checkout {
            if let Some(ref old_sha) = prev_head {
                if let Err(rollback_err) = run_git_async(
                    &self.shutdown_token,
                    repo_dir,
                    &["checkout", "--force", "--detach", old_sha],
                    GIT_FAST_TIMEOUT,
                )
                .await
                {
                    err = CoreError::RecoveryRequired(format!(
                        "部署失败（{err}），恢复原模型版本失败（{rollback_err}）"
                    ));
                }
            }
            if let Ok((cur_commit, rem_commit, recent_commits)) =
                inspect_local_repo_async(&self.shutdown_token, repo_dir).await
            {
                let mut st = self.state.lock().unwrap();
                st.update_available = match (&cur_commit, &rem_commit) {
                    (Some(c), Some(r)) => c.sha != r.sha,
                    _ => false,
                };
                st.current_commit = cur_commit;
                st.remote_latest_commit = rem_commit;
                st.recent_commits = recent_commits;
            }
            return Err(err);
        }

        post_checkout
    }

    pub async fn current_repo_head_sha(&self) -> Option<String> {
        let repo_dir = &self.paths.filter_repo_dir;
        if !repo_dir.join(".git").exists() {
            return None;
        }
        rev_parse_async(&self.shutdown_token, repo_dir, "HEAD")
            .await
            .ok()
    }

    pub async fn start_service(&self) -> Result<LocalFilterDeploymentStatus, CoreError> {
        *self.last_auto_restart_at.lock().unwrap() = None;
        let mut runner_guard = self.op_lock.lock().await;
        self.reap_exited_runner(&mut runner_guard);
        let sanitized = self.desired_params.read().unwrap().clone();
        if runner_guard.is_some() {
            self.sync_or_restart_runner_locked(&mut runner_guard, &sanitized)
                .await?;
        } else if let Err(err) = self
            .start_runner_locked(&mut runner_guard, &sanitized)
            .await
        {
            let err_msg = err.to_string();
            let should_emit = {
                let st = self.state.lock().unwrap();
                st.service_state != LocalFilterServiceState::Error
                    || st.last_error.as_deref() != Some(err_msg.as_str())
            };
            if should_emit {
                self.emit_stage(LocalFilterServiceState::Error, None, Some(err_msg));
            }
            return Err(err);
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
        let sanitized = params.sanitized();
        let mut runner_guard = self.op_lock.lock().await;
        self.reap_exited_runner(&mut runner_guard);
        if runner_guard.is_some() {
            return self
                .sync_or_restart_runner_locked(&mut runner_guard, &sanitized)
                .await;
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

        self.start_runner_locked(&mut runner_guard, &sanitized)
            .await?;
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
        {
            let _exec = self.predict_execution.write().await;
            self.stop_runner_inner(&mut runner_guard).await;
        }
        {
            let mut st = self.state.lock().unwrap();
            st.service_state = LocalFilterServiceState::Stopped;
            st.service_endpoint = None;
            st.running_pid = None;
            st.active_runner_synced = None;
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
            let _exec =
                tokio::time::timeout(Duration::from_secs(2), self.predict_execution.write()).await;
            self.stop_runner_inner(&mut runner_guard).await;
        }
        let mut st = self.state.lock().unwrap();
        st.service_state = LocalFilterServiceState::Stopped;
        st.service_endpoint = None;
        st.running_pid = None;
        st.active_runner_synced = None;
        st.stage_message = None;
    }

    pub async fn reconfigure_to_desired(&self) -> Result<(), CoreError> {
        let mut guard = self.op_lock.lock().await;
        self.reap_exited_runner(&mut guard);
        if guard.is_some() {
            let latest = self.desired_params.read().unwrap().clone();
            self.sync_or_restart_runner_locked(&mut guard, &latest)
                .await?;
        }
        Ok(())
    }

    pub async fn reconfigure_params(&self, params: &FilterModelParams) -> Result<(), CoreError> {
        let sanitized = params.sanitized();
        self.set_desired_params(&sanitized);
        self.reconfigure_to_desired().await
    }

    /// Ensures the active runner's parameters match `sanitized_params`.
    /// If `POST /configure` fails or times out, cleanly stops and restarts the runner with
    /// `sanitized_params` instead of allowing stale-parameter predictions.
    async fn sync_or_restart_runner_locked(
        &self,
        runner_guard: &mut Option<RunningRunner>,
        sanitized_params: &FilterModelParams,
    ) -> Result<String, CoreError> {
        let (endpoint, port, token, synced_arc) = {
            let Some(ref running) = *runner_guard else {
                return Err(CoreError::InvalidInput(
                    "本地过滤守护进程未运行，请先部署或启动服务".into(),
                ));
            };
            (
                running.endpoint.clone(),
                running.port,
                running.runner_token.clone(),
                running.synced_params.clone(),
            )
        };

        if synced_arc.lock().unwrap().as_ref() == Some(sanitized_params) {
            return Ok(endpoint);
        }

        let _exec = self.predict_execution.write().await;
        if synced_arc.lock().unwrap().as_ref() == Some(sanitized_params) {
            return Ok(endpoint);
        }

        match self
            .post_configure_http(port, &token, sanitized_params)
            .await
        {
            Ok(()) => {
                *synced_arc.lock().unwrap() = Some(sanitized_params.clone());
                Ok(endpoint)
            }
            Err(PredictLocalError::Transport(cfg_err) | PredictLocalError::HttpStatus(cfg_err)) => {
                *synced_arc.lock().unwrap() = None;
                if self.shutdown_token.is_cancelled() {
                    return Err(cfg_err);
                }
                tracing::warn!(
                    error = %cfg_err,
                    "Failed to sync local filter runtime parameters; restarting daemon to avoid stale-parameter verdicts"
                );
                self.stop_runner_inner(runner_guard).await;
                if let Err(restart_err) = self
                    .start_runner_locked(runner_guard, sanitized_params)
                    .await
                {
                    let err_msg = restart_err.to_string();
                    let should_emit = {
                        let st = self.state.lock().unwrap();
                        st.service_state != LocalFilterServiceState::Error
                            || st.last_error.as_deref() != Some(err_msg.as_str())
                    };
                    if should_emit {
                        self.emit_stage(LocalFilterServiceState::Error, None, Some(err_msg));
                    }
                    return Err(restart_err);
                }
                let restarted_ep = runner_guard
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
                Ok(restarted_ep)
            }
        }
    }

    async fn post_configure_http(
        &self,
        port: u16,
        token: &str,
        params: &FilterModelParams,
    ) -> Result<(), PredictLocalError> {
        let url = format!("http://127.0.0.1:{port}/configure");
        let send_fut = self
            .http_client
            .post(&url)
            .timeout(CONFIGURE_HTTP_TIMEOUT)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
            .json(&params.sanitized())
            .send_audited("filter.local.configure");
        let resp = tokio::select! {
            biased;
            _ = self.shutdown_token.cancelled() => {
                return Err(PredictLocalError::Transport(CoreError::Io(
                    std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "操作在应用退出时被取消",
                    ),
                )));
            }
            res = send_fut => {
                res.map_err(|e| {
                    PredictLocalError::Transport(CoreError::Network(format!(
                        "同步本地过滤运行参数失败: {e}"
                    )))
                })?
            }
        };
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
        let sanitized = params.sanitized();

        let effective_imgs: Vec<String> = if sanitized.enable_image {
            images_base64
                .iter()
                .filter(|s| !s.trim().is_empty())
                .take(sanitized.max_images as usize)
                .cloned()
                .collect()
        } else {
            Vec::new()
        };

        let sub_batches =
            split_images_by_raw_budget(&effective_imgs, MAX_RAW_PREDICT_IMAGE_BATCH_BYTES);
        let mut sub_results = Vec::with_capacity(sub_batches.len());

        for sub_imgs in sub_batches {
            let sub_count = sub_imgs.len();
            let req = RawFilterPredictRequest {
                title: Some(title.to_string()),
                description: description.map(str::to_owned),
                tags: tags.to_vec(),
                image_base64: None,
                images_base64: sub_imgs,
            };
            let probs = self
                .predict_raw_once(&req, auto_restart, &sanitized)
                .await?;
            sub_results.push((sub_count, probs));
        }

        merge_managed_predictions(sub_results, &sanitized)
            .ok_or_else(|| CoreError::Network("本地过滤模型未返回完整的逐图评分".into()))
    }

    fn endpoint_matches_synced(
        &self,
        endpoint: &str,
        sanitized_params: &FilterModelParams,
    ) -> bool {
        let st = self.state.lock().unwrap();
        if let (Some(ref ep), Some(ref synced_arc)) =
            (&st.service_endpoint, &st.active_runner_synced)
        {
            return ep == endpoint && synced_arc.lock().unwrap().as_ref() == Some(sanitized_params);
        }
        false
    }

    async fn ensure_synced_endpoint_for_predict(
        &self,
        sanitized_params: &FilterModelParams,
        auto_restart: bool,
    ) -> Result<String, CoreError> {
        if let Ok(mut guard) = self.op_lock.try_lock() {
            self.reap_exited_runner(&mut guard);
        }

        {
            let st = self.state.lock().unwrap();
            if let (Some(ref ep), Some(ref synced_arc)) =
                (&st.service_endpoint, &st.active_runner_synced)
            {
                if synced_arc.lock().unwrap().as_ref() == Some(sanitized_params) {
                    return Ok(ep.clone());
                }
            }
        }

        let mut guard = self.op_lock.lock().await;
        self.reap_exited_runner(&mut guard);
        if guard.is_some() {
            return self
                .sync_or_restart_runner_locked(&mut guard, sanitized_params)
                .await;
        }

        if !auto_restart || !self.is_deployed() {
            return Err(CoreError::InvalidInput(
                "本地过滤守护进程未运行，请先部署或启动服务".into(),
            ));
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

        self.start_runner_locked(&mut guard, sanitized_params)
            .await?;
        let endpoint = guard
            .as_ref()
            .map(|r| r.endpoint.clone())
            .ok_or_else(|| CoreError::Network("本地过滤服务启动后无可用地址".into()))?;
        let st_copy = {
            let st = self.state.lock().unwrap();
            let env = st.last_env.clone().unwrap_or_default();
            self.snapshot_from_state(env, &st)
        };
        drop(guard);
        self.events
            .emit(CoreEvent::LocalFilterStatusChanged { status: st_copy });
        Ok(endpoint)
    }

    async fn predict_raw_once(
        &self,
        payload: &RawFilterPredictRequest,
        auto_restart: bool,
        sanitized_params: &FilterModelParams,
    ) -> Result<ManagedFilterPrediction, CoreError> {
        let (endpoint, send_result) = {
            let mut chosen_endpoint = String::new();
            let mut exec_guard_opt = None;
            for _ in 0..2 {
                let ep = self
                    .ensure_synced_endpoint_for_predict(sanitized_params, auto_restart)
                    .await?;
                let exec_guard = self.predict_execution.read().await;
                if self.endpoint_matches_synced(&ep, sanitized_params) {
                    chosen_endpoint = ep;
                    exec_guard_opt = Some(exec_guard);
                    break;
                }
            }
            let Some(_exec_guard) = exec_guard_opt else {
                return Err(CoreError::Network(
                    "本地过滤运行参数正在切换，请重试评估".into(),
                ));
            };
            let res = self.send_predict_http(&chosen_endpoint, payload).await;
            (chosen_endpoint, res)
        };

        match send_result {
            Ok(probs) => Ok(probs),
            Err(PredictLocalError::HttpStatus(err)) => Err(err),
            Err(PredictLocalError::Transport(err)) => {
                if self.shutdown_token.is_cancelled() {
                    return Err(err);
                }
                let mut guard = self.op_lock.lock().await;
                self.reap_exited_runner(&mut guard);

                // If the runner bound to `endpoint` suffered a transport failure, terminate it before recovery
                let mut terminated_status: Option<LocalFilterDeploymentStatus> = None;
                if guard.as_ref().is_some_and(|r| r.endpoint == endpoint) {
                    let _exec = self.predict_execution.write().await;
                    self.stop_runner_inner(&mut guard).await;
                    let mut st = self.state.lock().unwrap();
                    st.service_state = LocalFilterServiceState::Error;
                    st.service_endpoint = None;
                    st.running_pid = None;
                    st.active_runner_synced = None;
                    st.stage_message = None;
                    st.last_error = Some(err.to_string());
                    let env = st.last_env.clone().unwrap_or_default();
                    terminated_status = Some(self.snapshot_from_state(env, &st));
                }

                if !auto_restart || !self.is_deployed() {
                    if let Some(status) = terminated_status {
                        self.events
                            .emit(CoreEvent::LocalFilterStatusChanged { status });
                    }
                    return Err(err);
                }

                // Reuse & sync if another concurrent caller already restarted a fresh runner
                let retry_endpoint = if guard.is_some() {
                    self.sync_or_restart_runner_locked(&mut guard, sanitized_params)
                        .await?
                } else {
                    {
                        let mut last_restart = self.last_auto_restart_at.lock().unwrap();
                        if last_restart.is_some_and(|t| t.elapsed() < AUTO_RESTART_COOLDOWN) {
                            if let Some(status) = terminated_status {
                                self.events
                                    .emit(CoreEvent::LocalFilterStatusChanged { status });
                            }
                            return Err(err);
                        }
                        *last_restart = Some(Instant::now());
                    }
                    self.start_runner_locked(&mut guard, sanitized_params)
                        .await?;
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

                let _exec_guard = self.predict_execution.read().await;
                if !self.endpoint_matches_synced(&retry_endpoint, sanitized_params) {
                    return Err(CoreError::Network(
                        "本地过滤运行参数正在切换，请重试评估".into(),
                    ));
                }
                match self.send_predict_http(&retry_endpoint, payload).await {
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
        payload: &RawFilterPredictRequest,
    ) -> Result<ManagedFilterPrediction, PredictLocalError> {
        let url = format!("{}/predict", endpoint.trim_end_matches('/'));
        let send_fut = self
            .http_client
            .post(&url)
            .timeout(PREDICT_HTTP_TIMEOUT)
            .json(payload)
            .send_audited_quiet("filter.local.predict");
        let resp = tokio::select! {
            biased;
            _ = self.shutdown_token.cancelled() => {
                return Err(PredictLocalError::Transport(CoreError::Io(
                    std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "操作在应用退出时被取消",
                    ),
                )));
            }
            res = send_fut => {
                res.map_err(|e| {
                    PredictLocalError::Transport(CoreError::Network(format!(
                        "调用本地过滤模型推理失败: {e}"
                    )))
                })?
            }
        };
        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().await.unwrap_or_default();
            return Err(PredictLocalError::HttpStatus(CoreError::Network(format!(
                "本地过滤模型推理返回 HTTP {status}: {detail}"
            ))));
        }
        resp.json::<ManagedFilterPrediction>().await.map_err(|e| {
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
        match self.spawn_runner(params, self.web_gpu_enabled()).await {
            Ok(runner) => {
                self.set_running_runner(runner_guard, runner);
                Ok(())
            }
            Err(err) => {
                self.emit_stage(LocalFilterServiceState::Error, None, Some(err.to_string()));
                *self.last_auto_restart_at.lock().unwrap() = Some(Instant::now());
                Err(err)
            }
        }
    }

    async fn spawn_runner(
        &self,
        params: &FilterModelParams,
        web_gpu_enabled: bool,
    ) -> Result<RunningRunner, CoreError> {
        let deploy_dir = self.paths.filter_repo_dir.join("deploy");
        let venv_py = venv_python_path(self.venv_dir(web_gpu_enabled));
        if !is_executable_file(&venv_py) {
            return Err(CoreError::InvalidInput(
                "本地过滤模型尚未完成部署，请先执行一键部署".into(),
            ));
        }
        validate_deploy_assets(&deploy_dir, web_gpu_enabled)?;

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

        self.spawn_runner_process(&venv_py, params, web_gpu_enabled)
            .await
    }

    async fn spawn_runner_process(
        &self,
        venv_py: &Path,
        params: &FilterModelParams,
        web_gpu_enabled: bool,
    ) -> Result<RunningRunner, CoreError> {
        let rng = SystemRandom::new();
        let mut token_bytes = [0u8; 32];
        rng.fill(&mut token_bytes)
            .map_err(|_| CoreError::Config("生成守护进程鉴权令牌失败".into()))?;
        let runner_token = hex::encode(token_bytes);

        let boot_params = params.sanitized();
        let params_json =
            serde_json::to_string(&boot_params).map_err(|e| CoreError::Config(e.to_string()))?;

        let mut cmd = tokio::process::Command::new(venv_py);
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
        let inherited_provider = std::env::var(VISION_PROVIDER_ENV).ok();
        configure_runner_vision_provider(&mut cmd, web_gpu_enabled, inherited_provider.as_deref());
        configure_tokio_cmd(&mut cmd);

        let audit = CommandAudit::start("filter.runner", cmd.as_std());
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                audit.finish_error(&err);
                return Err(err.into());
            }
        };
        let pid = child.id();
        audit.finish_spawned(pid);
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
            let mut pending_line = String::new();
            loop {
                match stderr.read(&mut buf).await {
                    Ok(0) | Err(_) => {
                        log_runner_stderr_line(pid, &pending_line);
                        break;
                    }
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(&buf[..n]);
                        pending_line.push_str(&chunk);
                        while let Some(pos) = pending_line.find('\n') {
                            log_runner_stderr_line(pid, &pending_line[..pos]);
                            pending_line.drain(..=pos);
                        }
                        if pending_line.len() > MAX_STDERR_TAIL_BYTES {
                            log_runner_stderr_line(pid, &pending_line);
                            pending_line.clear();
                        }
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
        if web_gpu_enabled && banner.vision_provider.as_deref() != Some(WEBGPU_PROVIDER) {
            drop(stdin);
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(CoreError::Unsupported(
                "本地过滤守护进程未能启用 WebGPU 视觉推理".into(),
            ));
        }

        let endpoint = format!("http://127.0.0.1:{}", banner.port);

        // Re-check latest desired_params in case save_config updated them while Python was booting
        let latest_desired = self.desired_params.read().unwrap().clone();
        let initial_synced = if latest_desired != boot_params {
            match self
                .post_configure_http(banner.port, &runner_token, &latest_desired)
                .await
            {
                Ok(()) => Some(latest_desired),
                Err(PredictLocalError::Transport(err) | PredictLocalError::HttpStatus(err)) => {
                    tracing::warn!(error = %err, "Failed to sync latest filter parameters after daemon start; marked for reconfiguration");
                    None
                }
            }
        } else {
            Some(boot_params)
        };
        let synced_params = Arc::new(Mutex::new(initial_synced));

        Ok(RunningRunner {
            child,
            stdin: Some(stdin),
            pid,
            endpoint,
            port: banner.port,
            runner_token,
            stderr_tail,
            synced_params,
        })
    }

    fn set_running_runner(&self, runner_guard: &mut Option<RunningRunner>, runner: RunningRunner) {
        let mut st = self.state.lock().unwrap();
        st.service_state = LocalFilterServiceState::Running;
        st.service_endpoint = Some(runner.endpoint.clone());
        st.running_pid = runner.pid;
        st.active_runner_synced = Some(runner.synced_params.clone());
        st.stage_message = None;
        st.last_error = None;
        *runner_guard = Some(runner);
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
            if let Some(ref r) = *runner_guard {
                *r.synced_params.lock().unwrap() = None;
            }
            *runner_guard = None;
            let status_snapshot = {
                let mut st = self.state.lock().unwrap();
                st.service_state = LocalFilterServiceState::Error;
                st.service_endpoint = None;
                st.running_pid = None;
                st.active_runner_synced = None;
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
        if let Some(mut running) = runner_guard.take() {
            *running.synced_params.lock().unwrap() = None;
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
        if let Some(err) = last_error.as_deref() {
            tracing::warn!(
                state = ?service_state,
                stage = stage_message.as_deref().unwrap_or_default(),
                error = %err,
                "Local filter service stage changed (with error)"
            );
        } else {
            tracing::info!(
                state = ?service_state,
                stage = stage_message.as_deref().unwrap_or_default(),
                "Local filter service stage changed"
            );
        }
        let status = {
            let mut st = self.state.lock().unwrap();
            st.service_state = service_state;
            if !matches!(service_state, LocalFilterServiceState::Running) {
                st.service_endpoint = None;
                st.running_pid = None;
                st.active_runner_synced = None;
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
        let web_gpu_enabled = self.web_gpu_enabled();
        LocalFilterDeploymentStatus {
            env,
            deployed: self.is_backend_deployed(web_gpu_enabled),
            web_gpu_enabled,
            repo_dir: clean_os_path(&self.paths.filter_repo_dir),
            venv_dir: clean_os_path(self.venv_dir(web_gpu_enabled)),
            service_state: st.service_state,
            service_endpoint: st.service_endpoint.clone(),
            running_pid: st.running_pid,
            current_commit: st.current_commit.clone(),
            remote_latest_commit: st.remote_latest_commit.clone(),
            update_available: st.update_available,
            recent_commits: st.recent_commits.clone(),
            stage_message: st.stage_message.clone(),
            last_error: st.last_error.clone(),
            cache_generation: None,
        }
    }
}

/// Forwards one line of the Python runner's stderr into the application log.
fn log_runner_stderr_line(pid: Option<u32>, line: &str) {
    let line = line.trim();
    if !line.is_empty() {
        tracing::info!(
            target: "l4a::filter_runner",
            pid,
            "{}",
            l4a_logging::redact::truncate(line, 1000)
        );
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

    let audit = CommandAudit::start("filter.deploy", cmd.as_std());
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            audit.finish_error(&err);
            return Err(err.into());
        }
    };
    tokio::select! {
        () = cancel.cancelled() => {
            audit.finish_message("cancelled because the application is exiting");
            Err(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "操作在应用退出时被取消",
            )))
        }
        res = tokio::time::timeout(timeout_dur, child.wait_with_output()) => {
            match res {
                Ok(Ok(output)) => {
                    audit.record_output(&output);
                    Ok(output)
                }
                Ok(Err(err)) => {
                    audit.finish_error(&err);
                    Err(CoreError::Io(err))
                }
                Err(_) => {
                    audit.finish_message(&format!("timed out after {}s", timeout_dur.as_secs()));
                    let message = format!("外部命令执行超时 ({}s)", timeout_dur.as_secs());
                    Err(CoreError::Network(message))
                }
            }
        }
    }
}

async fn venv_is_usable_async(
    cancel: &CancellationToken,
    venv_py: &Path,
    web_gpu_enabled: bool,
) -> Result<bool, CoreError> {
    if web_gpu_enabled {
        let version = run_cancellable_command(
            cancel,
            venv_py.as_os_str(),
            None,
            &[
                "-c",
                PYTHON_VERSION_CHECK,
                &WEBGPU_PYTHON_MIN_MINOR.to_string(),
            ],
            &[],
            Duration::from_secs(20),
        )
        .await?;
        if !version.status.success() {
            return Ok(false);
        }
    }
    let out = run_cancellable_command(
        cancel,
        venv_py.as_os_str(),
        None,
        &["-m", "pip", "--version"],
        &[("PYTHONIOENCODING", "utf-8")],
        Duration::from_secs(20),
    )
    .await?;
    Ok(out.status.success())
}

async fn is_valid_git_repo_async(
    cancel: &CancellationToken,
    repo_dir: &Path,
) -> Result<bool, CoreError> {
    let out = run_cancellable_command(
        cancel,
        OsStr::new("git"),
        Some(repo_dir),
        &[
            "--git-dir",
            ".git",
            "--work-tree",
            ".",
            "rev-parse",
            "--is-inside-work-tree",
        ],
        GIT_STALL_ENVS,
        GIT_FAST_TIMEOUT,
    )
    .await?;
    Ok(out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true")
}

async fn venv_deps_importable_async(
    cancel: &CancellationToken,
    venv_py: &Path,
) -> Result<bool, CoreError> {
    let out = run_cancellable_command(
        cancel,
        venv_py.as_os_str(),
        None,
        &["-c", "import onnxruntime, numpy, PIL"],
        &[("PYTHONIOENCODING", "utf-8")],
        Duration::from_secs(20),
    )
    .await?;
    Ok(out.status.success())
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

async fn try_rev_parse_commit_async(
    cancel: &CancellationToken,
    repo_dir: &Path,
    spec: &str,
) -> Option<String> {
    let out = run_git_async(
        cancel,
        repo_dir,
        &["rev-parse", "--verify", "--quiet", spec],
        GIT_FAST_TIMEOUT,
    )
    .await
    .ok()?;
    let sha = out.trim();
    (sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| sha.to_ascii_lowercase())
}

async fn resolve_target_commit_sha_async(
    cancel: &CancellationToken,
    repo_dir: &Path,
    target: &str,
) -> Result<String, CoreError> {
    let origin_spec = format!("refs/remotes/origin/{target}^{{commit}}");
    let direct_spec = format!("{target}^{{commit}}");

    for spec in [&origin_spec, &direct_spec] {
        if let Some(sha) = try_rev_parse_commit_async(cancel, repo_dir, spec).await {
            return Ok(sha);
        }
    }

    run_git_async(
        cancel,
        repo_dir,
        &["fetch", "origin", target],
        GIT_NETWORK_TIMEOUT,
    )
    .await?;

    for spec in [
        origin_spec.as_str(),
        direct_spec.as_str(),
        "FETCH_HEAD^{commit}",
    ] {
        if let Some(sha) = try_rev_parse_commit_async(cancel, repo_dir, spec).await {
            return Ok(sha);
        }
    }

    Err(CoreError::InvalidInput(format!(
        "无法解析目标 Commit 或远端分支: {target}"
    )))
}

pub(crate) fn split_images_by_raw_budget(
    images_base64: &[String],
    max_raw_bytes: usize,
) -> Vec<Vec<String>> {
    if images_base64.is_empty() {
        return vec![Vec::new()];
    }
    let mut batches: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_bytes: usize = 0;

    for img in images_base64 {
        let est_raw = img.len().saturating_mul(3) / 4;
        if !current.is_empty() && current_bytes.saturating_add(est_raw) > max_raw_bytes {
            batches.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current_bytes = current_bytes.saturating_add(est_raw);
        current.push(img.clone());
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

// A raw endpoint owns its calibration and exposes no raw gallery rows.
pub(crate) fn merge_raw_max(parts: Vec<(usize, FilterProbabilities)>) -> FilterProbabilities {
    let mut parts = parts.into_iter();
    let Some((count, mut merged)) = parts.next() else {
        return FilterProbabilities::default();
    };
    merged.corpse_image_indices.retain(|&index| index < count);
    if parts.len() == 0 {
        return merged;
    }
    let mut aligned = count == merged.per_image_probs.len();
    let max_optional = |a: Option<f32>, b: Option<f32>| match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    let mut fusion_prob = merged.fusion_score();
    let mut offset = count;
    for (count, part) in parts {
        fusion_prob = fusion_prob.max(part.fusion_score());
        merged.merge_corpse_scores(&part, offset, count);
        offset += count;
        merged.gore_prob = merged.gore_prob.max(part.gore_prob);
        merged.nsfw_prob = merged.nsfw_prob.max(part.nsfw_prob);
        merged.image_prob = max_optional(merged.image_prob, part.image_prob);
        merged.image_gore_prob = max_optional(merged.image_gore_prob, part.image_gore_prob);
        merged.image_nsfw_prob = max_optional(merged.image_nsfw_prob, part.image_nsfw_prob);
        aligned &= count == part.per_image_probs.len();
        merged.per_image_probs.extend(part.per_image_probs);
    }
    if !aligned {
        merged.per_image_probs.clear();
    }
    merged.set_fusion_score(fusion_prob);
    merged
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
                    short_sha: sha.chars().take(7).collect(),
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
    fn cpu_runner_clears_inherited_webgpu_and_keeps_other_providers() {
        let mut cmd = tokio::process::Command::new("python");
        configure_runner_vision_provider(&mut cmd, false, Some(" WeBgPu "));
        let (key, value) = cmd.as_std().get_envs().next().unwrap();
        assert_eq!(key, VISION_PROVIDER_ENV);
        assert_eq!(value, None);

        let mut cmd = tokio::process::Command::new("python");
        configure_runner_vision_provider(&mut cmd, false, Some("cuda"));
        assert_eq!(cmd.as_std().get_envs().count(), 0);

        configure_runner_vision_provider(&mut cmd, true, Some("cuda"));
        let (key, value) = cmd.as_std().get_envs().next().unwrap();
        assert_eq!(key, VISION_PROVIDER_ENV);
        assert_eq!(value, Some(OsStr::new("webgpu")));
    }

    #[cfg(unix)]
    #[test]
    fn webgpu_probe_selects_a_compatible_versioned_python() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        for (name, minor, has_venv) in [
            ("python3", 10, 1),
            ("python3.13", 13, 0),
            ("python3.12", 12, 1),
        ] {
            let path = dir.path().join(name);
            fs::write(
                &path,
                format!("#!/bin/sh\n[ \"$3\" -le {minor} ] || exit 2\nprintf '%s\\n' \"$0\" '3.{minor}.0' '{has_venv}'\n"),
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let path = Some(dir.path().as_os_str());
        let (cpu_path, cpu_version, _) = probe_deployment_python(false, path).unwrap();
        assert_eq!(Path::new(&cpu_path), dir.path().join("python3"));
        assert_eq!(cpu_version, "3.10.0");

        let (webgpu_path, webgpu_version, has_venv) = probe_deployment_python(true, path).unwrap();
        assert_eq!(Path::new(&webgpu_path), dir.path().join("python3.12"));
        assert_eq!(webgpu_version, "3.12.0");
        assert!(has_venv);
    }

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

    #[test]
    fn split_images_by_raw_budget_enforces_30_mib_subbatches_and_merges_per_image_probs() {
        // Three ~12 MiB raw images (~16 MiB base64 each):
        // img0 + img1 = 24 MiB <= 30 MiB; adding img2 = 36 MiB > 30 MiB -> [2, 1] sub-batches!
        let twelve_mib_b64 = "A".repeat(16 * 1024 * 1024);
        let imgs = vec![
            twelve_mib_b64.clone(),
            twelve_mib_b64.clone(),
            twelve_mib_b64,
        ];
        let batches = split_images_by_raw_budget(&imgs, MAX_RAW_PREDICT_IMAGE_BATCH_BYTES);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].len(), 2);
        assert_eq!(batches[1].len(), 1);

        let merged = merge_raw_max(vec![
            (
                2,
                FilterProbabilities {
                    prob: 0.12,
                    gore_prob: 0.02,
                    nsfw_prob: 0.11,
                    text_prob: Some(0.03),
                    image_prob: Some(0.12),
                    per_image_probs: vec![0.04, 0.12],
                    ..Default::default()
                },
            ),
            (
                1,
                FilterProbabilities {
                    prob: 0.91,
                    gore_prob: 0.01,
                    nsfw_prob: 0.90,
                    text_prob: Some(0.03),
                    image_prob: Some(0.91),
                    per_image_probs: vec![0.91],
                    ..Default::default()
                },
            ),
        ]);
        assert_eq!(merged.prob, 0.91);
        assert_eq!(merged.nsfw_prob, 0.90);
        assert_eq!(merged.image_prob, Some(0.91));
        assert_eq!(merged.per_image_probs, vec![0.04, 0.12, 0.91]);
    }

    #[test]
    fn merge_raw_max_keeps_corpse_floor_out_of_fusion_and_remaps_hits() {
        let single = FilterProbabilities {
            corpse_image_indices: vec![0, 1],
            ..Default::default()
        };
        assert_eq!(merge_raw_max(vec![(1, single)]).corpse_image_indices, [0]);

        let merged = merge_raw_max(vec![
            (
                2,
                FilterProbabilities {
                    prob: 0.20,
                    fusion_prob: Some(0.05),
                    corpse_prob: Some(0.41),
                    corpse_pile_prob: Some(0.02),
                    corpse_review_floor: Some(0.20),
                    // Index 2 is outside this two-image sub-batch and must be dropped.
                    corpse_image_indices: vec![1, 2],
                    per_image_probs: vec![0.03, 0.05],
                    ..Default::default()
                },
            ),
            (
                2,
                FilterProbabilities {
                    prob: 0.20,
                    fusion_prob: Some(0.12),
                    corpse_prob: Some(0.39),
                    corpse_pile_prob: Some(0.25),
                    corpse_review_floor: Some(0.20),
                    corpse_image_indices: vec![0],
                    per_image_probs: vec![0.12, 0.01],
                    ..Default::default()
                },
            ),
        ]);
        assert_eq!(merged.prob, 0.20);
        assert_eq!(merged.fusion_score(), 0.12);
        assert_eq!(merged.corpse_prob, Some(0.41));
        assert_eq!(merged.corpse_pile_prob, Some(0.25));
        assert_eq!(merged.corpse_image_indices, vec![1, 2]);
        assert_eq!(merged.per_image_probs, vec![0.03, 0.05, 0.12, 0.01]);
    }

    #[cfg(unix)]
    #[test]
    fn venv_python_path_and_probe_preserve_virtualenv_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let sys_env = probe_environment();
        assert!(sys_env.python_available);
        let sys_py = sys_env.python_path.as_ref().unwrap();
        let real_target = fs::canonicalize(sys_py).unwrap();

        let fake_venv = dir.path().join("custom-venv");
        let status = std::process::Command::new(sys_py)
            .args(["-m", "venv", "--without-pip"])
            .arg(&fake_venv)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "python -m venv --without-pip must succeed"
        );

        let bin_dir = fake_venv.join("bin");
        let symlink_py = bin_dir.join("python");
        let symlink_py3 = bin_dir.join("python3");

        let resolved_venv_py = venv_python_path(&fake_venv);
        assert_eq!(
            resolved_venv_py,
            make_absolute_preserving_symlinks(&symlink_py)
        );
        assert_ne!(
            resolved_venv_py, real_target,
            "venv_python_path must preserve symlink instead of canonicalizing"
        );
        assert!(fs::symlink_metadata(&resolved_venv_py)
            .unwrap()
            .file_type()
            .is_symlink());

        let probed = probe_python_candidates(
            &[("python3", &[])],
            Some(bin_dir.as_os_str()),
            CPU_PYTHON_MIN_MINOR,
        )
        .expect("probe_python_candidates should find symlinked python3");
        assert_eq!(
            Path::new(&probed.0),
            make_absolute_preserving_symlinks(&symlink_py3)
        );
        assert_ne!(Path::new(&probed.0), real_target.as_path());
    }

    #[tokio::test]
    async fn send_predict_http_uses_snake_case_images_base64_and_never_camel_case() {
        use crate::engine::NullEventSink;
        use axum::{http::StatusCode, routing::post, Json, Router};

        let app = Router::new().route(
            "/predict",
            post(|Json(body): Json<serde_json::Value>| async move {
                if body.get("imagesBase64").is_some() || body.get("imageBase64").is_some() {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": "camelCase forbidden" })),
                    );
                }
                let imgs = body
                    .get("images_base64")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                assert_eq!(imgs.len(), 2);
                assert_eq!(imgs[0].as_str(), Some("aW1nMA=="));
                assert_eq!(imgs[1].as_str(), Some("aW1nMQ=="));
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "prob": 0.88,
                        "gore_prob": 0.05,
                        "nsfw_prob": 0.87,
                        "per_image_probs": [0.10, 0.88]
                    })),
                )
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_runtime_dir(dir.path().join("runtime"));
        let mgr = LocalFilterManager::new(paths, Arc::new(NullEventSink));
        let payload = RawFilterPredictRequest {
            title: Some("Test".into()),
            description: None,
            tags: vec!["Map".into()],
            image_base64: None,
            images_base64: vec!["aW1nMA==".into(), "aW1nMQ==".into()],
        };
        let prediction = mgr
            .send_predict_http(&format!("http://{addr}"), &payload)
            .await
            .unwrap_or_else(|_| panic!("expected strict snake_case /predict to succeed"));
        let probs = prediction.probabilities;
        assert_eq!(probs.prob, 0.88);
        assert_eq!(probs.per_image_probs, vec![0.10, 0.88]);

        server_handle.abort();
    }

    #[tokio::test]
    async fn is_valid_git_repo_rejects_corrupt_git_dir_inside_parent_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("git")
            .arg("init")
            .arg(dir.path())
            .status()
            .unwrap();
        assert!(status.success(), "git init must succeed");

        let corrupt_child = dir.path().join("corrupt-child");
        fs::create_dir_all(corrupt_child.join(".git")).unwrap();

        let cancel = CancellationToken::new();
        assert!(
            is_valid_git_repo_async(&cancel, dir.path()).await.unwrap(),
            "valid parent git repo must be accepted"
        );
        assert!(
            !is_valid_git_repo_async(&cancel, &corrupt_child)
                .await
                .unwrap(),
            "corrupt nested .git directory must NOT fall back to ancestor git repo"
        );

        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(
            is_valid_git_repo_async(&cancelled, dir.path())
                .await
                .is_err(),
            "cancelled token must return Err rather than Ok(false)"
        );
    }
}
