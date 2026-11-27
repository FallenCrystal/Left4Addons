//! Operation audit log for IPC commands invoked from the desktop UI.
//!
//! Every user-facing operation is recorded under the `l4a::op` target with its parameters,
//! duration and outcome, so that a log file can reconstruct what the user did and what failed.

use l4a_protocol::{
    AddonBatchReport, AddonOperationOutcome, DesktopError, FilterEvaluationBatchResponse,
    FilterTargetOutcome, LocalFilterDeploymentStatus, ProfileApplyReport, ReconcileBatchReport,
    SearchResponse, SyncReport,
};
use std::future::Future;
use std::time::Instant;

pub(crate) const OP_TARGET: &str = "l4a::op";

/// Runs `fut`, logging start, duration, a result summary and any error.
pub(crate) async fn audited<T, E, F>(
    op: &'static str,
    detail: String,
    summarize: fn(&T) -> String,
    fut: F,
) -> Result<T, DesktopError>
where
    F: Future<Output = Result<T, E>>,
    E: Into<DesktopError>,
{
    tracing::info!(target: OP_TARGET, op, detail = %detail, "Operation started");
    let started = Instant::now();
    let result = fut.await.map_err(Into::into);
    let elapsed_ms = started.elapsed().as_millis() as u64;
    match &result {
        Ok(value) => tracing::info!(
            target: OP_TARGET,
            op,
            elapsed_ms,
            result = %summarize(value),
            "Operation completed"
        ),
        Err(err) => tracing::warn!(
            target: OP_TARGET,
            op,
            elapsed_ms,
            code = ?err.code,
            error = %err.message,
            "Operation failed"
        ),
    }
    result
}

pub(crate) fn no_summary<T>(_: &T) -> String {
    String::new()
}

pub(crate) fn batch_summary(report: &AddonBatchReport) -> String {
    let (mut applied, mut unchanged, mut failed, mut skipped) = (0, 0, 0, 0);
    let mut errors = Vec::new();
    for item in &report.items {
        match &item.outcome {
            AddonOperationOutcome::Applied => applied += 1,
            AddonOperationOutcome::Unchanged => unchanged += 1,
            AddonOperationOutcome::NotAttempted => skipped += 1,
            AddonOperationOutcome::Failed { error } => {
                failed += 1;
                if errors.len() < 5 {
                    errors.push(format!("{}: {}", item.addon_id.0, error.message));
                }
            }
        }
    }
    let mut out =
        format!("applied={applied} unchanged={unchanged} failed={failed} not_attempted={skipped}");
    if !errors.is_empty() {
        out.push_str(&format!(" errors=[{}]", errors.join("; ")));
    }
    if let Some(err) = &report.refresh_error {
        out.push_str(&format!(" refresh_error={}", err.message));
    }
    out
}

pub(crate) fn profile_summary(report: &ProfileApplyReport) -> String {
    format!(
        "enabled={} disabled={} skipped_workshop_only={} missing={} {}",
        report.enabled_count,
        report.disabled_count,
        report.skipped_workshop_only.len(),
        report.missing_addons.len(),
        batch_summary(&report.batch)
    )
}

pub(crate) fn reconcile_summary(report: &ReconcileBatchReport) -> String {
    let failed = report.items.iter().filter(|item| !item.ok).count();
    format!("items={} failed={failed}", report.items.len())
}

pub(crate) fn sync_summary(report: &SyncReport) -> String {
    format!(
        "checked={} updates={} removed_upstream={} dependency_diffs={} obsolete_dependencies={} collection_diffs={} warnings={}",
        report.checked_items,
        report.updates_available.len(),
        report.removed_upstream.len(),
        report.dependency_diffs.len(),
        report.obsolete_dependencies.len(),
        report.collection_diffs.len(),
        report.warnings.len()
    )
}

pub(crate) fn search_summary(response: &SearchResponse) -> String {
    format!(
        "items={} total={} page={} source={:?}",
        response.items.len(),
        response.total,
        response.page,
        response.source
    )
}

pub(crate) fn filter_status_summary(status: &LocalFilterDeploymentStatus) -> String {
    format!(
        "deployed={} web_gpu={} service={:?} endpoint={} pid={:?} commit={}",
        status.deployed,
        status.web_gpu_enabled,
        status.service_state,
        status.service_endpoint.as_deref().unwrap_or("-"),
        status.running_pid,
        status
            .current_commit
            .as_ref()
            .map(|commit| commit.sha.as_str())
            .unwrap_or("-")
    )
}

pub(crate) fn filter_batch_summary(response: &FilterEvaluationBatchResponse) -> String {
    let (mut flagged, mut safe, mut incomplete, mut failed) = (0, 0, 0, 0);
    for item in &response.items {
        match &item.outcome {
            FilterTargetOutcome::Completed { verdict, complete } => {
                if verdict.flagged {
                    flagged += 1;
                } else {
                    safe += 1;
                }
                if !complete {
                    incomplete += 1;
                }
            }
            FilterTargetOutcome::Failed { .. } => failed += 1,
        }
    }
    format!(
        "items={} flagged={flagged} safe={safe} incomplete={incomplete} failed={failed} cache_generation={}",
        response.items.len(),
        response.cache_generation
    )
}
