#[allow(dead_code)]
mod support;

use axum::{
    extract::State as AxumState,
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use image::{ImageBuffer, Rgb};
use l4a_core::db::LocalDb;
use l4a_core::{AppPaths, EventSink, L4aEngine, NullEventSink, TokenVault};
use l4a_protocol::{
    AppConfig, CatalogFilterPublicInfo, ClientFilterConfig, ClientFilterSourceMode, CoreEvent,
    DeployLocalFilterRequest, ExclusionDisplayLevel, FilterDecisionSource,
    FilterEvaluationBatchResponse, FilterEvaluationTarget, FilterItemVerdict, FilterPredictRequest,
    FilterProbabilities, FilterTargetOutcome, LlmTokenSource, LocalFilterServiceState,
    RawFilterPredictRequest, UpsertFilterExclusionRequest,
};
use l4a_steam::MockSteamBridge;
use serde_json::{json, Value};
use std::io::Cursor;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use tokio::net::TcpListener;

use support::{create_test_vpk, make_engine, make_engine_with_events, RecordedEvents};

fn unwrap_verdicts(batch: FilterEvaluationBatchResponse) -> Vec<FilterItemVerdict> {
    assert!(
        batch.policy_signature.starts_with("pol_"),
        "expected canonical policy_signature starting with pol_, got {}",
        batch.policy_signature
    );
    batch
        .items
        .into_iter()
        .map(|item| match item.outcome {
            FilterTargetOutcome::Completed { verdict, complete } => {
                assert_eq!(verdict.complete, complete);
                verdict
            }
            FilterTargetOutcome::Failed { error } => {
                panic!(
                    "expected target {} to succeed, failed with: {error}",
                    item.target_key
                )
            }
        })
        .collect()
}

fn make_tiny_jpeg(r: u8, g: u8, b: u8) -> Vec<u8> {
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_pixel(4, 4, Rgb([r, g, b]));
    let mut cursor = Cursor::new(Vec::new());
    img.write_to(&mut cursor, image::ImageFormat::Jpeg).unwrap();
    cursor.into_inner()
}

fn run_git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test Bot")
        .env("GIT_AUTHOR_EMAIL", "bot@example.com")
        .env("GIT_COMMITTER_NAME", "Test Bot")
        .env("GIT_COMMITTER_EMAIL", "bot@example.com")
        .output()
        .expect("git should execute");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write_mock_left4filters_deploy(repo_dir: &Path, version: &str, fixed_prob: f32) {
    let deploy_dir = repo_dir.join("deploy");
    std::fs::create_dir_all(&deploy_dir).unwrap();
    let py = format!(
        r#"class Left4Filter:
    def __init__(self, deploy_dir, num_threads=4):
        self.config = {{"model_name": "MockLeft4Filters", "version": "{version}"}}
        self.num_threads = num_threads

    def predict(self, title="", description="", tags=None, image=None, images=None):
        p = {fixed_prob}
        if "nsfw" in (title or "").lower():
            p = 0.88
        return {{"prob": p, "gore_prob": 0.02, "nsfw_prob": p}}
"#
    );
    std::fs::write(deploy_dir.join("l4filter.py"), py).unwrap();
    std::fs::write(deploy_dir.join("vision_encoder.onnx"), b"mock-vis-onnx").unwrap();
    std::fs::write(deploy_dir.join("fusion_head.onnx"), b"mock-head-onnx").unwrap();
    std::fs::write(deploy_dir.join("text_vocab.json"), r#"{"vocab":{}}"#).unwrap();
    // Empty requirements.txt avoids network pip calls in offline integration tests
    std::fs::write(deploy_dir.join("requirements.txt"), "# mock offline test\n").unwrap();
}

fn install_offline_runner_shims(venv_dir: &Path) {
    let venv_py = if cfg!(windows) {
        venv_dir.join("Scripts").join("python.exe")
    } else {
        venv_dir.join("bin").join("python")
    };
    let site_out = Command::new(&venv_py)
        .args(["-c", "import site; print(site.getsitepackages()[0])"])
        .output()
        .unwrap();
    assert!(site_out.status.success());
    let site_dir = Path::new(String::from_utf8_lossy(&site_out.stdout).trim()).to_path_buf();
    write_offline_runner_shims(&site_dir);
}

fn write_offline_runner_shims(site_dir: &Path) {
    std::fs::write(site_dir.join("onnxruntime.py"), "").unwrap();
    std::fs::write(
        site_dir.join("numpy.py"),
        r#"float32 = float
class ndarray:
    def __init__(self, rows):
        self._rows = [list(r) for r in rows]
        self.shape = (len(self._rows), len(self._rows[0]) if self._rows else 0)
    def __getitem__(self, idx):
        return self._rows[idx]
    def max(self, axis=0):
        cols = self.shape[1]
        return [max(r[c] for r in self._rows) for c in range(cols)]
    def mean(self, axis=0):
        cols = self.shape[1]
        n = max(1, len(self._rows))
        return [sum(r[c] for r in self._rows) / n for c in range(cols)]
def asarray(rows, dtype=None):
    return ndarray(rows)
def sort(arr, axis=0):
    cols = arr.shape[1]
    sorted_cols = [sorted(r[c] for r in arr._rows) for c in range(cols)]
    return ndarray([[sorted_cols[c][r] for c in range(cols)] for r in range(arr.shape[0])])
"#,
    )
    .unwrap();
    let pil_dir = site_dir.join("PIL");
    std::fs::create_dir_all(&pil_dir).unwrap();
    std::fs::write(pil_dir.join("__init__.py"), "from . import Image\n").unwrap();
    std::fs::write(
        pil_dir.join("Image.py"),
        r#"MAX_IMAGE_PIXELS = 16777216

class ImageHeader:
    size = (4, 4)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False

def open(_stream):
    return ImageHeader()
"#,
    )
    .unwrap();
}

#[test]
fn token_vault_roundtrip_identity_isolation_and_env_var_mode() {
    let dir = tempdir().unwrap();
    let sealed_path = dir.path().join("config").join("llm_token.sealed");
    let key_a = dir.path().join("os-config-a").join("vault.key");
    let key_b = dir.path().join("os-config-b").join("vault.key");

    let vault = TokenVault::with_custom_identity(&sealed_path, &key_a, "machine-001", "uid:1000");
    assert!(!vault.has_sealed_token());

    vault.seal("sk-live-secret-token-abc123").unwrap();
    assert!(vault.has_sealed_token());
    assert_eq!(
        vault.unseal().unwrap().as_str(),
        "sk-live-secret-token-abc123"
    );

    // Wrong machine_id must be rejected
    let wrong_machine =
        TokenVault::with_custom_identity(&sealed_path, &key_a, "machine-999", "uid:1000");
    assert!(wrong_machine.unseal().is_err());

    // Wrong user_id must be rejected
    let wrong_user =
        TokenVault::with_custom_identity(&sealed_path, &key_a, "machine-001", "uid:2000");
    assert!(wrong_user.unseal().is_err());

    // Wrong user vault key must be rejected
    let other_key_vault = TokenVault::with_custom_identity(
        dir.path().join("other.sealed"),
        &key_b,
        "machine-001",
        "uid:1000",
    );
    other_key_vault.seal("other-token").unwrap();
    let wrong_key =
        TokenVault::with_custom_identity(&sealed_path, &key_b, "machine-001", "uid:1000");
    assert!(wrong_key.unseal().is_err());

    // EnvVar mode never touches disk and validates env var name
    let env_only_sealed = dir.path().join("never_created.sealed");
    let env_vault = TokenVault::with_custom_identity(
        &env_only_sealed,
        dir.path().join("never_created.key"),
        "machine-001",
        "uid:1000",
    );
    let unique_env = format!("L4A_TEST_TOKEN_{}", std::process::id());
    std::env::set_var(&unique_env, "  sk-env-only-secret  ");
    let resolved = env_vault
        .resolve_token(LlmTokenSource::EnvVar, &unique_env)
        .unwrap();
    assert_eq!(resolved.as_str(), "sk-env-only-secret");
    assert!(!env_only_sealed.exists());
    let status = env_vault.status(&unique_env);
    assert!(status.env_var_present);
    assert!(!status.has_encrypted_token);
    std::env::remove_var(&unique_env);

    assert!(env_vault
        .resolve_token(LlmTokenSource::EnvVar, "123-INVALID-ENV")
        .is_err());
}

#[tokio::test]
async fn local_git_repo_deployment_pinning_and_update_tracking() {
    let dir = tempdir().unwrap();
    let upstream_repo = dir.path().join("upstream-left4filters");
    std::fs::create_dir_all(&upstream_repo).unwrap();

    run_git(&upstream_repo, &["init", "-b", "main"]);
    write_mock_left4filters_deploy(&upstream_repo, "1.0.0", 0.12);
    run_git(&upstream_repo, &["add", "."]);
    run_git(&upstream_repo, &["commit", "-m", "feat: initial model v1"]);
    let commit1 = run_git(&upstream_repo, &["rev-parse", "HEAD"]);

    // Ensure distinct commit timestamp / second commit
    write_mock_left4filters_deploy(&upstream_repo, "2.0.0", 0.18);
    run_git(&upstream_repo, &["add", "."]);
    run_git(
        &upstream_repo,
        &["commit", "-m", "feat: recalibrated model v2"],
    );
    let commit2 = run_git(&upstream_repo, &["rev-parse", "HEAD"]);
    assert_ne!(commit1, commit2);

    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();
    let events = Arc::new(RecordedEvents::default());
    let cfg = AppConfig {
        loading_dir: loading_dir.to_str().unwrap().into(),
        https_only: false,
        filter: ClientFilterConfig {
            repo_url: upstream_repo.to_str().unwrap().into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let engine = make_engine_with_events(
        dir.path(),
        cfg,
        Arc::new(MockSteamBridge::new(false)),
        events.clone(),
    );

    let initial_status = engine.get_local_filter_status().await.unwrap();
    assert!(initial_status.env.git_available);
    assert!(initial_status.env.python_available);
    assert!(!initial_status.deployed);

    // 1. Deploy pinned to commit1
    let deployed_v1 = engine
        .deploy_local_filter(DeployLocalFilterRequest {
            repo_url: Some(upstream_repo.to_str().unwrap().into()),
            track_latest: false,
            target_commit: Some(commit1.clone()),
            start_after_deploy: false,
            install_web_gpu: None,
        })
        .await
        .unwrap();

    assert!(deployed_v1.deployed);
    assert_eq!(deployed_v1.service_state, LocalFilterServiceState::Stopped);
    assert_eq!(
        deployed_v1.current_commit.as_ref().map(|c| c.sha.as_str()),
        Some(commit1.as_str())
    );
    assert!(engine.paths().filter_runner_path.is_file());
    // Verify git working tree inside Left4Filters is 100% clean
    let porcelain = run_git(&engine.paths().filter_repo_dir, &["status", "--porcelain"]);
    assert!(porcelain.is_empty(), "expected clean repo, got {porcelain}");

    // 2. Check updates should detect commit2 and list both commits
    let update_status = engine.check_local_filter_updates().await.unwrap();
    assert!(update_status.update_available);
    assert_eq!(
        update_status
            .current_commit
            .as_ref()
            .map(|c| c.sha.as_str()),
        Some(commit1.as_str())
    );
    assert_eq!(
        update_status
            .remote_latest_commit
            .as_ref()
            .map(|c| c.sha.as_str()),
        Some(commit2.as_str())
    );
    assert_eq!(update_status.recent_commits.len(), 2);

    // 3. One-click update to latest (commit2)
    let deployed_v2 = engine
        .deploy_local_filter(DeployLocalFilterRequest {
            repo_url: None,
            track_latest: true,
            target_commit: None,
            start_after_deploy: false,
            install_web_gpu: None,
        })
        .await
        .unwrap();

    assert!(deployed_v2.deployed);
    assert!(!deployed_v2.update_available);
    assert_eq!(
        deployed_v2.current_commit.as_ref().map(|c| c.sha.as_str()),
        Some(commit2.as_str())
    );

    let recorded = events.drain();
    assert!(recorded
        .iter()
        .any(|ev| matches!(ev, CoreEvent::LocalFilterStatusChanged { .. })));

    engine.shutdown().await;
}

#[tokio::test]
async fn webgpu_deployment_is_optional_and_persists_the_installed_backend() {
    let dir = tempdir().unwrap();
    let upstream_repo = dir.path().join("upstream");
    std::fs::create_dir(&upstream_repo).unwrap();
    run_git(&upstream_repo, &["init", "-b", "main"]);
    write_mock_left4filters_deploy(&upstream_repo, "1.0.0", 0.1);
    run_git(&upstream_repo, &["add", "deploy"]);
    run_git(&upstream_repo, &["commit", "-m", "CPU model"]);

    let config = AppConfig {
        filter: ClientFilterConfig {
            repo_url: upstream_repo.to_str().unwrap().into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let engine = make_engine(
        dir.path(),
        config.clone(),
        Arc::new(MockSteamBridge::new(false)),
    );
    let request: DeployLocalFilterRequest =
        serde_json::from_value(json!({"startAfterDeploy": false})).unwrap();
    let cpu = engine.deploy_local_filter(request.clone()).await.unwrap();
    assert!(cpu.deployed);
    assert!(!cpu.web_gpu_enabled);

    let webgpu_request = DeployLocalFilterRequest {
        install_web_gpu: Some(true),
        ..request.clone()
    };
    let missing = engine
        .deploy_local_filter(webgpu_request.clone())
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("requirements-webgpu.txt"));
    assert!(!engine.paths().filter_webgpu_venv_dir.exists());

    let deploy_dir = upstream_repo.join("deploy");
    let webgpu_requirements = deploy_dir.join("requirements-webgpu.txt");
    std::fs::write(&webgpu_requirements, "--no-index\n-r requirements.txt\n").unwrap();
    std::fs::write(
        deploy_dir.join("l4filter.py"),
        r#"import os
from pathlib import Path

class Left4Filter:
    def __init__(self, deploy_dir, num_threads=4):
        gpu = os.getenv("LEFT4FILTERS_VISION_PROVIDER", "").strip().lower() == "webgpu"
        if gpu and (Path(deploy_dir).parent.parent / "mock-no-webgpu").exists():
            raise RuntimeError("mock WebGPU adapter unavailable")
        self.vision_provider = "WebGpuExecutionProvider" if gpu else "CPUExecutionProvider"
"#,
    )
    .unwrap();
    run_git(&upstream_repo, &["add", "deploy"]);
    run_git(&upstream_repo, &["commit", "-m", "Add WebGPU support"]);

    write_offline_runner_shims(&engine.paths().filters_dir);
    engine.start_local_filter_service().await.unwrap();

    #[cfg(unix)]
    let stale_venv = {
        use std::os::unix::fs::PermissionsExt;

        let venv = &engine.paths().filter_webgpu_venv_dir;
        let python = venv.join("bin/python");
        std::fs::create_dir_all(python.parent().unwrap()).unwrap();
        std::fs::write(
            &python,
            "#!/bin/sh\n[ \"$1\" = '-m' ] && [ \"$2\" = 'pip' ] && exit 0\nexit 2\n",
        )
        .unwrap();
        std::fs::set_permissions(python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let stale_venv = venv.join("old-python-3.10");
        std::fs::write(&stale_venv, "").unwrap();
        stale_venv
    };

    let adapter_blocker = engine.paths().filters_dir.join("mock-no-webgpu");
    std::fs::write(&adapter_blocker, "").unwrap();
    let start_webgpu = DeployLocalFilterRequest {
        start_after_deploy: true,
        ..webgpu_request
    };
    engine
        .deploy_local_filter(start_webgpu.clone())
        .await
        .unwrap_err();

    #[cfg(unix)]
    assert!(!stale_venv.exists());
    let recovered = engine.get_local_filter_status().await.unwrap();
    assert!(recovered.deployed);
    assert!(!recovered.web_gpu_enabled);
    assert_eq!(recovered.current_commit, cpu.current_commit);
    assert_eq!(recovered.service_state, LocalFilterServiceState::Running);
    assert!(recovered.last_error.is_some());
    assert!(!engine.paths().filters_dir.join("webgpu-enabled").exists());

    std::fs::remove_file(&adapter_blocker).unwrap();
    let webgpu = engine.deploy_local_filter(start_webgpu).await.unwrap();
    assert!(webgpu.deployed && webgpu.web_gpu_enabled);
    assert_eq!(webgpu.service_state, LocalFilterServiceState::Running);
    assert_ne!(webgpu.venv_dir, cpu.venv_dir);
    assert!(engine.paths().filter_venv_dir.is_dir());
    install_offline_runner_shims(&engine.paths().filter_webgpu_venv_dir);
    engine.shutdown().await;
    drop(engine);

    let engine = make_engine(dir.path(), config, Arc::new(MockSteamBridge::new(false)));
    let started = engine.start_local_filter_service().await.unwrap();
    assert!(started.web_gpu_enabled);
    assert_eq!(started.service_state, LocalFilterServiceState::Running);

    std::fs::write(&webgpu_requirements, "--no-index\nonnxruntime-ep-webgpu\n").unwrap();
    run_git(&upstream_repo, &["add", "deploy"]);
    run_git(
        &upstream_repo,
        &["commit", "-m", "Require an unavailable WebGPU package"],
    );
    let failure = engine
        .deploy_local_filter(request.clone())
        .await
        .unwrap_err();
    assert!(failure.to_string().contains("WebGPU Python"));
    let status = engine.get_local_filter_status().await.unwrap();
    assert!(status.deployed && status.web_gpu_enabled);
    assert_eq!(status.current_commit, webgpu.current_commit);

    std::fs::write(adapter_blocker, "").unwrap();
    let cpu_again = engine
        .deploy_local_filter(DeployLocalFilterRequest {
            install_web_gpu: Some(false),
            start_after_deploy: true,
            ..request
        })
        .await
        .unwrap();
    assert!(cpu_again.deployed);
    assert!(!cpu_again.web_gpu_enabled);
    assert_eq!(cpu_again.service_state, LocalFilterServiceState::Running);
    assert_eq!(cpu_again.venv_dir, cpu.venv_dir);
    engine.shutdown().await;
}

#[derive(Default)]
struct MockFilterState {
    predict_calls: AtomicUsize,
    llm_calls: AtomicUsize,
    last_auth: Mutex<Option<String>>,
    last_llm_body: Mutex<Option<Value>>,
    img1_bytes: Vec<u8>,
    img2_bytes: Vec<u8>,
}

#[tokio::test]
async fn two_stage_evaluator_openai_judge_and_modality_subdivision_end_to_end() {
    let img1 = make_tiny_jpeg(20, 180, 40); // safe green image
    let img2 = make_tiny_jpeg(240, 20, 180); // sensitive image
    let img2_b64 = STANDARD.encode(&img2);

    let mock_state = Arc::new(MockFilterState {
        img1_bytes: img1.clone(),
        img2_bytes: img2.clone(),
        ..Default::default()
    });

    let state_for_predict = mock_state.clone();
    let img2_b64_pred = img2_b64.clone();
    let predict_handler = move |AxumState(st): AxumState<Arc<MockFilterState>>,
                                Json(req): Json<FilterPredictRequest>| async move {
        st.predict_calls.fetch_add(1, Ordering::SeqCst);
        let title = req.title.unwrap_or_default();
        let desc = req.description.unwrap_or_default();

        let probs = if title.contains("[LOW]") {
            FilterProbabilities {
                prob: 0.05,
                gore_prob: 0.01,
                nsfw_prob: 0.04,
                text_prob: Some(0.03),
                title_prob: Some(0.02),
                description_prob: Some(0.02),
                image_prob: Some(0.04),
                per_image_probs: vec![0.04],
                ..Default::default()
            }
        } else if title.contains("[HIGH_IMG_ONLY]") {
            FilterProbabilities {
                prob: 0.89,
                gore_prob: 0.02,
                nsfw_prob: 0.88,
                text_prob: Some(0.03),
                title_prob: Some(0.02),
                description_prob: Some(0.02),
                image_prob: Some(0.89),
                image_gore_prob: Some(0.02),
                image_nsfw_prob: Some(0.88),
                per_image_probs: vec![0.89],
                ..Default::default()
            }
        } else if title.contains("[HIGH_TXT_ONLY]") {
            FilterProbabilities {
                prob: 0.91,
                gore_prob: 0.01,
                nsfw_prob: 0.90,
                text_prob: Some(0.91),
                text_gore_prob: Some(0.01),
                text_nsfw_prob: Some(0.90),
                title_prob: Some(0.88),
                description_prob: Some(0.04),
                image_prob: Some(0.02),
                per_image_probs: vec![0.02],
                ..Default::default()
            }
        } else if title.contains("[MULTI_IMG]") {
            let per_img: Vec<f32> = if !req.image_urls.is_empty() {
                assert!(
                    req.images_base64.is_empty(),
                    "Catalog mode should pass remote URLs via image_urls without base64 upload"
                );
                req.image_urls
                    .iter()
                    .map(|u| if u.contains("nsfw.jpg") { 0.92 } else { 0.03 })
                    .collect()
            } else {
                req.images_base64
                    .iter()
                    .map(|b64| if b64 == &img2_b64_pred { 0.92 } else { 0.03 })
                    .collect()
            };
            FilterProbabilities {
                prob: 0.92,
                gore_prob: 0.02,
                nsfw_prob: 0.91,
                text_prob: Some(0.02),
                title_prob: Some(0.02),
                description_prob: Some(0.01),
                image_prob: Some(0.92),
                per_image_probs: per_img,
                ..Default::default()
            }
        } else if title.contains("[MID_LLM_TRUE]") || title.contains("[MID_LLM_FALSE]") {
            let _ = desc;
            FilterProbabilities {
                prob: 0.42,
                gore_prob: 0.05,
                nsfw_prob: 0.41,
                text_prob: Some(0.10),
                title_prob: Some(0.08),
                description_prob: Some(0.09),
                image_prob: Some(0.45),
                per_image_probs: vec![0.45],
                ..Default::default()
            }
        } else {
            FilterProbabilities::default()
        };

        Json(probs)
    };

    let llm_handler = move |AxumState(st): AxumState<Arc<MockFilterState>>,
                            headers: HeaderMap,
                            Json(body): Json<Value>| async move {
        st.llm_calls.fetch_add(1, Ordering::SeqCst);
        *st.last_auth.lock().unwrap() = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let serialized = body.to_string();
        *st.last_llm_body.lock().unwrap() = Some(body);

        let verdict_str = if serialized.contains("[MID_LLM_TRUE]") {
            "true"
        } else {
            "false"
        };

        Json(json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": verdict_str
                },
                "finish_reason": "stop"
            }]
        }))
    };

    let img1_route_state = mock_state.clone();
    let img2_route_state = mock_state.clone();

    let app = Router::new()
        .route("/api/v1/filter/predict", post(predict_handler))
        .route("/v1/chat/completions", post(llm_handler))
        .route(
            "/img/safe.jpg",
            get(move || {
                let bytes = img1_route_state.img1_bytes.clone();
                async move {
                    (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "image/jpeg")],
                        bytes,
                    )
                        .into_response()
                }
            }),
        )
        .route(
            "/img/nsfw.jpg",
            get(move || {
                let bytes = img2_route_state.img2_bytes.clone();
                async move {
                    (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "image/jpeg")],
                        bytes,
                    )
                        .into_response()
                }
            }),
        )
        .with_state(state_for_predict);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();
    let custom_sys_prompt = "Custom strict L4D2 moderation policy v99.";

    let cfg = AppConfig {
        loading_dir: loading_dir.to_str().unwrap().into(),
        https_only: false,
        catalog_server_url: Some(base_url.clone()),
        filter: ClientFilterConfig {
            enabled: true,
            mode: ClientFilterSourceMode::Catalog,
            enable_direct_flag: true,
            flag_threshold: 0.72,
            llm_enabled: true,
            llm_threshold: 0.20,
            llm_endpoint: format!("{base_url}/v1"),
            llm_model: "gpt-4o-mini-test".into(),
            llm_send_images: true,
            llm_system_prompt: custom_sys_prompt.into(),
            llm_token_source: LlmTokenSource::EncryptedLocal,
            subdivide_modalities: true,
            modality_threshold: 0.30,
            ..Default::default()
        },
        ..Default::default()
    };

    let events = Arc::new(RecordedEvents::default());
    let engine = make_engine_with_events(
        dir.path(),
        cfg,
        Arc::new(MockSteamBridge::new(false)),
        events.clone(),
    );
    engine.set_allow_loopback_image_urls_for_testing(true);

    // Isolate TokenVault inside tempdir so we never touch user's real ~/.config
    let isolated_vault = TokenVault::with_custom_identity(
        &engine.paths().llm_token_path,
        dir.path().join("isolated-os-vault").join("vault.key"),
        "test-machine-id",
        "uid:4242",
    );
    engine.set_token_vault(isolated_vault);
    let tok_status = engine
        .set_filter_llm_token(Some("sk-vault-test-token-777".into()))
        .unwrap();
    assert!(tok_status.has_encrypted_token);

    let safe_img_url = format!("{base_url}/img/safe.jpg");
    let nsfw_img_url = format!("{base_url}/img/nsfw.jpg");

    // 1. Low score < llm_threshold -> ModelSafe, LLM not called
    let low_verdicts = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:1001".into(),
                addon_id: None,
                workshop_id: None,
                title: "[LOW] Normal Campaign Map".into(),
                description: Some("A regular 4-chapter co-op campaign".into()),
                tags: vec!["Campaign".into()],
                image_sources: vec![safe_img_url.clone()],
                revision_hint: None,
                bypass_cache: false,
            }])
            .await
            .unwrap(),
    );
    assert_eq!(low_verdicts.len(), 1);
    assert!(!low_verdicts[0].flagged);
    assert_eq!(
        low_verdicts[0].decision_source,
        FilterDecisionSource::ModelSafe
    );
    assert_eq!(mock_state.llm_calls.load(Ordering::SeqCst), 0);

    // 2. High score >= flag_threshold -> ModelDirectFlag, LLM not called + Image-only subdivision
    let high_img_verdicts = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:1002".into(),
                addon_id: None,
                workshop_id: None,
                title: "[HIGH_IMG_ONLY] innocuous skin name".into(),
                description: Some("clean text description".into()),
                tags: vec!["Survivors".into()],
                image_sources: vec![nsfw_img_url.clone()],
                revision_hint: None,
                bypass_cache: false,
            }])
            .await
            .unwrap(),
    );
    assert!(high_img_verdicts[0].flagged);
    assert_eq!(
        high_img_verdicts[0].decision_source,
        FilterDecisionSource::ModelDirectFlag
    );
    assert!(high_img_verdicts[0].image_sensitive);
    assert!(!high_img_verdicts[0].text_sensitive);
    assert_eq!(high_img_verdicts[0].evaluated_image_indices, vec![0]);
    assert_eq!(high_img_verdicts[0].sensitive_image_indices, vec![0]);
    assert_eq!(mock_state.llm_calls.load(Ordering::SeqCst), 0);

    // 3. Mid score [llm_threshold, flag_threshold) -> escalates to OpenAI Chat Completions
    let mid_true = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:1003".into(),
                addon_id: None,
                workshop_id: None,
                title: "[MID_LLM_TRUE] Borderline Survivor Mod".into(),
                description: Some("Check preview image carefully".into()),
                tags: vec!["Survivors".into(), "Anime".into()],
                image_sources: vec![nsfw_img_url.clone()],
                revision_hint: None,
                bypass_cache: false,
            }])
            .await
            .unwrap(),
    );
    assert!(mid_true[0].flagged);
    assert_eq!(
        mid_true[0].decision_source,
        FilterDecisionSource::LlmFlaggedTrue
    );
    assert!(mid_true[0].image_sensitive);
    assert!(!mid_true[0].text_sensitive);
    assert_eq!(mock_state.llm_calls.load(Ordering::SeqCst), 1);

    assert_eq!(
        mock_state.last_auth.lock().unwrap().as_deref(),
        Some("Bearer sk-vault-test-token-777")
    );
    let captured_llm = mock_state.last_llm_body.lock().unwrap().clone().unwrap();
    assert_eq!(captured_llm["model"], "gpt-4o-mini-test");
    let msgs = captured_llm["messages"].as_array().unwrap();
    assert_eq!(msgs[0]["role"], "system");
    assert_eq!(msgs[0]["content"], custom_sys_prompt);
    assert_eq!(msgs[1]["role"], "user");
    let content_parts = msgs[1]["content"].as_array().unwrap();
    let user_text = content_parts[0]["text"].as_str().unwrap();
    assert!(user_text.contains("<untrusted_addon_content>"));
    assert!(user_text.contains("[MID_LLM_TRUE] Borderline Survivor Mod"));
    assert!(user_text.contains(
        "Return ONLY a JSON boolean literal true (if sensitive/unsafe) or false (if safe)."
    ));
    let sent_img_url = content_parts[1]["image_url"]["url"].as_str().unwrap();
    assert!(sent_img_url.starts_with("data:image/jpeg;base64,"));
    assert!(sent_img_url.ends_with(&img2_b64));

    // Mid score returning false -> LlmFlaggedFalse
    let mid_false = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:1004".into(),
                addon_id: None,
                workshop_id: None,
                title: "[MID_LLM_FALSE] Red HUD Font".into(),
                description: Some("Just red damage numbers".into()),
                tags: vec!["UI".into()],
                image_sources: vec![safe_img_url.clone()],
                revision_hint: None,
                bypass_cache: false,
            }])
            .await
            .unwrap(),
    );
    assert!(!mid_false[0].flagged);
    assert_eq!(
        mid_false[0].decision_source,
        FilterDecisionSource::LlmFlaggedFalse
    );
    assert!(!mid_false[0].image_sensitive);
    assert!(!mid_false[0].text_sensitive);
    assert_eq!(mock_state.llm_calls.load(Ordering::SeqCst), 2);

    // 4. Modality subdivision: text-only sensitive & multi-image specific index
    let sub_verdicts = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![
                FilterEvaluationTarget {
                    target_key: "ws:1005".into(),
                    addon_id: None,
                    workshop_id: None,
                    title: "[HIGH_TXT_ONLY] Explicit R18 Title".into(),
                    description: Some("Innocent body text".into()),
                    tags: vec!["Skin".into()],
                    image_sources: vec![safe_img_url.clone()],
                    revision_hint: None,
                    bypass_cache: false,
                },
                FilterEvaluationTarget {
                    target_key: "ws:1006".into(),
                    addon_id: None,
                    workshop_id: None,
                    title: "[MULTI_IMG] Pack With Mixed Screenshots".into(),
                    description: Some("Pack of textures".into()),
                    tags: vec!["Skin".into()],
                    image_sources: vec![safe_img_url.clone(), nsfw_img_url.clone()],
                    revision_hint: None,
                    bypass_cache: false,
                },
            ])
            .await
            .unwrap(),
    );

    // Text-only item
    assert!(sub_verdicts[0].flagged);
    assert!(sub_verdicts[0].text_sensitive);
    assert!(sub_verdicts[0].title_sensitive);
    assert!(!sub_verdicts[0].description_sensitive);
    assert!(!sub_verdicts[0].image_sensitive);
    assert!(sub_verdicts[0].sensitive_image_indices.is_empty());

    // Multi-image item: ONLY index 1 is sensitive!
    assert!(sub_verdicts[1].flagged);
    assert!(!sub_verdicts[1].text_sensitive);
    assert!(sub_verdicts[1].image_sensitive);
    assert_eq!(sub_verdicts[1].evaluated_image_indices, vec![0, 1]);
    assert_eq!(sub_verdicts[1].sensitive_image_indices, vec![1]);

    // 5. SQLite cache hit verification & clear_filter_cache
    let calls_before_cached = mock_state.predict_calls.load(Ordering::SeqCst);
    let cached_again = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:1006".into(),
                addon_id: None,
                workshop_id: None,
                title: "[MULTI_IMG] Pack With Mixed Screenshots".into(),
                description: Some("Pack of textures".into()),
                tags: vec!["Skin".into()],
                image_sources: vec![safe_img_url, nsfw_img_url],
                revision_hint: None,
                bypass_cache: false,
            }])
            .await
            .unwrap(),
    );
    assert_eq!(cached_again[0].sensitive_image_indices, vec![1]);
    assert_eq!(
        mock_state.predict_calls.load(Ordering::SeqCst),
        calls_before_cached
    );

    let status = engine.set_filter_llm_token(None).unwrap();
    assert!(!status.has_encrypted_token);
    assert!(status.cache_generation > tok_status.cache_generation);
    let after_token_clear = engine.snapshot().unwrap();
    assert_eq!(
        after_token_clear.filter_cache_generation,
        status.cache_generation
    );
    assert_eq!(after_token_clear.cached_filter_verdicts.len(), 4);
    assert!(after_token_clear.cached_filter_verdicts.iter().all(|v| {
        !matches!(
            v.decision_source,
            FilterDecisionSource::LlmFlaggedTrue | FilterDecisionSource::LlmFlaggedFalse
        )
    }));
    assert_eq!(engine.clear_filter_cache().unwrap().count, 4);
}

