use super::L4aEngine;
use crate::config::{normalize_filter_preset_params, save_config};
use crate::db::LocalDb;
use crate::error::CoreError;
use crate::filter::deploy::{validate_repo_url, validate_target_commit};
use crate::filter::secret::validate_env_var_name;
use crate::filter::{evaluate_targets, TokenVault};
use l4a_protocol::{
    AddonId, ClientFilterConfig, CoreEvent, DashboardSnapshot, DeployLocalFilterRequest,
    FilterCacheClearResult, FilterEvaluationBatchResponse, FilterEvaluationTarget,
    FilterTokenStatus, LlmTokenSource, LocalFilterDeploymentStatus, UpsertFilterExclusionRequest,
    DEFAULT_FILTER_REPO_URL, DEFAULT_LLM_ENDPOINT, DEFAULT_LLM_MODEL, DEFAULT_LLM_SYSTEM_PROMPT,
    DEFAULT_LLM_TOKEN_ENV_VAR, LEGACY_LLM_SYSTEM_PROMPT,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

#[derive(Default)]
pub(crate) struct FilterCacheState {
    pub(crate) generation: u64,
    // Private to the running engine. An empty token represents any unavailable credential.
    env_token_fingerprint: Option<[u8; 32]>,
}

fn uses_env_token(cfg: &ClientFilterConfig) -> bool {
    cfg.llm_enabled && cfg.llm_token_source == LlmTokenSource::EnvVar
}

impl FilterCacheState {
    pub(super) fn new(
        db: &LocalDb,
        cfg: &ClientFilterConfig,
        vault: &TokenVault,
    ) -> Result<Self, CoreError> {
        let mut state = Self::default();
        state.refresh_env_token(db, cfg, vault)?;
        Ok(state)
    }

    fn refresh_env_token(
        &mut self,
        db: &LocalDb,
        cfg: &ClientFilterConfig,
        vault: &TokenVault,
    ) -> Result<bool, CoreError> {
        if !uses_env_token(cfg) {
            return Ok(false);
        }
        let token = vault.resolve_token(LlmTokenSource::EnvVar, &cfg.llm_token_env_var);
        self.observe_env_token(db, &token)
    }

    fn observe_env_token(
        &mut self,
        db: &LocalDb,
        token: &Result<Zeroizing<String>, CoreError>,
    ) -> Result<bool, CoreError> {
        let value = token.as_ref().map_or("", |value| value.as_str());
        let fingerprint = Sha256::digest(value.as_bytes()).into();
        if self.env_token_fingerprint == Some(fingerprint) {
            return Ok(false);
        }

        // The startup credential is also unknown to any verdicts persisted by a prior process.
        db.clear_llm_filter_verdicts()?;
        let changed = self.env_token_fingerprint.replace(fingerprint).is_some();
        self.generation += u64::from(changed);
        Ok(changed)
    }
}

fn sanitize_filter_basics(cfg: &mut ClientFilterConfig) {
    normalize_filter_preset_params(cfg);

    cfg.flag_threshold = if cfg.flag_threshold.is_finite() {
        cfg.flag_threshold.clamp(0.0, 1.0)
    } else {
        0.72
    };
    cfg.llm_threshold = if cfg.llm_threshold.is_finite() {
        cfg.llm_threshold.clamp(0.0, 1.0)
    } else {
        0.20
    };
    cfg.modality_threshold = if cfg.modality_threshold.is_finite() {
        cfg.modality_threshold.clamp(0.0, 1.0)
    } else {
        0.30
    };

    let trimmed_repo = cfg.repo_url.trim();
    cfg.repo_url = if trimmed_repo.is_empty() {
        DEFAULT_FILTER_REPO_URL.to_string()
    } else {
        trimmed_repo.to_string()
    };

    cfg.custom_endpoint_url = cfg
        .custom_endpoint_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let trimmed_endpoint = cfg.llm_endpoint.trim();
    cfg.llm_endpoint = if trimmed_endpoint.is_empty() {
        DEFAULT_LLM_ENDPOINT.to_string()
    } else {
        trimmed_endpoint.to_string()
    };

    let trimmed_model = cfg.llm_model.trim();
    cfg.llm_model = if trimmed_model.is_empty() {
        DEFAULT_LLM_MODEL.to_string()
    } else {
        trimmed_model.to_string()
    };

    let prompt = cfg.llm_system_prompt.trim();
    if prompt.is_empty() || prompt == LEGACY_LLM_SYSTEM_PROMPT {
        cfg.llm_system_prompt = DEFAULT_LLM_SYSTEM_PROMPT.to_string();
    }
}

pub(crate) fn sanitize_filter_config(
    cfg: &mut ClientFilterConfig,
    https_only: bool,
) -> Result<(), CoreError> {
    sanitize_filter_basics(cfg);

    cfg.repo_url = validate_repo_url(&cfg.repo_url, https_only)?;

    cfg.pinned_commit = match cfg
        .pinned_commit
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(commit) => Some(validate_target_commit(commit)?),
        None => None,
    };

    let trimmed_env = cfg.llm_token_env_var.trim();
    cfg.llm_token_env_var = if trimmed_env.is_empty() {
        DEFAULT_LLM_TOKEN_ENV_VAR.to_string()
    } else {
        validate_env_var_name(trimmed_env)?.to_string()
    };

    Ok(())
}

