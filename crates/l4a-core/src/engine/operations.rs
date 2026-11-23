use super::L4aEngine;
use crate::error::CoreError;
use crate::grouping::{
    apply_profile, create_manual_bundle, delete_bundle, delete_profile, delete_user_group,
    rebuild_auto_bundles, save_profile, save_user_group, set_bundle_locked,
    track_workshop_collection, untrack_workshop_collection,
};
use crate::ops::{
    apply_renames, delete_addons, move_addons, preview_batch_rename, search_by_map_code,
    toggle_addons,
};
use crate::reconcile::resolve_reconcile_issue;
use l4a_protocol::{
    AddonBatchReport, AddonDirType, AddonId, AddonSummary, BundleId, BundleRecord,
    DashboardSnapshot, GroupId, InstallState, IssueId, LoadoutProfile, ProfileApplyReport,
    ProfileId, ReconcileAction, ReconcileBatchItem, ReconcileBatchReport, ReconcileItemResult,
    RenamePreviewItem, RenameRuleConfig, TrackedCollectionRecord, UserGroup, WorkshopId,
};
use std::collections::{BTreeSet, HashSet};

impl L4aEngine {
    pub fn resolve_issue(
        &self,
        issue_id: &IssueId,
        action: ReconcileAction,
    ) -> Result<DashboardSnapshot, CoreError> {
        let workspace = self.lock_workspace()?;
        let cfg = self.config();
        self.suppress_watcher();
        let result = resolve_reconcile_issue(
            &self.db,
            &cfg,
            &self.paths.image_cache_dir,
            issue_id,
            action,
        );
        if result.is_err() {
            self.refresh_after_error(&cfg);
        } else {
            self.refresh_bundles_logged();
        }
        let snapshot = result.and_then(|()| self.snapshot_locked());
        drop(workspace);
        self.emit_workspace_update();
        snapshot
    }

    pub fn resolve_issues_batch(
        &self,
        items: &[ReconcileBatchItem],
    ) -> Result<ReconcileBatchReport, CoreError> {
        let workspace = self.lock_workspace()?;
        let cfg = self.config();
        self.suppress_watcher();

        let mut results = Vec::with_capacity(items.len());
        let mut any_ok = false;
        for item in items {
            let error = resolve_reconcile_issue(
                &self.db,
                &cfg,
                &self.paths.image_cache_dir,
                &item.issue_id,
                item.action,
            )
            .err()
            .map(|err| {
                self.refresh_after_error(&cfg);
                err.into()
            });
            if error.is_none() {
                any_ok = true;
            }
            results.push(ReconcileItemResult {
                issue_id: item.issue_id.clone(),
                action: item.action,
                ok: error.is_none(),
                error,
            });
        }
        if any_ok {
            self.refresh_bundles_logged();
        }

        let snapshot = self.snapshot_locked();
        drop(workspace);
        self.emit_workspace_update();
        Ok(ReconcileBatchReport {
            items: results,
            snapshot: snapshot?,
        })
    }

    pub fn move_addons(
        &self,
        addon_ids: &[AddonId],
        target_dir_type: AddonDirType,
    ) -> Result<AddonBatchReport, CoreError> {
        let workspace = self.lock_workspace()?;
        let cfg = self.config();
        self.suppress_watcher();
        let report = move_addons(
            &self.db,
            &cfg,
            &self.paths.image_cache_dir,
            addon_ids,
            target_dir_type,
        )?;
        if report.refresh_error.is_none() {
            self.refresh_bundles_logged();
        }
        drop(workspace);
        self.emit_workspace_update();
        Ok(report)
    }

    pub fn toggle_addons(
        &self,
        addon_ids: &[AddonId],
        enabled: bool,
        allow_workshop_dir: bool,
    ) -> Result<AddonBatchReport, CoreError> {
        let workspace = self.lock_workspace()?;
        let cfg = self.config();
        self.suppress_watcher();
        let report = toggle_addons(
            &self.db,
            &cfg,
            &self.paths.image_cache_dir,
            addon_ids.iter().map(|id| (id, enabled)),
            allow_workshop_dir,
        )?;
        drop(workspace);
        self.emit_workspace_update();
        Ok(report)
    }

    pub fn preview_rename(
        &self,
        rules: &RenameRuleConfig,
        addon_ids: &[AddonId],
    ) -> Result<Vec<RenamePreviewItem>, CoreError> {
        let _workspace = self.lock_workspace()?;
        preview_batch_rename(&self.db, &self.config(), rules, addon_ids)
    }

    pub fn apply_renames(
        &self,
        renames: &[(AddonId, String)],
    ) -> Result<AddonBatchReport, CoreError> {
        let workspace = self.lock_workspace()?;
        let cfg = self.config();
        self.suppress_watcher();
        let report = apply_renames(&self.db, &cfg, &self.paths.image_cache_dir, renames)?;
        if report.refresh_error.is_none() {
            self.refresh_bundles_logged();
        }
        drop(workspace);
        self.emit_workspace_update();
        Ok(report)
    }