#[test]
fn library_exclusions_and_vpk_fingerprint_invalidation() {
    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();

    let vpk_path = loading_dir.join("123456.vpk");
    create_test_vpk(
        &vpk_path,
        "Trusted Survivor Skin",
        "Mapper",
        false,
        None,
        b"v1-original-payload",
    );

    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );

    let snap1 = engine.scan().unwrap();
    assert_eq!(snap1.addons.len(), 1);
    let addon = &snap1.addons[0];
    let hash_v1 = addon.content_hash.clone().expect("installed VPK has hash");

    let snap_with_excl = engine
        .upsert_filter_exclusion(UpsertFilterExclusionRequest {
            addon_id: addon.id.clone(),
            display_level: ExclusionDisplayLevel::BlurImage,
            bind_fingerprint: Some(true),
            note: Some("Safe skin verified manually".into()),
        })
        .unwrap();

    assert_eq!(snap_with_excl.filter_exclusions.len(), 1);
    let excl = &snap_with_excl.filter_exclusions[0];
    assert_eq!(excl.addon_id, addon.id);
    assert_eq!(excl.display_level, ExclusionDisplayLevel::BlurImage);
    assert!(excl.bind_fingerprint);
    assert_eq!(excl.bound_fingerprint.as_deref(), Some(hash_v1.as_str()));
    assert_eq!(excl.current_fingerprint.as_deref(), Some(hash_v1.as_str()));
    assert!(excl.fingerprint_valid);

    // Replace VPK on disk with modified content (different size & hash) and rescan
    create_test_vpk(
        &vpk_path,
        "Trusted Survivor Skin",
        "Mapper",
        false,
        None,
        b"v2-modified-payload-with-different-bytes",
    );

    let snap_after_modify = engine.scan().unwrap();
    assert_eq!(snap_after_modify.filter_exclusions.len(), 1);
    let invalidated = &snap_after_modify.filter_exclusions[0];
    assert_eq!(
        invalidated.bound_fingerprint.as_deref(),
        Some(hash_v1.as_str())
    );
    assert_ne!(
        invalidated.current_fingerprint,
        invalidated.bound_fingerprint
    );
    assert!(
        !invalidated.fingerprint_valid,
        "modifying VPK on disk must invalidate fingerprint-bound exclusion"
    );

    let display_edit = engine
        .upsert_filter_exclusion(UpsertFilterExclusionRequest {
            addon_id: invalidated.addon_id.clone(),
            display_level: ExclusionDisplayLevel::HideText,
            bind_fingerprint: None,
            note: invalidated.note.clone(),
        })
        .unwrap();
    let edited = &display_edit.filter_exclusions[0];
    assert_eq!(edited.display_level, ExclusionDisplayLevel::HideText);
    assert!(edited.bind_fingerprint);
    assert!(!edited.fingerprint_valid);
    assert_eq!(edited.bound_fingerprint, invalidated.bound_fingerprint);

    let snap_deleted = engine.delete_filter_exclusion(&addon.id).unwrap();
    assert!(snap_deleted.filter_exclusions.is_empty());
}

