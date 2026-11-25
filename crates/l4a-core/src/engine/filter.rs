use super::L4aEngine;
use crate::config::save_config;
use crate::error::CoreError;
use crate::filter::deploy::validate_target_commit;
use crate::filter::secret::validate_env_var_name;
use crate::filter::{evaluate_targets, TokenVault};
use l4a_protocol::{
    AddonId, ClientFilterConfig, CoreEvent, DashboardSnapshot, DeployLocalFilterRequest,
    FilterEvaluationTarget, FilterItemVerdict, FilterTokenStatus, LocalFilterDeploymentStatus,
    UpsertFilterExclusionRequest, DEFAULT_FILTER_REPO_URL, DEFAULT_LLM_ENDPOINT, DEFAULT_LLM_MODEL,
    DEFAULT_LLM_SYSTEM_PROMPT, DEFAULT_LLM_TOKEN_ENV_VAR,
};

pub(crate) fn sanitize_filter_config(cfg: &mut ClientFilterConfig) -> Result<(), CoreError> {
    cfg.local_params = cfg.local_params.sanitized();

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

    cfg.pinned_commit = match cfg
        .pinned_commit
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(commit) => Some(validate_target_commit(commit)?),
        None => None,
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

    let trimmed_env = cfg.llm_token_env_var.trim();
    cfg.llm_token_env_var = if trimmed_env.is_empty() {
        DEFAULT_LLM_TOKEN_ENV_VAR.to_string()
    } else {
        validate_env_var_name(trimmed_env)?.to_string()
    };

    if cfg.llm_system_prompt.trim().is_empty() {
        cfg.llm_system_prompt = DEFAULT_LLM_SYSTEM_PROMPT.to_string();
    }

    Ok(())
}

impl L4aEngine {
    pub fn set_token_vault(&self, vault: TokenVault) {
        *self.token_vault.write().unwrap() = vault;
    }

    pub async fn get_local_filter_status(&self) -> Result<LocalFilterDeploymentStatus, CoreError> {
        Ok(self.local_filter.status().await)
    }

    pub async fn check_local_filter_updates(
        &self,
    ) -> Result<LocalFilterDeploymentStatus, CoreError> {
        let cfg = self.config();
        self.local_filter
            .check_updates(&cfg.filter.repo_url, cfg.https_only)
            .await
    }

    pub async fn deploy_local_filter(
        &self,
        request: DeployLocalFilterRequest,
    ) -> Result<LocalFilterDeploymentStatus, CoreError> {
        let cfg = self.config();
        let status = self
            .local_filter
            .deploy(
                &request,
                &cfg.filter.repo_url,
                &cfg.filter.local_params,
                cfg.https_only,
            )
            .await?;

        // Persist updated deployment tracking settings back to config
        let workspace = self.lock_workspace()?;
        let mut updated_cfg = self.config();
        let mut changed = false;
        if let Some(ref repo_url) = request
            .repo_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
        {
            if &updated_cfg.filter.repo_url != repo_url {
                updated_cfg.filter.repo_url = repo_url.clone();
                changed = true;
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
        if changed {
            save_config(&self.paths.settings_path, &updated_cfg)?;
            *self.config.write().unwrap() = updated_cfg;
        }
        drop(workspace);
        if changed {
            self.emit_workspace_update();
        }

        Ok(status)
    }

    pub async fn start_local_filter_service(
        &self,
    ) -> Result<LocalFilterDeploymentStatus, CoreError> {
        let cfg = self.config();
        self.local_filter
            .start_service(&cfg.filter.local_params)
            .await
    }

    pub async fn stop_local_filter_service(
        &self,
    ) -> Result<LocalFilterDeploymentStatus, CoreError> {
        self.local_filter.stop_service().await
    }

    pub fn get_filter_token_status(&self) -> Result<FilterTokenStatus, CoreError> {
        let cfg = self.config();
        let vault = self.token_vault.read().unwrap();
        Ok(vault.status(&cfg.filter.llm_token_env_var))
    }

    pub fn set_filter_llm_token(
        &self,
        token: Option<String>,
    ) -> Result<FilterTokenStatus, CoreError> {
        let vault = self.token_vault.read().unwrap();
        match token.as_deref().map(str::trim) {
            Some(s) if !s.is_empty() => vault.seal(s)?,
            _ => vault.clear()?,
        }
        let cfg = self.config();
        Ok(vault.status(&cfg.filter.llm_token_env_var))
    }

    pub async fn evaluate_filter_items(
        &self,
        targets: Vec<FilterEvaluationTarget>,
    ) -> Result<Vec<FilterItemVerdict>, CoreError> {
        evaluate_targets(self, targets).await
    }

    pub fn clear_filter_cache(&self) -> Result<u64, CoreError> {
        self.db.clear_filter_verdicts()
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
