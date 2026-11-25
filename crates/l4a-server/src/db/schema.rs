use super::{
    has_column, has_table, ServerRepo, INSERT_SEARCH_DOCUMENT, STATE_BACKFILL_CURSOR,
    STATE_BACKFILL_SYNCED_TOTAL, STATE_INCREMENTAL_SCAN, STATE_SEARCH_INDEX_VERSION,
};
use crate::search::{self, SEARCH_INDEX_VERSION};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

impl ServerRepo {
    pub(super) fn migrate(&self) -> anyhow::Result<()> {
        self.with_transaction(Self::migrate_tx)
    }

    fn migrate_tx(conn: &Transaction<'_>) -> anyhow::Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS workshop_items (
                workshop_id INTEGER PRIMARY KEY,
                file_type TEXT NOT NULL,
                status TEXT NOT NULL,
                title TEXT NOT NULL,
                description TEXT,
                creator_steam_id TEXT,
                creator_name TEXT,
                preview_url TEXT,
                previews_json TEXT NOT NULL DEFAULT '[]',
                file_url TEXT,
                file_size INTEGER,
                time_created INTEGER,
                time_updated INTEGER,
                tags_json TEXT NOT NULL DEFAULT '[]',
                collection_children_json TEXT NOT NULL DEFAULT '[]',
                subscriptions INTEGER,
                favorites INTEGER,
                is_admin_override INTEGER NOT NULL DEFAULT 0,
                source TEXT NOT NULL,
                fetched_at TEXT NOT NULL,
                removed_at TEXT,
                last_verified_at TEXT NOT NULL,
                visibility_verified_at TEXT,
                is_listed INTEGER,
                steam_version INTEGER,
                catalog_indexed INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_workshop_items_status_updated ON workshop_items(status, time_updated DESC);
            CREATE INDEX IF NOT EXISTS idx_workshop_items_file_type ON workshop_items(file_type);
            CREATE INDEX IF NOT EXISTS idx_workshop_items_subscriptions ON workshop_items(subscriptions DESC);

            CREATE TABLE IF NOT EXISTS workshop_item_dependencies (
                parent_workshop_id INTEGER NOT NULL,
                dep_workshop_id INTEGER NOT NULL,
                dep_title TEXT,
                required INTEGER NOT NULL DEFAULT 1,
                status TEXT NOT NULL DEFAULT 'active',
                updated_at TEXT NOT NULL,
                PRIMARY KEY (parent_workshop_id, dep_workshop_id),
                FOREIGN KEY (parent_workshop_id) REFERENCES workshop_items(workshop_id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_dependencies_child ON workshop_item_dependencies(dep_workshop_id);

            CREATE TABLE IF NOT EXISTS workshop_item_revisions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                workshop_id INTEGER NOT NULL,
                status TEXT NOT NULL,
                title TEXT NOT NULL,
                time_updated INTEGER,
                dependencies_json TEXT NOT NULL,
                source TEXT NOT NULL,
                recorded_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_revisions_wid ON workshop_item_revisions(workshop_id, id DESC);

            CREATE TABLE IF NOT EXISTS audit_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                workshop_id INTEGER,
                actor TEXT NOT NULL,
                action TEXT NOT NULL,
                reason TEXT NOT NULL,
                status TEXT,
                time_updated INTEGER,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_audit_log_created ON audit_log(id DESC);

            CREATE TABLE IF NOT EXISTS sync_state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;

        if !has_column(conn, "workshop_items", "previews_json")? {
            conn.execute(
                "ALTER TABLE workshop_items ADD COLUMN previews_json TEXT NOT NULL DEFAULT '[]'",
                [],
            )?;
        }
        if !has_column(conn, "workshop_items", "is_admin_override")? {
            conn.execute(
                "ALTER TABLE workshop_items ADD COLUMN is_admin_override INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        conn.execute(
            "UPDATE workshop_items SET is_admin_override = 1
             WHERE source = 'admin_override' AND is_admin_override = 0",
            [],
        )?;
        if !has_column(conn, "workshop_items", "is_listed")? {
            conn.execute(
                "ALTER TABLE workshop_items ADD COLUMN is_listed INTEGER",
                [],
            )?;
            conn.execute(
                "UPDATE workshop_items SET is_listed = 1 WHERE is_admin_override = 1",
                [],
            )?;
        }
        if !has_column(conn, "workshop_items", "visibility_verified_at")? {
            conn.execute(
                "ALTER TABLE workshop_items ADD COLUMN visibility_verified_at TEXT",
                [],
            )?;
            conn.execute(
                "UPDATE workshop_items SET visibility_verified_at = last_verified_at WHERE is_listed IS NOT NULL",
                [],
            )?;
        }
        if !has_column(conn, "workshop_items", "steam_version")? {
            conn.execute(
                "ALTER TABLE workshop_items ADD COLUMN steam_version INTEGER",
                [],
            )?;
            conn.execute(
                "UPDATE workshop_items SET steam_version = time_updated WHERE is_admin_override = 0",
                [],
            )?;
        }
        if !has_column(conn, "workshop_items", "catalog_indexed")? {
            conn.execute(
                "ALTER TABLE workshop_items ADD COLUMN catalog_indexed INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
            // The legacy cache recorded no per-item catalog provenance beyond admin overrides.
            conn.execute(
                "UPDATE workshop_items SET catalog_indexed = 1 WHERE is_admin_override = 1",
                [],
            )?;
            conn.execute(
                "DELETE FROM sync_state WHERE key IN (?1, ?2, ?3)",
                params![
                    STATE_INCREMENTAL_SCAN,
                    STATE_BACKFILL_CURSOR,
                    STATE_BACKFILL_SYNCED_TOTAL,
                ],
            )?;
        }
        conn.execute_batch(
            r#"
            DROP INDEX IF EXISTS idx_workshop_items_catalog_public;
            CREATE INDEX IF NOT EXISTS idx_workshop_items_catalog_public_v2
                ON workshop_items(
                    catalog_indexed,
                    is_listed,
                    file_type,
                    COALESCE(subscriptions, 0) DESC,
                    workshop_id ASC
                );
            CREATE INDEX IF NOT EXISTS idx_workshop_items_catalog_relevance_v1
                ON workshop_items(
                    catalog_indexed,
                    is_listed,
                    file_type,
                    COALESCE(subscriptions, 0) DESC,
                    COALESCE(time_updated, 0) DESC,
                    workshop_id ASC
                );
            "#,
        )?;
        if has_column(conn, "workshop_item_revisions", "trust_score")? {
            conn.execute(
                "ALTER TABLE workshop_item_revisions DROP COLUMN trust_score",
                [],
            )?;
        }

        let index_current = Self::search_index_is_current(conn)?;
        // Older CM builds may have cached account-visible details as full private rows.
        Self::scrub_private_details_tx(conn, None, index_current)?;
        if !index_current {
            Self::rebuild_search_index_tx(conn)?;
        }
        Ok(())
    }

    fn search_index_is_current(tx: &Transaction<'_>) -> anyhow::Result<bool> {
        let current_version: Option<String> = tx
            .query_row(
                "SELECT value FROM sync_state WHERE key = ?1",
                params![STATE_SEARCH_INDEX_VERSION],
                |r| r.get(0),
            )
            .optional()?;
        Ok(current_version.as_deref() == Some(SEARCH_INDEX_VERSION)
            && has_table(tx, "workshop_items_fts")?)
    }

    fn rebuild_search_index_tx(tx: &Transaction<'_>) -> anyhow::Result<()> {
        tx.execute_batch(
            r#"
            DROP TABLE IF EXISTS workshop_items_fts;
            CREATE VIRTUAL TABLE workshop_items_fts USING fts5(
                title_tokens,
                title_compact,
                title_pinyin,
                creator_tokens,
                desc_tokens,
                desc_aliases,
                tokenize = 'unicode61 remove_diacritics 0'
            );
            "#,
        )?;

        let mut select_stmt =
            tx.prepare("SELECT workshop_id, title, creator_name, description FROM workshop_items")?;
        let rows = select_stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut insert_stmt = tx.prepare(INSERT_SEARCH_DOCUMENT)?;
        for row in rows {
            let (wid, title, creator_name, description) = row?;
            let doc = search::build_indexed_document(
                &title,
                creator_name.as_deref(),
                description.as_deref(),
            );
            insert_stmt.execute(params![
                wid,
                doc.title_tokens,
                doc.title_compact,
                doc.title_pinyin,
                doc.creator_tokens,
                doc.desc_tokens,
                doc.desc_aliases,
            ])?;
        }
        drop(insert_stmt);
        drop(select_stmt);

        tx.execute(
            "INSERT INTO sync_state (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![STATE_SEARCH_INDEX_VERSION, SEARCH_INDEX_VERSION],
        )?;
        Ok(())
    }

    pub(super) fn upsert_search_index_tx(
        tx: &Connection,
        wid: i64,
        title: &str,
        creator_name: Option<&str>,
        description: Option<&str>,
    ) -> anyhow::Result<()> {
        let doc = search::build_indexed_document(title, creator_name, description);
        tx.prepare_cached("DELETE FROM workshop_items_fts WHERE rowid = ?1")?
            .execute(params![wid])?;
        tx.prepare_cached(INSERT_SEARCH_DOCUMENT)?.execute(params![
            wid,
            doc.title_tokens,
            doc.title_compact,
            doc.title_pinyin,
            doc.creator_tokens,
            doc.desc_tokens,
            doc.desc_aliases,
        ])?;
        Ok(())
    }
}
