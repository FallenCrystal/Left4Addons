use crate::error::CoreError;
use rusqlite::Connection;

pub fn run_migrations(conn: &Connection) -> Result<(), CoreError> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS addons (
            id TEXT PRIMARY KEY,
            workshop_id INTEGER,
            title TEXT NOT NULL,
            author TEXT,
            version TEXT,
            install_state TEXT NOT NULL,
            primary_dir TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            current_file_name TEXT,
            current_path TEXT,
            content_hash TEXT,
            file_size INTEGER NOT NULL DEFAULT 0,
            files_count INTEGER NOT NULL DEFAULT 0,
            cover_image TEXT,
            categories_json TEXT NOT NULL DEFAULT '[]',
            maps_json TEXT NOT NULL DEFAULT '[]',
            addon_info_json TEXT NOT NULL DEFAULT '{}',
            has_dummy_bypass INTEGER NOT NULL DEFAULT 0,
            local_time_updated INTEGER,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_addons_workshop_id ON addons(workshop_id);
        CREATE INDEX IF NOT EXISTS idx_addons_content_hash ON addons(content_hash);

        CREATE TABLE IF NOT EXISTS physical_files (
            path TEXT PRIMARY KEY,
            dir_type TEXT NOT NULL,
            file_name TEXT NOT NULL,
            enabled INTEGER NOT NULL,
            role TEXT NOT NULL,
            workshop_id INTEGER,
            content_hash TEXT,
            dummy_original_hash TEXT,
            dummy_original_time_updated INTEGER,
            file_size INTEGER NOT NULL,
            modified_at INTEGER NOT NULL,
            addon_id TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_physical_files_wid ON physical_files(workshop_id);

        CREATE TABLE IF NOT EXISTS workshop_snapshots (
            workshop_id INTEGER PRIMARY KEY,
            file_type TEXT NOT NULL,
            status TEXT NOT NULL,
            title TEXT NOT NULL,
            description TEXT,
            creator_steam_id TEXT,
            creator_name TEXT,
            preview_url TEXT,
            file_url TEXT,
            file_size INTEGER,
            time_created INTEGER,
            time_updated INTEGER,
            tags_json TEXT NOT NULL DEFAULT '[]',
            collection_children_json TEXT NOT NULL DEFAULT '[]',
            source TEXT NOT NULL,
            fetched_at TEXT NOT NULL,
            removed_detected_at TEXT
        );

        CREATE TABLE IF NOT EXISTS workshop_dependencies (
            parent_workshop_id INTEGER NOT NULL,
            dep_workshop_id INTEGER NOT NULL,
            dep_title TEXT,
            required INTEGER NOT NULL DEFAULT 1,
            status TEXT NOT NULL DEFAULT 'active',
            updated_at TEXT NOT NULL,
            PRIMARY KEY (parent_workshop_id, dep_workshop_id)
        );
        CREATE INDEX IF NOT EXISTS idx_workshop_deps_dep_status ON workshop_dependencies(dep_workshop_id, status);

        CREATE TABLE IF NOT EXISTS bundles (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            locked INTEGER NOT NULL DEFAULT 0,
            maps_json TEXT NOT NULL DEFAULT '[]',
            cover_image TEXT
        );

        CREATE TABLE IF NOT EXISTS bundle_members (
            bundle_id TEXT NOT NULL,
            addon_id TEXT NOT NULL,
            position INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (bundle_id, addon_id),
            FOREIGN KEY (bundle_id) REFERENCES bundles(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS tracked_collections (
            collection_id INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            description TEXT,
            auto_sync INTEGER NOT NULL DEFAULT 0,
            removed_upstream_json TEXT NOT NULL DEFAULT '[]',
            last_synced_at TEXT
        );

        CREATE TABLE IF NOT EXISTS collection_items (
            collection_id INTEGER NOT NULL,
            workshop_id INTEGER NOT NULL,
            position INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (collection_id, workshop_id),
            FOREIGN KEY (collection_id) REFERENCES tracked_collections(collection_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS user_groups (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            tags_json TEXT NOT NULL DEFAULT '[]',
            color TEXT
        );

        CREATE TABLE IF NOT EXISTS user_group_addons (
            group_id TEXT NOT NULL,
            addon_id TEXT NOT NULL,
            PRIMARY KEY (group_id, addon_id),
            FOREIGN KEY (group_id) REFERENCES user_groups(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS loadout_profiles (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT ''
        );

        CREATE TABLE IF NOT EXISTS profile_addons (
            profile_id TEXT NOT NULL,
            addon_id TEXT NOT NULL,
            PRIMARY KEY (profile_id, addon_id),
            FOREIGN KEY (profile_id) REFERENCES loadout_profiles(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS reconcile_issues (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            workshop_id INTEGER,
            addon_id TEXT,
            loading_path TEXT,
            workshop_path TEXT,
            summary TEXT NOT NULL,
            recommended_action TEXT NOT NULL,
            available_actions_json TEXT NOT NULL,
            resolved INTEGER NOT NULL DEFAULT 0,
            detected_at TEXT NOT NULL,
            state_fingerprint TEXT NOT NULL DEFAULT ''
        );

        CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            status TEXT NOT NULL,
            target_id TEXT NOT NULL,
            progress_percent INTEGER NOT NULL DEFAULT 0,
            message TEXT,
            error TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        "#,
    )?;

    ensure_column_exists(
        conn,
        "reconcile_issues",
        "state_fingerprint",
        "ALTER TABLE reconcile_issues ADD COLUMN state_fingerprint TEXT NOT NULL DEFAULT ''",
    )?;

    Ok(())
}

fn ensure_column_exists(
    conn: &Connection,
    table: &str,
    column: &str,
    alter_sql: &str,
) -> Result<(), CoreError> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&pragma)?;
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(Result::ok)
        .collect();
    if !cols.iter().any(|c| c == column) {
        conn.execute(alter_sql, [])?;
    }
    Ok(())
}