#[tokio::test]
async fn custom_endpoint_raw_predict_fallback_and_unimodal_followup() {
    let img_safe = make_tiny_jpeg(10, 200, 10);
    let img_nsfw = make_tiny_jpeg(220, 10, 200);
    let img_nsfw_b64 = STANDARD.encode(&img_nsfw);
    let img_safe_data_url = format!("data:image/jpeg;base64,{}", STANDARD.encode(&img_safe));
    let img_nsfw_data_url = format!("data:image/jpeg;base64,{img_nsfw_b64}");

    let raw_calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = raw_calls.clone();
    // Raw Left4Filters /predict server: does NOT expose /api/v1/filter/predict and returns ONLY {prob, gore_prob, nsfw_prob}
    let app = Router::new().route(
        "/predict",
        post(move |Json(req): Json<RawFilterPredictRequest>| {
            let calls_ref = calls_ref.clone();
            let img_nsfw_b64 = img_nsfw_b64.clone();
            async move {
                calls_ref.fetch_add(1, Ordering::SeqCst);
                let has_nsfw_img = req.images_base64.iter().any(|b| b == &img_nsfw_b64);
                let p = if has_nsfw_img { 0.87 } else { 0.03 };
                Json(json!({
                    "prob": p,
                    "gore_prob": 0.01,
                    "nsfw_prob": p,
                }))
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();

    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: true, // Loopback custom endpoint must still work with https_only=true
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::CustomEndpoint,
                custom_endpoint_url: Some(format!("http://{addr}")),
                subdivide_modalities: true,
                modality_threshold: 0.30,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );

    let verdicts = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:9001".into(),
                addon_id: None,
                workshop_id: None,
                title: "Clean Survivor Title".into(),
                description: Some("Clean description".into()),
                tags: vec!["Survivors".into()],
                image_sources: vec![img_safe_data_url, img_nsfw_data_url],
                revision_hint: None,
                bypass_cache: false,
            }])
            .await
            .unwrap(),
    );

    assert_eq!(verdicts.len(), 1);
    assert!(verdicts[0].flagged);
    assert!(!verdicts[0].text_sensitive);
    assert!(verdicts[0].image_sensitive);
    assert_eq!(verdicts[0].evaluated_image_indices, vec![0, 1]);
    assert_eq!(verdicts[0].sensitive_image_indices, vec![1]);
    // 1 joint call + follow-up text/title/desc + 2 per-image calls >= 4
    assert!(raw_calls.load(Ordering::SeqCst) >= 4);
}

#[tokio::test]
async fn local_python_runner_daemon_lifecycle_and_live_reconfigure() {
    let dir = tempdir().unwrap();
    let upstream_repo = dir.path().join("upstream-runner-repo");
    std::fs::create_dir_all(&upstream_repo).unwrap();

    run_git(&upstream_repo, &["init", "-b", "main"]);
    write_mock_left4filters_deploy(&upstream_repo, "1.2.0", 0.15);
    run_git(&upstream_repo, &["add", "."]);
    run_git(&upstream_repo, &["commit", "-m", "feat: runner test repo"]);

    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: false,
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::Local,
                repo_url: upstream_repo.to_str().unwrap().into(),
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );

    engine
        .deploy_local_filter(DeployLocalFilterRequest {
            repo_url: Some(upstream_repo.to_str().unwrap().into()),
            track_latest: true,
            target_commit: None,
            start_after_deploy: false,
            install_web_gpu: None,
        })
        .await
        .unwrap();

    install_offline_runner_shims(&engine.paths().filter_venv_dir);

    let started = engine.start_local_filter_service().await.unwrap();
    assert_eq!(started.service_state, LocalFilterServiceState::Running);
    assert!(started.running_pid.is_some());
    assert!(started
        .service_endpoint
        .as_deref()
        .is_some_and(|u| u.starts_with("http://127.0.0.1:")));

    // Evaluate against the live Python runner
    let verdicts = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![
                FilterEvaluationTarget {
                    target_key: "ws:5001".into(),
                    addon_id: None,
                    workshop_id: None,
                    title: "Safe Weapon Skin".into(),
                    description: Some("Standard rifle".into()),
                    tags: vec!["Weapons".into()],
                    image_sources: vec![],
                    revision_hint: None,
                    bypass_cache: false,
                },
                FilterEvaluationTarget {
                    target_key: "ws:5002".into(),
                    addon_id: None,
                    workshop_id: None,
                    title: "Explicit NSFW Skin".into(),
                    description: Some("Adult mod".into()),
                    tags: vec!["Survivors".into()],
                    image_sources: vec![],
                    revision_hint: None,
                    bypass_cache: false,
                },
            ])
            .await
            .unwrap(),
    );
    assert!(!verdicts[0].flagged);
    assert!(verdicts[1].flagged);
    assert_eq!(
        verdicts[1].decision_source,
        FilterDecisionSource::ModelDirectFlag
    );

    // Live reconfigure via save_config (NsfwOnly -> GoreOnly zeroes out nsfw_prob!)
    let mut next_cfg = engine.config();
    next_cfg.filter.local_params.nsfw_weight = 0.0;
    next_cfg.filter.local_params.gore_weight = 1.0;
    engine.save_config(next_cfg).unwrap();

    let reevaluated = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:5002".into(),
                addon_id: None,
                workshop_id: None,
                title: "Explicit NSFW Skin".into(),
                description: Some("Adult mod".into()),
                tags: vec!["Survivors".into()],
                image_sources: vec![],
                revision_hint: None,
                bypass_cache: false, // policy_sig changed automatically, so cache invalidates!
            }])
            .await
            .unwrap(),
    );
    assert!(
        !reevaluated[0].flagged,
        "after live /configure with nsfw_weight=0.0, pure NSFW item must drop to safe"
    );

    let stopped = engine.stop_local_filter_service().await.unwrap();
    assert_eq!(stopped.service_state, LocalFilterServiceState::Stopped);
    assert!(stopped.running_pid.is_none());
    engine.shutdown().await;
}

#[tokio::test]
async fn llm_judge_safe_json_think_tags_content_filter_and_direct_flag_disabled() {
    let last_llm_req: Arc<Mutex<Option<(String, Value)>>> = Arc::new(Mutex::new(None));
    let capture_ref = last_llm_req.clone();

    let app = Router::new()
        .route(
            "/api/v1/filter/predict",
            post(|Json(req): Json<FilterPredictRequest>| async move {
                let title = req.title.unwrap_or_default();
                if title.contains("[CORPSE_REVIEW]") {
                    assert_eq!(req.images_base64.len(), 2);
                    return Json(FilterProbabilities {
                        prob: 0.20,
                        fusion_prob: Some(0.08),
                        corpse_prob: Some(0.43),
                        corpse_review_floor: Some(0.20),
                        corpse_image_indices: vec![1],
                        text_prob: Some(0.40),
                        per_image_probs: vec![0.01, 0.08],
                        ..Default::default()
                    });
                }
                let prob = if title.contains("[DIRECT_OFF]") {
                    0.96
                } else {
                    0.45
                };
                Json(FilterProbabilities {
                    prob,
                    gore_prob: 0.02,
                    nsfw_prob: prob,
                    text_prob: Some(prob),
                    title_prob: Some(prob),
                    ..Default::default()
                })
            }),
        )
        .route(
            "/openai/deployments/o3/chat/completions",
            post(
                move |uri: axum::http::Uri, Json(body): Json<Value>| {
                    let capture_ref = capture_ref.clone();
                    async move {
                        *capture_ref.lock().unwrap() = Some((uri.to_string(), body.clone()));
                        let serialized = body.to_string();
                        if serialized.contains("[SAFE_TRUE]") {
                            Json(json!({
                                "choices": [{
                                    "index": 0,
                                    "finish_reason": "stop",
                                    "message": { "role": "assistant", "content": r#"{"safe": true}"# }
                                }]
                            }))
                        } else if serialized.contains("[SAFE_FALSE]") {
                            Json(json!({
                                "choices": [{
                                    "index": 0,
                                    "finish_reason": "stop",
                                    "message": { "role": "assistant", "content": r#"{"safe": false}"# }
                                }]
                            }))
                        } else if serialized.contains("[THINK_TRUE]") {
                            Json(json!({
                                "choices": [{
                                    "index": 0,
                                    "finish_reason": "stop",
                                    "message": {
                                        "role": "assistant",
                                        "content": "<think>\nChecking survivor outfit...\nExplicit content.\n</think>\ntrue"
                                    }
                                }]
                            }))
                        } else {
                            Json(json!({
                                "choices": [{
                                    "index": 0,
                                    "finish_reason": "content_filter",
                                    "message": { "role": "assistant", "content": null }
                                }]
                            }))
                        }
                    }
                },
            ),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();

    // 1. enable_direct_flag = false & llm_enabled = false -> must return ModelSafe even at prob=0.96
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: false,
            catalog_server_url: Some(base_url.clone()),
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::Catalog,
                enable_direct_flag: false,
                flag_threshold: 0.72,
                llm_enabled: false,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );

    let direct_off = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:2001".into(),
                addon_id: None,
                workshop_id: None,
                title: "[DIRECT_OFF] High Score Mod".into(),
                description: None,
                tags: vec![],
                image_sources: vec![],
                revision_hint: None,
                bypass_cache: false,
            }])
            .await
            .unwrap(),
    );
    assert!(!direct_off[0].flagged);
    assert_eq!(
        direct_off[0].decision_source,
        FilterDecisionSource::ModelSafe
    );

    // 2. Enable LLM with reasoning model `o3-mini` and Azure-style query parameter
    let vault = TokenVault::with_custom_identity(
        &engine.paths().llm_token_path,
        dir.path().join("vault-dir").join("vault.key"),
        "machine-llm-reg",
        "uid:1000",
    );
    engine.set_token_vault(vault);
    engine
        .set_filter_llm_token(Some("sk-reg-test".into()))
        .unwrap();

    let mut llm_cfg = engine.config();
    llm_cfg.filter.enable_direct_flag = true;
    llm_cfg.filter.llm_enabled = true;
    llm_cfg.filter.llm_threshold = 0.20;
    llm_cfg.filter.flag_threshold = 0.80;
    llm_cfg.filter.llm_endpoint =
        format!("{base_url}/openai/deployments/o3?api-version=2024-10-21");
    llm_cfg.filter.llm_model = "o3-mini".into();
    llm_cfg.filter.llm_token_source = LlmTokenSource::EncryptedLocal;
    engine.save_config_async(llm_cfg).await.unwrap();

    let llm_verdicts = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![
                FilterEvaluationTarget {
                    target_key: "ws:2002".into(),
                    addon_id: None,
                    workshop_id: None,
                    title: "[SAFE_TRUE] Clean Mod".into(),
                    description: None,
                    tags: vec![],
                    image_sources: vec![],
                    revision_hint: None,
                    bypass_cache: false,
                },
                FilterEvaluationTarget {
                    target_key: "ws:2003".into(),
                    addon_id: None,
                    workshop_id: None,
                    title: "[SAFE_FALSE] Unsafe Mod".into(),
                    description: None,
                    tags: vec![],
                    image_sources: vec![],
                    revision_hint: None,
                    bypass_cache: false,
                },
                FilterEvaluationTarget {
                    target_key: "ws:2004".into(),
                    addon_id: None,
                    workshop_id: None,
                    title: "[THINK_TRUE] Reasoning Mod".into(),
                    description: None,
                    tags: vec![],
                    image_sources: vec![],
                    revision_hint: None,
                    bypass_cache: false,
                },
                FilterEvaluationTarget {
                    target_key: "ws:2005".into(),
                    addon_id: None,
                    workshop_id: None,
                    title: "[CONTENT_FILTER] Provider Blocked Mod".into(),
                    description: None,
                    tags: vec![],
                    image_sources: vec![],
                    revision_hint: None,
                    bypass_cache: false,
                },
            ])
            .await
            .unwrap(),
    );

    assert_eq!(llm_verdicts.len(), 4);
    assert!(
        !llm_verdicts[0].flagged,
        r#"{{"safe": true}} must evaluate to false (not flagged)"#
    );
    assert_eq!(
        llm_verdicts[0].decision_source,
        FilterDecisionSource::LlmFlaggedFalse
    );

    assert!(
        llm_verdicts[1].flagged,
        r#"{{"safe": false}} must evaluate to true (flagged)"#
    );
    assert_eq!(
        llm_verdicts[1].decision_source,
        FilterDecisionSource::LlmFlaggedTrue
    );

    assert!(
        llm_verdicts[2].flagged,
        "<think>...</think> true must evaluate to true (flagged)"
    );
    assert_eq!(
        llm_verdicts[2].decision_source,
        FilterDecisionSource::LlmFlaggedTrue
    );

    assert!(
        llm_verdicts[3].flagged,
        "finish_reason == content_filter must evaluate to true (flagged)"
    );
    assert_eq!(
        llm_verdicts[3].decision_source,
        FilterDecisionSource::LlmFlaggedTrue
    );

    let (captured_uri, captured_body) = last_llm_req.lock().unwrap().clone().unwrap();
    assert_eq!(
        captured_uri,
        "/openai/deployments/o3/chat/completions?api-version=2024-10-21"
    );
    assert!(captured_body.get("temperature").is_none());
    assert_eq!(captured_body["max_completion_tokens"], 512);

    let mut cfg = engine.config();
    cfg.filter.flag_threshold = 0.19;
    engine.save_config_async(cfg).await.unwrap();
    let preview = |r| {
        format!(
            "data:image/jpeg;base64,{}",
            STANDARD.encode(make_tiny_jpeg(r, 20, 20))
        )
    };
    let reviewed = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:2006".into(),
                addon_id: None,
                workshop_id: None,
                title: "[CORPSE_REVIEW] [THINK_TRUE] Gameplay preview".into(),
                description: None,
                tags: vec![],
                image_sources: vec![preview(20), String::new(), preview(30)],
                revision_hint: None,
                bypass_cache: false,
            }])
            .await
            .unwrap(),
    );
    assert_eq!(
        reviewed[0].decision_source,
        FilterDecisionSource::LlmFlaggedTrue
    );
    assert_eq!(reviewed[0].evaluated_score, 0.20);
    assert_eq!(reviewed[0].sensitive_image_indices, vec![2]);
}

