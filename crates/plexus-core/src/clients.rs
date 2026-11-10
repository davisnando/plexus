//! Register the endpoint with MCP clients by editing their config.
//! JSON files are merged (only our `plexus` entry changes, and the
//! `plexus-{slug}` entries of old per-profile endpoints are removed) and
//! backed up to `*.plexus.bak` before writing.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Client {
    ClaudeCode,
    ClaudeDesktop,
    Cursor,
    Vscode,
}

/// The name Plexus registers itself under in client configs.
pub const SERVER_NAME: &str = "plexus";

/// Header telling Plexus the session's folder; Claude Code expands `${PWD}`.
const CLAUDE_CODE_FOLDER: (&str, &str) = ("X-Plexus-Folder", "${PWD}");

pub struct Endpoint<'a> {
    pub url: &'a str,
    pub token: &'a str,
    /// Slugs of the profiles from before folder routing; their
    /// `plexus-{slug}` entries are replaced by the single `plexus` one.
    pub legacy_slugs: &'a [String],
}

impl Endpoint<'_> {
    fn legacy_names(&self) -> Vec<String> {
        self.legacy_slugs.iter().map(|s| format!("plexus-{s}")).collect()
    }
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is not set"))
}

fn config_path(client: Client) -> Result<PathBuf> {
    let home = home()?;
    let app_support = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else if cfg!(windows) {
        PathBuf::from(std::env::var("APPDATA").context("APPDATA is not set")?)
    } else {
        home.join(".config")
    };
    Ok(match client {
        Client::Cursor => home.join(".cursor/mcp.json"),
        Client::Vscode => app_support.join("Code/User/mcp.json"),
        Client::ClaudeDesktop => app_support.join("Claude/claude_desktop_config.json"),
        Client::ClaudeCode => home.join(".claude.json"),
    })
}

/// The entry Plexus writes for a client, as `(top-level key, server config)`.
pub fn entry(client: Client, endpoint: &Endpoint) -> (&'static str, Value) {
    let auth = format!("Bearer {}", endpoint.token);
    match client {
        Client::Cursor => (
            "mcpServers",
            json!({ "url": endpoint.url, "headers": { "Authorization": auth } }),
        ),
        Client::Vscode => (
            "servers",
            json!({ "type": "http", "url": endpoint.url, "headers": { "Authorization": auth } }),
        ),
        // Claude Code expands `${PWD}` per session, so every request says which
        // folder it comes from; on protocol 2026-07-28 that is the only way.
        Client::ClaudeCode => (
            "mcpServers",
            json!({
                "type": "http",
                "url": endpoint.url,
                "headers": { "Authorization": auth, CLAUDE_CODE_FOLDER.0: CLAUDE_CODE_FOLDER.1 },
            }),
        ),
        // Desktop's config file only launches stdio servers; mcp-remote bridges to HTTP.
        Client::ClaudeDesktop => (
            "mcpServers",
            json!({
                "command": "npx",
                "args": ["-y", "mcp-remote", endpoint.url, "--header", "Authorization:${PLEXUS_AUTH}"],
                "env": { "PLEXUS_AUTH": auth },
            }),
        ),
    }
}

/// Add or replace the profile's entry. Returns a human-readable result.
pub fn install(client: Client, endpoint: &Endpoint, shell_path: Option<&str>) -> Result<String> {
    if client == Client::ClaudeCode {
        // Claude Code rewrites ~/.claude.json constantly; let its CLI do the edit.
        if let Ok(msg) = install_with_claude_cli(endpoint, shell_path) {
            return Ok(msg);
        }
    }
    let path = config_path(client)?;
    let (key, value) = entry(client, endpoint);
    let removed = merge_json(&path, key, Some(value), &endpoint.legacy_names())?;
    let mut msg = format!("Added {SERVER_NAME} to {}.", path.display());
    msg.push_str(&replaced(&removed));
    if client == Client::ClaudeDesktop {
        msg.push_str(" Restart Claude Desktop to load it.");
    }
    Ok(msg)
}