pub(crate) fn sanitize_filter_config_for_startup(cfg: &mut ClientFilterConfig, https_only: bool) {
    sanitize_filter_basics(cfg);

    match validate_repo_url(&cfg.repo_url, https_only) {
        Ok(valid) => cfg.repo_url = valid,
        Err(_) => cfg.repo_url = DEFAULT_FILTER_REPO_URL.to_string(),
    }

    cfg.pinned_commit = cfg
        .pinned_commit
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|c| validate_target_commit(c).ok());

    let trimmed_env = cfg.llm_token_env_var.trim().to_string();
    cfg.llm_token_env_var = if trimmed_env.is_empty() {
        DEFAULT_LLM_TOKEN_ENV_VAR.to_string()
    } else {
        match validate_env_var_name(&trimmed_env) {
            Ok(valid) => valid.to_string(),
            Err(_) => DEFAULT_LLM_TOKEN_ENV_VAR.to_string(),
        }
    };
}

impl L4aEngine {
    pub fn set_token_vault(&self, vault: TokenVault) {
        *self.token_vault.write().unwrap() = vault;
    }

    pub(crate) fn refresh_filter_env_token(
        &self,
        cache: &mut FilterCacheState,
        cfg: &ClientFilterConfig,
    ) -> Result<bool, CoreError> {
        cache.refresh_env_token(&self.db, cfg, &self.token_vault.read().unwrap())
    }

    pub(crate) fn resolve_filter_llm_token(
        &self,
        cache: &mut FilterCacheState,
        cfg: &ClientFilterConfig,
    ) -> Result<Zeroizing<String>, CoreError> {
        let token = self
            .token_vault
            .read()
            .unwrap()
            .resolve_token(cfg.llm_token_source, &cfg.llm_token_env_var);
        if uses_env_token(cfg) {
            cache.observe_env_token(&self.db, &token)?;
        }
        token
    }

    pub async fn get_local_filter_status(&self) -> Result<LocalFilterDeploymentStatus, CoreError> {
        let mut status = self.local_filter.status().await;
        status.cache_generation = Some(self.filter_cache.lock().unwrap().generation);
        Ok(status)
    }

    pub async fn check_local_filter_updates(
        &self,
    ) -> Result<LocalFilterDeploymentStatus, CoreError> {
        let cfg = self.config();
        let mut status = self
            .local_filter
            .check_updates(&cfg.filter.repo_url, cfg.https_only)
            .await?;
        status.cache_generation = Some(self.filter_cache.lock().unwrap().generation);
        Ok(status)
    }

