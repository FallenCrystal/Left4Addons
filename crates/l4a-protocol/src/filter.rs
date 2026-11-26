use crate::ids::{AddonId, WorkshopId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const DEFAULT_FILTER_REPO_URL: &str = "https://github.com/FallenCrystal/Left4Filters.git";
pub const CATALOG_FILTER_MAX_BODY_BYTES: usize = 48 * 1024 * 1024;
pub const CATALOG_FILTER_IMAGE_FETCH_FAILED: &str = "image_fetch_failed";
pub const FILTER_REMOTE_VERDICT_MAX_AGE_SECS: i64 = 3_600;
pub const DEFAULT_LLM_ENDPOINT: &str = "https://api.openai.com/v1";
pub const DEFAULT_LLM_MODEL: &str = "gpt-4o-mini";
pub const DEFAULT_LLM_TOKEN_ENV_VAR: &str = "L4A_LLM_API_KEY";
pub const DEFAULT_LLM_SYSTEM_PROMPT: &str = "You are a strict content moderation reviewer for Left 4 Dead 2 Steam Workshop addons. Flag items that contain explicit NSFW/sexualized content (nudity, lingerie/bikini fetishes, explicit sexual poses, R18 adult mods) or extreme/graphic gore closeup imagery/text beyond ordinary L4D2 gameplay UI, red fonts, maps, scripts, or standard clothed anime/survivor skins.";

fn default_true() -> bool {
    true
}

fn default_threads() -> u32 {
    4
}

fn default_max_images() -> u32 {
    2
}

fn default_unit_weight() -> f32 {
    1.0
}

fn default_flag_threshold() -> f32 {
    0.72
}

fn default_llm_threshold() -> f32 {
    0.20
}

fn default_modality_threshold() -> f32 {
    0.30
}

fn default_repo_url() -> String {
    DEFAULT_FILTER_REPO_URL.to_string()
}

fn default_llm_endpoint() -> String {
    DEFAULT_LLM_ENDPOINT.to_string()
}

fn default_llm_model() -> String {
    DEFAULT_LLM_MODEL.to_string()
}

fn default_llm_env_var() -> String {
    DEFAULT_LLM_TOKEN_ENV_VAR.to_string()
}

fn default_llm_system_prompt() -> String {
    DEFAULT_LLM_SYSTEM_PROMPT.to_string()
}

fn default_catalog_filter_cache_ttl() -> u64 {
    3600
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FilterGalleryAggregation {
    #[default]
    Max,
    Top2Mean,
    Mean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FilterPresetId {
    #[default]
    Recommended,
    Strict,
    Relaxed,
    NsfwOnly,
    GoreOnly,
    LowCpu,
    Custom,
}

impl FilterPresetId {
    pub fn default_params(self) -> FilterModelParams {
        match self {
            Self::Recommended | Self::Custom => FilterModelParams::default(),
            Self::Strict => FilterModelParams {
                threads: 4,
                enable_text: true,
                enable_image: true,
                max_images: 4,
                gallery_aggregation: FilterGalleryAggregation::Max,
                gore_weight: 1.15,
                nsfw_weight: 1.15,
                temperature: 0.90,
                bias: 0.35,
            },
            Self::Relaxed => FilterModelParams {
                threads: 4,
                enable_text: true,
                enable_image: true,
                max_images: 1,
                gallery_aggregation: FilterGalleryAggregation::Max,
                gore_weight: 0.90,
                nsfw_weight: 0.90,
                temperature: 1.15,
                bias: -0.30,
            },
            Self::NsfwOnly => FilterModelParams {
                gore_weight: 0.0,
                nsfw_weight: 1.0,
                ..FilterModelParams::default()
            },
            Self::GoreOnly => FilterModelParams {
                gore_weight: 1.0,
                nsfw_weight: 0.0,
                ..FilterModelParams::default()
            },
            Self::LowCpu => FilterModelParams {
                threads: 2,
                max_images: 1,
                ..FilterModelParams::default()
            },
        }
    }

    pub fn recommended_thresholds(self) -> (f32, f32) {
        match self {
            Self::Strict => (0.15, 0.55),
            Self::Relaxed => (0.30, 0.82),
            _ => (0.20, 0.72),
        }
    }
}

/// Model runtime & calibration parameters shared by Client Local Deployment and Catalog Server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterModelParams {
    #[serde(default = "default_threads")]
    pub threads: u32,
    #[serde(default = "default_true")]
    pub enable_text: bool,
    #[serde(default = "default_true")]
    pub enable_image: bool,
    #[serde(default = "default_max_images")]
    pub max_images: u32,
    #[serde(default)]
    pub gallery_aggregation: FilterGalleryAggregation,
    #[serde(default = "default_unit_weight")]
    pub gore_weight: f32,
    #[serde(default = "default_unit_weight")]
    pub nsfw_weight: f32,
    #[serde(default = "default_unit_weight")]
    pub temperature: f32,
    #[serde(default)]
    pub bias: f32,
}

impl Default for FilterModelParams {
    fn default() -> Self {
        Self {
            threads: default_threads(),
            enable_text: true,
            enable_image: true,
            max_images: default_max_images(),
            gallery_aggregation: FilterGalleryAggregation::Max,
            gore_weight: 1.0,
            nsfw_weight: 1.0,
            temperature: 1.0,
            bias: 0.0,
        }
    }
}

impl FilterModelParams {
    pub fn sanitized(&self) -> Self {
        Self {
            threads: self.threads.clamp(1, 16),
            enable_text: self.enable_text,
            enable_image: self.enable_image,
            max_images: self.max_images.clamp(1, 8),
            gallery_aggregation: self.gallery_aggregation,
            gore_weight: if self.gore_weight.is_finite() {
                self.gore_weight.clamp(0.0, 3.0)
            } else {
                1.0
            },
            nsfw_weight: if self.nsfw_weight.is_finite() {
                self.nsfw_weight.clamp(0.0, 3.0)
            } else {
                1.0
            },
            temperature: if self.temperature.is_finite() {
                self.temperature.clamp(0.25, 4.0)
            } else {
                1.0
            },
            bias: if self.bias.is_finite() {
                self.bias.clamp(-5.0, 5.0)
            } else {
                0.0
            },
        }
    }

    pub fn is_identity_calibration(&self) -> bool {
        let s = self.sanitized();
        (s.gore_weight - 1.0).abs() < 1e-5
            && (s.nsfw_weight - 1.0).abs() < 1e-5
            && (s.temperature - 1.0).abs() < 1e-5
            && s.bias.abs() < 1e-5
    }

    pub fn adjust_triplet(&self, ov: f32, gore: f32, nsfw: f32) -> (f32, f32, f32) {
        let s = self.sanitized();
        if s.is_identity_calibration() {
            return (round6(ov), round6(gore), round6(nsfw));
        }
        let gw = s.gore_weight;
        let nw = s.nsfw_weight;
        let temp = s.temperature;
        let bias = s.bias;

        let adj_single = |p: f32, w: f32| -> f32 {
            if w <= 1e-6 {
                0.0
            } else {
                from_logit((to_logit(p) + w.ln()) / temp + bias)
            }
        };

        let g_adj = adj_single(gore, gw);
        let n_adj = adj_single(nsfw, nw);

        let ov_adj = if gw <= 1e-6 && nw <= 1e-6 {
            0.0
        } else if gw <= 1e-6 {
            n_adj
        } else if nw <= 1e-6 {
            g_adj
        } else if (gw - 1.0).abs() < 1e-5 && (nw - 1.0).abs() < 1e-5 {
            from_logit(to_logit(ov) / temp + bias)
        } else {
            let g_w = from_logit(to_logit(gore) + gw.ln());
            let n_w = from_logit(to_logit(nsfw) + nw.ln());
            let orig_or = clamp_prob(1.0 - (1.0 - clamp_prob(gore)) * (1.0 - clamp_prob(nsfw)));
            let new_or = clamp_prob(1.0 - (1.0 - g_w) * (1.0 - n_w));
            let delta_scale = gw.min(nw) / gw.max(nw).max(1.0);
            let delta_logit = (to_logit(ov) - to_logit(orig_or)) * delta_scale;
            from_logit((to_logit(new_or) + delta_logit) / temp + bias)
        };

        (round6(ov_adj), round6(g_adj), round6(n_adj))
    }

    pub fn adjust_scalar(&self, p: f32) -> f32 {
        let s = self.sanitized();
        let max_w = s.gore_weight.max(s.nsfw_weight);
        if max_w <= 1e-6 {
            return 0.0;
        }
        if s.is_identity_calibration() {
            return round6(p);
        }
        round6(from_logit(
            (to_logit(p) + max_w.ln()) / s.temperature + s.bias,
        ))
    }

    pub fn adjust_probabilities(&self, raw: &FilterProbabilities) -> FilterProbabilities {
        if self.is_identity_calibration() {
            return raw.clone();
        }
        let (prob, gore_prob, nsfw_prob) =
            self.adjust_triplet(raw.prob, raw.gore_prob, raw.nsfw_prob);

        let (text_prob, text_gore_prob, text_nsfw_prob) =
            match (raw.text_prob, raw.text_gore_prob, raw.text_nsfw_prob) {
                (Some(tp), Some(tg), Some(tn)) => {
                    let (a, b, c) = self.adjust_triplet(tp, tg, tn);
                    (Some(a), Some(b), Some(c))
                }
                (Some(tp), tg, tn) => (Some(self.adjust_scalar(tp)), tg, tn),
                _ => (None, None, None),
            };

        let (image_prob, image_gore_prob, image_nsfw_prob) =
            match (raw.image_prob, raw.image_gore_prob, raw.image_nsfw_prob) {
                (Some(ip), Some(ig), Some(in_)) => {
                    let (a, b, c) = self.adjust_triplet(ip, ig, in_);
                    (Some(a), Some(b), Some(c))
                }
                (Some(ip), ig, in_) => (Some(self.adjust_scalar(ip)), ig, in_),
                _ => (None, None, None),
            };

        FilterProbabilities {
            prob,
            gore_prob,
            nsfw_prob,
            text_prob,
            text_gore_prob,
            text_nsfw_prob,
            title_prob: raw.title_prob.map(|v| self.adjust_scalar(v)),
            description_prob: raw.description_prob.map(|v| self.adjust_scalar(v)),
            image_prob,
            image_gore_prob,
            image_nsfw_prob,
            per_image_probs: raw
                .per_image_probs
                .iter()
                .map(|v| self.adjust_scalar(*v))
                .collect(),
            scoring_revision: raw.scoring_revision.clone(),
        }
    }
}

fn clamp_prob(p: f32) -> f32 {
    if !p.is_finite() {
        return 1e-6;
    }
    p.clamp(1e-6, 1.0 - 1e-6)
}

fn to_logit(p: f32) -> f32 {
    let c = clamp_prob(p);
    (c / (1.0 - c)).ln()
}

fn from_logit(x: f32) -> f32 {
    if x >= 40.0 {
        1.0
    } else if x <= -40.0 {
        0.0
    } else {
        1.0 / (1.0 + (-x).exp())
    }
}

fn round6(v: f32) -> f32 {
    if !v.is_finite() {
        return 0.0;
    }
    ((v.clamp(0.0, 1.0) as f64) * 1_000_000.0).round() as f32 / 1_000_000.0
}

/// Pure calibrated sensitivity probabilities in `[0, 1]` (with optional unimodal breakdown).
/// Returned by both `Left4Filters` runner and Catalog Server `/api/v1/filter/predict`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FilterProbabilities {
    pub prob: f32,
    #[serde(alias = "gore_prob", default)]
    pub gore_prob: f32,
    #[serde(alias = "nsfw_prob", default)]
    pub nsfw_prob: f32,
    #[serde(alias = "text_prob", default, skip_serializing_if = "Option::is_none")]
    pub text_prob: Option<f32>,
    #[serde(
        alias = "text_gore_prob",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub text_gore_prob: Option<f32>,
    #[serde(
        alias = "text_nsfw_prob",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub text_nsfw_prob: Option<f32>,
    #[serde(alias = "title_prob", default, skip_serializing_if = "Option::is_none")]
    pub title_prob: Option<f32>,
    #[serde(
        alias = "description_prob",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub description_prob: Option<f32>,
    #[serde(alias = "image_prob", default, skip_serializing_if = "Option::is_none")]
    pub image_prob: Option<f32>,
    #[serde(
        alias = "image_gore_prob",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub image_gore_prob: Option<f32>,
    #[serde(
        alias = "image_nsfw_prob",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub image_nsfw_prob: Option<f32>,
    #[serde(
        alias = "per_image_probs",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub per_image_probs: Vec<f32>,
    #[serde(
        alias = "scoring_revision",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub scoring_revision: Option<String>,
}

/// Private scoring response from the managed runner. Raw rows are used only to
/// combine byte-budget subbatches and never serialized by the public Catalog API.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedFilterPrediction {
    #[serde(flatten)]
    pub probabilities: FilterProbabilities,
    #[serde(default)]
    pub image_rows: Vec<[f32; 3]>,
    #[serde(default)]
    pub joint_rows: Vec<[f32; 3]>,
}

