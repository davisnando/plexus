//! Register a profile endpoint with MCP clients by editing their config.
//! JSON files are merged (only our `plexus-{slug}` entry changes) and backed
//! up to `*.plexus.bak` before writing.

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

pub struct Endpoint<'a> {
    pub slug: &'a str,
    pub url: &'a str,
    pub token: &'a str,
}

impl Endpoint<'_> {
    pub fn server_name(&self) -> String {
        format!("plexus-{}", self.slug)
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
        Client::ClaudeCode => (
            "mcpServers",
            json!({ "type": "http", "url": endpoint.url, "headers": { "Authorization": auth } }),
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
    merge_json(&path, key, &endpoint.server_name(), Some(value))?;
    let mut msg = format!("Added {} to {}.", endpoint.server_name(), path.display());
    if client == Client::ClaudeDesktop {
        msg.push_str(" Restart Claude Desktop to load it.");
    }
    Ok(msg)
}

pub fn uninstall(client: Client, endpoint: &Endpoint) -> Result<()> {
    let path = config_path(client)?;
    let (key, _) = entry(client, endpoint);
    if path.exists() {
        merge_json(&path, key, &endpoint.server_name(), None)?;
    }
    Ok(())
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
    let name = endpoint.server_name();
    let _ = run(&["mcp", "remove", "--scope", "user", &name]);
    let header = format!("Authorization: Bearer {}", endpoint.token);
    let out = run(&[
        "mcp", "add", "--transport", "http", "--scope", "user", &name, endpoint.url, "--header",
        &header,
    ])
    .context("`claude` CLI not found")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(format!("Added {name} to Claude Code (user scope)."))
}

fn merge_json(path: &Path, key: &str, name: &str, value: Option<Value>) -> Result<()> {
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
            servers.insert(name.to_string(), v);
        }
        None => {
            servers.remove(name);
        }
    }
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_without_touching_other_servers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, r#"{"mcpServers":{"other":{"url":"x"}},"theme":"dark"}"#).unwrap();
        let ep = Endpoint {
            slug: "work",
            url: "http://127.0.0.1:7420/mcp/work",
            token: "t",
        };
        let (key, value) = entry(Client::Cursor, &ep);
        merge_json(&path, key, &ep.server_name(), Some(value)).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcpServers"]["other"]["url"], "x");
        assert_eq!(
            v["mcpServers"]["plexus-work"]["headers"]["Authorization"],
            "Bearer t"
        );
        assert!(dir.path().join("mcp.json.plexus.bak").exists());

        merge_json(&path, key, &ep.server_name(), None).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v["mcpServers"].get("plexus-work").is_none());
    }

    #[test]
    fn refuses_jsonc() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, "// comment\n{}").unwrap();
        assert!(merge_json(&path, "servers", "plexus-x", Some(json!({}))).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "// comment\n{}");
    }
}