#[tokio::test]
async fn batch_partial_failure_dedup_fail_open_cache_guard_and_snapshot_verdicts() {
    let nsfw_jpeg = make_tiny_jpeg(240, 20, 180);
    let nsfw_b64 = STANDARD.encode(&nsfw_jpeg);
    let img_available = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let predict_calls = Arc::new(AtomicUsize::new(0));

    let img_flag_route = img_available.clone();
    let predict_calls_route = predict_calls.clone();
    let nsfw_b64_route = nsfw_b64.clone();

    let app = Router::new()
        .route(
            "/predict",
            post(move |Json(req): Json<FilterPredictRequest>| {
                let predict_calls_route = predict_calls_route.clone();
                let nsfw_b64_route = nsfw_b64_route.clone();
                async move {
                    predict_calls_route.fetch_add(1, Ordering::SeqCst);
                    let title = req.title.unwrap_or_default();
                    if title.contains("[SERVER_BOOM]") {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"error": "simulated backend failure"})),
                        )
                            .into_response();
                    }
                    let has_nsfw_img = req.images_base64.iter().any(|b| b == &nsfw_b64_route);
                    let p = if has_nsfw_img || title.contains("[FLAG_ME]") {
                        0.91
                    } else {
                        0.04
                    };
                    (
                        StatusCode::OK,
                        Json(FilterProbabilities {
                            prob: p,
                            gore_prob: 0.01,
                            nsfw_prob: p,
                            text_prob: Some(if title.contains("[FLAG_ME]") {
                                0.91
                            } else {
                                0.04
                            }),
                            title_prob: Some(if title.contains("[FLAG_ME]") {
                                0.91
                            } else {
                                0.04
                            }),
                            image_prob: has_nsfw_img.then_some(0.91),
                            per_image_probs: if has_nsfw_img { vec![0.91] } else { vec![] },
                            ..Default::default()
                        }),
                    )
                        .into_response()
                }
            }),
        )
        .route(
            "/flaky-nsfw.jpg",
            get(move || {
                let ready = img_flag_route.load(Ordering::SeqCst);
                let bytes = nsfw_jpeg.clone();
                async move {
                    if !ready {
                        (StatusCode::SERVICE_UNAVAILABLE, Vec::<u8>::new()).into_response()
                    } else {
                        (
                            StatusCode::OK,
                            [(header::CONTENT_TYPE, "image/jpeg")],
                            bytes,
                        )
                            .into_response()
                    }
                }
            }),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();

    let events = Arc::new(RecordedEvents::default());
    let engine = make_engine_with_events(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: true,
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::CustomEndpoint,
                custom_endpoint_url: Some(format!("{base_url}/predict")),
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
        events.clone(),
    );
    engine.set_allow_loopback_image_urls_for_testing(true);

    // 1. Batch with duplicate (target_key, content_sig) + 1 failing item + 1 valid flagged item
    let dup_target = FilterEvaluationTarget {
        target_key: "addon:workshop:3001".into(),
        addon_id: None,
        workshop_id: None,
        title: "Clean Map".into(),
        description: None,
        tags: vec![],
        image_sources: vec![],
        revision_hint: None,
        bypass_cache: false,
    };
    let batch_res = engine
        .evaluate_filter_items(vec![
            dup_target.clone(),
            dup_target.clone(),
            FilterEvaluationTarget {
                target_key: "addon:workshop:3002".into(),
                addon_id: None,
                workshop_id: None,
                title: "[SERVER_BOOM] Broken Item".into(),
                description: None,
                tags: vec![],
                image_sources: vec![],
                revision_hint: None,
                bypass_cache: false,
            },
            FilterEvaluationTarget {
                target_key: "addon:workshop:3003".into(),
                addon_id: None,
                workshop_id: None,
                title: "[FLAG_ME] Adult Mod".into(),
                description: None,
                tags: vec![],
                image_sources: vec![],
                revision_hint: None,
                bypass_cache: false,
            },
        ])
        .await
        .unwrap();

    // 4 items returned in input order: 3001 (Completed), 3001 (Completed), 3002 (Failed), 3003 (Completed)
    assert_eq!(batch_res.items.len(), 4);
    assert_eq!(batch_res.items[0].target_key, "addon:workshop:3001");
    assert_eq!(batch_res.items[1].target_key, "addon:workshop:3001");
    assert_eq!(batch_res.items[2].target_key, "addon:workshop:3002");
    assert_eq!(batch_res.items[3].target_key, "addon:workshop:3003");
    assert!(matches!(
        &batch_res.items[0].outcome,
        FilterTargetOutcome::Completed {
            verdict,
            complete: true
        } if !verdict.flagged && verdict.complete
    ));
    assert!(matches!(
        &batch_res.items[2].outcome,
        FilterTargetOutcome::Failed { error } if error.contains("500")
    ));
    assert!(matches!(
        &batch_res.items[3].outcome,
        FilterTargetOutcome::Completed {
            verdict,
            complete: true
        } if verdict.flagged && verdict.complete
    ));
    // Dedup ensured only 3 backend /predict calls (3001 once, 3002 once, 3003 once), NOT 4!
    assert_eq!(predict_calls.load(Ordering::SeqCst), 3);

    // Verify FilterVerdictsUpdated events stream per completed card with canonical policy_signature matching snapshot
    let snap = engine.snapshot().unwrap();
    assert_eq!(snap.filter_policy_signature, batch_res.policy_signature);
    let drained = events.drain();
    let streamed_keys: Vec<String> = drained
        .iter()
        .filter_map(|ev| match ev {
            CoreEvent::FilterVerdictsUpdated {
                verdicts,
                policy_signature: Some(sig),
                cache_generation,
            } if sig == &snap.filter_policy_signature
                && *cache_generation == snap.filter_cache_generation =>
            {
                Some(
                    verdicts
                        .iter()
                        .map(|v| v.target_key.clone())
                        .collect::<Vec<_>>(),
                )
            }
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(streamed_keys.len(), 2);
    assert!(streamed_keys.contains(&"addon:workshop:3001".to_string()));
    assert!(streamed_keys.contains(&"addon:workshop:3003".to_string()));
    assert!(!streamed_keys.contains(&"addon:workshop:3002".to_string()));

    // Verify snapshot.cached_filter_verdicts is populated immediately at startup / snapshot()!
    assert_eq!(snap.cached_filter_verdicts.len(), 2);

    // 2. Fail-open cache poisoning guard: transient image 503 must return `complete: false`, NOT emit final event, and NOT cache `!flagged`!
    let flaky_url = format!("{base_url}/flaky-nsfw.jpg");
    let first_try = engine
        .evaluate_filter_items(vec![FilterEvaluationTarget {
            target_key: "ws:3099".into(),
            addon_id: None,
            workshop_id: None,
            title: "Innocent Title With NSFW Preview".into(),
            description: None,
            tags: vec![],
            image_sources: vec![flaky_url.clone()],
            revision_hint: None,
            bypass_cache: false,
        }])
        .await
        .unwrap();
    assert_eq!(first_try.items.len(), 1);
    match &first_try.items[0].outcome {
        FilterTargetOutcome::Completed { verdict, complete } => {
            assert!(
                !verdict.flagged,
                "when preview returns 503, text-only score is safe"
            );
            assert!(
                !complete,
                "incomplete safe verdict must report complete == false"
            );
            assert!(!verdict.complete);
        }
        other => panic!("expected Completed(complete=false), got {other:?}"),
    }
    let incomplete_events = events.drain();
    assert!(
        !incomplete_events
            .iter()
            .any(|ev| matches!(ev, CoreEvent::FilterVerdictsUpdated { .. })),
        "incomplete safe evaluation must NOT emit final FilterVerdictsUpdated"
    );

    // Now the image server recovers (200 OK). Re-evaluating with `bypass_cache: false` MUST NOT hit a poisoned safe cache!
    img_available.store(true, Ordering::SeqCst);
    let second_try = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:3099".into(),
                addon_id: None,
                workshop_id: None,
                title: "Innocent Title With NSFW Preview".into(),
                description: None,
                tags: vec![],
                image_sources: vec![flaky_url],
                revision_hint: None,
                bypass_cache: false,
            }])
            .await
            .unwrap(),
    );
    assert!(
        second_try[0].flagged && second_try[0].complete,
        "transient image failure must not poison filter_verdicts with false-safe!"
    );

    // 3. Stale policy_sig pruning: changing threshold via save_config prunes old policy rows immediately
    let mut updated_cfg = engine.config();
    updated_cfg.filter.flag_threshold = 0.65;
    engine.save_config_async(updated_cfg).await.unwrap();
    let _ = engine
        .evaluate_filter_items(vec![dup_target])
        .await
        .unwrap();
    // Total rows across entire `filter_verdicts` table must only belong to the new policy_sig (1 row)!
    assert_eq!(engine.clear_filter_cache().unwrap().count, 1);
}

#[tokio::test]
async fn local_filter_crash_auto_recovery_and_unc_or_empty_commit_guards() {
    let dir = tempdir().unwrap();
    let upstream_repo = dir.path().join("upstream-crash-repo");
    std::fs::create_dir_all(&upstream_repo).unwrap();

    run_git(&upstream_repo, &["init", "-b", "main"]);
    write_mock_left4filters_deploy(&upstream_repo, "1.0.0", 0.15);
    run_git(&upstream_repo, &["add", "."]);
    run_git(
        &upstream_repo,
        &["commit", "-m", "feat: crash recovery repo"],
    );

    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: false,
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::Local,
                auto_start_local_service: true,
                repo_url: upstream_repo.to_str().unwrap().into(),
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );

    // Reject `!track_latest` with empty/whitespace `target_commit`
    let bad_deploy = engine
        .deploy_local_filter(DeployLocalFilterRequest {
            repo_url: Some(upstream_repo.to_str().unwrap().into()),
            track_latest: false,
            target_commit: Some("   ".into()),
            start_after_deploy: false,
            install_web_gpu: None,
        })
        .await;
    assert!(bad_deploy.is_err());

    // Reject UNC repo_url in save_config
    let mut bad_cfg = engine.config();
    bad_cfg.filter.repo_url = r"\\attacker.example.com\share\repo".into();
    assert!(engine.save_config(bad_cfg).is_err());

    // Deploy cleanly
    engine
        .deploy_local_filter(DeployLocalFilterRequest {
            repo_url: Some(upstream_repo.to_str().unwrap().into()),
            track_latest: true,
            target_commit: None,
            start_after_deploy: false,
            install_web_gpu: None,
        })
        .await
        .unwrap();

    install_offline_runner_shims(&engine.paths().filter_venv_dir);

    let started = engine.start_local_filter_service().await.unwrap();
    let pid1 = started.running_pid.expect("runner PID present");

    // Kill the Python runner process externally to simulate an unexpected crash
    #[cfg(unix)]
    {
        let status = Command::new("kill")
            .args(["-9", &pid1.to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/PID", &pid1.to_string()])
            .status();
    }
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    // Evaluating a new uncached item must automatically reap dead runner, restart once, and succeed!
    let verdicts = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:7777".into(),
                addon_id: None,
                workshop_id: None,
                title: "Explicit NSFW After Crash".into(),
                description: Some("Recovered evaluation".into()),
                tags: vec![],
                image_sources: vec![],
                revision_hint: None,
                bypass_cache: true,
            }])
            .await
            .unwrap(),
    );
    assert_eq!(verdicts.len(), 1);
    assert!(verdicts[0].flagged);

    let after_recovery = engine.get_local_filter_status().await.unwrap();
    assert_eq!(
        after_recovery.service_state,
        LocalFilterServiceState::Running
    );
    let pid2 = after_recovery.running_pid.expect("restarted runner PID");
    assert_ne!(pid1, pid2, "crash recovery must spawn a new runner PID");

    engine.shutdown().await;
}

#[test]
fn local_non_workshop_vpk_in_place_edit_migrates_unbound_and_bound_exclusions() {
    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();

    let unbound_vpk = loading_dir.join("custom_survivor_alpha.vpk");
    let bound_vpk = loading_dir.join("custom_survivor_beta.vpk");

    create_test_vpk(
        &unbound_vpk,
        "Local Alpha Skin",
        "LocalAuthor",
        false,
        None,
        b"alpha-v1-bytes",
    );
    create_test_vpk(
        &bound_vpk,
        "Local Beta Skin",
        "LocalAuthor",
        false,
        None,
        b"beta-v1-bytes",
    );

    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );

    let snap1 = engine.scan().unwrap();
    assert_eq!(snap1.addons.len(), 2);

    let alpha_v1 = snap1
        .addons
        .iter()
        .find(|a| a.current_file_name.as_deref() == Some("custom_survivor_alpha.vpk"))
        .unwrap()
        .clone();
    let beta_v1 = snap1
        .addons
        .iter()
        .find(|a| a.current_file_name.as_deref() == Some("custom_survivor_beta.vpk"))
        .unwrap()
        .clone();
    assert!(alpha_v1.workshop_id.is_none());
    assert!(alpha_v1.id.as_str().starts_with("hash:"));
    assert!(beta_v1.workshop_id.is_none());
    assert!(beta_v1.id.as_str().starts_with("hash:"));
    let beta_hash_v1 = beta_v1.content_hash.clone().unwrap();

    // Unbound exclusion on alpha_v1
    engine
        .upsert_filter_exclusion(UpsertFilterExclusionRequest {
            addon_id: alpha_v1.id.clone(),
            display_level: ExclusionDisplayLevel::HideImage,
            bind_fingerprint: Some(false),
            note: Some("Unbound local exclusion".into()),
        })
        .unwrap();

    // Fingerprint-bound exclusion on beta_v1
    engine
        .upsert_filter_exclusion(UpsertFilterExclusionRequest {
            addon_id: beta_v1.id.clone(),
            display_level: ExclusionDisplayLevel::BlurImage,
            bind_fingerprint: Some(true),
            note: Some("Bound local exclusion".into()),
        })
        .unwrap();

    // Modify both local VPKs in-place on disk with new payload hashes
    create_test_vpk(
        &unbound_vpk,
        "Local Alpha Skin",
        "LocalAuthor",
        false,
        None,
        b"alpha-v2-modified-payload",
    );
    create_test_vpk(
        &bound_vpk,
        "Local Beta Skin",
        "LocalAuthor",
        false,
        None,
        b"beta-v2-modified-payload",
    );

    let snap2 = engine.scan().unwrap();
    let alpha_v2 = snap2
        .addons
        .iter()
        .find(|a| {
            a.install_state == l4a_protocol::InstallState::Installed
                && a.current_file_name.as_deref() == Some("custom_survivor_alpha.vpk")
        })
        .unwrap();
    let beta_v2 = snap2
        .addons
        .iter()
        .find(|a| {
            a.install_state == l4a_protocol::InstallState::Installed
                && a.current_file_name.as_deref() == Some("custom_survivor_beta.vpk")
        })
        .unwrap();
    assert_ne!(alpha_v1.id, alpha_v2.id);
    assert_ne!(beta_v1.id, beta_v2.id);

    assert_eq!(snap2.filter_exclusions.len(), 2);

    // Unbound exclusion migrated to alpha_v2 and stays valid!
    let excl_alpha = snap2
        .filter_exclusions
        .iter()
        .find(|e| e.addon_id == alpha_v2.id)
        .expect("unbound exclusion must migrate to new hash AddonId");
    assert!(!excl_alpha.bind_fingerprint);
    assert!(excl_alpha.fingerprint_valid);

    // Bound exclusion migrated to beta_v2, preserving bound_fingerprint = beta_hash_v1 and marking fingerprint_valid = false!
    let excl_beta = snap2
        .filter_exclusions
        .iter()
        .find(|e| e.addon_id == beta_v2.id)
        .expect(
            "bound exclusion must migrate to new hash AddonId instead of stranding on dead row",
        );
    assert!(excl_beta.bind_fingerprint);
    assert_eq!(
        excl_beta.bound_fingerprint.as_deref(),
        Some(beta_hash_v1.as_str())
    );
    assert_eq!(excl_beta.current_fingerprint, beta_v2.content_hash);
    assert!(!excl_beta.fingerprint_valid);
}

#[tokio::test]
async fn ssrf_loopback_blocked_by_default_and_preset_params_applied_on_toml_load() {
    // 1. Verify settings.toml with `preset = "strict"` and omitted localParams applies Strict defaults
    let dir = tempdir().unwrap();
    let settings_path = dir.path().join("settings.toml");
    std::fs::write(
        &settings_path,
        r#"
loadingDir = ""
workshopDir = ""

[filter]
enabled = true
preset = "strict"
"#,
    )
    .unwrap();
    let loaded = l4a_core::load_config(&settings_path).unwrap();
    assert_eq!(loaded.filter.preset, l4a_protocol::FilterPresetId::Strict);
    assert_eq!(
        loaded.filter.local_params,
        l4a_protocol::FilterPresetId::Strict.default_params()
    );

    // 2. Verify loopback image URLs are NOT fetched in production mode (default allow_loopback = false)
    let loopback_img_hits = Arc::new(AtomicUsize::new(0));
    let hits_ref = loopback_img_hits.clone();
    let jpeg = make_tiny_jpeg(250, 10, 10);

    let app = Router::new()
        .route(
            "/secret-internal.jpg",
            get(move || {
                let hits_ref = hits_ref.clone();
                let bytes = jpeg.clone();
                async move {
                    hits_ref.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "image/jpeg")],
                        bytes,
                    )
                        .into_response()
                }
            }),
        )
        .route(
            "/predict",
            post(|Json(req): Json<RawFilterPredictRequest>| async move {
                assert!(
                    req.images_base64.is_empty() && req.image_base64.is_none(),
                    "blocked SSRF URL must never be forwarded or uploaded"
                );
                Json(FilterProbabilities {
                    prob: 0.05,
                    gore_prob: 0.01,
                    nsfw_prob: 0.04,
                    ..Default::default()
                })
            }),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: true,
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::CustomEndpoint,
                custom_endpoint_url: Some(format!("{base_url}/predict")),
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );
    // Intentionally do NOT call set_allow_loopback_image_urls_for_testing(true)!
    let _ = engine
        .evaluate_filter_items(vec![FilterEvaluationTarget {
            target_key: "ws:8888".into(),
            addon_id: None,
            workshop_id: None,
            title: "SSRF Probe Addon".into(),
            description: None,
            tags: vec![],
            image_sources: vec![
                format!("{base_url}/secret-internal.jpg"),
                "http://169.254.169.254/latest/meta-data/".into(),
                "https://evil.example.com/track.jpg".into(),
            ],
            revision_hint: None,
            bypass_cache: true,
        }])
        .await
        .unwrap();

    assert_eq!(
        loopback_img_hits.load(Ordering::SeqCst),
        0,
        "production mode must never fetch loopback image URLs without explicit testing flag"
    );
}

