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
            "SELECT id, alias, name, template_id, transport, enabled, idle_timeout_secs, created_at
             FROM connections ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], row_to_connection)?;
        rows.map(|r| r?.map_err(Into::into)).collect()
    }

    pub fn get_connection(&self, id: &str) -> Result<Option<Connection>> {
        let db = self.db();
        let row = db
            .query_row(
                "SELECT id, alias, name, template_id, transport, enabled, idle_timeout_secs, created_at
                 FROM connections WHERE id = ?1",
                [id],
                row_to_connection,
            )
            .optional()?;
        row.transpose().map_err(Into::into)
    }

    pub fn insert_connection(&self, c: &Connection) -> Result<()> {
        self.db().execute(
            "INSERT INTO connections (id, alias, name, template_id, transport, enabled, idle_timeout_secs, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                c.id,
                c.alias,
                c.name,
                c.template_id,
                serde_json::to_string(&c.transport)?,
                c.enabled,
                c.idle_timeout_secs as i64,
                c.created_at
            ],
        )?;
        Ok(())
    }

    pub fn update_connection(&self, c: &Connection) -> Result<()> {
        self.db().execute(
            "UPDATE connections SET alias = ?2, name = ?3, template_id = ?4, transport = ?5,
             enabled = ?6, idle_timeout_secs = ?7 WHERE id = ?1",
            params![
                c.id,
                c.alias,
                c.name,
                c.template_id,
                serde_json::to_string(&c.transport)?,
                c.enabled,
                c.idle_timeout_secs as i64
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
            db.prepare("SELECT id, slug, name, mode, created_at FROM profiles ORDER BY created_at")?;
        let mut profiles = stmt
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
        let mut stmt = db.prepare(
            "SELECT pc.profile_id, pc.connection_id FROM profile_connections pc
             JOIN connections c ON c.id = pc.connection_id ORDER BY c.created_at",
        )?;
        let links = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (profile_id, connection_id) in links {
            if let Some(p) = profiles.iter_mut().find(|p| p.id == profile_id) {
                p.connection_ids.push(connection_id);
            }
        }
        Ok(profiles)
    }

    pub fn upsert_profile(&self, p: &Profile) -> Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO profiles (id, slug, name, mode, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET slug = ?2, name = ?3, mode = ?4",
            params![p.id, p.slug, p.name, p.mode.as_str(), p.created_at],
        )?;
        tx.execute(
            "DELETE FROM profile_connections WHERE profile_id = ?1",
            [&p.id],
        )?;
        for cid in &p.connection_ids {
            tx.execute(
                "INSERT INTO profile_connections (profile_id, connection_id) VALUES (?1, ?2)",
                params![p.id, cid],
            )?;
        }
        tx.commit()?;
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
            created_at,
        }))
}