    pub fn delete_addons(
        &self,
        addon_ids: &[AddonId],
        delete_disk_file: bool,
        purge_known_record: bool,
    ) -> Result<AddonBatchReport, CoreError> {
        let workspace = self.lock_workspace()?;
        let report = self.delete_addons_locked(addon_ids, delete_disk_file, purge_known_record)?;
        drop(workspace);
        self.emit_workspace_update();
        Ok(report)
    }

    pub fn safe_delete_obsolete_dependency(
        &self,
        workshop_id: WorkshopId,
        delete_disk_file: bool,
        purge_known_record: bool,
    ) -> Result<AddonBatchReport, CoreError> {
        let workspace = self.lock_workspace()?;
        let addons = self.db.list_addons()?;
        let installed: HashSet<_> = addons
            .iter()
            .filter(|addon| addon.install_state == InstallState::Installed)
            .filter_map(|addon| addon.workshop_id)
            .collect();
        let parents: BTreeSet<_> = self
            .db
            .list_all_active_dependency_edges()?
            .into_iter()
            .filter_map(|(parent, dep)| {
                (dep == workshop_id && installed.contains(&parent)).then_some(parent)
            })
            .collect();
        if !parents.is_empty() {
            let ids: Vec<_> = parents.into_iter().map(|id| id.to_string()).collect();
            return Err(CoreError::Conflict(format!(
                "已安装的工坊条目仍依赖 {workshop_id}: {}",
                ids.join(", ")
            )));
        }
        let addon = addons
            .iter()
            .find(|addon| addon.workshop_id == Some(workshop_id))
            .ok_or_else(|| CoreError::NotFound(format!("未找到模组: {workshop_id}")))?;
        let report = self.delete_addons_locked(
            std::slice::from_ref(&addon.id),
            delete_disk_file,
            purge_known_record,
        )?;
        drop(workspace);
        self.emit_workspace_update();
        Ok(report)
    }

    fn delete_addons_locked(
        &self,
        addon_ids: &[AddonId],
        delete_disk_file: bool,
        purge_known_record: bool,
    ) -> Result<AddonBatchReport, CoreError> {
        self.suppress_watcher();
        let report = delete_addons(
            &self.db,
            &self.config(),
            &self.paths.image_cache_dir,
            addon_ids,
            delete_disk_file,
            purge_known_record,
        )?;
        if report.refresh_error.is_none() {
            self.refresh_bundles_logged();
        }
        Ok(report)
    }

    pub fn rebuild_bundles(&self) -> Result<Vec<BundleRecord>, CoreError> {
        let workspace = self.lock_workspace()?;
        let res = rebuild_auto_bundles(&self.db)?;
        drop(workspace);
        self.emit_workspace_update();
        Ok(res)
    }

    pub fn create_manual_bundle(
        &self,
        name: String,
        member_addon_ids: Vec<AddonId>,
    ) -> Result<BundleRecord, CoreError> {
        create_manual_bundle(&self.db, name, member_addon_ids)
    }

    pub fn set_bundle_locked(&self, id: &BundleId, locked: bool) -> Result<(), CoreError> {
        set_bundle_locked(&self.db, id, locked)
    }

    pub fn delete_bundle(&self, id: &BundleId) -> Result<(), CoreError> {
        let workspace = self.lock_workspace()?;
        delete_bundle(&self.db, id)?;
        drop(workspace);
        self.emit_workspace_update();
        Ok(())
    }

    pub async fn track_collection(
        &self,
        collection_id: WorkshopId,
        auto_sync: bool,
    ) -> Result<TrackedCollectionRecord, CoreError> {
        let resolver = self.resolver.read().unwrap().clone();
        track_workshop_collection(&self.db, &resolver, collection_id, auto_sync).await
    }

    pub fn untrack_collection(&self, collection_id: WorkshopId) -> Result<(), CoreError> {
        untrack_workshop_collection(&self.db, collection_id)
    }

    pub fn save_user_group(&self, group: UserGroup) -> Result<UserGroup, CoreError> {
        save_user_group(&self.db, group)
    }

    pub fn delete_user_group(&self, id: &GroupId) -> Result<(), CoreError> {
        delete_user_group(&self.db, id)
    }

    pub fn save_profile(&self, profile: LoadoutProfile) -> Result<LoadoutProfile, CoreError> {
        save_profile(&self.db, profile)
    }

    pub fn delete_profile(&self, id: &ProfileId) -> Result<(), CoreError> {
        delete_profile(&self.db, id)
    }

    pub fn apply_profile(&self, id: &ProfileId) -> Result<ProfileApplyReport, CoreError> {
        let workspace = self.lock_workspace()?;
        let cfg = self.config();
        self.suppress_watcher();
        let report = apply_profile(&self.db, id, &self.paths.image_cache_dir, &cfg)?;
        drop(workspace);
        self.emit_workspace_update();
        Ok(report)
    }

    pub fn search_by_map_code(&self, query: &str) -> Result<Vec<AddonSummary>, CoreError> {
        search_by_map_code(&self.db, query)
    }
}