pub fn aggregate_gallery_triplets(rows: &[[f32; 3]], mode: FilterGalleryAggregation) -> [f32; 3] {
    if rows.is_empty() {
        return [0.0; 3];
    }
    std::array::from_fn(|col| match mode {
        FilterGalleryAggregation::Max => rows.iter().map(|r| r[col]).fold(0.0, f32::max),
        FilterGalleryAggregation::Mean => {
            rows.iter().map(|r| r[col]).sum::<f32>() / rows.len() as f32
        }
        FilterGalleryAggregation::Top2Mean => {
            let mut values: Vec<_> = rows.iter().map(|r| r[col]).collect();
            values.sort_unstable_by(|a, b| b.total_cmp(a));
            let top = &values[..values.len().min(2)];
            top.iter().sum::<f32>() / top.len() as f32
        }
    })
}

pub fn merge_managed_predictions(
    mut parts: Vec<(usize, ManagedFilterPrediction)>,
    params: &FilterModelParams,
) -> Option<FilterProbabilities> {
    let has_missing_rows = parts
        .iter()
        .any(|(count, part)| part.image_rows.len() != *count || part.joint_rows.len() != *count);
    if has_missing_rows && parts.len() > 1 {
        return None;
    }
    let mut probabilities = std::mem::take(&mut parts.first_mut()?.1.probabilities);
    if parts.len() == 1 && (parts[0].0 == 0 || has_missing_rows) {
        return Some(probabilities);
    }
    let mut image_rows = Vec::new();
    let mut joint_rows = Vec::new();
    for (_, part) in parts {
        image_rows.extend(part.image_rows);
        joint_rows.extend(part.joint_rows);
    }
    let [ov, g, n] = aggregate_gallery_triplets(&joint_rows, params.gallery_aggregation);
    (
        probabilities.prob,
        probabilities.gore_prob,
        probabilities.nsfw_prob,
    ) = params.adjust_triplet(ov, g, n);
    let [ov, g, n] = aggregate_gallery_triplets(&image_rows, params.gallery_aggregation);
    let (ov, g, n) = params.adjust_triplet(ov, g, n);
    probabilities.image_prob = Some(ov);
    probabilities.image_gore_prob = Some(g);
    probabilities.image_nsfw_prob = Some(n);
    probabilities.per_image_probs = image_rows
        .into_iter()
        .map(|[ov, g, n]| params.adjust_triplet(ov, g, n).0)
        .collect();
    Some(probabilities)
}

