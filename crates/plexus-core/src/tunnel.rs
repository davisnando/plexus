//! Public tunnel: runs `cloudflared tunnel --url` (a Cloudflare quick tunnel,
//! no account needed) against the local server and reports the
//! `https://*.trycloudflare.com` URL it gets. The URL changes on every start.

use std::{
    collections::VecDeque,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use tokio::{io::AsyncBufReadExt, sync::oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    hub::{Hub, HubEvent},
    model::TunnelStatus,
};

const LOG_LINES: usize = 20;

pub(crate) struct Tunnel {
    state: Mutex<State>,
}

struct State {
    status: TunnelStatus,
    /// Bumped on every start/stop so a stale process task can't overwrite the status.
    generation: u64,
    running: Option<(CancellationToken, oneshot::Receiver<()>)>,
}

impl Default for Tunnel {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                status: TunnelStatus::Off,
                generation: 0,
                running: None,
            }),
        }
    }
}

impl Hub {
    pub fn tunnel_status(&self) -> TunnelStatus {
        self.tunnel().state.lock().unwrap().status.clone()
    }

    /// `https://….trycloudflare.com` while the tunnel is up.
    pub fn public_url(&self) -> Option<String> {
        match self.tunnel_status() {
            TunnelStatus::Running { url } => Some(url),
            _ => None,
        }
    }

    /// The `Host` that tunnelled requests arrive with.
    pub(crate) fn tunnel_host(&self) -> Option<String> {
        self.public_url()
            .and_then(|u| u.strip_prefix("https://").map(String::from))
    }

    fn set_tunnel_status(&self, generation: u64, status: TunnelStatus) {
        {
            let mut state = self.tunnel().state.lock().unwrap();
            if state.generation != generation || state.status == status {
                return;
            }
            state.status = status.clone();
        }
        self.emit(HubEvent::TunnelStatus { status });
    }

    /// (Re)start the tunnel to the current server port.
    pub async fn start_tunnel(&self) -> Result<()> {
        self.stop_tunnel().await;
        let port = self.server_port();
        let generation = {
            let mut state = self.tunnel().state.lock().unwrap();
            state.generation += 1;
            state.generation
        };
        let result = self.spawn_tunnel(generation, port);
        if let Err(e) = &result {
            self.set_tunnel_status(generation, TunnelStatus::Error { message: format!("{e:#}") });
        }
        result
    }

    fn spawn_tunnel(&self, generation: u64, port: u16) -> Result<()> {
        if port == 0 {
            bail!("the Plexus server is not running");
        }
        let binary = find_cloudflared(self.shell_path()).ok_or_else(|| {
            anyhow!("cloudflared is not installed. Install it with `brew install cloudflared`.")
        })?;
        let mut command = tokio::process::Command::new(binary);
        command
            // Quick tunnels default to QUIC on UDP 7844 with no fallback, which
            // many networks block; HTTP/2 goes over TCP.
            .args(["tunnel", "--no-autoupdate", "--protocol", "http2", "--url"])
            .arg(format!("http://127.0.0.1:{port}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(path) = self.shell_path() {
            command.env("PATH", path);
        }
        let mut child = command
            .spawn()
            .map_err(|e| anyhow!("could not start cloudflared: {e}"))?;
        let stderr = child.stderr.take().expect("stderr is piped");

        let ct = CancellationToken::new();
        let (done_tx, done_rx) = oneshot::channel();
        {
            let mut state = self.tunnel().state.lock().unwrap();
            state.running = Some((ct.clone(), done_rx));
        }
        self.set_tunnel_status(generation, TunnelStatus::Starting);

        let log = Arc::new(Mutex::new(VecDeque::new()));
        let hub = self.clone();
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            // The URL is printed before the edge connection exists; it only
            // serves traffic once a connection is registered.
            let mut url = None;
            let cancelled = loop {
                tokio::select! {
                    _ = ct.cancelled() => break true,
                    line = lines.next_line() => match line {
                        Ok(Some(line)) => {
                            tracing::debug!(target: "plexus_core::tunnel", "{line}");
                            if url.is_none() {
                                url = quick_tunnel_url(&line);
                            }
                            if let Some(url) = &url
                                && line.contains("Registered tunnel connection")
                            {
                                hub.set_tunnel_status(generation, TunnelStatus::Running { url: url.clone() });
                            }
                            let mut log = log.lock().unwrap();
                            if log.len() == LOG_LINES {
                                log.pop_front();
                            }
                            log.push_back(line);
                        }
                        // stderr closed: cloudflared is exiting.
                        _ => break false,
                    },
                }
            };
            let exit = if cancelled {
                let _ = child.kill().await;
                None
            } else {
                tokio::select! {
                    _ = ct.cancelled() => {
                        let _ = child.kill().await;
                        None
                    }
                    status = child.wait() => Some(status),
                }
            };
            if let Some(status) = exit {
                let tail = log.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
                let code = status.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string());
                tracing::warn!("cloudflared exited ({code})");
                hub.set_tunnel_status(
                    generation,
                    TunnelStatus::Error {
                        message: format!("cloudflared exited ({code})\n{tail}").trim().to_string(),
                    },
                );
            }
            let _ = done_tx.send(());
        });
        Ok(())
    }

    pub async fn stop_tunnel(&self) {
        let (running, generation) = {
            let mut state = self.tunnel().state.lock().unwrap();
            state.generation += 1;
            (state.running.take(), state.generation)
        };
        if let Some((ct, done)) = running {
            ct.cancel();
            let _ = tokio::time::timeout(Duration::from_secs(3), done).await;
        }
        self.set_tunnel_status(generation, TunnelStatus::Off);
        // Tokens stay valid, but the issuer URL they were minted under is gone.
        self.remote().clear_pending();
    }
}

/// Picks the quick tunnel URL out of a cloudflared log line.
fn quick_tunnel_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let url: String = line[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '/'))
        .collect();
    let host = url.strip_prefix("https://")?;
    (host.ends_with(".trycloudflare.com") && host != "api.trycloudflare.com").then_some(url)
}

fn find_cloudflared(shell_path: Option<&str>) -> Option<PathBuf> {
    let path = shell_path
        .map(String::from)
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/opt/homebrew/bin".into(), "/usr/local/bin".into()])
        .map(|dir| dir.join("cloudflared"))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::quick_tunnel_url;

    #[test]
    fn finds_the_quick_tunnel_url() {
        let line = "2026-10-23T10:00:00Z INF |  https://lively-otter-steps-ring.trycloudflare.com                    |";
        assert_eq!(
            quick_tunnel_url(line).as_deref(),
            Some("https://lively-otter-steps-ring.trycloudflare.com")
        );
        assert_eq!(
            quick_tunnel_url("INF Requesting new quick Tunnel on trycloudflare.com..."),
            None
        );
        assert_eq!(
            quick_tunnel_url("ERR failed to request https://api.trycloudflare.com/tunnel"),
            None
        );
    }
}