#[tokio::test]
async fn raw_predict_snake_case_wire_format_in_local_and_custom_endpoint_modes() {
    let safe_jpeg = make_tiny_jpeg(10, 200, 10);
    let nsfw_jpeg = make_tiny_jpeg(245, 12, 195);
    let nsfw_b64 = STANDARD.encode(&nsfw_jpeg);
    let safe_data_url = format!("data:image/jpeg;base64,{}", STANDARD.encode(&safe_jpeg));
    let nsfw_data_url = format!("data:image/jpeg;base64,{nsfw_b64}");

    // 1. CustomEndpoint mode against a strict stock Left4Filters `/predict` mock that REJECTS
    //    `imagesBase64` / `imageBase64` with HTTP 400 and requires `images_base64`!
    let strict_nsfw_b64 = nsfw_b64.clone();
    let saw_snake_images = Arc::new(AtomicUsize::new(0));
    let saw_ref = saw_snake_images.clone();
    let strict_app = Router::new().route(
        "/predict",
        post(move |Json(raw): Json<Value>| {
            let strict_nsfw_b64 = strict_nsfw_b64.clone();
            let saw_ref = saw_ref.clone();
            async move {
                if raw.get("imagesBase64").is_some()
                    || raw.get("imageBase64").is_some()
                    || raw.get("imageUrls").is_some()
                {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error": "camelCase image keys rejected by raw /predict"})),
                    )
                        .into_response();
                }
                let imgs = raw
                    .get("images_base64")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if !imgs.is_empty() {
                    saw_ref.fetch_add(1, Ordering::SeqCst);
                }
                let is_nsfw = imgs
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|s| s == strict_nsfw_b64);
                let p = if is_nsfw { 0.91 } else { 0.02 };
                (
                    StatusCode::OK,
                    Json(json!({
                        "prob": p,
                        "gore_prob": 0.01,
                        "nsfw_prob": p,
                    })),
                )
                    .into_response()
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, strict_app).await.unwrap();
    });

    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();

    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: false,
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::CustomEndpoint,
                custom_endpoint_url: Some(format!("http://{addr}")),
                subdivide_modalities: true,
                modality_threshold: 0.30,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );

    let custom_verdicts = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:9100".into(),
                addon_id: None,
                workshop_id: None,
                title: "Clean Title".into(),
                description: Some("Clean desc".into()),
                tags: vec!["Skin".into()],
                image_sources: vec![safe_data_url.clone(), nsfw_data_url.clone()],
                revision_hint: Some("rev-custom".into()),
                bypass_cache: false,
            }])
            .await
            .unwrap(),
    );
    assert!(custom_verdicts[0].flagged);
    assert_eq!(custom_verdicts[0].evaluated_image_indices, vec![0, 1]);
    assert_eq!(custom_verdicts[0].sensitive_image_indices, vec![1]);
    // Joint + 2 per-image unimodal calls all carried `images_base64`
    assert!(saw_snake_images.load(Ordering::SeqCst) >= 3);

    // 2. Local mode: replace `filter_runner_path` with a strict Python `/predict` runner that
    //    returns 400 if `imagesBase64` / `imageBase64` is sent, and requires `images_base64`!
    let upstream_repo = dir.path().join("upstream-local-snake");
    std::fs::create_dir_all(&upstream_repo).unwrap();
    run_git(&upstream_repo, &["init", "-b", "main"]);
    let deploy_dir = upstream_repo.join("deploy");
    std::fs::create_dir_all(&deploy_dir).unwrap();
    let py_model = format!(
        r#"import base64
NSFW_BYTES = base64.b64decode("{nsfw_b64}")

class Left4Filter:
    def __init__(self, deploy_dir, num_threads=4):
        self.config = {{"model_name": "MockSnakeModel", "version": "1.0.0"}}

    def predict(self, title="", description="", tags=None, image=None, images=None):
        has_nsfw = False
        if image is not None and image == NSFW_BYTES:
            has_nsfw = True
        if images is not None and any(im == NSFW_BYTES for im in images):
            has_nsfw = True
        p = 0.93 if has_nsfw else 0.02
        return {{"prob": p, "gore_prob": 0.01, "nsfw_prob": p}}
"#
    );
    std::fs::write(deploy_dir.join("l4filter.py"), py_model).unwrap();
    std::fs::write(deploy_dir.join("vision_encoder.onnx"), b"vis").unwrap();
    std::fs::write(deploy_dir.join("fusion_head.onnx"), b"head").unwrap();
    std::fs::write(deploy_dir.join("text_vocab.json"), r#"{"vocab":{}}"#).unwrap();
    std::fs::write(deploy_dir.join("requirements.txt"), "# offline\n").unwrap();
    run_git(&upstream_repo, &["add", "."]);
    run_git(
        &upstream_repo,
        &["commit", "-m", "feat: snake_case image model"],
    );

    let mut local_cfg = engine.config();
    local_cfg.filter.mode = ClientFilterSourceMode::Local;
    local_cfg.filter.repo_url = upstream_repo.to_str().unwrap().into();
    local_cfg.filter.local_params.max_images = 4;
    engine.save_config_async(local_cfg).await.unwrap();

    engine
        .deploy_local_filter(DeployLocalFilterRequest {
            repo_url: Some(upstream_repo.to_str().unwrap().into()),
            track_latest: true,
            target_commit: None,
            start_after_deploy: false,
            install_web_gpu: None,
        })
        .await
        .unwrap();
    install_offline_runner_shims(&engine.paths().filter_venv_dir);

    // Also verify LocalFilterManager sends `images_base64` (never `imagesBase64`) by wrapping
    // a strict HTTP proxy in front or verifying with `filter_runner.py`: first start runner and
    // assert multi-image evaluation via `images_base64` identifies image slot 1!
    engine.start_local_filter_service().await.unwrap();
    let local_verdicts = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:9101".into(),
                addon_id: None,
                workshop_id: None,
                title: "Clean Local Addon".into(),
                description: Some("Clean local desc".into()),
                tags: vec!["Skin".into()],
                image_sources: vec![safe_data_url, nsfw_data_url],
                revision_hint: Some("rev-local".into()),
                bypass_cache: false,
            }])
            .await
            .unwrap(),
    );
    assert!(local_verdicts[0].flagged);
    assert_eq!(local_verdicts[0].evaluated_image_indices, vec![0, 1]);
    assert_eq!(local_verdicts[0].sensitive_image_indices, vec![1]);

    engine.shutdown().await;
}

#[tokio::test]
async fn in_place_addon_vpk_rescan_invalidates_snapshot_cached_filter_verdict() {
    let app = Router::new().route(
        "/predict",
        post(|Json(req): Json<RawFilterPredictRequest>| async move {
            let title = req.title.unwrap_or_default();
            let p = if title.contains("Explicit V2") {
                0.94
            } else {
                0.03
            };
            Json(FilterProbabilities {
                prob: p,
                gore_prob: 0.01,
                nsfw_prob: p,
                text_prob: Some(p),
                title_prob: Some(p),
                ..Default::default()
            })
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();
    let vpk_path = loading_dir.join("777888.vpk");
    create_test_vpk(
        &vpk_path,
        "Safe V1 Survivor",
        "Mapper",
        false,
        None,
        b"v1-safe-bytes",
    );

    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: false,
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::CustomEndpoint,
                custom_endpoint_url: Some(format!("http://{addr}/predict")),
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );

    let snap1 = engine.scan().unwrap();
    assert_eq!(snap1.addons.len(), 1);
    let addon_v1 = &snap1.addons[0];
    assert_eq!(addon_v1.id.as_str(), "ws:777888");

    let target_v1 = l4a_core::filter::build_addon_filter_target(addon_v1);
    let expected_rev_v1 = target_v1.revision_hint.clone();
    let batch1 = engine.evaluate_filter_items(vec![target_v1]).await.unwrap();
    let verdicts1 = unwrap_verdicts(batch1);
    assert!(!verdicts1[0].flagged);
    assert_eq!(verdicts1[0].revision_hint, expected_rev_v1);

    let snap_cached = engine.snapshot().unwrap();
    assert_eq!(snap_cached.cached_filter_verdicts.len(), 1);
    assert_eq!(
        snap_cached.cached_filter_verdicts[0].revision_hint,
        expected_rev_v1
    );

    // Modify 777888.vpk in-place on disk (same AddonId `ws:777888`, new content/title)
    create_test_vpk(
        &vpk_path,
        "Explicit V2 Survivor",
        "Mapper",
        false,
        None,
        b"v2-modified-nsfw-bytes-longer",
    );
    let snap2 = engine.scan().unwrap();
    assert_eq!(snap2.addons.len(), 1);
    assert_eq!(snap2.addons[0].id.as_str(), "ws:777888");
    assert!(
        snap2.cached_filter_verdicts.is_empty(),
        "in-place VPK update must immediately drop stale cached verdict from snapshot"
    );

    let target_v2 = l4a_core::filter::build_addon_filter_target(&snap2.addons[0]);
    let verdicts2 = unwrap_verdicts(engine.evaluate_filter_items(vec![target_v2]).await.unwrap());
    assert!(verdicts2[0].flagged);
    assert_eq!(engine.snapshot().unwrap().cached_filter_verdicts.len(), 1);
}

#[tokio::test]
async fn gallery_slot_mapping_with_404_gap_incomplete_safe_and_failed_outcomes() {
    let safe_jpeg = make_tiny_jpeg(20, 210, 30);
    let nsfw_jpeg = make_tiny_jpeg(250, 15, 200);
    let nsfw_b64 = STANDARD.encode(&nsfw_jpeg);
    let nsfw_b64_pred = nsfw_b64.clone();

    let app = Router::new()
        .route(
            "/predict",
            post(move |Json(req): Json<RawFilterPredictRequest>| {
                let nsfw_b64_pred = nsfw_b64_pred.clone();
                async move {
                    if req.title.as_deref().unwrap_or("").contains("[HARD_FAIL]") {
                        return (
                            StatusCode::BAD_GATEWAY,
                            Json(json!({"error": "upstream down"})),
                        )
                            .into_response();
                    }
                    let per_img: Vec<f32> = req
                        .images_base64
                        .iter()
                        .map(|b| if b == &nsfw_b64_pred { 0.94 } else { 0.02 })
                        .collect();
                    let max_img = per_img.iter().copied().fold(0.0_f32, f32::max);
                    (
                        StatusCode::OK,
                        Json(FilterProbabilities {
                            prob: max_img.max(0.02),
                            gore_prob: 0.01,
                            nsfw_prob: max_img.max(0.02),
                            text_prob: Some(0.02),
                            title_prob: Some(0.02),
                            image_prob: (!per_img.is_empty()).then_some(max_img),
                            per_image_probs: per_img,
                            ..Default::default()
                        }),
                    )
                        .into_response()
                }
            }),
        )
        .route(
            "/img/missing_0.jpg",
            get(|| async { (StatusCode::NOT_FOUND, Vec::<u8>::new()).into_response() }),
        )
        .route(
            "/img/safe_1.jpg",
            get(move || {
                let bytes = safe_jpeg.clone();
                async move {
                    (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "image/jpeg")],
                        bytes,
                    )
                        .into_response()
                }
            }),
        )
        .route(
            "/img/nsfw_2.jpg",
            get(move || {
                let bytes = nsfw_jpeg.clone();
                async move {
                    (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "image/jpeg")],
                        bytes,
                    )
                        .into_response()
                }
            }),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: false,
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::CustomEndpoint,
                custom_endpoint_url: Some(format!("{base_url}/predict")),
                subdivide_modalities: true,
                modality_threshold: 0.30,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );
    engine.set_allow_loopback_image_urls_for_testing(true);

    let url_0_404 = format!("{base_url}/img/missing_0.jpg");
    let url_1_safe = format!("{base_url}/img/safe_1.jpg");
    let url_2_nsfw = format!("{base_url}/img/nsfw_2.jpg");

    let batch = engine
        .evaluate_filter_items(vec![
            // Item A: slot 0 404s, slot 1 safe, slot 2 NSFW -> evaluated=[1, 2], sensitive=[2], complete=true
            FilterEvaluationTarget {
                target_key: "ws:slot-flagged".into(),
                addon_id: None,
                workshop_id: None,
                title: "Gallery With Missing Cover".into(),
                description: None,
                tags: vec![],
                image_sources: vec![url_0_404.clone(), url_1_safe.clone(), url_2_nsfw],
                revision_hint: Some("r1".into()),
                bypass_cache: false,
            },
            // Item B: slot 0 404s, slot 1 safe -> unflagged with load error -> complete=false!
            FilterEvaluationTarget {
                target_key: "ws:slot-incomplete-safe".into(),
                addon_id: None,
                workshop_id: None,
                title: "Safe Item With Missing Gallery Slot".into(),
                description: None,
                tags: vec![],
                image_sources: vec![url_0_404, url_1_safe],
                revision_hint: Some("r1".into()),
                bypass_cache: false,
            },
            // Item C: backend 502 -> FilterTargetOutcome::Failed
            FilterEvaluationTarget {
                target_key: "ws:slot-failed".into(),
                addon_id: None,
                workshop_id: None,
                title: "[HARD_FAIL] Broken Upstream".into(),
                description: None,
                tags: vec![],
                image_sources: vec![],
                revision_hint: Some("r1".into()),
                bypass_cache: false,
            },
        ])
        .await
        .unwrap();

    assert_eq!(batch.items.len(), 3);

    match &batch.items[0].outcome {
        FilterTargetOutcome::Completed { verdict, complete } => {
            assert!(verdict.flagged);
            assert!(!*complete && !verdict.complete);
            assert_eq!(verdict.evaluated_image_indices, vec![1, 2]);
            assert_eq!(verdict.sensitive_image_indices, vec![2]);
        }
        other => panic!("expected Completed for item 0, got {other:?}"),
    }

    match &batch.items[1].outcome {
        FilterTargetOutcome::Completed { verdict, complete } => {
            assert!(!verdict.flagged);
            assert!(!*complete && !verdict.complete);
            assert_eq!(verdict.evaluated_image_indices, vec![1]);
            assert!(verdict.sensitive_image_indices.is_empty());
        }
        other => panic!("expected Completed(complete=false) for item 1, got {other:?}"),
    }

    assert!(matches!(
        &batch.items[2].outcome,
        FilterTargetOutcome::Failed { error } if error.contains("502")
    ));

    assert!(engine.snapshot().unwrap().cached_filter_verdicts.is_empty());
}

#[tokio::test]
async fn catalog_mixed_local_and_remote_images_preserve_exact_gallery_slot_order() {
    let safe_jpeg = make_tiny_jpeg(25, 200, 25);
    let nsfw_jpeg = make_tiny_jpeg(245, 25, 210);
    let nsfw_b64 = STANDARD.encode(&nsfw_jpeg);
    let nsfw_data_url = format!("data:image/jpeg;base64,{nsfw_b64}");

    let nsfw_b64_pred = nsfw_b64.clone();
    let app = Router::new()
        .route(
            "/api/v1/filter/predict",
            post(move |Json(req): Json<FilterPredictRequest>| {
                let nsfw_b64_pred = nsfw_b64_pred.clone();
                async move {
                    assert!(
                        req.image_urls.is_empty(),
                        "mixed remote + inline sources in Catalog mode must send all images in slot order via images_base64"
                    );
                    let per_img: Vec<f32> = req
                        .images_base64
                        .iter()
                        .map(|b| if b == &nsfw_b64_pred { 0.93 } else { 0.02 })
                        .collect();
                    Json(FilterProbabilities {
                        prob: 0.93,
                        gore_prob: 0.01,
                        nsfw_prob: 0.93,
                        text_prob: Some(0.02),
                        title_prob: Some(0.02),
                        image_prob: Some(0.93),
                        per_image_probs: per_img,
                        ..Default::default()
                    })
                }
            }),
        )
        .route(
            "/remote-safe-slot0.jpg",
            get(move || {
                let bytes = safe_jpeg.clone();
                async move {
                    (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "image/jpeg")],
                        bytes,
                    )
                        .into_response()
                }
            }),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: false,
            catalog_server_url: Some(base_url.clone()),
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::Catalog,
                subdivide_modalities: true,
                modality_threshold: 0.30,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );
    engine.set_allow_loopback_image_urls_for_testing(true);

    // Slot 0 is a remote URL (safe), Slot 1 is a data URL (NSFW).
    // Previously Catalog mode sent `images_base64` before `image_urls`, inverting slots 0 and 1!
    let verdicts = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:mixed-catalog-slots".into(),
                addon_id: None,
                workshop_id: None,
                title: "Mixed Gallery Item".into(),
                description: None,
                tags: vec![],
                image_sources: vec![format!("{base_url}/remote-safe-slot0.jpg"), nsfw_data_url],
                revision_hint: None,
                bypass_cache: false,
            }])
            .await
            .unwrap(),
    );
    assert_eq!(verdicts[0].evaluated_image_indices, vec![0, 1]);
    assert_eq!(
        verdicts[0].sensitive_image_indices,
        vec![1],
        "slot 1 (inline NSFW) must not shift ahead of slot 0 (remote safe)"
    );
}