impl FilterProbabilities {
    pub fn score_for(&self, target: FilterScoreTarget) -> f32 {
        match target {
            FilterScoreTarget::Overall => self.prob,
            FilterScoreTarget::Gore => self.gore_prob,
            FilterScoreTarget::Nsfw => self.nsfw_prob,
            FilterScoreTarget::MaxCategory => self.gore_prob.max(self.nsfw_prob),
        }
    }
}

/// Public request contract for Catalog `/api/v1/filter/predict` (and admin test endpoint).
/// Intentionally contains NO model parameter overrides — parameters are strictly server-controlled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FilterPredictRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workshop_id: Option<WorkshopId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(
        alias = "image_base64",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub image_base64: Option<String>,
    #[serde(
        alias = "images_base64",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub images_base64: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_urls: Vec<String>,
}

/// Wire request DTO for raw `Left4Filters` `/predict` endpoints (`deploy/server.py` & `filter_runner.py`),
/// which expect `snake_case` image fields (`image_base64`, `images_base64`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct RawFilterPredictRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images_base64: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogFilterPublicInfo {
    pub enabled: bool,
    pub ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_version: Option<String>,
    pub max_images: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scoring_revision: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CatalogFilterBackendKind {
    #[default]
    ManagedPython,
    UpstreamHttp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogFilterConfigDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub backend: CatalogFilterBackendKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deploy_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_bin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_url: Option<String>,
    #[serde(default = "default_catalog_filter_cache_ttl")]
    pub cache_ttl_secs: u64,
    #[serde(default)]
    pub params: FilterModelParams,
}

