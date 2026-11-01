# Plexus — local MCP hub

Plexus is a menu-bar app that aggregates many MCP servers — including several
instances of the same server (three Jiras, two Sentrys) — behind one local MCP
endpoint per *profile*. Think Composio, but local, open, and in Rust.

## Decisions (from design interview, 2026-10-23)

| Topic | Decision |
|---|---|
| Client entry | Streamable HTTP on `http://127.0.0.1:7420/mcp/{profile}` (port configurable) |
| Tool surface | Per profile: **direct** (namespaced tools) or **meta** (search/describe/call) |
| Upstreams | Local stdio (npx/uvx/binary), remote HTTP w/ static auth, remote HTTP w/ OAuth |
| Profiles | Yes — subset of connections, own endpoint, own bearer token, per-tool policy |
| Secrets | OS keychain (`keyring` crate); non-secret config in SQLite |
| Catalog | Curated built-in templates + official MCP Registry search + custom |
| Local auth | Bearer token per profile, bind 127.0.0.1, Host/Origin checks (DNS rebinding) |
| Client setup | Auto-configure Claude Code, Claude Desktop, Cursor, VS Code (+ copy snippet) |
| Lifecycle | Lazy start, cached tool lists, idle shutdown (default 10 min) |
| Platforms | macOS first; no mac-only APIs in core so Win/Linux are cheap later |
| Frontend | Svelte 5 SPA (Vite, hash router) + Tailwind v4. SvelteKit dropped: no SSR/routing needs inside Tauri |
| Observability | Connection health, live call log (metadata only), approval prompts |
| Tool names | `{alias}__{tool}`, alias user-set; hash-truncate if > 64 chars |
| Meta search | In-process BM25 over tool name/description/connection |
| MCP scope | Tools + resources (prompts later) |
| Shell | Menu bar only (no Dock icon), launch at login on by default |
| OAuth multi-account | System browser + "use a private window / log out first" hint |
| Approvals | Always-on-top popup: Allow once / Always allow / Deny, auto-deny after 60 s |
| Call log | Metadata only (ts, profile, connection, tool, duration, status, error), 7-day retention |

## Architecture

```
 Claude Code / Cursor / VS Code / Claude Desktop (via mcp-remote)
                 │  Streamable HTTP + Bearer
                 ▼
 ┌──────────────────────── Plexus process ────────────────────────┐
 │ axum server 127.0.0.1:7420                                      │
 │   /mcp/{profile}      → ProfileService (rmcp ServerHandler)     │
 │   /oauth/callback     → OAuthManager                            │
 │                                                                 │
 │ Hub (plexus-core)                                               │
 │   Store (SQLite) · Secrets (keychain) · Registry/Templates      │
 │   UpstreamManager ── Upstream{stdio | http} (rmcp client)       │
 │   ToolIndex (namespacing + BM25) · Policy · ApprovalBroker      │
 │   CallLog · EventBus (broadcast) ──► UI + tray                  │
 │                                                                 │
 │ Tauri shell (src-tauri): tray, dashboard window, approval       │
 │   window, autostart, single-instance, IPC commands              │
 └─────────────────────────────────────────────────────────────────┘
                 │ stdio / HTTP
                 ▼
     jira_acme  jira_client2  sentry_prod  github  …
```

### Repository layout

```
Cargo.toml                 workspace
crates/plexus-core/        all logic, no Tauri dependency (testable headless)
crates/plexus-core/tests/  integration tests with a fake MCP server
crates/plexus-core/src/bin/fake-mcp.rs   tiny rmcp stdio server used by tests
src-tauri/                 Tauri 2 app: tray, windows, IPC → plexus-core
ui/                        Svelte 5 + Tailwind SPA (catalog in ui/src/lib/catalog.ts)
```

`plexus-core` exposes a `Hub` with async methods; `src-tauri` is a thin shell
that forwards IPC commands to it and implements the `ApprovalBroker` trait with
a popup window. A future headless `plexusd` is possible for free.

## Data model (SQLite, `~/Library/Application Support/dev.plexus.app/plexus.db`)

```sql
connections(
  id TEXT PK, alias TEXT UNIQUE,          -- ^[a-z][a-z0-9_]{0,23}$
  name TEXT, template_id TEXT NULL,
  transport TEXT,                         -- 'stdio' | 'http'
  command TEXT, args JSON, env JSON,      -- stdio, non-secret env
  secret_env JSON,                        -- list of env keys stored in keychain
  url TEXT, auth TEXT, headers JSON,      -- http; auth: none|bearer|header|oauth
  enabled INT, idle_timeout_secs INT,
  tool_cache JSON, tool_cache_at INT, resource_cache JSON,
  created_at INT)
profiles(id TEXT PK, slug TEXT UNIQUE, name TEXT, mode TEXT /*direct|meta*/, created_at INT)
profile_connections(profile_id, connection_id, PK(profile_id, connection_id))
tool_policies(profile_id, connection_id, tool TEXT, policy TEXT /*hidden|approval|allow*/,
  PK(profile_id, connection_id, tool))
call_log(id INT PK, ts INT, profile_id, connection_id, tool TEXT,
  duration_ms INT, status TEXT /*ok|error|denied|timeout*/, error TEXT NULL)
settings(key TEXT PK, value JSON)
```

