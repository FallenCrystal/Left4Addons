use super::L4aEngine;
use crate::config::{normalize_filter_preset_params, save_config};
use crate::error::CoreError;
use crate::filter::deploy::{validate_repo_url, validate_target_commit};
use crate::filter::secret::validate_env_var_name;
use crate::filter::{evaluate_targets, TokenVault};
use l4a_protocol::{
    AddonId, ClientFilterConfig, CoreEvent, DashboardSnapshot, DeployLocalFilterRequest,
    FilterCacheClearResult, FilterEvaluationBatchResponse, FilterEvaluationTarget,
    FilterTokenStatus, LocalFilterDeploymentStatus, UpsertFilterExclusionRequest,
    DEFAULT_FILTER_REPO_URL, DEFAULT_LLM_ENDPOINT, DEFAULT_LLM_MODEL, DEFAULT_LLM_SYSTEM_PROMPT,
    DEFAULT_LLM_TOKEN_ENV_VAR,
};

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

    if cfg.llm_system_prompt.trim().is_empty() {
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

    pub async fn get_local_filter_status(&self) -> Result<LocalFilterDeploymentStatus, CoreError> {
        let mut status = self.local_filter.status().await;
        status.cache_generation = Some(*self.filter_cache_generation.lock().unwrap());
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
        status.cache_generation = Some(*self.filter_cache_generation.lock().unwrap());
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
                        let mut generation = self.filter_cache_generation.lock().unwrap();
                        let _ = self.db.clear_filter_verdicts();
                        *generation += 1;
                    }
                    self.emit_workspace_update();
                }
                return Err(err);
            }
        };

        // Persist updated deployment tracking settings back to config and clear prior model verdicts,
        // even when `start_after_deploy` fails after the new commit has already been checked out.
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
        let mut generation = self.filter_cache_generation.lock().unwrap();
        if changed {
            save_config(&self.paths.settings_path, &updated_cfg)?;
            *self.config.write().unwrap() = updated_cfg;
        }
        self.db.clear_filter_verdicts()?;
        *generation += 1;
        let next_generation = *generation;
        drop(generation);
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
        status.cache_generation = Some(*self.filter_cache_generation.lock().unwrap());
        Ok(status)
    }

    pub async fn stop_local_filter_service(
        &self,
    ) -> Result<LocalFilterDeploymentStatus, CoreError> {
        let mut status = self.local_filter.stop_service().await?;
        status.cache_generation = Some(*self.filter_cache_generation.lock().unwrap());
        Ok(status)
    }

    pub fn get_filter_token_status(&self) -> Result<FilterTokenStatus, CoreError> {
        let generation = self.filter_cache_generation.lock().unwrap();
        let cfg = self.config();
        let vault = self.token_vault.read().unwrap();
        Ok(FilterTokenStatus {
            cache_generation: *generation,
            ..vault.status(&cfg.filter.llm_token_env_var)
        })
    }

    pub fn set_filter_llm_token(
        &self,
        token: Option<String>,
    ) -> Result<FilterTokenStatus, CoreError> {
        let mut generation = self.filter_cache_generation.lock().unwrap();
        let vault = self.token_vault.read().unwrap();
        match token.as_deref().map(str::trim) {
            Some(s) if !s.is_empty() => vault.seal(s)?,
            _ => vault.clear()?,
        }
        self.db.clear_llm_filter_verdicts()?;
        *generation += 1;
        let cfg = self.config();
        Ok(FilterTokenStatus {
            cache_generation: *generation,
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
        let mut generation = self.filter_cache_generation.lock().unwrap();
        let count = self.db.clear_filter_verdicts()?;
        *generation += 1;
        if let Some(entry) = self.catalog_scoring_cache.lock().unwrap().as_mut() {
            entry.checked_at = None;
        }
        Ok(FilterCacheClearResult {
            count,
            cache_generation: *generation,
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
