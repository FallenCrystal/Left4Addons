use super::metadata::{parse_enum, serialize_enum};
use super::LocalDb;
use crate::error::CoreError;
use chrono::{DateTime, Utc};
use l4a_protocol::{
    AddonId, IssueId, ReconcileAction, ReconcileIssue, ReconcileIssueKind, TaskId, TaskKind,
    TaskRecord, TaskStatus, WorkshopId,
};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};

impl LocalDb {
    pub fn replace_open_reconcile_issues(
        &self,
        detected: &[(ReconcileIssue, String)],
    ) -> Result<(), CoreError> {
        self.write(|tx| Self::replace_open_reconcile_issues_tx(tx, detected))
    }

    pub(crate) fn replace_open_reconcile_issues_tx(
        tx: &Connection,
        detected: &[(ReconcileIssue, String)],
    ) -> Result<(), CoreError> {
        let mut resolved_fps: HashMap<String, String> = HashMap::new();
        {
            let mut stmt = tx
                .prepare("SELECT id, state_fingerprint FROM reconcile_issues WHERE resolved = 1")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (id, fp) = row?;
                resolved_fps.insert(id, fp);
            }
        }

        let mut current_ids: HashSet<&str> = HashSet::with_capacity(detected.len());
        tx.execute("DELETE FROM reconcile_issues WHERE resolved = 0", [])?;

        for (issue, fp) in detected {
            current_ids.insert(issue.id.as_str());
            if let Some(prev_fp) = resolved_fps.get(issue.id.as_str()) {
                if prev_fp == fp {
                    continue;
                }
            }
            tx.execute(
                r#"
                INSERT OR REPLACE INTO reconcile_issues (
                    id, kind, workshop_id, addon_id, loading_path, workshop_path,
                    summary, recommended_action, available_actions_json, resolved, detected_at, state_fingerprint
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                "#,
                params![
                    issue.id.as_str(),
                    serialize_enum(&issue.kind),
                    issue.workshop_id.map(|w| w.as_u64() as i64),
                    issue.addon_id.as_ref().map(AddonId::as_str),
                    issue.loading_path,
                    issue.workshop_path,
                    issue.summary,
                    serialize_enum(&issue.recommended_action),
                    serde_json::to_string(&issue.available_actions).unwrap_or_else(|_| "[]".into()),
                    if issue.resolved { 1 } else { 0 },
                    issue.detected_at.to_rfc3339(),
                    fp,
                ],
            )?;
        }

        for old_id in resolved_fps.keys() {
            if !current_ids.contains(old_id.as_str()) {
                tx.execute(
                    "DELETE FROM reconcile_issues WHERE id = ?1",
                    params![old_id],
                )?;
            }
        }

        Ok(())
    }

    pub fn list_open_reconcile_issues(&self) -> Result<Vec<ReconcileIssue>, CoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, workshop_id, addon_id, loading_path, workshop_path, summary,
                    recommended_action, available_actions_json, resolved, detected_at, state_fingerprint
             FROM reconcile_issues WHERE resolved = 0 ORDER BY detected_at DESC, id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            let kind_str: String = r.get(1)?;
            let wid_raw: Option<i64> = r.get(2)?;
            let addon_raw: Option<String> = r.get(3)?;
            let rec_str: String = r.get(7)?;
            let avail_json: String = r.get(8)?;
            let det_str: String = r.get(10)?;
            Ok(ReconcileIssue {
                id: IssueId(format!(
                    "{}@{}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(11)?
                )),
                kind: parse_enum(&kind_str, ReconcileIssueKind::OrphanDummy),
                workshop_id: wid_raw.and_then(|v| WorkshopId::new(v as u64).ok()),
                addon_id: addon_raw.map(AddonId),
                loading_path: r.get(4)?,
                workshop_path: r.get(5)?,
                summary: r.get(6)?,
                recommended_action: parse_enum(&rec_str, ReconcileAction::Ignore),
                available_actions: serde_json::from_str(&avail_json).unwrap_or_default(),
                resolved: r.get::<_, i64>(9)? != 0,
                detected_at: DateTime::parse_from_rfc3339(&det_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn mark_issue_resolved(&self, id: &IssueId) -> Result<(), CoreError> {
        self.write(|tx| Self::mark_issue_resolved_tx(tx, id))
    }

    pub(crate) fn mark_issue_resolved_tx(conn: &Connection, id: &IssueId) -> Result<(), CoreError> {
        if let Some((key, fingerprint)) = id.as_str().rsplit_once('@') {
            let affected = conn.execute(
                "UPDATE reconcile_issues SET resolved = 1 WHERE id = ?1 AND state_fingerprint = ?2 AND resolved = 0",
                params![key, fingerprint],
            )?;
            if affected == 0 {
                return Err(CoreError::Conflict("对账问题已变化，请刷新后重试".into()));
            }
        } else {
            conn.execute(
                "UPDATE reconcile_issues SET resolved = 1 WHERE id = ?1",
                params![id.as_str()],
            )?;
        }
        Ok(())
    }

    pub fn save_task(&self, task: &TaskRecord) -> Result<(), CoreError> {
        let conn = self.lock_conn()?;
        Self::save_task_tx(&conn, task)
    }

    pub(crate) fn save_task_tx(conn: &Connection, task: &TaskRecord) -> Result<(), CoreError> {
        conn.execute(
            r#"
            INSERT INTO tasks (id, kind, status, target_id, target_title, progress_percent, message, error, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(id) DO UPDATE SET
                status = excluded.status,
                target_title = COALESCE(excluded.target_title, tasks.target_title),
                progress_percent = excluded.progress_percent,
                message = excluded.message,
                error = excluded.error,
                updated_at = excluded.updated_at
            "#,
            params![
                task.id.as_str(),
                serialize_enum(&task.kind),
                serialize_enum(&task.status),
                task.target_id,
                task.target_title,
                task.progress_percent as i64,
                task.message,
                task.error,
                task.created_at.to_rfc3339(),
                task.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_tasks(&self) -> Result<Vec<TaskRecord>, CoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, status, target_id, target_title, progress_percent, message, error, created_at, updated_at
             FROM tasks ORDER BY created_at DESC LIMIT 500",
        )?;
        let rows = stmt.query_map([], |r| {
            let kind_s: String = r.get(1)?;
            let status_s: String = r.get(2)?;
            let c_s: String = r.get(8)?;
            let u_s: String = r.get(9)?;
            Ok(TaskRecord {
                id: TaskId(r.get(0)?),
                kind: parse_enum(&kind_s, TaskKind::Scan),
                status: parse_enum(&status_s, TaskStatus::Queued),
                target_id: r.get(3)?,
                target_title: r.get(4)?,
                progress_percent: r.get::<_, i64>(5)?.clamp(0, 100) as u8,
                message: r.get(6)?,
                error: r.get(7)?,
                created_at: DateTime::parse_from_rfc3339(&c_s)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                updated_at: DateTime::parse_from_rfc3339(&u_s)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}