#[tokio::test]
async fn catalog_uses_advertised_image_cap_and_retries_with_available_inline_data() {
    let jpeg = make_tiny_jpeg(35, 80, 180);
    let encoded = STANDARD.encode(&jpeg);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let app = Router::new()
        .route(
            "/api/v1/filter/info",
            get(|| async {
                Json(CatalogFilterPublicInfo {
                    enabled: true,
                    ready: true,
                    model_name: None,
                    model_version: None,
                    max_images: 1,
                    scoring_revision: Some("rev-one".into()),
                })
            }),
        )
        .route(
            "/api/v1/filter/predict",
            post(move |Json(req): Json<FilterPredictRequest>| {
                captured.lock().unwrap().push(req.clone());
                async move {
                    if !req.image_urls.is_empty() {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "code": l4a_protocol::CATALOG_FILTER_IMAGE_FETCH_FAILED,
                            })),
                        )
                            .into_response();
                    }
                    Json(FilterProbabilities {
                        prob: 0.05,
                        image_prob: Some(0.05),
                        per_image_probs: vec![0.05],
                        scoring_revision: Some("rev-one".into()),
                        ..Default::default()
                    })
                    .into_response()
                }
            }),
        )
        .route(
            "/preview.jpg",
            get(move || {
                let jpeg = jpeg.clone();
                async move { ([(header::CONTENT_TYPE, "image/jpeg")], jpeg) }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempdir().unwrap();
    let engine = make_engine(
        dir.path(),
        AppConfig {
            https_only: false,
            catalog_server_url: Some(base.clone()),
            filter: ClientFilterConfig {
                mode: ClientFilterSourceMode::Catalog,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );
    engine.set_allow_loopback_image_urls_for_testing(true);

    let verdicts = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:catalog-retry".into(),
                addon_id: None,
                workshop_id: None,
                title: "Catalog gallery".into(),
                description: None,
                tags: vec![],
                image_sources: vec![
                    format!("{base}/preview.jpg"),
                    "data:image/png;base64,unused".into(),
                ],
                revision_hint: None,
                bypass_cache: false,
            }])
            .await
            .unwrap(),
    );
    assert!(verdicts[0].complete);
    assert_eq!(verdicts[0].evaluated_image_indices, vec![0]);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].image_urls, vec![format!("{base}/preview.jpg")]);
    assert!(requests[0].images_base64.is_empty());
    assert!(requests[1].image_urls.is_empty());
    assert_eq!(requests[1].images_base64, vec![encoded]);
    task.abort();
}

#[tokio::test]
async fn catalog_revision_change_during_llm_review_rechecks_the_old_prediction() {
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;
    use tokio::sync::Notify;

    let newer = Arc::new(AtomicBool::new(false));
    let health_revision = newer.clone();
    let predict_revision = newer.clone();
    let review_started = Arc::new(Notify::new());
    let finish_review = Arc::new(Notify::new());
    let review_route_started = review_started.clone();
    let review_route_finish = finish_review.clone();
    let fake_catalog = Router::new()
        .route(
            "/api/v1/filter/info",
            get(move || {
                let revision = if health_revision.load(Ordering::SeqCst) {
                    "b"
                } else {
                    "a"
                };
                async move {
                    Json(CatalogFilterPublicInfo {
                        enabled: true,
                        ready: true,
                        model_name: None,
                        model_version: None,
                        max_images: 2,
                        scoring_revision: Some(revision.into()),
                    })
                }
            }),
        )
        .route(
            "/api/v1/filter/predict",
            post(move || {
                let (revision, prob) = if predict_revision.load(Ordering::SeqCst) {
                    ("b", 0.91)
                } else {
                    ("a", 0.4)
                };
                async move {
                    Json(FilterProbabilities {
                        prob,
                        scoring_revision: Some(revision.into()),
                        ..Default::default()
                    })
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move || {
                let started = review_route_started.clone();
                let finish = review_route_finish.clone();
                async move {
                    started.notify_one();
                    finish.notified().await;
                    Json(json!({ "choices": [{ "message": { "content": "false" } }] }))
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, fake_catalog).await.unwrap() });
    let dir = tempdir().unwrap();
    let engine = Arc::new(make_engine(
        dir.path(),
        AppConfig {
            https_only: false,
            catalog_server_url: Some(base.clone()),
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::Catalog,
                llm_enabled: true,
                llm_endpoint: format!("{base}/v1"),
                llm_token_source: LlmTokenSource::EncryptedLocal,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    ));
    engine.set_token_vault(TokenVault::with_custom_identity(
        &engine.paths().llm_token_path,
        dir.path().join("test.key"),
        "machine",
        "account",
    ));
    engine
        .set_filter_llm_token(Some("fixture-key".into()))
        .unwrap();

    fn target(id: &str) -> FilterEvaluationTarget {
        FilterEvaluationTarget {
            target_key: format!("ws:{id}"),
            addon_id: None,
            workshop_id: None,
            title: format!("Fixture {id}"),
            description: None,
            tags: vec![],
            image_sources: vec![],
            revision_hint: None,
            bypass_cache: false,
        }
    }

    let old_engine = engine.clone();
    let old_batch = tokio::spawn(async move {
        old_engine
            .evaluate_filter_items(vec![target("1")])
            .await
            .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(5), review_started.notified())
        .await
        .unwrap();
    newer.store(true, Ordering::SeqCst);
    let new_batch = engine
        .evaluate_filter_items(vec![target("2")])
        .await
        .unwrap();
    finish_review.notify_one();
    let old_batch = old_batch.await.unwrap();
    assert_eq!(old_batch.policy_signature, new_batch.policy_signature);
    assert_eq!(
        unwrap_verdicts(old_batch)[0].decision_source,
        FilterDecisionSource::ModelDirectFlag
    );
    let cached = engine.snapshot().unwrap().cached_filter_verdicts;
    assert_eq!(cached.len(), 2);
    assert!(cached
        .iter()
        .all(|v| v.probabilities.scoring_revision.as_deref() == Some("b")));
    server.abort();
}

#[tokio::test]
async fn configure_failure_restarts_local_runner_instead_of_predicting_with_stale_params() {
    let dir = tempdir().unwrap();
    let upstream_repo = dir.path().join("upstream-reconfig-fail-repo");
    std::fs::create_dir_all(&upstream_repo).unwrap();

    run_git(&upstream_repo, &["init", "-b", "main"]);
    let deploy_dir = upstream_repo.join("deploy");
    std::fs::create_dir_all(&deploy_dir).unwrap();
    // Mock Left4Filter that fails if re-instantiated on `threads` change inside the SAME process
    // (`POST /configure` returns HTTP 400), while succeeding on a fresh process spawn!
    let py = r#"_INIT_CALLS = 0

class Left4Filter:
    def __init__(self, deploy_dir, num_threads=4):
        global _INIT_CALLS
        _INIT_CALLS += 1
        if _INIT_CALLS > 1:
            raise RuntimeError("simulated /configure reinit failure")
        self.config = {"model_name": "MockStrictSync", "version": "1.0.0"}
        self.num_threads = num_threads

    def predict(self, title="", description="", tags=None, image=None, images=None):
        p = 0.88 if "nsfw" in (title or "").lower() else 0.05
        return {"prob": p, "gore_prob": 0.01, "nsfw_prob": p}
"#;
    std::fs::write(deploy_dir.join("l4filter.py"), py).unwrap();
    std::fs::write(deploy_dir.join("vision_encoder.onnx"), b"vis").unwrap();
    std::fs::write(deploy_dir.join("fusion_head.onnx"), b"head").unwrap();
    std::fs::write(deploy_dir.join("text_vocab.json"), r#"{"vocab":{}}"#).unwrap();
    std::fs::write(deploy_dir.join("requirements.txt"), "# offline\n").unwrap();
    run_git(&upstream_repo, &["add", "."]);
    run_git(&upstream_repo, &["commit", "-m", "feat: strict sync model"]);

    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: false,
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::Local,
                auto_start_local_service: false, // Even with auto_start=false, failed /configure on running service must restart cleanly!
                repo_url: upstream_repo.to_str().unwrap().into(),
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );

    engine
        .deploy_local_filter(DeployLocalFilterRequest {
            repo_url: Some(upstream_repo.to_str().unwrap().into()),
            track_latest: true,
            target_commit: None,
            start_after_deploy: false,
            install_web_gpu: None,
        })
        .await
        .unwrap();
    install_offline_runner_shims(&engine.paths().filter_venv_dir);

    let started = engine.start_local_filter_service().await.unwrap();
    let pid1 = started.running_pid.expect("initial runner PID");

    let initial_eval = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:sync-test".into(),
                addon_id: None,
                workshop_id: None,
                title: "Explicit NSFW Outfit".into(),
                description: None,
                tags: vec![],
                image_sources: vec![],
                revision_hint: None,
                bypass_cache: false,
            }])
            .await
            .unwrap(),
    );
    assert!(initial_eval[0].flagged);

    // Change both `threads` (4 -> 2, triggering `POST /configure` HTTP 400 in PID 1!) AND
    // `nsfw_weight` (1.0 -> 0.0, so stale PID 1 would return 0.88 flagged while fresh PID 2 returns 0.0 safe!).
    let mut updated = engine.config();
    updated.filter.local_params.threads = 2;
    updated.filter.local_params.nsfw_weight = 0.0;
    updated.filter.local_params.gore_weight = 1.0;
    engine.save_config_async(updated).await.unwrap();

    let after_reconfig = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:sync-test".into(),
                addon_id: None,
                workshop_id: None,
                title: "Explicit NSFW Outfit".into(),
                description: None,
                tags: vec![],
                image_sources: vec![],
                revision_hint: None,
                bypass_cache: false,
            }])
            .await
            .unwrap(),
    );
    assert!(
        !after_reconfig[0].flagged,
        "must never predict with stale nsfw_weight=1.0 after /configure failure"
    );

    let status_after = engine.get_local_filter_status().await.unwrap();
    let pid2 = status_after.running_pid.expect("restarted runner PID");
    assert_ne!(
        pid1, pid2,
        "failed /configure must stop old runner and spawn a fresh runner with updated params"
    );

    engine.shutdown().await;
}

#[tokio::test]
async fn catalog_scoring_revision_rotation_and_remote_ttl_invalidate_cached_verdicts() {
    let active_rev = Arc::new(Mutex::new("rev-alpha".to_string()));
    let predict_hits = Arc::new(AtomicUsize::new(0));

    let rev_info = active_rev.clone();
    let rev_pred = active_rev.clone();
    let hits_pred = predict_hits.clone();

    let app = Router::new()
        .route(
            "/api/v1/filter/info",
            get(move || {
                let rev = rev_info.lock().unwrap().clone();
                async move {
                    Json(CatalogFilterPublicInfo {
                        enabled: true,
                        ready: true,
                        model_name: Some("Left4FiltersModel".into()),
                        model_version: Some("1.0.0".into()),
                        max_images: 4,
                        scoring_revision: Some(rev),
                    })
                }
            }),
        )
        .route(
            "/api/v1/filter/predict",
            post(move |Json(_req): Json<FilterPredictRequest>| {
                let rev = rev_pred.lock().unwrap().clone();
                let hits_pred = hits_pred.clone();
                async move {
                    hits_pred.fetch_add(1, Ordering::SeqCst);
                    let p = if rev == "rev-alpha" { 0.05 } else { 0.91 };
                    Json(FilterProbabilities {
                        prob: p,
                        gore_prob: 0.01,
                        nsfw_prob: p,
                        text_prob: Some(p),
                        title_prob: Some(p),
                        scoring_revision: Some(rev),
                        ..Default::default()
                    })
                }
            }),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: false,
            catalog_server_url: Some(base_url),
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::Catalog,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );

    let target = FilterEvaluationTarget {
        target_key: "ws:cat-rot-1".into(),
        addon_id: None,
        workshop_id: None,
        title: "Mod Sensitive Under Beta Calibration".into(),
        description: None,
        tags: vec![],
        image_sources: vec![],
        revision_hint: Some("v1".into()),
        bypass_cache: false,
    };

    let batch_alpha = engine
        .evaluate_filter_items(vec![target.clone()])
        .await
        .unwrap();
    let sig_alpha = batch_alpha.policy_signature.clone();
    let v_alpha = unwrap_verdicts(batch_alpha);
    assert!(!v_alpha[0].flagged);
    assert_eq!(predict_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        engine.snapshot().unwrap().filter_policy_signature,
        sig_alpha
    );
    assert_eq!(engine.snapshot().unwrap().cached_filter_verdicts.len(), 1);

    // Rotate Catalog scoring_revision to `rev-beta` and expire 30s info TTL
    *active_rev.lock().unwrap() = "rev-beta".into();
    engine.expire_catalog_scoring_cache_for_testing();

    let batch_beta = engine
        .evaluate_filter_items(vec![target.clone()])
        .await
        .unwrap();
    let sig_beta = batch_beta.policy_signature.clone();
    assert_ne!(sig_alpha, sig_beta);
    let v_beta = unwrap_verdicts(batch_beta);
    assert!(
        v_beta[0].flagged,
        "Catalog scoring_revision rotation must invalidate cached rev-alpha safe verdict!"
    );
    assert_eq!(predict_hits.load(Ordering::SeqCst), 2);
    assert_eq!(engine.snapshot().unwrap().filter_policy_signature, sig_beta);

    // Also verify 1-hour max-age expiry on remote verdicts even when policy_sig is unchanged
    let mut aged = v_beta[0].clone();
    aged.evaluated_at = chrono::Utc::now() - chrono::TimeDelta::minutes(61);
    let content_sig = l4a_core::filter::evaluator::compute_content_signature(&target, 8);
    engine
        .db()
        .put_cached_filter_verdict(
            &target.target_key,
            &content_sig,
            None,
            &sig_beta,
            &sig_beta,
            &aged,
        )
        .unwrap();
    assert!(
        engine.snapshot().unwrap().cached_filter_verdicts.is_empty(),
        "remote verdict older than 1 hour must expire from snapshot"
    );
    let _ = engine.evaluate_filter_items(vec![target]).await.unwrap();
    assert_eq!(
        predict_hits.load(Ordering::SeqCst),
        3,
        "expired >1h remote verdict must trigger fresh backend evaluation"
    );
}