Keychain (service `dev.plexus.app`): `conn:{id}:env:{KEY}`, `conn:{id}:header:{NAME}`,
`conn:{id}:oauth` (JSON token set + client registration), `profile:{id}:token`.
Profile tokens are cached in memory at startup to avoid a keychain hit per request.

## Endpoint & security

- Bind `127.0.0.1` only. Reject requests whose `Host` isn't `127.0.0.1:PORT` /
  `localhost:PORT`, and whose `Origin` (if present) isn't a localhost origin.
- `Authorization: Bearer <profile token>` required; constant-time compare.
  Tokens are 32 random bytes, base64url, rotatable from the UI.
- `/oauth/callback` is unauthenticated but only accepts a pending `state`.

## Remote access (tunnel)

- `tunnel.rs` runs `cloudflared tunnel --no-autoupdate --protocol http2 --url http://127.0.0.1:PORT` (quick
  tunnel, no account). HTTP/2 because quick tunnels default to QUIC on UDP 7844 with no fallback. The status turns
  `running` on "Registered tunnel connection", not when the URL is first printed. The tunnel follows the server
  across port changes, and `settings.tunnel_enabled` restores it at launch.
- A Host guard on the whole router: loopback `Host` without `cf-ray` is local; the tunnel's hostname is remote and
  limited to `/mcp/*`, `/auth/*`, `/.well-known/*` and `/health`; anything else is 403. rmcp's own Host allowlist
  is disabled in favour of this.
- Bearer clients use the profile token over the tunnel unchanged.
- Web connectors (claude.ai, ChatGPT) use OAuth 2.1 (`remote.rs`). Each profile is an authorization server with
  issuer `{base}/auth/{slug}`, so the profile is known without relying on the `resource` parameter. 401s carry
  `WWW-Authenticate: Bearer resource_metadata=…`; metadata is served per RFC 9728 and 8414 (plus OIDC and
  path-appended aliases). Dynamic client registration accepts https, loopback http or app-scheme redirect URIs and
  keeps at most 100 clients without grants. The auth code flow requires S256 PKCE. Consent goes through
  `ApprovalBroker::request_grant`: an always-on-top popup with a 4-digit code that the browser page also shows
  (the page polls with a meta refresh). It times out after 5 minutes, and at most 3 prompts can be pending.
- Tokens: 1 h access tokens in memory (a restart forces a refresh); 90-day refresh tokens stored as SHA-256 in
  `oauth_grants`, rotated and single use. Grants are listed and revocable per profile, and deleting a profile
  cascades to them.

## Proxy semantics

**Direct mode.** `tools/list` returns, for every enabled connection in the
profile, its cached tools renamed `{alias}__{tool}` minus `hidden` policies.
If the name exceeds 64 chars: `{alias}__{tool[..n]}_{hash6}`. A reverse map
resolves calls. Descriptions get a `[{connection name}]` prefix so the LLM can
tell `jira_acme` from `jira_client2`.

**Meta mode.** Exposes six tools:
`list_connections`, `search_tools(query, connection?, limit=10, include_schema?)`,
`describe_tool(name | names)` (returns input schemas),
`call_tool(name, arguments, connections?, fields?, max_chars?, reason?)`,
`call_tools(calls[], max_chars?, reason?)` and `connect(alias)`.
Search is BM25 over tokenized alias + connection name + tool name + description.

- *Fan-out*: `connections` is a list of aliases or `"*"`, and `name` may be the bare upstream
  tool name. `"*"` picks every connection with the same `template_id` as the named tool.
  The calls run concurrently, and each result comes back under a `## [n] name (status)` header.
- *Batch*: `call_tools` runs independent calls concurrently, at most 20 including fan-out.
- *Shaping* (`shape.rs`): `fields` are dotted paths with `[]` for arrays (`issues[].key`),
  applied to JSON text blocks. `max_chars` (default 50 000, 0 = off) is shared across results.
  When text is cut, a note says so.
- *Reason*: passed through to the approval popup.
- *connect*: restarts the upstream. On `needs_auth` it starts the OAuth flow, switching
  `auth: none` remotes to OAuth as the dashboard does, and opens the browser through the shell's
  URL opener. The URL is returned as well.

**Resources** (both modes): URIs rewritten to `plexus://{alias}/{urlencoded original}`;
`resources/read` reverses it.

**Calls.** Resolve → policy check (`approval` → `ApprovalBroker`, 60 s timeout →
error result) → ensure upstream running → forward → log metadata → return.
Upstream errors come back as `isError: true` tool results, not protocol errors.

**Change notifications.** When a connection's tool list changes (upstream
`list_changed`, edit, enable/disable, profile membership), every live session of
affected profiles gets `notifications/tools/list_changed`.

## Upstream lifecycle

States: `idle` → `starting` → `running` → (`idle` after timeout) / `error(msg)` /
`needs_auth`. Tool lists are cached in SQLite so `tools/list` doesn't spawn
processes; a connection with no cache is started for discovery (parallel, 30 s
timeout). Crashes mark `error` with the last ~20 stderr lines; next call retries.

