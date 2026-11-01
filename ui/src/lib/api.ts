import { invoke } from '@tauri-apps/api/core';

export type Transport =
  | {
      kind: 'stdio';
      command: string;
      args: string[];
      env: Record<string, string>;
      secret_env: string[];
    }
  | {
      kind: 'http';
      url: string;
      auth: 'none' | 'bearer' | 'oauth';
      headers: Record<string, string>;
      secret_headers: string[];
    };

export type ConnectionStatus =
  | { state: 'disabled' | 'idle' | 'starting' | 'running' | 'needs_auth' }
  | { state: 'error'; message: string };

export interface Connection {
  id: string;
  alias: string;
  name: string;
  template_id: string | null;
  transport: Transport;
  enabled: boolean;
  idle_timeout_secs: number;
  created_at: number;
  status: ConnectionStatus;
  tool_count: number;
  secrets_present: string[];
}

export interface ConnectionInput {
  alias: string;
  name: string;
  template_id: string | null;
  transport: Transport;
  enabled: boolean;
  idle_timeout_secs?: number | null;
  secrets: Record<string, string>;
}

export interface ToolInfo {
  name: string;
  exposed_name: string;
  title: string | null;
  description: string | null;
  read_only: boolean | null;
  destructive: boolean | null;
}

export type ProfileMode = 'direct' | 'meta';
export type ToolPolicy = 'allow' | 'approval' | 'hidden';

export interface Profile {
  id: string;
  slug: string;
  name: string;
  mode: ProfileMode;
  connection_ids: string[];
  created_at: number;
  url: string;
  /** Through the tunnel, while it is up. */
  public_url: string | null;
}

export interface ProfileInput {
  slug: string;
  name: string;
  mode: ProfileMode;
  connection_ids: string[];
}

export interface ProfileTool {
  connection_id: string;
  connection_alias: string;
  connection_name: string;
  tool: string;
  exposed_name: string;
  description: string | null;
  policy: ToolPolicy;
}

export interface CallLogEntry {
  id: number;
  ts: number;
  profile_id: string;
  connection_id: string;
  tool: string;
  duration_ms: number;
  status: 'ok' | 'error' | 'denied' | 'timeout';
  error: string | null;
}

export interface Settings {
  port: number;
  default_idle_timeout_secs: number;
  log_retention_days: number;
  approval_timeout_secs: number;
  tunnel_enabled: boolean;
}

export type TunnelStatus =
  | { state: 'off' | 'starting' }
  | { state: 'running'; url: string }
  | { state: 'error'; message: string };

export interface RemoteGrant {
  id: string;
  profile_id: string;
  client_name: string;
  redirect_uri: string;
  created_at: number;
  last_used_at: number;
}

export interface ServerStatus {
  running: boolean;
  port: number;
  error: string | null;
}

export interface ApprovalRequest {
  id: string;
  profile_name: string;
  connection_name: string;
  connection_alias: string;
  tool: string;
  arguments: unknown;
  reason: string | null;
}

export interface GrantRequest {
  id: string;
  profile_name: string;
  client_name: string;
  redirect_host: string;
  code: string;
  timeout_secs: number;
}

export type Prompt = ({ kind: 'call' } & ApprovalRequest) | ({ kind: 'grant' } & GrantRequest);

export type ClientKind = 'claude-code' | 'claude-desktop' | 'cursor' | 'vscode';

export type ApprovalDecision = 'allow_once' | 'always_allow' | 'deny';

export type HubEvent =
  | { type: 'connection_status'; connection_id: string; status: ConnectionStatus }
  | { type: 'capabilities_changed'; connection_id: string }
  | { type: 'connections_changed' }
  | { type: 'profiles_changed' }
  | { type: 'call_logged'; entry: CallLogEntry }
  | { type: 'tunnel_status'; status: TunnelStatus }
  | { type: 'grants_changed'; profile_id: string };

export const api = {
  listConnections: () => invoke<Connection[]>('list_connections'),
  createConnection: (input: ConnectionInput) => invoke<Connection>('create_connection', { input }),
  updateConnection: (id: string, input: ConnectionInput) =>
    invoke<Connection>('update_connection', { id, input }),
  deleteConnection: (id: string) => invoke<void>('delete_connection', { id }),
  testConnection: (id: string) => invoke<number>('test_connection', { id }),
  stopConnection: (id: string) => invoke<void>('stop_connection', { id }),
  connectionTools: (id: string) => invoke<ToolInfo[]>('connection_tools', { id }),

  listProfiles: () => invoke<Profile[]>('list_profiles'),
  createProfile: (input: ProfileInput) => invoke<Profile>('create_profile', { input }),
  updateProfile: (id: string, input: ProfileInput) => invoke<Profile>('update_profile', { id, input }),
  deleteProfile: (id: string) => invoke<void>('delete_profile', { id }),
  profileToken: (id: string) => invoke<string>('profile_token', { id }),
  rotateProfileToken: (id: string) => invoke<string>('rotate_profile_token', { id }),
  profileTools: (id: string) => invoke<ProfileTool[]>('profile_tools', { id }),
  setToolPolicy: (profileId: string, connectionId: string, tool: string, policy: ToolPolicy) =>
    invoke<void>('set_tool_policy', { profileId, connectionId, tool, policy }),

  recentCalls: (limit = 200, beforeId?: number) =>
    invoke<CallLogEntry[]>('recent_calls', { limit, beforeId }),
  getSettings: () => invoke<Settings>('get_settings'),
  saveSettings: (settings: Settings) => invoke<void>('save_settings', { settings }),
  serverStatus: () => invoke<ServerStatus>('server_status'),

  getApproval: (id: string) => invoke<Prompt | null>('get_approval', { id }),
  resolveApproval: (id: string, decision: ApprovalDecision) =>
    invoke<void>('resolve_approval', { id, decision }),

  startOauth: (id: string) => invoke<void>('start_oauth', { id }),
  signOut: (id: string) => invoke<void>('sign_out', { id }),
  installClient: (profileId: string, client: ClientKind) =>
    invoke<string>('install_client', { profileId, client }),

  tunnelStatus: () => invoke<TunnelStatus>('tunnel_status'),
  setTunnel: (enabled: boolean) => invoke<void>('set_tunnel', { enabled }),
  remoteGrants: (profileId: string) => invoke<RemoteGrant[]>('remote_grants', { profileId }),
  revokeGrant: (id: string) => invoke<void>('revoke_grant', { id }),
};

export function toInput(c: Connection): ConnectionInput {
  return {
    alias: c.alias,
    name: c.name,
    template_id: c.template_id,
    transport: c.transport,
    enabled: c.enabled,
    idle_timeout_secs: c.idle_timeout_secs,
    secrets: {},
  };
}

export function profileInput(p: Profile): ProfileInput {
  return { slug: p.slug, name: p.name, mode: p.mode, connection_ids: [...p.connection_ids] };
}