#[tokio::test]
async fn late_finishing_old_policy_evaluation_never_prunes_active_policy_cache() {
    let slow_entered = Arc::new(tokio::sync::Notify::new());
    let slow_release = Arc::new(tokio::sync::Notify::new());

    let entered_route = slow_entered.clone();
    let release_route = slow_release.clone();

    let app = Router::new().route(
        "/predict",
        post(move |Json(req): Json<RawFilterPredictRequest>| {
            let entered_route = entered_route.clone();
            let release_route = release_route.clone();
            async move {
                let title = req.title.unwrap_or_default();
                if title.contains("[SLOW_OLD_POLICY]") {
                    entered_route.notify_one();
                    release_route.notified().await;
                }
                Json(FilterProbabilities {
                    prob: 0.80,
                    gore_prob: 0.01,
                    nsfw_prob: 0.80,
                    text_prob: Some(0.80),
                    title_prob: Some(0.80),
                    ..Default::default()
                })
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();
    let engine = Arc::new(make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: false,
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::CustomEndpoint,
                custom_endpoint_url: Some(format!("http://{addr}/predict")),
                flag_threshold: 0.72,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    ));

    let engine_slow = engine.clone();
    let slow_task = tokio::spawn(async move {
        engine_slow
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:slow-old".into(),
                addon_id: None,
                workshop_id: None,
                title: "[SLOW_OLD_POLICY] In-Flight Item".into(),
                description: None,
                tags: vec![],
                image_sources: vec![],
                revision_hint: None,
                bypass_cache: false,
            }])
            .await
            .unwrap()
    });

    slow_entered.notified().await;

    // Switch active policy while `ws:slow-old` is in flight, and populate active policy's cache!
    let mut next_cfg = engine.config();
    next_cfg.filter.flag_threshold = 0.55;
    engine.save_config_async(next_cfg).await.unwrap();

    let fast_batch = engine
        .evaluate_filter_items(vec![FilterEvaluationTarget {
            target_key: "ws:fast-new".into(),
            addon_id: None,
            workshop_id: None,
            title: "Active Policy Item".into(),
            description: None,
            tags: vec![],
            image_sources: vec![],
            revision_hint: None,
            bypass_cache: false,
        }])
        .await
        .unwrap();
    let active_sig = fast_batch.policy_signature.clone();
    assert_eq!(engine.snapshot().unwrap().cached_filter_verdicts.len(), 1);

    // Now allow the old-policy evaluation to finish late
    slow_release.notify_one();
    let slow_batch = slow_task.await.unwrap();
    assert_ne!(slow_batch.policy_signature, active_sig);

    // Active policy's cached verdict MUST remain intact, and old-policy verdict MUST NOT be persisted!
    let snap = engine.snapshot().unwrap();
    assert_eq!(snap.filter_policy_signature, active_sig);
    assert_eq!(snap.cached_filter_verdicts.len(), 1);
    assert_eq!(snap.cached_filter_verdicts[0].target_key, "ws:fast-new");
    let engine_slow = engine.clone();
    let slow_task = tokio::spawn(async move {
        engine_slow
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:before-clear".into(),
                addon_id: None,
                workshop_id: None,
                title: "[SLOW_OLD_POLICY] Waiting during cache clear".into(),
                description: None,
                tags: vec![],
                image_sources: vec![],
                revision_hint: None,
                bypass_cache: false,
            }])
            .await
            .unwrap()
    });
    slow_entered.notified().await;
    let cleared = engine.clear_filter_cache().unwrap();
    assert_eq!(cleared.count, 1);
    slow_release.notify_one();
    let stale = slow_task.await.unwrap();
    assert_eq!(stale.policy_signature, active_sig);
    assert!(stale.cache_generation < cleared.cache_generation);
    assert!(matches!(
        stale.items[0].outcome,
        FilterTargetOutcome::Failed { .. }
    ));
    assert!(engine.snapshot().unwrap().cached_filter_verdicts.is_empty());
}

#[tokio::test]
async fn deploy_resolves_remote_only_branch_names_for_detached_checkout() {
    let dir = tempdir().unwrap();
    let upstream_repo = dir.path().join("upstream-branch-repo");
    std::fs::create_dir_all(&upstream_repo).unwrap();

    run_git(&upstream_repo, &["init", "-b", "main"]);
    write_mock_left4filters_deploy(&upstream_repo, "1.0.0", 0.10);
    run_git(&upstream_repo, &["add", "."]);
    run_git(&upstream_repo, &["commit", "-m", "feat: main branch model"]);
    let main_sha = run_git(&upstream_repo, &["rev-parse", "HEAD"]);

    run_git(&upstream_repo, &["checkout", "-b", "feature/test-branch"]);
    write_mock_left4filters_deploy(&upstream_repo, "2.1.0-branch", 0.25);
    run_git(&upstream_repo, &["add", "."]);
    run_git(
        &upstream_repo,
        &["commit", "-m", "feat: experimental branch model"],
    );
    let branch_sha = run_git(&upstream_repo, &["rev-parse", "HEAD"]);
    // Switch upstream HEAD back to `main` so `feature/test-branch` is a non-default remote branch
    run_git(&upstream_repo, &["checkout", "main"]);
    assert_ne!(main_sha, branch_sha);

    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: false,
            filter: ClientFilterConfig {
                repo_url: upstream_repo.to_str().unwrap().into(),
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );

    let deployed = engine
        .deploy_local_filter(DeployLocalFilterRequest {
            repo_url: Some(upstream_repo.to_str().unwrap().into()),
            track_latest: false,
            target_commit: Some("feature/test-branch".into()),
            start_after_deploy: false,
            install_web_gpu: None,
        })
        .await
        .unwrap();

    assert!(deployed.deployed);
    assert_eq!(
        deployed.current_commit.as_ref().map(|c| c.sha.as_str()),
        Some(branch_sha.as_str()),
        "remote-only branch name must resolve to exact commit SHA in detached HEAD"
    );

    // Also test a branch created in upstream AFTER clone with single-ref `FETCH_HEAD` resolution
    run_git(&upstream_repo, &["checkout", "-b", "exp/fetch-head-branch"]);
    write_mock_left4filters_deploy(&upstream_repo, "3.0.0-exp", 0.35);
    run_git(&upstream_repo, &["add", "."]);
    run_git(&upstream_repo, &["commit", "-m", "feat: fetch-head branch"]);
    let exp_sha = run_git(&upstream_repo, &["rev-parse", "HEAD"]);
    run_git(&upstream_repo, &["checkout", "main"]);

    // Restrict remote.origin.fetch to main only so `git fetch --all` won't create `refs/remotes/origin/exp/...`,
    // forcing `git fetch origin exp/fetch-head-branch` + `FETCH_HEAD^{commit}` resolution!
    run_git(
        &engine.paths().filter_repo_dir,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/main:refs/remotes/origin/main",
        ],
    );

    let deployed_exp = engine
        .deploy_local_filter(DeployLocalFilterRequest {
            repo_url: Some(upstream_repo.to_str().unwrap().into()),
            track_latest: false,
            target_commit: Some("exp/fetch-head-branch".into()),
            start_after_deploy: false,
            install_web_gpu: None,
        })
        .await
        .unwrap();
    assert_eq!(
        deployed_exp.current_commit.as_ref().map(|c| c.sha.as_str()),
        Some(exp_sha.as_str()),
        "FETCH_HEAD^{{commit}} fallback must resolve fetched branch SHA cleanly"
    );

    engine.shutdown().await;
}