macOS GUI apps don't inherit the shell `PATH`, so at startup Plexus resolves it
via `$SHELL -ilc 'printf %s "$PATH"'` and uses it for stdio spawns (`npx`, `uvx`).

## OAuth (remote MCP)

Per MCP authorization spec: on 401 read `WWW-Authenticate` → protected resource
metadata (RFC 9728) → auth server metadata (RFC 8414) → dynamic client
registration (RFC 7591) → auth code + PKCE in the **system browser**, redirect
`http://127.0.0.1:7420/oauth/callback`. Tokens + client registration live in the
keychain per connection, so two Atlassian connections are fully independent.
Refresh on expiry/401; on refresh failure state becomes `needs_auth` (tray +
UI show a "Reconnect" action). Built on `rmcp`'s `auth` feature with a
keychain-backed credential store where it fits.

## Catalog

- `templates/*.json`: id, name, icon, description, transport variants (e.g.
  Jira via Atlassian remote OAuth **or** `uvx mcp-atlassian` with API token),
  fields with `secret: bool`, defaults, alias suggestion.
  v1 set: Jira/Confluence, Sentry, GitHub, Linear, Slack, Notion, Filesystem, Postgres.
- Registry: `GET https://registry.modelcontextprotocol.io/v0/servers?search=…`,
  mapping `packages` (npm→`npx -y`, pypi→`uvx`, oci→`docker run -i`) and `remotes`
  to the same template shape. Cached 24 h.
- Custom: raw command/args/env or URL/headers.

## Client auto-config

Every write: read → JSON merge (only our `plexus-{profile}` key) → back up to
`*.plexus.bak` → write. Show the exact change before applying.

| Client | Mechanism |
|---|---|
| Claude Code | `claude mcp add --scope user --transport http plexus-{slug} URL --header "Authorization: Bearer …"`; fallback edit `~/.claude.json` |
| Cursor | `~/.cursor/mcp.json` → `mcpServers.plexus-{slug} = {url, headers}` |
| VS Code | `~/Library/Application Support/Code/User/mcp.json` → `servers.plexus-{slug} = {type:"http", url, headers}` |
| Claude Desktop | config file only supports stdio → `npx -y mcp-remote URL --header "Authorization: Bearer …"` (requires Node; UI says so) |

## UI

Tray menu: status line (`Plexus · 7420 · 6 connections`), connections with
status dots and "Reconnect" for `needs_auth`, profiles (copy URL), Open
Dashboard, Pause/Resume server, Quit. Closing the dashboard hides it.

Dashboard (SvelteKit SPA): **Connections** (list, status, add from catalog /
registry / custom, detail: config, tools, test call), **Profiles** (members,
mode, per-tool policy toggles, token reveal/rotate, install-to-client buttons),
**Activity** (live call log, filters), **Settings** (port, autostart, idle
timeout, retention). Approval popup is a separate small always-on-top window.

## Milestones

1. **Scaffold** — workspace, Tauri 2 + SvelteKit + Tailwind, tray-only app,
   single-instance, autostart, hide-on-close.
2. **Core proxy** — Store, Secrets, stdio + static HTTP upstreams, lifecycle,
   direct mode, axum endpoint with profiles + bearer + Host/Origin checks,
   integration tests (two instances of fake-mcp in one profile).
3. **Dashboard** — connections CRUD (custom), profiles, health in UI + tray,
   activity log.
4. **Meta mode, resources, policies, approvals.**
5. **OAuth remotes.**
6. **Catalog templates + registry browser.**
7. **Client auto-config.**
8. **Packaging** — `.dmg`, signing/notarization notes, app icon.

## Out of scope for v1

Prompts proxying, sampling/elicitation passthrough, multi-user access,
stderr log viewer, payload logging, Windows/Linux packaging.

## Pinned versions

Rust 1.99, `tauri` 2.12 (3.x is alpha), `rmcp` 3.7, `axum` 0.8, `rusqlite` 0.40
(bundled), `keyring` 4.2, `oauth2` 5; Node 26, pnpm 11, SvelteKit 3.1,
Svelte 5.57, Tailwind 4.3.

## Implementation notes (v1 as built)

- `connections.transport` is a JSON column (`{kind: stdio|http, ...}`) rather than one column per field.
- Curated templates live in `ui/src/lib/catalog.ts`; registry search runs from the UI
  (`ui/src/lib/registry.ts`, CSP allows `registry.modelcontextprotocol.io`). Remotes without declared
  headers start with no auth; a 401 marks the connection `needs_auth` and the UI offers "Sign in", which
  switches it to OAuth.
- The hub endpoint speaks both MCP lifecycles rmcp 3.7 supports: legacy sessions (list_changed pushed via
  the session peer) and 2026-07-28 stateless (`listen` subscriptions).
- Upstreams are connected with the legacy `initialize` handshake for the widest compatibility.
- `.dmg` is unsigned; for distribution add a Developer ID certificate and notarization
  (`APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` for `tauri build`).
