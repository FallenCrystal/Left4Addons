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
    AppConfig, ClientFilterConfig, ClientFilterSourceMode, CoreEvent, DeployLocalFilterRequest,
    ExclusionDisplayLevel, FilterDecisionSource, FilterEvaluationTarget, FilterPredictRequest,
    FilterProbabilities, LlmTokenSource, LocalFilterServiceState, UpsertFilterExclusionRequest,
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

fn install_offline_runner_shims(engine: &L4aEngine) {
    let venv_py = if cfg!(windows) {
        engine
            .paths()
            .filter_venv_dir
            .join("Scripts")
            .join("python.exe")
    } else {
        engine.paths().filter_venv_dir.join("bin").join("python")
    };
    let site_out = Command::new(&venv_py)
        .args(["-c", "import site; print(site.getsitepackages()[0])"])
        .output()
        .unwrap();
    assert!(site_out.status.success());
    let site_dir = Path::new(String::from_utf8_lossy(&site_out.stdout).trim()).to_path_buf();
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
    std::fs::write(pil_dir.join("Image.py"), "MAX_IMAGE_PIXELS = 16777216\n").unwrap();
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
    let low_verdicts = engine
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
        .unwrap();
    assert_eq!(low_verdicts.len(), 1);
    assert!(!low_verdicts[0].flagged);
    assert_eq!(
        low_verdicts[0].decision_source,
        FilterDecisionSource::ModelSafe
    );
    assert_eq!(mock_state.llm_calls.load(Ordering::SeqCst), 0);

    // 2. High score >= flag_threshold -> ModelDirectFlag, LLM not called + Image-only subdivision
    let high_img_verdicts = engine
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
        .unwrap();
    assert!(high_img_verdicts[0].flagged);
    assert_eq!(
        high_img_verdicts[0].decision_source,
        FilterDecisionSource::ModelDirectFlag
    );
    assert!(high_img_verdicts[0].image_sensitive);
    assert!(!high_img_verdicts[0].text_sensitive);
    assert_eq!(high_img_verdicts[0].sensitive_image_indices, vec![0]);
    assert_eq!(mock_state.llm_calls.load(Ordering::SeqCst), 0);

    // 3. Mid score [llm_threshold, flag_threshold) -> escalates to OpenAI Chat Completions
    let mid_true = engine
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
        .unwrap();
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
    let mid_false = engine
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
        .unwrap();
    assert!(!mid_false[0].flagged);
    assert_eq!(
        mid_false[0].decision_source,
        FilterDecisionSource::LlmFlaggedFalse
    );
    assert!(!mid_false[0].image_sensitive);
    assert!(!mid_false[0].text_sensitive);
    assert_eq!(mock_state.llm_calls.load(Ordering::SeqCst), 2);

    // 4. Modality subdivision: text-only sensitive & multi-image specific index
    let sub_verdicts = engine
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
        .unwrap();

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
    assert_eq!(sub_verdicts[1].sensitive_image_indices, vec![1]);

    // 5. SQLite cache hit verification & clear_filter_cache
    let calls_before_cached = mock_state.predict_calls.load(Ordering::SeqCst);
    let cached_again = engine
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
        .unwrap();
    assert_eq!(cached_again[0].sensitive_image_indices, vec![1]);
    assert_eq!(
        mock_state.predict_calls.load(Ordering::SeqCst),
        calls_before_cached
    );

    let cleared = engine.clear_filter_cache().unwrap();
    assert_eq!(cleared, 6);
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
            bind_fingerprint: true,
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
        post(move |Json(req): Json<FilterPredictRequest>| {
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

    let verdicts = engine
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
        .unwrap();

    assert_eq!(verdicts.len(), 1);
    assert!(verdicts[0].flagged);
    assert!(!verdicts[0].text_sensitive);
    assert!(verdicts[0].image_sensitive);
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
        })
        .await
        .unwrap();

    install_offline_runner_shims(&engine);

    let started = engine.start_local_filter_service().await.unwrap();
    assert_eq!(started.service_state, LocalFilterServiceState::Running);
    assert!(started.running_pid.is_some());
    assert!(started
        .service_endpoint
        .as_deref()
        .is_some_and(|u| u.starts_with("http://127.0.0.1:")));

    // Evaluate against the live Python runner
    let verdicts = engine
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
        .unwrap();
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

    let reevaluated = engine
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
        .unwrap();
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

    let direct_off = engine
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
        .unwrap();
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

    let llm_verdicts = engine
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
        .unwrap();

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

    // 3 succeeded items returned (2 from deduplicated 3001 + 1 from 3003); 3002 failed without aborting siblings!
    assert_eq!(batch_res.len(), 3);
    assert_eq!(batch_res[0].target_key, "addon:workshop:3001");
    assert_eq!(batch_res[1].target_key, "addon:workshop:3001");
    assert_eq!(batch_res[2].target_key, "addon:workshop:3003");
    assert!(batch_res[2].flagged);
    // Dedup ensured only 3 backend /predict calls (3001 once, 3002 once, 3003 once), NOT 4!
    assert_eq!(predict_calls.load(Ordering::SeqCst), 3);

    // Verify FilterVerdictsUpdated event carries non-empty policy_signature
    let drained = events.drain();
    assert!(drained.iter().any(|ev| matches!(
        ev,
        CoreEvent::FilterVerdictsUpdated {
            verdicts,
            policy_signature: Some(sig)
        } if verdicts.len() == 3 && !sig.is_empty()
    )));

    // Verify snapshot.cached_filter_verdicts is populated immediately at startup / snapshot()!
    let snap = engine.snapshot().unwrap();
    assert_eq!(snap.cached_filter_verdicts.len(), 2);

    // 2. Fail-open cache poisoning guard: transient image 503 must NOT cache `!flagged`!
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
    assert!(
        !first_try[0].flagged,
        "when preview returns 503, text-only score is safe"
    );

    // Now the image server recovers (200 OK). Re-evaluating with `bypass_cache: false` MUST NOT hit a poisoned safe cache!
    img_available.store(true, Ordering::SeqCst);
    let second_try = engine
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
        .unwrap();
    assert!(
        second_try[0].flagged,
        "transient image failure must not poison filter_verdicts with false-safe!"
    );

    // 3. Stale policy_sig pruning: changing threshold and evaluating a new item prunes old policy rows
    let mut updated_cfg = engine.config();
    updated_cfg.filter.flag_threshold = 0.65;
    engine.save_config_async(updated_cfg).await.unwrap();
    engine
        .evaluate_filter_items(vec![dup_target])
        .await
        .unwrap();
    // Total rows across entire `filter_verdicts` table must only belong to the new policy_sig (1 row)!
    assert_eq!(engine.clear_filter_cache().unwrap(), 1);
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
        })
        .await
        .unwrap();

    install_offline_runner_shims(&engine);

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
    let verdicts = engine
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
        .unwrap();
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
            bind_fingerprint: false,
            note: Some("Unbound local exclusion".into()),
        })
        .unwrap();

    // Fingerprint-bound exclusion on beta_v1
    engine
        .upsert_filter_exclusion(UpsertFilterExclusionRequest {
            addon_id: beta_v1.id.clone(),
            display_level: ExclusionDisplayLevel::BlurImage,
            bind_fingerprint: true,
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
            post(|Json(req): Json<FilterPredictRequest>| async move {
                assert!(
                    req.images_base64.is_empty() && req.image_urls.is_empty(),
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