#[tokio::test]
async fn malformed_unicode_data_urls_do_not_panic_and_concurrent_ipc_batches_are_bounded() {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak_in_flight = Arc::new(AtomicUsize::new(0));

    let in_flight_ref = in_flight.clone();
    let peak_ref = peak_in_flight.clone();

    let app = Router::new().route(
        "/predict",
        post(move |Json(_req): Json<RawFilterPredictRequest>| {
            let in_flight_ref = in_flight_ref.clone();
            let peak_ref = peak_ref.clone();
            async move {
                let cur = in_flight_ref.fetch_add(1, Ordering::SeqCst) + 1;
                peak_ref.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(35)).await;
                in_flight_ref.fetch_sub(1, Ordering::SeqCst);
                Json(FilterProbabilities {
                    prob: 0.04,
                    gore_prob: 0.01,
                    nsfw_prob: 0.03,
                    text_prob: Some(0.04),
                    title_prob: Some(0.04),
                    ..Default::default()
                })
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();
    let engine = Arc::new(make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: false,
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::CustomEndpoint,
                custom_endpoint_url: Some(format!("http://{addr}/predict")),
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    ));

    // 1. Malformed multibyte UTF-8 `data:` image inputs must never panic
    let unicode_batch = engine
        .evaluate_filter_items(vec![FilterEvaluationTarget {
            target_key: "ws:unicode-malformed".into(),
            addon_id: None,
            workshop_id: None,
            title: "Unicode Malformed Data URL Test".into(),
            description: None,
            tags: vec![],
            image_sources: vec![
                "data:中文;base64,AAA".into(),
                "data:image/中文;base64,AAA".into(),
                "中文".into(),
            ],
            revision_hint: None,
            bypass_cache: true,
        }])
        .await
        .unwrap();
    assert_eq!(unicode_batch.items.len(), 1);
    match &unicode_batch.items[0].outcome {
        FilterTargetOutcome::Completed { verdict, complete } => {
            assert!(!verdict.flagged);
            assert!(!*complete && !verdict.complete);
        }
        other => panic!("expected Completed(complete=false), got {other:?}"),
    }

    // 2. Spawn 3 concurrent IPC batches of 4 uncached items (12 items total) and verify global
    //    `filter_eval_semaphore` bounds concurrent backend evaluations to <= 4!
    let mut handles = Vec::new();
    for batch_idx in 0..3 {
        let eng = engine.clone();
        handles.push(tokio::spawn(async move {
            let targets: Vec<FilterEvaluationTarget> = (0..4)
                .map(|i| FilterEvaluationTarget {
                    target_key: format!("ws:conc-{batch_idx}-{i}"),
                    addon_id: None,
                    workshop_id: None,
                    title: format!("Concurrent Item {batch_idx}-{i}"),
                    description: None,
                    tags: vec![],
                    image_sources: vec![],
                    revision_hint: None,
                    bypass_cache: false,
                })
                .collect();
            eng.evaluate_filter_items(targets).await.unwrap()
        }));
    }
    for h in handles {
        let res = h.await.unwrap();
        assert_eq!(res.items.len(), 4);
    }
    let observed_peak = peak_in_flight.load(Ordering::SeqCst);
    assert!(
        observed_peak <= 4,
        "expected global filter_eval_semaphore to bound concurrent /predict calls to <= 4, got {observed_peak}"
    );
}

#[tokio::test]
async fn ws_target_with_addon_id_caches_cleanly_failed_token_preserves_llm_cache_and_startup_failure_enters_error_cooldown(
) {
    let dir = tempdir().unwrap();
    let loading = dir.path().join("loading");
    std::fs::create_dir_all(&loading).unwrap();

    let vpk_path = loading.join("safe_campaign.vpk");
    create_test_vpk(
        &vpk_path,
        "Safe Campaign",
        "Mapper",
        false,
        None,
        b"safe-campaign-payload",
    );

    let predict_calls = Arc::new(AtomicUsize::new(0));
    let predict_calls_srv = predict_calls.clone();
    let app = Router::new().route(
        "/predict",
        post(move || {
            let calls = predict_calls_srv.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::json!({
                    "prob": 0.12,
                    "gore_prob": 0.05,
                    "nsfw_prob": 0.10
                }))
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading.to_string_lossy().into_owned(),
            https_only: false,
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::CustomEndpoint,
                custom_endpoint_url: Some(format!("http://{addr}/predict")),
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );
    let snap = engine.scan().unwrap();
    let addon = snap.addons.first().expect("scanned addon").clone();

    // 1. A `ws:` target carrying both `addon_id` and a `https://` URL in `revision_hint` (4+ colons)
    //    must NOT be misclassified as a stale local addon and must hit cache on repeat evaluation.
    let ws_target = FilterEvaluationTarget {
        target_key: "ws:123456".into(),
        addon_id: Some(addon.id.clone()),
        workshop_id: Some(l4a_protocol::WorkshopId(123456)),
        title: "Workshop Mirror Item".into(),
        description: Some("Clean description".into()),
        tags: vec!["Campaign".into()],
        image_sources: vec![],
        revision_hint: Some(
            "1763900000:Workshop Mirror Item:imgs=1:https://images.steamusercontent.com/ugc/1/a.jpg"
                .into(),
        ),
        bypass_cache: false,
    };
    let first = engine
        .evaluate_filter_items(vec![ws_target.clone()])
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(predict_calls.load(Ordering::SeqCst), 1);

    let second = engine.evaluate_filter_items(vec![ws_target]).await.unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(
        predict_calls.load(Ordering::SeqCst),
        1,
        "ws: target with addon_id and URL revision_hint must hit SQLite cache"
    );

    // 2. Failing `set_filter_llm_token` (oversized >8192 bytes) must NOT wipe existing LLM verdicts.
    let active_sig = first.policy_signature.clone();
    let llm_verdict = l4a_protocol::FilterItemVerdict {
        target_key: "ws:llm-keep".into(),
        revision_hint: None,
        flagged: true,
        decision_source: FilterDecisionSource::LlmFlaggedTrue,
        evaluated_score: 0.45,
        probabilities: FilterProbabilities {
            prob: 0.45,
            gore_prob: 0.10,
            nsfw_prob: 0.44,
            ..Default::default()
        },
        text_sensitive: true,
        title_sensitive: true,
        description_sensitive: false,
        image_sensitive: false,
        evaluated_image_indices: vec![],
        sensitive_image_indices: vec![],
        complete: true,
        evaluated_at: chrono::Utc::now(),
    };
    engine
        .db()
        .put_cached_filter_verdict(
            "ws:llm-keep",
            "sig-keep",
            None,
            &active_sig,
            &active_sig,
            &llm_verdict,
        )
        .unwrap();
    let too_long = "x".repeat(9000);
    assert!(engine.set_filter_llm_token(Some(too_long)).is_err());
    let still_cached = engine
        .db()
        .get_cached_filter_verdict("ws:llm-keep", "sig-keep", &active_sig, None, None)
        .unwrap();
    assert!(
        still_cached.is_some(),
        "failed vault.seal must not delete cached LLM verdicts"
    );

    // 3. When auto-start fails after emitting `Starting`, `service_state` must transition to `Error`
    //    (never stuck in `Starting`) and enforce the 3s restart cooldown on immediate retries.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let paths = engine.paths();
        let deploy_dir = paths.filter_repo_dir.join("deploy");
        std::fs::create_dir_all(paths.filter_repo_dir.join(".git")).unwrap();
        std::fs::create_dir_all(&deploy_dir).unwrap();
        for f in [
            "l4filter.py",
            "vision_encoder.onnx",
            "fusion_head.onnx",
            "text_vocab.json",
            "requirements.txt",
        ] {
            std::fs::write(deploy_dir.join(f), "stub").unwrap();
        }
        let venv_bin = paths.filter_venv_dir.join("bin");
        std::fs::create_dir_all(&venv_bin).unwrap();
        let failing_py = venv_bin.join("python");
        std::fs::write(&failing_py, "#!/usr/bin/env sh\necho 'boom' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&failing_py, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(&paths.filter_runner_path, "# stub runner\n").unwrap();

        let mut local_cfg = engine.config();
        local_cfg.filter.mode = ClientFilterSourceMode::Local;
        local_cfg.filter.auto_start_local_service = true;
        engine.save_config_async(local_cfg).await.unwrap();

        let local_target = FilterEvaluationTarget {
            target_key: "ws:auto-start-fail".into(),
            addon_id: None,
            workshop_id: None,
            title: "Auto Start Failure Probe".into(),
            description: None,
            tags: vec![],
            image_sources: vec![],
            revision_hint: None,
            bypass_cache: true,
        };
        let batch1 = engine
            .evaluate_filter_items(vec![local_target.clone()])
            .await
            .unwrap();
        match &batch1.items[0].outcome {
            FilterTargetOutcome::Failed { error } => {
                assert!(
                    error.contains("boom"),
                    "expected failing runner stderr 'boom', got: {error}"
                );
            }
            other => panic!("expected Failed outcome on broken runner boot, got {other:?}"),
        }
        let status = engine.get_local_filter_status().await.unwrap();
        assert_eq!(
            status.service_state,
            LocalFilterServiceState::Error,
            "failed auto-start must transition service_state out of Starting into Error"
        );

        let batch2 = engine
            .evaluate_filter_items(vec![local_target])
            .await
            .unwrap();
        match &batch2.items[0].outcome {
            FilterTargetOutcome::Failed { error } => {
                assert!(
                    error.contains("冷却"),
                    "immediate retry after startup failure must hit 3s cooldown, got: {error}"
                );
            }
            other => panic!("expected cooldown Failed outcome, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn catalog_image_scoring_disabled_still_allows_llm_image_review() {
    use l4a_protocol::WorkshopId;
    use std::sync::atomic::{AtomicBool, Ordering};

    let llm_saw_image = Arc::new(AtomicBool::new(false));
    let llm_saw_image_route = llm_saw_image.clone();

    let app = Router::new()
        .route(
            "/api/v1/filter/info",
            get(|| async {
                Json(CatalogFilterPublicInfo {
                    enabled: true,
                    ready: true,
                    model_name: None,
                    model_version: None,
                    max_images: 0,
                    scoring_revision: Some("text-only".into()),
                })
            }),
        )
        .route(
            "/api/v1/filter/predict",
            post(|Json(request): Json<FilterPredictRequest>| async move {
                assert!(request.image_urls.is_empty());
                assert!(request.images_base64.is_empty());
                assert_eq!(request.workshop_id, Some(WorkshopId::new(42).unwrap()));
                Json(serde_json::json!({
                    "prob": 0.45,
                    "goreProb": 0.10,
                    "nsfwProb": 0.44,
                    "textProb": 0.45,
                    "titleProb": 0.45,
                    "scoringRevision": "text-only"
                }))
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |Json(body): Json<serde_json::Value>| {
                let llm_saw_image = llm_saw_image_route.clone();
                async move {
                    let raw = body.to_string();
                    if raw.contains("image_url") {
                        llm_saw_image.store(true, Ordering::SeqCst);
                    }
                    Json(serde_json::json!({
                        "choices": [{
                            "message": {
                                "content": "{\"sensitive\": true}"
                            }
                        }]
                    }))
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: false,
            catalog_server_url: Some(format!("http://{addr}")),
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::Catalog,
                enable_direct_flag: true,
                flag_threshold: 0.80,
                llm_enabled: true,
                llm_threshold: 0.20,
                llm_endpoint: format!("http://{addr}/v1/chat/completions"),
                llm_model: "mock-vision".into(),
                llm_send_images: true,
                llm_token_source: LlmTokenSource::EncryptedLocal,
                subdivide_modalities: true,
                modality_threshold: 0.30,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );
    engine
        .set_filter_llm_token(Some("sk-test-token-123".into()))
        .unwrap();

    // Valid 1x1 PNG data URL
    let png_data_url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+X2dIAAAAASUVORK5CYII=";
    let verdicts = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:llm-image-fallback".into(),
                addon_id: None,
                workshop_id: Some(WorkshopId::new(42).unwrap()),
                title: "Borderline Title".into(),
                description: None,
                tags: vec![],
                image_sources: vec![png_data_url.into()],
                revision_hint: None,
                bypass_cache: true,
            }])
            .await
            .unwrap(),
    );
    assert_eq!(verdicts.len(), 1);
    let verdict = &verdicts[0];
    assert!(
        llm_saw_image.load(Ordering::SeqCst),
        "LLM request must include the image"
    );
    assert!(verdict.flagged);
    assert_eq!(
        verdict.decision_source,
        FilterDecisionSource::LlmFlaggedTrue
    );
    assert_eq!(verdict.evaluated_image_indices, vec![0]);
    assert!(
        verdict.image_sensitive,
        "LLM flagging with images must conservatively mark images sensitive when model did not score images"
    );
    assert_eq!(verdict.sensitive_image_indices, vec![0]);

    server_handle.abort();
}

#[tokio::test]
async fn custom_endpoint_image_disabled_does_not_fabricate_zero_image_scores_for_llm_flagged_item()
{
    use axum::{routing::post, Json, Router};
    use serde_json::json;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    let image_only_probes = Arc::new(AtomicUsize::new(0));
    let image_only_probes_srv = image_only_probes.clone();

    let app = Router::new()
        .route(
            "/api/v1/filter/predict",
            post(move |Json(body): Json<serde_json::Value>| {
                let image_only_probes_srv = image_only_probes_srv.clone();
                async move {
                    let has_title = body
                        .get("title")
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| !s.trim().is_empty());
                    if !has_title {
                        image_only_probes_srv.fetch_add(1, Ordering::SeqCst);
                        return Json(json!({
                            "prob": 0.0,
                            "goreProb": 0.0,
                            "nsfwProb": 0.0
                        }));
                    }
                    // Simulates an l4a-server / Catalog-compatible endpoint with `enable_image = false`:
                    // returns textProb/titleProb, omits imageProb/perImageProbs.
                    Json(json!({
                        "prob": 0.45,
                        "goreProb": 0.10,
                        "nsfwProb": 0.45,
                        "textProb": 0.45,
                        "titleProb": 0.45
                    }))
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(|| async move {
                Json(json!({
                    "choices": [{
                        "message": { "content": "{\"sensitive\": true}" }
                    }]
                }))
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let dir = tempdir().unwrap();
    let loading_dir = dir.path().join("addons");
    std::fs::create_dir_all(&loading_dir).unwrap();
    let engine = make_engine(
        dir.path(),
        AppConfig {
            loading_dir: loading_dir.to_str().unwrap().into(),
            https_only: false,
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::CustomEndpoint,
                custom_endpoint_url: Some(format!("http://{addr}/api/v1/filter/predict")),
                enable_direct_flag: true,
                flag_threshold: 0.85,
                subdivide_modalities: true,
                modality_threshold: 0.30,
                llm_enabled: true,
                llm_threshold: 0.25,
                llm_endpoint: format!("http://{addr}/v1/chat/completions"),
                llm_model: "test-vision".into(),
                llm_send_images: true,
                llm_token_source: LlmTokenSource::EncryptedLocal,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
    );
    engine
        .set_filter_llm_token(Some("sk-test-token-456".into()))
        .unwrap();

    let png_data_url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+X2dIAAAAASUVORK5CYII=";
    let verdicts = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:custom-img-disabled".into(),
                addon_id: None,
                workshop_id: None,
                title: "Borderline Custom Endpoint Title".into(),
                description: None,
                tags: vec![],
                image_sources: vec![png_data_url.into()],
                revision_hint: None,
                bypass_cache: true,
            }])
            .await
            .unwrap(),
    );
    assert_eq!(verdicts.len(), 1);
    let verdict = &verdicts[0];
    assert_eq!(
        image_only_probes.load(Ordering::SeqCst),
        0,
        "must not run image-only probes when server returned textProb without imageProb"
    );
    assert!(verdict.flagged);
    assert_eq!(
        verdict.decision_source,
        FilterDecisionSource::LlmFlaggedTrue
    );
    assert!(
        verdict.probabilities.image_prob.is_none()
            && verdict.probabilities.per_image_probs.is_empty(),
        "image probabilities must remain absent rather than fabricated 0.0"
    );
    assert!(
        verdict.image_sensitive,
        "LLM flagging with images must preserve image_sensitive=true on image-disabled CustomEndpoint"
    );
    assert_eq!(verdict.sensitive_image_indices, vec![0]);

    server_handle.abort();
}

#[tokio::test]
async fn streaming_evaluation_emits_fast_and_cached_cards_without_waiting_for_slow_cards() {
    let slow_started = Arc::new(tokio::sync::Notify::new());
    let slow_release = Arc::new(tokio::sync::Notify::new());
    let slow_started_srv = slow_started.clone();
    let slow_release_srv = slow_release.clone();

    let app = Router::new().route(
        "/api/v1/filter/predict",
        post(move |Json(req): Json<FilterPredictRequest>| {
            let started = slow_started_srv.clone();
            let release = slow_release_srv.clone();
            async move {
                let title = req.title.unwrap_or_default();
                if title.contains("[SLOW_BLOCKER]") {
                    started.notify_one();
                    release.notified().await;
                }
                let is_flagged = title.contains("[FLAG]");
                Json(FilterProbabilities {
                    prob: if is_flagged { 0.91 } else { 0.04 },
                    gore_prob: 0.01,
                    nsfw_prob: if is_flagged { 0.91 } else { 0.04 },
                    text_prob: Some(if is_flagged { 0.91 } else { 0.04 }),
                    title_prob: Some(if is_flagged { 0.91 } else { 0.04 }),
                    ..Default::default()
                })
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let dir = tempdir().unwrap();
    let loading = dir.path().join("loading");
    std::fs::create_dir_all(&loading).unwrap();

    struct ChannelEventSink {
        tx: tokio::sync::mpsc::UnboundedSender<CoreEvent>,
    }

    impl EventSink for ChannelEventSink {
        fn emit(&self, event: CoreEvent) {
            let _ = self.tx.send(event);
        }
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let sink = Arc::new(ChannelEventSink { tx });
    let engine = Arc::new(make_engine_with_events(
        dir.path(),
        AppConfig {
            loading_dir: loading.to_string_lossy().into_owned(),
            https_only: false,
            filter: ClientFilterConfig {
                enabled: true,
                mode: ClientFilterSourceMode::CustomEndpoint,
                custom_endpoint_url: Some(format!("http://{addr}/api/v1/filter/predict")),
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
        sink,
    ));

    let make_target = |key: &str, title: &str| FilterEvaluationTarget {
        target_key: key.into(),
        addon_id: None,
        workshop_id: None,
        title: title.into(),
        description: None,
        tags: vec![],
        image_sources: vec![],
        revision_hint: Some("rev-1".into()),
        bypass_cache: false,
    };

    // Prime one cached verdict so the mixed batch contains both a cache hit and uncached items.
    let cached_target = make_target("ws:cached-0", "Cached Fast Card");
    let _ = unwrap_verdicts(
        engine
            .evaluate_filter_items(vec![cached_target.clone()])
            .await
            .unwrap(),
    );
    while rx.try_recv().is_ok() {}

    // Put the slow blocker at index 0 followed by 5 fast uncached cards (> EVAL_CONCURRENCY=4)
    // and 1 cached card to prove both immediate emission and zero head-of-line blocking.
    let batch_targets = vec![
        make_target("ws:slow-0", "[SLOW_BLOCKER] Slow Card"),
        make_target("ws:fast-1", "Fast Safe 1"),
        make_target("ws:fast-2", "[FLAG] Fast Flagged 2"),
        make_target("ws:fast-3", "Fast Safe 3"),
        make_target("ws:fast-4", "Fast Safe 4"),
        make_target("ws:fast-5", "Fast Safe 5"),
        cached_target,
    ];

    let engine_for_batch = engine.clone();
    let batch_handle = tokio::spawn(async move {
        engine_for_batch
            .evaluate_filter_items(batch_targets)
            .await
            .unwrap()
    });

    // Wait until the slow blocker has entered the server handler.
    tokio::time::timeout(std::time::Duration::from_secs(5), slow_started.notified())
        .await
        .expect("slow card should start evaluating");

    // Collect all 6 non-blocked cards BEFORE releasing the slow card!
    let mut arrived_before_slow: Vec<String> = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while arrived_before_slow.len() < 6 {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("fast & cached cards must emit before slow card completes")
            .expect("event channel should stay open");
        if let CoreEvent::FilterVerdictsUpdated { verdicts, .. } = ev {
            for verdict in verdicts {
                arrived_before_slow.push(verdict.target_key);
            }
        }
    }

    assert!(
        !batch_handle.is_finished(),
        "batch future must still be running while slow card is blocked"
    );
    assert!(
        !arrived_before_slow.contains(&"ws:slow-0".to_string()),
        "slow card has not been released yet"
    );
    for expected in [
        "ws:cached-0",
        "ws:fast-1",
        "ws:fast-2",
        "ws:fast-3",
        "ws:fast-4",
        "ws:fast-5",
    ] {
        assert!(
            arrived_before_slow.contains(&expected.to_string()),
            "expected {expected} to stream before slow card finishes; got {arrived_before_slow:?}"
        );
    }

    // Release slow card and verify batch response preserves exact original input order.
    slow_release.notify_one();
    let response = batch_handle.await.unwrap();
    let returned_keys: Vec<&str> = response
        .items
        .iter()
        .map(|item| item.target_key.as_str())
        .collect();
    assert_eq!(
        returned_keys,
        vec![
            "ws:slow-0",
            "ws:fast-1",
            "ws:fast-2",
            "ws:fast-3",
            "ws:fast-4",
            "ws:fast-5",
            "ws:cached-0",
        ]
    );

    let slow_event = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(ev) = rx.recv().await {
            if let CoreEvent::FilterVerdictsUpdated { verdicts, .. } = ev {
                if verdicts.iter().any(|v| v.target_key == "ws:slow-0") {
                    return true;
                }
            }
        }
        false
    })
    .await
    .expect("slow card should emit once released");
    assert!(slow_event);

    server_handle.abort();
}

#[tokio::test]
async fn streaming_evaluation_suppresses_writes_and_events_after_mid_batch_invalidation() {
    use tokio::sync::Notify;

    let gate_started = Arc::new(Notify::new());
    let gate_release = Arc::new(Notify::new());
    let started_for_route = gate_started.clone();
    let release_for_route = gate_release.clone();

    let app = Router::new().route(
        "/api/v1/filter/predict",
        post(move |Json(req): Json<RawFilterPredictRequest>| {
            let started = started_for_route.clone();
            let release = release_for_route.clone();
            async move {
                let title = req.title.unwrap_or_default();
                if title.contains("[HOLD]") {
                    started.notify_one();
                    release.notified().await;
                }
                Json(FilterProbabilities {
                    prob: 0.04,
                    gore_prob: 0.01,
                    nsfw_prob: 0.01,
                    text_prob: Some(0.04),
                    title_prob: Some(0.04),
                    ..Default::default()
                })
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let dir = tempdir().unwrap();
    let loading = dir.path().join("addons");
    std::fs::create_dir_all(&loading).unwrap();
    let vpk_path = loading.join("919191.vpk");
    create_test_vpk(
        &vpk_path,
        "[HOLD] Local Mutable Addon V1",
        "Mapper",
        false,
        None,
        b"v1-bytes",
    );

    struct ChannelEventSink {
        tx: tokio::sync::mpsc::UnboundedSender<CoreEvent>,
    }
    impl EventSink for ChannelEventSink {
        fn emit(&self, event: CoreEvent) {
            let _ = self.tx.send(event);
        }
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let sink = Arc::new(ChannelEventSink { tx });
    let engine = Arc::new(make_engine_with_events(
        dir.path(),
        AppConfig {
            loading_dir: loading.to_string_lossy().into_owned(),
            https_only: false,
            filter: ClientFilterConfig {
                enabled: true,
                apply_to_library: true,
                apply_to_workshop: true,
                mode: ClientFilterSourceMode::CustomEndpoint,
                custom_endpoint_url: Some(format!("http://{addr}/api/v1/filter/predict")),
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockSteamBridge::new(false)),
        sink,
    ));

    let drain_verdict_keys = |rx: &mut tokio::sync::mpsc::UnboundedReceiver<CoreEvent>| {
        let mut keys = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let CoreEvent::FilterVerdictsUpdated { verdicts, .. } = ev {
                keys.extend(verdicts.into_iter().map(|v| v.target_key));
            }
        }
        keys
    };

    // Case 1: Mid-batch clear_filter_cache() suppresses remaining SQLite writes & FilterVerdictsUpdated events.
    let engine_clear = engine.clone();
    let clear_handle = tokio::spawn(async move {
        engine_clear
            .evaluate_filter_items(vec![FilterEvaluationTarget {
                target_key: "ws:clear-victim".into(),
                addon_id: None,
                workshop_id: None,
                title: "[HOLD] Clear Victim".into(),
                description: None,
                tags: vec![],
                image_sources: vec![],
                revision_hint: Some("rev-clear".into()),
                bypass_cache: false,
            }])
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), gate_started.notified())
        .await
        .expect("case 1 request should reach server");
    let _ = engine.clear_filter_cache().unwrap();
    let _ = drain_verdict_keys(&mut rx);
    gate_release.notify_one();
    let _ = clear_handle.await.unwrap();

    assert!(
        drain_verdict_keys(&mut rx).is_empty(),
        "mid-batch clear_filter_cache must suppress streamed FilterVerdictsUpdated"
    );
    assert_eq!(
        engine.clear_filter_cache().unwrap().count,
        0,
        "mid-batch clear_filter_cache must suppress SQLite verdict persistence"
    );

    // Case 2: Mid-batch automatic scope disable (apply_to_library = false) suppresses writes & events.
    let snap_v1 = engine.scan().unwrap();
    let addon_v1 = snap_v1.addons[0].clone();
    let target_v1 = l4a_core::filter::build_addon_filter_target(&addon_v1);
    let _ = drain_verdict_keys(&mut rx);

    let engine_disable = engine.clone();
    let target_for_disable = target_v1.clone();
    let disable_handle = tokio::spawn(async move {
        engine_disable
            .evaluate_automatic_filter_items(vec![target_for_disable])
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), gate_started.notified())
        .await
        .expect("case 2 request should reach server");
    let mut disabled_cfg = engine.config();
    disabled_cfg.filter.apply_to_library = false;
    engine.save_config_async(disabled_cfg).await.unwrap();
    let _ = drain_verdict_keys(&mut rx);
    gate_release.notify_one();
    let _ = disable_handle.await.unwrap();

    assert!(
        drain_verdict_keys(&mut rx).is_empty(),
        "mid-batch apply_to_library=false must suppress streamed FilterVerdictsUpdated"
    );
    assert!(
        engine.snapshot().unwrap().cached_filter_verdicts.is_empty(),
        "mid-batch apply_to_library=false must suppress SQLite verdict persistence"
    );

    // Re-enable apply_to_library for Case 3.
    let mut reenabled_cfg = engine.config();
    reenabled_cfg.filter.apply_to_library = true;
    engine.save_config_async(reenabled_cfg).await.unwrap();
    let _ = drain_verdict_keys(&mut rx);

    // Case 3: Mid-batch local addon disk modification + scan() suppresses stale verdict write & event.
    let engine_disk = engine.clone();
    let target_for_disk = target_v1;
    let disk_handle = tokio::spawn(async move {
        engine_disk
            .evaluate_automatic_filter_items(vec![target_for_disk])
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), gate_started.notified())
        .await
        .expect("case 3 request should reach server");
    create_test_vpk(
        &vpk_path,
        "Local Mutable Addon V2 Changed On Disk",
        "Mapper",
        false,
        None,
        b"v2-modified-bytes",
    );
    let _ = engine.scan().unwrap();
    let _ = drain_verdict_keys(&mut rx);
    gate_release.notify_one();
    let _ = disk_handle.await.unwrap();

    assert!(
        drain_verdict_keys(&mut rx).is_empty(),
        "mid-batch local addon disk modification must suppress stale streamed FilterVerdictsUpdated"
    );
    assert!(
        engine.snapshot().unwrap().cached_filter_verdicts.is_empty(),
        "mid-batch local addon disk modification must suppress stale SQLite verdict persistence"
    );

    server_handle.abort();
}