impl Default for CatalogFilterConfigDto {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: CatalogFilterBackendKind::ManagedPython,
            deploy_dir: None,
            python_bin: None,
            upstream_url: None,
            cache_ttl_secs: default_catalog_filter_cache_ttl(),
            params: FilterModelParams::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CatalogFilterStatus {
    pub config: CatalogFilterConfigDto,
    pub ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_version: Option<String>,
    pub total_requests: u64,
    pub cache_hits: u64,
    pub failed_requests: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

// ============================================================================
// Desktop Client Filter Configuration, Deployment, LLM Escalation & Exclusions
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClientFilterSourceMode {
    #[default]
    Local,
    Catalog,
    CustomEndpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FilterScoreTarget {
    #[default]
    Overall,
    Gore,
    Nsfw,
    MaxCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LlmTokenSource {
    #[default]
    EnvVar,
    EncryptedLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FlaggedItemAction {
    MarkOnly,
    HideImage,
    #[default]
    BlurCard,
    HideCard,
}

/// Progressive display levels for Library Exclusions (`排除项展示页面等级`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionDisplayLevel {
    Normal,
    #[default]
    LazyLoad,
    BlurImage,
    HideImage,
    HideText,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientFilterConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub mode: ClientFilterSourceMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_endpoint_url: Option<String>,
    #[serde(default = "default_true")]
    pub apply_to_workshop: bool,
    #[serde(default = "default_true")]
    pub apply_to_library: bool,

    // Local Git + venv deployment settings
    #[serde(default = "default_repo_url")]
    pub repo_url: String,
    #[serde(default = "default_true")]
    pub track_latest: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_commit: Option<String>,
    #[serde(default = "default_true")]
    pub auto_start_local_service: bool,
    #[serde(default)]
    pub preset: FilterPresetId,
    #[serde(default)]
    pub local_params: FilterModelParams,

    // Thresholds & OpenAI Chat Completions LLM judge settings
    #[serde(default)]
    pub score_target: FilterScoreTarget,
    #[serde(default = "default_true")]
    pub enable_direct_flag: bool,
    #[serde(default = "default_flag_threshold")]
    pub flag_threshold: f32,
    #[serde(default)]
    pub llm_enabled: bool,
    #[serde(default = "default_llm_threshold")]
    pub llm_threshold: f32,
    #[serde(default = "default_llm_endpoint")]
    pub llm_endpoint: String,
    #[serde(default = "default_llm_model")]
    pub llm_model: String,
    #[serde(default = "default_true")]
    pub llm_send_images: bool,
    #[serde(default = "default_llm_system_prompt")]
    pub llm_system_prompt: String,
    #[serde(default)]
    pub llm_token_source: LlmTokenSource,
    #[serde(default = "default_llm_env_var")]
    pub llm_token_env_var: String,

    // Flagged UI actions & unimodal subdivision
    #[serde(default)]
    pub flagged_action: FlaggedItemAction,
    #[serde(default = "default_true")]
    pub subdivide_modalities: bool,
    #[serde(default = "default_modality_threshold")]
    pub modality_threshold: f32,
}

impl Default for ClientFilterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: ClientFilterSourceMode::Local,
            custom_endpoint_url: None,
            apply_to_workshop: true,
            apply_to_library: true,
            repo_url: default_repo_url(),
            track_latest: true,
            pinned_commit: None,
            auto_start_local_service: true,
            preset: FilterPresetId::Recommended,
            local_params: FilterModelParams::default(),
            score_target: FilterScoreTarget::Overall,
            enable_direct_flag: true,
            flag_threshold: default_flag_threshold(),
            llm_enabled: false,
            llm_threshold: default_llm_threshold(),
            llm_endpoint: default_llm_endpoint(),
            llm_model: default_llm_model(),
            llm_send_images: true,
            llm_system_prompt: default_llm_system_prompt(),
            llm_token_source: LlmTokenSource::EnvVar,
            llm_token_env_var: default_llm_env_var(),
            flagged_action: FlaggedItemAction::BlurCard,
            subdivide_modalities: true,
            modality_threshold: default_modality_threshold(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterTokenStatus {
    pub has_encrypted_token: bool,
    pub env_var_name: String,
    pub env_var_present: bool,
    #[serde(default)]
    pub cache_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterCacheClearResult {
    pub count: u64,
    pub cache_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FilterEnvReport {
    pub git_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_path: Option<String>,
    pub python_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_path: Option<String>,
    pub venv_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterCommitInfo {
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub committed_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LocalFilterServiceState {
    #[default]
    Stopped,
    Deploying,
    Starting,
    Running,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocalFilterDeploymentStatus {
    pub env: FilterEnvReport,
    pub deployed: bool,
    pub repo_dir: String,
    pub venv_dir: String,
    pub service_state: LocalFilterServiceState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_commit: Option<FilterCommitInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_latest_commit: Option<FilterCommitInfo>,
    pub update_available: bool,
    #[serde(default)]
    pub recent_commits: Vec<FilterCommitInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_generation: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeployLocalFilterRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    #[serde(default = "default_true")]
    pub track_latest: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_commit: Option<String>,
    #[serde(default = "default_true")]
    pub start_after_deploy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterExclusionRecord {
    pub addon_id: AddonId,
    pub display_level: ExclusionDisplayLevel,
    pub bind_fingerprint: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_fingerprint: Option<String>,
    pub fingerprint_valid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertFilterExclusionRequest {
    pub addon_id: AddonId,
    pub display_level: ExclusionDisplayLevel,
    /// An omitted value preserves the existing binding on a display-only edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_fingerprint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterDecisionSource {
    ModelSafe,
    ModelDirectFlag,
    LlmFlaggedTrue,
    LlmFlaggedFalse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterEvaluationTarget {
    /// Stable client-side target identifier (`addon:<AddonId>` or `ws:<WorkshopId>`).
    pub target_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addon_id: Option<AddonId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workshop_id: Option<WorkshopId>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Ordered image sources: remote URLs (`https://...`), local cache keys (`addon_cover_...`), or data URLs.
    #[serde(default)]
    pub image_sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_hint: Option<String>,
    #[serde(default)]
    pub bypass_cache: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterItemVerdict {
    pub target_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_hint: Option<String>,
    pub flagged: bool,
    pub decision_source: FilterDecisionSource,
    pub evaluated_score: f32,
    pub probabilities: FilterProbabilities,
    pub text_sensitive: bool,
    pub title_sensitive: bool,
    pub description_sensitive: bool,
    pub image_sensitive: bool,
    /// Original slot indices in `FilterEvaluationTarget.image_sources` that were actually evaluated.
    #[serde(default)]
    pub evaluated_image_indices: Vec<usize>,
    /// Original slot indices in `FilterEvaluationTarget.image_sources` marked sensitive.
    #[serde(default)]
    pub sensitive_image_indices: Vec<usize>,
    /// Whether all selected media were scored and the verdict can be reused.
    #[serde(default = "default_true")]
    pub complete: bool,
    pub evaluated_at: DateTime<Utc>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum FilterTargetOutcome {
    Completed {
        verdict: FilterItemVerdict,
        complete: bool,
    },
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterEvaluationItemResult {
    pub target_key: String,
    #[serde(flatten)]
    pub outcome: FilterTargetOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterEvaluationBatchResponse {
    pub policy_signature: String,
    #[serde(default)]
    pub cache_generation: u64,
    pub items: Vec<FilterEvaluationItemResult>,
}

impl ClientFilterConfig {
    /// Canonical deterministic policy identifier shared across native engine, IPC events, and UI.
    pub fn canonical_policy_signature(
        &self,
        catalog_server_url: Option<&str>,
        https_only: bool,
        catalog_scoring_revision: Option<&str>,
    ) -> String {
        let p = self.local_params.sanitized();
        let mode_part = match self.mode {
            ClientFilterSourceMode::Local => format!(
                "local:txt={}:img={}:max={}:agg={:?}:gw={:.4}:nw={:.4}:t={:.4}:b={:.4}:commit={}",
                p.enable_text,
                p.enable_image,
                p.max_images,
                p.gallery_aggregation,
                p.gore_weight,
                p.nsfw_weight,
                p.temperature,
                p.bias,
                if self.track_latest {
                    "latest"
                } else {
                    self.pinned_commit.as_deref().unwrap_or("latest")
                }
            ),
            ClientFilterSourceMode::Catalog => format!(
                "catalog:url={}:https={}:rev={}",
                catalog_server_url
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches('/'),
                https_only,
                catalog_scoring_revision.unwrap_or("unverified")
            ),
            ClientFilterSourceMode::CustomEndpoint => format!(
                "custom:url={}:https={}:max={}",
                self.custom_endpoint_url
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches('/'),
                https_only,
                p.max_images
            ),
        };
        let llm_part = if self.llm_enabled {
            format!(
                "1:t={:.4}:ep={}:m={}:img={}:sys={}:token={}:env={}",
                self.llm_threshold,
                self.llm_endpoint.trim().trim_end_matches('/'),
                self.llm_model.trim(),
                self.llm_send_images,
                fnv1a_hex(self.llm_system_prompt.trim().as_bytes()),
                match self.llm_token_source {
                    LlmTokenSource::EnvVar => "env_var",
                    LlmTokenSource::EncryptedLocal => "encrypted_local",
                },
                if self.llm_token_source == LlmTokenSource::EnvVar {
                    self.llm_token_env_var.trim()
                } else {
                    ""
                }
            )
        } else {
            "0".to_string()
        };
        let raw = format!(
            "v3|{mode_part}|st={:?}|df={}:{:.4}|llm={llm_part}|sub={}:{:.4}",
            self.score_target,
            self.enable_direct_flag,
            self.flag_threshold,
            self.subdivide_modalities,
            self.modality_threshold
        );
        format!("pol_{}", fnv1a_hex(raw.as_bytes()))
    }
}

pub fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut h1: u64 = 0xcbf29ce484222325;
    let mut h2: u64 = 0x84222325cbf29ce4;
    for (idx, &b) in bytes.iter().enumerate() {
        h1 ^= u64::from(b);
        h1 = h1.wrapping_mul(0x100000001b3);
        h2 ^= u64::from(b.rotate_left((idx % 7) as u32));
        h2 = h2.wrapping_mul(0x100000001b3);
    }
    format!("{h1:016x}{h2:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_params_preserve_probabilities() {
        let params = FilterModelParams::default();
        assert!(params.is_identity_calibration());
        let raw = FilterProbabilities {
            prob: 0.721234,
            gore_prob: 0.013712,
            nsfw_prob: 0.718001,
            text_prob: Some(0.025),
            text_gore_prob: Some(0.01),
            text_nsfw_prob: Some(0.015),
            title_prob: Some(0.02),
            description_prob: Some(0.03),
            image_prob: Some(0.719),
            image_gore_prob: Some(0.012),
            image_nsfw_prob: Some(0.715),
            per_image_probs: vec![0.719, 0.018],
            scoring_revision: Some("rev1".into()),
        };
        let adj = params.adjust_probabilities(&raw);
        assert_eq!(adj, raw);
    }

    #[test]
    fn nsfw_only_preset_zeroes_gore_and_tracks_nsfw() {
        let params = FilterPresetId::NsfwOnly.default_params();
        let (ov, g, n) = params.adjust_triplet(0.95, 0.94, 0.002);
        assert_eq!(g, 0.0);
        assert!(
            ov < 0.01,
            "pure gore item should drop to safe in NsfwOnly: {ov}"
        );
        assert!((ov - n).abs() < 1e-5);

        let (ov2, g2, n2) = params.adjust_triplet(0.88, 0.01, 0.87);
        assert_eq!(g2, 0.0);
        assert!(n2 > 0.85);
        assert_eq!(ov2, n2);
    }

    #[test]
    fn raw_filter_predict_request_serializes_snake_case_image_fields() {
        let req = RawFilterPredictRequest {
            title: Some("Addon".into()),
            description: None,
            tags: vec!["UI".into()],
            image_base64: Some("AAA=".into()),
            images_base64: vec!["BBB=".into()],
        };
        let encoded = serde_json::to_value(&req).unwrap();
        assert_eq!(encoded["image_base64"], "AAA=");
        assert_eq!(encoded["images_base64"], serde_json::json!(["BBB="]));
        assert!(encoded.get("imageBase64").is_none());
        assert!(encoded.get("imagesBase64").is_none());
    }

    #[test]
    fn canonical_policy_signature_tracks_catalog_scoring_revision() {
        let cfg = ClientFilterConfig {
            enabled: true,
            mode: ClientFilterSourceMode::Catalog,
            ..Default::default()
        };
        let sig_a = cfg.canonical_policy_signature(Some("https://cat.example"), true, Some("r1"));
        let sig_b = cfg.canonical_policy_signature(Some("https://cat.example"), true, Some("r2"));
        assert_ne!(sig_a, sig_b);
    }

    #[test]
    fn canonical_policy_signature_ignores_pinned_commit_when_track_latest_true() {
        let base = ClientFilterConfig {
            enabled: true,
            mode: ClientFilterSourceMode::Local,
            track_latest: true,
            pinned_commit: None,
            ..Default::default()
        };
        let with_stale_pin = ClientFilterConfig {
            pinned_commit: Some("deadbeef1234".into()),
            ..base.clone()
        };
        let pinned = ClientFilterConfig {
            track_latest: false,
            pinned_commit: Some("deadbeef1234".into()),
            ..base.clone()
        };
        assert_eq!(
            base.canonical_policy_signature(None, true, None),
            with_stale_pin.canonical_policy_signature(None, true, None)
        );
        assert_ne!(
            base.canonical_policy_signature(None, true, None),
            pinned.canonical_policy_signature(None, true, None)
        );
    }
}