    pub async fn deploy_local_filter(
        &self,
        request: DeployLocalFilterRequest,
    ) -> Result<LocalFilterDeploymentStatus, CoreError> {
        let cfg = self.config();
        let head_before = self.local_filter.current_repo_head_sha().await;
        let mut outcome = match self
            .local_filter
            .deploy(&request, &cfg.filter.repo_url, cfg.https_only)
            .await
        {
            Ok(outcome) => outcome,
            Err(err) => {
                let head_after = self.local_filter.current_repo_head_sha().await;
                if head_after != head_before {
                    if let Ok(_workspace) = self.lock_workspace() {
                        let mut cache = self.filter_cache.lock().unwrap();
                        let _ = self.db.clear_filter_verdicts();
                        cache.generation += 1;
                    }
                    self.emit_workspace_update();
                }
                return Err(err);
            }
        };

        // Persist deployment settings and clear prior verdicts for retained installations.
        let workspace = self.lock_workspace()?;
        let mut updated_cfg = self.config();
        let mut changed = false;
        let requested_repo_url = request
            .repo_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if requested_repo_url.is_some() {
            if let Ok(canonical_url) =
                validate_repo_url(&outcome.validated_repo_url, updated_cfg.https_only)
            {
                if updated_cfg.filter.repo_url != canonical_url {
                    updated_cfg.filter.repo_url = canonical_url;
                    changed = true;
                }
            }
        }
        if updated_cfg.filter.track_latest != request.track_latest {
            updated_cfg.filter.track_latest = request.track_latest;
            changed = true;
        }
        let next_pinned = if request.track_latest {
            None
        } else {
            request
                .target_commit
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        if updated_cfg.filter.pinned_commit != next_pinned {
            updated_cfg.filter.pinned_commit = next_pinned;
            changed = true;
        }
        let mut cache = self.filter_cache.lock().unwrap();
        if changed {
            save_config(&self.paths.settings_path, &updated_cfg)?;
            *self.config.write().unwrap() = updated_cfg;
        }
        self.db.clear_filter_verdicts()?;
        cache.generation += 1;
        let next_generation = cache.generation;
        drop(cache);
        drop(workspace);
        self.emit_workspace_update();

        if let Some(start_err) = outcome.start_error {
            return Err(start_err);
        }
        outcome.status.cache_generation = Some(next_generation);
        Ok(outcome.status)
    }

    pub async fn start_local_filter_service(
        &self,
    ) -> Result<LocalFilterDeploymentStatus, CoreError> {
        let mut status = self.local_filter.start_service().await?;
        status.cache_generation = Some(self.filter_cache.lock().unwrap().generation);
        Ok(status)
    }

    pub async fn stop_local_filter_service(
        &self,
    ) -> Result<LocalFilterDeploymentStatus, CoreError> {
        let mut status = self.local_filter.stop_service().await?;
        status.cache_generation = Some(self.filter_cache.lock().unwrap().generation);
        Ok(status)
    }

    pub fn get_filter_token_status(&self) -> Result<FilterTokenStatus, CoreError> {
        let (status, changed) = {
            let mut cache = self.filter_cache.lock().unwrap();
            let cfg = self.config().filter;
            let vault = self.token_vault.read().unwrap();
            let token = vault.resolve_token(LlmTokenSource::EnvVar, &cfg.llm_token_env_var);
            let changed = uses_env_token(&cfg) && cache.observe_env_token(&self.db, &token)?;
            let status = FilterTokenStatus {
                cache_generation: cache.generation,
                ..vault.status_with_env_var(&cfg.llm_token_env_var, token.is_ok())
            };
            (status, changed)
        };
        if changed {
            self.emit_workspace_update();
        }
        Ok(status)
    }

    pub fn set_filter_llm_token(
        &self,
        token: Option<String>,
    ) -> Result<FilterTokenStatus, CoreError> {
        let mut cache = self.filter_cache.lock().unwrap();
        let vault = self.token_vault.read().unwrap();
        match token.as_deref().map(str::trim) {
            Some(s) if !s.is_empty() => vault.seal(s)?,
            _ => vault.clear()?,
        }
        self.db.clear_llm_filter_verdicts()?;
        cache.generation += 1;
        let cfg = self.config();
        Ok(FilterTokenStatus {
            cache_generation: cache.generation,
            ..vault.status(&cfg.filter.llm_token_env_var)
        })
    }

    pub async fn evaluate_filter_items(
        &self,
        targets: Vec<FilterEvaluationTarget>,
    ) -> Result<FilterEvaluationBatchResponse, CoreError> {
        evaluate_targets(self, targets, false).await
    }

    pub async fn evaluate_automatic_filter_items(
        &self,
        targets: Vec<FilterEvaluationTarget>,
    ) -> Result<FilterEvaluationBatchResponse, CoreError> {
        evaluate_targets(self, targets, true).await
    }

    pub fn clear_filter_cache(&self) -> Result<FilterCacheClearResult, CoreError> {
        let mut cache = self.filter_cache.lock().unwrap();
        let count = self.db.clear_filter_verdicts()?;
        cache.generation += 1;
        if let Some(entry) = self.catalog_scoring_cache.lock().unwrap().as_mut() {
            entry.checked_at = None;
        }
        Ok(FilterCacheClearResult {
            count,
            cache_generation: cache.generation,
        })
    }

    pub fn upsert_filter_exclusion(
        &self,
        request: UpsertFilterExclusionRequest,
    ) -> Result<DashboardSnapshot, CoreError> {
        let workspace = self.lock_workspace()?;
        self.db.upsert_filter_exclusion(&request)?;
        let snapshot = self.snapshot_locked()?;
        drop(workspace);
        self.emit_workspace_update();
        Ok(snapshot)
    }

    pub fn delete_filter_exclusion(
        &self,
        addon_id: &AddonId,
    ) -> Result<DashboardSnapshot, CoreError> {
        let workspace = self.lock_workspace()?;
        self.db.delete_filter_exclusion(addon_id)?;
        let snapshot = self.snapshot_locked()?;
        drop(workspace);
        self.emit_workspace_update();
        Ok(snapshot)
    }

    pub(crate) fn emit_event(&self, event: CoreEvent) {
        self.events.emit(event);
    }
}
