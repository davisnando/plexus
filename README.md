# Plexus

A local MCP hub that lives in your menu bar. Add MCP servers, including the same server several times for different
accounts (three Jira sites, two Sentry orgs), group them into **profiles**, and point Claude Code, Cursor, VS Code or
Claude Desktop at one endpoint per profile.

```
Claude Code ──► http://127.0.0.1:7420/mcp/work ──► jira_acme · jira_client · sentry_prod
Cursor      ──► http://127.0.0.1:7420/mcp/side ──► linear · sentry_prod
```

## Features

- **Many instances of one server.** Every connection has an alias, and its tools are exposed as `{alias}__{tool}`
  (e.g. `jira_acme__create_issue`).
- **Profiles.** Each profile has its own endpoint and bearer token, a routing matrix that picks its connections, and
  per-tool policies (allow / ask / hide).
- **Two tool modes.** *Every tool* re-exports all tools directly. *Search & call* exposes six meta tools
  (`list_connections`, `search_tools`, `describe_tool`, `call_tool`, `call_tools`, `connect`) with BM25 search, which
  keeps the client's context small when many servers are connected. Calls can fan out over several instances
  (`connections: "*"` runs one tool on every Jira), run in parallel batches, keep only selected JSON `fields`, and are
  capped at `max_chars`. Agents can pass a `reason` that the approval prompt shows, and `connect` restarts a failing
  connection or opens the browser sign-in.
- **Upstream types.** Local stdio (npx, uvx, docker, any binary), remote HTTP with static headers or a bearer token, and
  remote HTTP with OAuth: discovery, dynamic client registration and PKCE in your browser, with separate tokens per
  connection.
- **Catalog.** Curated templates (Jira/Confluence, Atlassian, Sentry, GitHub, GitLab, Linear, Notion, Slack, Postgres,
  Supabase, Stripe, Honeycomb, Grafana, Cloudflare, Context7, Playwright, Fetch, Filesystem), search of
  the official MCP Registry, or any custom command or URL.
- **Secrets** live in the macOS Keychain. Config lives in SQLite at `~/Library/Application Support/dev.plexus.app/`.
- **Lazy processes.** Upstreams start on first use, their tool lists are cached, and they stop after an idle timeout.
- **Approvals.** Tools marked *ask* open an always-on-top prompt (Allow once / Always allow / Deny), which auto-denies
  after 60 s.
- **Activity log.** Tool, connection, duration and status for each call. Arguments and results are never stored.
- **One-click client setup** for Claude Code (via `claude mcp add`), Cursor, VS Code and Claude Desktop (via
  `mcp-remote`). Config files are merged and backed up to `*.plexus.bak` first.
- **Public tunnel.** One switch starts a Cloudflare quick tunnel (`brew install cloudflared`, no account), so every
  profile also gets an `https://….trycloudflare.com/mcp/{slug}` endpoint. CLI agents elsewhere use the profile token.
  claude.ai and ChatGPT custom connectors sign in with OAuth: each profile is its own authorization server, and you
  approve every sign-in in a popup on your Mac. Sign-ins are listed on the profile page and can be revoked. The
  address changes each time the tunnel starts.

## Security model

The server binds `127.0.0.1` only. Every request needs the profile's bearer token (compared in constant time) or an
OAuth access token you approved for that profile. Requests carrying an `Origin` header are rejected, which blocks
browser pages, and a `Host` other than localhost or the tunnel's hostname is refused, which blocks DNS rebinding.
Through the tunnel only `/mcp/*`, the OAuth routes and `/health` are reachable. Opening the tunnel URL alone gets
nobody in: OAuth needs a click on this Mac, and at most three sign-in prompts can wait at once.

## Develop

Requirements: Rust 1.99+, Node 26+, pnpm 11.

```sh
pnpm install
pnpm dev                 # tauri dev: Vite + the app (it appears in the menu bar)
cargo test -p plexus-core                       # unit + end-to-end proxy tests
cargo test -p plexus-core -- --ignored          # live OAuth discovery + quick tunnel tests (network, cloudflared)
pnpm --dir ui check                             # svelte-check
pnpm build                                      # release .app + .dmg in target/release/bundle
# If the dmg step fails with "hdiutil: couldn't unmount ... Resource busy", Finder/Spotlight grabbed
# the temp volume: `CI=true pnpm build` skips the Finder window styling.
```

Builds are ad-hoc signed unless `APPLE_SIGNING_IDENTITY` names a code signing certificate in your keychain
(`security find-identity -v -p codesigning`). Sign with a stable identity for daily use: macOS ties keychain access to
the app's signature, so an ad-hoc build asks for the keychain again after every rebuild. Release builds are also
notarized when `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password) and `APPLE_TEAM_ID` are set.

## License

MIT, see [LICENSE](LICENSE).

The dashboard opens from the menu bar icon, or by launching the app a second time. To iterate on the UI in a normal
browser with mocked data, open `http://localhost:5173/preview.html#/profiles` while `pnpm dev` (or `pnpm --dir ui dev`)
is running.

## Layout

| Path | What |
|---|---|
| `crates/plexus-core` | All logic, no UI dependency: store, keychain, upstream lifecycle, aggregation, HTTP endpoint, OAuth, client config |
| `crates/plexus-core/src/bin/fake-mcp.rs` | Tiny MCP server used by the tests |
| `src-tauri` | Tauri shell: tray menu, dashboard and approval windows, IPC commands |
| `ui` | Svelte 5 + Tailwind 4 dashboard |
| `DESIGN.md` | Decisions and architecture |
