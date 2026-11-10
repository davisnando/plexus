use std::{collections::HashMap, path::Path, sync::Mutex};

use anyhow::{Context, Result};
use rmcp::model::{Resource, Tool};
use rusqlite::{Connection as Db, OptionalExtension, params};

use crate::model::*;

const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE connections (
    id TEXT PRIMARY KEY,
    alias TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    template_id TEXT,
    transport TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    idle_timeout_secs INTEGER NOT NULL,
    tool_cache TEXT,
    resource_cache TEXT,
    cache_at INTEGER,
    created_at INTEGER NOT NULL
);
CREATE TABLE profiles (
    id TEXT PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    mode TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE profile_connections (
    profile_id TEXT NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    connection_id TEXT NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    PRIMARY KEY (profile_id, connection_id)
);
CREATE TABLE tool_policies (
    profile_id TEXT NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    connection_id TEXT NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    tool TEXT NOT NULL,
    policy TEXT NOT NULL,
    PRIMARY KEY (profile_id, connection_id, tool)
);
CREATE TABLE call_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,
    profile_id TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    duration_ms INTEGER NOT NULL,
    status TEXT NOT NULL,
    error TEXT
);
CREATE INDEX call_log_ts ON call_log(ts);
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#, r#"
CREATE TABLE oauth_clients (
    client_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    redirect_uris TEXT NOT NULL,
    secret_hash TEXT,
    created_at INTEGER NOT NULL
);
CREATE TABLE oauth_grants (
    id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL REFERENCES oauth_clients(client_id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    refresh_hash TEXT NOT NULL UNIQUE,
    refresh_expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL
);
"#, r#"
-- Folder routing: one endpoint (the oldest profile), connections scoped by folder.
-- Connections routed into any profile stay available everywhere; the rest were
-- not exposed anywhere and get an empty folder list, which keeps it that way.
ALTER TABLE connections ADD COLUMN scope TEXT NOT NULL DEFAULT '{"kind":"everywhere"}';
UPDATE connections SET scope = '{"kind":"folders","folders":[]}'
    WHERE EXISTS (SELECT 1 FROM profiles)
    AND id NOT IN (SELECT connection_id FROM profile_connections);
-- Tool policies merge into the endpoint; the strictest one per tool wins.
CREATE TEMP TABLE merged_policies AS
    SELECT connection_id, tool,
        MAX(CASE policy WHEN 'hidden' THEN 2 WHEN 'approval' THEN 1 ELSE 0 END) AS level
    FROM tool_policies GROUP BY connection_id, tool;
DELETE FROM tool_policies;
INSERT INTO tool_policies (profile_id, connection_id, tool, policy)
    SELECT (SELECT id FROM profiles ORDER BY created_at, id LIMIT 1), connection_id, tool,
        CASE level WHEN 2 THEN 'hidden' ELSE 'approval' END
    FROM merged_policies WHERE level > 0;
DROP TABLE merged_policies;
UPDATE oauth_grants SET profile_id = (SELECT id FROM profiles ORDER BY created_at, id LIMIT 1);
DELETE FROM profile_connections;
"#];

/// SQLite-backed persistence for everything except secrets.
pub struct Store {
    db: Mutex<Db>,
}

#[derive(Debug, Clone, Default)]
pub struct CapabilityCache {
    pub tools: Vec<Tool>,
    pub resources: Vec<Resource>,
    pub cached_at: Option<Timestamp>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Db::open(path).with_context(|| format!("open {}", path.display()))?;
        // Folder routing (migration 3) folds profiles into one endpoint; keep a copy.
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if (1..3).contains(&version) {
            let backup = path.with_extension("db.before-folder-routing");
            if !backup.exists() {
                db.execute("VACUUM INTO ?1", [backup.to_string_lossy()])
                    .with_context(|| format!("back up {}", path.display()))?;
            }
        }
        Self::init(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Db::open_in_memory()?)
    }

    fn init(db: Db) -> Result<Self> {
        db.pragma_update(None, "foreign_keys", "ON")?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            db.execute_batch(sql)
                .with_context(|| format!("migration {}", i + 1))?;
            db.pragma_update(None, "user_version", i as i64 + 1)?;
        }
        Ok(Self { db: Mutex::new(db) })
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Db> {
        self.db.lock().expect("store mutex poisoned")
    }

    // ---- connections ----

    pub fn list_connections(&self) -> Result<Vec<Connection>> {
        let db = self.db();
        let mut stmt = db.prepare(
            "SELECT id, alias, name, template_id, transport, enabled, idle_timeout_secs, created_at, scope
             FROM connections ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], row_to_connection)?;
        rows.map(|r| r?.map_err(Into::into)).collect()
    }

    pub fn get_connection(&self, id: &str) -> Result<Option<Connection>> {
        let db = self.db();
        let row = db
            .query_row(
                "SELECT id, alias, name, template_id, transport, enabled, idle_timeout_secs, created_at, scope
                 FROM connections WHERE id = ?1",
                [id],
                row_to_connection,
            )
            .optional()?;
        row.transpose().map_err(Into::into)
    }

    pub fn insert_connection(&self, c: &Connection) -> Result<()> {
        self.db().execute(
            "INSERT INTO connections (id, alias, name, template_id, transport, enabled, idle_timeout_secs, created_at, scope)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                c.id,
                c.alias,
                c.name,
                c.template_id,
                serde_json::to_string(&c.transport)?,
                c.enabled,
                c.idle_timeout_secs as i64,
                c.created_at,
                serde_json::to_string(&c.scope)?
            ],
        )?;
        Ok(())
    }

    pub fn update_connection(&self, c: &Connection) -> Result<()> {
        self.db().execute(
            "UPDATE connections SET alias = ?2, name = ?3, template_id = ?4, transport = ?5,
             enabled = ?6, idle_timeout_secs = ?7, scope = ?8 WHERE id = ?1",
            params![
                c.id,
                c.alias,
                c.name,
                c.template_id,
                serde_json::to_string(&c.transport)?,
                c.enabled,
                c.idle_timeout_secs as i64,
                serde_json::to_string(&c.scope)?
            ],
        )?;
        Ok(())
    }

    pub fn delete_connection(&self, id: &str) -> Result<()> {
        self.db()
            .execute("DELETE FROM connections WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn load_caches(&self) -> Result<HashMap<String, CapabilityCache>> {
        let db = self.db();
        let mut stmt =
            db.prepare("SELECT id, tool_cache, resource_cache, cache_at FROM connections")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<i64>>(3)?,
            ))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, tools, resources, cached_at) = row?;
            out.insert(
                id,
                CapabilityCache {
                    tools: tools
                        .and_then(|t| serde_json::from_str(&t).ok())
                        .unwrap_or_default(),
                    resources: resources
                        .and_then(|t| serde_json::from_str(&t).ok())
                        .unwrap_or_default(),
                    cached_at,
                },
            );
        }
        Ok(out)
    }

    pub fn save_cache(&self, id: &str, cache: &CapabilityCache) -> Result<()> {
        self.db().execute(
            "UPDATE connections SET tool_cache = ?2, resource_cache = ?3, cache_at = ?4 WHERE id = ?1",
            params![
                id,
                serde_json::to_string(&cache.tools)?,
                serde_json::to_string(&cache.resources)?,
                cache.cached_at
            ],
        )?;
        Ok(())
    }

    // ---- profiles ----

    pub fn list_profiles(&self) -> Result<Vec<Profile>> {
        let db = self.db();
        let mut stmt =
            db.prepare("SELECT id, slug, name, mode, created_at FROM profiles ORDER BY created_at, id")?;
        let profiles = stmt
            .query_map([], |r| {
                Ok(Profile {
                    id: r.get(0)?,
                    slug: r.get(1)?,
                    name: r.get(2)?,
                    mode: ProfileMode::parse(&r.get::<_, String>(3)?),
                    connection_ids: vec![],
                    created_at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(profiles)
    }

    pub fn upsert_profile(&self, p: &Profile) -> Result<()> {
        self.db().execute(
            "INSERT INTO profiles (id, slug, name, mode, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET slug = ?2, name = ?3, mode = ?4",
            params![p.id, p.slug, p.name, p.mode.as_str(), p.created_at],
        )?;
        Ok(())
    }

    pub fn delete_profile(&self, id: &str) -> Result<()> {
        self.db().execute("DELETE FROM profiles WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---- tool policies ----

    /// Map of `(connection_id, tool)` to policy for a profile; absent means allow.
    pub fn tool_policies(&self, profile_id: &str) -> Result<HashMap<(String, String), ToolPolicy>> {
        let db = self.db();
        let mut stmt = db.prepare(
            "SELECT connection_id, tool, policy FROM tool_policies WHERE profile_id = ?1",
        )?;
        let rows = stmt.query_map([profile_id], |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, String>(1)?),
                ToolPolicy::parse(&r.get::<_, String>(2)?),
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_tool_policy(
        &self,
        profile_id: &str,
        connection_id: &str,
        tool: &str,
        policy: ToolPolicy,
    ) -> Result<()> {
        let db = self.db();
        if policy == ToolPolicy::Allow {
            db.execute(
                "DELETE FROM tool_policies WHERE profile_id = ?1 AND connection_id = ?2 AND tool = ?3",
                params![profile_id, connection_id, tool],
            )?;
        } else {
            db.execute(
                "INSERT INTO tool_policies (profile_id, connection_id, tool, policy) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT DO UPDATE SET policy = ?4",
                params![profile_id, connection_id, tool, policy.as_str()],
            )?;
        }
        Ok(())
    }

    // ---- call log ----

    pub fn insert_call(&self, e: &CallLogEntry) -> Result<i64> {
        let db = self.db();
        db.execute(
            "INSERT INTO call_log (ts, profile_id, connection_id, tool, duration_ms, status, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                e.ts,
                e.profile_id,
                e.connection_id,
                e.tool,
                e.duration_ms,
                e.status.as_str(),
                e.error
            ],
        )?;
        Ok(db.last_insert_rowid())
    }

    pub fn recent_calls(&self, limit: u32, before_id: Option<i64>) -> Result<Vec<CallLogEntry>> {
        let db = self.db();
        let mut stmt = db.prepare(
            "SELECT id, ts, profile_id, connection_id, tool, duration_ms, status, error
             FROM call_log WHERE id < ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![before_id.unwrap_or(i64::MAX), limit], |r| {
            Ok(CallLogEntry {
                id: r.get(0)?,
                ts: r.get(1)?,
                profile_id: r.get(2)?,
                connection_id: r.get(3)?,
                tool: r.get(4)?,
                duration_ms: r.get(5)?,
                status: CallStatus::parse(&r.get::<_, String>(6)?),
                error: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn prune_calls(&self, older_than: Timestamp) -> Result<usize> {
        Ok(self
            .db()
            .execute("DELETE FROM call_log WHERE ts < ?1", [older_than])?)
    }

    // ---- settings ----

    pub fn settings(&self) -> Result<Settings> {
        let raw: Option<String> = self
            .db()
            .query_row("SELECT value FROM settings WHERE key = 'settings'", [], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(raw
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default())
    }

    pub fn save_settings(&self, s: &Settings) -> Result<()> {
        self.db().execute(
            "INSERT INTO settings (key, value) VALUES ('settings', ?1)
             ON CONFLICT(key) DO UPDATE SET value = ?1",
            [serde_json::to_string(s)?],
        )?;
        Ok(())
    }

    // ---- remote OAuth clients and grants ----

    pub fn insert_oauth_client(&self, c: &OAuthClient) -> Result<()> {
        self.db().execute(
            "INSERT INTO oauth_clients (client_id, name, redirect_uris, secret_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                c.client_id,
                c.name,
                serde_json::to_string(&c.redirect_uris)?,
                c.secret_hash,
                now()
            ],
        )?;
        Ok(())
    }

    pub fn oauth_client(&self, client_id: &str) -> Result<Option<OAuthClient>> {
        let row = self
            .db()
            .query_row(
                "SELECT client_id, name, redirect_uris, secret_hash FROM oauth_clients WHERE client_id = ?1",
                [client_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.map(|(client_id, name, uris, secret_hash)| OAuthClient {
            client_id,
            name,
            redirect_uris: serde_json::from_str(&uris).unwrap_or_default(),
            secret_hash,
        }))
    }

    /// Registration is open to anyone who can reach the tunnel, so keep only
    /// the newest `keep` clients that have no grants.
    pub fn prune_oauth_clients(&self, keep: usize) -> Result<()> {
        self.db().execute(
            "DELETE FROM oauth_clients WHERE client_id IN (
                SELECT client_id FROM oauth_clients
                WHERE client_id NOT IN (SELECT client_id FROM oauth_grants)
                ORDER BY created_at DESC LIMIT -1 OFFSET ?1)",
            [keep as i64],
        )?;
        Ok(())
    }

    pub fn insert_grant(&self, g: &RemoteGrant, client_id: &str, refresh: &RefreshToken) -> Result<()> {
        self.db().execute(
            "INSERT INTO oauth_grants (id, profile_id, client_id, redirect_uri, refresh_hash,
             refresh_expires_at, created_at, last_used_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                g.id,
                g.profile_id,
                client_id,
                g.redirect_uri,
                refresh.hash,
                refresh.expires_at,
                g.created_at,
                g.last_used_at
            ],
        )?;
        Ok(())
    }

    /// `(grant id, profile id, client id, refresh expiry)` for a refresh token hash.
    pub fn grant_by_refresh(&self, hash: &str) -> Result<Option<(String, String, String, Timestamp)>> {
        Ok(self
            .db()
            .query_row(
                "SELECT id, profile_id, client_id, refresh_expires_at FROM oauth_grants WHERE refresh_hash = ?1",
                [hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?)
    }

    /// Swap in a new refresh token; false if the old one was already used.
    pub fn rotate_refresh(&self, grant_id: &str, old_hash: &str, new: &RefreshToken) -> Result<bool> {
        let n = self.db().execute(
            "UPDATE oauth_grants SET refresh_hash = ?3, refresh_expires_at = ?4, last_used_at = ?5
             WHERE id = ?1 AND refresh_hash = ?2",
            params![grant_id, old_hash, new.hash, new.expires_at, now()],
        )?;
        Ok(n == 1)
    }

    pub fn list_grants(&self, profile_id: &str) -> Result<Vec<RemoteGrant>> {
        let db = self.db();
        let mut stmt = db.prepare(
            "SELECT g.id, g.profile_id, c.name, g.redirect_uri, g.created_at, g.last_used_at
             FROM oauth_grants g JOIN oauth_clients c ON c.client_id = g.client_id
             WHERE g.profile_id = ?1 ORDER BY g.last_used_at DESC",
        )?;
        let rows = stmt.query_map([profile_id], |r| {
            Ok(RemoteGrant {
                id: r.get(0)?,
                profile_id: r.get(1)?,
                client_name: r.get(2)?,
                redirect_uri: r.get(3)?,
                created_at: r.get(4)?,
                last_used_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Deletes a grant; returns its profile id if it existed.
    pub fn delete_grant(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .db()
            .query_row(
                "DELETE FROM oauth_grants WHERE id = ?1 RETURNING profile_id",
                [id],
                |r| r.get(0),
            )
            .optional()?)
    }
}

/// A dynamically registered OAuth client of the remote endpoint.
#[derive(Debug, Clone)]
pub struct OAuthClient {
    pub client_id: String,
    pub name: String,
    pub redirect_uris: Vec<String>,
    /// SHA-256 of the client secret, for confidential clients.
    pub secret_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RefreshToken {
    pub hash: String,
    pub expires_at: Timestamp,
}

fn row_to_connection(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Connection>> {
    let id: String = r.get(0)?;
    let alias = r.get(1)?;
    let name = r.get(2)?;
    let template_id = r.get(3)?;
    let transport: String = r.get(4)?;
    let enabled = r.get(5)?;
    let idle_timeout_secs = r.get::<_, i64>(6)? as u64;
    let created_at = r.get(7)?;
    let scope: String = r.get(8)?;
    let scope = serde_json::from_str(&scope).unwrap_or_default();
    Ok(serde_json::from_str(&transport)
        .with_context(|| format!("decode transport of connection {id}"))
        .map(|transport| Connection {
            id,
            alias,
            name,
            template_id,
            transport,
            enabled,
            idle_timeout_secs,
            scope,
            created_at,
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database from before folder routing: two profiles, one connection in
    /// both, one in neither, conflicting policies and a grant on the newer profile.
    #[test]
    fn migrates_profiles_into_one_endpoint() {
        let db = Db::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..2] {
            db.execute_batch(sql).unwrap();
        }
        db.pragma_update(None, "user_version", 2).unwrap();
        db.execute_batch(
            r#"
            INSERT INTO connections (id, alias, name, transport, idle_timeout_secs, created_at)
                VALUES ('c1', 'jira', 'Jira', '{"kind":"http","url":"https://x"}', 600, 1),
                       ('c2', 'unused', 'Unused', '{"kind":"http","url":"https://y"}', 600, 2);
            INSERT INTO profiles (id, slug, name, mode, created_at)
                VALUES ('old', 'work', 'Work', 'meta', 1), ('new', 'side', 'Side', 'direct', 2);
            INSERT INTO profile_connections VALUES ('old', 'c1'), ('new', 'c1');
            INSERT INTO tool_policies VALUES
                ('old', 'c1', 'delete', 'approval'), ('new', 'c1', 'delete', 'hidden'),
                ('new', 'c1', 'search', 'approval');
            INSERT INTO oauth_clients (client_id, name, redirect_uris, created_at)
                VALUES ('cl', 'claude.ai', '[]', 1);
            INSERT INTO oauth_grants VALUES ('g', 'new', 'cl', 'https://claude.ai', 'h', 9, 1, 1);
            "#,
        )
        .unwrap();
        let store = Store::init(db).unwrap();

        let profiles = store.list_profiles().unwrap();
        assert_eq!(profiles[0].id, "old", "the oldest profile becomes the endpoint");
        assert_eq!(profiles[0].mode, ProfileMode::Meta);
        let scope = |id| store.get_connection(id).unwrap().unwrap().scope;
        assert_eq!(scope("c1"), Scope::Everywhere);
        assert_eq!(scope("c2"), Scope::Folders { folders: vec![] });
        let policies = store.tool_policies("old").unwrap();
        assert_eq!(policies[&("c1".into(), "delete".into())], ToolPolicy::Hidden);
        assert_eq!(policies[&("c1".into(), "search".into())], ToolPolicy::Approval);
        assert!(store.tool_policies("new").unwrap().is_empty());
        assert_eq!(store.list_grants("old").unwrap().len(), 1);
    }
}