pub fn uninstall(client: Client, endpoint: &Endpoint) -> Result<()> {
    let path = config_path(client)?;
    let (key, _) = entry(client, endpoint);
    if path.exists() {
        merge_json(&path, key, None, &endpoint.legacy_names())?;
    }
    Ok(())
}

fn replaced(removed: &[String]) -> String {
    if removed.is_empty() {
        String::new()
    } else {
        format!(" It replaces {}.", removed.join(", "))
    }
}

fn install_with_claude_cli(endpoint: &Endpoint, shell_path: Option<&str>) -> Result<String> {
    let run = |args: &[&str]| {
        let mut cmd = std::process::Command::new("claude");
        cmd.args(args).stdin(std::process::Stdio::null());
        if let Some(path) = shell_path {
            cmd.env("PATH", path);
        }
        cmd.output()
    };
    let _ = run(&["mcp", "remove", "--scope", "user", SERVER_NAME]);
    let header = format!("Authorization: Bearer {}", endpoint.token);
    let folder = format!("{}: {}", CLAUDE_CODE_FOLDER.0, CLAUDE_CODE_FOLDER.1);
    let out = run(&[
        "mcp", "add", "--transport", "http", "--scope", "user", SERVER_NAME, endpoint.url,
        "--header", &header, "--header", &folder,
    ])
    .context("`claude` CLI not found")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr));
    }
    let removed: Vec<String> = endpoint
        .legacy_names()
        .into_iter()
        .filter(|name| {
            run(&["mcp", "remove", "--scope", "user", name]).is_ok_and(|o| o.status.success())
        })
        .collect();
    Ok(format!(
        "Added {SERVER_NAME} to Claude Code (user scope).{}",
        replaced(&removed)
    ))
}

/// Sets (or with `None` removes) our entry under `key` and removes the
/// `legacy` entries. Returns the legacy names that were present.
fn merge_json(path: &Path, key: &str, value: Option<Value>, legacy: &[String]) -> Result<Vec<String>> {
    let mut root = if path.exists() {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        if raw.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str::<Value>(&raw).with_context(|| {
                format!(
                    "{} isn't plain JSON (comments?), so Plexus won't rewrite it. Copy the snippet instead.",
                    path.display()
                )
            })?
        }
    } else {
        json!({})
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow!("{} is not a JSON object", path.display()))?;
    let servers = obj
        .entry(key)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("`{key}` in {} is not an object", path.display()))?;
    match value {
        Some(v) => {
            servers.insert(SERVER_NAME.to_string(), v);
        }
        None => {
            servers.remove(SERVER_NAME);
        }
    }
    let removed: Vec<String> = legacy
        .iter()
        .filter(|name| servers.remove(name.as_str()).is_some())
        .cloned()
        .collect();
    if path.exists() {
        let backup = path.with_extension(format!(
            "{}.plexus.bak",
            path.extension().and_then(|e| e.to_str()).unwrap_or("json")
        ));
        std::fs::copy(path, &backup).with_context(|| format!("back up {}", path.display()))?;
    } else if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("plexus.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&root)? + "\n")?;
    std::fs::rename(&tmp, path)?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_without_touching_other_servers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"other":{"url":"x"},"plexus-work":{"url":"old"}},"theme":"dark"}"#,
        )
        .unwrap();
        let legacy = vec!["work".to_string()];
        let ep = Endpoint {
            url: "http://127.0.0.1:7420/mcp",
            token: "t",
            legacy_slugs: &legacy,
        };
        let (key, value) = entry(Client::Cursor, &ep);
        let removed = merge_json(&path, key, Some(value), &ep.legacy_names()).unwrap();
        assert_eq!(removed, ["plexus-work"]);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcpServers"]["other"]["url"], "x");
        assert_eq!(v["mcpServers"]["plexus"]["headers"]["Authorization"], "Bearer t");
        assert!(v["mcpServers"].get("plexus-work").is_none());
        assert!(dir.path().join("mcp.json.plexus.bak").exists());

        merge_json(&path, key, None, &[]).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v["mcpServers"].get("plexus").is_none());
    }

    #[test]
    fn refuses_jsonc() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, "// comment\n{}").unwrap();
        assert!(merge_json(&path, "servers", Some(json!({})), &[]).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "// comment\n{}");
    }
}
